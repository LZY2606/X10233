//! 领域模型：GNSS 载波相位 / 周跳候选 / 弧段修订史。
//! 所有领域量携带单位或坐标基准；原始表示在导入层保留，派生层才规范化。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 虚拟时钟：测试注入固定时间，服务器用系统时间。单位：毫秒（逻辑时间戳）。
pub type VirtualMs = i64;

/// 观测码（RINEX 风格）。频率编号与观测码均按历元原样保存。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange, // 伪距，单位 m
    Carrier,     // 载波相位，单位 cycle
    Doppler,     // Doppler，单位 Hz
    Snr,         // 信噪比，单位 dB-Hz
}

/// 一条按历元原样保存的观测值（导入层，不可变）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObs {
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64, // GPS 时标，毫秒；坐标基准字段见 time_scale
    pub code: String,  // 观测码原样，如 "L1C" / "C2W" / "D1C" / "S2W"
    pub kind: ObsKind,
    pub freq_no: u8,   // 频率编号 1/2
    pub raw_value: String, // 原始文本表示，原样保留
    pub lli: u8,       // LLI 标志按历元原样保存
    pub norm_value: Option<f64>, // 派生层规范化值；无法转换为 None
    pub norm_unit: Option<String>, // 规范化单位
    pub diag: Option<String>,    // 无法转换时的诊断，值留在原位置
}

/// 整机钟跳事件（导入层原样保存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub station: String,
    pub epoch_ms: i64,
    pub kind: String,      // "receiver_clock_jump"
    pub raw_value: String, // 原始表示，如 "+1ms"
    pub offset_ms: Option<f64>,
    pub diag: Option<String>,
}

/// 源数据版本：原始输入一经提交不可覆盖，修订以新版本+父版本表达。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceVersion {
    pub id: String,
    pub version_no: i64,
    pub parent_id: Option<String>,
    pub content_hash: String,
    pub note: String,
    pub created_seq: i64,
    pub vtime: VirtualMs,
}

/// 规则版本：算法版本 + 参数指纹。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleVersion {
    pub id: String,
    pub algo_version: String,
    pub params: DetectParams,
    pub fingerprint: String,
    pub parent_id: Option<String>,
    pub created_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectParams {
    pub iono_threshold_m: f64,     // 电离层组合历元差阈值，单位 m
    pub mw_threshold_cycle: f64,   // Melbourne-Wubbena 离群阈值，单位 宽巷周
    pub td_threshold_cycle: f64,   // 时间差分残差阈值，单位 周
    pub clock_jump_min_sats: usize, // 判定整机钟跳所需的同时跳变卫星数
    pub clock_jump_tol_cycle: f64, // 钟跳整周一致性容差，单位 周
}

impl Default for DetectParams {
    fn default() -> Self {
        Self {
            iono_threshold_m: 0.20,
            mw_threshold_cycle: 0.6,
            td_threshold_cycle: 0.5,
            clock_jump_min_sats: 2,
            clock_jump_tol_cycle: 0.15,
        }
    }
}

impl RuleVersion {
    pub fn fingerprint_of(algo: &str, params: &DetectParams) -> String {
        let s = format!("{}|{}", algo, serde_json::to_string(params).unwrap());
        hex_sha256(s.as_bytes())
    }
}

/// 弧段：站-星-频率的连续观测段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub freq_no: u8,
    pub start_epoch_ms: i64,
    pub end_epoch_ms: i64,
    pub source_version_id: String,
    pub rule_version_id: String,
    pub stale: bool,
    pub seq: i64,
}

/// 检测器标识。任一检测器都不作为真值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detector {
    IonosphereCombo,  // 电离层（几何无关）组合
    MelbourneWubbena, // Melbourne-Wubbena 组合
    TimeDifference,   // 时间差分（载波-Doppler 预报）
}

impl Detector {
    pub fn as_str(&self) -> &'static str {
        match self {
            Detector::IonosphereCombo => "iono",
            Detector::MelbourneWubbena => "mw",
            Detector::TimeDifference => "td",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "iono" => Some(Detector::IonosphereCombo),
            "mw" => Some(Detector::MelbourneWubbena),
            "td" => Some(Detector::TimeDifference),
            _ => None,
        }
    }
}

