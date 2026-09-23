// SQLite persistence layer.
//
// Responsibilities:
//   * schema migration + project bootstrap
//   * append-only raw imports/amendments (verbatim lines, normalised values)
//   * current-view resolution (supersession by direct-source amendment)
//   * branches/events/params
//   * canonical export/import preserving stable identities, ordering, lineage

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{short_hash, DetectorParams, ALGO_VERSION};
use crate::parser::{parse, Directive};
use crate::schema::SCHEMA;
use crate::vtime::VirtualClock;

pub const C: f64 = 299_792_458.0;

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

// ---------- project ----------

pub fn ensure_project(
    conn: &mut Connection,
    clock: &mut VirtualClock,
    project_id: &str,
    name: &str,
    seed: i64,
) -> rusqlite::Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM project WHERE id=?1",
        params![project_id],
        |r| r.get(0),
    )?;
    if exists == 0 {
        conn.execute(
            "INSERT INTO project(id,name,created_at,seed,algo_version) VALUES(?1,?2,?3,?4,?5)",
            params![project_id, name, clock.tick(), seed, ALGO_VERSION],
        )?;
        // main branch
        conn.execute(
            "INSERT INTO branch(id,project_id,name,parent_branch_id,fork_after_seq)
             VALUES(?1,?2,'main',NULL,0)",
            params![format!("br-{}", short_hash(&format!("{project_id}|main"))), project_id],
        )?;
        // default params v1
        let p = DetectorParams::default();
        let fp = crate::models::param_fingerprint(&p);
        let pid = format!("pm-{}", short_hash(&format!("{project_id}|1|{fp}")));
        conn.execute(
            "INSERT INTO param_set(id,project_id,version_no,parent_id,label,fingerprint,params_json,created_at)
             VALUES(?1,?2,1,NULL,'default',?3,?4,?5)",
            params![pid, project_id, fp, p.to_json().to_string(), clock.tick()],
        )?;
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('project_id',?1)",
            params![project_id],
        )?;
    }
    Ok(())
}

pub fn project_id(conn: &Connection) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key='project_id'", [], |r| r.get(0))
        .optional()
}

pub fn main_branch_id(conn: &Connection, project: &str) -> rusqlite::Result<String> {
    conn.query_row(
        "SELECT id FROM branch WHERE project_id=?1 AND name='main'",
        params![project],
        |r| r.get(0),
    )
}

pub fn branch_id_by_name(conn: &Connection, project: &str, name: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM branch WHERE project_id=?1 AND name=?2",
        params![project, name],
        |r| r.get(0),
    )
    .optional()
}

