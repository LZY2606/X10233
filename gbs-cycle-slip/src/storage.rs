//! SQLite 持久层：项目、分支、只增事件日志、原始批次、派生行世代。
//!
//! 派生行采用 `first_gen / last_gen` 区间表示可见世代：
//! - 局部输入变化只使依赖它的行失效（last_gen 落定），其余行身份不变；
//! - 过期行仍保留在库里，可按旧世代查看，但不会出现在当前视图。

use crate::domain::ParsedBatch;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA: &str = r#"
PRAGMA journal_mode=WAL;

CREATE TABLE IF NOT EXISTS projects (
    project_uid TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    seed_ms     INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    created_ms  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS branches (
    project_uid       TEXT NOT NULL REFERENCES projects(project_uid),
    branch_uid        TEXT NOT NULL,
    name              TEXT NOT NULL,
    parent_branch_uid TEXT,
    created_event_uid TEXT,
    created_at        TEXT,
    PRIMARY KEY (project_uid, branch_uid),
    UNIQUE (project_uid, name)
);

CREATE TABLE IF NOT EXISTS events (
    project_uid TEXT NOT NULL,
    branch_uid  TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    event_uid   TEXT NOT NULL,
    kind        TEXT NOT NULL,
    payload     TEXT NOT NULL,
    recorded_at TEXT NOT NULL,
    recorded_ms INTEGER NOT NULL,
    PRIMARY KEY (project_uid, branch_uid, seq)
);

CREATE TABLE IF NOT EXISTS raw_batches (
    project_uid       TEXT NOT NULL,
    branch_uid        TEXT NOT NULL,
    seq               INTEGER NOT NULL,
    batch_uid         TEXT NOT NULL,
    kind              TEXT NOT NULL,
    filename          TEXT NOT NULL,
    parent_batch_uid  TEXT,
    content_text      TEXT NOT NULL,
    accepted          INTEGER NOT NULL,
    diagnostics_json  TEXT NOT NULL,
    committed_at      TEXT NOT NULL,
    committed_ms      INTEGER NOT NULL,
    PRIMARY KEY (project_uid, branch_uid, seq)
);

CREATE TABLE IF NOT EXISTS raw_observations (
    project_uid  TEXT NOT NULL,
    batch_uid    TEXT NOT NULL,
    ordinal      INTEGER NOT NULL,
    station      TEXT NOT NULL,
    sat          TEXT NOT NULL,
    system       TEXT NOT NULL,
    freq_num     TEXT NOT NULL,
    code         TEXT NOT NULL,
    epoch_tag    TEXT NOT NULL,
    epoch_offset REAL,
    pseudorange_m REAL,
    pseudorange_raw TEXT NOT NULL,
    phase_cycles REAL,
    phase_raw TEXT NOT NULL,
    doppler_hz REAL,
    doppler_raw TEXT NOT NULL,
    lli INTEGER,
    lli_raw TEXT NOT NULL,
    snr_db_hz REAL,
    snr_raw TEXT NOT NULL,
    raw_line TEXT NOT NULL,
    PRIMARY KEY (project_uid, batch_uid, ordinal)
);

CREATE TABLE IF NOT EXISTS raw_sat_states (
    project_uid TEXT NOT NULL,
    batch_uid   TEXT NOT NULL,
    ordinal     INTEGER NOT NULL,
    sat         TEXT NOT NULL,
    epoch_tag   TEXT NOT NULL,
    epoch_offset REAL,
    health_raw  TEXT NOT NULL,
    tokens_json TEXT NOT NULL,
    raw_line    TEXT NOT NULL,
    PRIMARY KEY (project_uid, batch_uid, ordinal)
);

CREATE TABLE IF NOT EXISTS raw_clock_events (
    project_uid TEXT NOT NULL,
    batch_uid   TEXT NOT NULL,
    ordinal     INTEGER NOT NULL,
    applies_to  TEXT NOT NULL,
    epoch_tag   TEXT NOT NULL,
    epoch_offset REAL,
    jump_ms     REAL,
    jump_ms_raw TEXT NOT NULL,
    raw_line    TEXT NOT NULL,
    PRIMARY KEY (project_uid, batch_uid, ordinal)
);

CREATE TABLE IF NOT EXISTS generations (
    project_uid TEXT NOT NULL,
    branch_uid  TEXT NOT NULL,
    gen_no      INTEGER NOT NULL,
    event_uid   TEXT NOT NULL,
    note        TEXT NOT NULL,
    PRIMARY KEY (project_uid, branch_uid, gen_no)
);

CREATE TABLE IF NOT EXISTS derived_rows (
    project_uid  TEXT NOT NULL,
    branch_uid   TEXT NOT NULL,
    row_uid      TEXT NOT NULL,
    kind         TEXT NOT NULL,
    sort_key     TEXT NOT NULL,
    dep_json     TEXT NOT NULL,
    data_json    TEXT NOT NULL,
    first_gen    INTEGER NOT NULL,
    last_gen     INTEGER,
    PRIMARY KEY (project_uid, branch_uid, row_uid)
);

CREATE INDEX IF NOT EXISTS idx_derived_lookup
    ON derived_rows(project_uid, branch_uid, kind, first_gen, last_gen);
CREATE INDEX IF NOT EXISTS idx_obs_identity
    ON raw_observations(project_uid, station, sat, freq_num, code, epoch_tag);
"#;

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        conn.pragma_update(None, "busy_timeout", 10_000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// 测试用内存库（配合临时目录的调用方也可以改用 open）。
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    pub fn insert_parsed_batch(
        &mut self,
        project_uid: &str,
        batch_uid: &str,
        parsed: &ParsedBatch,
    ) -> rusqlite::Result<()> {
        for o in &parsed.observations {
            self.conn.execute(
                "INSERT INTO raw_observations VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    project_uid,
                    batch_uid,
                    o.ordinal,
                    o.station,
                    o.sat,
                    o.system,
                    o.freq_num,
                    o.code,
                    o.epoch_tag,
                    o.epoch_offset_s,
                    o.pseudorange_m,
                    o.pseudorange_raw,
                    o.phase_cycles,
                    o.phase_raw,
                    o.doppler_hz,
                    o.doppler_raw,
                    o.lli,
                    o.lli_raw,
                    o.snr_db_hz,
                    o.snr_raw,
                    o.raw_line,
                ],
            )?;
        }
        for s in &parsed.sat_states {
            self.conn.execute(
                "INSERT INTO raw_sat_states VALUES (?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    project_uid,
                    batch_uid,
                    s.ordinal,
                    s.sat,
                    s.epoch_tag,
                    s.epoch_offset_s,
                    s.health_raw,
                    serde_json::to_string(&s.tokens).unwrap(),
                    s.raw_line,
                ],
            )?;
        }
        for c in &parsed.clock_events {
            self.conn.execute(
                "INSERT INTO raw_clock_events VALUES (?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    project_uid,
                    batch_uid,
                    c.ordinal,
                    c.applies_to,
                    c.epoch_tag,
                    c.epoch_offset_s,
                    c.jump_ms,
                    c.jump_ms_raw,
                    c.raw_line,
                ],
            )?;
        }
        Ok(())
    }
}
