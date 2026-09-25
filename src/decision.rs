//! 人工决定与事件（事件溯源）。决定只写"偏移层"，绝不改动原始观测。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionAction {
    /// 采纳某个修复假设（写入整周偏移层）
    Accept,
    /// 否定该候选（非周跳 / 不修复）
    Reject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    /// 候选组定位：站-星历元边界
    pub epoch_index: usize,
    pub satellite: String,
    pub action: DecisionAction,
    /// Accept 时各频带整周偏移；Reject 为空
    pub offsets: std::collections::BTreeMap<u8, i64>,
    pub note: String,
    pub created_at: String,
    /// 被本决定取代的旧决定 id（冲突解决也写入历史）
    pub supersedes: Option<String>,
    /// 在哪个参数指纹/来源版本上做出
    pub param_fingerprint: String,
    pub source_import: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
    ProjectCreated,
    Imported,
    ParamsRevised,
    DecisionRecorded,
    ForkCreated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    /// 分支内严格递增序号，决定顺序
    pub seq: u64,
    pub at: String,
    pub kind: EventKind,
    /// 机器可读取的判断依据载荷
    pub payload: serde_json::Value,
}
