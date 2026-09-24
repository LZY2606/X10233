//! SQLite 持久化：原始层只追加、派生层带版本/指纹/直接来源。

use crate::input::RawBatch;
use std::sync::Mutex;

pub struct Db {
    pub conn: Mutex<rusqlite::Connection>,
}

const SCHEMA: &str = include_str!("../schema.sql");

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Db> {
        let conn = rusqlite::Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    pub fn in_memory() -> rusqlite::Result<Db> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    /// 写入一批原始输入（不可变）。返回 (batch_id, seq)。
    pub fn insert_batch(&self, project_id: &str, label: &str, batch: &RawBatch) -> rusqlite::Result<(String, i64)> {
        use crate::id::id;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM batches WHERE project_id = ?1",
            rusqlite::params![project_id], |r| r.get(0))?;
        let batch_id = id("bat", &[project_id.as_bytes(), label.as_bytes(), &seq.to_le_bytes()]);

        tx.execute("UPDATE batches SET superseded = 1 WHERE project_id = ?1 AND superseded = 0",
            rusqlite::params![project_id])?;
        tx.execute(
            "INSERT INTO batches (id, project_id, seq, label, raw_text, parent_batch_id, created_via)
             VALUES (?1,?2,?3,?4,?5,(SELECT id FROM batches WHERE project_id=?2 AND seq=?3-1),'import')",
            rusqlite::params![batch_id, project_id, seq, label, "" /* raw_text 在调用处写入以保持简单 */],
        )?;

        // 历元锚点
        let mut epoch_ids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut insert_epoch = |tx: &rusqlite::Transaction, iso: &str, micros: i64| {
            if epoch_ids.contains_key(iso) { return Ok(()); }
            let eid = id("epo", &[project_id.as_bytes(), iso.as_bytes()]);
            tx.execute("INSERT INTO epochs (id, project_id, batch_seq, micros, iso)
                        VALUES (?1,?2,?3,?4,?5)
                        ON CONFLICT(id) DO UPDATE SET batch_seq = excluded.batch_seq",
                rusqlite::params![eid, project_id, seq, micros, iso])?;
            epoch_ids.insert(iso.to_string(), eid);
            Ok::<_, rusqlite::Error>(())
        };

        for o in &batch.obs_records {
            if let Some(micros) = crate::input::parse_epoch(&o.epoch_iso) {
                insert_epoch(&tx, &o.epoch_iso, micros)?;
                let eid = epoch_ids.get(&o.epoch_iso).unwrap();
                let oid = id("obs", &[project_id.as_bytes(), o.sat.as_bytes(), o.epoch_iso.as_bytes(), o.code.as_bytes()]);
                tx.execute(
                    "INSERT OR REPLACE INTO obs
                     (id, project_id, batch_seq, epoch_id, micros, sat, code, band, pseudo, phase, doppler, lli, lli_raw, snr, raw_line, status)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,'active')",
                    rusqlite::params![
                        oid, project_id, seq, eid, micros, o.sat, o.code, o.band,
                        serde_json::to_string(&o.pseudo).unwrap(),
                        serde_json::to_string(&o.phase).unwrap(),
                        serde_json::to_string(&o.doppler).unwrap(),
                        o.lli, o.lli_raw,
                        serde_json::to_string(&o.snr).unwrap(),
                        o.line_no],
                )?;
            }
        }
        for c in &batch.clock_events {
            if let Some(micros) = crate::input::parse_epoch(&c.epoch_iso) {
                insert_epoch(&tx, &c.epoch_iso, micros)?;
                let eid = epoch_ids.get(&c.epoch_iso).unwrap();
                let cid = id("clk", &[project_id.as_bytes(), c.epoch_iso.as_bytes(), c.kind.as_bytes()]);
                tx.execute(
                    "INSERT OR REPLACE INTO clock_events (id, project_id, batch_seq, epoch_id, micros, kind, delta, note, raw_line)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    rusqlite::params![cid, project_id, seq, eid, micros, c.kind,
                        serde_json::to_string(&c.delta).unwrap(), c.note, c.line_no],
                )?;
            }
        }
        for s in &batch.sat_states {
            if let Some(micros) = crate::input::parse_epoch(&s.epoch_iso) {
                insert_epoch(&tx, &s.epoch_iso, micros)?;
                let eid = epoch_ids.get(&s.epoch_iso).unwrap();
                let sid = id("sat", &[project_id.as_bytes(), s.sat.as_bytes(), s.epoch_iso.as_bytes()]);
                tx.execute(
                    "INSERT OR REPLACE INTO sat_states
                     (id, project_id, batch_seq, epoch_id, micros, sat, x, y, z, clock_bias, datum, health, raw_line)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                    rusqlite::params![sid, project_id, seq, eid, micros, s.sat,
                        serde_json::to_string(&s.x).unwrap(), serde_json::to_string(&s.y).unwrap(),
                        serde_json::to_string(&s.z).unwrap(), serde_json::to_string(&s.clock_bias).unwrap(),
                        s.datum.0, s.health, s.line_no],
                )?;
            }
        }
        for f in &batch.freqs {
            let fid = id("frq", &[project_id.as_bytes(), f.sat.as_bytes(), &f.band.to_le_bytes(), f.code.as_bytes()]);
            tx.execute(
                "INSERT OR REPLACE INTO freqs (id, project_id, batch_seq, sat, band, code, frequency_hz, raw_line)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![fid, project_id, seq, f.sat, f.band, f.code,
                    serde_json::to_string(&f.frequency_hz).unwrap(), f.line_no],
            )?;
        }
        for d in &batch.diagnostics {
            let did = id("dia", &[batch_id.as_bytes(), &d.line_no.to_le_bytes(), d.code.as_bytes(), d.raw_line.as_bytes()]);
            tx.execute(
                "INSERT OR REPLACE INTO diagnostics (id, project_id, batch_seq, line_no, code, message, raw_line, fatal)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![did, project_id, seq, d.line_no, d.code, d.message, d.raw_line, d.fatal],
            )?;
        }

        // 更新项目测站信息
        if !batch.marker_name.is_empty() {
            tx.execute("UPDATE projects SET marker_name=?2, marker_number=?3, receiver=?4 WHERE id=?1",
                rusqlite::params![project_id, batch.marker_name, batch.marker_number, batch.receiver])?;
        }

        tx.commit()?;
        // raw_text 单独写（可能很长）
        let mut conn = self.conn.lock().unwrap();
        let text = batches_text(batch);
        conn.execute("UPDATE batches SET raw_text=?2 WHERE id=?1", rusqlite::params![batch_id, text])?;
        Ok((batch_id, seq))
    }
}

/// 从解析结果重建稳定排序的文本（用于原始层自描述；导入时原样文本另由 API 层存档）。
pub fn batches_text(b: &RawBatch) -> String {
    serde_json::to_string(b).unwrap_or_default()
}