/// 候选状态：候选、拒绝原因、误差与未知状态都是一等数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Open,       // 待审
    Accepted,   // 被采纳（由裁定事件驱动，仅存偏移层）
    Rejected,   // 被否定，拒绝原因必填
    Ambiguous,  // 多个等分整周偏移，无法唯一决定，保留全部合法候选
    Unavailable, // 输入缺失（如频率缺失）导致该检测器无法给出结论
    Error,      // 计算错误，附诊断
}

impl CandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CandidateStatus::Open => "open",
            CandidateStatus::Accepted => "accepted",
            CandidateStatus::Rejected => "rejected",
            CandidateStatus::Ambiguous => "ambiguous",
            CandidateStatus::Unavailable => "unavailable",
            CandidateStatus::Error => "error",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "accepted" => CandidateStatus::Accepted,
            "rejected" => CandidateStatus::Rejected,
            "ambiguous" => CandidateStatus::Ambiguous,
            "unavailable" => CandidateStatus::Unavailable,
            "error" => CandidateStatus::Error,
            _ => CandidateStatus::Open,
        }
    }
}

/// 周跳/钟跳候选。钟跳与单星周跳分开存放。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub arc_key: String, // station|sat|freq（freq=0 表示整机钟跳，跨频率）
    pub station: String,
    pub sat: String,
    pub freq_no: u8,
    pub epoch_ms: i64,
    pub detector: Detector,
    pub kind: String, // "cycle_slip" | "clock_jump"
    pub magnitude_cycles: Option<f64>, // 估计跳变量，单位 周；未知为 None
    pub score: Option<f64>,            // 候选得分；未知为 None
    pub status: CandidateStatus,
    pub rejection_reason: Option<String>,
    pub error: Option<String>,
    pub alternatives: Vec<(i32, i32)>, // 多个等分整周偏移的合法 (N1,N2) 候选
    pub basis: serde_json::Value,      // 机器可读取的判断依据
    pub source_version_id: String,
    pub rule_version_id: String,
    pub stale: bool,
    pub seq: i64,
}

/// 修复：只存偏移层，不改原始观测。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOffset {
    pub id: String,
    pub arc_key: String,
    pub station: String,
    pub sat: String,
    pub freq_no: u8,
    pub from_epoch_ms: i64, // 自该历元起生效
    pub offset_cycles: f64, // 偏移量，单位 周
    pub decision_id: String,
    pub branch_id: i64,
    pub seq: i64,
}

/// 人工裁定：接受 / 否定 / 参数调整。可分叉重算。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub candidate_id: String,
    pub action: String, // "accept" | "reject" | "adjust"
    pub payload: serde_json::Value,
    pub actor: String,
    pub branch_id: i64,
    pub supersedes: Option<String>,
    pub seq: i64,
    pub vtime: VirtualMs,
}

/// 事件：所有判断依据写成机器可读取的事件，构成可重放历史。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub branch_id: i64,
    pub id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub vtime: VirtualMs,
}

/// 分支：人工裁定可分叉重算；回到任意旧事件可恢复当时依赖版本与可见结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub fork_seq: i64, // 从父分支的哪个事件序号分叉
    pub label: String,
}

/// 组合残差点（派生层，带单位）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualPoint {
    pub arc_key: String,
    pub combo: String, // "iono_gf_m" | "mw_cycle" | "td_cycle"
    pub epoch_ms: i64,
    pub value: Option<f64>, // 频率缺失等情况下为 None
    pub unit: String,
    pub stale: bool,
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

// 避免引入 hex crate，内联一个最小实现。
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        const T: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(bytes.as_ref().len() * 2);
        for b in bytes.as_ref() {
            s.push(T[(b >> 4) as usize] as char);
            s.push(T[(b & 0xf) as usize] as char);
        }
        s
    }
}

pub fn arc_key(station: &str, sat: &str, freq_no: u8) -> String {
    format!("{}|{}|L{}", station, sat, freq_no)
}
