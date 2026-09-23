//! 状态机与检测器：事件物化 -> 有效原始版本合并 -> 弧段/组合残差/候选。
//! 所有派生结果带算法版本、参数指纹与直接来源；输入片段只影响依赖它的弧段。
use crate::events::{EventPayload, ViewState, ALGO_VERSION};
use crate::ids::stable_id;
use crate::model::*;
use crate::store::Store;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub fn params_fingerprint(p: &DetectorParams) -> String {
    let json = serde_json::to_string(p).unwrap_or_default();
    stable_id("pp", &[ALGO_VERSION, &json])
}

// ---------- 事件物化 ----------

pub fn materialize(store: &Store, to: i64) -> Result<ViewState, String> {
    let chain = store.chain_to(to)?;
    let mut voided = HashSet::new();
    for ev in &chain {
        match &ev.payload {
            EventPayload::Undo { target_event, .. } => {
                voided.insert(*target_event);
            }
            EventPayload::ResolveConflict { drop_event, .. } => {
                voided.insert(*drop_event);
            }
            _ => {}
        }
    }
    let mut view = ViewState {
        head: to,
        params: DetectorParams::default(),
        params_fp: params_fingerprint(&DetectorParams::default()),
        ..Default::default()
    };
    for ev in &chain {
        if voided.contains(&ev.id) {
            continue;
        }
        match &ev.payload {
            EventPayload::ImportRaw { batch_id, .. } => view.batches.push(batch_id.clone()),
            EventPayload::ReviseParams { params, params_fp } => {
                view.params = params.clone();
                view.params_fp = params_fp.clone();
            }
            EventPayload::Adjudicate {
                candidate_id, verdict, chosen, note, detector_seen, params_fp, at_epoch,
            } => view.decisions.push(DecisionRecord {
                event_id: ev.id,
                candidate_id: candidate_id.clone(),
                verdict: *verdict,
                chosen: *chosen,
                note: note.clone(),
                detector: detector_from_str(detector_seen),
                params_fp: params_fp.clone(),
                at_epoch: at_epoch.clone(),
            }),
            EventPayload::ApplyRepair { entry } => {
                if !voided.contains(&entry.decision_event) {
                    view.repairs.push(entry.clone());
                }
            }
            EventPayload::RemoveRepair { arc_id, from_epoch, band, decision_event } => {
                view.repairs.retain(|r| {
                    !(r.arc_id == *arc_id
                        && r.from_epoch == *from_epoch
                        && r.band == *band
                        && r.decision_event == *decision_event)
                });
            }
            EventPayload::Undo { .. } | EventPayload::ResolveConflict { .. } => {}
        }
    }
    Ok(view)
}

fn detector_from_str(s: &str) -> Detector {
    match s {
        "GF" => Detector::IonosphereGeometryFree,
        "MW" => Detector::MelbourneWubbena,
        _ => Detector::TimeDifference,
    }
}

// ---------- 原始版本合并 ----------

#[derive(Clone)]
struct EpochRec {
    batch_id: String,
    epoch: RawEpoch,
}

#[derive(Default)]
pub struct MergedStream {
    pub station: String,
    pub epochs: BTreeMap<i64, (String, RawEpoch)>,
    pub clock_events: BTreeMap<i64, (crate::units::Quantity, String)>,
    pub sat_states: Vec<(SatState, String)>,
    pub diagnostics: Vec<(String, ParseDiagnostic)>,
    pub untimed: Vec<(String, RawEpoch)>,
}

/// 每个 stream 在链上最后一次 ImportRaw 为根批次；修订批次经 replaces 局部覆盖。
pub fn effective_root_ids(store: &Store, view: &ViewState) -> Result<Vec<String>, String> {
    let chain = store.chain_to(view.head)?;
    let mut per_stream: BTreeMap<String, String> = BTreeMap::new();
    for ev in &chain {
        if let EventPayload::ImportRaw { batch_id, stream, .. } = &ev.payload {
            per_stream.insert(stream.clone(), batch_id.clone());
        }
    }
    Ok(per_stream.into_values().collect())
}

pub fn merge(store: &Store, view: &ViewState) -> Result<MergedStream, String> {
    let mut out = MergedStream::default();
    let roots = effective_root_ids(store, view)?;
    let mut batches: Vec<RawBatch> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack = roots;
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let b = store.get_raw(&id)?;
        if let Some(p) = &b.parent_version {
            stack.push(p.clone());
        }
        batches.push(b);
    }
    batches.sort_by_key(|b| b.submitted_event);
    for batch in batches {
        if out.station.is_empty() {
            out.station = batch.station.clone();
        }
        for d in &batch.diagnostics {
            out.diagnostics.push((batch.id.clone(), d.clone()));
        }
        for ev in &batch.clock_events {
            if let Some(t) = ev.at.gps_sec {
                out.clock_events.insert(t, (ev.step.clone(), batch.id.clone()));
            }
        }
        for st in &batch.sat_states {
            out.sat_states.push((st.clone(), batch.id.clone()));
        }
        let full = batch.replaces.is_none();
        let replaced: HashSet<String> =
            batch.replaces.clone().unwrap_or_default().into_iter().collect();
        for epoch in batch.epochs {
            match epoch.time.gps_sec {
                Some(t) => {
                    let key = epoch.time.key();
                    if full || replaced.contains(&key) {
                        out.epochs.insert(t, (batch.id.clone(), epoch));
                    }
                }
                None => out.untimed.push((batch.id.clone(), epoch)),
            }
        }
    }
    Ok(out)
}

