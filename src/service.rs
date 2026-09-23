//! Application service: import (append-only), materialization, replay/fork and
//! review operations. Detection math lives in `engine`; this module owns
//! provenance, parameter fingerprints and the event-sourced state machine.

use crate::clock::Clock;
use crate::db;
use crate::engine::{self, EffObs, EffectiveWorld, EEpoch, SatEpoch, SatState, ALGO_VERSION};
use crate::freq::{band_for_code, Band};
use crate::ids::id;
use crate::model::*;
use crate::parser::{self, RawField};
use rusqlite::{params, Connection, Row};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub struct App {
    pub conn: Connection,
    pub clock: Arc<dyn Clock>,
}

pub fn sha256_hex(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    hex::encode(h.finalize())
}

#[derive(serde::Serialize)]
pub struct ImportReport {
    pub commit_id: String,
    pub seq: i64,
    pub diagnostics: Vec<parser::ParseDiag>,
    pub n_epochs: usize,
    pub n_obs: usize,
    pub n_nav: usize,
    pub n_clock_events: usize,
}

impl App {
    pub fn new(conn: Connection, clock: Arc<dyn Clock>) -> Self { App { conn, clock } }

    pub fn create_project(&self, name: &str, cex: Option<&str>) -> rusqlite::Result<Project> {
        let pid = id("proj", &[name]);
        let p = Project { id: pid.clone(), name: name.into(),
            created_at: self.clock.now(), counterexample_key: cex.map(String::from) };
        db::put_project(&self.conn, &p)?;
        let main = Branch {
            id: id("branch", &[&pid, "main"]), name: "main".into(),
            parent_branch_id: None, forked_from_event_seq: None,
            created_at: self.clock.now(), note: "initial review line".into(),
        };
        db::put_branch(&self.conn, &main)?;
        Ok(p)
    }

