//! 领域模型: 所有领域量携带单位或坐标基准; 原始表示不可变, 派生层做规范化。
use serde::{Deserialize, Serialize};

pub const C_M_PER_S: f64 = 299_792_458.0;
pub const F1_HZ: f64 = 1575.42e6;
pub const F2_HZ: f64 = 1227.60e6;
pub const LAM1_M: f64 = C_M_PER_S / F1_HZ;
pub const LAM2_M: f64 = C_M_PER_S / F2_HZ;
pub const LAM_WL_M: f64 = C_M_PER_S / (F1_HZ - F2_HZ);
pub const ENGINE_VERSION: &str = "cbs-engine/1";

/// GPS 历元: 周 + 周内秒(坐标基准: GPS 时)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Epoch {
    pub week: i64,
    pub tow: f64,
}

impl Epoch {
    pub fn key(&self) -> (i64, i64) {
        (self.week, (self.tow * 1000.0).round() as i64)
    }
    pub fn minus(&self, other: &Epoch) -> f64 {
        (self.week - other.week) as f64 * 604_800.0 + (self.tow - other.tow)
    }
}

/// 观测值行: raw 为历元原样表示; value/unit/freq 为派生规范化, 无法转换时
/// value=None 且 diag 给出机器可读诊断, 原值留在原位置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsRow {
    pub id: String,
    pub batch_id: String,
    pub week: i64,
    pub tow: f64,
    pub sat: String,
    pub code: String,
    pub raw: String,
    pub lli: Option<i64>,
    pub ssi: Option<i64>,
    pub value: Option<f64>,
    pub unit: String,
    pub freq_hz: Option<f64>,
    pub freq_origin: Option<String>,
    pub diag: Option<String>,
}

impl ObsRow {
    pub fn epoch(&self) -> Epoch {
        Epoch { week: self.week, tow: self.tow }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRecord {
    pub id: String,
    pub batch_id: String,
    pub week: i64,
    pub tow: f64,
    pub flag: i64,
    pub num_sats: i64,
    pub rcv_clk_offset_raw: Option<String>,
    pub rcv_clk_offset_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diag {
    pub id: String,
    pub batch_id: String,
    pub line_no: i64,
    pub severity: String, // "error" | "warning"
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

/// 算法参数: 任何修订产生新版本(父版本链), 指纹参与派生值身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    pub max_gap_s: f64,
    pub clock_min_m: f64,
    pub clock_min_sats: i64,
    pub td_tol_cycles: f64,
    pub gf_tol_cycles: f64,
    pub mw_tol_cycles: f64,
    pub mw_window: i64,
    pub complexity_penalty: f64,
    pub engine_version: String,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 60.0,
            clock_min_m: 5.0,
            clock_min_sats: 2,
            td_tol_cycles: 0.25,
            gf_tol_cycles: 0.30,
            mw_tol_cycles: 0.50,
            mw_window: 5,
            complexity_penalty: 1e-4,
            engine_version: ENGINE_VERSION.to_string(),
        }
    }
}

/// 标准频率目录(坐标基准: 标准频带编号)。RINEX 头可覆盖。
pub fn standard_freq_hz(sat: &str, code: &str) -> Option<(f64, String)> {
    if code.len() != 3 || !code.starts_with('L') {
        return None;
    }
    let band = code.as_bytes()[1] as char;
    let sys = sat.chars().next()?;
    match (sys, band) {
        ('G', '1') => Some((F1_HZ, "std-band:G1".into())),
        ('G', '2') => Some((F2_HZ, "std-band:G2".into())),
        ('G', '5') => Some((1_176.45e6, "std-band:G5".into())),
        ('R', '1') => Some((1_602.0e6, "std-band:R1(k=0)".into())),
        ('R', '2') => Some((1_246.0e6, "std-band:R2(k=0)".into())),
        ('E', '1') => Some((F1_HZ, "std-band:E1".into())),
        ('E', '5') => Some((1_176.45e6, "std-band:E5a".into())),
        _ => None,
    }
}

pub fn unit_of(code: &str) -> &'static str {
    match code.chars().next() {
        Some('C') => "m",
        Some('L') => "cycles",
        Some('D') => "Hz",
        Some('S') => "dB-Hz",
        _ => "",
    }
}
