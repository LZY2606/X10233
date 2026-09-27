//! 领域模型：原始输入（按历元原样保存）与一等数据结构。
//!
//! 所有领域量都携带单位或坐标基准；导入时保留原始文本表示，
//! 派生层才做规范化，无法转换的值留在原位置并附诊断。

use serde::{Deserialize, Serialize};

/// 带单位/基准与原始表示的领域量。
/// `value` 为规范化后的 SI 值（可能为 None，此时 `diagnostic` 必填）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quant {
    /// 规范化后的 SI 值；无法转换时为 None。
    pub value: Option<f64>,
    /// 单位或坐标基准，例如 "s"、"cycle"、"m"、"Hz"、"dB-Hz"、"ITRF2020:m"。
    pub unit: String,
    /// 导入文件中的原始文本表示（逐字保留）。
    pub raw: String,
    /// 无法转换时的诊断信息；可转换时为 None。
    pub diagnostic: Option<String>,
}

impl Quant {
    pub fn new(value: Option<f64>, unit: &str, raw: String, diagnostic: Option<String>) -> Self {
        Self { value, unit: unit.to_string(), raw, diagnostic }
    }

    pub fn parsed(value: f64, unit: &str, raw: String) -> Self {
        Self { value: Some(value), unit: unit.to_string(), raw, diagnostic: None }
    }

    pub fn broken(unit: &str, raw: String, diagnostic: String) -> Self {
        Self { value: None, unit: unit.to_string(), raw, diagnostic: Some(diagnostic) }
    }

    pub fn is_usable(&self) -> bool {
        self.value.map(|v| v.is_finite()).unwrap_or(false)
    }
}

/// 单个观测码上的观测项（同一卫星同一历元可有多个码/频率）。
/// LLI 与信噪比都挂在“具体观测码”上，而不是频率上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObsItem {
    /// 观测码，如 C1C、L1C、D1C、S1C。
    pub code: String,
    /// 按观测码语义带单位的量（C=米，L=周，D=Hz，S=原始信噪比序号/dB-Hz）。
    pub quant: Quant,
    /// Loss of Lock Indicator 原始数字（如 '1'），无则为 None。
    pub lli: Option<u8>,
}

/// 一个历元内某颗卫星的全部观测码。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSatEpoch {
    /// 卫星标识，如 G03、R07。
    pub sat: String,
    /// 该卫星该历元的各观测码。
    pub items: Vec<RawObsItem>,
}

/// 接收机钟跳事件（导入层原样记录；是否真为“整机钟跳”由派生层判定）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEventRecord {
    /// 事件所在历元序号（该输入版本内，0 起）。
    pub epoch_index: i64,
    /// 事件类型原文，如 "clock_jump"、"CLOCK JUMP"。
    pub kind: String,
    /// 幅度（可选，单位秒），原文保留。
    pub magnitude_s: Option<Quant>,
    /// 备注/原文。
    pub note: String,
}

/// 卫星状态（星历/位置），坐标基准必须显式给出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateRecord {
    pub epoch_gps_s: f64,
    pub sat: String,
    /// X/Y/Z 坐标及其基准（如 ECEF:ITRF2020，单位米）。
    pub pos: [Quant; 3],
    /// 钟差（秒，原文保留）。
    pub clock_bias_s: Option<Quant>,
}

/// 一个输入版本：原始输入一经提交不可覆盖，新版本以父版本关系表达。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputVersion {
    pub id: String,
    pub parent_id: Option<String>,
    pub label: String,
    pub format: String,
    /// 提交时的墙钟时间（仅作展示；不参与任何身份计算）。
    pub created_at: String,
    /// 历元元数据与卫星观测。
    pub epochs: Vec<EpochRecord>,
    pub clock_events: Vec<ClockEventRecord>,
    pub sat_states: Vec<SatStateRecord>,
    /// 导入诊断（格式错误、无法转换的值等，均为一等数据，绝不丢弃）。
    pub diagnostics: Vec<ImportDiagnostic>,
}

