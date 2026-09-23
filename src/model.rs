//! 原始层（不可变、按历元原样保存）与事件载荷（append-only、可分叉）。

use crate::units::{Qty, EpochTime};
use serde::{Deserialize, Serialize};

// ---------- 原始层 ----------

/// 原始观测码：与 RINEX 观测类型一一对应（如 L1/C1/D1/S1/L2 ...）。
/// frequency_number 来自观测码派生（L1->1, L2->2, L5->5 ...），观测码原样保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    /// rawobs_<batch>_<epoch#>_<sat>_<code>
    pub id: String,
    pub epoch_id: String,
    pub batch_id: String,
    pub site: String,
    pub sat: String,
    /// 观测码原文，如 "L1"
    pub obs_code: String,
    /// 观测类型大类
    pub kind: ObsKind,
    /// 频率编号 1/2/5...
    pub frequency_number: i32,
    pub qty: Qty,
    /// RINEX LLI 原文（0-9 或空白 -> None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lli: Option<i32>,
    /// RINEX 信号强度原文 1-9
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal_strength: Option<i32>,
    /// 原始 14/16 列字段文本
    pub raw_field: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    /// epoch_<batch>_<#>
    pub id: String,
    pub batch_id: String,
    pub site: String,
    pub index_in_batch: i64,
    pub time: EpochTime,
    pub epoch_flag: i32,
    pub sats: Vec<String>,
    /// 历元头原文（含钟漂/时钟偏移列原文）
    pub raw_header: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_clock_offset_raw: Option<String>,
}

/// 整机钟跳事件（台站级，不是任何单星的周跳）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEventRow {
    /// clockevt_<batch>_<#>
    pub id: String,
    pub batch_id: String,
    pub site: String,
    /// 绑定的历元 id（按轴时间最近邻精确匹配；匹配失败带诊断）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_id: Option<String>,
    pub at_raw: String,
    pub at_axis_seconds: Option<f64>,
    /// 钟跳大小（秒，带符号，原文保留）
    pub jump: Qty,
    pub event_kind: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateRow {
    /// satstate_<batch>_<#>
    pub id: String,
    pub batch_id: String,
    pub site: String,
    pub sat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_id: Option<String>,
    pub at_axis_seconds: Option<f64>,
    pub x_m: Qty,
    pub y_m: Qty,
    pub z_m: Qty,
    pub datum: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticRow {
    /// diag_<batch>_<#>
    pub id: String,
    pub batch_id: String,
    pub project_id: String,
    pub severity: String, // error | warning
    pub code: String,
    pub line_no: Option<i64>,
    pub context: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBatch {
    pub id: String,
    pub project_id: String,
    pub seq: i64,
    pub kind: String, // rinex_o | fragment_v1 | clock_events | sat_states
    pub filename: String,
    pub content: String,
    pub content_fp: String,
    pub parent_batch_id: Option<String>,
    /// fragment_v1：替换哪些 (epoch#, sat, code) 原始单元
    #[serde(default)]
    pub replaces: Vec<FragmentReplacement>,
    pub submitted_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentReplacement {
    pub epoch_index: i64,
    pub sat: String,
    pub obs_code: String,
    /// 新单元原文，如 "    1234.567  " 或 16 空格（缺失）
    pub new_field_raw: String,
    /// 被替换单元的 rawobs id（身份保持不变）
    pub target_obs_id: String,
    pub reason: String,
}

// ---------- 事件（append-only）----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub seq: i64,
    pub branch_id: String,
    pub parent_event_id: Option<String>,
    pub kind: String,
    pub at_ms: i64,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    ProjectCreated {
        site: String,
        name: String,
    },
    /// 提交一批原始材料（不可覆盖；fragment 通过 replaces 表达新版本/父版本关系）
    RawSubmitted {
        batch_id: String,
        kind: String,
        filename: String,
        content_fp: String,
        parent_batch_id: Option<String>,
        n_epochs: i64,
        n_obs: i64,
        n_clock_events: i64,
        n_sat_states: i64,
        n_diagnostics: i64,
    },
    /// 参数修订（新版本 + 父版本关系，不动任何原始值）
    RulesRevised {
        ruleset_id: String,
        parent_ruleset_id: Option<String>,
        params: std::collections::BTreeMap<String, f64>,
        param_fp: String,
        note: String,
    },
    /// 人工裁定：接受/拒绝某候选；接受时修复只落在 offset 层
    Decision {
        candidate_id: String,
        link_id: String,
        arc_id: Option<String>,
        verdict: String, // accepted | rejected
        /// 接受时选择的 (n1,n2) 整周偏移；单频 n2=None
        n1: Option<i64>,
        n2: Option<i64>,
        note: String,
        conflict_with: Option<String>, // 分叉后与另一分支决定冲突时记录
    },
    /// 撤销上一条人工裁定（也写入历史）
    DecisionRetracted {
        decision_event_id: String,
        candidate_id: String,
        reason: String,
    },
    /// 从某事件分叉重算
    Fork {
        new_branch_id: String,
        name: String,
        from_event_id: String,
    },
}
