//! 领域数据类型、单位/坐标基准与常量。
//!
//! 约定：原始观测值一经导入即原样保存（字符串令牌留在 raw_line / *_raw 字段），
//! 规范化后的 f64 只存在于派生计算中。所有领域量在 JSON 中携带单位或基准。

use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: &str = "GBSV1";
pub const BUNDLE_VERSION: &str = "gbs-bundle/1";
pub const ALGO_VERSION: &str = "gbs-cycle-slip-algo/1.0.0";
/// 真空光速（m/s），用于钟跳的码相位换算。
pub const C_LIGHT: f64 = 299_792_458.0;
/// 历元偏移默认坐标基准（秒，相对参考历元的 TT 标量，仅用于排序与时间差）。
pub const DEFAULT_REFERENCE: &str = "2026-01-01T00:00:00Z";
pub const EPOCH_DATUM: &str = "SECONDS_SINCE_REFERENCE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "WARNING")]
    Warning,
}

/// 导入诊断：格式错误导致整批拒收；无法转换的值以 WARNING 留存在原位置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub line_no: i64,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

/// 频率定义（频率编号 + 观测码唯一定位一个相位/码信号）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandDef {
    pub system: String,
    /// RINEX 频率编号，例如 "L1"、"L2"、"L5"。
    pub freq_num: String,
    /// RINEX 观测码，例如 "1C"、"2W"、"5X"。
    pub code: String,
    /// 标称频率（Hz）。
    pub freq_hz: f64,
    /// 波长（m）。
    pub wavelength_m: f64,
}

impl BandDef {
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.system, self.freq_num, self.code)
    }

    pub fn gps_bands() -> Vec<BandDef> {
        let mk = |num: &str, code: &str, f: f64| BandDef {
            system: "G".to_string(),
            freq_num: num.to_string(),
            code: code.to_string(),
            freq_hz: f,
            wavelength_m: C_LIGHT / f,
        };
        vec![
            mk("L1", "1C", 1_575_420_000.0),
            mk("L2", "2W", 1_227_600_000.0),
            mk("L5", "5X", 1_176_450_000.0),
        ]
    }

    pub fn lookup(system: &str, freq_num: &str, code: &str) -> Option<BandDef> {
        // 当前离线案例包只使用 GPS；表结构允许后续扩展其他系统。
        if system == "G" {
            Self::gps_bands()
                .into_iter()
                .find(|b| b.freq_num == freq_num && b.code == code)
        } else {
            None
        }
    }
}

/// 单条原始观测历元记录（站-星-频率）。
///
/// 数值字段为 None 有两种含义：缺测（`*`，正常）与无法转换（非法令牌，附带诊断）。
/// 无论哪种，raw_line 都按历元原样保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub ordinal: i64,
    pub station: String,
    pub sat: String,
    pub system: String,
    pub freq_num: String,
    pub code: String,
    /// 历元原始令牌。
    pub epoch_tag: String,
    /// 相对参考历元的偏移（秒）；无法转换时为 None。
    pub epoch_offset_s: Option<f64>,
    pub epoch_datum: String,
    pub reference_epoch: String,
    pub pseudorange_m: Option<f64>,
    pub pseudorange_raw: String,
    pub phase_cycles: Option<f64>,
    pub phase_raw: String,
    pub doppler_hz: Option<f64>,
    pub doppler_raw: String,
    pub lli: Option<i64>,
    pub lli_raw: String,
    pub snr_db_hz: Option<f64>,
    pub snr_raw: String,
    pub raw_line: String,
}

/// 卫星状态行（健康状态等按历元原样保存；坐标令牌不参与周跳计算，仅留存展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSatState {
    pub ordinal: i64,
    pub sat: String,
    pub epoch_tag: String,
    pub epoch_offset_s: Option<f64>,
    pub epoch_datum: String,
    pub reference_epoch: String,
    pub health_raw: String,
    pub tokens: Vec<String>,
    pub raw_line: String,
}

/// 整机钟跳事件记录（来自文件中的显式 CLOCK 行；检测结果在派生层另行生成）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEvent {
    pub ordinal: i64,
    pub applies_to: String,
    pub epoch_tag: String,
    pub epoch_offset_s: Option<f64>,
    pub epoch_datum: String,
    pub reference_epoch: String,
    pub jump_ms_raw: String,
    pub jump_ms: Option<f64>,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedBatch {
    pub format_version: String,
    pub reference_epoch: String,
    pub observations: Vec<RawObservation>,
    pub sat_states: Vec<RawSatState>,
    pub clock_events: Vec<RawClockEvent>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

impl ParsedBatch {
    pub fn acceptable(&self) -> bool {
        !self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
}

/// 检测器/裁定使用的参数（指纹参与派生结果身份）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectParams {
    /// 电离层（geometry-free）组合容差，单位：L1 周。
    pub gf_tol_cycles: f64,
    /// Melbourne-Wubbena 宽巷容差（周）。
    pub mw_tol_cycles: f64,
    /// 时间差分/Doppler 约束容差（周）。
    pub td_tol_cycles: f64,
    /// 超过该间隔（秒，严格大于）自动断开弧段。
    pub gap_split_seconds: f64,
    /// 判定整机钟跳所需的最小卫星数。
    pub clock_min_sats: i64,
    /// 判定整机钟跳所需的同向跳变卫星占比。
    pub clock_consensus_fraction: f64,
    /// 整周假设搜索半径（周，每个频率维 ±该值）。
    pub search_half_window: i64,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gf_tol_cycles: 0.15,
            mw_tol_cycles: 0.5,
            td_tol_cycles: 0.30,
            gap_split_seconds: 60.0,
            clock_min_sats: 3,
            clock_consensus_fraction: 0.75,
            search_half_window: 2,
        }
    }
}
