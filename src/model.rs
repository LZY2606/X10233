//! 领域模型：所有领域量携带单位或坐标基准；原始表示与规范化值分离。

use serde::{Deserialize, Serialize};

/// 观测值单位。无法识别的单位保留为 `Unknown`，值留在原位置并附诊断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meter,   // 伪距 C*
    Cycle,   // 载波相位 L*
    Hertz,   // Doppler D*
    DbHz,    // 信噪比 S*
    Flags,   // LLI
    Unknown, // 无法转换：留在原位置并附诊断
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cyc",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Flags => "flags",
            Unit::Unknown => "?",
        }
    }
    pub fn from_code(code: &str) -> Unit {
        match code.chars().next() {
            Some('C') => Unit::Meter,
            Some('L') => Unit::Cycle,
            Some('D') => Unit::Hertz,
            Some('S') => Unit::DbHz,
            Some(_) if code == "LLI" => Unit::Flags,
            _ => Unit::Unknown,
        }
    }
}

/// 一条原始观测值：raw 永远保留，parsed 是派生的规范化视图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub input_id: String,
    pub epoch_ms: i64,
    pub epoch_raw: String,
    pub sat: String,
    pub freq: u8,
    pub code: String,
    pub raw_value: String,
    pub value: Option<f64>,
    pub unit: Unit,
    pub diag: Option<String>,
}

/// 接收机钟跳事件（按历元原样保存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: i64,
    pub input_id: String,
    pub epoch_ms: i64,
    pub raw_offset: String,
    pub offset_s: Option<f64>,
    pub diag: Option<String>,
}

/// 检测参数版本。修订以新版本 + 父版本表达。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSet {
    pub max_gap_s: f64,          // 弧段断裂的最大历元间隔
    pub iono_threshold_cyc: f64, // 电离层(几何无关)组合阈值
    pub mw_threshold_cyc: f64,   // Melbourne-Wübbena 阈值
    pub td_threshold_cyc: f64,   // 时间差分阈值
    pub clock_common_ratio: f64, // 同历元共同跳变占比达到则判整机钟跳
    pub ambiguity_tol_cyc: f64,  // 等分整周歧义容差
}

impl Default for ParamSet {
    fn default() -> Self {
        ParamSet {
            max_gap_s: 45.0,
            iono_threshold_cyc: 0.4,
            mw_threshold_cyc: 0.8,
            td_threshold_cyc: 0.5,
            clock_common_ratio: 0.6,
            ambiguity_tol_cyc: 0.05,
        }
    }
}

/// 弧段：同一 (站, 星, 频率) 的连续观测段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub freq: u8,
    pub start_ms: i64,
    pub end_ms: i64,
    pub epoch_count: u32,
    pub stale: bool,
}

/// 候选状态机：proposed → accepted / rejected；组内互斥时同组其余自动 rejected。
/// 无法唯一决定时保留全部合法候选（status=proposed, group_id 相同）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub run_id: String,
    pub arc_id: String,
    pub detector: String, // "iono-gf" | "mw" | "td" —— 任一检测器都不当真值
    pub epoch_ms: i64,
    pub freq: u8,
    pub est_cycles: Option<f64>,
    pub score: f64,
    pub status: String, // proposed | accepted | rejected | unknown
    pub reason: Option<String>,
    pub group_id: Option<String>,
    pub stale: bool,
}

/// 修复只存偏移层：原始观测永不改写。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repair {
    pub id: String,
    pub branch_id: String,
    pub arc_id: String,
    pub from_ms: i64,
    pub freq: u8,
    pub offset_cycles: f64,
    pub decision_event_seq: i64,
}

/// 事件：机器可读取的判断依据。一切人工决定、导入、修订、撤销、分叉都是事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub branch_id: String,
    pub seq: i64, // 分支内单调序号
    pub kind: String,
    pub payload: serde_json::Value,
    pub vtime: String, // 虚拟时间（测试可注入）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent_branch: Option<String>,
    pub fork_event_seq: Option<i64>,
    pub head_seq: i64,
}

/// 一次重算：记录算法版本、参数指纹与直接来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub branch_id: String,
    pub algo_version: String,
    pub param_id: String,
    pub param_fp: String,
    pub input_id: String,
    pub input_fp: String,
    pub decision_fp: String,
    pub created_seq: i64,
    pub superseded: bool,
}
