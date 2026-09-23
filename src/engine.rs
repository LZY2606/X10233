//! Cycle-slip engine.
//!
//! Detectors are deliberately independent; no single detector is treated as
//! truth:
//!   * TD       - time-differenced carrier vs Doppler prediction (per band)
//!   * GF       - geometry-free / ionospheric combination L4 (m)
//!   * MW       - Melbourne-Wubbena wide lane (WL cycles)
//!   * LLI      - receiver loss-of-lock indicator
//!   * CLOCK    - cross-satellite consistency + imported clock events
//! A joint integer hypothesis is scored by how many phase detectors support
//! it; ties are preserved as first-class candidates. Repairs are stored only
//! as an offset layer (never rewriting observations).

use crate::model::*;
use crate::parser;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const C_LIGHT: f64 = 299_792_458.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetParams {
    pub td_tol_cycles: f64,
    pub gf_tol_m: f64,
    pub mw_tol_wl_cycles: f64,
    pub max_gap_s: f64,
    pub search_radius_cycles: i64,
}

impl Default for DetParams {
    fn default() -> Self {
        DetParams {
            td_tol_cycles: 0.25,
            gf_tol_m: 0.05,
            mw_tol_wl_cycles: 0.5,
            max_gap_s: 60.0,
            search_radius_cycles: 7,
        }
    }
}

/// Effective observation at one epoch for one satellite, with batch override
/// already applied. Missing frequencies simply lack the corresponding keys.
#[derive(Debug, Clone, Default)]
pub struct EffRow {
    pub epoch_id: String,
    pub offset_s: f64,
    pub ord: i64,
    pub batch_id: String,
    pub tokens: BTreeMap<String, RawToken>,
}

#[derive(Debug, Clone)]
pub struct EffInput {
    pub epochs: Vec<EpochRec>,
    pub freqs: Vec<FrequencyDef>,
    /// (satellite) -> ordered effective rows
    pub rows: BTreeMap<String, Vec<EffRow>>,
    pub clock_events: Vec<ClockEventRec>,
    /// effective batch per epoch (the batch that supplies the epoch block)
}

#[derive(Debug, Clone)]
pub struct ArcSeg {
    pub satellite: String,
    pub band: String,
    pub arc_seq: i64,
    pub start: usize,
    pub end: usize, // inclusive indices into effective rows for that sat
    pub split_reasons: Vec<String>,
}

/// One recomputation result for an event (import / parameter revision).
pub struct RecomputeOutcome {
    pub arcs: Vec<ProducedNode>,
    pub boundaries: Vec<ProducedNode>,
}

/// Band number for an observation key: the digit of the frequency band,
/// e.g. L1C/P1/D1/S1 -> 1, L2W/P2/D2/S2 -> 2. LLI1 -> 1.
pub fn key_band(key: &str) -> Option<u8> {
    let bytes = key.as_bytes();
    // keys like L1C, P1, D1, S1, LLI1
    if let Some(rest) = key.strip_prefix("LLI") {
        return rest.chars().next()?.to_digit(10).map(|d| d as u8);
    }
    if bytes.is_empty() {
        return None;
    }
    bytes[1..]
        .chars()
        .next()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c as u8 - b'0')
}

pub fn key_kind(key: &str) -> &'static str {
    if key.starts_with("LLI") {
        "lli"
    } else if key.starts_with('L') {
        "phase"
    } else if key.starts_with('P') {
        "code"
    } else if key.starts_with('D') {
        "doppler"
    } else if key.starts_with('S') {
        "snr"
    } else if key == "AZ" {
        "az"
    } else if key == "EL" {
        "el"
    } else if key == "CLOCK_BIAS" {
        "clock_bias"
    } else {
        "other"
    }
}

pub fn freq_for_band<'a>(freqs: &'a [FrequencyDef], system: &str, band: u8) -> Option<&'a FrequencyDef> {
    let band_s = band.to_string();
    freqs
        .iter()
        .find(|f| f.system == system && f.band == band_s)
}

