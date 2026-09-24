//! 领域常量、参数指纹、频率表与有效快照装载。
//!
//! 规范化（波长、组合、弧段）只发生在派生层；原始层保持原样。

use crate::db::Db;
use crate::id::id;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;
pub const GPS_F1: f64 = 1_575_420_000.0;
pub const GPS_F2: f64 = 1_227_600_000.0;

/// 检测器/算法版本。任何检测逻辑变更都必须提升此版本。
pub const ALGO_VERSION: &str = "algo-slip-1.0.0";

/// 参数修订版本（参数修订事件里携带）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub rules_version: String,
    pub gap_seconds: f64,
    pub gf_threshold_cycle: f64,
    pub mw_threshold_wl_cycle: f64,
    pub td_sigma_m: f64,
    pub doppler_sigma_hz: f64,
    pub lli_prior_prob: f64,
    pub search_half_width: i64,
    pub score_tolerance: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            rules_version: "rules-1".to_string(),
            gap_seconds: 15.0,
            gf_threshold_cycle: 0.25,
            mw_threshold_wl_cycle: 0.5,
            td_sigma_m: 3.0,
            doppler_sigma_hz: 0.5,
            lli_prior_prob: 0.5,
            search_half_width: 3,
            score_tolerance: 0.5,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        let body = serde_json::to_string(self).unwrap_or_default();
        id("pfp", &[body.as_bytes()])
    }
}

/// 标准频率（Hz），按 (系统, band) 查找；显式 FREQ 记录优先。
pub fn standard_frequency(sat: &str, band: i64) -> Option<f64> {
    let sys = sat.chars().next().unwrap_or('G');
    match (sys, band) {
        ('G', 1) => Some(GPS_F1),
        ('G', 2) => Some(GPS_F2),
        ('R', 1) => Some(1_602_000_000.0),
        ('R', 2) => Some(1_246_000_000.0),
        ('E', 1) => Some(1_575_420_000.0),
        ('E', 5) => Some(1_176_450_000.0),
        _ => None,
    }
}

pub fn wavelength(freq_hz: f64) -> f64 {
    SPEED_OF_LIGHT / freq_hz
}

#[derive(Debug, Clone)]
pub struct EffObs {
    pub obs_id: String,
    pub epoch_micros: i64,
    pub sat: String,
    pub code: String,
    pub band: i64,
    pub pseudo_m: Option<f64>,
    pub phase_c: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub lli: Option<i64>,
    pub snr_dbhz: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct EffClock {
    pub epoch_micros: i64,
    pub kind: String,
    pub delta_s: Option<f64>,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct EffState {
    pub epoch_micros: i64,
    pub sat: String,
    pub xyz_m: Option<[f64; 3]>,
    pub clock_bias_s: Option<f64>,
    pub health: String,
}

/// 某分支在给定事件序号上的有效输入快照（最新 seq 批次 + 显式时钟事件）。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub obs: Vec<EffObs>,
    pub clocks: Vec<EffClock>,
    pub states: Vec<EffState>,
    pub freqs: HashMap<(String, i64), f64>,
}

impl Snapshot {
    pub fn load(db: &Db, project_id: &str) -> rusqlite::Result<Snapshot> {
        let mut snap = Snapshot::default();
        {
            let mut stmt = db.conn.prepare(
                "SELECT o.id, e.micros, o.sat, o.code, o.band,
                        json_extract(o.pseudo, '$.value'),
                        json_extract(o.phase, '$.value'),
                        json_extract(o.doppler, '$.value'),
                        o.lli,
                        json_extract(o.snr, '$.value')
                 FROM obs o JOIN epochs e ON e.id = o.epoch_id
                 WHERE e.project_id = ?1
                   AND e.batch_seq = (SELECT MAX(b.seq) FROM batches b WHERE b.project_id = ?1 AND b.superseded = 0)
                 ORDER BY e.micros, o.sat, o.band",
            )?;
            let rows = stmt.query_map(params![project_id], |r| {
                Ok(EffObs {
                    obs_id: r.get::<_, String>(0)?,
                    epoch_micros: r.get(1)?,
                    sat: r.get(2)?,
                    code: r.get(3)?,
                    band: r.get(4)?,
                    pseudo_m: r.get(5)?,
                    phase_c: r.get(6)?,
                    doppler_hz: r.get(7)?,
                    lli: r.get(8)?,
                    snr_dbhz: r.get(9)?,
                })
            })?;
            for row in rows { snap.obs.push(row?); }
        }
        {
            let mut stmt = db.conn.prepare(
                "SELECT e.micros, c.kind, json_extract(c.delta, '$.value'), c.note
                 FROM clock_events c JOIN epochs e ON e.id = c.epoch_id
                 WHERE c.project_id = ?1
                   AND c.batch_seq = (SELECT MAX(b.seq) FROM batches b WHERE b.project_id = ?1 AND b.superseded = 0)
                 ORDER BY e.micros")?;
            let rows = stmt.query_map(params![project_id], |r| {
                Ok(EffClock { epoch_micros: r.get(0)?, kind: r.get(1)?, delta_s: r.get(2)?, note: r.get(3)? })
            })?;
            for row in rows { snap.clocks.push(row?); }
        }
        {
            let mut stmt = db.conn.prepare(
                "SELECT e.micros, s.sat,
                        json_extract(s.x, '$.value'), json_extract(s.y, '$.value'), json_extract(s.z, '$.value'),
                        json_extract(s.clock_bias, '$.value'), s.health
                 FROM sat_states s JOIN epochs e ON e.id = s.epoch_id
                 WHERE s.project_id = ?1
                   AND s.batch_seq = (SELECT MAX(b.seq) FROM batches b WHERE b.project_id = ?1 AND b.superseded = 0)
                 ORDER BY e.micros, s.sat")?;
            let rows = stmt.query_map(params![project_id], |r| {
                Ok(EffState {
                    epoch_micros: r.get(0)?, sat: r.get(1)?,
                    xyz_m: match (r.get::<_, Option<f64>>(2)?, r.get::<_, Option<f64>>(3)?, r.get::<_, Option<f64>>(4)?) {
                        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
                        _ => None,
                    },
                    clock_bias_s: r.get(5)?,
                    health: r.get(6)?,
                })
            })?;
            for row in rows { snap.states.push(row?); }
        }
        {
            let mut stmt = db.conn.prepare(
                "SELECT sat, band, json_extract(frequency_hz, '$.value')
                 FROM freqs WHERE project_id = ?1
                 AND batch_seq = (SELECT MAX(b.seq) FROM batches b WHERE b.project_id = ?1 AND b.superseded = 0)")?;
            let rows = stmt.query_map(params![project_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<f64>>(2)?))
            })?;
            for row in rows {
                let (sat, band, f) = row?;
                if let Some(f) = f { snap.freqs.insert((sat, band), f); }
            }
        }
        Ok(snap)
    }

    pub fn frequency(&self, sat: &str, band: i64) -> Option<f64> {
        self.freqs.get(&(sat.to_string(), band)).copied()
            .or_else(|| standard_frequency(sat, band))
    }
}
