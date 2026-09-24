//! Export/import bundles.
//!
//! A bundle is a single deterministic JSON document containing every raw
//! batch (verbatim text), bands, all events (hash chain), and a sha256
//! content hash. Re-importing into an empty database reproduces identities,
//! ordering and provenance exactly — including after the local db file was
//! deleted.

use super::project::PROJECT_ID;
use super::Store;
use crate::ids;
use crate::parser;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub version: i64,
    pub project_id: String,
    pub project_name: String,
    pub head_event_id: String,
    pub batches: Vec<BatchBundle>,
    pub events: Vec<EventBundle>,
    pub content_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchBundle {
    pub id: String,
    pub seq: i64,
    pub label: String,
    pub raw_text: String,
    pub raw_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBundle {
    pub id: String,
    pub parent_id: Option<String>,
    pub branch: String,
    pub seq: i64,
    pub kind: String,
    pub body_json: String,
    pub created_at_ms: i64,
    pub batch_id: Option<String>,
    pub param_fp: Option<String>,
}

impl Store {
    pub fn export_bundle(&self) -> Bundle {
        self.with_conn(|c| {
            let project_name: String = c
                .query_row(
                    "SELECT name FROM projects WHERE id=?1",
                    params![PROJECT_ID],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            let head: String = c
                .query_row(
                    "SELECT head_event_id FROM projects WHERE id=?1",
                    params![PROJECT_ID],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            let mut batches = Vec::new();
            {
                let mut stmt = c
                    .prepare("SELECT id,seq,label,raw_text,raw_sha FROM batches WHERE project_id=?1 ORDER BY seq")
                    .unwrap();
                let rows = stmt
                    .query_map(params![PROJECT_ID], |r| {
                        Ok(BatchBundle {
                            id: r.get(0)?,
                            seq: r.get(1)?,
                            label: r.get(2)?,
                            raw_text: r.get(3)?,
                            raw_sha: r.get(4)?,
                        })
                    })
                    .unwrap();
                batches = rows.flatten().collect();
            }
            let mut events = Vec::new();
            {
                let mut stmt = c
                    .prepare(
                        "SELECT id,parent_id,branch,seq,kind,body_json,created_at_ms,batch_id,param_fp
                         FROM events WHERE project_id=?1 ORDER BY seq",
                    )
                    .unwrap();
                let rows = stmt
                    .query_map(params![PROJECT_ID], |r| {
                        Ok(EventBundle {
                            id: r.get(0)?,
                            parent_id: r.get(1)?,
                            branch: r.get(2)?,
                            seq: r.get(3)?,
                            kind: r.get(4)?,
                            body_json: r.get(5)?,
                            created_at_ms: r.get(6)?,
                            batch_id: r.get(7)?,
                            param_fp: r.get(8)?,
                        })
                    })
                    .unwrap();
                events = rows.flatten().collect();
            }
            let mut b = Bundle {
                format: "carrier-breakpoint-bundle".to_string(),
                version: 1,
                project_id: PROJECT_ID.to_string(),
                project_name,
                head_event_id: head,
                batches,
                events,
                content_sha: String::new(),
            };
            let material = bundle_material(&b);
            b.content_sha = ids::sha256_hex(material.as_bytes());
            b
        })
    }

    /// Import into an *empty* project. Raw batches are re-parsed and re-stored
    /// deterministically; events are inserted verbatim (ids preserved), then
    /// the tip view is recomputed.
    pub fn import_bundle(&self, bundle: &Bundle) -> Result<usize, String> {
        let stored_sha = bundle.content_sha.clone();
        let mut b2 = bundle.clone();
        b2.content_sha = String::new();
        let recomputed = ids::sha256_hex(bundle_material(&b2).as_bytes());
        if recomputed != stored_sha {
            return Err(format!(
                "bundle integrity mismatch: expected {stored_sha}, computed {recomputed}"
            ));
        }
        if bundle.project_id != PROJECT_ID {
            return Err("bundle project id does not match local project".to_string());
        }
        let n = self.with_conn_mut(|c| {
            let existing: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM batches WHERE project_id=?1",
                    params![PROJECT_ID],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if existing != 0 {
                return Err("import target database is not empty".to_string());
            }
            let tx = c.transaction().map_err(|e| e.to_string())?;
            for bb in &bundle.batches {
                let parsed = parser::parse(&bb.raw_text, &bb.id);
                if parsed.raw_sha != bb.raw_sha {
                    return Err(format!(
                        "raw sha mismatch for batch seq {}: stored {}, parsed {}",
                        bb.seq, bb.raw_sha, parsed.raw_sha
                    ));
                }
                insert_bundle_batch(&tx, bb, &parsed)?;
            }
            for e in &bundle.events {
                tx.execute(
                    "INSERT INTO events(id,project_id,parent_id,branch,seq,kind,body_json,created_at_ms,batch_id,param_fp)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        e.id, PROJECT_ID, e.parent_id, e.branch, e.seq, e.kind, e.body_json,
                        e.created_at_ms, e.batch_id, e.param_fp
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.execute(
                "UPDATE projects SET head_event_id=?1, name=?2 WHERE id=?3",
                params![bundle.head_event_id, bundle.project_name, PROJECT_ID],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok::<usize, String>(bundle.events.len())
        })?;
        if !bundle.head_event_id.is_empty() {
            self.recompute_at(&bundle.head_event_id);
            self.recompute_tip();
        }
        Ok(n)
    }
}

fn insert_bundle_batch(
    tx: &rusqlite::Transaction<'_>,
    bb: &BatchBundle,
    parsed: &crate::model::ParsedImport,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO batches(id,project_id,seq,kind,label,raw_sha,parent_batch_id,imported_at_ms,raw_text)
         VALUES(?1,?2,?3,'cbf-v1',?4,?5,NULL,0,?6)",
        params![bb.id, PROJECT_ID, bb.seq, bb.label, bb.raw_sha, bb.raw_text],
    )
    .map_err(|e| e.to_string())?;
    for b in &parsed.bands {
        tx.execute(
            "INSERT INTO bands(project_id,batch_id,band,phase_code,pseudo_code,doppler_code,\
             snr_code,frequency_raw,frequency_hz,wavelength_raw,wavelength_m,unit,datum)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                PROJECT_ID, bb.id, b.band, b.phase_code, b.pseudo_code, b.doppler_code, b.snr_code,
                b.frequency_raw, b.frequency_hz, b.wavelength_raw, b.wavelength_m, b.unit, b.datum
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for o in &parsed.observations {
        tx.execute(
            "INSERT INTO raw_obs(natural_id,project_id,batch_id,line_no,epoch_raw,epoch_secs,\
             epoch_datum,sat,code,kind,band,raw_value,raw_unit,value,diagnostic)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                o.natural_id, PROJECT_ID, o.batch_id, o.line_no, o.epoch_raw, o.epoch_secs,
                o.epoch_datum, o.sat, o.code, format!("{:?}", o.kind), o.band, o.raw_value,
                o.raw_unit, o.value, o.diagnostic
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for k in &parsed.clock_events {
        tx.execute(
            "INSERT INTO raw_clock(natural_id,project_id,batch_id,line_no,epoch_raw,epoch_secs,\
             epoch_datum,raw_delta,raw_unit,delta_ns,diagnostic)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                k.natural_id, PROJECT_ID, k.batch_id, k.line_no, k.epoch_raw, k.epoch_secs,
                k.epoch_datum, k.raw_delta, k.raw_unit, k.delta_ns, k.diagnostic
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for s in &parsed.sat_states {
        tx.execute(
            "INSERT INTO raw_sat_state(natural_id,project_id,batch_id,line_no,epoch_raw,epoch_secs,\
             epoch_datum,sat,az_raw,el_raw,health_raw,datum,az_deg,el_deg,diagnostic)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                s.natural_id, PROJECT_ID, s.batch_id, s.line_no, s.epoch_raw, s.epoch_secs,
                s.epoch_datum, s.sat, s.az_raw, s.el_raw, s.health_raw, s.datum, s.az_deg,
                s.el_deg, s.diagnostic
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    for d in &parsed.diagnostics {
        tx.execute(
            "INSERT INTO import_diagnostics(batch_id,line_no,code,message,severity)
             VALUES(?1,?2,?3,?4,?5)",
            params![bb.id, d.line_no, d.code, d.message, d.severity],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn bundle_material(b: &Bundle) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        b.version,
        b.project_id,
        b.head_event_id,
        b.batches
            .iter()
            .map(|x| format!("{}:{}:{}", x.seq, x.id, x.raw_sha))
            .collect::<Vec<_>>()
            .join(","),
        b.events
            .iter()
            .map(|x| format!("{}:{}:{}", x.seq, x.id, x.parent_id.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(",")
    )
}
