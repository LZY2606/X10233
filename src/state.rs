//! Event-sourced state machine and deterministic materialization.
//!
//! Raw submissions are immutable versions; parameter revisions and human
//! decisions are append-only events. Derived objects are content-addressed:
//! a local source change expires only the results that actually depend on it,
//! and expired rows stay viewable but are never reported as current.

use crate::detect::*;
use crate::domain::reasons;
use crate::domain::*;
use crate::ingest::fingerprint_bundle;
use crate::vclock::{Rng, VClock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn params_fp(p: &Params) -> String {
    let canon = serde_json::to_string(p).unwrap();
    short_hash(&canon)
}

// ---------------------------------------------------------------------------
// raw versions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawVersionRec {
    pub version: u32,
    pub parent_version: Option<u32>,
    pub fingerprint: String,
    pub kind: String,
    pub sources: Vec<String>,
    pub committed_event_id: String,
    pub bundle: RawBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRec {
    pub boundary_id: String,
    pub candidate_id: Option<String>,
    pub decision: String,
    pub offsets: BTreeMap<String, i64>,
    pub event_id: String,
    pub actor: String,
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// materialized view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prov {
    pub id: String,
    pub algo_version: String,
    pub params_fp: String,
    pub raw_version: u32,
    pub basis_event_id: String,
    pub direct_sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewCandidate {
    pub id: String,
    pub detector: String,
    pub bands: Vec<u8>,
    pub offset_cycles: BTreeMap<String, i64>,
    pub residual: f64,
    pub residual_unit: String,
    pub score: f64,
    pub tie_rand: f64,
    pub status: String,
    pub rejection_reason: Option<String>,
    pub note: Option<String>,
    pub prov: Prov,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewResult {
    pub id: String,
    pub detector: String,
    pub bands: Vec<u8>,
    pub residual: f64,
    pub residual_unit: String,
    pub fired: bool,
    pub unknown: bool,
    pub reason: String,
    pub candidates: Vec<ViewCandidate>,
    pub prov: Prov,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewBandResult {
    pub td: ViewResult,
    pub lli: ViewResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewBoundary {
    pub id: String,
    pub sat: String,
    pub from_epoch_no: u32,
    pub to_epoch_no: u32,
    pub dt_seconds: f64,
    pub missing_bands: Vec<u8>,
    pub lli_bands: Vec<u8>,
    pub clock_suspect: bool,
    pub per_band: BTreeMap<String, ViewBandResult>,
    pub combos: Vec<ViewResult>,
    pub decision: String,
    pub adopted_offsets: BTreeMap<String, i64>,
    pub decided_by_event: Option<String>,
    pub note: Option<String>,
    pub prov: Prov,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewArc {
    pub id: String,
    pub sat: String,
    pub band: u8,
    pub start_epoch_no: u32,
    pub end_epoch_no: u32,
    pub epoch_count: usize,
    pub ended_by: String,
    pub prov: Prov,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewClock {
    pub id: String,
    pub at_epoch_no: u32,
    pub is_clock_jump: bool,
    pub median_step_cycles: f64,
    pub agreeing_sats: usize,
    pub total_sats: usize,
    pub reason: String,
    pub raw_event_sources: Vec<String>,
    pub prov: Prov,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewMeasurement {
    pub id: String,
    pub sat: String,
    pub obs_code: String,
    pub freq_no: u8,
    pub range_raw: Option<String>,
    pub phase_raw: Option<String>,
    pub doppler_raw: Option<String>,
    pub lli: Option<u8>,
    pub snr_raw: Option<String>,
    pub snr_value: Option<f64>,
    pub unit: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewEpoch {
    pub id: String,
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub dt_seconds: f64,
    pub uneven: bool,
    pub recv_clk_offset_raw: String,
    pub recv_clk_offset_s: Option<f64>,
    pub clock_event_recorded: bool,
    pub measurements: Vec<ViewMeasurement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewSatState {
    pub epoch_no: u32,
    pub sat: String,
    pub x: RawValue,
    pub y: RawValue,
    pub z: RawValue,
    pub sat_clock_bias: RawValue,
    pub datum: Datum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub project_id: String,
    pub at_event_id: String,
    pub at_event_seq: i64,
    pub branch: String,
    pub algo_version: String,
    pub params: Params,
    pub params_fp: String,
    pub raw_version: Option<u32>,
    pub raw_fingerprint: Option<String>,
    pub station: String,
    pub marker: String,
    pub receiver: String,
    pub receiver_pos: Vec<f64>,
    pub receiver_pos_unit: String,
    pub constellation: String,
    pub epochs: Vec<ViewEpoch>,
    pub sat_states: Vec<ViewSatState>,
    pub raw_clock_events: Vec<RawClockEvent>,
    pub clocks: Vec<ViewClock>,
    pub boundaries: Vec<ViewBoundary>,
    pub arcs: Vec<ViewArc>,
    pub diagnostics: Vec<String>,
}

// ---------------------------------------------------------------------------
// replay engine
// ---------------------------------------------------------------------------

pub struct Engine {
    pub events: Vec<Event>,
}

impl Engine {
    pub fn new(events: Vec<Event>) -> Self {
        Self { events }
    }

    /// Replay the audit log up to and inclusive of `at_seq` (or all),
    /// following parent links to reconstruct the dependency versions that
    /// were visible at that point rather than overwriting with newest math.
    pub fn materialize(&self, project_id: &str, at_seq: Option<i64>) -> Snapshot {
        let mut selected: Vec<&Event> = Vec::new();
        let mut head: Option<String> = None;
        for ev in &self.events {
            if let Some(limit) = at_seq {
                if ev.seq > limit {
                    break;
                }
            }
            if ev.parent_event_id.is_none() {
                selected.clear();
                head = Some(ev.id.clone());
            } else if head.is_some() && ev.parent_event_id == head {
                head = Some(ev.id.clone());
            } else {
                continue;
            }
            selected.push(ev);
        }

        let mut params = Params::default();
        let mut raw_versions: Vec<RawVersionRec> = Vec::new();
        let mut decisions: Vec<DecisionRec> = Vec::new();
        let mut diagnostics: Vec<String> = Vec::new();
        let mut latest_event: Option<&Event> = None;
        let mut branch = "main".to_string();

        for ev in &selected {
            latest_event = Some(*ev);
            branch = ev.branch.clone();
            match ev.kind {
                EventKind::ProjectCreated => {}
                EventKind::RawCommitted => {
                    let bundle: RawBundle =
                        serde_json::from_value(ev.payload["bundle"].clone()).expect("bundle");
                    let parent_version = ev.payload["parent_version"].as_u64().map(|v| v as u32);
                    let kind = ev.payload["kind"].as_str().unwrap_or("full").to_string();
                    let sources = ev.payload["sources"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    if let Some(diag) = ev.payload.get("diagnostics").and_then(|v| v.as_array()) {
                        for d in diag {
                            if let Some(s) = d.as_str() {
                                diagnostics.push(format!("[{}] {s}", ev.id));
                            }
                        }
                    }
                    let version = raw_versions.len() as u32 + 1;
                    raw_versions.push(RawVersionRec {
                        version,
                        parent_version,
                        fingerprint: fingerprint_bundle(&bundle),
                        kind,
                        sources,
                        committed_event_id: ev.id.clone(),
                        bundle,
                    });
                }
                EventKind::ParamsRevised => {
                    params = serde_json::from_value(ev.payload["params"].clone()).expect("params");
                }
                EventKind::Adjudicated | EventKind::ConflictResolved => {
                    let offsets = ev
                        .payload["offsets"]
                        .as_object()
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, v)| v.as_i64().map(|x| (k.clone(), x)))
                                .collect()
                        })
                        .unwrap_or_default();
                    decisions.push(DecisionRec {
                        boundary_id: ev.payload["boundary_id"].as_str().unwrap_or("").into(),
                        candidate_id: ev.payload["candidate_id"].as_str().map(String::from),
                        decision: ev.payload["decision"].as_str().unwrap_or("unresolved").into(),
                        offsets,
                        event_id: ev.id.clone(),
                        actor: ev.actor.clone(),
                        note: ev.payload["note"].as_str().map(String::from),
                    });
                }
            }
        }

        let last = latest_event.expect("snapshot requires events");
        let raw = raw_versions.last();
        let pfp = params_fp(&params);
        let mut snap = Snapshot {
            project_id: project_id.into(),
            at_event_id: last.id.clone(),
            at_event_seq: last.seq,
            branch,
            algo_version: ALGO_VERSION.into(),
            params_fp: pfp.clone(),
            params,
            raw_version: raw.map(|r| r.version),
            raw_fingerprint: raw.map(|r| r.fingerprint.clone()),
            station: raw.map(|r| r.bundle.station.clone()).unwrap_or_default(),
            marker: raw.map(|r| r.bundle.marker.clone()).unwrap_or_default(),
            receiver: raw.map(|r| r.bundle.receiver.clone()).unwrap_or_default(),
            receiver_pos: raw.map(|r| r.bundle.receiver_pos.clone()).unwrap_or_default(),
            receiver_pos_unit: raw.map(|r| r.bundle.receiver_pos_unit.as_str().to_string()).unwrap_or_else(|| Unit::Meter.as_str().into()),
            constellation: raw.map(|r| r.bundle.constellation.clone()).unwrap_or_default(),
            epochs: Vec::new(),
            sat_states: Vec::new(),
            raw_clock_events: raw.map(|r| r.bundle.clock_events.clone()).unwrap_or_default(),
            clocks: Vec::new(),
            boundaries: Vec::new(),
            arcs: Vec::new(),
            diagnostics,
        };

        if let Some(rv) = raw {
            self.materialize_derived(&mut snap, rv, &decisions, last);
        }
        snap
    }

    fn materialize_derived(
        &self,
        snap: &mut Snapshot,
        rv: &RawVersionRec,
        decisions: &[DecisionRec],
        last: &Event,
    ) {
        let p = &snap.params;
        let b = &rv.bundle;
        let set = build_series(&b.station, &b.epochs);

        // ---- epochs (raw view, verbatim strings retained) ----
        for (i, e) in b.epochs.iter().enumerate() {
            let dt = if i == 0 {
                0.0
            } else {
                set.epochs[i].seconds - set.epochs[i - 1].seconds
            };
            let mut meas = Vec::new();
            for m in &e.measurements {
                let diag = [
                    m.range.as_ref().and_then(|v| v.diagnostic.clone()),
                    m.phase.as_ref().and_then(|v| v.diagnostic.clone()),
                    m.doppler.as_ref().and_then(|v| v.diagnostic.clone()),
                    m.snr.as_ref().and_then(|v| v.diagnostic.clone()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(";");
                meas.push(ViewMeasurement {
                    id: format!("m:{}:{}:{}", e.epoch_no, m.sat, m.obs_code),
                    sat: m.sat.clone(),
                    obs_code: m.obs_code.clone(),
                    freq_no: m.freq_no,
                    range_raw: m.range.as_ref().map(|v| v.raw.clone()),
                    phase_raw: m.phase.as_ref().map(|v| v.raw.clone()),
                    doppler_raw: m.doppler.as_ref().map(|v| v.raw.clone()),
                    lli: m.lli,
                    snr_raw: m.snr.as_ref().map(|v| v.raw.clone()),
                    snr_value: m.snr.as_ref().and_then(|v| v.value),
                    unit: "raw(m|cycle|m/s|dbhz)".into(),
                    diagnostic: if diag.is_empty() { None } else { Some(diag) },
                });
            }
            meas.sort_by(|x, y| x.sat.cmp(&y.sat).then(x.obs_code.cmp(&y.obs_code)));
            let clock_event = b
                .clock_events
                .iter()
                .any(|c| c.epoch_no == e.epoch_no);
            snap.epochs.push(ViewEpoch {
                id: format!("ep:{}", e.epoch_no),
                epoch_no: e.epoch_no,
                gpst_iso: e.gpst_iso.clone(),
                dt_seconds: dt,
                uneven: i > 0 && (dt - 1.0).abs() > 1e-6,
                recv_clk_offset_raw: e.recv_clk_offset_raw.raw.clone(),
                recv_clk_offset_s: e.recv_clk_offset_raw.value,
                clock_event_recorded: clock_event,
                measurements: meas,
            });
        }
        snap.sat_states = b.sat_states.clone().into_iter().map(|s| ViewSatState {
            epoch_no: s.epoch_no,
            sat: s.sat,
            x: s.x,
            y: s.y,
            z: s.z,
            sat_clock_bias: s.sat_clock_bias,
            datum: s.datum,
        }).collect();

        // ---- clock findings (project level; separated from slips) ----
        for idx in 1..set.epochs.len() {
            let v = detect_clock_jump(&set, idx, p);
            if !v.is_clock_jump && !set.epochs[idx..].iter().any(|_| false) {
                // still record a finding row only when something notable or
                // raw evidence exists
            }
            let raw_sources: Vec<String> = b
                .clock_events
                .iter()
                .filter(|c| c.epoch_no == v.at_epoch_no)
                .map(|c| format!("rawclock:e{}", c.epoch_no))
                .collect();
            if v.is_clock_jump || !raw_sources.is_empty() || v.agreeing_sats > 0 {
                snap.clocks.push(ViewClock {
                    id: format!("clk:e{}", v.at_epoch_no),
                    at_epoch_no: v.at_epoch_no,
                    is_clock_jump: v.is_clock_jump,
                    median_step_cycles: v.median_step_cycles,
                    agreeing_sats: v.agreeing_sats,
                    total_sats: v.total_sats,
                    reason: v.reason,
                    raw_event_sources: raw_sources,
                    prov: prov(snap, rv, last, vec![format!("ep:{}", v.at_epoch_no)]),
                });
            }
        }

        // clock epoch lookup
        let clock_epochs: BTreeSet<u32> = snap
            .clocks
            .iter()
            .filter(|c| c.is_clock_jump)
            .map(|c| c.at_epoch_no)
            .collect();

        // ---- per-satellite boundaries ----
        let mut sats: Vec<String> = set.bands.iter().map(|x| x.sat.clone()).collect();
        sats.sort();
        sats.dedup();

        let mut rng = Rng::seeded(p.seed);
        let mut boundaries: Vec<ViewBoundary> = Vec::new();

        for sat in sats {
            let mut bands: Vec<u8> = set
                .bands
                .iter()
                .filter(|x| x.sat == sat)
                .map(|x| x.band)
                .collect();
            bands.sort();
            bands.dedup();

            for idx in 1..set.epochs.len() {
                let to = set.epochs[idx].epoch_no;
                let from = set.epochs[idx - 1].epoch_no;
                let dt = set.epochs[idx].seconds - set.epochs[idx - 1].seconds;
                let any_present_now = bands.iter().any(|bd| {
                    set.bands
                        .iter()
                        .find(|x| x.sat == sat && x.band == *bd)
                        .map(|x| x.points[idx].is_some())
                        .unwrap_or(false)
                });
                if !any_present_now {
                    continue; // satellite absent entirely at this epoch
                }

                let bid = boundary_id(&rv.fingerprint, &sat, from, to);
                let mut per_band = BTreeMap::new();
                let mut combos = Vec::new();
                let mut missing_bands = Vec::new();
                let mut lli_bands = Vec::new();
                let clock_suspect = clock_epochs.contains(&to);

                for &bd in &bands {
                    let bs = set
                        .bands
                        .iter()
                        .find(|x| x.sat == sat && x.band == bd)
                        .unwrap();
                    let present_now = bs.points[idx].is_some();
                    let present_prev = bs.points[idx - 1].is_some();

                    let td = if !present_now || !present_prev {
                        missing_bands.push(bd);
                        unknown_result(Detector::TimeDifference, vec![bd], reasons::FREQUENCY_MISSING, snap, rv, last, &bid)
                    } else {
                        let mut out = time_difference(&set, bs, idx, p.td_gate_cycles);
                        if clock_suspect && out.fired {
                            out.fired = false;
                            out.reason = reasons::CLOCK_JUMP_SUPPRESSED.into();
                            out.candidates = vec![zero_int(vec![bd])];
                        }
                        view_result(&out, &bid, snap, rv, last, &mut rng)
                    };

                    let lli_out = if !present_now {
                        unknown_result(Detector::Lli, vec![bd], reasons::FREQUENCY_MISSING, snap, rv, last, &bid)
                    } else {
                        let raw_td = DetectorOutput {
                            detector: Detector::TimeDifference,
                            fired: td.fired,
                            unknown: td.unknown,
                            residual: td.residual,
                            unit: parse_unit(&td.residual_unit),
                            reason: td.reason.clone(),
                            candidates: td.candidates.iter().map(int_from_view).collect(),
                        };
                        let out = lli_flag(bs, idx, &raw_td);
                        view_result(&out, &bid, snap, rv, last, &mut rng)
                    };
                    if lli_out.fired {
                        lli_bands.push(bd);
                    }
                    per_band.insert(bd.to_string(), ViewBandResult { td, lli: lli_out });
                }

                // multi-frequency combos on consecutive band pairs
                for win in bands.windows(2) {
                    let (b1, b2) = (win[0], win[1]);
                    let gf = ionosphere_gf(&set, &sat, b1, b2, idx, p);
                    combos.push(view_result(&gf, &bid, snap, rv, last, &mut rng));
                    let mw = melbourne_wubbena(&set, &sat, b1, b2, idx, p);
                    combos.push(view_result(&mw, &bid, snap, rv, last, &mut rng));
                }

                boundaries.push(ViewBoundary {
                    id: bid,
                    sat: sat.clone(),
                    from_epoch_no: from,
                    to_epoch_no: to,
                    dt_seconds: dt,
                    missing_bands,
                    lli_bands,
                    clock_suspect,
                    per_band,
                    combos,
                    decision: Decision::Unresolved.as_str().into(),
                    adopted_offsets: BTreeMap::new(),
                    decided_by_event: None,
                    note: None,
                    prov: prov(snap, rv, last, vec![format!("ep:{from}"), format!("ep:{to}")]),
                });
            }
        }

        // ---- decisions (offset layer only) ----
        for d in decisions {
            if let Some(bnd) = boundaries.iter_mut().find(|x| x.id == d.boundary_id) {
                bnd.decision = d.decision.clone();
                bnd.adopted_offsets = d.offsets.clone();
                bnd.decided_by_event = Some(d.event_id.clone());
                bnd.note = d.note.clone();
            }
        }

        // mark rejected non-adopted candidates
        for bnd in &mut boundaries {
            let adopted: &BTreeMap<String, i64> = &bnd.adopted_offsets;
            for (_, br) in bnd.per_band.iter_mut() {
                reject_others(&mut br.td.candidates, adopted, bnd.decision);
            }
            for combo in bnd.combos.iter_mut() {
                reject_others(&mut combo.candidates, adopted, bnd.decision);
            }
        }

        // ---- arcs ----
        snap.boundaries = boundaries;
        snap.arcs = compute_arcs(&set, snap, rv, last);
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn boundary_id(raw_fp: &str, sat: &str, from: u32, to: u32) -> String {
    format!("bnd:{}", short_hash(&format!("{raw_fp}|{sat}|{from}|{to}"))[..12].to_string())
}

fn prov(snap: &Snapshot, rv: &RawVersionRec, last: &Event, sources: Vec<String>) -> Prov {
    Prov {
        id: format!("prov:{}", short_hash(&format!("{}|{}|{}|{}", ALGO_VERSION, snap.params_fp, rv.version, sources.join(",")))[..12].to_string()),
        algo_version: ALGO_VERSION.into(),
        params_fp: snap.params_fp.clone(),
        raw_version: rv.version,
        basis_event_id: last.id.clone(),
        direct_sources: sources,
    }
}

fn zero_int(bands: Vec<u8>) -> IntCandidate {
    IntCandidate {
        bands,
        offset_cycles: vec![0],
        residual: 0.0,
        unit: Unit::Cycle,
        note: None,
    }
}

fn parse_unit(s: &str) -> Unit {
    match s {
        "m" => Unit::Meter,
        "cycle" => Unit::Cycle,
        "m/s" => Unit::MetersPerSecond,
        "Hz" => Unit::Hertz,
        "s" => Unit::Second,
        _ => Unit::Dimensionless,
    }
}

fn int_from_view(c: &ViewCandidate) -> IntCandidate {
    let mut offsets = vec![0i64; c.bands.len()];
    for (i, b) in c.bands.iter().enumerate() {
        offsets[i] = *c.offset_cycles.get(&b.to_string()).unwrap_or(&0);
    }
    IntCandidate {
        bands: c.bands.clone(),
        offset_cycles: offsets,
        residual: c.residual,
        unit: parse_unit(&c.residual_unit),
        note: c.note.clone(),
    }
}

fn unknown_result(
    det: Detector,
    bands: Vec<u8>,
    reason: &str,
    snap: &Snapshot,
    rv: &RawVersionRec,
    last: &Event,
    bid: &str,
) -> ViewResult {
    ViewResult {
        id: format!("res:{}", short_hash(&format!("{bid}|{}", det.as_str()))[..14].to_string()),
        detector: det.as_str().into(),
        bands: bands.clone(),
        residual: f64::NAN,
        residual_unit: match det {
            Detector::IonosphereFree => Unit::Meter.as_str().into(),
            Detector::Lli => Unit::Dimensionless.as_str().into(),
            _ => Unit::Cycle.as_str().into(),
        },
        fired: false,
        unknown: true,
        reason: reason.into(),
        candidates: Vec::new(),
        prov: prov(snap, rv, last, vec![bid.into()]),
    }
}

fn view_result(
    out: &DetectorOutput,
    bid: &str,
    snap: &Snapshot,
    rv: &RawVersionRec,
    last: &Event,
    rng: &mut Rng,
) -> ViewResult {
    let res_id = format!(
        "res:{}",
        short_hash(&format!("{bid}|{}", out.detector.as_str()))[..14].to_string()
    );

    // score: closeness of residual to an integer-consistent fit
    let mut scored: Vec<(IntCandidate, f64, f64)> = out
        .candidates
        .iter()
        .map(|c| {
            let magnitude = c.offset_cycles.iter().map(|x| x.unsigned_abs() as f64).sum::<f64>();
            let score = 1.0 / (1.0 + c.residual.abs() + 0.001 * magnitude);
            (c.clone(), score, rng.unit())
        })
        .collect();
    // best residual defines the legal group; equal residual => equal split
    let best_res = scored
        .iter()
        .map(|(c, _, _)| (c.residual.abs() * 1e9).round() / 1e9)
        .fold(f64::INFINITY, f64::min);
    let equal_count = scored
        .iter()
        .filter(|(c, _, _)| ((c.residual.abs() * 1e9).round() / 1e9 - best_res).abs() < 1e-7)
        .count();

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut candidates = Vec::new();
    for (c, score, tie_rand) in scored {
        let is_best = ((c.residual.abs() * 1e9).round() / 1e9 - best_res).abs() < 1e-7;
        let status = if !is_best {
            CandidateStatus::Rejected.as_str()
        } else if equal_count > 1 {
            CandidateStatus::Underdetermined.as_str()
        } else {
            CandidateStatus::Unique.as_str()
        };
        let offsets: BTreeMap<String, i64> = c
            .bands
            .iter()
            .zip(c.offset_cycles.iter())
            .map(|(b, o)| (b.to_string(), *o))
            .collect();
        let cid = format!(
            "cand:{}",
            short_hash(&format!(
                "{res_id}|{}|{}",
                c.bands.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(","),
                c.offset_cycles.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
            ))[..14].to_string()
        );
        let rejection = if !is_best {
            Some("worse_fit_than_legal_alternative".to_string())
        } else {
            None
        };
        candidates.push(ViewCandidate {
            id: cid,
            detector: out.detector.as_str().into(),
            bands: c.bands,
            offset_cycles: offsets,
            residual: c.residual,
            residual_unit: c.unit.as_str().into(),
            score,
            tie_rand,
            status: status.into(),
            rejection_reason: rejection,
            note: c.note,
            prov: prov(snap, rv, last, vec![bid.into()]),
        });
    }

    ViewResult {
        id: res_id,
        detector: out.detector.as_str().into(),
        bands: out
            .candidates
            .first()
            .map(|c| c.bands.clone())
            .unwrap_or_default(),
        residual: out.residual,
        residual_unit: out.unit.as_str().into(),
        fired: out.fired,
        unknown: out.unknown,
        reason: out.reason.clone(),
        candidates,
        prov: prov(snap, rv, last, vec![bid.into()]),
    }
}

fn reject_others(cands: &mut [ViewCandidate], adopted: &BTreeMap<String, i64>, decision: &str) {
    if decision != Decision::Accepted.as_str() && decision != Decision::Rejected.as_str() {
        return;
    }
    for c in cands.iter_mut() {
        let matches = c.offset_cycles.iter().all(|(k, v)| adopted.get(k) == Some(v))
            && adopted.len() == c.offset_cycles.len();
        if decision == Decision::Accepted.as_str() && !matches && c.status != CandidateStatus::Rejected.as_str() {
            c.status = CandidateStatus::Rejected.as_str();
            c.rejection_reason = Some("not_adopted:alternative_offset".into());
        }
        if decision == Decision::Rejected.as_str() && matches {
            c.status = CandidateStatus::Rejected.as_str();
            c.rejection_reason = Some("rejected_by_reviewer".into());
        }
    }
}

/// Arcs split at physical frequency gaps and at *accepted* non-zero offset
/// repairs (the offset layer never mutates raw phase).
fn compute_arcs(
    set: &SeriesSet,
    snap: &Snapshot,
    rv: &RawVersionRec,
    last: &Event,
) -> Vec<ViewArc> {
    let mut arcs = Vec::new();
    let by_key: HashMap<(String, u32, u32), &ViewBoundary> = snap
        .boundaries
        .iter()
        .map(|b| ((b.sat.clone(), b.from_epoch_no, b.to_epoch_no), b))
        .collect();

    for bs in &set.bands {
        let present: Vec<(usize, &Point)> = bs
            .points
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.as_ref().map(|p| (i, p)))
            .collect();
        if present.is_empty() {
            continue;
        }
        let mut run_start_idx = present[0].0;
        let mut run_count = 1usize;

        for w in present.windows(2) {
            let (i0, p0) = w[0];
            let (i1, p1) = w[1];
            let physical_gap = i1 != i0 + 1;
            let bnd = by_key.get(&(bs.sat.clone(), p0.epoch_no, p1.epoch_no)).copied();
            let accepted_nonzero = bnd
                .map(|b| {
                    b.decision == Decision::Accepted.as_str()
                        && b.adopted_offsets.values().any(|v| *v != 0)
                })
                .unwrap_or(false);

            if physical_gap || accepted_nonzero {
                let end_ep = set.epochs[i0].epoch_no;
                let reason = if accepted_nonzero {
                    "accepted_repair_offset".to_string()
                } else {
                    reasons::FREQUENCY_MISSING.to_string()
                };
                arcs.push(make_arc(
                    snap,
                    rv,
                    last,
                    bs,
                    set.epochs[run_start_idx].epoch_no,
                    end_ep,
                    run_count,
                    reason,
                ));
                run_start_idx = i1;
                run_count = 0;
            }
            run_count += 1;
        }

        let last_idx = present.last().unwrap().0;
        arcs.push(make_arc(
            snap,
            rv,
            last,
            bs,
            set.epochs[run_start_idx].epoch_no,
            set.epochs[last_idx].epoch_no,
            run_count,
            "end_of_data".into(),
        ));
    }
    arcs.sort_by(|a, b| {
        a.sat
            .cmp(&b.sat)
            .then(a.band.cmp(&b.band))
            .then(a.start_epoch_no.cmp(&b.start_epoch_no))
    });
    arcs
}

#[allow(clippy::too_many_arguments)]
fn make_arc(
    snap: &Snapshot,
    rv: &RawVersionRec,
    last: &Event,
    bs: &BandSeries,
    start: u32,
    end: u32,
    count: usize,
    ended_by: String,
) -> ViewArc {
    let id = format!(
        "arc:{}",
        short_hash(&format!("{}|{}|{}|{}|{}", rv.fingerprint, bs.sat, bs.band, start, end))[..12].to_string()
    );
    ViewArc {
        id,
        sat: bs.sat.clone(),
        band: bs.band,
        start_epoch_no: start,
        end_epoch_no: end,
        epoch_count: count.max(1),
        ended_by,
        prov: prov(snap, rv, last, vec![format!("ep:{start}"), format!("ep:{end}")]),
    }
}

// ---------------------------------------------------------------------------
// raw patch merge (additive only; existing raw text can never be overwritten)
// ---------------------------------------------------------------------------

pub fn merge_patch(base: &RawBundle, patch: &RawBundle, mut diags: Vec<String>) -> (RawBundle, Vec<String>) {
    let mut out = base.clone();
    if out.station.is_empty() {
        out.station = patch.station.clone();
    }
    for e in &patch.epochs {
        if let Some(existing) = out.epochs.iter().find(|x| x.epoch_no == e.epoch_no) {
            // same-numbered epoch may only gain previously absent frequencies
            let mut merged = existing.clone();
            for m in &e.measurements {
                let occupied = merged.measurements.iter().any(|x| x.sat == m.sat && x.obs_code == m.obs_code);
                if occupied {
                    diags.push(format!(
                        "epoch {} sat {} code {} already present in raw layer; overwrite refused",
                        e.epoch_no, m.sat, m.obs_code
                    ));
                } else {
                    merged.measurements.push(m.clone());
                }
            }
            let pos = out.epochs.iter().position(|x| x.epoch_no == e.epoch_no).unwrap();
            out.epochs[pos] = merged;
        } else {
            out.epochs.push(e.clone());
        }
    }
    out.epochs.sort_by_key(|e| e.epoch_no);
    for c in &patch.clock_events {
        if !out.clock_events.iter().any(|x| x.epoch_no == c.epoch_no && x.kind == c.kind) {
            out.clock_events.push(c.clone());
        }
    }
    for s in &patch.sat_states {
        if !out.sat_states.iter().any(|x| x.epoch_no == s.epoch_no && x.sat == s.sat) {
            out.sat_states.push(s.clone());
        }
    }
    out.sat_states.sort_by(|a, b| a.epoch_no.cmp(&b.epoch_no).then(a.sat.cmp(&b.sat)));
    (out, diags)
}

/// Latest event reachable along the `project_created -> ...` chain.
pub fn head_event(events: &[Event]) -> Option<&Event> {
    let mut head = events.first();
    for ev in events.iter().skip(1) {
        if let Some(parent) = &ev.parent_event_id {
            if head.map(|h| h.id == *parent).unwrap_or(false) {
                head = Some(ev);
            }
        } else if ev.kind == EventKind::ProjectCreated {
            head = Some(ev);
        }
    }
    head
}

pub fn branch_chains(events: &[Event]) -> Vec<Vec<Event>> {
    let mut chains: Vec<Vec<Event>> = Vec::new();
    for ev in events {
        if ev.kind == EventKind::ProjectCreated {
            chains.push(vec![ev.clone()]);
            continue;
        }
        let mut attached = false;
        for chain in chains.iter_mut() {
            if chain.last().map(|h| h.id == ev.parent_event_id.clone().unwrap_or_default()).unwrap_or(false) {
                chain.push(ev.clone());
                attached = true;
                break;
            }
        }
        if !attached {
            // new branch rooted at its parent event
            if let Some(pid) = &ev.parent_event_id {
                let mut chain: Vec<Event> = events.iter().filter(|e| e.id == *pid).cloned().collect();
                chain.push(ev.clone());
                chains.push(chain);
            }
        }
    }
    chains
}

/// All events visible on the chain ending at `tip_id`.
pub fn chain_until(events: &[Event], tip_id: &str) -> Vec<Event> {
    let mut by_id: HashMap<&str, &Event> = HashMap::new();
    for e in events {
        by_id.insert(e.id.as_str(), e);
    }
    let mut out = Vec::new();
    let mut cursor = tip_id;
    loop {
        let Some(e) = by_id.get(cursor).copied() else { break };
        out.push(e.clone());
        match &e.parent_event_id {
            Some(p) => cursor = p.as_str(),
            None => break,
        }
    }
    out.reverse();
    out
}
