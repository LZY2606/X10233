//! 事件溯源：项目历史上只追加事件。
//!
//! 事件类型覆盖完整叙事：
//! - `import`：提交一个原始版本（不可变；parent_version 表达修订关系）
//! - `params`：参数修订（指纹化，仅依赖这些参数的派生结果过期）
//! - `decision`：人工裁定（接受候选/否定/手工偏移），只写偏移层
//! - `undo`：撤销一次裁定
//!
//! 事件身份由序号+内容确定性决定；回到任意事件可做 as-of 重放。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Import,
    Params,
    Decision,
    Undo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// 单调序号（项目内）。
    pub seq: i64,
    pub kind: EventKind,
    /// ISO8601 时间（虚拟时钟可固定）。
    pub at: String,
    /// 事件标签/说明（界面时间线使用）。
    pub label: String,
    /// 不透明 JSON 负载。
    pub payload: serde_json::Value,
}

/// 人工裁定负载（偏移层，绝不修改原始观测）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionPayload {
    /// 目标边界边（arc-edge）稳定身份。
    pub edge_id: String,
    /// `accept`（采纳某候选）/`reject`（否定该边为周跳）/`manual`（手工偏移）。
    pub action: String,
    /// 采纳/手工时对 band -> 整周偏移（cycles）；reject 时为空。
    #[serde(default)]
    pub offset_cycles: std::collections::BTreeMap<String, i64>,
    /// 采纳时引用的候选稳定身份（溯源）；manual 可为 None。
    #[serde(default)]
    pub candidate_id: Option<String>,
    /// 审阅者（固定演示用 `reviewer`，测试可固定）。
    pub reviewer: String,
    /// 备注。
    #[serde(default)]
    pub note: String,
}
