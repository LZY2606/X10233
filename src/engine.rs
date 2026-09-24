//! Computation engine: arcs, clock-jump separation, detectors
//! (time-difference / geometry-free ionosphere combination / Melbourne-Wubbena),
//! candidate fusion, judgement events and local-expiry tracking.
//!
//! Principles encoded here:
//! - The three detectors may yield different candidates; none is truth.
//! - Receiver clock jumps and per-satellite cycle slips are separated.
//! - Repairs live only in an offset layer; raw observations are never edited.
//! - Candidates, reject reasons, errors and unknown states are first-class rows.
//! - A local input change expires only dependent results; expired results stay
//!   viewable but are marked stale, never posed as current.

use crate::clock::Clock;
use crate::db::{append_event, branch_ancestry, Db, EventRec, ALGORITHM_VERSION};
use crate::identity::{fid, fp};
use crate::model::*;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct ObsCell {
    pub id: String,
    pub natural_id: String,
    pub epoch_iso: String,
    pub epoch_s: i64,
    pub sat: String,
    pub band: Band,
    pub code: String,
    pub c: Option<f64>,
    pub l: Option<f64>,
    pub d: Option<f64>,
    pub s: Option<f64>,
    pub lli: bool,
    pub diag: Option<String>,
}

#[derive(Debug)]
pub struct ActiveInput {
    pub params: Params,
    pub params_version: i64,
    pub batch_ids: Vec<String>,
    pub decisions: Vec<Value>,
    pub basis_event_id: String,
    pub basis_seq: i64,
    pub ancestry: Vec<EventRec>,
}

pub fn resolve_active(db: &Db, project_id: &str, branch: &str, at_seq: Option<i64>) -> ActiveInput {
    let ancestry = branch_ancestry(db, project_id, branch, at_seq);
    let mut params_version = 1i64;
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let mut decisions = Vec::new();
    let mut basis_event_id = "genesis".to_string();
    let mut basis_seq = 0i64;
    for ev in &ancestry {
        basis_event_id = ev.id.clone();
        basis_seq = ev.seq;
        match ev.kind.as_str() {
            "import" => {
                if let (Some(label), Some(batch)) = (
                    ev.payload.get("label").and_then(Value::as_str),
                    ev.payload.get("batch_id").and_then(Value::as_str),
                ) {
                    labels.insert(label.to_string(), batch.to_string());
                }
            }
            "params_revision" => {
                if let Some(v) = ev.payload.get("version").and_then(Value::as_i64) {
                    params_version = v;
                }
            }
            "decision" => decisions.push(ev.payload.clone()),
            _ => {}
        }
    }
    let params = db.params_at(project_id, params_version).unwrap_or_default();
    ActiveInput {
        params,
        params_version,
        batch_ids: labels.into_values().collect(),
        decisions,
        basis_event_id,
        basis_seq,
        ancestry,
    }
}

fn load_obs(db: &Db, project_id: &str, batch_ids: &[String]) -> Vec<ObsCell> {
    let c = db.conn.lock().unwrap();
    let mut by_natural: BTreeMap<String, ObsCell> = BTreeMap::new();
    for b in batch_ids {
        let mut st = c
            .prepare(
                "SELECT o.id,o.natural_id,o.epoch_iso,e.epoch_s,o.sat,o.band_num,o.code,
                        o.c_val,o.l_val,o.d_val,o.s_val,o.lli_val,o.diag
                 FROM raw_obs o JOIN raw_epochs e ON e.id=o.epoch_id
                 WHERE o.project_id=?1 AND o.batch_id=?2
                 ORDER BY o.ord",
            )
            .unwrap();
        let rows = st
            .query_map(params![project_id, b], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<f64>>(7)?,
                    r.get::<_, Option<f64>>(8)?,
                    r.get::<_, Option<f64>>(9)?,
                    r.get::<_, Option<f64>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                ))
            })
            .unwrap();
        for row in rows.flatten() {
            let cell = ObsCell {
                id: row.0,
                natural_id: row.1.clone(),
                epoch_iso: row.2,
                epoch_s: row.3,
                sat: row.4,
                band: Band::from_num(row.5 as u8),
                code: row.6,
                c: row.7,
                l: row.8,
                d: row.9,
                s: row.10,
                lli: row.11.map(|v| v & 1 == 1).unwrap_or(false),
                diag: row.12,
            };
            by_natural.insert(row.1, cell);
        }
    }
    let mut out: Vec<ObsCell> = by_natural.into_values().collect();
    out.sort_by(|a, b| {
        a.epoch_s
            .cmp(&b.epoch_s)
            .then(a.epoch_iso.cmp(&b.epoch_iso))
            .then(a.sat.cmp(&b.sat))
            .then(a.band.cmp(&b.band))
    });
    out
}

