//! 领域模型：所有领域量携带单位或坐标基准；原始表示保留在原始层。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 物理单位 / 坐标基准。未识别单位的量保留原始字符串，不做转换。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meter,
    Cycle,
    MeterPerSecond,
    Hertz,
    Second,
    MeterSquaredPerSecondSquared,
    Other(String),
    Raw,
}

impl Unit {
    pub fn label(&self) -> String {
        match self {
            Unit::Meter => "m".into(),
            Unit::Cycle => "cycle".into(),
            Unit::MeterPerSecond => "m/s".into(),
            Unit::Hertz => "Hz".into(),
            Unit::Second => "s".into(),
            Unit::MeterSquaredPerSecondSquared => "m^2/s^2".into(),
            Unit::Other(s) => s.clone(),
            Unit::Raw => "raw".into(),
        }
    }
    pub fn from_label(s: &str) -> Unit {
        match s.trim() {
            "m" => Unit::Meter,
            "cycle" => Unit::Cycle,
            "m/s" => Unit::MeterPerSecond,
            "Hz" => Unit::Hertz,
            "s" => Unit::Second,
            "m^2/s^2" => Unit::MeterSquaredPerSecondSquared,
            "raw" | "" => Unit::Raw,
            other => Unit::Other(other.to_string()),
        }
    }
}

/// 可诊断的标量值：能转换就转成 f64（规范化仅发生在派生层），
/// 无法转换时保留原始 token，并把诊断留在原处，绝不为画图丢值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measure {
    pub value: Option<f64>,
    pub unit: Unit,
    pub raw: String,
    pub diagnostic: Option<String>,
}

impl Measure {
    pub fn raw_only(raw: String, unit: Unit, diag: String) -> Self {
        Measure { value: None, unit, raw, diagnostic: Some(diag) }
    }
    pub fn numeric(value: f64, raw: String, unit: Unit) -> Self {
        Measure { value: Some(value), unit, raw, diagnostic: None }
    }
    pub fn is_usable(&self) -> bool {
        self.value.is_some() && self.diagnostic.is_none()
    }
}

/// 观测码类别（RINEX 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CodeKind {
    PseudoRange,
    CarrierPhase,
    Doppler,
    Snr,
}

/// 单条按历元原样保存的观测：站-星-频率（波段）-观测码粒度。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub station: String,
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub sat: String,
    pub band: String,
    pub code: String,
    pub kind: CodeKind,
    pub measure: Measure,
    pub source_ref: SourceRef,
}

/// 历元级原始事件行（如整机钟跳标记），逐字保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochEvent {
    pub station: String,
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub kind: String,
    pub fields: Vec<String>,
    pub source_ref: SourceRef,
}

/// 卫星状态（位置/速度、参考框架），按历元原样保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatelliteState {
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub sat: String,
    pub frame: String,
    pub x: Measure,
    pub y: Measure,
    pub z: Measure,
    pub vx: Measure,
    pub vy: Measure,
    pub vz: Measure,
    pub clock_bias: Measure,
    pub source_ref: SourceRef,
}

/// 频率定义：编号 + 频点。GLONASS 用 K 通道号。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyDef {
    pub system: String,
    pub band: String,
    pub code: String,
    pub frequency_hz: f64,
    pub glonass_k: Option<i32>,
    pub source_ref: SourceRef,
}

/// 直接来源指针：原始层实体都能反查到批次、文件名、行号与原文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub batch_id: String,
    pub file_name: String,
    pub line_no: usize,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedBatch {
    pub batch_id: String,
    pub kind: String,
    pub file_name: String,
    pub received_at_ms: u64,
    pub observations: Vec<Observation>,
    pub epoch_events: Vec<EpochEvent>,
    pub satellite_states: Vec<SatelliteState>,
    pub frequencies: Vec<FrequencyDef>,
    pub raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    pub file_name: String,
    pub line_no: usize,
    pub message: String,
    pub raw_line: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.file_name, self.line_no, self.message)
    }
}

/// 检测器身份。任何检测器都不是真值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detector {
    Td,
    Gf,
    Mw,
    Clock,
    Lli,
}

impl Detector {
    pub fn name(self) -> &'static str {
        match self {
            Detector::Td => "time-difference",
            Detector::Gf => "ionosphere-free/geometry-free",
            Detector::Mw => "Melbourne-Wubbena",
            Detector::Clock => "common-mode clock",
            Detector::Lli => "LLI indicator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityStatus {
    Current,
    Stale,
    Historical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    AcceptCandidate,
    RejectCandidate,
    ConfirmClock,
    MarkLliFalsePositive,
    ResolveConflict,
    Undo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub detector: Detector,
    pub band: Option<String>,
    /// 整数偏移假设（cycle），多频组合按波段给配对。
    pub offset_cycles: BTreeMap<String, i64>,
    pub residual: Measure,
    pub score: f64,
    pub reason: String,
    pub adopted: bool,
    pub rejected_reason: Option<String>,
    pub sources: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateGroup {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub detector: Detector,
    pub status: GroupStatus,
    pub candidates: Vec<Candidate>,
    pub note: Option<String>,
    pub sources: Vec<SourceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupStatus {
    Open,
    Accepted,
    Rejected,
    Conflict,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub cause: String,
    pub detector: Option<Detector>,
    pub gap_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcSegment {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub band: String,
    pub start_epoch: String,
    pub end_epoch: String,
    pub boundaries: Vec<Boundary>,
    pub status: EntityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualPoint {
    pub epoch_token: String,
    pub time_s: Option<f64>,
    pub value: Measure,
}

/// 多频组合残差序列（TD/GF/MW），全部带单位，无法转换的点留在原位置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComboSeries {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub combo: String,
    pub unit: String,
    pub points: Vec<ResidualPoint>,
}
