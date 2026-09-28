//! 领域模型：所有领域量都携带单位/坐标基准；原始表示与派生态分层保存。

use serde::{Deserialize, Serialize};

/// 带单位（必要时带坐标基准）的领域量。派生层只产出这种值，无法转换的值留在原始字段并附诊断。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub datum: Option<String>,
}

impl Quantity {
    pub fn new(value: f64, unit: &str) -> Self {
        Self { value, unit: unit.to_string(), datum: None }
    }
    pub fn with_datum(value: f64, unit: &str, datum: &str) -> Self {
        Self { value, unit: unit.to_string(), datum: Some(datum.to_string()) }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObsKind {
    Pseudorange,
    Phase,
    Doppler,
}

impl ObsKind {
    pub fn unit(&self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "metre",
            ObsKind::Phase => "cycle",
            ObsKind::Doppler => "hertz",
        }
    }
}

/// 单个观测码字段（如 C1C / L1C / D1C）。raw_value 保留历元原样表示。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObsField {
    /// RINEX 观测码，原样保存，如 "C1C"、"L2W"。
    pub code: String,
    pub kind: ObsKind,
    /// 由观测码推出的频率编号（频点），推不出时为 None 并附诊断。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub band: Option<u8>,
    /// RINEX 历元中的原始字符串（含空白语义：空字段 = 缺失）。
    pub raw_value: String,
    /// 规范化后的 SI 数值：伪距米、载波周、Doppler 赫兹；无法转换为 None。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value_si: Option<f64>,
    /// LLI 标志原始位（bit0 失锁等），无标志为 None。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lli: Option<u8>,
    /// 信噪比原始标志位 1..9，缺失为 None；不做映射以保留原始表示。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub snr_flag: Option<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}

impl ObsField {
    pub fn unit(&self) -> &'static str {
        self.kind.unit()
    }
    /// LLI bit0：前一历元以来失锁/周跳指示。
    pub fn lli_lost_lock(&self) -> bool {
        self.lli.map(|b| b & 0b001 != 0).unwrap_or(false)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SatEpoch {
    /// 卫星标识原样，如 "G02"。
    pub sat: String,
    pub fields: Vec<ObsField>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochRecord {
    /// 合并时间线上的稳定序号（提交时确定）。
    pub epoch_index: usize,
    /// 文件内次序（诊断用）。
    pub index_in_file: usize,
    /// ISO8601 UTC 文本，如 2026-09-28T00:00:00。
    pub iso: String,
    /// 相对该提交首历元的秒数（UTC 日历换算），单位秒。
    pub t_seconds: Quantity,
    pub epoch_flag_raw: String,
    /// 历元行接收机钟差原始字符串（空白=缺失）。
    pub clock_offset_raw: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub clock_offset_s: Option<f64>,
    pub sats: Vec<SatEpoch>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagLevel {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line_no: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub context: Option<String>,
}

/// 近似站坐标等头部元数据，原始坐标串保留、换算值另存。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ImportMeta {
    pub rinex_version: String,
    pub marker_name: String,
    pub observer_agency: String,
    pub receiver: String,
    pub antenna: String,
    pub approx_xyz_raw: [String; 3],
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub approx_xyz_m: Vec<f64>,
    pub approx_datum: String,
    pub types_observed_raw: Vec<String>,
}

/// RINEX 导入的解析结果（原始层）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsedImport {
    pub meta: ImportMeta,
    pub epochs: Vec<EpochRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 外部钟跳事件（野外记录等直接来源），与单星周跳严格分开。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClockEventInput {
    pub event_id: String,
    pub kind: String, // 目前仅 receiver_clock_jump
    pub epoch_index: usize,
    pub shift: Quantity,
    pub source: String,
    pub note: String,
}

/// 卫星状态：坐标与基准原样保留；案例包中用于核对，检测器不把它当真值。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SatelliteStateInput {
    pub sat: String,
    pub epoch_index: usize,
    pub x: Quantity,
    pub y: Quantity,
    pub z: Quantity,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub clock_bias_s: Option<f64>,
    pub source: String,
}
