//! Application services: immutable imports, rule revisions, decisions and
//! derived persistence with local expiration.

use crate::anyhow_lite::{err, Result};
use crate::db;
use crate::engine::{self, AcceptedOffset, Snapshot};
use crate::model::*;
use crate::replay::ReplayInput;
use crate::storage_writer;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Mutex;

/// Deterministic, seedable clock used by tests and the demo. It is a virtual
/// clock; no system time is read.
pub struct VirtualClock {
    now_ms: Mutex<i64>,
    step_ms: i64,
}
impl VirtualClock {
    pub fn new(seed_iso: &str, step_ms: i64) -> Result<Self> {
        Ok(VirtualClock { now_ms: Mutex::new(iso_to_ms(seed_iso)?), step_ms })
    }
    pub fn tick(&self) -> i64 {
        let mut g = self.now_ms.lock().unwrap();
        let v = *g;
        *g += self.step_ms;
        v
    }
}

pub fn iso_to_ms(iso: &str) -> Result<i64> {
    // 2026-01-01T00:00:00Z -> unix ms (civil calendar algorithm).
    let (date, time) = iso.split_once('T').ok_or_else(|| err("bad seed time"))?;
    let mut dp = date.split('-');
    let y: i32 = dp.next().ok_or_else(|| err("bad year"))?.parse().map_err(|_| err("bad year"))?;
    let m: u32 = dp.next().ok_or_else(|| err("bad month"))?.parse().map_err(|_| err("bad month"))?;
    let d: u32 = dp.next().ok_or_else(|| err("bad day"))?.parse().map_err(|_| err("bad day"))?;
    let t = time.trim_end_matches('Z');
    let mut tp = t.split(':');
    let hh: i64 = tp.next().unwrap_or("0").parse().unwrap_or(0);
    let mm: i64 = tp.next().unwrap_or("0").parse().unwrap_or(0);
    let ss: f64 = tp.next().unwrap_or("0").parse().unwrap_or(0.0);
    let days = civil_days(y, m, d);
    Ok(days * 86_400_000 + hh * 3_600_000 + mm * 60_000 + (ss * 1000.0) as i64)
}