pub fn next_seq(conn: &Connection, branch: &str) -> rusqlite::Result<i64> {
    let m: Option<i64> = conn
        .query_row("SELECT MAX(seq) FROM event WHERE branch_id=?1", params![branch], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(m.unwrap_or(0) + 1)
}

pub fn insert_event(
    conn: &mut Connection,
    clock: &mut VirtualClock,
    project: &str,
    branch: &str,
    seq: i64,
    kind: &str,
    label: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<String> {
    let eid = format!("ev-{}", short_hash(&format!("{branch}|{seq}|{kind}|{label}")));
    conn.execute(
        "INSERT INTO event(id,project_id,branch_id,seq,kind,label,payload_json,created_virtual_t)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            eid,
            project,
            branch,
            seq,
            kind,
            label,
            payload.to_string(),
            clock.tick()
        ],
    )?;
    Ok(eid)
}

pub fn events_on_branch(conn: &Connection, branch: &str) -> rusqlite::Result<Vec<(String, i64, String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id,seq,kind,label,payload_json FROM event WHERE branch_id=?1 ORDER BY seq",
    )?;
    let rows = stmt
        .query_map(params![branch], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- raw import / amendment ----------

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub submission_id: String,
    pub line_count: i64,
    pub error_count: i64,
    pub diagnostics: Vec<String>,
    pub mode: String,
    pub target_sat: Option<String>,
    pub target_epoch: Option<i64>,
}

pub fn import_submission(
    conn: &mut Connection,
    clock: &mut VirtualClock,
    project: &str,
    content: &str,
    kind: &str,            // import | amend_source
    label: &str,
    parent_submission: Option<&str>,
) -> rusqlite::Result<ImportResult> {
    // Submission identity is content-addressed + label, so identical imports
    // remain comparable while distinct amendments get distinct ids.
    let content_hash = short_hash(content);
    let sub_id = format!(
        "sb-{}",
        short_hash(&format!("{project}|{kind}|{label}|{content_hash}"))
    );

    let already: i64 = conn.query_row(
        "SELECT COUNT(*) FROM submission WHERE id=?1",
        params![sub_id],
        |r| r.get(0),
    )?;
    if already != 0 {
        return Ok(ImportResult {
            submission_id: sub_id,
            line_count: 0,
            error_count: 0,
            diagnostics: vec!["identical submission already present".to_string()],
            mode: "dedup".to_string(),
            target_sat: None,
            target_epoch: None,
        });
    }

    let parsed = parse(content, &sub_id);

    // Determine amendment metadata from AMEND directive if present.
    let mut mode: Option<String> = None;
    let mut target_sat: Option<String> = None;
    let mut target_epoch: Option<i64> = None;
    let mut reason: Option<String> = None;
    for pl in &parsed.lines {
        if let Some(Directive::Amend(a)) = &pl.directive {
            mode = Some(a.mode.clone());
            target_sat = Some(a.target_sat.clone());
            target_epoch = Some(a.epoch_idx);
            reason = Some(a.reason.clone());
        }
    }

    conn.execute(
        "INSERT INTO submission(id,project_id,kind,label,parent_id,mode,target_sat,target_epoch,reason,content_hash,received_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            sub_id, project, kind, label, parent_submission, mode, target_sat, target_epoch,
            reason, content_hash, clock.tick()
        ],
    )?;

    let mut error_count = 0i64;
    let mut diagnostics: Vec<String> = Vec::new();
    let mut current_epoch = 0i64;

    for pl in &parsed.lines {
        conn.execute(
            "INSERT INTO raw_line(id,submission_id,line_no,raw_text,directive,parse_status,diagnostic)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                pl.stable_line_id, sub_id, pl.line_no, pl.raw_text, pl.directive_name,
                pl.status, pl.diagnostic
            ],
        )?;
        if pl.status == "error" {
            error_count += 1;
            if let Some(d) = &pl.diagnostic {
                diagnostics.push(format!("line {}: {}", pl.line_no, d));
            }
        }
        match &pl.directive {
            Some(Directive::Epoch(e)) => {
                current_epoch = e.idx;
                let eid = crate::models::id_epoch(project, e.idx);
                conn.execute(
                    "INSERT OR IGNORE INTO epoch_rec(id,project_id,idx,t_seconds,iso_tag,rcvr_clk_s,clock_event_flag)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![eid, project, e.idx, e.t_seconds, e.iso_tag, e.rcvr_clk_s, e.clock_event_flag],
                )?;
            }
            Some(Directive::Station(rest)) => {
                for (k, v) in crate::parser::kv(rest) {
                    let id = format!("ha-{}", short_hash(&format!("{project}|STATION|{k}")));
                    conn.execute(
                        "INSERT OR IGNORE INTO header_attr(id,project_id,kind,key,value_raw)
                         VALUES(?1,?2,'STATION',?3,?4)",
                        params![id, project, k, v],
                    )?;
                }
            }
            Some(Directive::Datum(rest)) => {
                for (k, v) in crate::parser::kv(rest) {
                    let id = format!("ha-{}", short_hash(&format!("{project}|DATUM|{k}")));
                    conn.execute(
                        "INSERT OR IGNORE INTO header_attr(id,project_id,kind,key,value_raw)
                         VALUES(?1,?2,'DATUM',?3,?4)",
                        params![id, project, k, v],
                    )?;
                }
            }
            Some(Directive::Freq(f)) => {
                let wl = C / f.freq_hz;
                let fid = crate::models::id_freq(project, &f.gnss, &f.band);
                conn.execute(
                    "INSERT OR IGNORE INTO frequency_basis(id,project_id,gnss,band,freq_hz,wavelength_m,freq_number,codes_raw)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![fid, project, f.gnss, f.band, f.freq_hz, wl, f.freq_number, f.codes_raw],
                )?;
            }
            Some(Directive::Clock(cd)) => {
                let cid = format!(
                    "ce-{}",
                    short_hash(&format!("{project}|{sub_id}|{}|{}", cd.epoch_idx, cd.delta_s_raw))
                );
                conn.execute(
                    "INSERT INTO clock_event(id,project_id,submission_id,epoch_idx,delta_s_raw,scope_raw,cause_raw)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![cid, project, sub_id, cd.epoch_idx, cd.delta_s_raw, cd.scope_raw, cd.cause_raw],
                )?;
            }
            _ => {}
        }
    }

    // Insert observations and resolve supersession.
    for pl in &parsed.lines {
        if let Some(Directive::Obs(o)) = &pl.directive {
            let epoch = if o.epoch_idx > 0 { o.epoch_idx } else { current_epoch };
            let oid = crate::models::id_obs(project, &sub_id, &o.sat, epoch, &o.band);
            let mut value_status = "ok".to_string();
            let mut value_diag: Option<String> = None;
            let phase = match crate::parser::normalise_f64(&o.code_phase) {
                Ok(v) => Some(v),
                Err(e) => {
                    value_status = "unconvertible".to_string();
                    value_diag = Some(e);
                    None
                }
            };
            let pseudo = o.code_pseudo.as_ref().and_then(|x| x.parse::<f64>().ok());
            let doppler = o.code_doppler.as_ref().and_then(|x| x.parse::<f64>().ok());
            let lli = o.code_lli.as_ref().and_then(|x| x.parse::<i64>().ok());
            if o.code_pseudo.is_some() && pseudo.is_none() {
                value_status = "unconvertible".to_string();
                value_diag = Some(format!("bad pseudorange token"));
            }
            conn.execute(
                "INSERT INTO observation(id,project_id,submission_id,line_id,epoch_idx,sat,gnss,band,
                   code_phase,code_pseudo,code_doppler,code_snr,code_lli,phase_cy,pseudo_m,doppler_hz,lli_bits,
                   value_status,value_diag,is_current)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,1)",
                params![
                    oid, project, sub_id, pl.stable_line_id, epoch, o.sat, o.gnss, o.band,
                    o.code_phase, o.code_pseudo, o.code_doppler, o.code_snr, o.code_lli,
                    phase, pseudo, doppler, lli, value_status, value_diag
                ],
            )?;
        }
    }

    // Apply supersession for direct-source amendments.
    if mode.as_deref() == Some("supersede_direct_source") {
        if let (Some(sat), Some(ep)) = (target_sat, target_epoch) {
            conn.execute(
                "UPDATE observation SET is_current=0, superseded_by=?1
                 WHERE project_id=?2 AND sat=?3 AND epoch_idx=?4 AND is_current=1 AND id<>?1
                   AND submission_id<>?1",
                params![sub_id, project, sat, ep],
            )?;
            // New rows are already is_current=1; ensure older stay stale.
            conn.execute(
                "UPDATE observation SET is_current=0, superseded_by=?1
                 WHERE project_id=?2 AND sat=?3 AND epoch_idx=?4 AND submission_id<>?1",
                params![sub_id, project, sat, ep],
            )?;
        }
    }

    Ok(ImportResult {
        submission_id: sub_id,
        line_count: parsed.lines.len() as i64,
        error_count,
        diagnostics,
        mode: mode.unwrap_or_else(|| "full".to_string()),
        target_sat,
        target_epoch,
    })
}

