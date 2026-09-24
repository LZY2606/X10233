//! 领域模型。所有领域量携带单位或坐标基准；原始表示与规范化值并存。

use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "carrier-break-derive-1.0.0";

/// 带单位的标量。raw 保留导入时的原始文本，norm 才是派生层规范化结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Qty {
    pub value: f64,
    pub unit: String,
    pub raw: String,
    /// 无法转换时为 Some(诊断码)，此时 value=f64::NAN 且原值仍留在 raw。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl Qty {
    pub fn meters(v: f64, raw: impl Into<String>) -> Self {
        Qty { value: v, unit: "m".into(), raw: raw.into(), diagnostic: None }
    }
    pub fn cycles(v: f64, raw: impl Into<String>) -> Self {
        Qty { value: v, unit: "cycle".into(), raw: raw.into(), diagnostic: None }
    }
    pub fn hz(v: f64, raw: impl Into<String>) -> Self {
        Qty { value: v, unit: "Hz".into(), raw: raw.into(), diagnostic: None }
    }
    pub fn seconds(v: f64, raw: impl Into<String>) -> Self {
        Qty { value: v, unit: "s".into(), raw: raw.into(), diagnostic: None }
    }
    pub fn broken(unit: &str, raw: impl Into<String>, code: &str) -> Self {
        Qty { value: f64::NAN, unit: unit.into(), raw: raw.into(), diagnostic: Some(code.into()) }
    }
    pub fn usable(&self) -> bool {
        self.diagnostic.is_none() && self.value.is_finite()
    }
}

/// 频率定义（卫星状态导入），坐标基准为 ITRF 无关的频率基准。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrequencySpec {
    pub constellation: String,
    pub band: String,
    pub code: String,
    /// 频率编号（RINEX 频点编号原样保存）。
    pub freq_number: String,
    /// 波长，米/周。
    pub wavelength_m: f64,
    pub datum: String,
}

/// 钟跳事件，按历元原样保存（整机钟跳与单星周跳严格分开）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEvent {
    pub id: String,
    pub epoch_tag: String,
    pub raw: String,
    pub kind: String, // receiver_clock_jump
    pub shift_s: Qty,
    pub source_version: String,
}

/// 原始观测历元（不可覆盖；修订以新版本表达）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    pub id: String,
    pub tag: String,
    /// 历元时间（秒，相对项目时间基准）。
    pub time_s: Qty,
    pub time_raw: String,
    pub interval_from_prev_s: Option<f64>,
    pub regular_spacing: bool,
    pub raw_header: String,
    pub source_version: String,
    /// 该历元缺失的频点编号（如某星某频全无观测）。
    pub missing: Vec<String>,
}

/// 单颗卫星单频点的原始观测：伪距/载波/Doppler/LLI/信噪比全部原样保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub id: String,
    pub epoch_id: String,
    pub epoch_tag: String,
    pub sat: String,
    pub band: String,
    pub code: String,
    pub freq_number: String,
    pub pseudorange: Qty,
    pub carrier: Qty,
    pub doppler: Qty,
    pub lli: Qty,
    pub cnr: Qty,
    pub raw_line: String,
    pub source_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceVersion {
    pub id: String,
    pub parent_id: Option<String>,
    pub content_hash: String,
    pub raw_text: String,
    pub note: String,
}

/// 检测参数。指纹随内容变化，派生值绑定参数版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub gf_sigma_cycle: f64,
    pub mw_window: usize,
    pub mw_sigma_wl: f64,
    pub td_sigma_cycle: f64,
    pub clock_sigma_m: f64,
    pub lli_believe: bool,
    pub tie_keep_count: usize,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            gf_sigma_cycle: 0.5,
            mw_window: 4,
            mw_sigma_wl: 0.5,
            td_sigma_cycle: 0.5,
            clock_sigma_m: 100.0,
            lli_believe: true,
            tie_keep_count: 3,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        let s = serde_json::to_string(&[
            ALGO_VERSION,
            &self.gf_sigma_cycle.to_string(),
            &self.mw_window.to_string(),
            &self.mw_sigma_wl.to_string(),
            &self.td_sigma_cycle.to_string(),
            &self.clock_sigma_m.to_string(),
            &(if self.lli_believe { "1" } else { "0" }),
            &self.tie_keep_count.to_string(),
        ])
        .unwrap_or_default();
        crate::ids::digest(&[&s])
    }
}

/// 多频组合残差点（电离层 GF / MW / TD），都是一等派生数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComboPoint {
    pub id: String,
    pub detector: String, // gf | mw | td
    pub sat: String,
    pub epoch_tag: String,
    pub value: f64,
    pub unit: String,
    pub algo_version: String,
    pub params_fp: String,
    pub sources: Vec<String>,
    pub status: PointStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointStatus {
    Ok,
    MissingFrequency { band: String },
    Unconvertible { source: String, code: String },
    ClockJumpAffected,
    NoPriorWindow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Arc {
    pub id: String,
    pub sat: String,
    pub band: String,
    pub first_epoch: String,
    pub last_epoch: String,
    pub start_reason: String,
    pub end_reason: Option<String>,
    pub algo_version: String,
    pub params_fp: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub boundary_epoch: String,
    pub boundary_id: String,
    pub sat: String,
    pub detector: String, // gf | mw | td | lli | fused
    /// 目标层：n1 / n2 / wl；修复只写偏移层。
    pub target: String,
    pub offset_cycles: i64,
    pub score: f64,
    pub z_score: f64,
    pub sigma: f64,
    pub unit: String,
    pub estimate: f64,
    /// 是否为等分/并列最佳之一。
    pub tied: bool,
    pub status: CandidateStatus,
    pub reject_reason: Option<String>,
    pub algo_version: String,
    pub params_fp: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Open,
    Accepted,
    Rejected,
    Ambiguous,
    Error,
    Unknown,
}

/// 整机钟跳判定（与单星周跳分开的一等结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockCall {
    pub boundary_id: String,
    pub epoch_tag: String,
    pub common_shift_m: f64,
    pub support: usize,
    pub total: usize,
    pub classified: bool,
    pub sources: Vec<String>,
    pub status: String, // detected | rejected | unknown
}

/// 追加式事件。分支由 parent_id 链表达；当前头 = 各分支的最后一个事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub project: String,
    pub branch: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub clock_ms: i64,
}

/// 修复决定只落偏移层，永不改原始观测。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionPayload {
    pub candidate_id: String,
    pub boundary_id: String,
    pub sat: String,
    pub target: String,
    pub offset_cycles: i64,
    pub action: String, // accept | reject
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvePayload {
    pub boundary_id: String,
    pub sat: String,
    pub winning_candidate_id: String,
    pub rejected_candidate_ids: Vec<String>,
    pub rationale: String,
}
