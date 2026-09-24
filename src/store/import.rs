use super::project::PROJECT_ID;
use super::{EventBody, EventRecord, Store};
use crate::engine::DecisionAction;
use crate::ids;
use crate::model::{ObsKind, ParsedImport};
use crate::params::DetectorParams;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReceipt {
    pub batch_id: String,
    pub seq: i64,
    pub event: EventRecord,
    pub diagnostics: usize,
    pub observations: usize,
}

impl Store {
    /// Parse raw CBF text and commit it as one immutable batch. Natural ids
    /// embed the deterministic batch id.
    pub fn commit_import_raw(&self, label: &str, raw_text: &str) -> ImportReceipt {
        self.with_conn_mut(|c| {
            let tx = c.transaction().unwrap();
            let seq: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(seq),0)+1 FROM batches WHERE project_id=?1",
                    params![PROJECT_ID],
                    |r| r.get(0),
                )
                .unwrap();
            let raw_sha = ids::sha256_hex(raw_text.as_bytes());
            let batch_id = ids::batch_id(PROJECT_ID, seq, &raw_sha);
            let parsed = crate::parser::parse(raw_text, &batch_id);
            let parent_batch: Option<String> = tx
                .query_row(
                    "SELECT id FROM batches WHERE project_id=?1 ORDER BY seq DESC LIMIT 1",
                    params![PROJECT_ID],
                    |r| r.get(0),
                )
                .ok();
            let now = self.now_ms();
            tx.execute(
                "INSERT INTO batches(id,project_id,seq,kind,label,raw_sha,parent_batch_id,imported_at_ms,raw_text)
                 VALUES(?1,?2,?3,'cbf-v1',?4,?5,?6,?7,?8)",
                params![
                    batch_id, PROJECT_ID, seq, label, raw_sha, parent_batch, now, raw_text
                ],
            )
            .unwrap();
            for b in &parsed.bands {
                tx.execute(
                    "INSERT INTO bands(project_id,batch_id,band,phase_code,pseudo_code,doppler_code,\
                     snr_code,frequency_raw,frequency_hz,wavelength_raw,wavelength_m,unit,datum)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                    params![
                        PROJECT_ID, batch_id, b.band, b.phase_code, b.pseudo_code, b.doppler_code,
                        b.snr_code, b.frequency_raw, b.frequency_hz, b.wavelength_raw,
                        b.wavelength_m, b.unit, b.datum
                    ],
                )
                .unwrap();
            }
            for o in &parsed.observations {
                tx.execute(
                    "INSERT INTO raw_obs(natural_id,project_id,batch_id,line_no,epoch_raw,epoch_secs,\
                     epoch_datum,sat,code,kind,band,raw_value,raw_unit,value,diagnostic)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                    params![
                        o.natural_id, PROJECT_ID, o.batch_id, o.line_no, o.epoch_raw, o.epoch_secs,
                        o.epoch_datum, o.sat, o.code,
                        format!("{:?}", o.kind), o.band, o.raw_value, o.raw_unit, o.value,
                        o.diagnostic
                    ],
                )
                .unwrap();
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
                .unwrap();
            }
            for st in &parsed.sat_states {
                tx.execute(
                    "INSERT INTO raw_sat_state(natural_id,project_id,batch_id,line_no,epoch_raw,epoch_secs,\
                     epoch_datum,sat,az_raw,el_raw,health_raw,datum,az_deg,el_deg,diagnostic)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                    params![
                        st.natural_id, PROJECT_ID, st.batch_id, st.line_no, st.epoch_raw, st.epoch_secs,
                        st.epoch_datum, st.sat, st.az_raw, st.el_raw, st.health_raw, st.datum,
                        st.az_deg, st.el_deg, st.diagnostic
                    ],
                )
                .unwrap();
            }
            for d in &parsed.diagnostics {
                tx.execute(
                    "INSERT INTO import_diagnostics(batch_id,line_no,code,message,severity)
                     VALUES(?1,?2,?3,?4,?5)",
                    params![batch_id, d.line_no, d.code, d.message, d.severity],
                )
                .unwrap();
            }
            if !parsed.header.marker.is_empty() {
                tx.execute(
                    "UPDATE projects SET marker=?1, coord_datum=?2, coord_unit=?3 WHERE id=?4",
                    params![
                        parsed.header.marker, parsed.header.coord_datum,
                        parsed.header.coord_unit, PROJECT_ID
                    ],
                )
                .unwrap();
            }
            let body = EventBody::Import {
                batch_id: batch_id.clone(),
                seq,
                label: label.to_string(),
                raw_sha: raw_sha.clone(),
                observations: parsed.observations.len() as i64,
                diagnostics: parsed.diagnostics.len() as i64,
            };
            let event = super::events::append_event_locked(
                &tx, "main", "IMPORT", &body, Some(batch_id.clone()), None, self.now_ms(),
            );
            tx.execute(
                "UPDATE projects SET head_event_id=?1, created_event_id=COALESCE(NULLIF(created_event_id,''),?1) WHERE id=?2",
                params![event.id, PROJECT_ID],
            )
            .unwrap();
            tx.commit().unwrap();
            ImportReceipt {
                batch_id, seq, event,
                diagnostics: parsed.diagnostics.len(),
                observations: parsed.observations.len(),
            }
        })
    }

    /// A parameter revision is a new version with a parent link; old derived
    /// rows keep their fingerprint and remain replayable.
    pub fn commit_params(&self, params: &DetectorParams, reason: &str) -> EventRecord {
        let fp = params.fingerprint();
        let body = EventBody::Params {
            params: params.clone(),
            fingerprint: fp.clone(),
            reason: reason.to_string(),
        };
        self.append_event("PARAMS", &body, None, Some(fp))
    }

    pub fn commit_decision(
        &self,
        action: DecisionAction,
        candidate_id: Option<&str>,
        sat: Option<&str>,
        band: Option<&str>,
        at_epoch: Option<&str>,
        reason: &str,
        branch: &str,
        parent_override: Option<&str>,
    ) -> EventRecord {
        let body = EventBody::Decision {
            action: format!("{action:?}"),
            candidate_id: candidate_id.map(|s| s.to_string()),
            sat: sat.map(|s| s.to_string()),
            band: band.map(|s| s.to_string()),
            at_epoch: at_epoch.map(|s| s.to_string()),
            reason: reason.to_string(),
        };
        self.append_event_on_branch(branch, "DECISION", &body, None, None, parent_override)
    }
}