/// Build effective input from raw rows. `batches_in_order` is the chain from
/// root to newest; a later batch row for the same (epoch, sat) replaces token
/// values from earlier batches (per-key override; absent keys inherit).
pub fn build_effective(
    project_id: &str,
    epochs_all: &[(EpochRec, String)], // epoch + batch
    freqs: &[FrequencyDef],
    rows: &[SatObs],
    clock_events: &[ClockEventRec],
    batches_in_order: &[String],
) -> EffInput {
    // epoch union
    let mut epochs: Vec<EpochRec> = Vec::new();
    let mut seen = BTreeSet::new();
    for (e, _b) in epochs_all {
        if seen.insert(e.epoch_id.clone()) {
            epochs.push(e.clone());
        }
    }
    epochs.sort_by_key(|e| (e.offset_s.to_bits(), e.ord));

    // index rows by (epoch_id, sat)
    let mut by_key: BTreeMap<(String, String), Vec<&SatObs>> = BTreeMap::new();
    for r in rows {
        by_key.entry((r.epoch_id.clone(), r.satellite.clone())).or_default().push(r);
    }

    // clock events: latest batch wins for same epoch+kind
    let mut clock_by_key: BTreeMap<(String, String), &ClockEventRec> = BTreeMap::new();
    for ce in clock_events {
        clock_by_key.insert((ce.epoch_id.clone(), ce.kind.clone()), ce);
    }

    let mut sats: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        sats.insert(r.satellite.clone());
    }

    let mut eff_rows: BTreeMap<String, Vec<EffRow>> = BTreeMap::new();
    for sat in &sats {
        let mut seq = Vec::new();
        for ep in &epochs {
            let mut chosen: Option<&SatObs> = None;
            if let Some(cands) = by_key.get(&(ep.epoch_id.clone(), sat.clone())) {
                for b in batches_in_order.iter().rev() {
                    if let Some(r) = cands.iter().find(|r| r.batch_id == *b) {
                        chosen = Some(r);
                        break;
                    }
                }
                if chosen.is_none() {
                    chosen = cands.first().copied();
                }
            }
            let Some(r) = chosen else { continue };
            let mut tokens = BTreeMap::new();
            // inherit earlier batch tokens, then overlay chosen batch tokens
            if let Some(cands) = by_key.get(&(ep.epoch_id.clone(), sat.clone())) {
                for b in batches_in_order {
                    if let Some(pr) = cands.iter().find(|r| r.batch_id == *b) {
                        for t in &pr.tokens {
                            tokens.insert(t.key.clone(), t.clone());
                        }
                    }
                }
            }
            seq.push(EffRow {
                epoch_id: ep.epoch_id.clone(),
                offset_s: ep.offset_s,
                ord: ep.ord,
                batch_id: r.batch_id.clone(),
                tokens,
            });
        }
        eff_rows.insert(sat.clone(), seq);
    }

    let _ = project_id;
    EffInput {
        epochs,
        freqs: freqs.to_vec(),
        rows: eff_rows,
        clock_events: clock_by_key.values().copied().cloned().collect(),
    }
}