/// 单个历元：时间、接收机钟偏移、各卫星观测，全部按历元原样保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRecord {
    /// 输入版本内序号（0 起），用于稳定排序。
    pub index: i64,
    /// GPS 时间（自 1980-01-06 的连续秒，不含闰秒）。
    pub t_gps_s: f64,
    /// 时间戳原文（RINEX 历元行 / JSON 字符串）。
    pub time_raw: String,
    /// 接收机钟偏移（秒），原文保留。
    pub rx_clock_s: Option<Quant>,
    /// 该历元的原始历元行文本（逐字保存，用于反查）。
    pub raw_line: String,
    pub sats: Vec<RawSatEpoch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportDiagnostic {
    /// "epoch" / "sat" / "value" / "header" / "file"
    pub scope: String,
    pub epoch_index: Option<i64>,
    pub sat: Option<String>,
    pub code: Option<String>,
    pub message: String,
    /// 导致诊断的原始片段。
    pub raw: String,
}

/// 组合波段：系统 + 频段（弧段以波段而非观测码划分，LLI 仍留在观测码层）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BandKey {
    pub sat: String,
    /// 频段号，如 "1"、"2"、"5"。
    pub band: String,
}

impl BandKey {
    pub fn key(&self) -> String {
        format!("{}:{}", self.sat, self.band)
    }
}

/// 观测码类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
}

/// 从观测码（RINEX 3 字符，如 C1C/L1C/D1C/S1C；或 RINEX2 老码如 P1/L1）解析类别与频段。
pub fn parse_code(code: &str) -> Option<(ObsKind, String, char)> {
    let bytes = code.as_bytes();
    if bytes.len() >= 3 {
        let kind = match bytes[0] {
            b'C' | b'P' => ObsKind::Pseudorange,
            b'L' => ObsKind::Carrier,
            b'D' => ObsKind::Doppler,
            b'S' => ObsKind::Snr,
            _ => return None,
        };
        return Some((kind, (bytes[1] as char).to_string(), bytes[2] as char));
    }
    if bytes.len() == 2 {
        let kind = match bytes[0] {
            b'C' | b'P' => ObsKind::Pseudorange,
            b'L' => ObsKind::Carrier,
            b'D' => ObsKind::Doppler,
            b'S' => ObsKind::Snr,
            _ => return None,
        };
        return Some((kind, (bytes[1] as char).to_string(), 'C'));
    }
    None
}

/// 系统对应的频率表（频段 -> 标称频率 Hz）。GLONASS 需频率号，按频分通道换算。
pub fn band_frequency_hz(sat: &str, band: &str, glo_k: Option<i32>) -> Option<f64> {
    let sys = sat.chars().next().unwrap_or('G');
    match (sys, band) {
        ('G', "1") | ('E', "1") => Some(1575.42e6),
        ('G', "2") | ('E', "8") => Some(1227.60e6),
        ('G', "5") | ('E', "5") => Some(1176.45e6),
        ('E', "7") => Some(1207.140e6),
        ('E', "6") => Some(1278.750e6),
        ('R', "1") => Some(1602.0e6 + (glo_k.unwrap_or(0) as f64) * 9.0 / 16.0 * 1e6),
        ('R', "2") => Some(1246.0e6 + (glo_k.unwrap_or(0) as f64) * 7.0 / 16.0 * 1e6),
        ('C', "1") => Some(1575.42e6),
        ('C', "2") => Some(1207.140e6),
        ('C', "5") | ('C', "7") => Some(1176.45e6),
        _ => None,
    }
}

pub const C_LIGHT_M_S: f64 = 299_792_458.0;

/// 判定状态：候选、被拒原因、误差与未知都是一等数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Disposition {
    Proposed,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    /// 无法唯一决定：多个等分整周偏移仍合法。
    Ambiguous,
    Unique,
    Rejected,
}
