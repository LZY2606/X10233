//! Cycle-slip detection state machine.
//!
//! Three detectors run independently and their opinions are NOT a vote and no
//! single detector is treated as truth:
//!   * GF  geometry-free (ionosphere) phase combination,
//!   * MW  Melbourne-Wubbena widelane combination,
//!   * TD  time-difference of Doppler-predicted carrier per band.
//! Receiver clock jumps are a separate COMMON_MODE finding. Repair never
//! rewrites phase: accepted hypotheses become repair-offset rows only.

use crate::anyhow_lite::{err, Result};
use crate::model::*;
use crate::replay::{EpochInput, RawSlot, ReplayInput};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const C_F1: f64 = 1_575_420_000.0;
pub const C_F2: f64 = 1_227_600_000.0;
pub const W1: f64 = SPEED_OF_LIGHT_M_S / C_F1;
pub const W2: f64 = SPEED_OF_LIGHT_M_S / C_F2;

#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum ViewStatus {
    Ok,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct NormValue {
    pub quantity: Quantity,
    pub obs_id: String,
    pub code: String,
    pub band: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct SatEpochMeasurements {
    pub sat: String,
    pub epoch_seq: i64,
    pub epoch: String,
    pub by_code: BTreeMap<String, NormValue>,
    pub lli_codes: BTreeSet<String>,
    pub raw_codes: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct Proposal {
    pub n1: i64,
    pub n2: i64,
    pub detectors: BTreeMap<String, f64>, // detector -> score in [0,1]
}

#[derive(Debug, Clone)]
pub struct CandidateSet {
    pub sat: String,
    pub epoch_seq: i64,
    pub epoch: String,
    pub proposals: Vec<Proposal>,
    pub detector_views: BTreeMap<String, Value>,
    pub lli_flags: Vec<Value>,
    pub natural_id_input: String,
}

#[derive(Debug, Clone)]
pub struct PairView {
    pub detector: String,
    pub view: Value,
    pub source_obs_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ArcRecord {
    pub sat: String,
    pub code: String,
    pub start_seq: i64,
    pub end_seq: i64,
    pub start_epoch: String,
    pub end_epoch: String,
    pub end_reason: String,
}

#[derive(Debug, Clone)]
pub struct ClockFinding {
    pub epoch_seq: i64,
    pub epoch: String,
    pub jump_s: f64,
    pub affected_sats: Vec<String>,
    pub evidence: Value,
    pub source_event_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub project_id: String,
    pub rule_id: String,
    pub rule_seq: i64,
    pub rule_fingerprint: String,
    pub params: DetectorParams,
    pub norms: Vec<NormValue>,
    pub clock_findings: Vec<ClockFinding>,
    pub arcs: Vec<ArcRecord>,
    pub pair_views: Vec<PairViewRecord>,
    pub candidates: Vec<CandidateSet>,
    pub unknowns: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct PairViewRecord {
    pub sat: String,
    pub seq_a: i64,
    pub seq_b: i64,
    pub dt_s: f64,
    pub gap_flag: String,
    pub view: PairView,
}

fn normalize_slot(slot: &RawSlot) -> Option<NormValue> {
    // Keep the exact RINEX text in the raw layer; only derived rows carry SI.
    let v: f64 = slot.raw_value.parse().ok()?;
    let quantity = match slot.kind.as_str() {
        "pseudorange" => Quantity { value: v, unit: "m".into(), datum: None },
        "carrier" => Quantity { value: v, unit: "cycle".into(), datum: None },
        "doppler" => Quantity { value: v, unit: "Hz".into(), datum: None },
        "ssi" => Quantity { value: v, unit: "dBHz".into(), datum: None },
        _ => return None,
    };
    Some(NormValue {
        quantity,
        obs_id: slot.obs_id.clone(),
        code: slot.code.clone(),
        band: slot.band.clone(),
        kind: slot.kind.clone(),
    })
}

fn epoch_seconds(epoch: &str) -> Result<f64> {
    // Accepts ...THH:MM:SS.ffffffZ
    if epoch.len() < 20 || &epoch[10..11] != "T" {
        return Err(err("bad epoch format"));
    }
    let h: f64 = epoch[11..13].parse().map_err(|_| err("bad hour"))?;
    let m: f64 = epoch[14..16].parse().map_err(|_| err("bad minute"))?;
    let sec: f64 = epoch[17..epoch.len() - 1].parse().map_err(|_| err("bad second"))?;
    let date = days_from_civil(
        epoch[0..4].parse().map_err(|_| err("bad year"))?,
        epoch[5..7].parse().map_err(|_| err("bad month"))?,
        epoch[8..10].parse().map_err(|_| err("bad day"))?,
    );
    Ok((date as f64) * 86_400.0 + h * 3600.0 + m * 60.0 + sec)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let doy = (153 * (m as i64 + (if m > 2 { -3 } else { 9 })) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe - 719468
}

/// Build a snapshot from the inputs visible at a point in history. This is a
/// pure function of (raw material, rule version, accepted decisions), which is
/// what makes time travel and export/re-import exact.
pub fn compute_snapshot(input: &ReplayInput, accepted: &[AcceptedOffset]) -> Result<Snapshot> {
    let rule = input
        .rules
        .last()
        .ok_or_else(|| err("cannot compute without a rule version"))?;
    let params = rule.params.clone();

    let mut unknowns: Vec<Value> = Vec::new();
    let mut norms: Vec<NormValue> = Vec::new();

    // per epoch -> sat -> measurements
    let mut sat_epochs: BTreeMap<i64, BTreeMap<String, SatEpochMeasurements>> = BTreeMap::new();
    for e in &input.epochs {
        for slot in &e.slots {
            let me = sat_epochs.entry(e.seq).or_default();
            let m = me.entry(slot.sat.clone()).or_insert_with(|| SatEpochMeasurements {
                sat: slot.sat.clone(),
                epoch_seq: e.seq,
                epoch: e.epoch.clone(),
                by_code: BTreeMap::new(),
                lli_codes: BTreeSet::new(),
                raw_codes: BTreeSet::new(),
            });
            m.raw_codes.insert(slot.code.clone());
            if slot.lli.unwrap_or(0) & 1 != 0 {
                m.lli_codes.insert(slot.code.clone());
            }
            if let Some(diag) = &slot.diagnostic_code {
                unknowns.push(json!({
                    "scope": "normalization",
                    "sat": slot.sat,
                    "epoch_seq": e.seq,
                    "code": slot.code,
                    "code_detail": slot.diagnostic_message,
                    "code_name": diag,
                    "raw_value": slot.raw_value,
                    "status": "unknown",
                }));
                continue;
            }
            if let Some(n) = normalize_slot(slot) {
                m.by_code.insert(n.code.clone(), n.clone());
                norms.push(n);
            }
        }
    }

    let epoch_time: BTreeMap<i64, f64> = input
        .epochs
        .iter()
        .map(|e| (e.seq, epoch_seconds(&e.epoch).unwrap_or(f64::NAN)))
        .collect();

    // Receiver clock findings first; they annotate pairs but do not suppress
    // the per-satellite detectors' records.
    let mut clock_findings = analyze_clock(input, &sat_epochs, &epoch_time, &params);

    let sats: BTreeSet<String> = sat_epochs
        .values()
        .flat_map(|m| m.keys().cloned())
        .collect();

    let mut arcs: Vec<ArcRecord> = Vec::new();
    let mut pair_views: Vec<PairViewRecord> = Vec::new();
    let mut candidates: Vec<CandidateSet> = Vec::new();

    for sat in &sats {
        let seqs: Vec<i64> = sat_epochs
            .iter()
            .filter_map(|(seq, m)| if m.contains_key(sat) { Some(*seq) } else { None })
            .collect();

        for pair in seqs.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let ta = epoch_time[&a];
            let tb = epoch_time[&b];
            let dt = tb - ta;
            if !dt.is_finite() || dt <= 0.0 {
                unknowns.push(json!({"scope":"time","sat":sat,"epoch_seq":b,"status":"unknown",
                    "reason":"NON_MONOTONIC_OR_UNPARSEABLE_TIME"}));
                continue;
            }
            let gap_flag = if dt > params.max_gap_s { "LONG_GAP" } else { "REGULAR" };
            let ma = &sat_epochs[&a][sat];
            let mb = &sat_epochs[&b][sat];

            // Independent detector views, each may be unknown on its own.
            run_td(&params, ma, mb, dt, gap_flag, sat, &mut pair_views, &mut unknowns);
            run_gf(&params, ma, mb, dt, gap_flag, sat, &mut pair_views, &mut unknowns);
            run_mw(&params, ma, mb, dt, gap_flag, sat, &sat_epochs, &seqs, &mut pair_views, &mut unknowns);

            // Candidate synthesis at the later epoch.
            let pv: Vec<&PairViewRecord> = pair_views
                .iter()
                .filter(|p| p.sat == *sat && p.seq_b == b)
                .collect();
            if let Some(cs) = synthesize_candidate(&params, mb, &pv) {
                candidates.push(cs);
            }
        }

        // Arc state machine per obs code.
        for code in ["L1C", "L2W"] {
            arcs.extend(build_arcs(sat, code, &seqs, &sat_epochs, &epoch_time, &params, &clock_findings, &candidates));
        }
    }

    // Clock findings may be generated in epoch order already; sort for stable output.
    clock_findings.sort_by_key(|f| f.epoch_seq);

    Ok(Snapshot {
        project_id: input.project_id.clone(),
        rule_id: rule.id.clone(),
        rule_seq: rule.seq,
        rule_fingerprint: rule.fingerprint.clone(),
        params,
        norms,
        clock_findings,
        arcs,
        pair_views,
        candidates,
        unknowns,
    })
}

pub struct AcceptedOffset {
    pub candidate_natural: String,
    pub proposal: Value, // {"n1":..,"n2":..}
}

fn meas<'a>(m: &'a SatEpochMeasurements, code: &str) -> Option<&'a NormValue> {
    m.by_code.get(code)
}

fn q(m: &NormValue) -> f64 {
    m.quantity.value
}
fn ids(m: &SatEpochMeasurements, codes: &[&str]) -> Vec<String> {
    codes.iter().filter_map(|c| m.by_code.get(*c).map(|n| n.obs_id.clone())).collect()
}

/// TD: predicted carrier increment from averaged Doppler; residual converted
/// to cycles independently per band.
fn run_td(
    params: &DetectorParams,
    ma: &SatEpochMeasurements,
    mb: &SatEpochMeasurements,
    dt: f64,
    gap_flag: &str,
    sat: &str,
    out: &mut Vec<PairViewRecord>,
    unknowns: &mut Vec<Value>,
) {
    for (carrier, doppler, wavelength, detector, band) in [
        ("L1C", "D1C", W1, "TD_L1", "1"),
        ("L2W", "D2W", W2, "TD_L2", "2"),
    ] {
        let push_unknown = |unknowns: &mut Vec<Value>, reason: &str| {
            unknowns.push(json!({
                "scope":"detector","detector":detector,"sat":sat,
                "epoch_seq":mb.epoch_seq,"status":"unknown","reason":reason
            }));
        };
        let (la, lb, da, db) = match (meas(ma, carrier), meas(mb, carrier), meas(ma, doppler), meas(mb, doppler)) {
            (Some(la), Some(lb), Some(da), Some(db)) => (la, lb, da, db),
            _ => {
                push_unknown(unknowns, "MISSING_FREQUENCY");
                continue;
            }
        };
        let predicted_cycles = -(q(da) + q(db)) * 0.5 * dt;
        let delta_cycles = q(lb) - q(la);
        let residual_m = (delta_cycles - predicted_cycles) * wavelength;
        let nearest = (residual_m / wavelength).round() as i64;
        let score = if residual_m.abs() <= params.td_residual_limit_m {
            1.0
        } else {
            let rem_cycles = residual_m / wavelength - nearest as f64;
            (1.0 - (rem_cycles.abs() * wavelength / params.td_residual_limit_m)).max(0.0)
        };
        let view = json!({
            "status": "ok",
            "detector": detector,
            "sat": sat,
            "band": band,
            "wavelength_m": {"value": wavelength, "unit": "m/cycle"},
            "doppler_hz_a": {"value": q(da), "unit": "Hz"},
            "doppler_hz_b": {"value": q(db), "unit": "Hz"},
            "delta_carrier_cycle": {"value": delta_cycles, "unit": "cycle"},
            "predicted_carrier_cycle": {"value": predicted_cycles, "unit": "cycle"},
            "residual_m": {"value": residual_m, "unit": "m"},
            "integer_offset_cycle": {"value": nearest, "unit": "cycle"},
            "score": score,
        });
        out.push(PairViewRecord {
            sat: sat.to_string(),
            seq_a: ma.epoch_seq,
            seq_b: mb.epoch_seq,
            dt_s: dt,
            gap_flag: gap_flag.to_string(),
            view: PairView { detector: detector.to_string(), view, source_obs_ids: ids(ma, &[carrier, doppler]).into_iter().chain(ids(mb, &[carrier, doppler])).collect() },
        });
    }
}

/// GF geometry-free ionosphere combination L1*w1 - L2*w2 (metres).
fn run_gf(
    params: &DetectorParams,
    ma: &SatEpochMeasurements,
    mb: &SatEpochMeasurements,
    dt: f64,
    gap_flag: &str,
    sat: &str,
    out: &mut Vec<PairViewRecord>,
    unknowns: &mut Vec<Value>,
) {
    let need = |m: &SatEpochMeasurements| -> Option<(f64, f64, Vec<String>)> {
        let l1 = meas(m, "L1C")?;
        let l2 = meas(m, "L2W")?;
        Some((q(l1) * W1 - q(l2) * W2, 0.0, ids(m, &["L1C", "L2W"])))
    };
    let ((gfa, _, ida), (gfb, _, idb)) = match (need(ma), need(mb)) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            unknowns.push(json!({
                "scope":"detector","detector":"GF","sat":sat,"epoch_seq":mb.epoch_seq,
                "status":"unknown","reason":"MISSING_FREQUENCY"
            }));
            return;
        }
    };
    let delta_gf = gfb - gfa;
    let rate = delta_gf / dt;
    // Residual after the best-fit integer family n1*w1 - n2*w2.
    let family = best_gf_family(delta_gf, params.max_cycles);
    let view = json!({
        "status":"ok","detector":"GF","sat":sat,
        "gf_m_a":{"value":gfa,"unit":"m"},
        "gf_m_b":{"value":gfb,"unit":"m"},
        "delta_gf_m":{"value":delta_gf,"unit":"m"},
        "rate_m_per_s":{"value":rate,"unit":"m/s"},
        "rate_limit_m_per_s":{"value":params.gf_rate_limit_m_per_s,"unit":"m/s"},
        "rate_exceeded": rate.abs() > params.gf_rate_limit_m_per_s,
        "best_family": family,
    });
    out.push(PairViewRecord {
        sat: sat.to_string(), seq_a: ma.epoch_seq, seq_b: mb.epoch_seq, dt_s: dt,
        gap_flag: gap_flag.to_string(),
        view: PairView { detector: "GF".into(), view, source_obs_ids: ida.into_iter().chain(idb).collect() },
    });
}