/// Find phase-lock arcs per (satellite, band). Arcs split on time gaps beyond
/// `max_gap_s` and on missing band observations. Slip candidates live at the
/// boundaries between consecutive observations *within* a band presence run;
/// a missing-band interval creates two arcs with a `missing-frequency` split.
pub fn segment_arcs(input: &EffInput, params: &DetParams) -> BTreeMap<(String, u8), Vec<ArcSeg>> {
    let mut out: BTreeMap<(String, u8), Vec<ArcSeg>> = BTreeMap::new();
    for (sat, seq) in &input.rows {
        let system: String = sat.chars().next().map(|c| c.to_string()).unwrap_or_else(|| "G".into());
        // discover bands present from tokens
        let mut bands: BTreeSet<u8> = BTreeSet::new();
        for row in seq {
            for k in row.tokens.keys() {
                if key_kind(k) == "phase" {
                    if let Some(b) = key_band(k) {
                        bands.insert(b);
                    }
                }
            }
        }
        for band in bands {
            let phase_key = format!("L{}{}", band, phase_code_suffix(input, &system, band));
            let present: Vec<usize> = seq
                .iter()
                .enumerate()
                .filter(|(_, r)| r.tokens.contains_key(&phase_key) || has_phase_key(r, band))
                .map(|(i, _)| i)
                .collect();
            let mut segs = Vec::new();
            let mut arc_seq = 0i64;
            let mut start: Option<usize> = None;
            let mut last: Option<usize> = None;
            let mut reasons: Vec<String> = Vec::new();
            for &idx in &present {
                match (start, last) {
                    (None, _) => {
                        start = Some(idx);
                        last = Some(idx);
                        reasons = vec!["track-start".into()];
                    }
                    (Some(s), Some(l)) => {
                        let dt = seq[idx].offset_s - seq[l].offset_s;
                        let contiguous = idx == l + 1;
                        if dt > params.max_gap_s {
                            segs.push(ArcSeg {
                                satellite: sat.clone(),
                                band: format!("{}", band),
                                arc_seq,
                                start: s,
                                end: l,
                                split_reasons: std::mem::take(&mut reasons),
                            });
                            arc_seq += 1;
                            start = Some(idx);
                            reasons = vec![format!("time-gap {:.1}[s] exceeds {:.1}[s]", dt, params.max_gap_s)];
                        } else if !contiguous {
                            // missing band rows between l and idx
                            segs.push(ArcSeg {
                                satellite: sat.clone(),
                                band: format!("{}", band),
                                arc_seq,
                                start: s,
                                end: l,
                                split_reasons: std::mem::take(&mut reasons),
                            });
                            arc_seq += 1;
                            start = Some(idx);
                            reasons = vec!["missing-frequency".into()];
                        }
                        last = Some(idx);
                    }
                }
            }
            if let (Some(s), Some(l)) = (start, last) {
                segs.push(ArcSeg {
                    satellite: sat.clone(),
                    band: format!("{}", band),
                    arc_seq,
                    start: s,
                    end: l,
                    split_reasons: reasons,
                });
            }
            out.insert((sat.clone(), band), segs);
        }
    }
    out
}

fn has_phase_key(row: &EffRow, band: u8) -> bool {
    row.tokens.keys().any(|k| key_kind(k) == "phase" && key_band(k) == Some(band))
}

fn phase_code_suffix(input: &EffInput, system: &str, band: u8) -> String {
    // The sample key is L1C / L2W; derive suffix from the declared obs code.
    if let Some(f) = freq_for_band(&input.freqs, system, band) {
        // code L1C -> suffix after band digit is "C"; code L2W -> "W"
        let mut ch = f.code.chars();
        ch.next(); // L
        ch.next(); // 1
        ch.as_str().to_string()
    } else {
        String::new()
    }
}

pub fn phase_key(freqs: &[FrequencyDef], system: &str, band: u8) -> String {
    if let Some(f) = freq_for_band(freqs, system, band) {
        // frequency band label L1 + code suffix
        let suffix: String = f.code.chars().skip(2).collect();
        format!("L{}{}", band, suffix)
    } else {
        format!("L{}", band)
    }
}

fn tok_val(row: &EffRow, key: &str) -> Option<f64> {
    row.tokens.get(key).and_then(|t| t.value)
}

fn lli_val(row: &EffRow, band: u8) -> Option<u8> {
    let key = format!("LLI{}", band);
    row.tokens.get(&key).and_then(|t| t.value).map(|v| v as u8)
}