// ---------- current effective view ----------

pub fn current_observations(
    conn: &Connection,
    project: &str,
) -> rusqlite::Result<Vec<crate::models::ObsRow>> {
    let mut stmt = conn.prepare(
        "SELECT id,submission_id,line_id,epoch_idx,sat,gnss,band,code_phase,code_pseudo,
                code_doppler,code_snr,code_lli,phase_cy,pseudo_m,doppler_hz,lli_bits,value_status
         FROM observation WHERE project_id=?1 AND is_current=1
         ORDER BY epoch_idx, sat, band",
    )?;
    let rows = stmt
        .query_map(params![project], |r| {
            Ok(crate::models::ObsRow {
                id: r.get(0)?,
                submission_id: r.get(1)?,
                line_id: r.get(2)?,
                epoch_idx: r.get(3)?,
                sat: r.get(4)?,
                gnss: r.get(5)?,
                band: r.get(6)?,
                code_phase: r.get(7)?,
                code_pseudo: r.get(8)?,
                code_doppler: r.get(9)?,
                code_snr: r.get(10)?,
                code_lli: r.get(11)?,
                phase_cy: r.get(12)?,
                pseudo_m: r.get(13)?,
                doppler_hz: r.get(14)?,
                lli_bits: r.get(15)?,
                value_status: r.get(16)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn epochs(conn: &Connection, project: &str) -> rusqlite::Result<Vec<crate::models::EpochInfo>> {
    let mut stmt = conn.prepare(
        "SELECT idx,t_seconds,iso_tag,rcvr_clk_s,clock_event_flag FROM epoch_rec
         WHERE project_id=?1 ORDER BY idx",
    )?;
    let rows = stmt
        .query_map(params![project], |r| {
            Ok(crate::models::EpochInfo {
                idx: r.get(0)?,
                t_seconds: r.get(1)?,
                iso_tag: r.get(2)?,
                rcvr_clk_s: r.get(3)?,
                clock_event_flag: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn frequencies(conn: &Connection, project: &str) -> rusqlite::Result<Vec<crate::models::FrequencyBasis>> {
    let mut stmt = conn.prepare(
        "SELECT gnss,band,freq_hz,wavelength_m,freq_number,codes_raw
         FROM frequency_basis WHERE project_id=?1 ORDER BY gnss,band",
    )?;
    let rows = stmt
        .query_map(params![project], |r| {
            Ok(crate::models::FrequencyBasis {
                gnss: r.get(0)?,
                band: r.get(1)?,
                freq_hz: r.get(2)?,
                wavelength_m: r.get(3)?,
                freq_number: r.get(4)?,
                codes_raw: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn clock_events(conn: &Connection, project: &str) -> rusqlite::Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT epoch_idx,delta_s_raw,scope_raw FROM clock_event WHERE project_id=?1 ORDER BY epoch_idx",
    )?;
    let rows = stmt
        .query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- parameter versions ----------

pub fn latest_param_version(conn: &Connection, project: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(version_no),1) FROM param_set WHERE project_id=?1",
        params![project],
        |r| r.get(0),
    )
}

pub fn get_params_at_version(
    conn: &Connection,
    project: &str,
    version_no: i64,
) -> rusqlite::Result<(DetectorParams, String)> {
    let (json, fp): (String, String) = conn.query_row(
        "SELECT params_json,fingerprint FROM param_set WHERE project_id=?1 AND version_no=?2",
        params![project, version_no],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or(serde_json::json!({}));
    let map = v.as_object().cloned().unwrap_or_default();
    Ok((DetectorParams::from_map(&map), fp))
}

pub fn add_param_version(
    conn: &mut Connection,
    clock: &mut VirtualClock,
    project: &str,
    label: &str,
    params: &DetectorParams,
) -> rusqlite::Result<(String, i64, String)> {
    let next = latest_param_version(conn, project)? + 1;
    let fp = crate::models::param_fingerprint(params);
    let parent = format!(
        "pm-{}",
        short_hash(&format!("{project}|{}|parent", next - 1))
    );
    // Find actual parent id.
    let parent_id: Option<String> = conn
        .query_row(
            "SELECT id FROM param_set WHERE project_id=?1 AND version_no=?2",
            params![project, next - 1],
            |r| r.get(0),
        )
        .optional()?;
    let _ = parent;
    let id = format!("pm-{}", short_hash(&format!("{project}|{next}|{fp}")));
    conn.execute(
        "INSERT INTO param_set(id,project_id,version_no,parent_id,label,fingerprint,params_json,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            id, project, next, parent_id, label, fp, params.to_json().to_string(), clock.tick()
        ],
    )?;
    Ok((id, next, fp))
}

// Effective parameter version at a given event on a branch: follow the
// timeline and report the last revise_params at or before that seq.
pub fn effective_param_version(
    conn: &Connection,
    branch: &str,
    project: &str,
    up_to_seq: i64,
) -> rusqlite::Result<i64> {
    let mut ver: i64 = 1;
    for (_id, seq, kind, _label, payload) in events_on_branch(conn, branch)? {
        if seq > up_to_seq {
            break;
        }
        if kind == "revise_params" {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
                if let Some(n) = v.get("param_version").and_then(|x| x.as_i64()) {
                    ver = n;
                }
            }
        }
    }
    let _ = project;
    Ok(ver)
}

// ---------- branches ----------

pub fn create_branch(
    conn: &mut Connection,
    clock: &mut VirtualClock,
    project: &str,
    name: &str,
    fork_after_seq: i64,
) -> rusqlite::Result<(String, String)> {
    let parent = main_branch_id(conn, project)?;
    // Forks always derive from main in this workbench (single-level review fork),
    // but the lineage columns stay generic.
    let bid = format!("br-{}", short_hash(&format!("{project}|{name}")));
    let seq = fork_after_seq + 1;
    conn.execute(
        "INSERT INTO branch(id,project_id,name,parent_branch_id,fork_after_seq)
         VALUES(?1,?2,?3,?4,?5)",
        params![bid, project, name, parent, fork_after_seq],
    )?;
    let payload = serde_json::json!({
        "fork_name": name,
        "parent_branch": "main",
        "fork_after_seq": fork_after_seq,
    });
    let eid = insert_event(conn, clock, project, &bid, seq, "fork", &format!("fork:{name}"), &payload)?;
    conn.execute("UPDATE branch SET created_event_id=?1 WHERE id=?2", params![eid, bid])?;
    Ok((bid, eid))
}

pub fn list_branches(conn: &Connection, project: &str) -> rusqlite::Result<Vec<(String, String, Option<String>, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT id,name,parent_branch_id,fork_after_seq FROM branch WHERE project_id=?1 ORDER BY name",
    )?;
    let rows = stmt
        .query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------- canonical export / import ----------
//
// The archive is a single JSON document. Tables are exported in foreign-key
// dependency order with their primary keys intact, so deleting the database and
// re-importing restores identical identities, ordering and lineage.

const TABLES_IN_ORDER: &[&str] = &[
    "project",
    "branch",
    "param_set",
    "submission",
    "raw_line",
    "epoch_rec",
    "frequency_basis",
    "header_attr",
    "observation",
    "clock_event",
    "event",
    "run",
    "arc",
    "residual",
    "candidate",
    "offset_layer",
    "judgement",
    "meta",
];

fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(cols)
}

pub fn export_archive(conn: &Connection) -> rusqlite::Result<serde_json::Value> {
    let mut tables = serde_json::Map::new();
    for t in TABLES_IN_ORDER {
        let cols = table_columns(conn, t)?;
        let sql = format!("SELECT * FROM {t}");
        let mut stmt = conn.prepare(&sql)?;
        let n = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                let mut obj = serde_json::Map::new();
                for i in 0..n {
                    let name = r.column_type(i)?;
                    let val = match name {
                        rusqlite::types::Type::Null => serde_json::Value::Null,
                        rusqlite::types::Type::Integer => serde_json::Value::from(r.get::<_, i64>(i)?),
                        rusqlite::types::Type::Real => serde_json::Value::from(r.get::<_, f64>(i)?),
                        rusqlite::types::Type::Text => serde_json::Value::String(r.get::<_, String>(i)?),
                        rusqlite::types::Type::Blob => {
                            let bytes = r.get::<_, Vec<u8>>(i)?;
                            serde_json::Value::String(String::from_utf8_lossy(&bytes).to_string())
                        }
                    };
                    obj.insert(stmt.column_name(i)?.to_string(), val);
                }
                Ok(serde_json::Value::Object(obj))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let _ = cols;
        tables.insert((*t).to_string(), serde_json::Value::Array(rows));
    }
    Ok(serde_json::json!({
        "format": "carrier-break-bench-archive/1",
        "algo_version": ALGO_VERSION,
        "export_order": TABLES_IN_ORDER,
        "tables": tables,
    }))
}

pub fn import_archive(conn: &mut Connection, archive: &serde_json::Value) -> rusqlite::Result<usize> {
    let tables = archive
        .get("tables")
        .and_then(|v| v.as_object())
        .ok_or_else(|| rusqlite::Error::InvalidQuery)?;

    let tx = conn.unchecked_transaction()?;
    tx.pragma_update(None, "foreign_keys", "OFF")?;
    let mut total = 0usize;
    for t in TABLES_IN_ORDER {
        let Some(arr) = tables.get(*t).and_then(|v| v.as_array()) else {
            continue;
        };
        for row in arr {
            let obj = row.as_object().ok_or(rusqlite::Error::InvalidQuery)?;
            let cols = obj.keys().cloned().collect::<Vec<_>>();
            let col_sql = cols.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(",");
            let ph = (0..cols.len()).map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("INSERT OR REPLACE INTO {t}({col_sql}) VALUES({ph})");
            let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            for c in &cols {
                let v = obj.get(c).cloned().unwrap_or(serde_json::Value::Null);
                param_values.push(json_to_sql(v));
            }
            let refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
            tx.execute(&sql, refs.as_slice())?;
            total += 1;
        }
    }
    tx.pragma_update(None, "foreign_keys", "ON")?;
    tx.commit()?;
    Ok(total)
}

fn json_to_sql(v: serde_json::Value) -> Box<dyn rusqlite::ToSql> {
    match v {
        serde_json::Value::Null => Box::new(Option::<String>::None),
        serde_json::Value::Bool(b) => Box::new(b as i64),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else {
                Box::new(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Box::new(s),
        other => Box::new(other.to_string()),
    }
}
