//! 领域核心类型：原始观测（原样保存）、派生量（带算法版本/参数指纹/直接来源）、
//! 周跳候选、修复偏移层、弧段、事件与稳定身份。
//!
//! 设计原则：
//! - 原始层（raw）一经提交不可覆盖；修订 = 新版本 + 父版本指针。
//! - 派生层（derived）永远携带 algo_version + params_fingerprint + direct_source_ids。
//! - 所有领域量携带单位或坐标基准；无法转换的值留在原位置并附诊断。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 算法版本目录。任何派生计算必须引用其中之一。
pub const ALGO_GF_IONO: &str = "gf-iono-v1";
pub const ALGO_MW: &str = "melbourne-wubbena-v1";
pub const ALGO_TDCP: &str = "tdcp-v1";
pub const ALGO_CLOCKJUMP: &str = "clockjump-v1";
pub const ALGO_ARC: &str = "arc-seg-v1";
pub const ALGO_SCORE: &str = "candidate-score-v1";

/// 稳定身份：对内容做 SHA-256，截断为 16 字节十六进制（32 字符）。
/// 同一内容在任何数据库、任何机器上得到同一身份 —— 导出/删除/重导入后身份不变。
pub fn stable_id(kind: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    for p in parts {
        h.update(b"\x1f");
        h.update(p.as_bytes());
    }
    let digest = h.finalize();
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// 参数指纹：对规范化后的参数 JSON 做稳定散列。
pub fn params_fingerprint(params: &serde_json::Value) -> String {
    let canon = canonical_json(params);
    stable_id("params", &[&canon])
}

fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", k, canonical_json(&m[*k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(a) => {
            let inner: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 原始层：按历元原样保存
// ---------------------------------------------------------------------------

/// 一个观测量的原始表示：原样文本 + 声明单位 + 可选规范化值。
/// 无法转换时 normalized=None 且附诊断；原值永不丢失。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Qty {
    pub raw: String,
    pub unit: String,
    pub normalized: Option<f64>,
    /// 规范化后的基准单位（如 "m"、"cycles"、"Hz"）。
    pub normalized_unit: Option<String>,
    pub diagnostic: Option<String>,
}

impl Qty {
    pub fn ok(raw: &str, unit: &str, normalized: f64, normalized_unit: &str) -> Self {
        Qty {
            raw: raw.to_string(),
            unit: unit.to_string(),
            normalized: Some(normalized),
            normalized_unit: Some(normalized_unit.to_string()),
            diagnostic: None,
        }
    }
    pub fn bad(raw: &str, unit: &str, diagnostic: &str) -> Self {
        Qty {
            raw: raw.to_string(),
            unit: unit.to_string(),
            normalized: None,
            normalized_unit: None,
            diagnostic: Some(diagnostic.to_string()),
        }
    }
}

/// 观测码（如 C1C / L1C / D1C / S1C）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObsCode {
    pub kind: char,   // C/L/D/S
    pub band: u8,     // 1/2/5...
    pub attr: String, // 如 "C"、"W"、""（RINEX 属性位原样保留）
}

impl ObsCode {
    pub fn parse(s: &str) -> Result<Self, String> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 2 {
            return Err(format!("观测码过短: {s}"));
        }
        let kind = chars[0];
        if !matches!(kind, 'C' | 'L' | 'D' | 'S') {
            return Err(format!("未知观测码类别: {s}"));
        }
        let band = chars[1]
            .to_digit(10)
            .ok_or_else(|| format!("观测码频点非法: {s}"))? as u8;
        let attr: String = chars[2..].iter().collect();
        Ok(ObsCode { kind, band, attr })
    }
    pub fn to_string(&self) -> String {
        format!("{}{}{}", self.kind, self.band, self.attr)
    }
}

/// 单条原始观测量（一个卫星-码）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawObs {
    pub code: ObsCode,
    pub value: Qty,
    /// LLI 原始两位文本（如 "1"、"0"），原样保存。
    pub lli: Option<String>,
    /// 信号强度原始位（0-9），原样保存。
    pub ssi: Option<String>,
}

/// 一个卫星在一个历元的全部原始观测量。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawSatObs {
    pub sat: String, // 如 "G01"
    pub obs: Vec<RawObs>,
}

/// 原始历元：标签原样保存；t_seconds 为派生规范化（带指纹）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEpoch {
    pub label: String,
    pub flag: String,
    pub clock_offset: Option<Qty>,
    pub sats: Vec<RawSatObs>,
}