/// All integer pairs with |n|<=max whose geometry-free signature matches
/// `delta_gf` within one TD tolerance band. Equal-score alternatives are all
/// returned ("多个等分整周偏移" case).
fn best_gf_family(delta_gf: f64, max_cycles: i64) -> Value {
    let tol = 0.12;
    let mut matches: Vec<Value> = Vec::new();
    let mut best_resid = f64::INFINITY;
    let mut best: Option<(i64, i64)> = None;
    for n1 in -max_cycles..=max_cycles {
        // n2 derived from the equality family; evaluate several nearby ints.
        let n2_f = (n1 as f64 * W1 - delta_gf) / W2;
        for n2 in [n2_f.floor() as i64, n2_f.round() as i64, n2_f.ceil() as i64] {
            if n2.abs() > max_cycles {
                continue;
            }
            let resid = (n1 as f64 * W1 - n2 as f64 * W2 - delta_gf).abs();
            if resid < best_resid {
                best_resid = resid;
                best = Some((n1, n2));
            }
            if resid <= tol && !matches.iter().any(|v| v["n1"] == n1 && v["n2"] == n2) {
                matches.push(json!({"n1":n1,"n2":n2,"residual_m":{"value":resid,"unit":"m"}}));
            }
        }
    }
    matches.sort_by_key(|v| (v["n1"].as_i64().unwrap_or(0), v["n2"].as_i64().unwrap_or(0)));
    json!({
        "best_n1": best.map(|b| b.0),
        "best_n2": best.map(|b| b.1),
        "best_residual_m": {"value": best_resid, "unit":"m"},
        "matching_families": matches,
    })
}