/// Evaluate every detector at every adjacent observation pair for every
/// satellite/band, and emit arc + boundary nodes. Idempotent: identical input
/// produces identical node ids.
pub fn recompute(
    project_id: &str,
    input: &EffInput,
    params: &DetParams,
    param_fp: &str,
    producing_event: &str,
) -> RecomputeOutcome {
    let arcs_map = segment_arcs(input, params);
    let mut arc_nodes = Vec::new();
    let mut boundary_nodes = Vec::new();

    // clock event lookup by epoch
    let clock_at: BTreeMap<String, &ClockEventRec> =
        input.clock_events.iter().map(|c| (c.epoch_id.clone(), c)).collect();

    for ((sat, band), segs) in &arcs_map {
        let system: String = sat.chars().next().map(|c| c.to_string()).unwrap_or_else(|| "G".into());
        let seq = &input.rows[sat];
        let f1 = freq_for_band(&input.freqs, &system, *band).cloned();
        let pk = phase_key(&input.freqs, &system, *band);
        let ck = format!("C{}", band); // not used directly
        let _ = ck;
        for seg in segs {
            // gather stats
            let mut elev = Vec::new();
            let mut mw: Vec<f64> = Vec::new();
            for i in seg.start..=seg.end {
                if let Some(el) = tok_val(&seq[i], "EL") {
                    elev.push(el);
                }
                if let Some(v) = mw_value(input, seq, i, &system) {
                    mw.push(v);
                }
            }
            let mean_el = if elev.is_empty() {
                None
            } else {
                Some(elev.iter().sum::<f64>() / elev.len() as f64)
            };
            let mw_bias = if mw.len() >= 2 {
                Some(mw.iter().sum::<f64>() / mw.len() as f64)
            } else {
                None
            };
            let body = ArcBody {
                project_id: project_id.into(),
                satellite: sat.clone(),
                band: band.to_string(),
                arc_seq: seg.arc_seq,
                start_epoch_id: seq[seg.start].epoch_id.clone(),
                end_epoch_id: seq[seg.end].epoch_id.clone(),
                start_offset_s: seq[seg.start].offset_s,
                end_offset_s: seq[seg.end].offset_s,
                n_epochs: seg.end - seg.start + 1,
                split_reasons: seg.split_reasons.clone(),
                mean_elevation_deg: mean_el,
                mw_bias_wl_cycles: mw_bias,
            };
            let sources = direct_sources(seq, seg.start, seg.end, &[pk.clone()], param_fp, "arc");
            let body_j = serde_json::to_value(&body).unwrap();
            let id = sid("arc", &[
                project_id, sat, &band.to_string(), &seg.arc_seq.to_string(),
                param_fp, &canon_json(&body_j),
            ]);
            arc_nodes.push(ProducedNode {
                id,
                slot: arc_slot(project_id, sat, *band, seg.arc_seq),
                kind: "arc".into(),
                producing_event: producing_event.into(),
                param_fp: param_fp.into(),
                direct_sources: sources,
                body: body_j,
            });
        }

        // boundaries: adjacent present pairs for this band
        let present: Vec<usize> = (0..seq.len())
            .filter(|&i| tok_val(&seq[i], &pk).is_some())
            .collect();
        for w in present.windows(2) {
            let (li, ri) = (w[0], w[1]);
            let left = &seq[li];
            let right = &seq[ri];
            let dt = right.offset_s - left.offset_s;
            let mut detectors: Vec<DetectorHit> = Vec::new();

            // --- TD for this band ---
            let d_pred = match (tok_val(left, &format!("D{}", band)), tok_val(right, &format!("D{}", band))) {
                (Some(dl), Some(dr)) => Some(0.5 * (dl + dr) * dt),
                _ => None,
            };
            let (l_v, l_p) = (tok_val(left, &pk), tok_val(right, &pk));
            if let (Some(lv), Some(rv), Some(pred)) = (l_v, l_p, d_pred) {
                let residual = rv - lv - pred;
                let verdict = if residual.abs() <= params.td_tol_cycles {
                    "clean"
                } else {
                    "candidate"
                };
                detectors.push(DetectorHit {
                    detector: format!("TD-B{}", band),
                    residual,
                    residual_unit: "cyc".into(),
                    threshold: params.td_tol_cycles,
                    verdict: verdict.into(),
                    note: Some(format!(
                        "phase delta {:.4}[cyc] vs Doppler prediction {:.4f}[cyc] over {:.1}[s]",
                        rv - lv,
                        pred,
                        dt
                    )),
                });
            }

            // --- geometry-free ionospheric combination L4 (only at pair) ---
            if let Some(gf) = gf_residual(input, &system, left, right) {
                let verdict = if gf.abs() <= params.gf_tol_m { "clean" } else { "candidate" };
                detectors.push(DetectorHit {
                    detector: "GF-L4".into(),
                    residual: gf,
                    residual_unit: "m".into(),
                    threshold: params.gf_tol_m,
                    verdict: verdict.into(),
                    note: Some("geometry-free L1-L2 change; detects a-b patterns incl. (1,1)".into()),
                });
            }

            // --- Melbourne-Wubbena ---
            if let Some(mw) = mw_delta(input, &system, seq, li, ri) {
                let verdict = if mw.abs() <= params.mw_tol_wl_cycles {
                    "clean"
                } else {
                    "candidate"
                };
                detectors.push(DetectorHit {
                    detector: "MW-WL".into(),
                    residual: mw,
                    residual_unit: "wl_cyc".into(),
                    threshold: params.mw_tol_wl_cycles,
                    verdict: verdict.into(),
                    note: Some("wide-lane phase vs narrow-lane code; insensitive to (k,k) slips".into()),
                });
            }

            // --- LLI flag at right epoch ---
            let mut lli_flags = Vec::new();
            if let Some(f) = lli_val(right, *band) {
                if f != 0 {
                    lli_flags.push(format!("B{}=LLI{}", band, f));
                    detectors.push(DetectorHit {
                        detector: format!("LLI-B{}", band),
                        residual: f as f64,
                        residual_unit: "flag".into(),
                        threshold: 0.0,
                        verdict: "flagged".into(),
                        note: Some("receiver loss-of-lock indicator; may be a false alarm".into()),
                    });
                }
            }

            // --- clock: imported event wins, otherwise consistency check ---
            let clock_finding = if let Some(ce) = clock_at.get(&right.epoch_id) {
                Some(format!(
                    "clock-event:{}({:+.7}[s], source={})",
                    ce.kind, ce.delta_s, ce.source
                ))
            } else {
                clock_consistency(input, &system, sat, li, ri, dt, params)
            };
            if let Some(cf) = &clock_finding {
                detectors.push(DetectorHit {
                    detector: "CLOCK".into(),
                    residual: if cf.starts_with("clock-event") { 1.0 } else { 0.0 },
                    residual_unit: "event".into(),
                    threshold: 0.5,
                    verdict: if cf.starts_with("clock-event") { "clock-jump" } else { "clean" },
                    note: Some(cf.clone()),
                });
            }

            let candidates = enumerate_candidates(&detectors, params, input, &system, left, right, dt);

            // Arc seq containing the right endpoint
            let arc_seq_here = segs
                .iter()
                .find(|s| right_offset_in(s, ri, seq))
                .map(|s| s.arc_seq)
                .or_else(|| segs.first().map(|s| s.arc_seq))
                .unwrap_or(0);

            let body = BoundaryBody {
                project_id: project_id.into(),
                satellite: sat.clone(),
                band: band.to_string(),
                left_epoch_id: left.epoch_id.clone(),
                right_epoch_id: right.epoch_id.clone(),
                left_offset_s: left.offset_s,
                right_offset_s: right.offset_s,
                dt_s: dt,
                detectors,
                candidates,
                clock_finding,
                lli_flags,
            };
            let body_j = serde_json::to_value(&body).unwrap();
            let slot = boundary_slot(project_id, sat, *band, &left.epoch_id, &right.epoch_id);
            let sources = direct_sources_pair(left, right, param_fp, &system, input, *band);
            let id = sid("bnd", &[&slot, param_fp, &canon_json(&body_j)]);
            boundary_nodes.push(ProducedNode {
                id,
                slot,
                kind: "boundary".into(),
                producing_event: producing_event.into(),
                param_fp: param_fp.into(),
                direct_sources: sources,
                body: body_j,
            });
            let _ = arc_seq_here;
        }
    }
    RecomputeOutcome { arcs: arc_nodes, boundaries: boundary_nodes }
}