fn civil_days(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let doy = (153 * (m as i64 + if m > 2 { -3 } else { 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe - 719468
}

pub struct App {
    pub conn: Mutex<Connection>,
    pub clock: VirtualClock,
}

impl App {
    pub fn open(path: &str, seed_iso: &str) -> Result<Self> {
        let conn = db::open(path)?;
        Ok(App { conn: Mutex::new(conn), clock: VirtualClock::new(seed_iso, 1200)? })
    }
    pub fn in_memory(seed_iso: &str) -> Result<Self> {
        let conn = db::open_in_memory()?;
        Ok(App { conn: Mutex::new(conn), clock: VirtualClock::new(seed_iso, 1200)? })
    }

    pub fn create_project(&self, name: &str) -> Result<Value> {
        let id = short_id("proj", &[name]);
        let now = self.clock.tick();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO projects(id,name,seed,created_at_ms) VALUES (?1,?2,?3,?4)",
            rusqlite::params![id, name, SEED, now],
        )?;
        // Initial rule version.
        let params = DetectorParams::default();
        let fp = rule_fingerprint(&params);
        let rule_id = short_id("rule", &[id, "0", &canonical_json(&serde_json::to_value(&params)?)]);
        tx.execute(
            "INSERT INTO rule_versions(id,project_id,seq,parent_id,fingerprint,params_json,note,created_at_ms)
             VALUES (?1,?2,0,NULL,?3,?4,'initial',?5)",
            rusqlite::params![rule_id, id, fp, canonical_json(&serde_json::to_value(&params)?), now],
        )?;
        let event_id = short_id("evt", &[id, "0", "RULE_REVISION", &rule_id]);
        append_event_tx(&tx, id, 0, now, "RULE_REVISION", &json!({
            "rule_id": rule_id, "seq": 0, "fingerprint": fp,
            "params": params, "note": "initial rule",
            "algorithm_version": ALGORITHM_VERSION,
        }), &event_id)?;
        tx.commit()?;
        Ok(json!({"project_id": id, "name": name, "initial_rule_id": rule_id}))
    }

    /// Submit observation text. `kind` is OBS_FULL or OBS_PATCH. The submitted
    /// text is stored byte-for-byte and never overwritten.
    pub fn import_obs_text(&self, project_id: &str, kind: &str, text: &str, note: &str) -> Result<Value> {
        if kind != "OBS_FULL" && kind != "OBS_PATCH" {
            return Err(err("kind must be OBS_FULL or OBS_PATCH"));
        }
        let file = crate::parser::parse_rinex_obs(text).map_err(|errs| {
            err(&format!(
                "RINEX format rejected ({}): {}",
                errs.len(),
                errs.iter().take(3).map(|e| format!("[L{} {}] {}", e.line.unwrap_or(0), e.code, e.message)).collect::<Vec<_>>().join("; ")
            ))
        })?;
        let now = self.clock.tick();
        let mut conn = self.conn.lock().unwrap();
        let file_diag = file.diagnostics.clone();
        let n_epochs = file.epochs.len();
        let batch_id = insert_import_batch(&mut conn, project_id, kind, text, note, now, |tx, batch_id, _seq, _parent| {
            for e in &file.epochs {
                tx.execute(
                    "INSERT INTO raw_epochs(project_id,batch_id,seq,epoch,recv_clock_offset_s) VALUES (?1,?2,?3,?4,?5)",
                    rusqlite::params![project_id, batch_id, e.seq, e.epoch, e.recv_clock_offset_s],
                )?;
                for o in &e.obs {
                    let obs_id = raw_obs_id(project_id, batch_id, e.seq, &o.sat, &o.code);
                    tx.execute(
                        "INSERT INTO raw_obs(id,project_id,batch_id,epoch_seq,epoch,sat,code,band,freq_hz,
                           raw_field,raw_value,lli,ssi_raw,kind,diagnostic_code,diagnostic_message)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                        rusqlite::params![
                            obs_id, project_id, batch_id, e.seq, e.epoch, o.sat, o.code, o.band, o.freq_hz,
                            o.raw_field, o.raw_value, o.lli, o.ssi_raw,
                            serde_variant(&o.kind),
                            o.diagnostic.as_ref().map(|d| d.code.clone()),
                            o.diagnostic.as_ref().map(|d| d.message.clone())
                        ],
                    )?;
                }
            }
            for d in &file.diagnostics {
                tx.execute(
                    "INSERT INTO import_diagnostics(project_id,batch_id,severity,code,line,message)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![project_id, batch_id, d.severity, d.code, d.line, d.message],
                )?;
            }
            Ok::<_, rusqlite::Error>(())
        })?;
        let result = recompute_persist(&mut conn, project_id)?;
        Ok(json!({"imported":"obs","batch_id":batch_id,"epochs":n_epochs,
                  "diagnostics":file_diag,"recompute":result}))
    }

    /// Import receiver clock event log (one JSON object per line).
    pub fn import_clock_text(&self, project_id: &str, text: &str, note: &str) -> Result<Value> {
        let mut events: Vec<ClockEvent> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let ev: ClockEvent = serde_json::from_str(line).map_err(|e| {
                err(&format!("clock event line {}: {e}", i + 1))
            })?;
            events.push(ev);
        }
        let now = self.clock.tick();
        let mut conn = self.conn.lock().unwrap();
        let n = events.len();
        let batch_id = insert_import_batch(&mut conn, project_id, "CLOCK", text, note, now, |tx, batch_id, _s, _p| {
            for (i, ev) in events.iter().enumerate() {
                let id = short_id("clock", &[project_id, batch_id, &i.to_string(), &ev.epoch, &ev.event]);
                tx.execute(
                    "INSERT INTO raw_clock_events(id,project_id,batch_id,epoch,event,jump_s,source,detail_json)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    rusqlite::params![id, project_id, batch_id, ev.epoch, ev.event, ev.jump_s,
                        ev.source, canonical_json(&ev.detail)],
                )?;
            }
            Ok(())
        })?;
        let result = recompute_persist(&mut conn, project_id)?;
        Ok(json!({"imported":"clock","batch_id":batch_id,"events":n,"recompute":result}))
    }

    /// Import satellite state log (one JSON object per line), positions carry
    /// a coordinate datum explicitly.
    pub fn import_sat_state_text(&self, project_id: &str, text: &str, note: &str) -> Result<Value> {
        let mut states: Vec<SatelliteState> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let st: SatelliteState = serde_json::from_str(line).map_err(|e| {
                err(&format!("satellite state line {}: {e}", i + 1))
            })?;
            states.push(st);
        }
        let now = self.clock.tick();
        let mut conn = self.conn.lock().unwrap();
        let n = states.len();
        let _batch = insert_import_batch(&mut conn, project_id, "SAT_STATE", text, note, now, |tx, batch_id, _s, _p| {
            for (i, st) in states.iter().enumerate() {
                let id = short_id("sat", &[project_id, batch_id, &i.to_string(), &st.epoch, &st.sat]);
                tx.execute(
                    "INSERT INTO raw_sat_states(id,project_id,batch_id,epoch,sat,pos_x_m,pos_y_m,pos_z_m,datum,health,source)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    rusqlite::params![id, project_id, batch_id, st.epoch, st.sat,
                        st.position_xyz_m[0], st.position_xyz_m[1], st.position_xyz_m[2],
                        st.datum, st.health, st.source],
                )?;
            }
            Ok(())
        })?;
        Ok(json!({"imported":"sat_state","states":n}))
    }