/// MW widelane: N_w = (f1*L1 - f2*L2)/(f1-f2) - (f1*C1 + f2*C2)/(c) cycles.
fn run_mw(
    params: &DetectorParams,
    ma: &SatEpochMeasurements,
    mb: &SatEpochMeasurements,
    _dt: f64,
    gap_flag: &str,
    sat: &str,
    all: &BTreeMap<i64, BTreeMap<String, SatEpochMeasurements>>,
    seqs: &[i64],
    out: &mut Vec<PairViewRecord>,
    unknowns: &mut Vec<Value>,
) {
    let nw = |m: &SatEpochMeasurements| -> Option<(f64, Vec<String>)> {
        let l1 = meas(m, "L1C")?;
        let l2 = meas(m, "L2W")?;
        let c1 = meas(m, "C1C")?;
        let c2 = meas(m, "C2W")?;
        // Widelane phase in widelane cycles and narrow-lane code in metres;
        // N_w = phase_wl - code_nl / lambda_w, units: cycle.
        let phase_wl_cycles = (C_F1 * q(l1) - C_F2 * q(l2)) / (C_F1 - C_F2);
        let code_nl_m = (C_F1 * q(c1) + C_F2 * q(c2)) / (C_F1 + C_F2);
        let lambda_w = SPEED_OF_LIGHT_M_S / (C_F1 - C_F2);
        let n_w = phase_wl_cycles - code_nl_m / lambda_w;
        Some((n_w, ids(m, &["L1C", "L2W", "C1C", "C2W"])))
    };
    let (nwa, ida) = match nw(ma) {
        Some(v) => v,
        None => {
            unknowns.push(json!({"scope":"detector","detector":"MW","sat":sat,"epoch_seq":mb.epoch_seq,
                "status":"unknown","reason":"MISSING_FREQUENCY"}));
            return;
        }
    };
    let (nwb, idb) = match nw(mb) {
        Some(v) => v,
        None => {
            unknowns.push(json!({"scope":"detector","detector":"MW","sat":sat,"epoch_seq":mb.epoch_seq,
                "status":"unknown","reason":"MISSING_FREQUENCY"}));
            return;
        }
    };
    let _ = nwa; // prior mean computed below from the arc history
    let pos_b = seqs.iter().position(|s| *s == mb.epoch_seq).unwrap_or(0);
    let mut hist: Vec<f64> = Vec::new();
    for s in &seqs[..pos_b] {
        if let Some(mm) = all.get(s).and_then(|e| e.get(sat)) {
            if let Some((v, _)) = nw(mm) {
                hist.push(v);
            }
        }
    }
    if hist.len() < params.min_mw_history {
        unknowns.push(json!({"scope":"detector","detector":"MW","sat":sat,"epoch_seq":mb.epoch_seq,
            "status":"unknown","reason":"INSUFFICIENT_BASELINE",
            "history_length":hist.len(),"required":params.min_mw_history}));
        return;
    }
    let mean: f64 = hist.iter().sum::<f64>() / hist.len() as f64;
    let jump = nwb - mean;
    let nearest = jump.round() as i64;
    let score = if jump.abs() <= params.mw_limit_cycles {
        1.0
    } else {
        let frac = (jump - nearest as f64).abs();
        (1.0 - frac / params.mw_limit_cycles).max(0.0)
    };
    let view = json!({
        "status":"ok","detector":"MW","sat":sat,
        "mw_ambiguity_cycle_b":{"value":nwb,"unit":"cycle"},
        "baseline_mean_cycle":{"value":mean,"unit":"cycle"},
        "history_length":hist.len(),
        "jump_cycle":{"value":jump,"unit":"cycle"},
        "limit_cycle":{"value":params.mw_limit_cycles,"unit":"cycle"},
        "integer_offset_widelane_cycle":{"value":nearest,"unit":"cycle"},
        "score":score,
    });
    out.push(PairViewRecord {
        sat: sat.to_string(), seq_a: ma.epoch_seq, seq_b: mb.epoch_seq, dt: 0.0,
        gap_flag: gap_flag.to_string(),
        view: PairView { detector: "MW".into(), view, source_obs_ids: ida.into_iter().chain(idb).collect() },
    });
}