fn right_offset_in(s: &ArcSeg, ri: usize, seq: &[EffRow]) -> bool {
    ri >= s.start && ri <= s.end && seq.get(ri).is_some()
}

fn canon_json(v: &serde_json::Value) -> String {
    // serde_json preserves BTreeMap key order; float formatting is the only
    // concern, but inputs here are deterministic re-parsed values.
    serde_json::to_string(v).unwrap()
}

fn direct_sources(
    seq: &[EffRow],
    start: usize,
    end: usize,
    keys: &[String],
    param_fp: &str,
    kind: &str,
) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for i in start..=end {
        for k in keys {
            if let Some(t) = seq[i].tokens.get(k) {
                out.insert(format!("raw:{}:{}:{}", seq[i].epoch_id, k, t.raw_value));
            }
        }
    }
    out.insert(format!("param:{}", param_fp));
    out.insert(format!("algo:{}", ALGO_VERSION));
    let _ = kind;
    out.into_iter().collect()
}

fn direct_sources_pair(
    left: &EffRow,
    right: &EffRow,
    param_fp: &str,
    system: &str,
    input: &EffInput,
    band: u8,
) -> Vec<String> {
    let mut keys = vec![
        phase_key(&input.freqs, system, band),
        format!("P{}", band),
        format!("D{}", band),
        format!("LLI{}", band),
    ];
    if band == 1 {
        // GF and MW also use band 2
        keys.push(phase_key(&input.freqs, system, 2));
        keys.push("P2".into());
    }
    let mut out: BTreeSet<String> = BTreeSet::new();
    for row in [left, right] {
        for k in &keys {
            if let Some(t) = row.tokens.get(k) {
                out.insert(format!("raw:{}:{}:{}", row.epoch_id, k, t.raw_value));
            }
        }
    }
    if let Some(f) = freq_for_band(&input.freqs, system, band) {
        out.insert(format!("freq:{}", f.freq_hz));
    }
    out.insert(format!("param:{}", param_fp));
    out.insert(format!("algo:{}", ALGO_VERSION));
    out.into_iter().collect()
}

