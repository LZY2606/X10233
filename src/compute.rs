//! Multi-frequency combinations and the three independent detectors:
//! ionosphere / geometry-free (GF), Melbourne-Wubbena (MW) and time
//! difference with Doppler (TD). None is privileged as truth; ties and
//! rejects are first-class.

use crate::engine::{ApiError, ApiResult, Engine, RecomputeReport};
use crate::hash::id;
use crate::model::*;
use rusqlite::params;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
struct EffObs {
    gps_ns: i64,
    sat: String,
    code: String,
    value: Option<f64>,
    lli: i64,
    ss: String,
    input_id: String,
}

#[derive(Clone, Debug, Default)]
struct SatCodeAt {
    value: Option<f64>,
    lli: i64,
    ss: String,
    input_id: String,
}

#[derive(Clone, Debug)]
struct Freqs {
    f1_hz: f64,
    f2_hz: f64,
    l1: f64,
    l2: f64,
}

impl Freqs {
    fn fallback() -> Self {
        let f1 = 1_575_420_000.0;
        let f2 = 1_227_600_000.0;
        Freqs { f1_hz: f1, f2_hz: f2, l1: C_LIGHT / f1, l2: C_LIGHT / f2 }
    }
}

const C_LIGHT: f64 = 299_792_458.0;

