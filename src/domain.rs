//! 领域模型：RINEX 观测历元、卫星状态、弧段、候选、裁定、钟跳事件。
//! 原则：原始观测按历元原样保存（含原始文本与单位），派生层才做规范化。

use serde::{Deserialize, Serialize};

/// 观测值：同时保留原始文本表示与（可选的）规范化数值。
/// 无法解析/转换的值保留在原位置并附诊断，不为画图而丢弃。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObsValue {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String, // "m" | "cycles" | "Hz" | "dB-Hz" | "1"
    pub lli: Option<u8>,
    pub ssi: Option<u8>,
    pub diag: Option<String>, // 规范化诊断，例如 "unparseable"
}

impl ObsValue {
    pub fn missing() -> Self {
        Self { raw: String::new(), value: None, unit: "1".into(), lli: None, ssi: None, diag: None }
    }
    pub fn is_present(&self) -> bool {
        self.value.is_some()
    }
}

/// 单条观测码记录（如 C1C / L1C / D1C / S1C）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObsRecord {
    pub code: String, // 观测码，含频率编号，如 "L1C"
    pub obs: ObsValue,
}

/// 一颗卫星在一个历元的全部观测。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatObs {
    pub sat: String, // 如 "G05"
    pub records: Vec<ObsRecord>,
}

/// 一个历元（按 RINEX 原样；支持不等间隔）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpochObs {
    pub epoch: String, // 原始历元文本 "2024 03 01 00 00 00.0000000"
    pub epoch_seconds: f64, // 规范化：相对项目时标的秒（派生层）
    pub clock_offset: Option<f64>, // 接收机钟差（秒），整机钟跳事件由此体现
    pub sats: Vec<SatObs>,
}

/// 卫星状态（星历/健康标记），导入时按原样保存。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatState {
    pub sat: String,
    pub epoch_seconds: f64,
    pub x_km: f64,
    pub y_km: f64,
    pub z_km: f64,
    pub healthy: bool,
    pub frame: String, // 坐标基准，如 "ITRF2020"
}

/// 整机钟跳事件（与单星周跳严格分开）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockJumpEvent {
    pub epoch_seconds: f64,
    pub jump_ms: f64,
    pub source: String, // "rinex-header" | "receiver-flag" | "import-patch"
}

/// 解析后的数据集（导入单元）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dataset {
    pub station: String,
    pub marker: String,
    pub rinex_version: String,
    pub time_system: String, // 坐标/时间基准声明，如 "GPS"
    pub epochs: Vec<EpochObs>,
    pub sat_states: Vec<SatState>,
    pub clock_jumps: Vec<ClockJumpEvent>,
    pub diagnostics: Vec<String>, // 导入期诊断（保留在原位置的坏值等）
}

/// 观测码元数据：频率编号与观测类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeMeta {
    pub obs_type: char,       // C/L/D/S
    pub freq_no: u8,          // 频率编号 1/2/5...
    pub attribute: char,      // C/W/...
}

pub fn parse_code(code: &str) -> Option<CodeMeta> {
    let b = code.as_bytes();
    if b.len() < 2 { return None; }
    let obs_type = b[0] as char;
    if !matches!(obs_type, 'C' | 'L' | 'D' | 'S') { return None; }
    let freq_no = (b[1] as char).to_digit(10)? as u8;
    let attribute = if b.len() > 2 { b[2] as char } else { ' ' };
    Some(CodeMeta { obs_type, freq_no, attribute })
}

/// GNSS 频率（Hz），按系统与频率编号。
pub fn frequency_hz(sat: &str, freq_no: u8) -> Option<f64> {
    let sys = sat.chars().next()?;
    match (sys, freq_no) {
        ('G', 1) => Some(1_575_420_000.0),
        ('G', 2) => Some(1_227_600_000.0),
        ('G', 5) => Some(1_176_450_000.0),
        ('E', 1) => Some(1_575_420_000.0),
        ('E', 5) => Some(1_176_450_000.0),
        ('E', 7) => Some(1_207_140_000.0),
        ('C', 2) => Some(1_561_098_000.0),
        ('C', 6) => Some(1_268_520_000.0),
        _ => None,
    }
}

pub const C_LIGHT: f64 = 299_792_458.0;

pub fn wavelength_m(sat: &str, freq_no: u8) -> Option<f64> {
    frequency_hz(sat, freq_no).map(|f| C_LIGHT / f)
}

/// 弧段：站 × 星 × 频率编号。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Arc {
    pub arc_id: String,
    pub station: String,
    pub sat: String,
    pub freq_no: u8,
}

/// 检测器标识。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DetectorKind {
    IonosphereCombo,   // 电离层（几何自由）组合
    MelbourneWubbena,  // Melbourne-Wübbena 组合
    TimeDifference,    // 时间差分
}

impl DetectorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DetectorKind::IonosphereCombo => "iono_combo",
            DetectorKind::MelbourneWubbena => "melbourne_wubbena",
            DetectorKind::TimeDifference => "time_difference",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "iono_combo" => Some(DetectorKind::IonosphereCombo),
            "melbourne_wubbena" => Some(DetectorKind::MelbourneWubbena),
            "time_difference" => Some(DetectorKind::TimeDifference),
            _ => None,
        }
    }
}

/// 周跳候选（一等数据；任何检测器都不是真值）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub candidate_id: String,
    pub arc_id: String,
    pub epoch_seconds: f64,
    pub detector: DetectorKind,
    pub kind: String, // "cycle_slip" | "clock_jump" | "lli_flag"
    pub slip_cycles: Option<f64>, // 估计的整周偏移（未知则为 None）
    pub score: f64,
    pub residual: Option<f64>,
    pub status: String, // "open" | "accepted" | "rejected" | "superseded"
    pub detail: String, // 机器可读判断依据（JSON 字符串）
}

/// 修复假设 = 偏移层：只存整周偏移，不改原始观测。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OffsetLayer {
    pub layer_id: String,
    pub arc_id: String,
    pub epoch_seconds: f64,
    pub offset_cycles: f64,
    pub source_decision_id: String,
}

/// 人工裁定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub decision_id: String,
    pub candidate_id: String,
    pub action: String, // "accept" | "reject" | "adjust_params"
    pub rationale: String,
    pub reviewer: String,
}
