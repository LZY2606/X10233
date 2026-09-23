//! 重放引擎：把不可变事件逐步投影成当前/历史视图。
//!
//! - 同样的事件序列 + 规则版本 + 参数指纹 → 同样的结果（固定 seed、虚拟时间）。
//! - 局部源数据改动只令依赖它的弧段/候选过期；过期结果仍可查看，状态标 stale，不冒充当前。
//! - 检测器（TD/GF/MW/钟跳/LLI）只给候选，任何一个都不是真值。
//! - 修复只存“偏移层”，绝不改写原始载波。

use crate::domain::*;
use crate::events::{Event, EventPayload};
use crate::params::DetectorParams;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const C: f64 = 299_792_458.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockFinding {
    pub id: String,
    pub station: String,
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub jump_m: Measure,
    pub agreeing_sats: Vec<String>,
    pub total_sats: usize,
    pub annotated: bool,
    pub sources: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticNote {
    pub id: String,
    pub station: String,
    pub epoch_token: String,
    pub sat: String,
    pub band: String,
    pub code: String,
    pub message: String,
    pub raw: String,
    pub source: SourceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptedOffset {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub start_epoch: String,
    pub cycles: f64,
    pub by_event_id: String,
    pub candidate_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub head_event_id: String,
    pub seq: i64,
    pub label: String,
    pub canonical: bool,
    pub decided: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub project_id: String,
    pub at_event_id: Option<String>,
    pub head_event_id: Option<String>,
    pub canonical_head_event_id: Option<String>,
    pub params: DetectorParams,
    pub rules_version: String,
    pub param_fingerprint: String,
    pub algorithm_version: String,
    pub epochs: Vec<String>,
    pub epoch_time: BTreeMap<String, Option<f64>>,
    pub observations: Vec<Observation>,
    pub epoch_events: Vec<EpochEvent>,
    pub satellite_states: Vec<SatelliteState>,
    pub frequencies: Vec<FrequencyDef>,
    pub arcs: Vec<ArcSegment>,
    pub groups: Vec<CandidateGroup>,
    pub combos: Vec<ComboSeries>,
    pub clock_findings: Vec<ClockFinding>,
    pub diagnostics: Vec<DiagnosticNote>,
    pub offsets: Vec<AdoptedOffset>,
    pub events: Vec<Event>,
    pub branches: Vec<BranchInfo>,
    pub stale_entities: BTreeMap<String, String>,
    /// 当前链上已采纳候选 id 集合。
    pub adopted_candidate_ids: BTreeSet<String>,
    pub rejected_candidate_ids: BTreeSet<String>,
}

pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update(b"|");
        }
        h.update(p.as_bytes());
    }
    format!("{prefix}_{}", &hex::encode(h.finalize())[..16])
}

/// 把事件森林按 parent 链接起来，返回从根到某 event 的链（含自身）。
pub fn chain_to(events: &[Event], target_id: &str) -> Vec<Event> {
    let by_id: BTreeMap<&str, &Event> = events.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut chain = Vec::new();
    let mut cur = Some(target_id);
    let mut guard = 0usize;
    while let Some(id) = cur {
        guard += 1;
        if guard > events.len() + 2 {
            break;
        }
        if let Some(ev) = by_id.get(id.as_str()) {
            chain.push((*ev).clone());
            cur = ev.parent_event_id.as_deref();
        } else {
            break;
        }
    }
    chain.reverse();
    chain
}

/// 计算所有叶子（没有任何事件把它当 parent）。
pub fn leaf_events(events: &[Event]) -> Vec<Event> {
    let non_leaf: BTreeSet<String> =
        events.iter().filter_map(|e| e.parent_event_id.clone()).collect();
    let mut leaves: Vec<Event> =
        events.iter().filter(|e| !non_leaf.contains(&e.id)).cloned().collect();
    leaves.sort_by_key(|e| e.seq);
    leaves
}

/// 决定规范 head：
/// - 最新 ResolveConflict 决定 chosen_head；取该 head 的最新叶子后代；
/// - 无冲突决定时，取 seq 最大的叶子。
pub fn canonical_head(events: &[Event]) -> Option<String> {
    let leaves = leaf_events(events);
    let resolution = events
        .iter()
        .filter(|e| {
            matches!(
                e.payload,
                EventPayload::Decision {
                    kind: DecisionKind::ResolveConflict,
                    ..
                }
            )
        })
        .max_by_key(|e| e.seq);
    if let Some(r) = resolution {
        if let EventPayload::Decision { chosen_head_event_id: Some(chosen), .. } = &r.payload {
            let chosen_chain: BTreeSet<String> = chain_to(events, chosen).iter().map(|e| e.id.clone()).collect();
            let best = leaves
                .iter()
                .filter(|l| chain_to(events, &l.id).iter().any(|e| chosen_chain.contains(&e.id)))
                .max_by_key(|e| e.seq)
                .cloned();
            if let Some(b) = best {
                return Some(b.id);
            }
            return Some(chosen.clone());
        }
    }
    leaves.last().map(|e| e.id.clone())
}