pub fn arc_slot(project_id: &str, sat: &str, band: u8, arc_seq: i64) -> String {
    sid("slot-arc", &[project_id, sat, &band.to_string(), &arc_seq.to_string()])
}

pub fn boundary_slot(project_id: &str, sat: &str, band: u8, left: &str, right: &str) -> String {
    sid("slot-bnd", &[project_id, sat, &band.to_string(), left, right])
}

const F1: f64 = 1575.42e6;
const F2: f64 = 1227.60e6;

fn gf_value(l1_m: f64, l2_m: f64) -> f64 {
    l1_m - l2_m
}

fn gf_residual(input: &EffInput, system: &str, left: &EffRow, right: &EffRow) -> Option<f64> {
    let p1 = phase_key(input.freqs.as_slice(), system, 1);
    let p2 = phase_key(input.freqs.as_slice(), system, 2);
    let f1 = freq_for_band(input.freqs.as_slice(), system, 1)?;
    let f2 = freq_for_band(input.freqs.as_slice(), system, 2)?;
    let l1l = parser::cycles_to_meters(tok_val(left, &p1)?, f1.freq_hz);
    let l2l = parser::cycles_to_meters(tok_val(left, &p2)?, f2.freq_hz);
    let l1r = parser::cycles_to_meters(tok_val(right, &p1)?, f1.freq_hz);
    let l2r = parser::cycles_to_meters(tok_val(right, &p2)?, f2.freq_hz);
    Some(gf_value(l1r, l2r) - gf_value(l1l, l2l))
}

fn mw_value(input: &EffInput, seq: &[EffRow], i: usize, system: &str) -> Option<f64> {
    let row = &seq[i];
    let p1k = phase_key(input.freqs.as_slice(), system, 1);
    let p2k = phase_key(input.freqs.as_slice(), system, 2);
    let f1 = freq_for_band(input.freqs.as_slice(), system, 1)?;
    let f2 = freq_for_band(input.freqs.as_slice(), system, 2)?;
    let l1m = parser::cycles_to_meters(tok_val(row, &p1k)?, f1.freq_hz);
    let l2m = parser::cycles_to_meters(tok_val(row, &p2k)?, f2.freq_hz);
    let p1 = tok_val(row, "P1")?;
    let p2 = tok_val(row, "P2")?;
    let wl = C_LIGHT / (f1.freq_hz - f2.freq_hz);
    let wl_phase = (f1.freq_hz * l1m - f2.freq_hz * l2m) / (f1.freq_hz - f2.freq_hz);
    let nl_code = (f1.freq_hz * p1 + f2.freq_hz * p2) / (f1.freq_hz + f2.freq_hz);
    Some((wl_phase - nl_code) / wl)
}

fn mw_delta(input: &EffInput, system: &str, seq: &[EffRow], li: usize, ri: usize) -> Option<f64> {
    let a = mw_value(input, seq, li, system)?;
    let b = mw_value(input, seq, ri, system)?;
    Some(b - a)
}