/// Repairs (offset layer) from accepted decisions, in ancestry order.
/// Returns (sat, band, from_epoch_iso, offset_cycles, decision_id).
pub fn active_repairs(decisions: &[Value]) -> Vec<(String, Band, String, f64, String)> {
    let mut out = Vec::new();
    for d in decisions {
        if d.get("kind").and_then(Value::as_str) != Some("accept") {
            continue;
        }
        let sat = d.get("sat").and_then(Value::as_str).unwrap_or("").to_string();
        let from_epoch = d.get("boundary_epoch_iso").and_then(Value::as_str).unwrap_or("").to_string();
        let decision_id = d.get("decision_id").and_then(Value::as_str).unwrap_or("").to_string();
        let n1 = d.get("n1").and_then(Value::as_f64).unwrap_or(0.0);
        let n2 = d.get("n2").and_then(Value::as_f64);
        let kind = d.get("candidate_kind").and_then(Value::as_str).unwrap_or("pair");
        if n1 != 0.0 || kind == "single" {
            out.push((sat.clone(), Band::L1, from_epoch.clone(), n1, decision_id.clone()));
        }
        if kind == "pair" {
            if let Some(n2) = n2 {
                if n2 != 0.0 {
                    out.push((sat.clone(), Band::L2, from_epoch.clone(), n2, decision_id.clone()));
                }
            }
        }
    }
    out
}

/// Effective (repaired) phase: raw minus offsets whose from_epoch <= cell epoch.
fn effective_l(cell: &ObsCell, repairs: &[(String, Band, String, f64, String)], epoch_order: &BTreeMap<String, i64>) -> Option<f64> {
    let raw = cell.l?;
    let mut off = 0.0;
    let cell_ord = *epoch_order.get(&cell.epoch_iso).unwrap_or(&0);
    for (sat, band, from_epoch, offset, _) in repairs {
        if *sat == cell.sat && *band == cell.band {
            let from_ord = *epoch_order.get(from_epoch.as_str()).unwrap_or(&i64::MAX);
            if from_ord <= cell_ord {
                off += offset;
            }
        }
    }
    Some(raw - off)
}

fn mw_cycles(l1: f64, l2: f64, c1: f64, c2: f64) -> f64 {
    let f1 = GPS_L1_HZ;
    let f2 = GPS_L2_HZ;
    let lam_wl = C_MPS / (f1 - f2);
    (l1 - l2) - (f1 * c1 + f2 * c2) / ((f1 + f2) * lam_wl)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunOutcome {
    pub run_id: String,
    pub created: bool,
    pub status: String,
    pub seq: i64,
    pub source_hash: String,
    pub params_version: i64,
    pub n_arcs: usize,
    pub n_candidates: usize,
    pub n_detector_obs: usize,
    pub n_judge_events: usize,
}

struct Boundary {
    a_iso: String,
    b_iso: String,
    dt: f64,
}

struct ArcRec {
    natural_id: String,
    sat: String,
    band: u8,
    ord: usize,
    start_iso: String,
    end_iso: String,
    split_reason: String,
    epochs: Vec<String>,
}

struct DetRow {
    natural_id: String,
    arc_natural: Option<String>,
    boundary_b: String,
    sat: String,
    detector: DetectorKind,
    band: Option<u8>,
    residual: Option<f64>,
    unit: String,
    status: String,
    sources: Vec<String>,
    expected: Option<f64>,
    dt: f64,
}

struct CandRow {
    natural_id: String,
    arc_natural: Option<String>,
    boundary_b: String,
    sat: String,
    band: u8,
    kind: String,
    n1: Option<i64>,
    n2: Option<i64>,
    slip_l1: f64,
    score: f64,
    status: CandidateStatus,
    supporting: Vec<DetectorKind>,
    reject_reason: Option<String>,
}

struct JudgeRow {
    kind: JudgeKind,
    epoch_iso: Option<String>,
    sat: Option<String>,
    band: Option<u8>,
    payload: Value,
}
