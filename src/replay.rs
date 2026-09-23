//! Reconstruct the input material visible at a point in history.
//!
//! Raw submissions are immutable. An `OBS_PATCH` revises whole epoch records;
//! replaying up to a batch/rule boundary returns the then-current epochs,
//! which is how time-travel review and forking recover past conclusions.

use crate::model::{ClockEvent, DetectorParams, RinexFile, SatelliteState};
use rusqlite::Connection;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct RawSlot {
    pub epoch_seq: i64,
    pub epoch: String,
    pub sat: String,
    pub code: String,
    pub obs_id: String,
    pub batch_id: String,
    pub raw_value: String,
    pub raw_field: String,
    pub lli: Option<i64>,
    pub ssi_raw: Option<String>,
    pub kind: String,
    pub band: String,
    pub freq_hz: i64,
    pub diagnostic_code: Option<String>,
    pub diagnostic_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EpochInput {
    pub seq: i64,
    pub epoch: String,
    pub recv_clock_offset_s: String,
    pub batch_id: String,
    pub slots: Vec<RawSlot>, // per code
}

#[derive(Debug, Clone)]
pub struct ReplayInput {
    pub project_id: String,
    pub batches: Vec<BatchMeta>,
    pub epochs: Vec<EpochInput>,
    pub clock_events: Vec<(String, ClockEvent)>, // (raw id, event)
    pub sat_states: Vec<(String, SatelliteState)>,
    pub rules: Vec<RuleMeta>,
    pub decisions: Vec<DecisionAtTime>,
    pub events: Vec<(i64, String, Value)>,
    pub head_event_seq: i64,
}

#[derive(Debug, Clone)]
pub struct BatchMeta {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub kind: String,
    pub created_at_ms: i64,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct RuleMeta {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub fingerprint: String,
    pub params: DetectorParams,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct DecisionAtTime {
    pub event_seq: i64,
    pub candidate_natural: String,
    pub action: String,
    pub proposal: Option<Value>,
    pub reason: String,
    pub rule_id_at_time: String,
    pub undone_event_seq: Option<i64>,
}

pub fn load_replay_input(conn: &Connection, project_id: &str, head_event_seq: Option<i64>) -> anyhow_lite::Result<ReplayInput> {
    let head = match head_event_seq {
        Some(h) => h,
        None => conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE project_id = ?1",
            [project_id],
            |r| r.get::<_, i64>(0),
        )?,
    };

    // A batch is visible at head only if the IMPORT event that created it is
    // at or before head.
    let mut batches = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT b.id, b.seq, b.parent_id, b.kind, b.created_at_ms, b.note
             FROM batches b
             JOIN events e ON e.project_id = b.project_id
               AND e.kind = 'IMPORT'
               AND json_extract(e.payload_json, '$.batch_id') = b.id
             WHERE b.project_id = ?1 AND e.seq <= ?2
             ORDER BY b.seq",
        )?;
        let rows = stmt.query_map([project_id, head], |r| {
            Ok(BatchMeta {
                id: r.get(0)?,
                seq: r.get(1)?,
                parent_id: r.get(2)?,
                kind: r.get(3)?,
                created_at_ms: r.get(4)?,
                note: r.get(5)?,
            })
        })?;
        for r in rows {
            batches.push(r?);
        }
    }

    // Replay epoch records, later batches override whole epoch seq records.
    let mut epoch_order: Vec<i64> = Vec::new();
    let mut epochs_by_seq: std::collections::BTreeMap<i64, EpochInput> = std::collections::BTreeMap::new();
    for b in &batches {
        if b.kind != "OBS_FULL" && b.kind != "OBS_PATCH" {
            continue;
        }
        let mut estmt = conn.prepare(
            "SELECT seq, epoch, recv_clock_offset_s FROM raw_epochs
             WHERE project_id = ?1 AND batch_id = ?2 ORDER BY seq",
        )?;
        let erows: Vec<(i64, String, String)> = estmt
            .query_map([project_id, b.id.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut ostat = conn.prepare(
            "SELECT id, epoch_seq, epoch, sat, code, raw_value, raw_field, lli, ssi_raw, kind, band, freq_hz,
                    diagnostic_code, diagnostic_message
             FROM raw_obs WHERE project_id = ?1 AND batch_id = ?2 ORDER BY epoch_seq, code",
        )?;
        for (seq, epoch, clock) in erows {
            let mut slots = Vec::new();
            {
                let rows = ostat.query_map([project_id, b.id.as_str()], |r| {
                    Ok(RawSlot {
                        epoch_seq: r.get(1)?,
                        epoch: r.get(2)?,
                        sat: r.get(3)?,
                        code: r.get(4)?,
                        obs_id: r.get(0)?,
                        batch_id: b.id.clone(),
                        raw_value: r.get(5)?,
                        raw_field: r.get(6)?,
                        lli: r.get(7)?,
                        ssi_raw: r.get(8)?,
                        kind: r.get(9)?,
                        band: r.get(10)?,
                        freq_hz: r.get(11)?,
                        diagnostic_code: r.get(12)?,
                        diagnostic_message: r.get(13)?,
                    })
                })?;
                for r in rows {
                    let slot = r?;
                    if slot.epoch_seq == seq {
                        slots.push(slot);
                    }
                }
            }
            if !epochs_by_seq.contains_key(&seq) {
                epoch_order.push(seq);
            }
            epochs_by_seq.insert(
                seq,
                EpochInput { seq, epoch, recv_clock_offset_s: clock, batch_id: b.id.clone(), slots },
            );
        }
    }
    let epochs: Vec<EpochInput> = epochs_by_seq.into_values().collect();

    let mut clock_events = Vec::new();
    {
        let visible = visible_batch_ids(&batches);
        let mut stmt = conn.prepare(
            "SELECT id, epoch, event, jump_s, source, detail_json FROM raw_clock_events
             WHERE project_id = ?1 AND batch_id IN (SELECT value FROM json_each(?2))",
        )?;
        let jarr = serde_json::to_string(&visible).unwrap_or_else(|_| "[]".to_string());
        let rows = stmt.query_map(rusqlite::params![project_id, jarr], |r| {
            let detail: String = r.get(5)?;
            Ok((
                r.get::<_, String>(0)?,
                ClockEvent {
                    epoch: r.get(1)?,
                    event: r.get(2)?,
                    jump_s: r.get(3)?,
                    source: r.get(4)?,
                    detail: serde_json::from_str(&detail).unwrap_or(Value::Null),
                },
            ))
        })?;
        for r in rows {
            clock_events.push(r?);
        }
    }

    let mut sat_states = Vec::new();
    {
        let visible2 = visible_batch_ids(&batches);
        let jarr2 = serde_json::to_string(&visible2).unwrap_or_else(|_| "[]".to_string());
        let mut stmt = conn.prepare(
            "SELECT id, epoch, sat, pos_x_m, pos_y_m, pos_z_m, datum, health, source
             FROM raw_sat_states WHERE batch_id IN (SELECT value FROM json_each(?2))
               AND project_id = ?1 ORDER BY epoch, sat",
        )?;
        let rows = stmt.query_map(rusqlite::params![jarr2, project_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                SatelliteState {
                    epoch: r.get(1)?,
                    sat: r.get(2)?,
                    position_xyz_m: [r.get(3)?, r.get(4)?, r.get(5)?],
                    datum: r.get(6)?,
                    health: r.get(7)?,
                    source: r.get(8)?,
                },
            ))
        })?;
        for r in rows {
            sat_states.push(r?);
        }
    }

    let mut rules = Vec::new();
    {
        // Rule visible at head only if its RULE_REVISION event is at/before head.
        let mut stmt = conn.prepare(
            "SELECT r.id, r.seq, r.parent_id, r.fingerprint, r.params_json, r.created_at_ms
             FROM rule_versions r
             JOIN events e ON e.project_id = r.project_id
               AND e.kind = 'RULE_REVISION'
               AND json_extract(e.payload_json, '$.rule_id') = r.id
             WHERE r.project_id = ?1 AND e.seq <= ?2
             ORDER BY r.seq",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            let pj: String = r.get(4)?;
            Ok(RuleMeta {
                id: r.get(0)?,
                seq: r.get(1)?,
                parent_id: r.get(2)?,
                fingerprint: r.get(3)?,
                params: serde_json::from_str(&pj).unwrap_or_default(),
                created_at_ms: r.get(5)?,
            })
        })?;
        for r in rows {
            rules.push(r?);
        }
    }
    if rules.is_empty() {
        return Err(anyhow_lite::err("no rule versions for project"));
    }

    // Decisions with undo state at head.
    let mut decisions = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT e.seq, d.candidate_id, d.action, d.proposal_json, d.reason, d.rule_id_at_time, e.kind
             FROM decisions d JOIN events e ON e.id = d.event_id
             WHERE d.project_id = ?1 AND e.seq <= ?2 ORDER BY e.seq",
        )?;
        let rows = stmt.query_map([project_id, head], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        for r in rows {
            let (event_seq, cand, action, proposal, reason, rule_id, kind) = r?;
            if kind == "DECISION_UNDO" {
                continue; // handled below
            }
            decisions.push(DecisionAtTime {
                event_seq,
                candidate_natural: cand,
                action,
                proposal: proposal.and_then(|p| serde_json::from_str(&p).ok()),
                reason,
                rule_id_at_time: rule_id,
                undone_event_seq: None,
            });
        }
        // Mark undone.
        let mut ustmt = conn.prepare(
            "SELECT json_extract(payload_json, '$.decision_event_seq') FROM events
             WHERE project_id = ?1 AND kind = 'DECISION_UNDO' AND seq <= ?2",
        )?;
        let undone: Vec<i64> = ustmt
            .query_map([project_id, head], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for d in decisions.iter_mut() {
            if undone.contains(&d.event_seq) {
                d.undone_event_seq = Some(d.event_seq);
            }
        }
        decisions.retain(|d| d.undone_event_seq.is_none());
    }

    let mut events = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT seq, kind, payload_json FROM events WHERE project_id = ?1 AND seq <= ?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map([project_id, head], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        for r in rows {
            let (s, k, pj) = r?;
            events.push((s, k, serde_json::from_str(&pj).unwrap_or(Value::Null)));
        }
    }

    Ok(ReplayInput {
        project_id: project_id.to_string(),
        batches,
        epochs,
        clock_events,
        sat_states,
        rules,
        decisions,
        events,
        head_event_seq: head,
    })
}

/// Parse an imported obs batch text back into a RinexFile for diagnostics.
pub fn parse_batch_text(kind: &str, text: &str) -> anyhow_lite::Result<RinexFile> {
    if kind == "OBS_FULL" || kind == "OBS_PATCH" {
        crate::parser::parse_rinex_obs(text)
            .map_err(|errs| anyhow_lite::err(&format!("RINEX errors: {}", errs.len())))
    } else {
        Err(anyhow_lite::err("not an observation batch"))
    }
}


fn visible_batch_ids(batches: &[BatchMeta]) -> Vec<String> {
    batches.iter().map(|b| b.id.clone()).collect()
}