/// Cross-satellite clock-jump consistency. A receiver clock jump appears as a
/// common code+phase jump on *all* tracked satellites at the same epoch; a
/// single-satellite slip does not. Returns `Some(reason)` when suspicious.
fn clock_consistency(
    input: &EffInput,
    system: &str,
    sat: &str,
    li: usize,
    ri: usize,
    dt: f64,
    params: &DetParams,
) -> Option<String> {
    let p1k = phase_key(input.freqs.as_slice(), system, 1);
    let right_t = input.rows[sat].get(ri)?.offset_s;
    let mut big = 0usize;
    let mut total = 0usize;
    let mut sample_jump_m: Option<f64> = None;
    for (s2, seq2) in &input.rows {
        let pos = seq2.iter().position(|r| (r.offset_s - right_t).abs() < 1e-9);
        let Some(r_i) = pos else { continue };
        if r_i == 0 { continue; }
        let l = &seq2[r_i - 1];
        let r = &seq2[r_i];
        let (Some(lp), Some(rp)) = (tok_val(l, &p1k), tok_val(r, &p1k)) else { continue };
        let d = match (tok_val(l, "D1"), tok_val(r, "D1")) {
            (Some(dl), Some(dr)) => Some(0.5 * (dl + dr) * dt),
            _ => None,
        };
        let Some(pred) = d else { continue };
        total += 1;
        let jump_cyc = rp - lp - pred;
        let jump_m = parser::cycles_to_meters(jump_cyc, F1);
        let tol_m = parser::cycles_to_meters(params.td_tol_cycles, F1);
        if jump_m.abs() > tol_m.max(50.0) {
            big += 1;
            if sample_jump_m.is_none() {
                sample_jump_m = Some(jump_m);
            }
        }
    }
    if total >= 2 && big == total {
        Some(format!(
            "clock-consistency: {}/{} satellites show a common jump ({:+.3}[m]); treated as receiver clock jump, not per-satellite slip",
            big, total, sample_jump_m.unwrap_or(0.0)
        ))
    } else if big > 0 {
        Some(format!(
            "clock-consistency: {}/{} satellites jump; single-satellite slip not explained by clock",
            big, total
        ))
    } else {
        None
    }
}

fn verdict_map(detectors: &[DetectorHit]) -> BTreeMap<String, &DetectorHit> {
    detectors.iter().map(|d| (d.detector.clone(), d)).collect()
}

