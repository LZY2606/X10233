//! 领域模型基础量:单位、频率、参数指纹、弧段身份。
//! 所有领域量都携带单位;时间戳携带时标基准(原始字符串保留)。

use serde::{Deserialize, Serialize};

pub const C_M_PER_S: f64 = 299_792_458.0;
pub const ALGO_VER: &str = "cbd-1.0.0";

pub fn freq_hz(freq: i64) -> Option<f64> {
    match freq {
        1 => Some(1575.42e6),
        2 => Some(1227.60e6),
        5 => Some(1176.45e6),
        _ => None,
    }
}

pub fn wavelength_m(freq: i64) -> Option<f64> {
    freq_hz(freq).map(|f| C_M_PER_S / f)
}

pub fn widelane_lambda_m() -> f64 {
    C_M_PER_S / (1575.42e6 - 1227.60e6)
}

/// 检测参数。任何修订产生新 rev;参数指纹参与派生值身份。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub td_threshold_cycles: f64,
    pub mw_threshold_cycles: f64,
    pub gf_threshold_m: f64,
    pub clock_threshold_cycles: f64,
    pub clock_min_sats: i64,
    pub max_gap_sec: f64,
    pub ambig_max_n: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            td_threshold_cycles: 0.8,
            mw_threshold_cycles: 0.5,
            gf_threshold_m: 0.15,
            clock_threshold_cycles: 1.0,
            clock_min_sats: 3,
            max_gap_sec: 120.0,
            ambig_max_n: 3,
        }
    }
}

pub fn fnv_hex(data: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

pub fn params_fingerprint(p: &Params) -> String {
    fnv_hex(&serde_json::to_string(p).unwrap_or_default())
}

/// 弧段身份: 站|星|频率编号
pub fn arc_key(station: &str, sat: &str, freq: i64) -> String {
    format!("{}|{}|{}", station, sat, freq)
}

pub fn pair_key(station: &str, sat: &str) -> String {
    format!("{}|{}", station, sat)
}

pub fn split_arc(key: &str) -> Option<(String, String, i64)> {
    let parts: Vec<&str> = key.split('|').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].parse().ok()?,
    ))
}

/// 观测码单位。无法识别的码在导入层附诊断,不丢弃。
pub fn unit_for_code(code: &str) -> &'static str {
    match code.chars().next() {
        Some('C') => "m",
        Some('L') => "cycles",
        Some('D') => "Hz",
        Some('S') => "dB-Hz",
        _ => "unknown",
    }
}

pub fn code_freq(code: &str) -> Option<i64> {
    code.chars().nth(1).and_then(|c| c.to_digit(10)).map(|d| d as i64)
}

pub fn code_kind(code: &str) -> Option<char> {
    code.chars().next()
}
