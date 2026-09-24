//! 机器可读取事件：材料提交、规则/参数修订、计算结论、人工裁定、分叉、冲突解决与撤销。
//! 事件只追加，不可覆盖；回到任意旧事件即恢复当时依赖版本与可见结论。

use crate::model::Params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// 输入材料提交（不可变，含父版本链）
    InputCommitted,
    /// 参数/规则修订（产生新参数指纹）
    ParamRevised,
    /// 候选产生（计算结论的机器可读快照指针）
    CandidatesProposed,
    /// 接受某候选（修复只写偏移层）
    CandidateAccepted,
    /// 否定某候选
    CandidateRejected,
    /// 无法唯一决定：多个等分整周偏移
    AmbiguousNoted,
    /// 冲突被记录
    ConflictRecorded,
    /// 冲突解决
    ConflictResolved,
    /// 撤销某次人工决定（保留撤销本身）
    DecisionUndone,
    /// 从某事件分叉
    BranchForked,
    /// 导出归档
    Exported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// 内容寻址的稳定身份（与分支无关，便于分叉后对照）
    pub id: String,
    pub seq: i64,
    pub branch: String,
    pub kind: EventKind,
    /// 虚拟时间（固定 seed，测试可控），单位为 Unix 毫秒
    pub at_unix_ms: i64,
    /// 父事件（同分支线性链）
    pub parent_id: Option<String>,
    /// 机器可读判断依据
    pub rationale: Rationale,
    /// 关联对象（commit / boundary / candidate / branch 的稳定 id）
    pub refs: Vec<String>,
    /// 人类说明
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rationale {
    /// 规则版本
    pub algo_version: String,
    /// 参数指纹
    pub param_fingerprint: Option<String>,
    /// 直接来源（原始记录稳定 id）
    pub direct_sources: Vec<String>,
    /// 结论值（确定性 JSON 片段）
    pub payload: serde_json::Value,
}

impl Event {
    pub fn canonical(&self) -> String {
        serde_json::to_string(&EventCanon {
            branch: &self.branch,
            kind: &self.kind,
            at_unix_ms: self.at_unix_ms,
            parent_id: &self.parent_id,
            rationale: &self.rationale,
            refs: &self.refs,
            note: &self.note,
        })
        .unwrap_or_default()
    }
}

#[derive(Serialize)]
struct EventCanon<'a> {
    branch: &'a str,
    kind: &'a EventKind,
    at_unix_ms: i64,
    parent_id: &'a Option<String>,
    rationale: &'a Rationale,
    refs: &'a [String],
    note: &'a str,
}

/// 参数修订记录（前后指纹与完整参数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamChange {
    pub before: Params,
    pub after: Params,
    pub reason: String,
}