/// Enumerate all legal joint integer hypotheses within the search radius, score
/// them by supporting phase detectors, and preserve exact ties. No detector is
/// treated as truth: `rejected_by` records which detectors rule a candidate out.
fn enumerate_candidates(
    detectors: &[DetectorHit],
    params: &DetParams,
    input: &EffInput,
    system: &str,
    left: &EffRow,
    right: &EffRow,
    _dt: f64,
) -> Vec<Candidate> {
    let vm = verdict_map(detectors);
    let f1 = freq_for_band(input.freqs.as_slice(), system, 1);
    let f2 = freq_for_band(input.freqs.as_slice(), system, 2);
    let have_b1 = f1.map(|f| tok_val(right, &phase_key(input.freqs.as_slice(), system, 1)).is_some()).unwrap_or(false);
    let have_b2 = f2.map(|f| tok_val(right, &phase_key(input.freqs.as_slice(), system, 2)).is_some()).unwrap_or(false);

    let clock_dominates = vm
        .get("CLOCK")
        .map(|d| d.verdict == "clock-jump")
        .unwrap_or(false);

    // predicted detector residuals implied by a hypothesis (a,b)
    let td1_obs = vm.get("TD-B1").map(|d| d.residual);
    let td2_obs = vm.get("TD-B2").map(|d| d.residual);
    let gf_obs = vm.get("GF-L4").map(|d| d.residual);
    let mw_obs = vm.get("MW-WL").map(|d| d.residual);

    let mut raw: Vec<(i64, i64, i64, Vec<String>, Vec<String>)> = Vec::new();
    let r = params.search_radius_cycles;

    let mut check = |a: i64, b: i64, out: &mut Vec<(i64, i64, i64, Vec<String>, Vec<String>)>| {
        let mut supported: Vec<String> = Vec::new();
        let mut rejected: Vec<String> = Vec::new();
        // TD band 1
        if let Some(obs) = td1_obs {
            if (obs - a as f64).abs() <= params.td_tol_cycles {
                supported.push("TD-B1".into());
            } else {
                rejected.push("TD-B1".into());
            }
        }
        // TD band 2
        if let Some(obs) = td2_obs {
            if (obs - b as f64).abs() <= params.td_tol_cycles {
                supported.push("TD-B2".into());
            } else {
                rejected.push("TD-B2".into());
            }
        }
        // GF L4 in meters: lambda1*a - lambda2*b
        if let (Some(obs), Some(f1), Some(f2)) = (gf_obs, f1, f2) {
            let l1 = C_LIGHT / f1.freq_hz;
            let l2 = C_LIGHT / f2.freq_hz;
            let pred = l1 * a as f64 - l2 * b as f64;
            if (obs - pred).abs() <= params.gf_tol_m {
                supported.push("GF-L4".into());
            } else {
                rejected.push("GF-L4".into());
            }
        }
        // MW wide-lane cycles: a - b (code glitch may disagree)
        if let Some(obs) = mw_obs {
            let pred = a as f64 - b as f64;
            if (obs - pred).abs() <= params.mw_tol_wl_cycles {
                supported.push("MW-WL".into());
            } else {
                rejected.push("MW-WL".into());
            }
        }
        if clock_dominates {
            rejected.push("CLOCK:classifies-boundary-as-receiver-clock-jump".into());
        }
        let score = supported.len() as i64;
        out.push((a, b, score, supported, rejected));
    };

    if have_b1 && have_b2 {
        for a in -r..=r {
            for b in -r..=r {
                check(a, b, &mut raw);
            }
        }
    } else if have_b1 {
        for a in -r..=r {
            check(a, 0, &mut raw);
        }
    } else if have_b2 {
        for b in -r..=r {
            check(0, b, &mut raw);
        }
    }

    // keep only hypotheses with at least one support and no phase-detector
    // contradiction *among present detectors*; when detectors disagree, retain
    // every hypothesis so the ambiguity is explicit.
    let n_phase_det = [td1_obs.is_some(), td2_obs.is_some(), gf_obs.is_some(), mw_obs.is_some()]
        .iter()
        .filter(|x| **x)
        .count() as i64;

    let mut kept: Vec<_> = raw
        .into_iter()
        .filter(|(_, _, score, _, rej)| {
            *score >= 1 && !rej.iter().any(|r| r.starts_with("CLOCK:"))
        })
        .collect();

    if kept.is_empty() && !clock_dominates && n_phase_det > 0 {
        // Detectors are mutually inconsistent (e.g. code glitch). Keep all
        // hypotheses that satisfy at least one detector so nothing is hidden.
        // Re-enumerate for that case.
        let mut all: Vec<(i64, i64, i64, Vec<String>, Vec<String>)> = Vec::new();
        if have_b1 && have_b2 {
            for a in -r..=r {
                for b in -r..=r {
                    check(a, b, &mut all);
                }
            }
        } else if have_b1 {
            for a in -r..=r {
                check(a, 0, &mut all);
            }
        }
        kept = all.into_iter().filter(|(_, _, s, _, _)| *s >= 1).collect();
    }

    let max_score = kept.iter().map(|(_, _, s, _, _)| *s).max().unwrap_or(0);
    kept.sort_by(|x, y| {
        y.2.cmp(&x.2)
            .then_with(|| x.0.cmp(&y.0))
            .then_with(|| x.1.cmp(&y.1))
    });
    kept.into_iter()
        .enumerate()
        .map(|(i, (a, b, score, supported, rejected))| {
            let id = sid("cand", &[
                &left.epoch_id,
                &right.epoch_id,
                &a.to_string(),
                &b.to_string(),
                &i.to_string(),
            ]);
            Candidate {
                id,
                offset_b1_cycles: a,
                offset_b2_cycles: b,
                score,
                tied: score == max_score && score > 0,
                supported_by: supported,
                rejected_by: rejected,
            }
        })
        .collect()
}
