//! Application engine: import (immutable raw layer), event-sourced revisions,
//! multi-detector cycle-slip candidates, arc local expiry and stable
//! identity. The engine never treats one detector as ground truth.

use crate::db;
use crate::hash::{id, id_bytes, VirtualClock};
use crate::model::*;
use crate::parse;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

pub struct Engine<'a> {
    pub conn: &'a mut Connection,
    pub clock: VirtualClock,
}

fn db_err(e: rusqlite::Error) -> ApiError {
    ApiError { status: 500, message: format!("sqlite: {e}") }
}

pub fn hex_hash(bytes: &[u8]) -> String {
    format!("{:016x}", crate::hash::fnv1a64(bytes))
}

#[derive(Debug, serde::Serialize)]
pub struct ImportReport {
    pub input_id: String,
    pub raw_fields: i64,
    pub epochs: i64,
    pub diagnostics: i64,
    pub recompute: RecomputeReport,
}

#[derive(Debug, serde::Serialize, Default, Clone)]
pub struct RecomputeReport {
    pub arcs: usize,
    pub pair_points: usize,
    pub candidates: usize,
    pub stale_sats: Vec<String>,
}

impl<'a> Engine<'a> {
    fn insert_clock_event(&mut self, pid: &str, input_id: &str, ev: &ClockEvent)
        -> ApiResult<()>
    {
        let cid = id("clk", &[pid, input_id, &ev.at_second.to_string(),
                              &ev.magnitude_s.to_string(), &ev.scope]);
        self.conn.execute(
            "INSERT INTO clock_events(id,project_id,input_id,at_second,magnitude_s,
             scope,raw_text) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![cid, pid, input_id, ev.at_second, ev.magnitude_s,
                ev.scope, ev.raw_text],
        ).map_err(db_err)?;
        Ok(())
    }

    fn insert_diagnostic(&mut self, pid: &str, input_id: &str, d: &ImportDiagnostic)
        -> ApiResult<()>
    {
        let did = id("dia", &[pid, input_id, &d.line_no.to_string(), &d.code, &d.message]);
        self.conn.execute(
            "INSERT INTO import_diagnostics(id,project_id,input_id,line_no,severity,
             code,message,raw_text) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![did, pid, input_id, d.line_no, d.severity, d.code,
                d.message, d.raw_text],
        ).map_err(db_err)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn bad(msg: impl Into<String>) -> Self {
        ApiError { status: 400, message: msg.into() }
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        ApiError { status: 409, message: msg.into() }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        ApiError { status: 404, message: msg.into() }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

type ApiResult<T> = Result<T, ApiError>;

impl<'a> Engine<'a> {
    pub fn new(conn: &'a mut Connection, clock: VirtualClock) -> Self {
        Engine { conn, clock }
    }

    pub fn create_project(&mut self, name: &str) -> ApiResult<String> {
        let now = self.clock.now_ns();
        let pid = id("prj", &[name, &now.to_string()]);
        let payload = json!({"name": name}).to_string();
        let eid = id_bytes("ev",
            format!("{pid}\u{1f}CREATE\u{1f}{now}\u{1f}{payload}").as_bytes());
        self.conn.execute(
            "INSERT INTO events(id,project_id,seq,parent_id,kind,occurred_ns,payload_json)
             VALUES(?1,?2,0,NULL,'PROJECT_CREATED',?3,?4)",
            params![eid, pid, now, payload],
        ).map_err(db_err)?;
        self.conn.execute(
            "INSERT INTO projects(id,name,created_event_id,head_event_id)
             VALUES(?1,?2,?3,?3)",
            params![pid, name, eid],
        ).map_err(db_err)?;
        Ok(pid)
    }

    fn append_event(&mut self, pid: &str, kind: &str, payload: serde_json::Value)
        -> ApiResult<(String, i64)>
    {
        let parent: Option<String> = self.conn.query_row(
            "SELECT head_event_id FROM projects WHERE id=?1", params![pid],
            |r| r.get::<_, String>(0)).ok();
        let seq: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE project_id=?1",
            params![pid], |r| r.get(0)).unwrap_or(1);
        let now = self.clock.now_ns();
        let pj = payload.to_string();
        let eid = id_bytes("ev",
            format!("{pid}\u{1f}{parent:?}\u{1f}{kind}\u{1f}{now}\u{1f}{pj}").as_bytes());
        self.conn.execute(
            "INSERT INTO events(id,project_id,seq,parent_id,kind,occurred_ns,payload_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![eid, pid, seq, parent, kind, now, pj],
        ).map_err(db_err)?;
        self.conn.execute("UPDATE projects SET head_event_id=?1 WHERE id=?2",
            params![eid, pid]).map_err(db_err)?;
        Ok((eid, seq))
    }

    fn next_input_seq(&self, pid: &str) -> i64 {
        self.conn.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM input_versions WHERE project_id=?1",
            params![pid], |r| r.get(0)).unwrap_or(1)
    }

    /// Import a RINEX observation document plus satellite state. Raw inputs
    /// are immutable: an amendment is a new version with a parent relation.
    pub fn import_rinex(&mut self, pid: &str, filename: &str, rinex_text: &str,
                        state_text: Option<&str>) -> ApiResult<ImportReport> {
        let doc = match parse::parse_rinex(rinex_text) {
            Ok(d) => d,
            Err(diags) => {
                return Err(ApiError {
                    status: 400,
                    message: serde_json::json!({"errors": diags}).to_string(),
                });
            }
        };
        let state = state_text.map(parse::parse_state);
        let sha = hex_hash(rinex_text.as_bytes());
        let seq = self.next_input_seq(pid);
        let input_id = id("inp", &[pid, "RINEX", &seq.to_string(), &sha]);
        let tx_payload = json!({
            "input_id": input_id, "filename": filename, "kind": "RINEX",
            "seq": seq, "sha256": sha, "amendment_of": doc.amendment_of,
            "station": doc.station,
        });
        let (event_id, _) = self.append_event(pid, "RAW_IMPORTED", tx_payload)?;

        self.conn.execute(
            "INSERT INTO input_versions(id,project_id,seq,filename,kind,amendment_of,
             sha256,imported_event_id) VALUES(?1,?2,?3,?4,'RINEX',?5,?6,?7)",
            params![input_id, pid, seq, filename, doc.amendment_of, sha, event_id],
        ).map_err(db_err)?;

        for (i, line) in doc.raw_header_lines.iter().enumerate() {
            let lid = id("rln", &[&input_id, &(i as i64 + 1).to_string(), line]);
            self.conn.execute(
                "INSERT INTO raw_lines(id,project_id,input_id,line_no,raw_text)
                 VALUES(?1,?2,?3,?4,?5)",
                params![lid, pid, input_id, (i as i64) + 1, line],
            ).map_err(db_err)?;
        }

        for ev in &doc.clock_events {
            self.insert_clock_event(pid, &input_id, ev)?;
        }

        let mut raw_count = 0i64;
        for ep in &doc.epochs {
            for se in &ep.sat_epochs {
                for f in &se.fields {
                    let oid = id("obs", &[
                        &input_id, &ep.gps_ns.to_string(), &se.sat, &f.code,
                        f.raw_text.trim_end()]);
                    self.conn.execute(
                        "INSERT INTO raw_obs(id,project_id,input_id,gps_ns,epoch_line_no,
                         sat,code,kind,raw_text,value,lli,signal_strength,unit,diagnostic)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                        params![oid, pid, input_id, ep.gps_ns, ep.line_no, se.sat,
                            f.code, format!("{:?}", f.kind), f.raw_text, f.value,
                            f.lli, f.ss, f.kind.unit(), f.diagnostic],
                    ).map_err(db_err)?;
                    raw_count += 1;
                    if f.lli != 0 {
                        // first-class LLI indicator retained for review
                    }
                }
            }
        }

        for d in &doc.diagnostics {
            self.insert_diagnostic(pid, &input_id, d)?;
        }

        if let Some(st) = state {
            let state_id = id("inp", &[pid, "STATE", &seq.to_string()]);
            for (i, line) in st.raw_lines.iter().enumerate() {
                let lid = id("rln", &[&state_id, &(i as i64 + 1).to_string(), line]);
                self.conn.execute(
                    "INSERT INTO raw_lines(id,project_id,input_id,line_no,raw_text)
                     VALUES(?1,?2,?3,?4,?5)",
                    params![lid, pid, state_id, (i as i64) + 1, line],
                ).map_err(db_err)?;
            }
            for ev in &st.clock_events {
                self.insert_clock_event(pid, &state_id, ev)?;
            }
            for f in &st.frequencies {
                let band = if (f.freq_mhz - 1575.42).abs() < 1.0 { "L1" } else { "L2" };
                let fid = id("frq", &[pid, &f.sat, band, &f.freq_mhz.to_string()]);
                self.conn.execute(
                    "INSERT INTO frequencies(id,project_id,input_id,sat,band,freq_hz,
                     freq_number,wavelength_m,raw_text)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![fid, pid, state_id, f.sat, band, f.freq_mhz * 1e6,
                        f.freq_number, f.wavelength_m, f.raw_text],
                ).map_err(db_err)?;
            }
            for d in &st.diagnostics {
                self.insert_diagnostic(pid, &state_id, d)?;
            }
        }

        let report = self.recompute(pid)?;
        Ok(ImportReport {
            input_id, raw_fields: raw_count,
            epochs: doc.epochs.len() as i64,
            diagnostics: doc.diagnostics.len() as i64,
            recompute: report,
        })
    }