impl<'a> Engine<'a> {
    fn current_params(&self, pid: &str) -> ApiResult<(Params, String, Option<String>)> {
        // latest PARAM_REVISED event along current head lineage, else default
        let head: String = self.conn.query_row(
            "SELECT head_event_id FROM projects WHERE id=?1", params![pid],
            |r| r.get(0)).map_err(|_| ApiError::not_found("project"))?;
        let mut cur = Some(head);
        while let Some(e) = cur {
            let row: Option<(String, Option<String>, String)> = self.conn.query_row(
                "SELECT kind,parent_id,payload_json FROM events WHERE id=?1",
                params![e], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok();
            if let Some((kind, parent, payload)) = row {
                if kind == "PARAM_REVISED" {
                    let p: Params = serde_json::from_str(&payload)
                        .map_err(|e| ApiError::bad(format!("params: {e}")))?;
                    let fp = crate::engine::hex_hash(payload.as_bytes());
                    return Ok((p, fp, Some(e)));
                }
                cur = parent;
            } else {
                break;
            }
        }
        let p = Params::default();
        let pj = json!(p).to_string();
        let fp = crate::engine::hex_hash(pj.as_bytes());
        Ok((p, fp, None))
    }

    fn effective_obs(&self, pid: &str) -> Vec<EffObs> {
        // latest input per (gps_ns,sat,code) wins; missing value stays missing
        let mut sql = self.conn.prepare(
            "SELECT r.gps_ns,r.sat,r.code,r.value,r.lli,r.signal_strength,r.input_id,i.seq
             FROM raw_obs r JOIN input_versions i ON i.id=r.input_id
             WHERE r.project_id=?1 ORDER BY r.gps_ns,r.sat,r.code,i.seq"
        ).unwrap();
        let rows = sql.query_map(params![pid], |r| Ok((
            r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
            r.get::<_, Option<f64>>(3)?, r.get::<_, i64>(4)?,
            r.get::<_, String>(5)?, r.get::<_, String>(6)?,
            r.get::<_, i64>(7)?,
        ))).unwrap();
        let mut latest: BTreeMap<(i64, String, String), SatCodeAt> = BTreeMap::new();
        let mut order: Vec<(i64, String, String)> = Vec::new();
        for r in rows.flatten() {
            let key = (r.0, r.1.clone(), r.2.clone());
            if !latest.contains_key(&key) {
                order.push(key.clone());
            }
            latest.insert(key.clone(), SatCodeAt {
                value: r.3, lli: r.4, ss: r.5, input_id: r.6,
            });
        }
        order.into_iter().map(|k| {
            let v = &latest[&k];
            EffObs {
                gps_ns: k.0, sat: k.1.clone(), code: k.2.clone(),
                value: v.value, lli: v.lli, ss: v.ss.clone(),
                input_id: v.input_id.clone(),
            }
        }).collect()
    }

    fn clock_jump_ns(&self, pid: &str) -> BTreeSet<i64> {
        let mut sql = self.conn.prepare(
            "SELECT at_second FROM clock_events WHERE project_id=?1 AND scope='RECEIVER'"
        ).unwrap();
        let secs: Vec<f64> = sql.query_map(params![pid], |r| r.get(0))
            .unwrap().flatten().collect();
        let epochs: Vec<i64> = self.conn.prepare(
            "SELECT DISTINCT gps_ns FROM raw_obs WHERE project_id=?1 ORDER BY gps_ns")
            .unwrap().query_map(params![pid], |r| r.get::<_, i64>(0))
            .unwrap().flatten().collect();
        let first = *epochs.first().unwrap_or(&0);
        let mut out = BTreeSet::new();
        for s in secs {
            let target = super_ns_base(first, s);
            if let Some(nearest) = epochs.iter().cloned()
                .min_by_key(|ns| (*ns - target).abs())
            {
                if (nearest - target).abs() <= 1_000_000_000 {
                    out.insert(nearest);
                }
            }
        }
        out
    }

    fn sat_freqs(&self, pid: &str) -> BTreeMap<String, Freqs> {
        let mut sql = self.conn.prepare(
            "SELECT sat,band,freq_hz,wavelength_m FROM frequencies WHERE project_id=?1"
        ).unwrap();
        let rows = sql.query_map(params![pid], |r| Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?,
            r.get::<_, f64>(2)?, r.get::<_, f64>(3)?,
        ))).unwrap();
        let mut map: BTreeMap<String, Freqs> = BTreeMap::new();
        for (sat, band, hz, wl) in rows.flatten() {
            let f = map.entry(sat).or_insert_with(Freqs::fallback);
            if band == "L1" { f.f1_hz = hz; f.l1 = wl; }
            if band == "L2" { f.f2_hz = hz; f.l2 = wl; }
        }
        map
    }

    pub fn recompute(&mut self, pid: &str) -> ApiResult<RecomputeReport> {
        let (params, params_fp, _pevent) = self.current_params(pid)?;
        let event_id: String = self.conn.query_row(
            "SELECT head_event_id FROM projects WHERE id=?1", params![pid],
            |r| r.get(0)).map_err(|_| ApiError::not_found("project"))?;
        let obs = self.effective_obs(pid);
        let clock_ns = self.clock_jump_ns(pid);
        let freqs_map = self.sat_freqs(pid);

        let mut at: BTreeMap<(i64, String, String), &EffObs> = BTreeMap::new();
        let mut sats: BTreeSet<String> = BTreeSet::new();
        let mut sat_epochs: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
        for o in &obs {
            at.insert((o.gps_ns, o.sat.clone(), o.code.clone()), o);
            sats.insert(o.sat.clone());
            sat_epochs.entry(o.sat.clone()).or_default().insert(o.gps_ns);
        }

        let mut all_epochs: BTreeSet<i64> = BTreeSet::new();
        for e in sat_epochs.values() { all_epochs.extend(e.iter().copied()); }
        let all_epochs: Vec<i64> = all_epochs.into_iter().collect();
        let median_dt = median_gap_ns(&all_epochs).max(1);

        let mut report = RecomputeReport::default();
        for tbl in ["pair_points", "candidates", "arcs"] {
            self.conn.execute(
                &format!("UPDATE {tbl} SET fresh='STALE' WHERE project_id=?1"),
                params![pid]).map_err(crate::engine::db_err_pub)?;
        }

        let mut changed_sats: BTreeSet<String> = BTreeSet::new();
        for sat in &sats {
            let fr = freqs_map.get(sat).cloned().unwrap_or_else(Freqs::fallback);
            let epochs: Vec<i64> = sat_epochs[sat].iter().copied().collect();
            self.process_satellite(pid, &event_id, &params, &params_fp, sat,
                &epochs, &at, &clock_ns, &fr, median_dt, &mut report,
                &mut changed_sats)?;
        }
        report.stale_sats = changed_sats.into_iter().collect();
        Ok(report)
    }
}

fn median_gap_ns(epochs: &[i64]) -> i64 {
    if epochs.len() < 2 { return 1_000_000_000; }
    let mut gaps: Vec<i64> = epochs.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort();
    gaps[gaps.len() / 2]
}

fn super_ns_base(first_ns: i64, at_second: f64) -> i64 {
    let day_ns = (first_ns / 86_400_000_000_000) * 86_400_000_000_000;
    day_ns + (at_second * 1e9) as i64
}
