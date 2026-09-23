//! 事件溯源：一切状态变化都是追加事件；事件带父指针与内容哈希链。
//! 回到任意旧事件 = 重放到该事件为止，恢复当时的依赖版本与可见结论。
use crate::model::*;
use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "detectors/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum EventPayload {
    /// 导入原始批次（不可变；修订 = 新批次 + parent_version）。
    ImportRaw {
        batch_id: String,
        stream: String,
        station: String,
        parent_version: Option<String>,
        replaces: Option<Vec<String>>,
    },
    /// 参数修订：新参数 + 指纹；旧参数派生的候选保留为“已过期”。
    ReviseParams { params: DetectorParams, params_fp: String },
    /// 人工裁定（接受/否定/LLI 误报/挂起）。可分叉：parent 指向旧事件。
    /// 决定时看到的检测器/参数指纹/历元一并记录，供反查。
    Adjudicate {
        candidate_id: String,
        verdict: Verdict,
        chosen: Option<usize>,
        note: String,
        detector_seen: String,
        params_fp: String,
        at_epoch: String,
    },
    /// 撤销：使某个裁定事件不再生效（本身也是事件，写入历史）。
    Undo { target_event: i64, note: String },
    /// 冲突解决：在两个互相冲突的裁定中选定生效分支。
    ResolveConflict { keep_event: i64, drop_event: i64, note: String },
    /// 采纳后生成的修复偏移层条目（只存偏移，不改原始载波）。
    ApplyRepair { entry: OffsetEntry },
    /// 移除修复偏移层条目（撤销采纳的配套事件）。
    RemoveRepair { arc_id: String, from_epoch: String, band: char, decision_event: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub parent: Option<i64>,
    pub logical_time: i64,
    pub payload: EventPayload,
    pub hash: String,
}

/// 重放到某个事件为止得到的物化视图。
#[derive(Debug, Clone, Default)]
pub struct ViewState {
    pub head: i64,
    /// 原始批次链：batch_id -> RawBatch 元信息（内容在 raw_inputs 表）。
    pub batches: Vec<String>,
    pub params: DetectorParams,
    pub params_fp: String,
    pub decisions: Vec<DecisionRecord>,
    pub repairs: Vec<OffsetEntry>,
    /// 被撤销/被冲突解决掉的事件 id。
    pub voided: Vec<i64>,
}

impl ViewState {
    pub fn decision_for(&self, candidate_id: &str) -> Option<&DecisionRecord> {
        self.decisions.iter().rev().find(|d| d.candidate_id == candidate_id)
    }
}
