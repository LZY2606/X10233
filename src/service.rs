use std::sync::{Arc, Mutex};

use crate::clock::Clock;

pub struct ProjectService {
    pub conn: Mutex<rusqlite::Connection>,
    pub clock: Arc<dyn Clock>,
}

impl ProjectService {
    pub fn open(
        path: &str,
        clock: Arc<dyn Clock>,
        _rng_seed: u64,
    ) -> crate::error::Result<Self> {
        use rusqlite::OpenFlags;
        let conn = rusqlite::Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        Ok(Self { conn: Mutex::new(conn), clock })
    }
}
