//! In-memory projection of the event log.
//!
//! Replaying events in order reconstructs the active raw set, current
//! parameters, accepted offset layer, conflicts and undo state. Derived
//! objects (arcs, candidates, residuals) are recomputed with stable logical
//! identities so local input change invalidates only dependent results.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::event::{Event, EventPayload, RawBatch};
use crate::domain::math::{nearest_int, Bands};
use crate::model::boundary;
use crate::model::{
    ArcRecord, Candidate, CandidateStatus, ClockEvent, ClockEventType, DecisionKind,
    DecisionRecord, DetectorParams, DetectorVote, Id, Observation,
    OffsetHypothesis, Provenance, Rejection, ResidualPoint, SatState, ALGO_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreqStatus {
    Present,
    Missing,
}

/// Active observation view: keyed by (sv, epoch_raw, code) -> obs.
pub struct ProjectState {
    pub project_id: Id,
    pub name: String,
    pub station: String,
    pub batches: Vec<RawBatch>,
    pub obs: HashMap<ObsKey, Observation>,
    pub sat_states: HashMap<(String, String), SatState>,
    pub clock_events: Vec<ClockEvent>,
    pub params: DetectorParams,
    pub params_history: Vec<(String, DetectorParams, String)>,
    /// candidate logical key -> accepted offset layer
    pub accepted: HashMap<String, OffsetLayer>,
    /// candidate logical key -> rejected
    pub rejected: HashSet<String>,
    pub decisions: Vec<(DecisionRecord, Id)>,
    pub conflicts: HashMap<String, ConflictInfo>,
    /// events logically undone (kept in log, rendered ineffective)
    pub undone: HashSet<Id>,
    pub head_event_id: Option<Id>,
    pub head_seq: i64,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self::new(Id("default".into()), String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObsKey {
    pub sv: String,
    pub epoch: String,
    pub code: String,
}

#[derive(Debug, Clone)]
pub struct OffsetLayer {
    pub shift_b1: Option<i64>,
    pub shift_b2: Option<i64>,
    pub decided_by_event: Id,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct ConflictInfo {
    pub prior_event: Id,
    pub later_event: Id,
    pub detail: String,
    pub resolution: Option<Id>,
}

/// Full derived view at one point in history.
#[derive(serde::Serialize)]
pub struct DerivedView {
    pub arcs: Vec<ArcRecord>,
    pub candidates: Vec<Candidate>,
    pub residuals: Vec<ResidualPoint>,
    pub clock_events: Vec<ClockEvent>,
}

impl ProjectState {
    pub fn new(project_id: Id, name: String) -> Self {
        let params = DetectorParams::default();
        ProjectState {
            project_id,
            name,
            station: String::new(),
            batches: vec![],
            obs: HashMap::new(),
            sat_states: HashMap::new(),
            clock_events: vec![],
            params: params.clone(),
            params_history: vec![(params.fingerprint(), params, "initial".to_string())],
            accepted: HashMap::new(),
            rejected: HashSet::new(),
            decisions: vec![],
            conflicts: HashMap::new(),
            undone: HashSet::new(),
            head_event_id: None,
            head_seq: 0,
        }
    }

    pub fn apply(&mut self, ev: &Event) {
        self.head_event_id = Some(ev.id.clone());
        self.head_seq = ev.seq;
        match &ev.payload {
            EventPayload::ProjectCreated { name } => {
                self.name = name.clone();
            }
            EventPayload::RawBatch(batch) => {
                self.ingest_batch(batch);
            }
            EventPayload::ParamRevision { parent_fp, params, note } => {
                self.params = params.clone();
                self.params_history
                    .push((parent_fp.clone(), params.clone(), note.clone()));
            }
            EventPayload::Decision(rec) => self.apply_decision(ev.id.clone(), rec),
            EventPayload::Conflict { candidate_id, prior_event, detail } => {
                let key = candidate_id.0.clone();
                self.conflicts.entry(key).or_insert(ConflictInfo {
                    prior_event: prior_event.clone(),
                    later_event: ev.id.clone(),
                    detail: detail.clone(),
                    resolution: None,
                });
            }
            EventPayload::ConflictResolved { candidate_id, winning_event, .. } => {
                if let Some(c) = self.conflicts.get_mut(&candidate_id.0) {
                    c.resolution = Some(winning_event.clone());
                }
            }
            EventPayload::Undo { target_event, .. } => {
                self.undone.insert(target_event.clone());
            }
        }
    }

    fn ingest_batch(&mut self, batch: &RawBatch) {
        if self.station.is_empty() {
            if let Some(o) = batch.observations.first() {
                self.station = o.station.clone();
            }
        }
        for o in &batch.observations {
            let key = ObsKey { sv: o.sv.clone(), epoch: o.epoch_raw.clone(), code: o.code.clone() };
            // newer batch overrides the active view; the prior obs and batch
            // remain stored for lineage and replay.
            self.obs.insert(key, o.clone());
        }
        for s in &batch.sat_states {
            self.sat_states.insert((s.sv.clone(), s.epoch_raw.clone()), s.clone());
        }
        for c in &batch.declared_clock_events {
            self.clock_events.push(c.clone());
        }
        self.batches.push(batch.clone());
    }

    fn apply_decision(&mut self, by_event: Id, rec: &DecisionRecord) {
        if let Some(cid) = &rec.candidate_id {
            let key = cid.0.clone();
            match rec.kind {
                DecisionKind::Accept => {
                    self.rejected.remove(&key);
                    self.accepted.insert(
                        key,
                        OffsetLayer {
                            shift_b1: rec.shift_b1_cycles,
                            shift_b2: rec.shift_b2_cycles,
                            decided_by_event: by_event.clone(),
                            note: rec.note.clone(),
                        },
                    );
                }
                DecisionKind::Reject => {
                    self.accepted.remove(&key);
                    self.rejected.insert(key);
                }
                DecisionKind::ConflictResolved => {
                    if let Some(c) = self.conflicts.get_mut(&key) {
                        c.resolution = Some(by_event.clone());
                    }
                }
                DecisionKind::Undo => {
                    if let Some(ue) = &rec.undoes_event {
                        self.undone.insert(ue.clone());
                    }
                }
                DecisionKind::ParameterRevision => {}
            }
        }
        self.decisions.push((rec.clone(), by_event));
    }
}

// ---------------------------------------------------------------------------
// Derived computation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct EpochData {
    epoch_raw: String,
    gps: f64,
    /// code -> observation (per satellite), presence known via value.
    obs: HashMap<String, Observation>,
}

#[derive(Debug, Clone)]
struct SvTrack {
    sv: String,
    epochs: Vec<EpochData>,
}

fn phase_codes(b: u8) -> [&'static str; 2] {
    if b == 1 {
        ["L1W", "L1C"]
    } else if b == 2 {
        ["L2W", "L2C"]
    } else {
        ["", ""]
    }
}
fn code_codes(b: u8) -> [&'static str; 2] {
    if b == 1 {
        ["C1W", "C1C"]
    } else if b == 2 {
        ["C2W", "C2C"]
    } else {
        ["", ""]
    }
}

fn pick<'a>(ep: &'a EpochData, codes: &[&'static str]) -> Option<&'a Observation> {
    for c in codes {
        if !c.is_empty() {
            if let Some(o) = ep.obs.get(*c) {
                return Some(o);
            }
        }
    }
    None
}

impl ProjectState {
    fn tracks(&self) -> BTreeMap<String, SvTrack> {
        let mut by_sv: BTreeMap<String, Vec<EpochData>> = BTreeMap::new();
        let mut epochs: BTreeMap<(String, String), EpochData> = BTreeMap::new();
        for o in self.obs.values() {
            if o.station != self.station && !self.station.is_empty() {
                continue;
            }
            let key = (o.sv.clone(), o.epoch_raw.clone());
            let ep = epochs.entry(key).or_insert_with(|| EpochData {
                epoch_raw: o.epoch_raw.clone(),
                gps: o.gps_seconds.unwrap_or(0.0),
                obs: HashMap::new(),
            });
            ep.obs.insert(o.code.clone(), o.clone());
        }
        for ((sv, _epoch), ep) in epochs {
            by_sv.entry(sv).or_default().push(ep);
        }
        let mut out = BTreeMap::new();
        for (sv, mut eps) in by_sv {
            eps.sort_by(|a, b| a.gps.partial_cmp(&b.gps).unwrap());
            eps.dedup_by(|a, b| a.epoch_raw == b.epoch_raw);
            out.insert(sv.clone(), SvTrack { sv, epochs: eps });
        }
        out
    }

    fn sorted_epochs(&self) -> Vec<(String, f64)> {
        let mut e: BTreeMap<String, f64> = BTreeMap::new();
        for o in self.obs.values() {
            if let Some(g) = o.gps_seconds {
                e.entry(o.epoch_raw.clone()).or_insert(g);
            }
        }
        let mut v: Vec<(String, f64)> = e.into_iter().collect();
        v.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        v
    }

    /// Detect receiver (whole-instrument) clock events by the requirement that
    /// the SAME phase+code jump appears on every tracked satellite. A
    /// single-satellite jump is explicitly NOT a clock event.
    fn detect_clock_events(&self) -> Vec<ClockEvent> {
        let mut out = self.clock_events.clone();
        let epochs = self.sorted_epochs();
        let tracks = self.tracks();
        let declared: HashSet<String> =
            self.clock_events.iter().map(|c| c.epoch_raw.clone()).collect();

        for i in 1..epochs.len() {
            let (e_now, g_now) = &epochs[i];
            let (e_prev, g_prev) = &epochs[i - 1];
            let dt = g_now - g_prev;
            if dt <= 0.0 {
                continue;
            }
            // common-mode phase jump (L1 cycles) and code jump (m) per sv
            let mut phase_jumps: Vec<f64> = vec![];
            let mut code_jumps_m: Vec<f64> = vec![];
            for t in tracks.values() {
                let a = t.epochs.iter().find(|x| &x.epoch_raw == e_prev);
                let b = t.epochs.iter().find(|x| &x.epoch_raw == e_now);
                let (Some(a), Some(b)) = (a, b) else { continue };
                let la = pick(a, &phase_codes(1));
                let lb = pick(b, &phase_codes(1));
                let pa = pick(a, &code_codes(1));
                let pb = pick(b, &code_codes(1));
                let (Some(la), Some(lb), Some(pa), Some(pb)) = (la, lb, pa, pb) else {
                    continue;
                };
                let (Some(la), Some(lb), Some(pa), Some(pb)) =
                    (la.value.value, lb.value.value, pa.value.value, pb.value.value)
                else {
                    continue;
                };
                let lam = Bands { b1: 1, b2: 2 }.l1_m();
                // Prefer Doppler to predict the geometric phase change; the
                // code difference is predicted from the same dynamics so a
                // receiver time step moves both in common mode.
                let da_hz = pick(a, &doppler_codes(1)).and_then(|o| o.value.value);
                let db_hz = pick(b, &doppler_codes(1)).and_then(|o| o.value.value);
                let pred_phase = match (da_hz, db_hz) {
                    (Some(d0), Some(d1)) => -0.5 * (d0 + d1) * dt,
                    _ => (pb - pa) / lam,
                };
                phase_jumps.push((lb - la) - pred_phase);
                code_jumps_m.push(pb - pa - pred_phase * lam);
            }
            if phase_jumps.len() < 2 {
                continue;
            }
            let consensus = median(&phase_jumps);
            let agree = phase_jumps
                .iter()
                .all(|j| (j - consensus).abs() <= self.params.clock_common_sigma_cycles);
            let consensus_code = median(&code_jumps_m);
            let strong = consensus.abs() > 10.0
                && consensus_code.abs() > 100.0
                && agree;
            if strong && !declared.contains(e_now) {
                let lam = Bands { b1: 1, b2: 2 }.l1_m();
                let jump_seconds = consensus * lam / crate::domain::math::SPEED_OF_LIGHT_M_S;
                out.push(ClockEvent {
                    id: crate::model::stable_id(
                        "clock-det",
                        &format!("{}|{}", self.project_id, e_now),
                    ),
                    station: self.station.clone(),
                    epoch_raw: e_now.clone(),
                    gps_seconds: Some(*g_now),
                    event_type: ClockEventType::Detected,
                    jump_seconds: crate::model::Value::parsed(
                        format!("{jump_seconds:.10e}"),
                        jump_seconds,
                        crate::model::Reference::Second,
                    ),
                    sources: vec![],
                });
            }
        }
        out.sort_by(|a, b| a.gps_seconds.partial_cmp(&b.gps_seconds).unwrap());
        out.dedup_by_key(|c| c.epoch_raw.clone());
        out
    }
}

fn doppler_codes(b: u8) -> [&'static str; 2] {
    if b == 1 {
        ["D1W", "D1C"]
    } else if b == 2 {
        ["D2W", "D2C"]
    } else {
        ["", ""]
    }
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

// ---------------------------------------------------------------------------
// Arcs, candidates, residuals
// ---------------------------------------------------------------------------

impl ProjectState {
    fn prov(&self, sources: Vec<Id>) -> Provenance {
        Provenance {
            algo_version: ALGO_VERSION.to_string(),
            params_fp: self.params.fingerprint(),
            sources,
            produced_by_event: self.head_event_id.clone().unwrap_or(Id("none".into())),
        }
    }

    fn is_clock_epoch<'a>(
        &self,
        clock_events: &'a [ClockEvent],
        epoch: &str,
    ) -> Option<&'a ClockEvent> {
        clock_events.iter().find(|c| c.epoch_raw == epoch)
    }

    fn build_arcs(&self, clock_events: &[ClockEvent]) -> Vec<ArcRecord> {
        let mut arcs = vec![];
        for track in self.tracks().values() {
            for band in [1u8, 2u8] {
                let codes = phase_codes(band);
                let n = track.epochs.len();
                let mut idx = 0usize;
                let mut start_reason: &str = boundary::START;
                while idx < n {
                    // skip absent epochs, recording that they break arcs
                    let present = pick(&track.epochs[idx], &codes)
                        .map(|o| o.value.value.is_some())
                        .unwrap_or(false);
                    if !present {
                        start_reason = boundary::MISSING_FREQ;
                        idx += 1;
                        continue;
                    }
                    if self
                        .is_clock_epoch(clock_events, &track.epochs[idx].epoch_raw)
                        .is_some()
                    {
                        start_reason = boundary::CLOCK;
                    }
                    let begin = idx;
                    let mut end_reason: &str = boundary::END;
                    let mut j = idx + 1;
                    while j < n {
                        let cur = &track.epochs[j];
                        let prev = &track.epochs[j - 1];
                        let present_j = pick(cur, &codes)
                            .map(|o| o.value.value.is_some())
                            .unwrap_or(false);
                        if !present_j {
                            end_reason = boundary::MISSING_FREQ;
                            break;
                        }
                        if self.is_clock_epoch(clock_events, &cur.epoch_raw).is_some() {
                            end_reason = boundary::CLOCK;
                            break;
                        }
                        if cur.gps - prev.gps > self.params.arc_gap_seconds {
                            end_reason = boundary::GAP;
                            break;
                        }
                        j += 1;
                    }
                    let last = if j < n { j - 1 } else { n - 1 };
                    let first_ep = &track.epochs[begin];
                    let last_ep = &track.epochs[last];
                    let segment = &track.epochs[begin..=last];
                    let id = crate::model::stable_id(
                        "arc",
                        &format!(
                            "{}|{}|{}|{}|{}|{}",
                            self.project_id,
                            track.sv,
                            band,
                            first_ep.epoch_raw,
                            last_ep.epoch_raw,
                            self.params.fingerprint()
                        ),
                    );
                    arcs.push(ArcRecord {
                        id,
                        station: self.station.clone(),
                        sv: track.sv.clone(),
                        band,
                        code: codes[0].to_string(),
                        start_epoch_raw: first_ep.epoch_raw.clone(),
                        end_epoch_raw: last_ep.epoch_raw.clone(),
                        start_gps: Some(first_ep.gps),
                        end_gps: Some(last_ep.gps),
                        boundary_reason: format!("{start_reason} -> {end_reason}"),
                        revised_from: None,
                        provenance: self.prov(
                            segment
                                .iter()
                                .filter_map(|e| pick(e, &codes).map(|o| o.id.clone()))
                                .collect(),
                        ),
                    });
                    start_reason = end_reason;
                    idx = if j < n { j } else { n };
                }
            }
        }
        arcs
    }
}

// ---------------------------------------------------------------------------
// Candidate derivation
// ---------------------------------------------------------------------------

pub fn candidate_logical_key(
    project_id: &Id,
    station: &str,
    sv: &str,
    epoch: &str,
    b1: u8,
    b2: u8,
) -> String {
    format!("{project_id}|{station}|{sv}|{epoch}|{b1}-{b2}")
}

pub fn candidate_id(
    project_id: &Id,
    station: &str,
    sv: &str,
    epoch: &str,
    b1: u8,
    b2: u8,
) -> Id {
    crate::model::stable_id("cand", &candidate_logical_key(project_id, station, sv, epoch, b1, b2))
}

struct BoundaryInputs {
    l1_prev: Option<f64>,
    l1_now: Option<f64>,
    l2_prev: Option<f64>,
    l2_now: Option<f64>,
    p1_prev: Option<f64>,
    p1_now: Option<f64>,
    p2_prev: Option<f64>,
    p2_now: Option<f64>,
    d1_prev: Option<f64>,
    d1_now: Option<f64>,
    d2_prev: Option<f64>,
    d2_now: Option<f64>,
    lli1: u8,
    lli2: u8,
    lli_codes: Vec<(String, u8)>,
    b2_missing: bool,
    dt: f64,
}

impl ProjectState {
    /// Evaluate one station/sv epoch boundary. Pure given inputs.
    fn evaluate_boundary(
        &self,
        sv: &str,
        prev: &EpochData,
        now: &EpochData,
        sources: Vec<Id>,
    ) -> Option<Candidate> {
        let bands = Bands { b1: 1, b2: 2 };
        let get = |ep: &EpochData, codes: [&'static str; 2]| -> (Option<f64>, Option<Observation>) {
            let o = pick(ep, &codes);
            (o.and_then(|x| x.value.value), o.cloned())
        };
        let (l1p, _) = get(prev, phase_codes(1));
        let (l1n, o1n) = get(now, phase_codes(1));
        let (l2p, _) = get(prev, phase_codes(2));
        let (l2n, o2n) = get(now, phase_codes(2));
        let (p1p, _) = get(prev, code_codes(1));
        let (p1n, _) = get(now, code_codes(1));
        let (p2p, _) = get(prev, code_codes(2));
        let (p2n, _) = get(now, code_codes(2));
        let (d1p, _) = get(prev, doppler_codes(1));
        let (d1n, _) = get(now, doppler_codes(1));
        let (d2p, _) = get(prev, doppler_codes(2));
        let (d2n, _) = get(now, doppler_codes(2));

        if l1p.is_none() || l1n.is_none() {
            return None;
        }
        let b2_missing = l2p.is_none() || l2n.is_none();
        let mut lli_codes = vec![];
        if let Some(o) = &o1n {
            if o.lli != 0 {
                lli_codes.push((o.code.clone(), o.lli));
            }
        }
        if let Some(o) = &o2n {
            if o.lli != 0 {
                lli_codes.push((o.code.clone(), o.lli));
            }
        }

        let inp = BoundaryInputs {
            l1_prev: l1p,
            l1_now: l1n,
            l2_prev: l2p,
            l2_now: l2n,
            p1_prev: p1p,
            p1_now: p1n,
            p2_prev: p2p,
            p2_now: p2n,
            d1_prev: d1p,
            d1_now: d1n,
            d2_prev: d2p,
            d2_now: d2n,
            lli1: o1n.map(|o| o.lli).unwrap_or(0),
            lli2: o2n.map(|o| o.lli).unwrap_or(0),
            lli_codes,
            b2_missing,
            dt: now.gps - prev.gps,
        };

        let (l1p, l1n) = (inp.l1_prev.unwrap(), inp.l1_now.unwrap());
        // --- Doppler/time-difference prediction ---
        let pred1 = -0.5
            * (inp.d1_prev.unwrap_or(0.0) + inp.d1_now.unwrap_or(0.0))
            * inp.dt;
        let pred2 = -0.5
            * (inp.d2_prev.unwrap_or(0.0) + inp.d2_now.unwrap_or(0.0))
            * inp.dt;
        let td1 = l1n - l1p - pred1;
        let td2 = match (inp.l2_prev, inp.l2_now) {
            (Some(a), Some(b)) if pred2.abs() > 0.0 => Some(b - a - pred2),
            _ => None,
        };

        // --- Geometry-free ionosphere combination ---
        let gf = match (inp.l2_prev, inp.l2_now) {
            (Some(lp), Some(ln)) => {
                let now_m = bands.iono_phase_m(l1n, ln);
                let prev_m = bands.iono_phase_m(l1p, lp);
                Some(now_m - prev_m)
            }
            _ => None,
        };

        // --- Melbourne-Wubbena ---
        let mw = match (
            inp.l2_prev,
            inp.l2_now,
            inp.p1_prev,
            inp.p1_now,
            inp.p2_prev,
            inp.p2_now,
        ) {
            (Some(lp), Some(ln), Some(p1p), Some(p1n), Some(p2p), Some(p2n)) => {
                Some(bands.mw_cycles(l1n, ln, p1n, p2n)
                    - bands.mw_cycles(l1p, lp, p1p, p2p))
            }
            _ => None,
        };

        // Exceedances drive candidate creation.
        let td_hit = td1.abs() > self.params.td_sigma_cycles
            || td2.map(|x| x.abs() > self.params.td_sigma_cycles).unwrap_or(false);
        let gf_hit = gf
            .map(|x| x.abs() > self.params.gf_sigma_cycles * bands.l1_m())
            .unwrap_or(false);
        let mw_hit = mw
            .map(|x| x.abs() > self.params.mw_sigma_cycles)
            .unwrap_or(false);
        let lli_flag = inp.lli1 != 0 || inp.lli2 != 0;
        if !td_hit && !gf_hit && !mw_hit && !lli_flag {
            return None;
        }

        let mut votes: Vec<DetectorVote> = vec![];
        let mut rejections: Vec<Rejection> = vec![];

        // TD detector proposal (L1, and L2 when present)
        let td_n1 = if td_hit { Some(nearest_int(td1)) } else { None };
        let td_n2 = if let Some(t) = td2 {
            if t.abs() > self.params.td_sigma_cycles {
                Some(nearest_int(t))
            } else {
                None
            }
        } else {
            None
        };
        votes.push(DetectorVote {
            detector: "time-difference".to_string(),
            residual: Some(td1),
            residual_unit: "cycle(L1)".to_string(),
            shift_cycles_b1: td_n1,
            shift_cycles_b2: td_n2,
            score: gaussian_score(td1, self.params.td_sigma_cycles),
            note: if inp.d1_now.is_none() {
                "doppler unavailable; geometric prediction degraded".to_string()
            } else {
                "doppler-predicted".to_string()
            },
        });

        // GF ionosphere detector: a jump in metres maps to band cycles
        let (gf_n1, gf_n2) = match gf {
            Some(g) => {
                let (c1, c2) = bands.gf_jump_to_cycles(g);
                let hit = g.abs() > self.params.gf_sigma_cycles * bands.l1_m();
                if hit {
                    (Some(nearest_int(c1)), Some(nearest_int(c2)))
                } else {
                    (None, None)
                }
            }
            None => (None, None),
        };
        votes.push(DetectorVote {
            detector: "ionosphere-GF".to_string(),
            residual: gf,
            residual_unit: "m".to_string(),
            shift_cycles_b1: gf_n1,
            shift_cycles_b2: gf_n2,
            score: gf
                .map(|g| {
                    gaussian_score(g / bands.l1_m(), self.params.gf_sigma_cycles)
                })
                .unwrap_or(0.0),
            note: if gf.is_none() {
                "GF unavailable: frequency missing".to_string()
            } else {
                "geometry-free phase".to_string()
            },
        });

        // MW detector: jump in widelane cycles constrains n1 - n2
        let (mw_nw, mw_consistent) = match mw {
            Some(w) => {
                let nw = nearest_int(w);
                let frac = (w - nw as f64).abs();
                let consistent = frac <= self.params.tie_band_cycles;
                if w.abs() > self.params.mw_sigma_cycles {
                    (Some(nw), consistent)
                } else {
                    (None, consistent)
                }
            }
            None => (None, false),
        };
        votes.push(DetectorVote {
            detector: "melbourne-wubbena".to_string(),
            residual: mw,
            residual_unit: "wide-lane-cycle".to_string(),
            shift_cycles_b1: mw_nw,
            shift_cycles_b2: Some(0),
            score: mw.map(|w| gaussian_score(w, self.params.mw_sigma_cycles)).unwrap_or(0.0),
            note: if mw.is_none() {
                "MW unavailable: P1/P2 or L2 missing".to_string()
            } else if mw_consistent {
                "integer widelane jump consistent".to_string()
            } else {
                "code/phase disagree (code bump or non-integer)".to_string()
            },
        });

        // Build integer hypotheses by joint scoring over TD + GF + MW.
        let hypotheses = self.build_hypotheses(&inp, td1, td2, gf, mw);
        let tied: Vec<&OffsetHypothesis> =
            hypotheses.iter().filter(|h| h.tied).collect();
        let best = hypotheses.iter().max_by(|a, b| {
            a.total_score.partial_cmp(&b.total_score).unwrap()
        });

        let ckey = candidate_logical_key(
            &self.project_id,
            &self.station,
            sv,
            &now.epoch_raw,
            1,
            2,
        );

        let mut status = CandidateStatus::Proposed;
        // LLI-only with weak detector evidence -> false alarm, rejected.
        let detector_strong = best.map(|h| h.total_score > 0.65).unwrap_or(false);
        if lli_flag && !td_hit && !gf_hit && !mw_hit {
            status = CandidateStatus::Rejected;
            rejections.push(Rejection {
                reason_code: "lli-false-alarm".to_string(),
                detail: "LLI bit set but GF/MW/TD residuals all inside gates"
                    .to_string(),
                sources: sources.clone(),
            });
        } else if inp.b2_missing && hypotheses.is_empty() {
            status = CandidateStatus::Unknown;
            rejections.push(Rejection {
                reason_code: "frequency-missing".to_string(),
                detail: "L2 unavailable; multi-frequency combinations cannot be formed"
                    .to_string(),
                sources: sources.clone(),
            });
        } else if !tied.is_empty() && tied.len() > 1 {
            status = CandidateStatus::Ambiguous;
            rejections.push(Rejection {
                reason_code: "equally-divided-offset".to_string(),
                detail: format!(
                    "{} integer hypotheses tie within {:.2} cycles",
                    tied.len(),
                    self.params.tie_band_cycles
                ),
                sources: sources.clone(),
            });
        } else if mw.is_some() && !mw_consistent && gf_hit == false {
            // code-only disturbance: detectors disagree, none is truth
            status = CandidateStatus::Proposed;
            rejections.push(Rejection {
                reason_code: "detector-disagreement".to_string(),
                detail: "MW widelane jump is non-integer while phase combinations stay quiet"
                    .to_string(),
                sources: sources.clone(),
            });
        }

        if self.accepted.contains_key(&ckey) {
            status = CandidateStatus::Accepted;
        } else if self.rejected.contains(&ckey) {
            status = CandidateStatus::Rejected;
        }
        if self.conflicts.contains_key(&ckey) {
            status = CandidateStatus::Conflict;
        }

        let (arc_before, arc_after) = self.arc_pair(sv, prev, now);

        Some(Candidate {
            id: candidate_id(&self.project_id, &self.station, sv, &now.epoch_raw, 1, 2),
            station: self.station.clone(),
            sv: sv.to_string(),
            band1: 1,
            band2: 2,
            epoch_raw: now.epoch_raw.clone(),
            gps_seconds: Some(now.gps),
            prev_epoch_raw: prev.epoch_raw.clone(),
            arc_id_before: arc_before,
            arc_id_after: arc_after,
            status,
            votes,
            hypotheses,
            lli_flags: inp.lli_codes.clone(),
            rejections,
            error: None,
            provenance: self.prov(sources),
        })
    }

    fn arc_pair(&self, sv: &str, prev: &EpochData, now: &EpochData) -> (Id, Id) {
        let fp = self.params.fingerprint();
        let arc_id = |which: &str, ep: &str| {
            crate::model::stable_id(
                "arc",
                &format!("{}|{}|1|{}|{}", self.project_id, sv, which, ep),
            )
        };
        let _ = fp;
        (arc_id("seg", &prev.epoch_raw), arc_id("seg", &now.epoch_raw))
    }
}

fn gaussian_score(residual_cycles: f64, sigma_cycles: f64) -> f64 {
    let x = residual_cycles / sigma_cycles.max(1e-6);
    (-0.5 * x * x).exp()
}

// ---------------------------------------------------------------------------
// Hypothesis joint scoring
// ---------------------------------------------------------------------------

impl ProjectState {
    fn build_hypotheses(
        &self,
        inp: &BoundaryInputs,
        td1: f64,
        td2: Option<f64>,
        gf_m: Option<f64>,
        mw_w: Option<f64>,
    ) -> Vec<OffsetHypothesis> {
        let bands = Bands { b1: 1, b2: 2 };
        let mut hyps: Vec<OffsetHypothesis> = vec![];

        if inp.b2_missing {
            // Single frequency: only TD informs the L1 integer. Half-integer
            // true jumps produce two equidistant legal integers.
            let center = nearest_int(td1);
            for n1 in center - 1..=center + 1 {
                let td_res = td1 - n1 as f64;
                let score = gaussian_score(td_res, self.params.td_sigma_cycles);
                hyps.push(OffsetHypothesis {
                    shift_b1_cycles: Some(n1),
                    shift_b2_cycles: None,
                    total_score: score,
                    tied: false,
                });
            }
        } else {
            let c1 = nearest_int(td1);
            let c2 = td2.map(nearest_int).unwrap_or_else(|| {
                gf_m
                    .map(|g| {
                        let (_, cc2) = bands.gf_jump_to_cycles(g);
                        nearest_int(cc2)
                    })
                    .unwrap_or(0)
            });
            for n1 in c1 - 1..=c1 + 1 {
                for n2 in c2 - 1..=c2 + 1 {
                    let mut score = 0.0_f64;
                    let mut n_terms = 0.0_f64;
                    // TD term
                    let td_res = td1 - n1 as f64;
                    score += gaussian_score(td_res, self.params.td_sigma_cycles);
                    n_terms += 1.0;
                    if let Some(t2) = td2 {
                        score += gaussian_score(t2 - n2 as f64, self.params.td_sigma_cycles);
                        n_terms += 1.0;
                    }
                    // GF term: predicted metres - observed metres
                    if let Some(g) = gf_m {
                        let pred_m = n1 as f64 * bands.l1_m() - n2 as f64 * bands.l2_m();
                        score += gaussian_score(
                            (g - pred_m) / bands.l1_m(),
                            self.params.gf_sigma_cycles,
                        );
                        n_terms += 1.0;
                    }
                    // MW term: widelane n1 - n2
                    if let Some(w) = mw_w {
                        score += gaussian_score(
                            w - (n1 - n2) as f64,
                            self.params.mw_sigma_cycles,
                        );
                        n_terms += 1.0;
                    }
                    hyps.push(OffsetHypothesis {
                        shift_b1_cycles: Some(n1),
                        shift_b2_cycles: Some(n2),
                        total_score: score / n_terms,
                        tied: false,
                    });
                }
            }
        }

        // Descending score, deterministic tie-break by integer order.
        hyps.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap()
                .then_with(|| a.shift_b1_cycles.cmp(&b.shift_b1_cycles))
                .then_with(|| a.shift_b2_cycles.cmp(&b.shift_b2_cycles))
        });
        let best = hyps.first().map(|h| h.total_score).unwrap_or(0.0);
        for h in hyps.iter_mut() {
            h.tied = (best - h.total_score).abs() <= score_band_to_tolerance(self.params.tie_band_cycles);
        }
        hyps
    }
}

fn score_band_to_tolerance(_tie_cycles: f64) -> f64 {
    // Two hypotheses whose joint score differs by less than this are
    // operationally indistinguishable at the configured tie band.
    0.08
}

// ---------------------------------------------------------------------------
// View assembly
// ---------------------------------------------------------------------------

impl ProjectState {
    /// Recompute the full derived view from the active projection. Called per
    /// head event and at arbitrary historical event during review.
    pub fn derive_view(&self) -> DerivedView {
        let clock_events = self.detect_clock_events();
        let arcs = self.build_arcs(&clock_events);
        let mut candidates = vec![];
        let mut residuals = vec![];

        for track in self.tracks().values() {
            let bands = Bands { b1: 1, b2: 2 };
            let accepted_offset = |epoch: &str| -> Option<&OffsetLayer> {
                let key =
                    candidate_logical_key(&self.project_id, &self.station, &track.sv, epoch, 1, 2);
                self.accepted.get(&key)
            };

            for pair in track.epochs.windows(2) {
                let prev = &pair[0];
                let now = &pair[1];
                // skip receiver-clock boundaries for slip candidates (separated)
                if self.is_clock_epoch(&clock_events, &now.epoch_raw).is_some() {
                    continue;
                }
                let mut sources = vec![];
                for codes in [phase_codes(1), phase_codes(2), code_codes(1), code_codes(2)] {
                    if let Some(o) = pick(now, &codes) {
                        sources.push(o.id.clone());
                    }
                    if let Some(o) = pick(prev, &codes) {
                        sources.push(o.id.clone());
                    }
                }
                if let Some(c) = self.evaluate_boundary(&track.sv, prev, now, sources) {
                    candidates.push(c);
                }
            }

            // Residual time series at every epoch (plot). Offset layer applied.
            for ep in &track.epochs {
                let l1 = pick(ep, &phase_codes(1)).and_then(|o| o.value.value);
                let l2 = pick(ep, &phase_codes(2)).and_then(|o| o.value.value);
                let p1 = pick(ep, &code_codes(1)).and_then(|o| o.value.value);
                let p2 = pick(ep, &code_codes(2)).and_then(|o| o.value.value);

                let mut add = |combination: &str,
                               value: Option<f64>,
                               unit: &str,
                               diags: Vec<String>,
                               src: Vec<Id>| {
                    residuals.push(ResidualPoint {
                        id: crate::model::stable_id(
                            "resid",
                            &format!(
                                "{}|{}|{}|{}|{}",
                                self.project_id, track.sv, combination, ep.epoch_raw,
                                self.params.fingerprint()
                            ),
                        ),
                        station: self.station.clone(),
                        sv: track.sv.clone(),
                        epoch_raw: ep.epoch_raw.clone(),
                        gps_seconds: Some(ep.gps),
                        combination: combination.to_string(),
                        value,
                        unit: unit.to_string(),
                        diagnostics: diags,
                        sources: src,
                    });
                };

                // Apply accepted offsets (offset layer only; raw never edited).
                let mut l1c = l1;
                let mut l2c = l2;
                if let Some(off) = accepted_offset(&ep.epoch_raw) {
                    if let (Some(v), Some(n)) = (l1c, off.shift_b1) {
                        l1c = Some(v - n as f64);
                    }
                    if let (Some(v), Some(n)) = (l2c, off.shift_b2) {
                        l2c = Some(v - n as f64);
                    }
                }

                let src1 = pick(ep, &phase_codes(1)).map(|o| vec![o.id.clone()]).unwrap_or_default();
                match (l1c, l2c) {
                    (Some(a), Some(b)) => add(
                        "ionosphere-GF",
                        Some(bands.iono_phase_m(a, b)),
                        "m",
                        vec![],
                        src1.clone(),
                    ),
                    _ => add(
                        "ionosphere-GF",
                        None,
                        "m",
                        vec!["frequency missing: GF not formed".to_string()],
                        src1.clone(),
                    ),
                }
                match (l1c, l2c, p1, p2) {
                    (Some(a), Some(b), Some(c1v), Some(c2v)) => add(
                        "melbourne-wubbena",
                        Some(bands.mw_cycles(a, b, c1v, c2v)),
                        "wide-lane-cycle",
                        vec![],
                        src1,
                    ),
                    _ => add(
                        "melbourne-wubbena",
                        None,
                        "wide-lane-cycle",
                        vec!["P1/P2 or L2 missing: MW not formed".to_string()],
                        src1.clone(),
                    ),
                }
            }
        }

        candidates.sort_by(|a, b| {
            a.gps_seconds
                .partial_cmp(&b.gps_seconds)
                .unwrap()
                .then_with(|| a.sv.cmp(&b.sv))
        });
        residuals.sort_by(|a, b| {
            a.combination
                .cmp(&b.combination)
                .then_with(|| a.sv.cmp(&b.sv))
                .then_with(|| a.gps_seconds.partial_cmp(&b.gps_seconds).unwrap())
        });
        DerivedView { arcs, candidates, residuals, clock_events }
    }
}