    fn next_commit_seq(&self) -> rusqlite::Result<i64> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM raw_commits",
            [], |r| r.get(0))?)
    }

    /// Append raw material. Kind: obs | nav_gps | nav_glo | clocks.
    /// Raw bytes are stored verbatim and immutable; later fixes arrive as new
    /// commits with a parent pointer (`obs_patch` merges effective values only).
    pub fn import_text(&self, project_id: &str, kind: &str, filename: &str,
                       text: &str) -> Result<ImportReport, String> {
        self.import_text_inner(project_id, kind, filename, text)
    }

    fn import_text_inner(&self, project_id: &str, kind: &str, filename: &str,
                         text: &str) -> Result<ImportReport, String> {
        let seq = self.next_commit_seq().map_err(|e| e.to_string())?;
        let parent = self.conn.query_row(
            "SELECT id FROM raw_commits WHERE seq=(SELECT MAX(seq) FROM raw_commits)",
            [], |r| r.get::<_, String>(0)).ok();
        let commit = RawCommit {
            id: id("commit", &[project_id, &seq.to_string(), &sha256_hex(text.as_bytes())]),
            seq, parent_id: parent, kind: kind.into(), filename: filename.into(),
            raw_text: text.into(), sha256: sha256_hex(text.as_bytes()),
            imported_at: self.clock.now(),
        };
        db::put_raw_commit(&self.conn, &commit).map_err(|e| e.to_string())?;

        let mut rep = ImportReport {
            commit_id: commit.id.clone(), seq, diagnostics: vec![],
            n_epochs: 0, n_obs: 0, n_nav: 0, n_clock_events: 0,
        };

        match kind {
            "obs" | "obs_patch" => {
                let file = parser::parse_obs(text).map_err(|ds| serde_json::to_string(&ds).unwrap())?;
                rep.diagnostics = file.diagnostics.clone();
                rep.n_epochs = file.epochs.len();
                store_diagnostics(&self.conn, &commit.id, &file.diagnostics)?;
                // epochs are content-identified by time; a re-issued value at the
                // same (epoch,sat,code) points effective_commit_id at the new commit.
                for (i, ep) in file.epochs.iter().enumerate() {
                    let eid = id("epoch", &[project_id, &ep.time]);
                    db::put_epoch(&self.conn, &EpochRec {
                        id: eid.clone(), ord: i as i64, time: ep.time.clone(),
                        time_raw: ep.time_raw.clone(), flag: ep.flag.clone(),
                        clock_offset_raw: ep.clock_offset_raw.clone(),
                        clock_offset_s: ep.clock_offset,
                    }).map_err(|e| e.to_string())?;
                    for line in &ep.lines {
                        let oid = id("obs", &[project_id, &eid, &line.sat, &line.code]);
                        let existing: Option<String> = self.conn.query_row(
                            "SELECT id FROM observations WHERE id=?1", [&oid], |r| r.get(0)).ok();
                        let kind_ch = line.code.chars().next().unwrap();
                        let band = line.code.get(1..2)
                            .and_then(|b| band_for_code(&line.code))
                            .map(|b| match b { Band::L1 => "L1", Band::L2 => "L2" })
                            .unwrap_or("?");
                        db::put_obs(&self.conn, &ObsRec {
                            id: oid.clone(), epoch_id: eid.clone(),
                            sat: line.sat.clone(), code: line.code.clone(),
                            band: band.into(), kind: kind_ch.to_string(),
                            raw: line.field.raw.clone(), v: line.field.v,
                            unit: line.field.u.clone(), lli: line.field.lli.clone(),
                            ssi: line.field.ssi.clone(),
                            diagnostic: line.field.diagnostic.clone(),
                            effective_commit_id: Some(commit.id.clone()),
                        }).map_err(|e| e.to_string())?;
                        if existing.is_some() {
                            db::set_effective_commit(&self.conn, &oid, &commit.id)
                                .map_err(|e| e.to_string())?;
                        }
                        rep.n_obs += 1;
                    }
                }
            }
            "nav_gps" | "nav_glo" => {
                let hint = if kind == "nav_glo" { "glo" } else { "gps" };
                let recs = parser::parse_nav(text, hint)
                    .map_err(|ds| serde_json::to_string(&ds).unwrap())?;
                rep.n_nav = recs.len();
                for r in &recs {
                    let vals: Vec<RawField> = r.values.clone();
                    db::put_nav(&self.conn, &NavRecRow {
                        id: id("nav", &[project_id, &r.sat, &r.time]),
                        commit_id: commit.id.clone(), sat: r.sat.clone(),
                        time: r.time.clone(), time_raw: r.time_raw.clone(),
                        kind: r.kind.clone(), frequency_number: r.frequency_number,
                        values_json: serde_json::to_string(&vals).unwrap(),
                    }).map_err(|e| e.to_string())?;
                }
            }
            "clocks" => {
                let (evs, diags) = parser::parse_clock_events(text);
                rep.diagnostics = diags;
                store_diagnostics(&self.conn, &commit.id, &rep.diagnostics)?;
                rep.n_clock_events = evs.len();
                for e in &evs {
                    db::put_clock_ledger(&self.conn, &ClockLedgerRow {
                        id: id("clockledger", &[project_id, &e.time, &e.source]),
                        commit_id: commit.id.clone(), time: e.time.clone(),
                        source: e.source.clone(), magnitude_raw: e.magnitude_raw.clone(),
                        magnitude_s: e.magnitude, unit: e.unit.clone(), note: e.note.clone(),
                    }).map_err(|x| x.to_string())?;
                }
            }
            other => return Err(format!("unknown material kind: {other}")),
        }
        Ok(rep)
    }
}

