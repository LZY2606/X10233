//! 领域模型：原始表示、规范化值、单位与坐标基准全部显式携带。
//!
//! 约定：
//! - 原始 token 一律保留（无法转换的值留在原位置并附诊断，不为画图丢弃）。
//! - 规范化值为 SI：伪距 m、载波 phase（cycle）、Doppler（Hz）、SNR（dBHz）、
//!   频率（Hz）、波长（m/cycle）；方位角/天顶距（deg），坐标基准 WGS84。

use serde::{Deserialize, Serialize};

/// 所有领域量携带的单位标注（展示与复核用）。
pub mod units {
    pub const METER: &str = "m";
    pub const CYCLE: &str = "cycle";
    pub const HERTZ: &str = "Hz";
    pub const DBHZ: &str = "dBHz";
    pub const SECOND: &str = "s";
    pub const DEG: &str = "deg";
    pub const HZ: &str = "Hz";
    pub const WAVELENGTH: &str = "m/cycle";
    pub const DATUM: &str = "WGS84";
}

/// 原始值 + 规范化值。无法转换时 normalized=None，诊断写入 diagnostics。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawValue {
    /// 文件中的原始 token，原样保留（含格式错误，如 `1.2x3`）。
    pub raw: String,
    /// 规范化数值；无法解析或无定义时为 None。
    pub normalized: Option<f64>,
    /// 规范化值的单位；None 表示无法转换。
    pub unit: Option<&'static str>,
    /// 诊断信息（为什么留在原位置）。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagnostics: Vec<String>,
}

impl RawValue {
    pub fn parsed(raw: String, value: f64, unit: &'static str) -> Self {
        Self { raw, normalized: Some(value), unit: Some(unit), diagnostics: vec![] }
    }
    pub fn unparseable(raw: String, want_unit: &'static str, why: String) -> Self {
        Self { raw, normalized: None, unit: Some(want_unit), diagnostics: vec![why] }
    }
    pub fn blank(want_unit: &'static str) -> Self {
        Self { raw: String::new(), normalized: None, unit: Some(want_unit), diagnostics: vec![] }
    }
    pub fn usable(&self) -> bool {
        self.normalized.is_some()
    }
}

/// 一条频率声明（频率编号）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreqDef {
    /// 频率编号，如 `1`、`2`。
    pub band: String,
    /// 标准频率（Hz）。
    pub frequency_hz: f64,
    /// 波长（m/cycle）。
    pub wavelength: f64,
    /// 观测码（原样保存）：伪距/载波/Doppler/信噪比。
    pub code_pr: String,
    pub code_ph: String,
    pub code_dop: String,
    pub code_snr: String,
}

/// 卫星状态（SAT 行）。全部携带单位/基准。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    pub epoch: String,
    pub azimuth: RawValue,
    pub zenith: RawValue,
    pub health: String,
    pub datum: &'static str,
}

/// 单条观测：站 × 星 × 历元 × 频率 × 观测码。
/// 伪距/载波/Doppler/LLI/信噪比/观测码全部按历元原样保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub station: String,
    pub sat: String,
    pub epoch: String,
    pub band: String,
    /// 观测码（按历元原样保存）：伪距/载波/Doppler/信噪比 各自的码。
    pub code_pr: String,
    pub code_ph: String,
    pub code_dop: String,
    pub code_snr: String,
    pub pseudorange: RawValue,
    pub carrier: RawValue,
    pub doppler: RawValue,
    /// LLI 原始标志（RINEX 约定：bit0=周跳/失锁 等），原样字符。
    pub lli: String,
    pub snr: RawValue,
    /// 文件内行号（溯源用）。
    pub line_no: u32,
}

/// 钟跳事件（整机/接收机级，与单星周跳分开保存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub station: String,
    pub epoch: String,
    /// 原始 token。
    pub raw: String,
    /// 规范化（秒），无法转换则为 None 并附诊断。
    pub shift_seconds: RawValue,
    /// 事件类别（明确标注为 receiver clock，不归属任何卫星）。
    pub kind: &'static str,
    pub line_no: u32,
}

/// 一次解析得到的一个导入版本的全部内容。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionPayload {
    pub station: String,
    pub freqs: Vec<FreqDef>,
    pub sats: Vec<SatState>,
    pub observations: Vec<Observation>,
    pub clocks: Vec<ClockEvent>,
    /// 非致命诊断（无法转换的值等）。
    pub warnings: Vec<String>,
}