/// Receiver clock jumps are whole-receiver events. We honor the imported clock
/// event log and independently corroborate COMMON_MODE behavior across
/// satellites. They are never filed as per-satellite cycle-slip candidates.
fn analyze_clock(
    input: &ReplayInput,
    sat_epochs: &BTreeMap<i64, BTreeMap<String, SatEpochMeasurements>>,
    epoch_time: &BTreeMap<i64, f64>,
    params: &DetectorParams,
) -> Vec<ClockFinding> {
    let mut findings = Vec::new();
    for (raw_id, ev) in &input.clock_events {
        if ev.event != "CLOCK_JUMP" {
            continue;
        }
        // Find the epoch whose text matches.
        let epoch_input = input.epochs.iter().find(|e| e.epoch == ev.epoch);
        let Some(ei) = epoch_input else { continue };
        let Some(prev_seq) = input
            .epochs
            .iter()
            .rev()
            .find(|e| e.seq < ei.seq)
            .map(|e| e.seq)
        else {
            continue;
        };
        let dt = epoch_time[&ei.seq] - epoch_time[&prev_seq];
        let mut affected: Vec<String> = Vec::new();
        let mut medians: BTreeMap<String, Value> = BTreeMap::new();
        if let (Some(cur), Some(prev)) = (sat_epochs.get(&ei.seq), sat_epochs.get(&prev_seq)) {
            for sat in cur.keys() {
                let Some(mb) = cur.get(sat) else { continue };
                let Some(ma) = prev.get(sat) else { continue };
                let mut band_res = Vec::new();
                for (carrier, w) in [("L1C", W1), ("L2W", W2)] {
                    if let (Some(la), Some(lb)) = (meas(ma, carrier), meas(mb, carrier)) {
                        // Phase clock common-mode estimate: any residual here is
                        // dominated by clock at a microsecond jump (>299 m).
                        band_res.push(json!({"band":carrier,"phase_delta_cycle":{"value":q(lb)-q(la),"unit":"cycle"}}));
                    }
                }
                if !band_res.is_empty() {
                    affected.push(sat.clone());
                    medians.insert(sat.clone(), json!(band_res));
                }
            }
        }
        affected.sort();
        let jump_s = ev.jump_s.unwrap_or_default();
        if jump_s.abs() >= params.clock_jump_min_s {
            findings.push(ClockFinding {
                epoch_seq: ei.seq,
                epoch: ei.epoch.clone(),
                jump_s,
                affected_sats: affected.clone(),
                evidence: json!({
                    "mode":"COMMON_MODE",
                    "dt_s":{"value":dt,"unit":"s"},
                    "imported_event": ev,
                    "per_satellite": medians,
                    "common_across_satellites": affected.len() >= 2,
                }),
                source_event_ids: vec![raw_id.clone()],
            });
        }
    }
    findings
}