fn store_diagnostics(conn: &Connection, commit_id: &str,
                     ds: &[parser::ParseDiag]) -> Result<(), String> {
    for d in ds {
        db::put_diagnostic(conn, &DiagnosticRec {
            id: id("diag", &[commit_id, &d.line_no.to_string(), &d.code, &d.raw]),
            commit_id: commit_id.into(), line_no: d.line_no as i64,
            severity: d.severity.clone(), code: d.code.clone(),
            message: d.message.clone(), raw: d.raw.clone(),
        }).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------- parameter versions ----------

impl App {
    pub fn initial_params(&self) -> rusqlite::Result<Params> {
        self.put_params(DetParams::initial(), "initial detector parameters")
    }

    pub fn put_params(&self, dp: DetParams, note: &str) -> rusqlite::Result<Params> {
        let fp = format!("{}:{}", ALGO_VERSION, dp.fingerprint());
        let pid = id("params", &[&fp]);
        if let Ok(existing) = self.get_params_by_id(&pid) {
            return Ok(existing);
        }
        let seq = self.conn.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM params_versions", [], |r| r.get(0))?;
        let parent = self.conn.query_row(
            "SELECT id FROM params_versions WHERE seq=(SELECT MAX(seq) FROM params_versions)",
            [], |r| r.get::<_, String>(0)).ok();
        let p = Params {
            id: pid, seq, parent_id: parent, fingerprint: fp,
            json: serde_json::to_string(&dp).unwrap(),
            created_at: self.clock.now(), note: note.into(),
        };
        db::put_params(&self.conn, &p)?;
        Ok(p)
    }

    pub fn get_params_by_id(&self, pid: &str) -> rusqlite::Result<Params> {
        self.conn.query_row("SELECT * FROM params_versions WHERE id=?1", [pid], row_params)
    }

    pub fn latest_params_id(&self) -> rusqlite::Result<Option<String>> {
        self.conn.query_row(
            "SELECT id FROM params_versions ORDER BY seq DESC LIMIT 1", [], |r| r.get(0))
    }
}

fn row_params(r: &Row) -> rusqlite::Result<Params> {
    Ok(Params {
        id: r.get(0)?, seq: r.get(1)?, parent_id: r.get(2)?, fingerprint: r.get(3)?,
        json: r.get(4)?, created_at: r.get(5)?, note: r.get(6)?,
    })
}

// ---------- event state machine ----------

#[derive(serde::Deserialize)]
pub struct InitEv { pub params_id: String }
#[derive(serde::Deserialize)]
pub struct ParamEv { pub params_id: String, pub note: String }
#[derive(serde::Deserialize)]
pub struct DecideEv {
    pub target_kind: String,
    pub ckey: Option<String>,
    pub fkey: Option<String>,
    pub run_id: String,
    pub action: String,
    pub n1: Option<i64>,
    pub n2: Option<i64>,
    pub note: String,
}

impl App {
    pub fn require_branch(&self, project_id: &str, name: &str) -> rusqlite::Result<Branch> {
        self.conn.query_row(
            "SELECT * FROM branches WHERE id=?1 OR name=?2",
            params![id("branch", &[project_id, name]), name], row_branch)
    }
    pub fn main_branch(&self, project_id: &str) -> rusqlite::Result<Branch> {
        self.conn.query_row("SELECT * FROM branches WHERE name='main'", [], row_branch)
    }

    fn next_event_seq(&self, branch_id: &str) -> rusqlite::Result<i64> {
        self.conn.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch_id=?1",
            [branch_id], |r| r.get(0))
    }

    fn append(&self, branch: &Branch, kind: &str, payload: &serde_json::Value)
        -> rusqlite::Result<i64> {
        let seq = self.next_event_seq(&branch.id)?;
        db::append_event(&self.conn, &branch.id, seq, kind,
            &serde_json::to_string(payload).unwrap(), &self.clock.now())?;
        Ok(seq)
    }

    pub fn init_branch(&self, project_id: &str, params_id: &str) -> rusqlite::Result<i64> {
        let b = self.main_branch(project_id)?;
        if self.conn.query_row::<i64,_,_>(
            "SELECT COUNT(*) FROM events WHERE branch_id=?1", [&b.id], |r| r.get(0)).unwrap_or(0) > 0 {
            return Ok(0);
        }
        let payload = serde_json::json!({"params_id": params_id});
        self.append(&b, "init", &payload)
    }

    pub fn change_params(&self, branch_id: &str, params_id: &str, note: &str)
        -> rusqlite::Result<i64> {
        let b = self.get_branch(branch_id)?;
        self.append(&b, "parameter_change",
            &serde_json::json!({"params_id": params_id, "note": note}))
    }

    pub fn get_branch(&self, bid_or_name: &str) -> rusqlite::Result<Branch> {
        self.conn.query_row("SELECT * FROM branches WHERE id=?1 OR name=?1",
            [bid_or_name], row_branch)
    }

    pub fn fork(&self, source_branch_id: &str, at_seq: i64, new_name: &str, note: &str)
        -> rusqlite::Result<Branch> {
        let src = self.get_branch(source_branch_id)?;
        // copy prefix events into the new branch with identical seq numbers
        let nb = Branch {
            id: id("branch", &[&src.id, new_name, &at_seq.to_string()]),
            name: new_name.into(), parent_branch_id: Some(src.id.clone()),
            forked_from_event_seq: Some(at_seq), created_at: self.clock.now(),
            note: note.into(),
        };
        db::put_branch(&self.conn, &nb)?;
        let mut stmt = self.conn.prepare(
            "SELECT seq,kind,payload_json,at FROM events WHERE branch_id=?1 AND seq<=?2 ORDER BY seq")?;
        let rows: Vec<(i64,String,String,String)> = stmt.query_map(
            params![src.id, at_seq], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (seq, kind, payload, at) in rows {
            self.conn.execute("INSERT INTO events VALUES (?1,?2,?3,?4,?5)",
                params![seq, nb.id, kind, payload, at])?;
        }
        Ok(nb)
    }

    pub fn undo(&self, branch_id: &str, note: &str) -> rusqlite::Result<i64> {
        let b = self.get_branch(branch_id)?;
        self.append(&b, "undo", &serde_json::json!({"note": note}))
    }

    /// Record a human adjudication; rejected/accepted outcomes are new events,
    /// never overwrites. Repair offsets live in the offset layer only.
    pub fn decide(&self, branch_id: &str, ev: DecideEv) -> rusqlite::Result<i64> {
        let b = self.get_branch(branch_id)?;
        let payload = serde_json::to_value(&ev).unwrap();
        let seq = self.append(&b, "decision", &payload)?;

        // Persist a decision row keyed by stable target identity (ckey/fkey),
        // plus an offset-layer entry when a slip is accepted.
        let target_id = id("target", &[&ev.run_id, ev.ckey.as_deref().unwrap_or(""),
                                       ev.fkey.as_deref().unwrap_or("")]);
        let dec = Decision {
            id: id("dec", &[&b.id, &seq.to_string()]),
            branch_id: b.id.clone(), at_event_seq: seq,
            target_kind: ev.target_kind.clone(), target_id,
            target_run_id: ev.run_id.clone(), action: ev.action.clone(),
            n1: ev.n1, n2: ev.n2, params_id: None, note: ev.note.clone(),
            created_at: self.clock.now(),
        };
        db::put_decision(&self.conn, &dec)?;

        if ev.target_kind == "candidate" && ev.action == "accept" {
            if let (Some(n1), Some(n2), Some(ck)) = (ev.n1, ev.n2, ev.ckey.clone()) {
                // ckey = cand|<sat>|<epoch_id>|<n1>|<n2>
                let parts: Vec<&str> = ck.split('|').collect();
                if parts.len() == 5 {
                    let sat = parts[1].to_string();
                    let epoch_id = parts[2].to_string();
                    for (band, cycles) in [("L1", n1), ("L2", n2)] {
                        if cycles == 0 { continue; }
                        db::put_repair(&self.conn, &RepairOffset {
                            id: id("offset", &[&b.id, &sat, band, &epoch_id, &cycles.to_string()]),
                            branch_id: b.id.clone(), sat: sat.clone(),
                            band: band.into(), from_epoch_id: epoch_id.clone(),
                            cycles, layer: "offset_layer".into(), decision_id: dec.id.clone(),
                        })?;
                        let before = id("arc", &[&sat, band, &epoch_id, "gap-start"]);
                        let after = id("arc", &[&b.id, &sat, band, &epoch_id, "accepted-slip"]);
                        db::put_arc_revision(&self.conn, &ArcRevision {
                            id: id("arc_rev", &[&b.id, &seq.to_string(), &sat, band]),
                            branch_id: b.id.clone(), sat: sat.clone(), band: band.into(),
                            at_event_seq: seq, arc_before_id: before, arc_after_id: after,
                            decision_id: dec.id.clone(),
                            reason: format!("accepted slip ({},{}) via {}", n1, n2, ev.note),
                        })?;
                    }
                }
            }
        }
        Ok(seq)
    }
}

fn row_branch(r: &Row) -> rusqlite::Result<Branch> {
    Ok(Branch {
        id: r.get(0)?, name: r.get(1)?, parent_branch_id: r.get(2)?,
        forked_from_event_seq: r.get(3)?, created_at: r.get(4)?, note: r.get(5)?,
    })
}

// ---------- materialization / replay ----------

pub struct ViewState {
    pub branch: Branch,
    pub seq: i64,
    pub head_seq: i64,
    pub params: Params,
    pub det: DetParams,
    pub basis_commit_id: String,
    pub run_id: String,
    pub epochs: Vec<EpochRec>,
    pub obs: Vec<ObsRec>,
    pub findings: Vec<FindingView>,
    pub candidates: Vec<CandidateView>,
    pub arcs: Vec<ArcView>,
    pub events: Vec<EventRec>,
    pub decisions: Vec<Decision>,
    pub offsets: Vec<RepairOffset>,
    pub arc_revisions: Vec<ArcRevision>,
    pub clock_ledger: Vec<ClockLedgerRow>,
    pub nav: Vec<NavRecRow>,
    pub diagnostics: Vec<DiagnosticRec>,
    pub stale: bool,
    pub stale_reason: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct FindingView {
    #[serde(flatten)]
    pub f: Finding,
    pub key: String,
    pub is_current: bool,
    pub decision: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct CandidateView {
    #[serde(flatten)]
    pub c: Candidate,
    pub key: String,
    pub votes: Vec<String>,
    pub epoch_time: String,
    pub decision: Option<String>,
    pub is_current: bool,
}

#[derive(serde::Serialize, Clone)]
pub struct ArcView {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub start_epoch_id: String,
    pub start_time: String,
    pub end_epoch_id: String,
    pub end_time: String,
    pub reason_open: String,
    pub origin: String, // gap | accepted_slip
    pub is_current: bool,
}

impl App {
    /// Effective input basis: the most recent raw commit affecting observations.
    pub fn basis_commit_id(&self) -> rusqlite::Result<String> {
        self.conn.query_row(
            "SELECT id FROM raw_commits WHERE kind IN ('obs','obs_patch') ORDER BY seq DESC LIMIT 1",
            [], |r| r.get(0))
    }

    fn build_world(&self, project_id: &str) -> rusqlite::Result<EffectiveWorld> {
        let mut epochs: Vec<EpochRec> = {
            let mut s = self.conn.prepare("SELECT * FROM epochs ORDER BY ord")?;
            let v = s.query_map([], row_epoch)?.collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        epochs.sort_by(|a, b| a.time.cmp(&b.time));
        let times: Vec<String> = epochs.iter().map(|e| e.time.clone()).collect();
        let secs = engine::civil_to_seconds(&times);

        let mut obs_stmt = self.conn.prepare(
            "SELECT * FROM observations WHERE v IS NOT NULL ORDER BY sat,code")?;
        let obs: Vec<ObsRec> = obs_stmt.query_map([], row_obs)?
            .collect::<rusqlite::Result<_>>()?;

        // effective value per (epoch,sat,code): newest commit wins, but the
        // original row (id + raw token) is unchanged and provenance is tracked.
        let mut by_e: HashMap<String, Vec<&ObsRec>> = HashMap::new();
        for o in &obs { by_e.entry(o.epoch_id.clone()).or_default().push(o); }

        let mut eepochs = Vec::new();
        for (i, e) in epochs.iter().enumerate() {
            let mut sats: BTreeMap<String, SatEpoch> = BTreeMap::new();
            let mut latest: HashMap<String, &ObsRec> = HashMap::new();
            if let Some(rows) = by_e.get(&e.id) {
                for o in rows.clone() {
                    latest.entry(format!("{}|{}", o.sat, o.code))
                        .and_modify(|cur| {
                            if o.effective_commit_id > cur.effective_commit_id { *cur = o; }
                        }).or_insert(o);
                }
            }
            for o in latest.values() {
                let se = sats.entry(o.sat.clone()).or_default();
                let eff = EffObs {
                    obs_id: o.id.clone(),
                    commit_id: o.effective_commit_id.clone().unwrap_or_default(),
                    v: o.v.unwrap(), lli: o.lli.clone(),
                };
                let slot: &mut Option<EffObs> = match (o.kind.as_str(), o.band.as_str()) {
                    ("C", "L1") => &mut se.c1, ("C", "L2") => &mut se.c2,
                    ("L", "L1") => &mut se.l1, ("L", "L2") => &mut se.l2,
                    ("D", "L1") => &mut se.d1, ("D", "L2") => &mut se.d2,
                    _ => continue,
                };
                *slot = Some(eff);
                if o.kind == "L" && o.lli.is_some() {
                    if o.band == "L1" { se.l1_lli = o.lli.clone(); }
                    else { se.l2_lli = o.lli.clone(); }
                }
            }
            eepochs.push(EEpoch {
                id: e.id.clone(), ord: i as i64, t_s: secs[i],
                time: e.time.clone(), sats,
            });
        }

        let mut sat_state: HashMap<String, SatState> = HashMap::new();
        {
            let mut s = self.conn.prepare("SELECT sat,frequency_number FROM nav_records")?;
            let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i32>>(1)?)))?;
            for r in rows {
                let (sat, k) = r?;
                if k.is_some() { sat_state.entry(sat).or_default().glo_k = k; }
            }
        }
        let _ = project_id;
        Ok(EffectiveWorld { epochs: eepochs, sat_state })
    }
}

fn row_epoch(r: &Row) -> rusqlite::Result<EpochRec> {
    Ok(EpochRec {
        id: r.get(0)?, ord: r.get(1)?, time: r.get(2)?, time_raw: r.get(3)?,
        flag: r.get(4)?, clock_offset_raw: r.get(5)?, clock_offset_s: r.get(6)?,
    })
}
fn row_obs(r: &Row) -> rusqlite::Result<ObsRec> {
    Ok(ObsRec {
        id: r.get(0)?, epoch_id: r.get(1)?, sat: r.get(2)?, code: r.get(3)?,
        band: r.get(4)?, kind: r.get(5)?, raw: r.get(6)?, v: r.get(7)?,
        unit: r.get(8)?, lli: r.get(9)?, ssi: r.get(10)?, diagnostic: r.get(11)?,
        effective_commit_id: r.get(12)?,
    })
}

impl App {
    /// Replay a branch (optionally at an earlier event seq) and build the view.
    /// Runs are cached by (basis, params, algo) and never deleted: if the head
    /// basis/params moved on, the displayed run is marked stale.
    pub fn materialize(&self, project_id: &str, branch_id: &str, at_seq: Option<i64>)
        -> rusqlite::Result<ViewState> {
        let branch = if branch_id.is_empty() { self.main_branch(project_id)? }
                     else { self.get_branch(branch_id)? };
        let head_seq = self.conn.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM events WHERE branch_id=?1",
            [&branch.id], |r| r.get(0))?;
        let upto = at_seq.unwrap_or(head_seq).min(head_seq);

        let events: Vec<EventRec> = {
            let mut s = self.conn.prepare(
                "SELECT seq,branch_id,kind,payload_json,at FROM events
                 WHERE branch_id=?1 AND seq<=?2 ORDER BY seq")?;
            s.query_map(params![branch.id, upto], |r| Ok(EventRec {
                seq: r.get(0)?, branch_id: r.get(1)?, kind: r.get(2)?,
                payload_json: r.get(3)?, at: r.get(4)?,
            }))?.collect::<rusqlite::Result<_>>()?
        };

        // Determine parameter version at that point: last init/parameter_change,
        // honoring undo (skip the event being undone).
        let mut params_id: Option<String> = None;
        let mut skip_kind: Option<String> = None;
        for e in &events {
            match e.kind.as_str() {
                "init" => {
                    let v: InitEv = serde_json::from_str(&e.payload_json).unwrap();
                    params_id = Some(v.params_id);
                    skip_kind = None;
                }
                "parameter_change" => {
                    let v: ParamEv = serde_json::from_str(&e.payload_json).unwrap();
                    params_id = Some(v.params_id);
                }
                "undo" => { skip_kind = Some("last".into()); }
                _ => {}
            }
        }
        // Simple undo semantics: an undo event removes the effect of the last
        // parameter_change/decision for current-state computation (history rows
        // remain). Walk again to find the target seq.
        let undone_seqs = self.compute_undone(&events);
        let mut params_id: Option<String> = None;
        for e in &events {
            if undone_seqs.contains(&e.seq) { continue; }
            match e.kind.as_str() {
                "init" => {
                    let v: InitEv = serde_json::from_str(&e.payload_json).unwrap();
                    params_id = Some(v.params_id);
                }
                "parameter_change" => {
                    let v: ParamEv = serde_json::from_str(&e.payload_json).unwrap();
                    params_id = Some(v.params_id);
                }
                _ => {}
            }
        }
        let _ = skip_kind;
        let params = match params_id {
            Some(pid) => self.get_params_by_id(&pid)?,
            None => return Err(rusqlite::Error::from(
                rusqlite::ffi::Error::new(1))),
        };
        let det: DetParams = serde_json::from_str(&params.json).unwrap();

        // Decision events at this point in time (undo-aware).
        let decisions: Vec<Decision> = {
            let mut s = self.conn.prepare("SELECT * FROM decisions
                WHERE branch_id=?1 AND at_event_seq<=?2 ORDER BY at_event_seq")?;
            let all: Vec<Decision> = s.query_map(params![branch.id, upto], row_decision)?
                .collect::<rusqlite::Result<_>>()?;
            all.into_iter().filter(|d| !undone_seqs.contains(&d.at_event_seq)).collect()
        };

        let basis = self.basis_commit_id()?;
        let fp = format!("{}:{}", ALGO_VERSION, det.fingerprint());
        let run_id = id("run", &[&basis, &params.id]);

        // compute (cached)
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM runs WHERE id=?1", [&run_id], |r| r.get(0))?;
        if exists == 0 {
            let world = self.build_world(project_id)?;
            let out = engine::run_pipeline(&world, &det);
            db::put_run(&self.conn, &PipelineRun {
                id: run_id.clone(), basis_commit_id: basis.clone(),
                params_id: params.id.clone(), algo_version: ALGO_VERSION.into(),
                fingerprint: fp, created_at: self.clock.now(),
            })?;
            for f in &out.findings { db::put_finding(&self.conn, &run_id, f)?; }
            for c in &out.candidates { db::put_candidate(&self.conn, &run_id, c)?; }
        }

        // stale? current head basis or latest params differ
        let latest_basis = self.basis_commit_id().ok();
        let latest_params = self.latest_params_id()?.unwrap_or_default();
        let mut stale = false;
        let mut stale_reason = None;
        if latest_basis.as_deref() != Some(basis.as_str()) {
            stale = true;
            stale_reason = Some("source data changed after this run".into());
        }
        if latest_params != params.id {
            stale = true;
            stale_reason = Some("parameters changed after this run".into());
        }

        self.assemble_view(branch, upto, head_seq, params, det, basis, run_id,
                          events, decisions, stale, stale_reason)
    }

    fn compute_undone(&self, events: &[EventRec]) -> std::collections::BTreeSet<i64> {
        let mut undone = std::collections::BTreeSet::new();
        for e in events {
            if e.kind == "undo" {
                if let Some(target) = events.iter()
                    .filter(|x| x.seq < e.seq && !undone.contains(&x.seq)
                        && matches!(x.kind.as_str(), "decision" | "parameter_change"))
                    .last() {
                    undone.insert(target.seq);
                }
            }
        }
        undone
    }
}

fn row_decision(r: &Row) -> rusqlite::Result<Decision> {
    Ok(Decision {
        id: r.get(0)?, branch_id: r.get(1)?, at_event_seq: r.get(2)?,
        target_kind: r.get(3)?, target_id: r.get(4)?, target_run_id: r.get(5)?,
        action: r.get(6)?, n1: r.get(7)?, n2: r.get(8)?, params_id: r.get(9)?,
        note: r.get(10)?, created_at: r.get(11)?,
    })
}
