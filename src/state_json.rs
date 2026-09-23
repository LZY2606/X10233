//! Serialize a snapshot for the review UI and export. Every displayed value
//! carries a stable id, unit/datum and direct-source references.

use crate::engine::*;
use crate::model::{canonical_json, short_id};
use crate::service::natural_id_with_rule;
use serde_json::{json, Value};

pub fn norm_id(n: &NormValue, rule_id: &str) -> String {
    short_id("norm", &[&n.obs_id, rule_id])
}
pub fn arc_id(a: &ArcRecord, rule_id: &str) -> String {
    short_id("arc", &[rule_id, &a.sat, &a.code, &a.start_seq.to_string(), &a.end_seq.to_string(), &a.end_reason])
}
pub fn pair_id(p: &PairViewRecord, rule_id: &str) -> String {
    let v = &p.view.view;
    let mut sig = format!("{}|{}|{}|{}", p.sat, p.seq_a, p.seq_b, p.view.detector);
    for k in ["residual_m", "delta_gf_m", "jump_cycle"] {
        if let Some(val) = v.get(k).and_then(|x| x.get("value")) {
            sig.push('|');
            sig.push_str(&canonical_json(val));
        }
    }
    short_id("pair", &[rule_id, &sig])
}
pub fn candidate_id(natural: &str, rule_id: &str) -> String {
    natural_id_with_rule(natural, rule_id)
}

pub fn snapshot_to_json(snap: &Snapshot, _accepted: &[AcceptedOffset], head: Option<i64>) -> Value {
    json!({
        "project_id": snap.project_id,
        "head_event_seq": head.unwrap_or(-1),
        "algorithm_version": crate::model::ALGORITHM_VERSION,
        "rule": {
            "id": snap.rule_id,
            "seq": snap.rule_seq,
            "fingerprint": snap.rule_fingerprint,
            "params": snap.params,
        },
        "normalized_observations": snap.norms.iter().map(|n| json!({
            "id": norm_id(n, &snap.rule_id),
            "obs_id": n.obs_id,
            "code": n.code,
            "band": n.band,
            "kind": n.kind,
            "quantity": n.quantity,
            "direct_sources": [n.obs_id],
        })).collect::<Vec<_>>(),
        "clock_findings": snap.clock_findings.iter().map(|f| json!({
            "id": short_id("clockf", &[&snap.rule_id, &f.epoch_seq.to_string()]),
            "epoch_seq": f.epoch_seq,
            "epoch": f.epoch,
            "jump_s": {"value": f.jump_s, "unit":"s"},
            "mode": "COMMON_MODE",
            "affected_satellites": f.affected_sats,
            "evidence": f.evidence,
            "direct_sources": f.source_event_ids,
        })).collect::<Vec<_>>(),
        "arcs": snap.arcs.iter().map(|a| json!({
            "id": arc_id(a, &snap.rule_id),
            "satellite": a.sat,
            "code": a.code,
            "start_epoch_seq": a.start_seq,
            "end_epoch_seq": a.end_seq,
            "start_epoch": a.start_epoch,
            "end_epoch": a.end_epoch,
            "end_reason": a.end_reason,
        })).collect::<Vec<_>>(),
        "pair_results": snap.pair_views.iter().map(|p| json!({
            "id": pair_id(p, &snap.rule_id),
            "satellite": p.sat,
            "epoch_seq_a": p.seq_a,
            "epoch_seq_b": p.seq_b,
            "dt_s": {"value": p.dt_s, "unit":"s"},
            "gap_flag": p.gap_flag,
            "view": p.view.view,
            "direct_sources": p.view.source_obs_ids,
        })).collect::<Vec<_>>(),
        "candidates": snap.candidates.iter().map(|c| {
            let props: Vec<Value> = c.proposals.iter().map(|p| {
                let total: f64 = p.detectors.values().sum::<f64>() / p.detectors.len().max(1) as f64;
                json!({
                    "n1_cycle": {"value": p.n1, "unit":"cycle"},
                    "n2_cycle": {"value": p.n2, "unit":"cycle"},
                    "detector_scores": p.detectors,
                    "score": total,
                })
            }).collect();
            json!({
                "id": candidate_id(&c.natural_id_input, &snap.rule_id),
                "natural_key": c.natural_id_input,
                "satellite": c.sat,
                "epoch_seq": c.epoch_seq,
                "epoch": c.epoch,
                "proposals": props,
                "detector_views": c.detector_views,
                "lli_flags": c.lli_flags,
                "status": "candidate",
            })
        }).collect::<Vec<_>>(),
        "unknowns": snap.unknowns,
    })
}