const SEED: &str = "fixed-seed-2026-01-01T00:00:00Z";

fn serde_variant(k: &ObsKind) -> &'static str {
    match k {
        ObsKind::Pseudorange => "pseudorange",
        ObsKind::Carrier => "carrier",
        ObsKind::Doppler => "doppler",
        ObsKind::Ssi => "ssi",
        ObsKind::Unconvertible => "unconvertible",
    }
}

pub fn rule_fingerprint(p: &DetectorParams) -> String {
    short_id("fp", &[ALGORITHM_VERSION, &canonical_json(&serde_json::to_value(p).unwrap_or(Value::Null))])
}

pub fn raw_obs_id(project_id: &str, batch_id: &str, epoch_seq: i64, sat: &str, code: &str) -> String {
    short_id("obs", &[project_id, batch_id, &epoch_seq.to_string(), sat, code])
}

pub fn append_event_tx(
    tx: &rusqlite::Transaction,
    project_id: &str,
    seq: i64,
    at_ms: i64,
    kind: &str,
    payload: &Value,
    event_id: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO events(id,project_id,seq,at_ms,kind,payload_json) VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![event_id, project_id, seq, at_ms, kind, canonical_json(payload)],
    )?;
    Ok(())
}

fn next_batch_seq(tx: &rusqlite::Transaction, project_id: &str) -> rusqlite::Result<i64> {
    tx.query_row(
        "SELECT COALESCE(MAX(seq)+1,0) FROM batches WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )
}
fn next_event_seq(tx: &rusqlite::Transaction, project_id: &str) -> rusqlite::Result<i64> {
    tx.query_row(
        "SELECT COALESCE(MAX(seq)+1,0) FROM events WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )
}
fn last_batch_id(tx: &rusqlite::Transaction, project_id: &str) -> rusqlite::Result<Option<String>> {
    tx.query_row(
        "SELECT id FROM batches WHERE project_id=?1 ORDER BY seq DESC LIMIT 1",
        [project_id],
        |r| r.get(0),
    )
}

fn insert_import_batch<F>(
    conn: &mut Connection,
    project_id: &str,
    kind: &str,
    text: &str,
    note: &str,
    now: i64,
    write_rows: F,
) -> Result<String>
where
    F: FnOnce(&rusqlite::Transaction, &str, i64, Option<String>) -> std::result::Result<(), rusqlite::Error>,
{
    let tx = conn.transaction()?;
    let seq = next_batch_seq(&tx, project_id)?;
    let parent_id = last_batch_id(&tx, project_id)?;
    let content_hash = short_id("txt", &[text]);
    let batch_id = short_id("batch", &[project_id, &seq.to_string(), kind, &content_hash]);
    let event_seq = next_event_seq(&tx)?;
    tx.execute(
        "INSERT INTO batches(id,project_id,seq,parent_id,kind,original_text,content_hash,created_at_ms,created_by,note)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'reviewer',?9)",
        rusqlite::params![batch_id, project_id, seq, parent_id, kind, text, content_hash, now, note],
    )?;
    write_rows(&tx, &batch_id, seq, parent_id.clone())?;
    let event_id = short_id("evt", &[project_id, &event_seq.to_string(), "IMPORT", &batch_id]);
    append_event_tx(&tx, project_id, event_seq, now, "IMPORT", &json!({
        "batch_id": batch_id, "batch_seq": seq, "kind": kind, "parent_id": parent_id,
        "content_hash": content_hash, "note": note,
    }), &event_id)?;
    tx.commit()?;
    Ok(batch_id)
}

/// Recompute derived results, marking only rows whose direct sources or rule
/// changed as stale (local expiration); unchanged rows remain current.
pub fn recompute_persist(conn: &mut Connection, project_id: &str) -> Result<Value> {
    storage_writer::recompute(conn, project_id)
}

pub fn load_input(conn: &Connection, project_id: &str, head: Option<i64>) -> Result<ReplayInput> {
    Ok(crate::replay::load_replay_input(conn, project_id, head)?)
}

pub fn current_snapshot(conn: &Connection, project_id: &str) -> Result<(Snapshot, Vec<AcceptedOffset>)> {
    let input = load_input(conn, project_id, None)?;
    let accepted = accepted_offsets(&input);
    let snap = engine::compute_snapshot(&input, &accepted)?;
    Ok((snap, accepted))
}

pub fn accepted_offsets(input: &ReplayInput) -> Vec<AcceptedOffset> {
    input
        .decisions
        .iter()
        .filter(|d| d.action == "ACCEPT")
        .map(|d| AcceptedOffset {
            candidate_natural: d.candidate_natural.clone(),
            proposal: d.proposal.clone().unwrap_or_else(|| json!({"n1":0,"n2":0})),
        })
        .collect()
}

impl App {
    /// A parameter revision is a new rule version with a parent; it never
    /// mutates the previous parameters.
    pub fn revise_rule(&self, project_id: &str, params: DetectorParams, note: &str) -> Result<Value> {
        let mut conn = self.conn.lock().unwrap();
        let now = self.clock.tick();
        let (seq, parent_id): (i64, String) = {
            let tx = conn.transaction()?;
            let seq = tx.query_row(
                "SELECT COALESCE(MAX(seq)+1,0) FROM rule_versions WHERE project_id=?1",
                [project_id], |r| r.get(0))?;
            let parent_id = tx.query_row(
                "SELECT id FROM rule_versions WHERE project_id=?1 ORDER BY seq DESC LIMIT 1",
                [project_id], |r| r.get(0))?;
            let fp = rule_fingerprint(&params);
            let id = short_id("rule", &[project_id, &seq.to_string(), &canonical_json(&serde_json::to_value(&params)?)]);
            tx.execute(
                "INSERT INTO rule_versions(id,project_id,seq,parent_id,fingerprint,params_json,note,created_at_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![id, project_id, seq, parent_id, fp,
                    canonical_json(&serde_json::to_value(&params)?), note, now])?;
            let event_seq = next_event_seq(&tx)?;
            let event_id = short_id("evt", &[project_id, &event_seq.to_string(), "RULE_REVISION", &id]);
            append_event_tx(&tx, project_id, event_seq, now, "RULE_REVISION", &json!({
                "rule_id": id, "seq": seq, "parent_id": parent_id, "fingerprint": fp,
                "params": params, "note": note, "algorithm_version": ALGORITHM_VERSION,
            }), &event_id)?;
            tx.commit()?;
            (seq, id)
        };
        let result = recompute_persist(&mut conn, project_id)?;
        Ok(json!({"rule_seq":seq,"rule_id":parent_id,"recompute":result}))
    }

    /// Record an ACCEPT/REJECT decision on one natural candidate. Acceptance
    /// stores an offset-layer row only; phase rows are never rewritten.
    pub fn decide(&self, project_id: &str, candidate_natural: &str, action: &str,
                  proposal: Option<Value>, reason: &str) -> Result<Value> {
        if action != "ACCEPT" && action != "REJECT" {
            return Err(err("action must be ACCEPT or REJECT"));
        }
        let mut conn = self.conn.lock().unwrap();
        let now = self.clock.tick();
        let (snap, _) = current_snapshot(&conn, project_id)?;
        let cand = snap.candidates.iter().find(|c| {
            natural_id_with_rule(&c.natural_id_input, &snap.rule_id) == candidate_natural
        }).ok_or_else(|| err("candidate not found in current snapshot"))?;
        let prop = if action == "ACCEPT" {
            let p = proposal.clone().ok_or_else(|| err("ACCEPT requires a proposal"))?;
            let n1 = p.get("n1").and_then(|v| v.as_i64()).ok_or_else(|| err("proposal.n1 missing"))?;
            let n2 = p.get("n2").and_then(|v| v.as_i64()).ok_or_else(|| err("proposal.n2 missing"))?;
            if !cand.proposals.iter().any(|pr| pr.n1 == n1 && pr.n2 == n2) {
                return Err(err("proposal is not among the legal candidates"));
            }
            Some(p)
        } else { None };
        let tx = conn.transaction()?;
        let event_seq = next_event_seq(&tx)?;
        let event_id = short_id("evt", &[project_id, &event_seq.to_string(), action, candidate_natural]);
        let decision_id = short_id("dec", &[candidate_natural, action, &event_seq.to_string()]);
        tx.execute(
            "INSERT INTO decisions(id,project_id,candidate_id,action,proposal_json,reason,rule_id_at_time,event_id,created_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![decision_id, project_id, candidate_natural, action,
                prop.as_ref().map(canonical_json), reason, snap.rule_id, event_id, now])?;
        if action == "ACCEPT" {
            let p = prop.as_ref().unwrap();
            for (band, key) in [("1", "n1"), ("2", "n2")] {
                let cycles = p[key].as_f64().unwrap_or(0.0);
                if cycles != 0.0 {
                    let off_id = short_id("off", &[decision_id, band]);
                    tx.execute(
                        "INSERT INTO repair_offsets(id,project_id,candidate_id,decision_id,sat,band,cycles,unit,status)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,'cycle','current')",
                        rusqlite::params![off_id, project_id, candidate_natural, decision_id,
                            cand.sat, band, cycles])?;
                }
            }
        }
        append_event_tx(&tx, project_id, event_seq, now, "DECISION", &json!({
            "candidate_id": candidate_natural, "action": action, "proposal": prop,
            "reason": reason, "rule_id": snap.rule_id, "sat": cand.sat,
            "epoch_seq": cand.epoch_seq,
        }), &event_id)?;
        tx.commit()?;
        Ok(json!({"decision_id":decision_id,"event_seq":event_seq}))
    }

    /// Undo a decision. The undo is itself a historical event; rows stay.
    pub fn undo_decision(&self, project_id: &str, decision_event_seq: i64, reason: &str) -> Result<Value> {
        let mut conn = self.conn.lock().unwrap();
        let now = self.clock.tick();
        let tx = conn.transaction()?;
        let event_id_target: String = tx.query_row(
            "SELECT id FROM events WHERE project_id=?1 AND seq=?2 AND kind='DECISION'",
            rusqlite::params![project_id, decision_event_seq], |r| r.get(0))
            .map_err(|_| err("decision event not found"))?;
        let event_seq = next_event_seq(&tx)?;
        let event_id = short_id("evt", &[project_id, &event_seq.to_string(), "UNDO", &event_id_target]);
        tx.execute("UPDATE decisions SET undone_by_event_id=?1 WHERE event_id=?2 AND project_id=?3",
            rusqlite::params![event_id, event_id_target, project_id])?;
        tx.execute("UPDATE repair_offsets SET status='superseded' WHERE project_id=?1 AND decision_id IN
                    (SELECT id FROM decisions WHERE event_id=?2)",
            rusqlite::params![project_id, event_id_target])?;
        append_event_tx(&tx, project_id, event_seq, now, "DECISION_UNDO", &json!({
            "decision_event_seq": decision_event_seq, "reason": reason,
        }), &event_id)?;
        tx.commit()?;
        let result = recompute_persist(&mut conn, project_id)?;
        Ok(json!({"event_seq":event_seq,"recompute":result}))
    }

    /// Time-travel: reconstruct the visible conclusion at a past event without
    /// touching the database.
    pub fn view_at(&self, project_id: &str, head: i64) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        let input = load_input(&conn, project_id, Some(head))?;
        let accepted = accepted_offsets(&input);
        let snap = engine::compute_snapshot(&input, &accepted)?;
        Ok(crate::state_json::snapshot_to_json(&snap, &accepted, Some(head)))
    }
}

pub fn natural_id_with_rule(natural: &str, rule_id: &str) -> String {
    short_id("cand", &[natural, rule_id])
}