/// Merge independent detector opinions at one satellite/epoch into the full
/// set of legal hypotheses. When detectors disagree, nothing is collapsed:
/// every legal candidate survives with its per-detector score.
fn synthesize_candidate(
    params: &DetectorParams,
    mb: &SatEpochMeasurements,
    pv: &[&PairViewRecord],
) -> Option<CandidateSet> {
    let mut proposals: BTreeMap<(i64, i64), Proposal> = BTreeMap::new();
    let mut detector_views = BTreeMap::new();

    for p in pv {
        let det = p.view.detector.as_str();
        detector_views.insert(det.to_string(), p.view.view.clone());
    }

    let td1 = pv.iter().find(|p| p.view.detector == "TD_L1").and_then(|p| {
        p.view.view["integer_offset_cycle"]["value"].as_i64().map(|n| (n, p.view.view["score"].as_f64().unwrap_or(0.0)))
    });
    let td2 = pv.iter().find(|p| p.view.detector == "TD_L2").and_then(|p| {
        p.view.view["integer_offset_cycle"]["value"].as_i64().map(|n| (n, p.view.view["score"].as_f64().unwrap_or(0.0)))
    });

    let mut has_signal = false;

    // TD resolves both bands -> single joint hypothesis.
    if let (Some((n1, s1)), Some((n2, s2))) = (td1, td2) {
        has_signal = true;
        let entry = proposals.entry((n1, n2)).or_insert_with(|| Proposal { n1, n2, detectors: BTreeMap::new() });
        entry.detectors.insert("TD_L1".into(), s1);
        entry.detectors.insert("TD_L2".into(), s2);
    } else if let Some((n1, s1)) = td1 {
        // Only L1 available (L2 missing): TD gives n1, n2 unknown.
        has_signal = true;
        let entry = proposals.entry((n1, 0)).or_insert_with(|| Proposal { n1, n2: 0, detectors: BTreeMap::new() });
        entry.detectors.insert("TD_L1".into(), s1);
        entry.detectors.insert("TD_L2".into(), 0.0); // unknown band, kept explicit
    }

    // GF families: add every equally-scoring legal integer family.
    if let Some(gf) = pv.iter().find(|p| p.view.detector == "GF") {
        let families = gf.view.view["best_family"]["matching_families"].as_array();
        if let Some(families) = families {
            if !families.is_empty() {
                has_signal = true;
            }
            for f in families {
                let n1 = f["n1"].as_i64().unwrap_or(0);
                let n2 = f["n2"].as_i64().unwrap_or(0);
                let resid = f["residual_m"]["value"].as_f64().unwrap_or(1.0);
                let score = (1.0 - resid / 0.12).max(0.0);
                let entry = proposals.entry((n1, n2)).or_insert_with(|| Proposal { n1, n2, detectors: BTreeMap::new() });
                entry.detectors.insert("GF".into(), score);
                if let Some((_, s1)) = td1 {
                    entry.detectors.entry("TD_L1".to_string()).or_insert(if entry.n1 == td1.unwrap().0 { s1 } else { 0.0 });
                }
                if let Some((_, s2)) = td2 {
                    entry.detectors.entry("TD_L2".to_string()).or_insert(if entry.n2 == td2.unwrap().0 { s2 } else { 0.0 });
                }
            }
        }
    }

    // MW widelane integer jump: expressed as family-consistent n1,n2 options
    // nearest to what TD/GF already have. It is kept as an independent score.
    if let Some(mw) = pv.iter().find(|p| p.view.detector == "MW") {
        if let Some(jump) = mw.view.view["integer_offset_widelane_cycle"]["value"].as_i64() {
            has_signal = true;
            let score = mw.view.view["score"].as_f64().unwrap_or(0.0);
            // n1 - n2 == jump for the family entry.
            for prop in proposals.values_mut() {
                if prop.n1 - prop.n2 == jump {
                    prop.detectors.insert("MW".into(), score);
                } else {
                    prop.detectors.entry("MW".to_string()).or_insert(0.0);
                }
            }
        }
    }

    // LLI flags are indicators, not truth. Record them for review even when no
    // detector supports a slip (the LLI false-positive case).
    let lli_flags: Vec<Value> = mb
        .lli_codes
        .iter()
        .map(|c| json!({"code": c, "flag": "LLI_BIT0", "meaning": "possible slip indicator"}))
        .collect();

    let any_residual_signal = proposals.values().any(|p| {
        p.detectors.values().any(|v| *v < 0.999)
    }) || proposals
        .values()
        .any(|p| p.detectors.values().any(|v| *v > 0.0))
        && proposals.values().any(|p| p.detectors.values().any(|v| *v <= 1.0));

    let non_zero = proposals.keys().any(|(n1, n2)| *n1 != 0 || *n2 != 0);
    let _ = any_residual_signal;
    if !has_signal && lli_flags.is_empty() {
        return None;
    }
    if proposals.is_empty() && lli_flags.is_empty() {
        return None;
    }
    // Suppress the all-zero candidate unless an LLI flag demands review.
    let mut props: Vec<Proposal> = proposals.into_values().filter(|p| p.n1 != 0 || p.n2 != 0 || !lli_flags.is_empty()).collect();
    if !non_zero && lli_flags.is_empty() {
        props.clear();
    }
    if props.is_empty() && lli_flags.is_empty() {
        return None;
    }
    props.sort_by(|a, b| {
        let sa: f64 = a.detectors.values().sum::<f64>() / a.detectors.len().max(1) as f64;
        let sb: f64 = b.detectors.values().sum::<f64>() / b.detectors.len().max(1) as f64;
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            .then(a.n1.cmp(&b.n1)).then(a.n2.cmp(&b.n2))
    });

    let natural = natural_candidate_id(mb, &detector_views);
    Some(CandidateSet {
        sat: mb.sat.clone(),
        epoch_seq: mb.epoch_seq,
        epoch: mb.epoch.clone(),
        proposals: props,
        detector_views,
        lli_flags,
        natural_id_input: natural,
    })
}

