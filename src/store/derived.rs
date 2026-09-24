use super::project::PROJECT_ID;
use super::Store;
use crate::engine::compute;
use crate::model::*;
use rusqlite::params;
use std::collections::BTreeSet;

pub const KIND_STAT: &str = "statistic";
pub const KIND_CAND: &str = "candidate";
pub const KIND_ARC: &str = "arc";
pub const KIND_OFFSET: &str = "offset";

impl Store {
    /// Recompute at the tip of the current head chain, persist the snapshot,
    /// and refresh `materialized_view`. Rows whose stable identity vanished
    /// become STALE (their snapshots remain viewable).
    pub fn recompute_tip(&self) -> ViewState {
        let head = self.head();
        let view = if head.is_empty() {
            self.empty_view("ROOT")
        } else {
            self.recompute_at(&head)
        };
        self.materialize(&view);
        view
    }

    pub fn recompute_at(&self, viewpoint: &str) -> ViewState {
        let chain = self.ancestry(viewpoint);
        let batches: BTreeSet<String> = chain
            .iter()
            .filter_map(|e| match &e.body {
                crate::store::events::EventBody::Import { batch_id, .. } => {
                    Some(batch_id.clone())
                }
                _ => None,
            })
            .collect();
        let params = self.params_on_chain(viewpoint);
        let decisions = self.decisions_on_chain(viewpoint);
        let source = self.source_view_for_batches(&batches);
        let view = compute(viewpoint, &source, &params, &decisions);
        self.save_view(&view);
        view
    }

    pub fn empty_view(&self, viewpoint: &str) -> ViewState {
        ViewState {
            viewpoint_event_id: viewpoint.to_string(),
            params_version: self.current_params().fingerprint(),
            alg_fingerprint: crate::params::ALG_VERSION.to_string(),
            epochs: vec![],
            arcs: vec![],
            statistics: vec![],
            candidates: vec![],
            offsets: vec![],
            rejected: vec![],
            diagnostics: vec!["no imports yet".to_string()],
        }
    }

    pub fn save_view(&self, view: &ViewState) {
        self.with_conn_mut(|c| {
            let tx = c.transaction().unwrap();
            for s in &view.statistics {
                tx.execute(
                    "INSERT OR REPLACE INTO derived_statistics(id,project_id,viewpoint_event_id,\
                     detector,alg_version,param_fp,sat,band,at_seq,at_epoch,dt_secs,value,\
                     value_unit,flag,sources_json,diagnostic)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                    params![
                        s.id, PROJECT_ID, view.viewpoint_event_id, s.detector, s.alg_version,
                        s.param_fp, s.sat, s.band, s.at_seq, s.at_epoch, s.dt_secs, s.value,
                        s.value_unit,
                        format!("{:?}", s.flag),
                        serde_json::to_string(&s.sources).unwrap(),
                        s.diagnostic
                    ],
                )
                .unwrap();
            }
            for k in &view.candidates {
                tx.execute(
                    "INSERT OR REPLACE INTO derived_candidates(id,project_id,viewpoint_event_id,\
                     ambiguity_group,ambiguity_n,detector,alg_version,param_fp,sat,band,band_b,\
                     at_seq,at_epoch,offset_cycles,offset_b_cycles,offset_unknown,score,\
                     evidence_json,proposals_json,suggest_reject,status,status_reason,\
                     offset_ids_json,sources_json)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
                    params![
                        k.id, PROJECT_ID, view.viewpoint_event_id, k.ambiguity_group,
                        k.ambiguity_n, k.detector, k.alg_version, k.param_fp, k.sat, k.band,
                        k.band_b, k.at_seq, k.at_epoch, k.offset_cycles, k.offset_b_cycles,
                        k.offset_unknown as i64, k.score,
                        serde_json::to_string(&k.evidence).unwrap(),
                        serde_json::to_string(&k.detector_proposals).unwrap(),
                        k.suggest_reject as i64,
                        format!("{:?}", k.status),
                        k.status_reason,
                        serde_json::to_string(&k.offset_ids).unwrap(),
                        serde_json::to_string(&k.sources).unwrap()
                    ],
                )
                .unwrap();
            }
            for a in &view.arcs {
                tx.execute(
                    "INSERT OR REPLACE INTO derived_arcs(id,project_id,viewpoint_event_id,sat,\
                     band,start_seq,end_seq,start_epoch,end_epoch,state,close_reason,\
                     boundary_candidates_json)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                    params![
                        a.id, PROJECT_ID, view.viewpoint_event_id, a.sat, a.band, a.start_seq,
                        a.end_seq, a.start_epoch, a.end_epoch, format!("{:?}", a.state),
                        a.close_reason,
                        serde_json::to_string(&a.boundary_candidates).unwrap()
                    ],
                )
                .unwrap();
            }
            for o in &view.offsets {
                tx.execute(
                    "INSERT OR REPLACE INTO derived_offsets(id,project_id,viewpoint_event_id,sat,\
                     band,at_seq,at_epoch,cycles,unit,decision_event_id,rationale)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        o.id, PROJECT_ID, view.viewpoint_event_id, o.sat, o.band, o.at_seq,
                        o.at_epoch, o.cycles, o.unit, o.decision_event_id, o.rationale
                    ],
                )
                .unwrap();
            }
            for r in &view.rejected {
                tx.execute(
                    "INSERT INTO derived_rejected(project_id,viewpoint_event_id,candidate_id,\
                     reason_code,reason,decision_event_id)
                     VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        PROJECT_ID, view.viewpoint_event_id, r.candidate_id, r.reason_code,
                        r.reason, r.decision_event_id
                    ],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        });
    }

    fn materialize(&self, view: &ViewState) {
        self.with_conn_mut(|c| {
            let tx = c.transaction().unwrap();
            tx.execute("DELETE FROM materialized_view WHERE project_id=?1", params![PROJECT_ID])
                .unwrap();
            let mut ins = |kind: &str, id: &str| {
                tx.execute(
                    "INSERT INTO materialized_view(project_id,kind,stable_id,viewpoint_event_id,freshness)
                     VALUES(?1,?2,?3,?4,'CURRENT')",
                    params![PROJECT_ID, kind, id, view.viewpoint_event_id],
                )
                .unwrap();
            };
            for s in &view.statistics {
                ins(KIND_STAT, &s.id);
            }
            for k in &view.candidates {
                ins(KIND_CAND, &k.id);
            }
            for a in &view.arcs {
                ins(KIND_ARC, &a.id);
            }
            for o in &view.offsets {
                ins(KIND_OFFSET, &o.id);
            }
            tx.commit().unwrap();
        });
    }

    /// CURRENT when the row's stable identity exists at the tip; STALE rows
    /// are still readable from their viewpoint snapshot but cannot be shown as
    /// the current result.
    pub fn freshness(&self, kind: &str, stable_id: &str) -> &'static str {
        let cur = self.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM materialized_view WHERE project_id=?1 AND kind=?2 AND stable_id=?3",
                params![PROJECT_ID, kind, stable_id],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
        });
        if cur > 0 {
            "CURRENT"
        } else {
            "STALE"
        }
    }
}

// Re-export compute symbol so the module doc link stays valid.
#[allow(unused_import)]
use compute as _compute;
