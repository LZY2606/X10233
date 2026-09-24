//! 按历元原样保存的原始观测与卫星状态 / 钟跳事件。

use crate::domain::signal::ObsCode;
use crate::domain::units::Unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 一个 RINEX 字段（14/16 列宽）。原始文本与解析值同时保留；
/// 无法转换时 `value=None`，诊断留在 `diagnostics`，绝不丢弃。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawField {
    pub code: String,
    pub raw: String,
    pub value: Option<f64>,
    pub lli: Option<u8>,
    pub signal_strength: Option<u8>,
    pub unit: Unit,
    pub diagnostics: Vec<String>,
}

impl RawField {
    pub fn has_observation(&self) -> bool {
        self.value.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrequencyObs {
    pub band: u8,
    pub attr: char,
    /// key 为完整观测码，例如 "C1C"/"L1C"/"D1C"/"S1C"。
    pub fields: BTreeMap<String, RawField>,
}

impl FrequencyObs {
    pub fn field(&self, prefix: char) -> Option<&RawField> {
        let key = format!("{prefix}{}{}", self.band, self.attr);
        self.fields.get(&key)
    }
    pub fn range_m(&self) -> Option<f64> {
        self.field('C').or_else(|| self.field('P')).and_then(|f| f.value)
    }
    pub fn phase_cycles(&self) -> Option<f64> {
        self.field('L').and_then(|f| f.value)
    }
    pub fn doppler_hz(&self) -> Option<f64> {
        self.field('D').and_then(|f| f.value)
    }
    pub fn snr_db(&self) -> Option<f64> {
        self.field('S').and_then(|f| f.value)
    }
    pub fn lli(&self) -> Option<u8> {
        self.field('L').and_then(|f| f.lli)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpochObs {
    pub station: String,
    pub epoch_raw: String,
    pub epoch_ms: i64,
    pub epoch_flag: u8,
    pub receiver_clock_offset_raw: Option<String>,
    pub receiver_clock_offset_s: Option<f64>,
    /// sat id ("G09") -> band -> 频率观测
    pub sats: BTreeMap<String, FrequencyByBand>,
}

pub type FrequencyByBand = BTreeMap<u8, FrequencyObs>;

/// 接收机钟跳事件（整机）。来自 .clock 事件文件或解析器检测，按原文保存。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    pub raw: String,
    pub epoch_ms: i64,
    pub event: String,        // CLOCK_JUMP
    pub delta_s: Option<f64>, // 秒；无法解析时为 None
    pub note: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatState {
    pub epoch_ms: i64,
    pub sat: String,
    pub x_m: Option<f64>,
    pub y_m: Option<f64>,
    pub z_m: Option<f64>,
    pub vx_m_s: Option<f64>,
    pub vy_m_s: Option<f64>,
    pub vz_m_s: Option<f64>,
    pub clock_s: Option<f64>,
    pub frame: String,
    pub raw: String,
    pub diagnostics: Vec<String>,
}

/// 解析阶段收集的格式诊断（行号 + 严重度 + 原文 + 说明）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub line_no: usize,
    pub severity: String, // ERROR | WARN
    pub raw: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedProject {
    pub station: String,
    pub receiver_type: String,
    pub marker_name: String,
    pub glo_freq_numbers: BTreeMap<String, i32>,
    pub declared_codes: Vec<String>,
    pub epochs: Vec<EpochObs>,
    pub clock_events: Vec<ClockEvent>,
    pub sat_states: Vec<SatState>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub _codes: Vec<ObsCode>,
}