fn natural_candidate_id(mb: &SatEpochMeasurements, views: &BTreeMap<String, Value>) -> String {
    let mut keys: Vec<String> = views.keys().cloned().collect();
    keys.sort();
    let mut parts = vec![mb.sat.clone(), mb.epoch.clone()];
    for k in keys {
        parts.push(k);
        // include only the residual-defining numbers, not scores/params
        let v = &views[&k];
        for key in ["residual_m", "delta_gf_m", "jump_cycle", "integer_offset_cycle", "integer_offset_widelane_cycle"] {
            if let Some(val) = v.get(key).and_then(|x| x.get("value")) {
                parts.push(format!("{key}={}", canonical_json(val)));
            }
        }
    }
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    short_id("cand", &refs)
}

/// Arc state machine for one satellite/obs-code stream.
#[allow(clippy::too_many_arguments)]
fn build_arcs(
    sat: &str,
    code: &str,
    seqs: &[i64],
    sat_epochs: &BTreeMap<i64, BTreeMap<String, SatEpochMeasurements>>,
    epoch_time: &BTreeMap<i64, f64>,
    params: &DetectorParams,
    clocks: &[ClockFinding],
    candidates: &[CandidateSet],
) -> Vec<ArcRecord> {
    let mut arcs = Vec::new();
    let mut start: Option<(i64, String)> = None;
    let mut prev: Option<i64> = None;
    let mut end_reason = "OPEN";

    for seq in seqs {
        let present = sat_epochs[seq].get(sat).map(|m| m.by_code.contains_key(code)).unwrap_or(false);
        let epoch = sat_epochs[seq].get(sat).map(|m| m.epoch.clone()).unwrap_or_default();
        if present {
            if start.is_none() {
                start = Some((*seq, epoch.clone()));
                end_reason = "OPEN";
            }
            if let Some(p) = prev {
                let dt = epoch_time[seq] - epoch_time[&p];
                let clock_here = clocks.iter().any(|c| c.epoch_seq == *seq);
                let slip_here = candidates
                    .iter()
                    .any(|c| c.sat == sat && c.epoch_seq == *seq && c.proposals.iter().any(|pr| pr.n1 != 0 || pr.n2 != 0));
                if clock_here {
                    arcs.push(close(sat, code, start.unwrap(), (p, prev_epoch(sat_epochs, sat, p)), "CLOCK_JUMP"));
                    start = Some((*seq, epoch.clone()));
                } else if slip_here {
                    arcs.push(close(sat, code, start.unwrap(), (p, prev_epoch(sat_epochs, sat, p)), "SLIP"));
                    start = Some((*seq, epoch.clone()));
                } else if dt > params.max_gap_s {
                    arcs.push(close(sat, code, start.unwrap(), (p, prev_epoch(sat_epochs, sat, p)), "GAP"));
                    start = Some((*seq, epoch.clone()));
                }
            }
            prev = Some(*seq);
        } else if start.is_some() {
            // frequency missing at this epoch closes the arc
            arcs.push(close(sat, code, start.unwrap(), (prev.unwrap(), prev_epoch(sat_epochs, sat, prev.unwrap())), "MISSING_FREQUENCY"));
            start = None;
            prev = None;
        }
    }
    if let Some(st) = start {
        let last = prev.unwrap();
        arcs.push(close(sat, code, st, (last, prev_epoch(sat_epochs, sat, last)), end_reason));
    }
    arcs
}

fn prev_epoch(m: &BTreeMap<i64, BTreeMap<String, SatEpochMeasurements>>, sat: &str, seq: i64) -> String {
    m.get(&seq).and_then(|e| e.get(sat)).map(|x| x.epoch.clone()).unwrap_or_default()
}

fn close(sat: &str, code: &str, start: (i64, String), end: (i64, String), reason: &str) -> ArcRecord {
    ArcRecord {
        sat: sat.to_string(),
        code: code.to_string(),
        start_seq: start.0,
        end_seq: end.0,
        start_epoch: start.1,
        end_epoch: end.1,
        end_reason: reason.to_string(),
    }
}
