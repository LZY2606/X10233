//! 领域模型:GNSS 观测、弧段、候选、偏移层、事件。
//! 所有领域量都携带单位;原始表示在导入层保留,派生层才做规范化。

use serde::{Deserialize, Serialize};

pub const ALGO_NORM: &str = "norm/1";
pub const ALGO_ARC: &str = "arcseg/1";
pub const ALGO_GF: &str = "gfdet/1";   // 电离层(几何无关)组合
pub const ALGO_MW: &str = "mwdet/1";   // Melbourne-Wubbena
pub const ALGO_TD: &str = "tddet/1";   // 时间差分(含钟跳判别)
pub const ALGO_LLI: &str = "llidet/1"; // LLI 标志直读

pub const F1_HZ: f64 = 1575.42e6;
pub const F2_HZ: f64 = 1227.60e6;
pub const C_MPS: f64 = 299_792_458.0;

pub fn lambda_m(freq: u8) -> Option<f64> {
    match freq {
        1 => Some(C_MPS / F1_HZ),
        2 => Some(C_MPS / F2_HZ),
        _ => None,
    }
}

/// 虚拟时间:基准时刻 + 逻辑序号秒。全部测试/运行确定性。
pub const VIRTUAL_BASE_MS: i64 = 1_789_516_800_000; // 2026-09-19T00:00:00Z
pub fn virtual_time_ms(seq: i64) -> i64 {
    VIRTUAL_BASE_MS + seq * 1000
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub max_gap_s: f64,          // 弧段断裂间隔阈值
    pub gf_threshold_m: f64,     // 电离层组合跳变阈值(米)
    pub mw_threshold_m: f64,     // MW 组合跳变阈值(米)
    pub td_threshold_cyc: f64,   // 时间差分残差阈值(周)
    pub clock_common_ratio: f64, // 判定整机钟跳所需的共模卫星比例
    pub max_hyp: i32,            // 候选假设搜索的最大整周幅度
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 10.0,
            gf_threshold_m: 0.10,
            mw_threshold_m: 1.20,
            td_threshold_cyc: 0.50,
            clock_common_ratio: 0.5,
            max_hyp: 8,
        }
    }
}

/// 参数指纹:FNV-1a,确定性。
pub fn fingerprint_of(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

impl Params {
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&serde_json::to_string(self).unwrap())
    }
}

/// 规范化后的单条观测(派生层)。raw 永远保留;无法转换时 value=None 并附 diag。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormValue {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diag: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormObs {
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub freq: u8,
    pub pr: Option<NormValue>,  // 伪距, 单位 m
    pub ph: Option<NormValue>,  // 载波, 单位 cyc
    pub dp: Option<NormValue>,  // Doppler, 单位 Hz
    pub lli: u8,
    pub snr: Option<NormValue>, // 信噪比, 单位 dB-Hz
    pub channel: Option<u32>,   // 频率编号(通道)
    pub pr_code: Option<String>,// 观测码,原样
    pub ph_code: Option<String>,
    pub source_line: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    pub epoch_ms: i64,
    pub elev_deg: Option<f64>,
    pub health: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockEvent {
    pub epoch_ms: i64,
    pub jump_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub freq: u8,
    pub start_ms: i64,
    pub end_ms: i64,
    pub epochs: Vec<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CandKind {
    Slip,       // 单星周跳
    ClockJump,  // 整机钟跳(与单星周跳分开)
    LliFlag,    // LLI 报告(可能误报)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hypothesis {
    /// freq -> 整周偏移
    pub offsets: std::collections::BTreeMap<u8, i32>,
    pub resid: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub kind: CandKind,
    pub detector: String, // 算法版本;任一检测器都不当真值
    pub station: String,
    pub sat: String,      // ClockJump 时为 "*"(整机)
    pub epoch_ms: i64,
    pub arc_ids: Vec<String>,
    pub hyps: Vec<Hypothesis>, // 无法唯一决定时保留全部合法候选
    pub score: f64,
    pub status: String,   // proposed / rejected(auto) ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    pub source_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OffsetLayer {
    pub id: String,
    pub arc_id: String,
    pub from_epoch_ms: i64,
    pub offsets: std::collections::BTreeMap<u8, i32>,
    pub by_event: String,
}
