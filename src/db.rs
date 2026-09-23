//! SQLite 持久层。原始输入一经提交不可覆盖；修订以新版本 + 父版本关系表达。
//! 所有写入携带全局单调序号 seq（虚拟时间），重放 = 按 seq 过滤可见集合。

use rusqlite::{params, Connection, OptionalExtension};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS projects(
  uid TEXT PRIMARY KEY, name TEXT NOT NULL, created_seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sources(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, kind TEXT NOT NULL,
  version INTEGER NOT NULL, parent_version INTEGER, sha256 TEXT NOT NULL,
  raw TEXT NOT NULL, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS epochs(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, source_uid TEXT NOT NULL,
  station TEXT NOT NULL, sat TEXT NOT NULL, epoch_ms INTEGER NOT NULL,
  code TEXT NOT NULL, freq INTEGER NOT NULL,
  pr_m REAL, carrier REAL, doppler REAL, lli INTEGER NOT NULL, snr REAL,
  diag TEXT, raw_line TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_epochs ON epochs(project, station, sat, freq, epoch_ms);
CREATE TABLE IF NOT EXISTS clock_events(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, source_uid TEXT NOT NULL,
  station TEXT NOT NULL, epoch_ms INTEGER NOT NULL, jump_ms REAL NOT NULL, raw_line TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS nav_states(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, source_uid TEXT NOT NULL,
  sat TEXT NOT NULL, epoch_ms INTEGER NOT NULL,
  x_m REAL, y_m REAL, z_m REAL, health INTEGER NOT NULL, diag TEXT, raw_line TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS params(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, version INTEGER NOT NULL,
  parent_version INTEGER, json TEXT NOT NULL, fingerprint TEXT NOT NULL, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS candidates(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, branch TEXT NOT NULL,
  arc_key TEXT NOT NULL, station TEXT NOT NULL, sat TEXT NOT NULL, freq INTEGER,
  epoch_ms INTEGER NOT NULL, detector TEXT NOT NULL, combo TEXT NOT NULL,
  est_cycles REAL, est_int INTEGER, score REAL NOT NULL, reason TEXT,
  ambig_group TEXT, params_fp TEXT NOT NULL, source_version INTEGER NOT NULL, seq INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS idx_cand ON candidates(project, branch, station, sat);
CREATE TABLE IF NOT EXISTS status_events(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, branch TEXT NOT NULL,
  candidate_uid TEXT NOT NULL, status TEXT NOT NULL, reason TEXT,
  actor TEXT NOT NULL, note TEXT, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS repairs(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, branch TEXT NOT NULL,
  arc_key TEXT NOT NULL, station TEXT NOT NULL, sat TEXT NOT NULL, freq INTEGER,
  combo TEXT NOT NULL, from_epoch_ms INTEGER NOT NULL, cycles REAL NOT NULL,
  candidate_uid TEXT, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS decisions(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, branch TEXT NOT NULL,
  candidate_uid TEXT NOT NULL, action TEXT NOT NULL, note TEXT,
  actor TEXT NOT NULL, parent_uid TEXT, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS branches(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, name TEXT NOT NULL,
  parent_uid TEXT, fork_seq INTEGER NOT NULL, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS events(
  uid TEXT PRIMARY KEY, project TEXT NOT NULL, branch TEXT, seq INTEGER NOT NULL,
  kind TEXT NOT NULL, payload TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_events ON events(project, seq);
"#;

pub struct Engine {
    pub conn: Connection,
}

#[derive(Debug)]
pub enum EngineError {
    Import(Vec<crate::model::Diag>),
    NotFound(String),
    Conflict(String),
    BadInput(String),
    Other(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Import(ds) => write!(f, "import failed: {} diagnostic(s)", ds.len()),
            EngineError::NotFound(s) => write!(f, "not found: {}", s),
            EngineError::Conflict(s) => write!(f, "conflict: {}", s),
            EngineError::BadInput(s) => write!(f, "bad input: {}", s),
            EngineError::Other(s) => write!(f, "error: {}", s),
        }
    }
}
impl std::error::Error for EngineError {}

impl From<rusqlite::Error> for EngineError {
    fn from(e: rusqlite::Error) -> Self {
        EngineError::Other(e.to_string())
    }
}
impl From<serde_json::Error> for EngineError {
    fn from(e: serde_json::Error) -> Self {
        EngineError::Other(e.to_string())
    }
}

pub type Res<T> = Result<T, EngineError>;

impl Engine {
    pub fn open(path: &str) -> Res<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn memory() -> Res<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Res<Self> {
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(k,v) VALUES('seq','0')",
            [],
        )?;
        Ok(Engine { conn })
    }

    /// 全局单调序号：虚拟时间。所有排序与重放以它为准。
    pub fn next_seq(&self) -> Res<i64> {
        self.conn.execute(
            "UPDATE meta SET v = CAST(CAST(v AS INTEGER) + 1 AS TEXT) WHERE k='seq'",
            [],
        )?;
        let v: String = self
            .conn
            .query_row("SELECT v FROM meta WHERE k='seq'", [], |r| r.get(0))?;
        v.parse::<i64>().map_err(|e| EngineError::Other(e.to_string()))
    }

    pub fn max_seq(&self) -> Res<i64> {
        let v: String = self
            .conn
            .query_row("SELECT v FROM meta WHERE k='seq'", [], |r| r.get(0))?;
        v.parse::<i64>().map_err(|e| EngineError::Other(e.to_string()))
    }

    pub fn uid(&self, prefix: &str, seq: i64) -> String {
        format!("{}-{:06}", prefix, seq)
    }

    pub fn project_exists(&self, uid: &str) -> Res<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE uid=?1",
            params![uid],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// 分支链：从请求分支回溯到 main，返回 (branch_uid, cutoff_seq)。
    /// 子分支可见父分支 fork_seq 之前的全部行。
    pub fn branch_chain(&self, project: &str, branch: &str) -> Res<Vec<(String, i64)>> {
        let mut chain = Vec::new();
        let mut cur = branch.to_string();
        let mut cutoff = i64::MAX;
        for _ in 0..64 {
            let row: Option<(Option<String>, i64)> = self
                .conn
                .query_row(
                    "SELECT parent_uid, fork_seq FROM branches WHERE project=?1 AND uid=?2",
                    params![project, cur],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match row {
                Some((parent, fork_seq)) => {
                    chain.push((cur.clone(), cutoff));
                    match parent {
                        Some(p) => {
                            cutoff = fork_seq;
                            cur = p;
                        }
                        None => break,
                    }
                }
                None => return Err(EngineError::NotFound(format!("branch {}", cur))),
            }
        }
        Ok(chain)
    }

    /// 分支可见性 SQL 片段（用于 candidates/status_events/repairs/decisions/events）。
    pub fn visibility_sql(chain: &[(String, i64)]) -> (String, Vec<rusqlite::types::Value>) {
        let mut frag = String::new();
        let mut vals: Vec<rusqlite::types::Value> = Vec::new();
        for (i, (b, c)) in chain.iter().enumerate() {
            if i > 0 {
                frag.push_str(" OR ");
            }
            frag.push_str("(branch = ? AND seq <= ?)");
            vals.push(b.clone().into());
            vals.push((*c).into());
        }
        (frag, vals)
    }

    pub fn record_event(
        &self,
        project: &str,
        branch: Option<&str>,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Res<i64> {
        let seq = self.next_seq()?;
        let uid = self.uid("ev", seq);
        self.conn.execute(
            "INSERT INTO events(uid,project,branch,seq,kind,payload) VALUES(?1,?2,?3,?4,?5,?6)",
            params![uid, project, branch, seq, kind, payload.to_string()],
        )?;
        Ok(seq)
    }

    pub fn latest_source(&self, project: &str, kind: &str) -> Res<Option<(String, i64)>> {
        self.conn
            .query_row(
                "SELECT uid, version FROM sources WHERE project=?1 AND kind=?2 \
                 ORDER BY version DESC LIMIT 1",
                params![project, kind],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn latest_params(&self, project: &str) -> Res<Option<(String, i64, String, String)>> {
        self.conn
            .query_row(
                "SELECT uid, version, json, fingerprint FROM params WHERE project=?1 \
                 ORDER BY version DESC LIMIT 1",
                params![project],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(Into::into)
    }
}
