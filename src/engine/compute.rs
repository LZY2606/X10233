//! Deterministic recompute of the full derived view at one viewpoint.

use crate::ids;
use crate::model::*;
use crate::params::{DetectorParams, ALG_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One normalized phase observation: the raw token lives in the import layer;
/// here `cycles` may be None if the source value could not be converted.
#[derive(Debug, Clone)]
pub struct PhasePoint {
    pub merged_id: String,
    pub natural_id: String,
    pub batch_id: String,
    pub epoch_raw: String,
    pub t: f64,
    pub code: String,
    pub band: String,
    pub sat: String,
    pub cycles: Option<f64>,
    pub range_m: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub snr_dbhz: Option<f64>,
    pub lli_bits: Option<i64>,
    pub raw_phase: String,
    pub raw_range: String,
    pub raw_unit_phase: String,
    pub raw_unit_range: String,
    /// Merged identities per observation code (direct-source provenance).
    pub phase_id: Option<String>,
    pub pseudo_id: Option<String>,
    pub doppler_id: Option<String>,
    pub snr_id: Option<String>,
    pub lli_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClockPoint {
    pub merged_id: String,
    pub natural_id: String,
    pub batch_id: String,
    pub epoch_raw: String,
    pub t: f64,
    pub delta_ns: Option<f64>,
    pub raw_delta: String,
    pub raw_unit: String,
}

#[derive(Debug, Clone)]
pub struct StatePoint {
    pub merged_id: String,
    pub natural_id: String,
    pub batch_id: String,
    pub epoch_raw: String,
    pub sat: String,
    pub t: f64,
    pub az_deg: Option<f64>,
    pub el_deg: Option<f64>,
}

/// Fully merged source view at the viewpoint's tip: per natural key the
/// latest committed batch wins; older batch rows remain in the store.
#[derive(Debug, Clone, Default)]
pub struct SourceView {
    pub project: String,
    pub marker: String,
    pub bands: Vec<BandDef>,
    pub points: Vec<PhasePoint>,
    pub clocks: Vec<ClockPoint>,
    pub states: Vec<StatePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DecisionAction {
    AcceptCandidate,
    RejectCandidate,
    ForceBreak,
    ParameterRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub event_id: String,
    pub parent_event_id: String,
    pub action: DecisionAction,
    pub candidate_id: Option<String>,
    pub sat: Option<String>,
    pub band: Option<String>,
    pub at_epoch: Option<String>,
    pub reason: String,
    /// Event clock (ms), purely informational/ordering.
    pub clock_ms: i64,
}

#[derive(Clone)]
struct Obs {
    merged_id: String,
    natural_id: String,
    batch_id: String,
    value: Option<f64>,
    raw: String,
    raw_unit: String,
}

#[derive(Clone)]
struct Pt {
    seq: i64,
    epoch_raw: String,
    t: f64,
    phase: Option<Obs>,
    pseudo: Option<Obs>,
    doppler: Option<Obs>,
    snr: Option<Obs>,
    lli: Option<Obs>,
}

#[derive(Clone, Default)]
pub(crate) struct EdgeCand {
    pub td: Vec<(i64, f64)>, // offset, residual cycles
    pub mw: Vec<(i64, f64)>, // wide-lane offset n, residual wl cycles
    pub gf: Vec<(i64, i64, f64)>, // a,b, residual metres
    pub lli: bool,
    pub clock_declared: bool,
    pub irregular: bool,
    pub missing_b: bool,
    pub dt: f64,
    pub epoch_raw: String,
    pub stat_ids: BTreeSet<String>,
    pub proposals: Vec<String>,
}

pub fn compute(
    viewpoint_event_id: &str,
    source: &SourceView,
    params: &DetectorParams,
    decisions: &[DecisionRecord],
) -> ViewState {
    let mut e = Engine {
        source,
        params,
        param_fp: params.fingerprint(),
        decisions,
        epochs: Vec::new(),
        statistics: Vec::new(),
        candidates: Vec::new(),
        offsets: Vec::new(),
        rejected: Vec::new(),
        diagnostics: Vec::new(),
    };
    e.run(viewpoint_event_id)
}

struct Engine<'a> {
    source: &'a SourceView,
    params: &'a DetectorParams,
    param_fp: String,
    decisions: &'a [DecisionRecord],
    epochs: Vec<EpochInfo>,
    statistics: Vec<EdgeStatistic>,
    candidates: Vec<CycleCandidate>,
    offsets: Vec<OffsetEntry>,
    rejected: Vec<RejectedCandidate>,
    diagnostics: Vec<String>,
}

impl<'a> Engine<'a> {
    fn run(&mut self, viewpoint: &str) -> ViewState {
        // ---- epoch grid -----------------------------------------------------
        let mut epoch_set: BTreeMap<i64, (String, f64)> = BTreeMap::new();
        for p in &self.source.points {
            epoch_set
                .entry((p.t * 1000.0).round() as i64)
                .or_insert_with(|| (p.epoch_raw.clone(), p.t));
        }
        for c in &self.source.clocks {
            epoch_set
                .entry((c.t * 1000.0).round() as i64)
                .or_insert_with(|| (c.epoch_raw.clone(), c.t));
        }
        let epochs_sorted: Vec<(String, f64)> =
            epoch_set.into_iter().map(|(_, v)| v).collect();

        let declared_clock: BTreeMap<i64, f64> = {
            let mut m = BTreeMap::new();
            for c in &self.source.clocks {
                if let Some(d) = c.delta_ns {
                    if d.abs() >= self.params.clock_event_threshold_ns {
                        m.insert((c.t * 1000.0).round() as i64, d);
                    }
                }
            }
            m
        };

        let mut prev_t: Option<f64> = None;
        let mut epoch_seq_of_key: BTreeMap<i64, i64> = BTreeMap::new();
        for (i, (raw, t)) in epochs_sorted.iter().enumerate() {
            let seq = i as i64 + 1;
            epoch_seq_of_key.insert((*t * 1000.0).round() as i64, seq);
            let dt = prev_t.map(|pt| t - pt);
            let irregular = matches!(dt, Some(d) if d > self.params.nominal_dt_secs * self.params.gap_factor);
            if irregular {
                self.diagnostics.push(format!(
                    "irregular_gap: epoch {raw} dt={:.3}s exceeds {:.1} x {:.1}s",
                    dt.unwrap_or(0.0),
                    self.params.gap_factor,
                    self.params.nominal_dt_secs
                ));
            }
            self.epochs.push(EpochInfo {
                seq,
                epoch_raw: raw.clone(),
                t_secs: *t,
                dt_secs: dt,
                irregular_gap: irregular,
                declared_clock_delta_ns: {
                    let k = (*t * 1000.0).round() as i64;
                    declared_clock.get(&k).copied()
                },
            });
            prev_t = Some(*t);
        }
        let epoch_at = |seq: i64| -> (String, f64) {
            epochs_sorted[(seq - 1) as usize].clone()
        };

        // ---- index observations into (sat, band) tracks --------------------
        let tkey = |t: f64| -> i64 { (t * 1000.0).round() as i64 };
        let mut tracks: BTreeMap<(String, String), BTreeMap<i64, Pt>> = BTreeMap::new();
        for p in &self.source.points {
            let key = tkey(p.t);
            let seq = *epoch_seq_of_key.get(&key).unwrap_or(&0);
            if seq == 0 {
                continue;
            }
            let entry = tracks
                .entry((p.sat.clone(), p.band.clone()))
                .or_default()
                .entry(seq)
                .or_insert_with(|| Pt {
                    seq,
                    epoch_raw: p.epoch_raw.clone(),
                    t: p.t,
                    phase: None,
                    pseudo: None,
                    doppler: None,
                    snr: None,
                    lli: None,
                });
            let obs = Obs {
                merged_id: p.merged_id.clone(),
                natural_id: p.natural_id.clone(),
                batch_id: p.batch_id.clone(),
                value: p.cycles,
                raw: p.raw_phase.clone(),
                raw_unit: p.raw_unit_phase.clone(),
            };
            match ObsKind::from_code(&p.code) {
                ObsKind::Phase => entry.phase = Some(obs),
                ObsKind::Pseudorange => entry.pseudo = Some(Obs {
                    merged_id: p.merged_id.clone(),
                    value: p.range_m,
                    raw: p.raw_range.clone(),
                    raw_unit: p.raw_unit_range.clone(),
                    ..obs
                }),
                ObsKind::Doppler => entry.doppler = Some(Obs {
                    value: p.doppler_hz,
                    ..obs
                }),
                ObsKind::Snr => entry.snr = Some(Obs {
                    value: p.snr_dbhz,
                    ..obs
                }),
                ObsKind::Lli => entry.lli = Some(Obs {
                    value: p.lli_bits.map(|b| b as f64),
                    ..obs
                }),
            }
        }
        // ---- wavelength table ---------------------------------------------
        let wl = |band: &str| -> Option<f64> {
            self.source
                .bands
                .iter()
                .find(|b| b.band == band)
                .and_then(|b| b.wavelength())
        };

        let mut lli_hits_run: Vec<(String, String, i64, String, f64)> = Vec::new();
        // ---- per-edge statistics & proposals -------------------------------
        // Keys: (sat,band,seq_of_later_epoch) -> EdgeCand
        let mut edges: BTreeMap<(String, String, i64), EdgeCand> = BTreeMap::new();
        for ((sat, band), pts) in &tracks {
            let seqs: Vec<i64> = pts.keys().copied().collect();
            for w in seqs.windows(2) {
                let (s0, s1) = (w[0], w[1]);
                let (p0, p1) = (pts.get(&s0).unwrap(), pts.get(&s1).unwrap());
                let dt = p1.t - p0.t;
                let (e1_raw, e1_t) = epoch_at(s1);
                let key = (sat.clone(), band.clone(), s1);
                let ec = edges.entry(key.clone()).or_default();
                ec.dt = dt;
                ec.epoch_raw = e1_raw.clone();
                let irregular = dt > self.params.nominal_dt_secs * self.params.gap_factor;
                ec.irregular = irregular;
                let clock_declared = declared_clock.contains_key(&tkey(e1_t));
                ec.clock_declared = clock_declared;

                // TD statistic
                let mut td_sources = Vec::new();
                if let Some(o0) = &p0.phase {
                    td_sources.push(o0.merged_id.clone());
                }
                if let Some(o1) = &p1.phase {
                    td_sources.push(o1.merged_id.clone());
                }
                let td_val = match (&p0.phase, &p1.phase) {
                    (Some(a), Some(b)) => match (a.value, b.value) {
                        (Some(x), Some(y)) => Some((y - x).abs_signed()),
                        _ => None,
                    },
                    _ => None,
                };
                let td_id = ids::statistic_id(
                    "TD", ALG_VERSION, &self.param_fp, sat, band, &td_sources,
                );
                let mut flag = EdgeFlag::Normal;
                if clock_declared {
                    flag = EdgeFlag::ClockJumpDeclared;
                } else if irregular {
                    flag = EdgeFlag::IrregularGap;
                }
                if let Some(v) = td_val {
                    self.statistics.push(EdgeStatistic {
                        id: td_id.clone(),
                        detector: "TD".to_string(),
                        alg_version: ALG_VERSION.to_string(),
                        param_fp: self.param_fp.clone(),
                        sat: sat.clone(),
                        band: band.clone(),
                        at_seq: s1,
                        at_epoch: e1_raw.clone(),
                        dt_secs: dt,
                        value: Some(v),
                        value_unit: U_CYCLE.to_string(),
                        flag,
                        sources: td_sources.clone(),
                        diagnostic: None,
                    });
                    ec.stat_ids.insert(td_id);
                }
                if let Some(v) = td_val {
                    let k = nearest_legal(v, self.params.td_tol_cycles);
                    if let Some(n) = k {
                        if !(clock_declared && !self.decisions_force_break(sat, band, &e1_raw))
                        {
                            ec.td.push((n, v - n as f64));
                            ec.proposals.push(format!(
                                "TD({band}) n={n} resid={:.4}cyc",
                                v - n as f64
                            ));
                        }
                    }
                }
                let _ = e1_raw;
            }
        }

        // GF / MW combinations and LLI flags are added in a second pass so
        // missing-frequency situations are explicit.
        self.combination_edges(&tracks, &epoch_at, wl, &mut edges, &mut lli_hits_run);
        for (sat, band, seq, src, t) in std::mem::take(&mut lli_hits_run) {
            let (raw, _) = epoch_at(seq);
            let ec = edges.entry((sat, band, seq)).or_insert_with(|| {
                let mut x = EdgeCand::default();
                x.epoch_raw = raw;
                let _ = t;
                x
            });
            ec.lli = true;
            ec.stat_ids.insert(src);
        }
        self.clock_statistics(&tracks, &epoch_at, &declared_clock, &mut edges);

        // ---- reconcile hypotheses per edge --------------------------------
        let hypotheses = crate::engine::reconcile::reconcile_edges(
            &edges,
            self.params,
            &self.param_fp,
            &self.source.project,
        );
        for h in hypotheses {
            self.candidates.push(h);
        }

        // ---- apply human decisions (accept/reject at this viewpoint) ------
        self.apply_decisions(viewpoint);

        // ---- arcs ----------------------------------------------------------
        let arcs = self.build_arcs(&tracks, &epoch_at, &declared_clock);
        ViewState {
            viewpoint_event_id: viewpoint.to_string(),
            params_version: self.param_fp.clone(),
            alg_fingerprint: ALG_VERSION.to_string(),
            epochs: self.epochs.clone(),
            arcs,
            statistics: std::mem::take(&mut self.statistics),
            candidates: std::mem::take(&mut self.candidates),
            offsets: std::mem::take(&mut self.offsets),
            rejected: std::mem::take(&mut self.rejected),
            diagnostics: std::mem::take(&mut self.diagnostics),
        }
    }
}

trait AbsSigned {
    fn abs_signed(self) -> f64;
}
impl AbsSigned for f64 {
    fn abs_signed(self) -> f64 {
        self
    }
}

fn nearest_legal(v: f64, tol: f64) -> Option<i64> {
    let n = v.round() as i64;
    if (v - n as f64).abs() <= tol {
        Some(n)
    } else {
        None
    }
}

impl<'a> Engine<'a> {
    fn combination_edges(
        &mut self,
        tracks: &BTreeMap<(String, String), BTreeMap<i64, Pt>>,
        epoch_at: &impl Fn(i64) -> (String, f64),
        wl: impl Fn(&str) -> Option<f64>,
        edges: &mut BTreeMap<(String, String, i64), EdgeCand>,
        lli_pending: &mut Vec<(String, String, i64, String, f64)>,
    ) {
        // Gather satellites that carry both configured bands.
        let mut sats: BTreeSet<String> = BTreeSet::new();
        for (sat, band) in tracks.keys() {
            if band == &self.params.gf_band_a || band == &self.params.gf_band_b {
                sats.insert(sat.clone());
            }
        }
        let wa = wl(&self.params.gf_band_a);
        let wb = wl(&self.params.gf_band_b);
        for sat in sats {
            let ta = tracks.get(&(sat.clone(), self.params.gf_band_a.clone()));
            let tb = tracks.get(&(sat.clone(), self.params.gf_band_b.clone()));
            let (Some(ta), Some(tb), Some(wa), Some(wb)) = (ta, tb, wa, wb) else {
                continue;
            };
            let seqs: BTreeSet<i64> = ta.keys().chain(tb.keys()).copied().collect();
            let seqs: Vec<i64> = seqs.into_iter().collect();
            for w in seqs.windows(2) {
                let (s0, s1) = (w[0], w[1]);
                let a0 = ta.get(&s0);
                let a1 = ta.get(&s1);
                let b0 = tb.get(&s0);
                let b1 = tb.get(&s1);
                let pa0 = a0.and_then(|p| p.phase.as_ref());
                let pa1 = a1.and_then(|p| p.phase.as_ref());
                let pb0 = b0.and_then(|p| p.phase.as_ref());
                let pb1 = b1.and_then(|p| p.phase.as_ref());
                let ca0 = a0.and_then(|p| p.pseudo.as_ref());
                let ca1 = a1.and_then(|p| p.pseudo.as_ref());
                let cb0 = b0.and_then(|p| p.pseudo.as_ref());
                let cb1 = b1.and_then(|p| p.pseudo.as_ref());
                let (e1_raw, e1_t) = epoch_at(s1);
                let dt = match (a1, b1) {
                    (Some(x), _) => x.t - ta.get(&s0).map(|q| q.t).unwrap_or(x.t),
                    (_, Some(y)) => y.t - tb.get(&s0).map(|q| q.t).unwrap_or(y.t),
                    _ => 0.0,
                };
                let key = (sat.clone(), self.params.gf_band_a.clone(), s1);
                let ec = edges.entry(key).or_default();
                ec.dt = if ec.dt == 0.0 { dt } else { ec.dt };
                let missing_b = pb1.is_none() || pb0.is_none();
                if missing_b {
                    ec.missing_b = true;
                }

                // LLI flag propagation (both bands, all sats). Collect first
                // to avoid aliasing the GF edge mutable borrow.
                for (band, p1) in [
                    (self.params.gf_band_a.clone(), a1),
                    (self.params.gf_band_b.clone(), b1),
                ] {
                    if let Some(p1) = p1 {
                        if let Some(lli) = &p1.lli {
                            if let Some(bits) = lli.value {
                                if (bits as i64 & 1) == 1 {
                                    lli_pending.push((
                                        sat.clone(), band, s1, lli.merged_id.clone(), p1.t
                                    ));
                                }
                            }
                        }
                    }
                }

                let mut sources = Vec::new();
                for o in [pa0, pa1, pb0, pb1].into_iter().flatten() {
                    sources.push(o.merged_id.clone());
                }

                // ---- GF (geometry-free / ionosphere combination) -----------
                if let (Some(x0), Some(x1), Some(y0), Some(y1)) =
                    (phase_val(pa0), phase_val(pa1), phase_val(pb0), phase_val(pb1))
                {
                    let gf0 = x0 * wa - y0 * wb;
                    let gf1 = x1 * wa - y1 * wb;
                    let dgf = gf1 - gf0;
                    let id = ids::statistic_id(
                        "GF",
                        ALG_VERSION,
                        &self.param_fp,
                        &sat,
                        &format!("{}-{}", self.params.gf_band_a, self.params.gf_band_b),
                        &sources,
                    );
                    let clock_declared = self
                        .epochs
                        .iter()
                        .find(|e| e.seq == s1)
                        .and_then(|e| e.declared_clock_delta_ns)
                        .is_some();
                    let flag = if clock_declared {
                        EdgeFlag::ClockJumpDeclared
                    } else if missing_b {
                        EdgeFlag::MissingFrequency
                    } else if (e1_t - epoch_at(s0).1).abs()
                        > self.params.nominal_dt_secs * self.params.gap_factor
                    {
                        EdgeFlag::IrregularGap
                    } else {
                        EdgeFlag::Normal
                    };
                    self.statistics.push(EdgeStatistic {
                        id,
                        detector: "GF".to_string(),
                        alg_version: ALG_VERSION.to_string(),
                        param_fp: self.param_fp.clone(),
                        sat: sat.clone(),
                        band: format!("{}-{}", self.params.gf_band_a, self.params.gf_band_b),
                        at_seq: s1,
                        at_epoch: e1_raw.clone(),
                        dt_secs: dt,
                        value: Some(dgf),
                        value_unit: U_M.to_string(),
                        flag,
                        sources: sources.clone(),
                        diagnostic: None,
                    });
                    if !clock_declared {
                        let max = self.params.max_search_cycles;
                        for na in -max..=max {
                            for nb in -max..=max {
                                let resid = dgf - na as f64 * wa + nb as f64 * wb;
                                if resid.abs() <= self.params.gf_tol_m {
                                    ec.gf.push((na, nb, resid));
                                }
                            }
                        }
                        for (na, nb, r) in &ec.gf {
                            ec.proposals.push(format!(
                                "GF({},{}) na={na} nb={nb} resid={r:.4}m",
                                self.params.gf_band_a, self.params.gf_band_b
                            ));
                        }
                    }
                } else {
                    let id = ids::statistic_id(
                        "GF",
                        ALG_VERSION,
                        &self.param_fp,
                        &sat,
                        &format!("{}-{}", self.params.gf_band_a, self.params.gf_band_b),
                        &sources,
                    );
                    self.statistics.push(EdgeStatistic {
                        id,
                        detector: "GF".to_string(),
                        alg_version: ALG_VERSION.to_string(),
                        param_fp: self.param_fp.clone(),
                        sat: sat.clone(),
                        band: format!("{}-{}", self.params.gf_band_a, self.params.gf_band_b),
                        at_seq: s1,
                        at_epoch: e1_raw.clone(),
                        dt_secs: dt,
                        value: None,
                        value_unit: U_M.to_string(),
                        flag: EdgeFlag::MissingFrequency,
                        sources: sources.clone(),
                        diagnostic: Some(
                            "frequency missing at one side of edge; GF undefined".to_string(),
                        ),
                    });
                }

                // ---- MW (Melbourne-Wubbena wide lane) ----------------------
                let range0 = match (range_val(ca0), range_val(cb0)) {
                    (Some(r0), Some(r1)) => Some((r0 + r1) * 0.5),
                    _ => None,
                };
                let range1 = match (range_val(ca1), range_val(cb1)) {
                    (Some(r0), Some(r1)) => Some((r0 + r1) * 0.5),
                    _ => None,
                };
                if let (Some(x0), Some(x1), Some(y0), Some(y1), Some(r0), Some(r1), false) = (
                    phase_val(pa0),
                    phase_val(pa1),
                    phase_val(pb0),
                    phase_val(pb1),
                    range0,
                    range1,
                    missing_b,
                ) {
                    let denom = wa - wb;
                    if denom.abs() > 1e-9 {
                        let n0 = x0 - y0 - r0 / denom;
                        let n1 = x1 - y1 - r1 / denom;
                        let dn = n1 - n0;
                        let mut mw_sources = Vec::new();
                        for o in [pa0, pa1, pb0, pb1, ca0, ca1, cb0, cb1].into_iter().flatten() {
                            mw_sources.push(o.merged_id.clone());
                        }
                        let id = ids::statistic_id(
                            "MW",
                            ALG_VERSION,
                            &self.param_fp,
                            &sat,
                            &format!("{}-{}", self.params.gf_band_a, self.params.gf_band_b),
                            &mw_sources,
                        );
                        self.statistics.push(EdgeStatistic {
                            id,
                            detector: "MW".to_string(),
                            alg_version: ALG_VERSION.to_string(),
                            param_fp: self.param_fp.clone(),
                            sat: sat.clone(),
                            band: format!("{}-{}", self.params.gf_band_a, self.params.gf_band_b),
                            at_seq: s1,
                            at_epoch: e1_raw.clone(),
                            dt_secs: dt,
                            value: Some(dn),
                            value_unit: "wide_lane_cycle".to_string(),
                            flag: EdgeFlag::Normal,
                            sources: mw_sources,
                            diagnostic: None,
                        });
                        let clock_declared = self
                            .epochs
                            .iter()
                            .find(|e| e.seq == s1)
                            .and_then(|e| e.declared_clock_delta_ns)
                            .is_some();
                        if !clock_declared {
                            let n = dn.round() as i64;
                            if (dn - n as f64).abs() <= self.params.mw_tol_wl_cycles {
                                ec.mw.push((n, dn - n as f64));
                                ec.proposals
                                    .push(format!("MW wl_n={n} resid={:.4}wlcyc", dn - n as f64));
                            }
                        }
                    }
                }
            }
        }
    }

    fn clock_statistics(
        &mut self,
        tracks: &BTreeMap<(String, String), BTreeMap<i64, Pt>>,
        epoch_at: &impl Fn(i64) -> (String, f64),
        declared: &BTreeMap<i64, f64>,
        edges: &mut BTreeMap<(String, String, i64), EdgeCand>,
    ) {
        // Per-satellite common-mode estimate from TD on band A: the median
        // across sats of phase increments in metres; a declared clock jump is
        // corroborated when consensus >= configured minimum.
        let band = self.params.gf_band_a.clone();
        let mut edge_estimates: BTreeMap<i64, Vec<(String, f64)>> = BTreeMap::new();
        for ((sat, b), pts) in tracks {
            if b != &band {
                continue;
            }
            let seqs: Vec<i64> = pts.keys().copied().collect();
            for w in seqs.windows(2) {
                if let (Some(p0), Some(p1)) = (pts.get(&w[0]), pts.get(&w[1])) {
                    if let (Some(a), Some(c)) = (&p0.phase, &p1.phase) {
                        if let (Some(x), Some(y)) = (a.value, c.value) {
                            edge_estimates
                                .entry(w[1])
                                .or_default()
                                .push((sat.clone(), y - x));
                        }
                    }
                }
            }
        }
        for (seq, mut vals) in edge_estimates {
            if vals.len() < self.params.clock_consensus_min_sats {
                continue;
            }
            vals.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let median = vals[vals.len() / 2].1;
            let tkey = (epoch_at(seq).1 * 1000.0).round() as i64;
            let is_declared = declared.contains_key(&tkey);
            // Median is close to the declared jump in seconds * f1.
            let corroborated = is_declared
                || (median.abs() > 5.0
                    && vals
                        .iter()
                        .filter(|(_, v)| (*v - median).abs() < 1.0)
                        .count()
                        >= self.params.clock_consensus_min_sats);
            for (sat, v) in &vals {
                let sources: Vec<String> = tracks
                    .get(&(sat.clone(), band.clone()))
                    .and_then(|m| m.get(&seq))
                    .and_then(|p| p.phase.as_ref())
                    .map(|o| vec![o.merged_id.clone()])
                    .unwrap_or_default();
                let id = ids::statistic_id(
                    "CLOCK",
                    ALG_VERSION,
                    &self.param_fp,
                    sat,
                    &band,
                    &sources,
                );
                self.statistics.push(EdgeStatistic {
                    id,
                    detector: "CLOCK".to_string(),
                    alg_version: ALG_VERSION.to_string(),
                    param_fp: self.param_fp.clone(),
                    sat: sat.clone(),
                    band: band.clone(),
                    at_seq: seq,
                    at_epoch: epoch_at(seq).0,
                    dt_secs: 0.0,
                    value: Some(*v),
                    value_unit: U_CYCLE.to_string(),
                    flag: if corroborated {
                        EdgeFlag::ClockJumpDeclared
                    } else {
                        EdgeFlag::Normal
                    },
                    sources,
                    diagnostic: if is_declared {
                        Some(format!(
                            "receiver-wide clock event declared; common-mode median={median:.3} cycles on band {band}"
                        ))
                    } else {
                        None
                    },
                });
            }
            if corroborated {
                // No slip candidates are injected: clock jumps and per-sat
                // cycle slips remain separate objects.
                for (sat, _) in &vals {
                    if let Some(ec) = edges.get_mut(&(sat.clone(), band.clone(), seq)) {
                        ec.clock_declared = true;
                    }
                }
            }
        }
    }
}

fn phase_val(o: Option<&Obs>) -> Option<f64> {
    o.and_then(|x| x.value)
}
fn range_val(o: Option<&Obs>) -> Option<f64> {
    o.and_then(|x| x.value)
}

impl<'a> Engine<'a> {
    fn decisions_force_break(&self, sat: &str, band: &str, at_epoch: &str) -> bool {
        self.decisions
            .iter()
            .any(|d| matches!(d.action, DecisionAction::ForceBreak)
                && d.sat.as_deref() == Some(sat)
                && d.band.as_deref() == Some(band)
                && d.at_epoch.as_deref() == Some(at_epoch))
    }

    fn apply_decisions(&mut self, _viewpoint: &str) {
        // Decisions arrive in event order (the ledger is linearized along the
        // current branch by the store).
        for d in self.decisions {
            match d.action {
                DecisionAction::AcceptCandidate => {
                    let Some(cid) = &d.candidate_id else { continue };
                    let Some(idx) = self.candidates.iter().position(|c| &c.id == cid) else {
                        continue;
                    };
                    // Conflict rule: one offset per (sat,band,edge). If an
                    // offset already exists there, this accept is recorded as
                    // a rejected conflict in the caller via event history; in
                    // the recomputed view the later branch decision wins.
                    let c = &self.candidates[idx];
                    let conflict = self.offsets.iter().any(|o| {
                        o.sat == c.sat
                            && o.band == c.band
                            && o.at_epoch == c.at_epoch
                    }) || c.offset_b_cycles.is_some_and(|_| {
                        c.band_b.as_deref().is_some_and(|bb| {
                            self.offsets.iter().any(|o| {
                                o.sat == c.sat && o.band == bb && o.at_epoch == c.at_epoch
                            })
                        })
                    });
                    if conflict {
                        // Keep candidate Proposed; the conflicting decision
                        // event itself remains in the ledger for audit.
                        continue;
                    }
                    if c.offset_unknown {
                        // LLI with unknown offset cannot be applied to the
                        // offset layer; acceptance records status only.
                        self.candidates[idx].status = CandidateStatus::Accepted;
                        self.candidates[idx].status_reason =
                            Some("accepted_without_numeric_offset".to_string());
                        continue;
                    }
                    let off_a = OffsetEntry {
                        id: ids::offset_id(
                            &c.sat,
                            &c.band,
                            &c.at_epoch,
                            c.offset_cycles,
                            &d.event_id,
                        ),
                        sat: c.sat.clone(),
                        band: c.band.clone(),
                        at_seq: c.at_seq,
                        at_epoch: c.at_epoch.clone(),
                        cycles: c.offset_cycles,
                        unit: U_CYCLE.to_string(),
                        decision_event_id: d.event_id.clone(),
                        rationale: d.reason.clone(),
                    };
                    let mut ids_new = vec![off_a.id.clone()];
                    self.offsets.push(off_a);
                    if let (Some(nb_cycles), Some(bb)) =
                        (c.offset_b_cycles, c.band_b.clone())
                    {
                        let off_b = OffsetEntry {
                            id: ids::offset_id(&c.sat, &bb, &c.at_epoch, nb_cycles, &d.event_id),
                            sat: c.sat.clone(),
                            band: bb,
                            at_seq: c.at_seq,
                            at_epoch: c.at_epoch.clone(),
                            cycles: nb_cycles,
                            unit: U_CYCLE.to_string(),
                            decision_event_id: d.event_id.clone(),
                            rationale: d.reason.clone(),
                        };
                        ids_new.push(off_b.id.clone());
                        self.offsets.push(off_b);
                    }
                    self.candidates[idx].status = CandidateStatus::Accepted;
                    self.candidates[idx].status_reason = Some(d.reason.clone());
                    self.candidates[idx].offset_ids = ids_new;
                }
                DecisionAction::RejectCandidate => {
                    let Some(cid) = &d.candidate_id else { continue };
                    if let Some(idx) = self.candidates.iter().position(|c| &c.id == cid) {
                        self.candidates[idx].status = CandidateStatus::Rejected;
                        self.candidates[idx].status_reason = Some(d.reason.clone());
                        self.rejected.push(RejectedCandidate {
                            candidate_id: cid.clone(),
                            reason_code: "human_rejected".to_string(),
                            reason: d.reason.clone(),
                            decision_event_id: d.event_id.clone(),
                        });
                    }
                }
                DecisionAction::ForceBreak | DecisionAction::ParameterRevision => {
                    // ForceBreak affects arc construction; ParameterRevision is
                    // reflected by the param fingerprint already.
                }
            }
        }
    }

    fn build_arcs(
        &self,
        tracks: &BTreeMap<(String, String), BTreeMap<i64, Pt>>,
        epoch_at: &impl Fn(i64) -> (String, f64),
        declared: &BTreeMap<i64, f64>,
    ) -> Vec<Arc> {
        let mut arcs = Vec::new();
        for ((sat, band), pts) in tracks {
            let seqs: Vec<i64> = pts
                .iter()
                .filter(|(_, p)| p.phase.as_ref().and_then(|o| o.value).is_some())
                .map(|(s, _)| *s)
                .collect();
            if seqs.is_empty() {
                continue;
            }
            let start_seq = seqs[0];
            let mut cur = Arc {
                id: ids::arc_id(sat, band, &epoch_at(start_seq).0, start_seq),
                sat: sat.clone(),
                band: band.clone(),
                start_seq,
                end_seq: None,
                start_epoch: epoch_at(start_seq).0,
                end_epoch: None,
                state: ArcState::Open,
                close_reason: None,
                boundary_candidates: Vec::new(),
            };
            for w in seqs.windows(2) {
                let (s0, s1) = (w[0], w[1]);
                let (raw1, t1) = epoch_at(s1);
                let (_, t0) = epoch_at(s0);
                let dt = t1 - t0;
                let irregular =
                    dt > self.params.nominal_dt_secs * self.params.gap_factor;
                let clock = declared.contains_key(&((t1 * 1000.0).round() as i64));
                let force = self.decisions_force_break(sat, band, &raw1);
                let accepted_offset = self.offsets.iter().any(|o| {
                    o.sat == *sat && o.band == *band && o.at_epoch == raw1
                });
                // Candidate boundary list for this edge.
                let cands: Vec<String> = self
                    .candidates
                    .iter()
                    .filter(|c| {
                        c.sat == *sat
                            && c.at_epoch == raw1
                            && (&c.band == band
                                || c.band_b.as_deref() == Some(band.as_str()))
                    })
                    .map(|c| c.id.clone())
                    .collect();
                let break_here = clock || force || accepted_offset;
                if break_here || (irregular && !cands.is_empty()) {
                    cur.end_seq = Some(s0);
                    cur.end_epoch = Some(epoch_at(s0).0);
                    cur.state = ArcState::Closed;
                    cur.close_reason = Some(
                        if clock {
                            "receiver_clock_event".to_string()
                        } else if force {
                            "manual_force_break".to_string()
                        } else if accepted_offset {
                            "accepted_cycle_offset".to_string()
                        } else {
                            "irregular_gap_with_candidates".to_string()
                        },
                    );
                    arcs.push(cur);
                    cur = Arc {
                        id: ids::arc_id(sat, band, &raw1, s1),
                        sat: sat.clone(),
                        band: band.clone(),
                        start_seq: s1,
                        end_seq: None,
                        start_epoch: raw1.clone(),
                        end_epoch: None,
                        state: ArcState::Open,
                        close_reason: None,
                        boundary_candidates: cands.clone(),
                    };
                } else {
                    cur.boundary_candidates.extend(cands);
                }
            }
            arcs.push(cur);
        }
        arcs.sort_by(|a, b| {
            a.sat
                .cmp(&b.sat)
                .then(a.band.cmp(&b.band))
                .then(a.start_seq.cmp(&b.start_seq))
        });
        arcs
    }
}