/// 频率编号表（系统/频点 -> 载波频率 Hz）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreqSlot {
    pub system: char,
    pub band: u8,
    pub freq_hz: f64,
}

/// 原始批次（一次导入提交）。不可变。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBatch {
    pub batch_id: String,
    pub version: u32,
    pub parent_version: Option<u32>,
    pub format: String, // "obx/1"
    pub station: String,
    pub receiver_clock: String,
    pub coord_datum: String,
    pub freq_table: Vec<FreqSlot>,
    pub epochs: Vec<RawEpoch>,
    /// 显式钟跳事件（原始层事实，非派生）。
    pub clock_events: Vec<ClockEvent>,
    pub diagnostics: Vec<String>,
    pub submitted_at: String,
}

/// 钟跳事件（整机钟跳，原始事实层）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEvent {
    pub event_id: String,
    pub epoch_label: String,
    pub jump: Qty, // 秒
    pub scope: String, // "receiver"
    pub source: String, // 如 "receiver-log" / "operator"
}

// ---------------------------------------------------------------------------
// 派生层：弧段 / 候选 / 修复偏移
// ---------------------------------------------------------------------------

/// 弧段：同一 站-星-频率 的连续观测区间。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub arc_id: String,
    pub station: String,
    pub sat: String,
    pub band: u8,
    pub start_epoch_idx: usize,
    pub end_epoch_idx: usize,
    pub algo_version: String,
    pub params_fingerprint: String,
    pub direct_source_ids: Vec<String>,
    pub stale: bool,
}

/// 检测器给出的整周跳变（周）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlipPair {
    pub n1: i32,
    pub n2: i32,
}

/// 候选状态机。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
}

/// 周跳候选：一等数据。保留误差/未知状态与拒绝原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_id: String,
    pub arc_id: String,
    pub boundary_epoch_idx: usize,
    pub slip: SlipPair,
    /// 产生该候选的检测器及各自残差（周）。检测器之间可能给出不同候选，
    /// 不以任一检测器当真值。
    pub detector_evidence: Vec<DetectorEvidence>,
    pub score: f64,
    pub status: CandidateStatus,
    pub reject_reason: Option<String>,
    /// 误差或未知状态（如 "mw-unavailable"、"tdcp-gap"）。
    pub uncertainty: Vec<String>,
    pub algo_version: String,
    pub params_fingerprint: String,
    pub direct_source_ids: Vec<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorEvidence {
    pub detector: String, // ALGO_*
    pub residual_cycles: Option<f64>, // None = 未知/不可用
    pub supports: bool,
    pub note: String,
}

/// 修复偏移层：只存偏移，不改原始/规范化观测。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairOffset {
    pub repair_id: String,
    pub arc_id: String,
    pub boundary_epoch_idx: usize,
    pub slip: SlipPair,
    pub candidate_id: String,
    pub decision_event_id: String,
    pub active: bool,
    pub algo_version: String,
    pub params_fingerprint: String,
    pub direct_source_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// 事件日志（机器可读取的判断依据）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub seq: u64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

pub mod event_kind {
    pub const IMPORT_BATCH: &str = "import_batch";
    pub const PARAMS_REVISION: &str = "params_revision";
    pub const DETECT_RUN: &str = "detect_run";
    pub const DECISION: &str = "decision";
    pub const UNDO: &str = "undo";
    pub const CONFLICT_RESOLVE: &str = "conflict_resolve";
    pub const STALE_MARK: &str = "stale_mark";
    pub const FORK: &str = "fork";
    pub const EXPORT: &str = "export";
    pub const IMPORT_ARCHIVE: &str = "import_archive";
}

/// 检测参数（可修订；修订 = 新事件 + 新指纹）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectParams {
    pub gf_threshold_cycles: f64,
    pub mw_threshold_cycles: f64,
    pub tdcp_threshold_cycles: f64,
    pub clockjump_common_ratio: f64,
    pub clockjump_min_sats: usize,
    pub max_gap_seconds: f64,
    pub search_radius_cycles: i32,
    pub lli_reject_cycles: f64,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gf_threshold_cycles: 0.30,
            mw_threshold_cycles: 0.60,
            tdcp_threshold_cycles: 0.50,
            clockjump_common_ratio: 0.8,
            clockjump_min_sats: 3,
            max_gap_seconds: 120.0,
            search_radius_cycles: 3,
            lli_reject_cycles: 0.25,
        }
    }
}

impl DetectParams {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
    pub fn fingerprint(&self) -> String {
        params_fingerprint(&self.to_json())
    }
}
