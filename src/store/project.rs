use super::schema;
use crate::engine::{PhasePoint, SourceView};
use crate::model::{BandDef, ObsKind};
use crate::params::DetectorParams;
use crate::vtime::VirtualClock;
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
    clock: Mutex<Box<dyn VirtualClock>>,
    pub path: String,
}

/// Fixed project id used by the single-project local database. Export/import
/// preserves it verbatim.
pub const PROJECT_ID: &str = "project_local";

impl Store {
    pub fn open(path: &str, clock: Box<dyn VirtualClock>) -> rusqlite::Result<Self> {
        let mut conn = Connection::open(path)?;
        schema::init(&mut conn)?;
        let exists: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects WHERE id=?1", params![PROJECT_ID], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        if exists == 0 {
            conn.execute(
                "INSERT INTO projects(id,name,marker,coord_datum,coord_unit) VALUES(?1,'Carrier breakpoint review','','','')",
                params![PROJECT_ID],
            )?;
        }
        Ok(Store {
            conn: Mutex::new(conn),
            clock: Mutex::new(clock),
            path: path.to_string(),
        })
    }

    /// In-memory database (tests).
    pub fn open_memory(clock: Box<dyn VirtualClock>) -> rusqlite::Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        schema::init(&mut conn)?;
        conn.execute(
            "INSERT INTO projects(id,name) VALUES(?1,'test project')",
            params![PROJECT_ID],
        )?;
        Ok(Store {
            conn: Mutex::new(conn),
            clock: Mutex::new(clock),
            path: ":memory:".to_string(),
        })
    }

    pub fn tick(&self, ms: i64) {
        self.clock.lock().unwrap().advance_ms(ms);
    }
    pub fn now_ms(&self) -> i64 {
        self.clock.lock().unwrap().now_ms()
    }
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let c = self.clock.lock().unwrap();
        drop(c);
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }
    pub fn with_conn_mut<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let mut conn = self.conn.lock().unwrap();
        f(&mut conn)
    }

    pub fn head(&self) -> String {
        self.with_conn(|c| {
            c.query_row(
                "SELECT head_event_id FROM projects WHERE id=?1",
                params![PROJECT_ID],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default()
        })
    }

    pub fn set_head(&self, event_id: &str) {
        self.with_conn_mut(|c| {
            c.execute(
                "UPDATE projects SET head_event_id=?1 WHERE id=?2",
                params![event_id, PROJECT_ID],
            )
            .unwrap();
        });
    }

    pub fn current_params(&self) -> DetectorParams {
        self.with_conn(|c| {
            let body: Option<String> = c
                .query_row(
                    "SELECT body_json FROM events WHERE project_id=?1 AND kind='PARAMS'
                     ORDER BY seq DESC LIMIT 1",
                    params![PROJECT_ID],
                    |r| r.get(0),
                )
                .ok()
                .flatten();
            match body {
                Some(b) => serde_json::from_str(&b).unwrap_or_default(),
                None => DetectorParams::default(),
            }
        })
    }

    /// Build the merged source view at the tip (winning batch per natural key).
    pub fn source_view(&self) -> crate::engine::SourceView {
        self.source_view_impl(None)
    }

    /// Build the merged source view restricted to a specific ancestor set of
    /// batch ids (used when checking out / forking old events). `None` means
    /// every batch reachable from the project (all imports on the local db).
    pub fn source_view_for_batches(
        &self,
        allowed: &std::collections::BTreeSet<String>,
    ) -> crate::engine::SourceView {
        self.source_view_impl(Some(allowed))
    }

    fn source_view_impl(
        &self,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> crate::engine::SourceView {
        let keep = |b: &str| allowed.map(|a| a.contains(b)).unwrap_or(true);
        self.with_conn(|c| {
            let mut sv = SourceView {
                project: PROJECT_ID.to_string(),
                ..Default::default()
            };
            sv.marker = c
                .query_row(
                    "SELECT marker FROM projects WHERE id=?1",
                    params![PROJECT_ID],
                    |r| r.get::<_, String>(0),
                )
                .unwrap_or_default();
            // bands: union across batches, latest declaration per band wins.
            let mut bands: std::collections::BTreeMap<String, BandDef> =
                std::collections::BTreeMap::new();
            {
                let mut stmt = c
                    .prepare(
                        "SELECT band,phase_code,pseudo_code,doppler_code,snr_code,\
                         frequency_raw,frequency_hz,wavelength_raw,wavelength_m,unit,datum,batch_id \
                         FROM bands WHERE project_id=?1 ORDER BY rowid",
                    )
                    .unwrap();
                let rows = stmt
                    .query_map(params![PROJECT_ID], |r| {
                        Ok((r.get::<_, String>(11)?, r.get::<_, String>(0)?, BandDef {
                                band: r.get(0)?,
                                phase_code: r.get(1)?,
                                pseudo_code: r.get(2)?,
                                doppler_code: r.get(3)?,
                                snr_code: r.get(4)?,
                                frequency_raw: r.get(5)?,
                                frequency_hz: r.get(6)?,
                                wavelength_raw: r.get(7)?,
                                wavelength_m: r.get(8)?,
                                unit: r.get(9)?,
                                datum: r.get(10)?,
                            }))
                    })
                    .unwrap();
                for row in rows.flatten() {
                    let (batch, band, def) = row;
                    if !keep(&batch) { continue; }
                    bands.insert(band, def);
                }
            }
            sv.bands = bands.into_values().collect();
            // winning obs per (epoch,sat,code): highest seq batch wins.
            let winners = winning_naturals(c, "raw_obs", allowed);
            let mut stmt = c
                .prepare(
                    "SELECT natural_id,batch_id,epoch_raw,epoch_secs,sat,code,kind,band,\
                     raw_value,raw_unit,value FROM raw_obs WHERE project_id=?1",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![PROJECT_ID], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<f64>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, String>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, Option<f64>>(10)?,
                    ))
                })
                .unwrap();
            use std::collections::BTreeMap;
            // First pass collect phase points; each obs row updates a point.
            let mut points: BTreeMap<(String, String, String), PhasePoint> = BTreeMap::new();
            for r in rows.flatten() {
                let (natural, batch, epoch_raw, t, sat, code, kind_s, band, raw, unit, value) = r;
                let win = winners.get(&natural).map(|b| b == &batch).unwrap_or(false);
                if !win {
                    continue;
                }
                let key = (epoch_raw.clone(), sat.clone(), band.clone());
                let merged =
                    crate::ids::merge_obs_id(PROJECT_ID, &epoch_raw, &sat, &code);
                let p = points.entry(key.clone()).or_insert_with(|| PhasePoint {
                    merged_id: String::new(),
                    natural_id: natural.clone(),
                    batch_id: batch.clone(),
                    epoch_raw: epoch_raw.clone(),
                    t: t.unwrap_or(0.0),
                    code: code.clone(),
                    band: band.clone(),
                    sat: sat.clone(),
                    cycles: None,
                    range_m: None,
                    doppler_hz: None,
                    snr_dbhz: None,
                    lli_bits: None,
                    raw_phase: String::new(),
                    raw_range: String::new(),
                    raw_unit_phase: String::new(),
                    raw_unit_range: String::new(),
                    phase_id: None,
                    pseudo_id: None,
                    doppler_id: None,
                    snr_id: None,
                    lli_id: None,
                });
                let merged = crate::ids::merge_obs_id(PROJECT_ID, &epoch_raw, &sat, &code);
                p.merged_id = merged.clone();
                match kind_s.as_str() {
                    "Pseudorange" => {
                        p.range_m = value;
                        p.raw_range = raw;
                        p.raw_unit_range = unit;
                        p.pseudo_id = Some(merged);
                    }
                    "Phase" => {
                        p.cycles = value;
                        p.raw_phase = raw;
                        p.raw_unit_phase = unit;
                        p.phase_id = Some(merged);
                    }
                    "Doppler" => {
                        p.doppler_hz = value;
                        p.doppler_id = Some(merged);
                    }
                    "Snr" => {
                        p.snr_dbhz = value;
                        p.snr_id = Some(merged);
                    }
                    "Lli" => {
                        p.lli_bits = value.map(|v| v as i64);
                        p.lli_id = Some(merged);
                    }
                    _ => {}
                }
                let _ = ObsKind::from_code(&code);
            }
            sv.points = points.into_values().collect();
            sv.points.sort_by(|a, b| {
                a.t.partial_cmp(&b.t)
                    .unwrap()
                    .then(a.sat.cmp(&b.sat))
                    .then(a.band.cmp(&b.band))
            });

            let win_clk = winning_naturals(c, "raw_clock", allowed);
            let mut stmt = c
                .prepare(
                    "SELECT natural_id,batch_id,epoch_raw,epoch_secs,raw_delta,raw_unit,delta_ns \
                     FROM raw_clock WHERE project_id=?1",
                )
                .unwrap();
            let rows = stmt.query_map(params![PROJECT_ID], |r| {
                Ok(crate::engine::ClockPoint {
                    merged_id: String::new(),
                    natural_id: r.get(0)?,
                    batch_id: r.get::<_, String>(1)?,
                    epoch_raw: r.get(2)?,
                    t: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    raw_delta: r.get(4)?,
                    raw_unit: r.get(5)?,
                    delta_ns: r.get(6)?,
                })
            });
            let mut seen = std::collections::BTreeSet::new();
            for mut cp in rows.unwrap().flatten() {
                if !win_clk.get(&cp.natural_id).map(|b| b == &cp.batch_id).unwrap_or(false) {
                    continue;
                }
                cp.merged_id = crate::ids::merge_clock_id(PROJECT_ID, &cp.epoch_raw);
                if seen.insert(cp.merged_id.clone()) {
                    sv.clocks.push(cp);
                }
            }
            sv.clocks.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
            sv
        })
    }
}

/// Map natural_id -> winning batch id (latest batch seq) for a raw table.
fn winning_naturals(
    c: &Connection,
    table: &str,
    allowed: Option<&std::collections::BTreeSet<String>>,
) -> std::collections::BTreeMap<String, String> {
    let sql = format!(
        "SELECT r.natural_id, r.batch_id FROM {table} r
         JOIN batches b ON b.id = r.batch_id
         WHERE r.project_id=?1
         ORDER BY b.seq ASC"
    );
    let mut out = std::collections::BTreeMap::new();
    let mut stmt = c.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(params![super::project::PROJECT_ID], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap();
    for (n, b) in rows.flatten() {
        if allowed.map(|a| a.contains(&b)).unwrap_or(true) {
            out.insert(n, b); // later seq overwrites
        }
    }
    out
}
