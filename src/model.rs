//! 领域模型。原始层一律保留原始表示（字符串），派生层才携带数值与单位。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub line_no: Option<u32>,
    pub message: String,
}

/// 频率定义：频率编号（band）+ 观测码，按历元原样保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreqDef {
    pub constellation: String,
    pub band: String,
    pub code: String,
    /// 原始频率文本，例如 "1575420000"
    pub freq_raw: String,
    /// Hz；无法转换时为 None 并附诊断
    pub freq_hz: Option<f64>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationDef {
    pub station: String,
    pub marker_name: String,
    pub coord: Option<[f64; 3]>,
    pub datum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub station: String,
    pub epoch: f64,
    /// 原始历元文本
    pub epoch_raw: String,
    pub jump_ns_raw: String,
    pub jump_ns: Option<f64>,
    pub source: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    pub epoch: f64,
    pub epoch_raw: String,
    pub x_raw: String,
    pub y_raw: String,
    pub z_raw: String,
    pub clock_raw: String,
    pub x_m: Option<f64>,
    pub y_m: Option<f64>,
    pub z_m: Option<f64>,
    pub clock_s: Option<f64>,
    pub datum: String,
}

/// 单站单星单频单历元的原始观测。每个观测量都保留原始 token。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub station: String,
    pub sat: String,
    pub epoch: f64,
    pub epoch_raw: String,
    pub band: String,
    pub code: String,
    pub pseudorange_raw: Option<String>,
    pub carrier_raw: Option<String>,
    pub doppler_raw: Option<String>,
    pub lli_raw: Option<String>,
    pub snr_raw: Option<String>,
    pub pseudorange_m: Option<f64>,
    pub carrier_cycles: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub lli_flag: Option<i64>,
    pub snr_db_hz: Option<f64>,
    /// 原始 token 的物理单位，导入时按观测码记录、不做隐式换算
    pub units: RawUnits,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RawUnits {
    pub pseudorange: String,
    pub carrier: String,
    pub doppler: String,
    pub lli: String,
    pub snr: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedBundle {
    pub format: String,
    pub format_version: String,
    pub case_id: String,
    pub title: String,
    pub seed: u64,
    pub stations: Vec<StationDef>,
    pub freqs: Vec<FreqDef>,
    pub observations: Vec<RawObservation>,
    pub clock_events: Vec<ClockEvent>,
    pub sat_states: Vec<SatState>,
    pub diagnostics: Vec<Diagnostic>,
    /// 仅用于补丁：替换 (station,sat,epoch,band) 相同的观测
    pub replaces: bool,
}

/// 不可变事件（审计链）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub event_id: String,
    pub seq: i64,
    pub branch: String,
    pub parent_event_id: Option<String>,
    pub kind: String,
    pub virtual_ts: String,
    pub summary: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchRecord {
    pub name: String,
    pub forked_from_event: Option<String>,
    pub created_seq: i64,
}

/// 派生行的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowState {
    Current,
    Stale,
    Rejected,
    Accepted,
    Ambiguous,
    Unknown,
}

impl RowState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RowState::Current => "current",
            RowState::Stale => "stale",
            RowState::Rejected => "rejected",
            RowState::Accepted => "accepted",
            RowState::Ambiguous => "ambiguous",
            RowState::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> RowState {
        match s {
            "stale" => RowState::Stale,
            "rejected" => RowState::Rejected,
            "accepted" => RowState::Accepted,
            "ambiguous" => RowState::Ambiguous,
            "unknown" => RowState::Unknown,
            _ => RowState::Current,
        }
    }
}
