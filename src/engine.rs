// Domain computation engine.
//
// Three independent detectors, none treated as ground truth:
//   * GF  geometry-free / ionosphere residual   I = l1*L1 - l2*L2  (metre)
//   * MW  Melbourne-Wubbena widelane ambiguity  (cycle)
//   * TD  time-differenced, Doppler-aided phase residual per band (cycle)
//
// A receiver clock jump is a common-mode event across every satellite and is
// classified separately (kind='clock'); it must never be mislabelled as a
// per-satellite cycle slip. Repair is recorded only on the offset layer; the
// raw observations are immutable.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::params;

use crate::db::C;
use crate::models::{
    id_arc, id_cand, id_res, short_hash, DetectorParams, EpochInfo, FrequencyBasis, ObsRow,
    ALGO_VERSION,
};

#[derive(Debug, Clone)]
pub struct BandObs {
    pub obs: ObsRow,
    pub wavelength_m: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SatEpoch {
    pub bands: BTreeMap<String, BandObs>, // band "L1"/"L2"
}

#[derive(Debug, Clone)]
pub struct GfRes {
    pub epoch: i64,
    pub value: Option<f64>,
    pub status: String,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct MwRes {
    pub epoch: i64,
    pub value: Option<f64>,
    pub status: String,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct TdRes {
    pub epoch: i64,
    pub band: String,
    pub value: Option<f64>,
    pub status: String,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct Hypothesis {
    pub n1: i64,
    pub n2: i64,
    pub gf_error_m: f64,
    pub mw_error_cy: Option<f64>,
    pub td_error_cy: Option<f64>,
    pub votes: serde_json::Value,
    pub score: f64,
    pub gf_ok: bool,
    pub mw_ok: bool,
    pub td_ok: bool,
}

#[derive(Debug, Clone)]
pub struct SatData {
    pub sat: String,
    pub gnss: String,
    pub by_epoch: BTreeMap<i64, SatEpoch>,
    pub gf: Vec<GfRes>,
    pub mw: Vec<MwRes>,
    pub td: Vec<TdRes>,
    pub arc_breaks: BTreeMap<i64, BTreeSet<String>>, // break-at-epoch -> bands
}

pub struct EngineInput<'a> {
    pub project: &'a str,
    pub obs: &'a [ObsRow],
    pub epochs: &'a [EpochInfo],
    pub freqs: &'a [FrequencyBasis],
    pub params: &'a DetectorParams,
    pub param_fingerprint: &'a str,
    pub input_fingerprint: &'a str,
}

fn wavelength(freqs: &[FrequencyBasis], gnss: &str, band: &str) -> f64 {
    freqs
        .iter()
        .find(|f| f.gnss == gnss && f.band == band)
        .map(|f| f.wavelength_m)
        .unwrap_or_else(|| C / if band == "L1" { 1.57542e9 } else { 1.22760e9 })
}

pub fn organize(inp: &EngineInput) -> BTreeMap<String, SatData> {
    let mut sats: BTreeMap<String, SatData> = BTreeMap::new();
    for o in inp.obs {
        if o.value_status != "ok" || o.phase_cy.is_none() {
            // keep unconvertible visible but out of numeric combinations
            continue;
        }
        let sd = sats.entry(o.sat.clone()).or_insert_with(|| SatData {
            sat: o.sat.clone(),
            gnss: o.gnss.clone(),
            ..Default::default()
        });
        let wl = wavelength(inp.freqs, &o.gnss, &o.band);
        sd.by_epoch
            .entry(o.epoch_idx)
            .or_default()
            .bands
            .insert(o.band.clone(), BandObs { obs: o.clone(), wavelength_m: wl });
    }
    sats
}

pub fn epoch_times(epochs: &[EpochInfo]) -> BTreeMap<i64, f64> {
    epochs.iter().map(|e| (e.idx, e.t_seconds)).collect()
}

// ---------- detector residuals ----------

// GF (geometry-free / ionosphere combination): I = l1*phi1 - l2*phi2 [m].
// Its epoch difference isolates (l1*n1 - l2*n2); common receiver clock cancels.
fn gf_series(sd: &SatData) -> Vec<GfRes> {
    let mut out = Vec::new();
    let mut prev: Option<(i64, f64)> = None;
    for (ep, se) in &sd.by_epoch {
        let b1 = se.bands.get("L1");
        let b2 = se.bands.get("L2");
        match (b1, b2) {
            (Some(a), Some(b)) => {
                let v = a.wavelength_m * a.obs.phase_cy.unwrap()
                    - b.wavelength_m * b.obs.phase_cy.unwrap();
                let (diff, status) = match prev {
                    Some((pe, pv)) => (Some((v - pv, *ep - pe)), "ok"),
                    None => (None, "ok"),
                };
                out.push(GfRes {
                    epoch: *ep,
                    value: diff.map(|(d, _)| d),
                    status: if prev.is_some() { status.to_string() } else { "ok".to_string() },
                    detail: serde_json::json!({
                        "gf_absolute_m": v,
                        "delta_gf_m": diff.map(|(d,_)| d),
                        "prev_epoch": diff.map(|(_, pe)| pe),
                        "unit": "metre",
                        "combination": "l1*L1 - l2*L2",
                    }),
                });
                prev = Some((*ep, v));
            }
            _ => {
                out.push(GfRes {
                    epoch: *ep,
                    value: None,
                    status: "missing_pair".to_string(),
                    detail: serde_json::json!({
                        "unit": "metre",
                        "reason": "L1 or L2 absent at this epoch",
                    }),
                });
                prev = None; // gap resets differencing
            }
        }
    }
    out
}

// MW widelane ambiguity in cycles:
//   N_w = (phi1 - phi2) - (f1*P1 + f2*P2)/(C*(f1+f2)/? ) ... implemented using
//   per-band wavelengths l1,l2 directly:
//   P_wl_m = C/(f1+f2) * (P1/l1_nl... ) -- see formulas below.
// We use the equivalent robust form:
//   wl_wavelength = C/(f1-f2)
//   nl_code_m     = (f1*P1 + f2*P2)/(f1+f2)
//   N_w(cyc)      = (l1? no) -> (phi1-phi2) - nl_code_m/wl_wavelength
// Clock terms cancel exactly in N_w, so a receiver clock jump does not move MW.
fn mw_series(sd: &SatData, freqs: &[FrequencyBasis]) -> Vec<MwRes> {
    let f = |band: &str| -> f64 {
        freqs.iter().find(|x| x.gnss == sd.gnss && x.band == band)
            .map(|x| x.freq_hz)
            .unwrap_or(if band == "L1" { 1.57542e9 } else { 1.22760e9 })
    };
    let f1 = f("L1");
    let f2 = f("L2");
    let wl = C / (f1 - f2);
    let mut out = Vec::new();
    let mut prev: Option<f64> = None;
    for (ep, se) in &sd.by_epoch {
        let b1 = se.bands.get("L1");
        let b2 = se.bands.get("L2");
        if let (Some(a), Some(b)) = (b1, b2) {
            if let (Some(p1), Some(p2), Some(ph1), Some(ph2)) =
                (a.obs.pseudo_m, b.obs.pseudo_m, a.obs.phase_cy, b.obs.phase_cy)
            {
                let nl = (f1 * p1 + f2 * p2) / (f1 + f2);
                let nw = (ph1 - ph2) - nl / wl;
                let d = prev.map(|pv| nw - pv);
                out.push(MwRes {
                    epoch: *ep,
                    value: d,
                    status: "ok".to_string(),
                    detail: serde_json::json!({
                        "mw_absolute_cy": nw,
                        "delta_mw_cy": d,
                        "widelane_wavelength_m": wl,
                        "narrowlane_code_m": nl,
                        "unit": "cycle",
                        "combination": "(phi1-phi2) - (f1*P1+f2*P2)/((f1+f2)*wl)",
                    }),
                });
                prev = Some(nw);
                continue;
            }
        }
        out.push(MwRes {
            epoch: *ep,
            value: None,
            status: "missing_pair".to_string(),
            detail: serde_json::json!({"unit":"cycle","reason":"need L1,L2 phase+pseudorange"}),
        });
        prev = None;
    }
    out
}

// TD (time-differenced, Doppler aided) per band.
// Predicted phase change from Doppler: (D_i + D_prev)/2 * dt.
//   td_residual_cy = (phi_i - phi_prev) - doppler_pred
// A receiver clock adds f*dt_clk cycles identically to all satellites, which is
// why TD alone cannot separate clock from a common slip (handled downstream).
fn td_series(sd: &SatData, times: &BTreeMap<i64, f64>) -> Vec<TdRes> {
    let mut out = Vec::new();
    let mut prev: HashMap<String, (i64, f64, Option<f64>)> = HashMap::new();
    for (ep, se) in &sd.by_epoch {
        for (band, bo) in &se.bands {
            let cur_t = times.get(ep).copied();
            let cur_d = bo.obs.doppler_hz;
            let st = prev.get(band).copied();
            if let (Some((pe, pph, pd)), Some(ct), Some(pt)) =
                (st, cur_t, times.get(&pe).copied())
            {
                let dt = ct - pt;
                if let (Some(dnow), Some(dold)) = (cur_d, pd) {
                    if dt > 0.0 {
                        let pred = 0.5 * (dnow + dold) * dt;
                        let resid = (bo.obs.phase_cy.unwrap() - pph) - pred;
                        out.push(TdRes {
                            epoch: *ep,
                            band: band.clone(),
                            value: Some(resid),
                            status: "ok".to_string(),
                            detail: serde_json::json!({
                                "unit": "cycle",
                                "dt_s": dt,
                                "doppler_pred_cy": pred,
                                "phase_delta_cy": bo.obs.phase_cy.unwrap() - pph,
                                "doppler_present": true,
                            }),
                        });
                    }
                } else {
                    out.push(TdRes {
                        epoch: *ep,
                        band: band.clone(),
                        value: None,
                        status: "unknown".to_string(),
                        detail: serde_json::json!({
                            "unit":"cycle",
                            "reason":"Doppler missing on this pair; TD cannot decide",
                            "doppler_present": false,
                        }),
                    });
                }
            }
            prev.insert(band.clone(), (*ep, bo.obs.phase_cy.unwrap(), cur_d));
        }
    }
    out
}

// ---------- receiver clock jump (common mode) ----------

#[derive(Debug, Clone)]
pub struct ClockFinding {
    pub epoch: i64,
    pub is_clock: bool,
    pub mean_code_step_m: Option<f64>,
    pub mean_l1_step_cy: Option<f64>,
    pub consensus: usize,
    pub detail: serde_json::Value,
}

// A receiver clock jump moves pseudorange by c*tau and phase by f*tau on EVERY
// satellite simultaneously. Require a strong common-mode consensus; isolated
// single-satellite steps must not be classified as clock.
fn detect_clock(sats: &BTreeMap<String, SatData>, times: &BTreeMap<i64, f64>, p: &DetectorParams) -> BTreeMap<i64, ClockFinding> {
    let epochs: Vec<i64> = times.keys().copied().collect();
    let mut out = BTreeMap::new();
    for w in epochs.windows(2) {
        let (pe, ce) = (w[0], w[1]);
        let mut code_steps = Vec::new();
        let mut ph1_steps = Vec::new();
        for sd in sats.values() {
            if let (Some(prev), Some(cur)) = (sd.by_epoch.get(&pe), sd.by_epoch.get(&ce)) {
                if let (Some(a), Some(b)) = (prev.bands.get("L1"), cur.bands.get("L1")) {
                    if let (Some(pa), Some(pb)) = (a.obs.pseudo_m, b.obs.pseudo_m) {
                        let ds = pb - pa;
                        // subtract geometric+doppler-expected motion using TD residual scale;
                        // clock steps here are huge (~3e5 m), so just collect large common steps.
                        if ds.abs() > p.clock_threshold_m {
                            code_steps.push(ds);
                        }
                    }
                    if let (Some(pa), Some(pb)) = (a.obs.phase_cy, b.obs.phase_cy) {
                        // expected phase change from doppler
                        let dt = times[&ce] - times[&pe];
                        let expected = match (a.obs.doppler_hz, b.obs.doppler_hz) {
                            (Some(d0), Some(d1)) => 0.5 * (d0 + d1) * dt,
                            _ => 0.0,
                        };
                        let step = (pb - pa) - expected;
                        if step.abs() > 100_000.0 {
                            ph1_steps.push(step);
                        }
                    }
                }
            }
        }
        let n = sats.len().max(1);
        let consensus = code_steps.len();
        let is_clock = consensus * 2 >= n && consensus >= 2;
        if is_clock {
            let mean_code = mean(&code_steps);
            let mean_ph = mean(&ph1_steps);
            out.insert(
                ce,
                ClockFinding {
                    epoch: ce,
                    is_clock: true,
                    mean_code_step_m: mean_code,
                    mean_l1_step_cy: mean_ph,
                    consensus,
                    detail: serde_json::json!({
                        "scope": "receiver",
                        "code_steps_m": code_steps,
                        "phase_l1_steps_cy": ph1_steps,
                        "consensus_satellites": consensus,
                        "total_satellites": sats.len(),
                        "interpretation": "common-mode receiver clock jump; not a per-satellite slip",
                    }),
                },
            );
        }
    }
    out
}

fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    Some(xs.iter().sum::<f64>() / xs.len() as f64)
}

// ---------- slip hypothesis generation / fusion ----------

#[derive(Debug, Clone)]
pub struct EpochFindings {
    pub epoch: i64,
    pub gf_delta: Option<f64>,
    pub mw_delta: Option<f64>,
    pub td: BTreeMap<String, Option<f64>>, // band -> residual (None=unknown)
    pub lli_bits: BTreeMap<String, i64>,
    pub lli_set: bool,
    pub gf_status: String,
}

fn findings_at(sd: &SatData, ep: i64) -> EpochFindings {
    let gf = sd.gf.iter().find(|g| g.epoch == ep);
    let mw = sd.mw.iter().find(|m| m.epoch == ep);
    let mut td = BTreeMap::new();
    for t in sd.td.iter().filter(|t| t.epoch == ep) {
        td.insert(t.band.clone(), t.value);
    }
    let mut lli_bits = BTreeMap::new();
    let mut lli_set = false;
    if let Some(se) = sd.by_epoch.get(&ep) {
        for (band, bo) in &se.bands {
            if let Some(b) = bo.obs.lli_bits {
                lli_bits.insert(band.clone(), b);
                // bit 0 = possible slip, bit 1 = warning, bit 2 = antispoof/other;
                // any non-zero raises an LLI flag candidate.
                if b != 0 {
                    lli_set = true;
                }
            }
        }
    }
    EpochFindings {
        epoch: ep,
        gf_delta: gf.and_then(|g| g.value),
        mw_delta: mw.and_then(|m| m.value),
        td,
        lli_bits,
        lli_set,
        gf_status: gf.map(|g| g.status.clone()).unwrap_or_else(|| "missing_pair".to_string()),
    }
}

// Enumerate integer pairs in the lattice and score against every available
// detector. Each detector contributes an independent vote; no detector is the
// truth. If multiple pairs pass the same detectors they are all returned.
fn hypotheses(
    f: &EpochFindings,
    l1: f64,
    l2: f64,
    p: &DetectorParams,
) -> Vec<Hypothesis> {
    let nmax = p.max_lattice_n;
    let mut out = Vec::new();
    for n1 in -nmax..=nmax {
        for n2 in -nmax..=nmax {
            if n1 == 0 && n2 == 0 {
                // (0,0) only interesting if everything agrees no-slip; still score it
            }
            // GF error
            let mut gf_err = f64::NAN;
            let mut gf_ok = false;
            if let Some(dgf) = f.gf_delta {
                gf_err = (l1 * n1 as f64 - l2 * n2 as f64 - dgf).abs();
                gf_ok = gf_err <= p.gf_threshold_m;
            }
            // MW error
            let mut mw_err = None;
            let mut mw_ok = false;
            if let Some(dmw) = f.mw_delta {
                let e = (n1 as f64 - n2 as f64 - dmw).abs();
                mw_err = Some(e);
                mw_ok = e <= p.mw_threshold_cy;
            }
            // TD error: per band compare integer to observed residual; only
            // constraining when Doppler (hence TD) is available.
            let mut td_errs: Vec<f64> = Vec::new();
            let mut td_ok = true;
            let mut td_available = false;
            let mut td_vote = serde_json::json!({"status":"unknown","reason":"Doppler missing"});
            if let Some(v) = f.td.get("L1") {
                td_available = true;
                match v {
                    Some(r) => {
                        let e = (n1 as f64 - r).abs();
                        td_errs.push(e);
                        if e > p.td_threshold_cy { td_ok = false; }
                    }
                    None => { td_ok = false; td_vote["reason"] = serde_json::json!("L1 Doppler missing"); }
                }
            }
            if let Some(v) = f.td.get("L2") {
                td_available = true;
                match v {
                    Some(r) => {
                        let e = (n2 as f64 - r).abs();
                        td_errs.push(e);
                        if e > p.td_threshold_cy { td_ok = false; }
                    }
                    None => { td_ok = false; }
                }
            }
            if !td_available { td_ok = false; }
            let td_err = if td_errs.is_empty() { None } else { Some(td_errs.iter().cloned().fold(0.0, f64::max)) };

            // A detector "supports" a pair only when it has data and passes.
            let mut votes = serde_json::Map::new();
            let gf_vote = if f.gf_delta.is_some() {
                if gf_ok { "support" } else { "reject" }
            } else { "unavailable" };
            let mw_vote = if f.mw_delta.is_some() {
                if mw_ok { "support" } else { "reject" }
            } else { "unavailable" };
            let td_vote_s = if !td_available {
                "unavailable"
            } else if td_ok { "support" } else { "reject" };
            votes.insert("gf_ionosphere".into(), serde_json::json!(gf_vote));
            votes.insert("melbourne_wubbena".into(), serde_json::json!(mw_vote));
            votes.insert("time_difference".into(), serde_json::json!(td_vote_s));

            let supports = [gf_vote, mw_vote, td_vote_s].iter().filter(|v| **v == "support").count();
            let rejects = [gf_vote, mw_vote, td_vote_s].iter().filter(|v| **v == "reject").count();
            let available = [gf_vote, mw_vote, td_vote_s].iter().filter(|v| **v != "unavailable").count();
            // keep pairs that at least one available detector supports and none rejects
            let legal = supports >= 1 && rejects == 0 && available >= 1;
            let _ = td_vote;
            if !legal {
                continue;
            }
            // score: more support better; smaller residual magnitude better;
            // tie distance penalises ambiguous situations explicitly.
            let mag = gf_err.map(|x| if x.is_nan() {0.0} else {x}).unwrap_or(0.0)
                + mw_err.unwrap_or(0.0)
                + td_err.unwrap_or(0.0);
            let score = supports as f64 * 10.0 - mag;
            out.push(Hypothesis {
                n1, n2,
                gf_error_m: if gf_err.is_nan() { 0.0 } else { gf_err },
                mw_error_cy: mw_err,
                td_error_cy: td_err,
                votes: serde_json::Value::Object(votes),
                score,
                gf_ok, mw_ok, td_ok,
            });
        }
    }
    // keep top band by score but retain ties (multiple equally dividing offsets)
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(best) = out.first().map(|h| h.score) {
        out.retain(|h| best - h.score < 1e-9 || (h.score >= best - 0.0001));
    }
    out
}

// ---------- arcs ----------

#[derive(Debug, Clone)]
pub struct ArcSeg {
    pub sat: String,
    pub band: String,
    pub start: i64,
    pub end: i64,
    pub reason: String,
    pub source_key: String,
}

// Build per-(sat,band) arcs. A missing frequency (no observation at an epoch)
// splits the arc with reason=missing_frequency; uneven epoch spacing is recorded
// via dt but does not by itself split. Accepted slips and clock epochs are
// passed in as explicit boundaries.
pub fn build_arcs(
    run_id: &str,
    sats: &BTreeMap<String, SatData>,
    clock_epochs: &BTreeSet<i64>,
    accepted_slips: &BTreeMap<String, BTreeSet<i64>>, // "sat:band" -> epochs
) -> Vec<ArcSeg> {
    let mut segs = Vec::new();
    for (sat, sd) in sats {
        for band in ["L1", "L2"] {
            let present: Vec<i64> = sd
                .by_epoch
                .iter()
                .filter(|(_, se)| se.bands.contains_key(band))
                .map(|(ep, _)| *ep)
                .collect();
            if present.is_empty() {
                continue;
            }
            let mut start = present[0];
            let mut prev = present[0];
            let mut reason = "initial".to_string();
            for &ep in &present[1..] {
                let mut split = false;
                if ep != prev + 1 {
                    // missing epoch(s) for this band -> frequency gap
                    split = true;
                    reason = "missing_frequency".to_string();
                }
                let key = format!("{sat}:{band}");
                if clock_epochs.contains(&ep) {
                    split = true;
                    reason = "clock_boundary".to_string();
                }
                if accepted_slips.get(&key).map(|s| s.contains(&ep)).unwrap_or(false) {
                    split = true;
                    reason = "accepted_slip".to_string();
                }
                if split {
                    segs.push(ArcSeg {
                        sat: sat.clone(),
                        band: band.to_string(),
                        start,
                        end: prev,
                        reason: reason.clone(),
                        source_key: arc_source(run_id, sat, band, start),
                    });
                    start = ep;
                    reason = "initial".to_string();
                }
                prev = ep;
            }
            segs.push(ArcSeg {
                sat: sat.clone(),
                band: band.to_string(),
                start,
                end: prev,
                reason,
                source_key: arc_source(run_id, sat, band, start),
            });
        }
    }
    segs
}

fn arc_source(run_id: &str, sat: &str, band: &str, start: i64) -> String {
    format!(
        "arc-source:{}",
        short_hash(&format!("{run_id}|{sat}|{band}|{start}|inputs"))
    )
}

// ---------- run computation ----------

pub struct DecisionRef {
    pub sat: String,
    pub epoch: i64,
    pub n1: i64,
    pub n2: i64,
    pub action: String, // accept | reject
}

pub struct RunResult {
    pub run_id: String,
    pub arcs: Vec<ArcSeg>,
    pub accepted_slips: BTreeMap<String, BTreeSet<i64>>,
    pub ambiguous_keys: BTreeSet<String>,
}

pub fn compute_run(
    conn: &mut rusqlite::Connection,
    project: &str,
    branch: &str,
    up_to_seq: i64,
    obs: &[ObsRow],
    epochs: &[EpochInfo],
    freqs: &[FrequencyBasis],
    params: &DetectorParams,
    fp: &str,
    input_fingerprint: &str,
    decisions: &[DecisionRef],
    virtual_t: &str,
    note: &str,
) -> rusqlite::Result<RunResult> {
    let run_id = id_run(branch, up_to_seq);

    // Reuse an existing immutable run snapshot if present (never overwrite).
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM run WHERE id=?1", params![run_id], |r| r.get(0))?;
    if exists == 0 {
        conn.execute(
            "INSERT INTO run(id,project_id,branch_id,up_to_seq,algo_version,param_fingerprint,input_fingerprint,created_virtual_t,note)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![run_id, project, branch, up_to_seq, ALGO_VERSION, fp, input_fingerprint, virtual_t, note],
        )?;
    }

    let inp = EngineInput { project, obs, epochs, freqs, params, param_fingerprint: fp, input_fingerprint };
    let mut sats = organize(&inp);
    let times = epoch_times(epochs);
    for sd in sats.values_mut() {
        sd.gf = gf_series(sd);
        sd.mw = mw_series(sd, freqs);
        sd.td = td_series(sd, &times);
    }
    let clocks = detect_clock(&sats, &times, params);

    // accepted/rejected decisions up to this point
    let mut accepted: BTreeMap<(String, i64), (i64, i64)> = BTreeMap::new();
    let mut rejected: BTreeSet<(String, i64, i64, i64)> = BTreeSet::new();
    for d in decisions {
        match d.action.as_str() {
            "accept" => { accepted.insert((d.sat.clone(), d.epoch), (d.n1, d.n2)); }
            "reject" => { rejected.insert((d.sat.clone(), d.epoch, d.n1, d.n2)); }
            _ => {}
        }
    }
    let mut accepted_slips: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut ambiguous_keys = BTreeSet::new();

    if exists == 0 {
        // ---- residuals (GF / MW / TD) ----
        for sd in sats.values() {
            let l1 = wavelength(freqs, &sd.gnss, "L1");
            let l2 = wavelength(freqs, &sd.gnss, "L2");
            for g in &sd.gf {
                let rid = id_res(&run_id, "gf", &sd.sat, g.epoch, "L1L2");
                let (val, status) = match g.value { Some(v)=>(Some(v), g.status.clone()), None=>(None, g.status.clone()) };
                let src = source_key(&run_id, "gf", &sd.sat, g.epoch);
                conn.execute(
                    "INSERT INTO residual(id,run_id,project_id,sat,band,epoch_idx,detector,value,unit,status,freshness,detail_json,source_key,algo_version,param_fingerprint)
                     VALUES(?1,?2,?3,?4,'L1L2',?5,'gf',?6,'metre',?7,'current',?8,?9,?10,?11)",
                    params![rid, run_id, project, sd.sat, g.epoch, val, status, g.detail.to_string(), src, ALGO_VERSION, fp],
                )?;
            }
            for m in &sd.mw {
                let rid = id_res(&run_id, "mw", &sd.sat, m.epoch, "L1L2");
                let status = if m.value.is_some() { "ok" } else { &m.status };
                let src = source_key(&run_id, "mw", &sd.sat, m.epoch);
                conn.execute(
                    "INSERT INTO residual(id,run_id,project_id,sat,band,epoch_idx,detector,value,unit,status,freshness,detail_json,source_key,algo_version,param_fingerprint)
                     VALUES(?1,?2,?3,?4,'L1L2',?5,'mw',?6,'cycle',?7,'current',?8,?9,?10,?11)",
                    params![rid, run_id, project, sd.sat, m.epoch, m.value, status, m.detail.to_string(), src, ALGO_VERSION, fp],
                )?;
            }
            for t in &sd.td {
                let rid = id_res(&run_id, "td", &sd.sat, t.epoch, &t.band);
                let status = if t.value.is_some() { "ok" } else { &t.status };
                let src = source_key(&run_id, "td", &sd.sat, t.epoch);
                conn.execute(
                    "INSERT INTO residual(id,run_id,project_id,sat,band,epoch_idx,detector,value,unit,status,freshness,detail_json,source_key,algo_version,param_fingerprint)
                     VALUES(?1,?2,?3,?4,?5,?6,'td',?7,'cycle',?8,'current',?9,?10,?11,?12)",
                    params![rid, run_id, project, sd.sat, t.band, t.epoch, t.value, status, t.detail.to_string(), src, ALGO_VERSION, fp],
                )?;
            }
            let _ = (l1, l2);
        }

        // ---- clock candidates (common mode), stored once per clock epoch ----
        for (ep, cf) in &clocks {
            let cid = id_cand(&run_id, "RECEIVER", *ep, "clock", "clk".to_string(), "clk".to_string());
            conn.execute(
                "INSERT INTO candidate(id,run_id,project_id,sat,epoch_idx,n1,n2,kind,gf_error_m,mw_error_cy,td_error_cy,score,detector_votes_json,status,reject_reason,freshness,source_key,algo_version,param_fingerprint)
                 VALUES(?1,?2,?3,'RECEIVER',?4,NULL,NULL,'clock',NULL,NULL,NULL,100.0,?5,'accepted',NULL,'current',?6,?7,?8)",
                params![cid, run_id, project, ep, cf.detail.to_string(),
                    format!("clock-source:{}", short_hash(&format!("{run_id}|clock|{ep}"))), ALGO_VERSION, fp],
            )?;
        }

        // ---- per-satellite candidates + LLI flags ----
        for sd in sats.values() {
            let l1 = wavelength(freqs, &sd.gnss, "L1");
            let l2 = wavelength(freqs, &sd.gnss, "L2");
            for ep in sd.by_epoch.keys().copied().collect::<Vec<_>>() {
                if ep <= *times.keys().next().unwrap_or(&0) {
                    continue;
                }
                let f = findings_at(sd, ep);
                let clock_here = clocks.contains_key(&ep);

                if f.lli_set && !clock_here {
                    // LLI raises a candidate regardless; it may be a false alarm.
                    let lid = id_cand(&run_id, &sd.sat, ep, "lli_false_positive", "lli".into(), "lli".into());
                    // Decide rejection: no detector supports a non-zero slip.
                    let hyps = hypotheses(&f, l1, l2, params);
                    let nonzero = hyps.iter().any(|h| h.n1 != 0 || h.n2 != 0);
                    let (status, reason) = if nonzero {
                        ("proposed", Some("LLI set; detectors also find a non-zero slip".to_string()))
                    } else {
                        ("rejected", Some("LLI set but no phase break in GF/MW/TD -> false positive".to_string()))
                    };
                    conn.execute(
                        "INSERT INTO candidate(id,run_id,project_id,sat,epoch_idx,n1,n2,kind,gf_error_m,mw_error_cy,td_error_cy,score,detector_votes_json,status,reject_reason,freshness,source_key,algo_version,param_fingerprint)
                         VALUES(?1,?2,?3,?4,?5,NULL,NULL,'lli_false_positive',NULL,NULL,NULL,0.0,?6,?7,?8,'current',?9,?10,?11)",
                        params![lid, run_id, project, sd.sat, ep,
                            serde_json::json!({"lli_bits": f.lli_bits, "detectors":"independent"}).to_string(),
                            status, reason,
                            source_key(&run_id, "lli", &sd.sat, ep), ALGO_VERSION, fp],
                    )?;
                }

                if clock_here {
                    // clock epoch: skip per-satellite slip classification here
                    continue;
                }

                let hyps = hypotheses(&f, l1, l2, params);
                if hyps.is_empty() {
                    continue;
                }
                let ambiguous = hyps.len() > 1;
                if ambiguous {
                    ambiguous_keys.insert(format!("{}@{}", sd.sat, ep));
                }
                for h in &hyps {
                    let cid = id_cand(&run_id, &sd.sat, ep, "slip", h.n1.to_string(), h.n2.to_string());
                    let mut status = if ambiguous { "ambiguous".to_string() } else { "proposed".to_string() };
                    let mut reject_reason: Option<String> = None;
                    if let Some((a1, a2)) = accepted.get(&(sd.sat.clone(), ep)) {
                        if *a1 == h.n1 && *a2 == h.n2 {
                            status = "accepted".to_string();
                            accepted_slips.entry(format!("{}:L1", sd.sat)).or_default().insert(ep);
                            accepted_slips.entry(format!("{}:L2", sd.sat)).or_default().insert(ep);
                        } else {
                            status = "superseded".to_string();
                            reject_reason = Some("another hypothesis was accepted".to_string());
                        }
                    }
                    if rejected.contains(&(sd.sat.clone(), ep, h.n1, h.n2)) {
                        status = "rejected".to_string();
                        reject_reason = Some("rejected by reviewer".to_string());
                    }
                    // (0,0) no-slip hypothesis
                    if h.n1 == 0 && h.n2 == 0 && ambiguous == false {
                        // record as a benign no-slip confirmation, low score priority
                    }
                    conn.execute(
                        "INSERT INTO candidate(id,run_id,project_id,sat,epoch_idx,n1,n2,kind,gf_error_m,mw_error_cy,td_error_cy,score,detector_votes_json,status,reject_reason,freshness,source_key,algo_version,param_fingerprint)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,'slip',?8,?9,?10,?11,?12,?13,?14,'current',?15,?16,?17)",
                        params![cid, run_id, project, sd.sat, ep, h.n1, h.n2,
                            h.gf_error_m, h.mw_error_cy, h.td_error_cy, h.score,
                            h.votes.to_string(), status, reject_reason,
                            source_key(&run_id, "slip", &sd.sat, ep), ALGO_VERSION, fp],
                    )?;
                }
            }
        }

        // ---- arcs (after accepted slips known) ----
        let clock_set: BTreeSet<i64> = clocks.keys().copied().collect();
        let arcs = build_arcs(&run_id, &sats, &clock_set, &accepted_slips);
        for a in &arcs {
            let aid = id_arc(&run_id, &a.sat, &a.band, a.start);
            conn.execute(
                "INSERT INTO arc(id,run_id,project_id,sat,band,start_epoch,end_epoch,reason,source_key,status)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'current')",
                params![aid, run_id, project, a.sat, a.band, a.start, a.end, a.reason, a.source_key],
            )?;
        }

        // ---- offset layer (repair stored ONLY here; raw obs untouched) ----
        for ((sat, ep), (n1, n2)) in &accepted {
            for (band, n) in [("L1", n1), ("L2", n2)] {
                if *n != 0 {
                    let oid = format!("off-{}", short_hash(&format!("{run_id}|{sat}|{ep}|{band}|{n}")));
                    conn.execute(
                        "INSERT INTO offset_layer(id,project_id,run_id,sat,epoch_idx,band,offset_cy,decision_event_id)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,NULL)",
                        params![oid, project, run_id, sat, ep, band, n],
                    )?;
                }
            }
        }
    }

    // Re-read accepted slips/ambiguous even when run pre-existed.
    let mut acc2: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT sat,epoch_idx,n1,n2 FROM candidate WHERE run_id=?1 AND status='accepted' AND kind='slip'")?;
    let rows: Vec<(String,i64,i64,i64)> = stmt.query_map(params![run_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (sat, ep, n1, n2) in rows {
        let _=(n1,n2);
        acc2.entry(format!("{sat}:L1")).or_default().insert(ep);
        acc2.entry(format!("{sat}:L2")).or_default().insert(ep);
    }

    Ok(RunResult { run_id, arcs: Vec::new(), accepted_slips: acc2, ambiguous_keys })
}

fn source_key(run_id: &str, detector: &str, sat: &str, ep: i64) -> String {
    format!("source:{}", short_hash(&format!("{run_id}|{detector}|{sat}|{ep}")))
}

// Fingerprint of the current effective input set (used to detect whether a run
// is still based on the same direct sources).
pub fn input_fingerprint(obs: &[ObsRow]) -> String {
    let mut s = String::new();
    for o in obs {
        s.push_str(&format!("{}|{}|{}|{}|{}\n", o.id, o.epoch_idx, o.sat, o.band, o.code_phase));
    }
    short_hash(&s)
}
