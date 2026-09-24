//! 领域模型：RINEX 观测历元、卫星状态、弧段与事件。
//! 原则：伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号和观测码
//! 均按历元原样保存；派生层才做规范化。

use serde::{Deserialize, Serialize};

pub const C: f64 = 299_792_458.0;
pub const F1_HZ: f64 = 1575.42e6;
pub const F2_HZ: f64 = 1227.60e6;

/// 频率编号 -> 载波波长（米）。未知频率返回 None，调用方保留原值并附诊断。
pub fn wavelength(freq: i64) -> Option<f64> {
    match freq {
        1 => Some(C / F1_HZ),
        2 => Some(C / F2_HZ),
        _ => None,
    }
}

pub fn freq_hz(freq: i64) -> Option<f64> {
    match freq {
        1 => Some(F1_HZ),
        2 => Some(F2_HZ),
        _ => None,
    }
}

/// 一个（历元, 卫星, 频率）上的原始观测行。所有字段保留导入时的原样表示；
/// 无法转换的字段留 None 并在 diag 中附诊断，raw 保存原始行。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Obs {
    pub t: f64, // 日内秒（GPS 时标，由导入文本的历元头解析）
    pub station: String,
    pub sat: String,
    pub freq: i64, // 频率编号（1=L1, 2=L2）
    pub code_p: Option<String>, // 伪距观测码，如 C1C
    pub pseudo: Option<f64>,    // 伪距，单位：米
    pub code_l: Option<String>, // 载波观测码，如 L1C
    pub carrier: Option<f64>,   // 载波相位，单位：周
    pub lli: Option<i64>,       // LLI 失锁标志（原样）
    pub snr: Option<f64>,       // 信噪比，单位：dB-Hz
    pub code_d: Option<String>, // Doppler 观测码
    pub doppler: Option<f64>,   // Doppler，单位：周/秒
    pub raw: String,            // 原始行文本
    pub diag: Option<String>,   // 解析诊断
}

/// 钟跳事件（接收机日志先验，按历元原样保存）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockEvent {
    pub t: f64,
    pub offset_s: f64, // 单位：秒
    pub note: String,
    pub raw: String,
}

/// 无法解析的输入行：保留在原位置并附诊断。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reject {
    pub line_no: usize,
    pub raw: String,
    pub diag: String,
}

/// 解析历元时间 "2026-09-19T00:01:40.000Z" -> 日内秒。
pub fn parse_epoch_time(s: &str) -> Option<f64> {
    let tpos = s.find('T')?;
    let t = tpos;
    let t = &s[t + 1..];
    let t = t.trim_end_matches('Z');
    let mut parts = t.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

pub fn fmt_t(t: f64) -> String {
    let ti = t.floor() as i64;
    let h = (ti / 3600) % 24;
    let m = (ti / 60) % 60;
    let s = ti % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// 稳定身份：为每个显示值生成可回溯源头的标识。
pub fn obs_value_id(source_id: &str, idx: usize) -> String {
    format!("obs:{source_id}:{idx}")
}
pub fn combo_value_id(combo: &str, station: &str, sat: &str, t: f64) -> String {
    format!("cmb:{combo}:{station}:{sat}:{:.3}", t)
}
