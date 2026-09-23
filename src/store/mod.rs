//! SQLite persistence: immutable inputs, versioned revisions, append-only
//! event/branch log, derived compute sets with provenance and local expiry,
//! and a stable-identity export/import round trip.

use crate::util::Clock;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

pub mod ingest;
pub mod derive;
pub mod review;
pub mod exchange;
pub mod query;

pub const ALGO_VERSION: &str = "engine-gfmwtd-1.0.0";

pub fn fingerprint(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    let mut s = String::with_capacity(16);
    for b in d.iter().take(8) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

pub struct Store {
    pub conn: Mutex<Connection>,
    pub clock: Arc<dyn Clock>,
}

impl Store {
    pub fn open(path: &str, clock: Arc<dyn Clock>) -> rusqlite::Result<Arc<Store>> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        let store = Arc::new(Store {
            conn: Mutex::new(conn),
            clock,
        });
        store.init()?;
        Ok(store)
    }

    pub fn open_in_memory(clock: Arc<dyn Clock>) -> rusqlite::Result<Arc<Store>> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Arc::new(Store {
            conn: Mutex::new(conn),
            clock,
        });
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> rusqlite::Result<()> {
        let schema = include_str!("schema.sql");
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(schema)?;
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM branch WHERE branch_id='main'",
            [],
            |r| r.get(0),
        )?;
        if exists == 0 {
            conn.execute(
                "INSERT INTO branch(branch_id, seq, parent_branch_id, name, created_ms)
                 VALUES ('main', 1, NULL, 'main', ?)",
                [self.clock.now_ms()],
            )?;
        }
        Ok(())
    }

    pub fn next_seq(&self, table: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        let col = match table {
            "imports" => "seq",
            "branch" => "seq",
            "obs_value" => "seq",
            _ => "seq",
        };
        let sql = format!("SELECT COALESCE(MAX({col}),0)+1 FROM {table}");
        conn.query_row(&sql, [], |r| r.get(0)).unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::VirtualClock;

    #[test]
    fn opens_and_seeds_main_branch() {
        let clk = Arc::new(VirtualClock::new(1_700_000_000_000));
        let s = Store::open_in_memory(clk).unwrap();
        let n: i64 = s
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM branch", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
