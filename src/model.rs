//! 领域基础模型：原始量、导入、观测历元、卫星状态。
//!
//! 设计原则（对应需求）：
//! * 原始输入一经提交不可覆盖：`RawImport` 只追加，后续修订以新版本与父版本关系表达。
//! * 所有领域量携带单位/坐标基准：`Measure { value, unit }`。
//! * 无法转换的值留在原位置并附诊断：`RawValue` 同时保存 `raw_repr` 与解析结果。

use serde::{Deserialize, Serialize};

/// 测量值：数值 + 单位/基准。原始层只做“原样保存 + 尝试换算”，规范化只发生在派生层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub value: f64,
    pub unit: String,
}

impl Measure {
    pub fn new(value: f64, unit: impl Into<String>) -> Self {
        Self { value, unit: unit.into() }
    }
}

/// 原始观测分量（单一信号上的一个量），保留原始文本表示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawValue {
    /// 原始文本，例如 "20463172.987"、"12.3"、"oops"。永不改写。
    pub raw_repr: String,
    /// 声明单位（单位后缀，缺省由量纲决定），原样保存。
    pub unit_hint: String,
    /// 成功换算到规范单位后的数值（伪距 m / 相位 cycle / Doppler Hz / SNR dBHz）。
    pub normalized: Option<f64>,
    /// 无法转换时留在原位置的诊断；None 表示健康。
    pub diagnostic: Option<String>,
}

/// 一条站-星-信号（频点）上的历元观测。缺测分量为 None（频率缺失情形）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RawObservation {
    pub pseudo_range: Option<RawValue>,
    pub carrier_phase: Option<RawValue>,
    pub doppler: Option<RawValue>,
    pub snr: Option<RawValue>,
    /// RINEX LLI 标志（0/1/2…），原样整数保存；None 表示该历元未给标志。
    pub lli: Option<u8>,
}

/// 卫星状态（历元原样保存，坐标基准为高度角/方位角）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SatState {
    pub elevation: Option<Measure>,
    pub azimuth: Option<Measure>,
    pub health: Option<String>,
}

/// 整机钟跳事件（历元原样保存）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    /// 发生位置：在第 `between_epoch` 个历元与前一历元之间（0 基，0 表示首历元）。
    pub between_epoch: usize,
    pub jump: Measure,
    pub source: String,
}

/// 一次原始导入：不可变快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawImport {
    /// 稳定身份，形如 `imp_<短哈希>`，由内容决定（导出/删除/再导入后不变）。
    pub id: String,
    /// 版本号（从 1 起）。
    pub version: u32,
    /// 父版本导入 id；首个版本为 None。
    pub parent_id: Option<String>,
    /// 修订原因；首版为 None。
    pub revision_note: Option<String>,
    /// 虚拟提交时间（固定 seed 时间线，测试可复现）。
    pub committed_at: String,
    /// 站点标识，原样保存。
    pub station: String,
    /// 历元时间戳（虚拟时间，ISO8601），允许不等间隔。
    pub epoch_times: Vec<String>,
    /// 历元间隔（秒），由时间戳差得到；None 表示首历元无前向间隔。
    pub dt_seconds: Vec<Option<f64>>,
    /// observations[epoch_index][satellite][freq_number]
    pub observations: Vec<std::collections::BTreeMap<String, std::collections::BTreeMap<u8, RawObservation>>>,
    /// sat_states[epoch_index][satellite]
    pub sat_states: Vec<std::collections::BTreeMap<String, SatState>>,
    pub clock_events: Vec<ClockEvent>,
    /// 导入时记录的解析诊断（不阻断导入：问题留在原位置）。
    pub diagnostics: Vec<String>,
    /// 规范化表：频率编号 -> (观测码, 频率 Hz)，来自头声明。
    pub freq_table: std::collections::BTreeMap<u8, FreqSpec>,
}

/// 频率编号对应的观测码与频率（坐标基准：频率 Hz；波长 m）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreqSpec {
    pub code: String,
    pub frequency_hz: f64,
}

impl FreqSpec {
    pub fn wavelength_m(&self) -> f64 {
        299_792_458.0 / self.frequency_hz
    }
}
