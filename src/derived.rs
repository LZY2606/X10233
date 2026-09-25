//! 派生层：规范化观测、弧段、历元边界残差、候选组、修复偏移。
//! 所有派生值都携带算法版本、参数指纹与直接来源。

use crate::model::RawImport;
use serde::{Deserialize, Serialize};

/// 一个频点上规范化后的历元观测（规范化只发生在这一层；原始层不动）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormObs {
    pub pseudo_range_m: Option<f64>,
    pub carrier_phase_cyc: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub snr_dbhz: Option<f64>,
    pub lli: Option<u8>,
    /// 对应原始分量无法转换时的诊断（值留在原始层原位置）。
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectorKind {
    /// 电离层（几何无关）组合历元差
    GeometryFree,
    /// Melbourne-Wubbena 宽巷
    MelbourneWubbena,
    /// 载波时间差分（Doppler 预测对照）
    TimeDifference,
    /// RINEX LLI 标志（仅旁证，可误报）
    Lli,
}

impl DetectorKind {
    pub fn code(&self) -> &'static str {
        match self {
            DetectorKind::GeometryFree => "GF",
            DetectorKind::MelbourneWubbena => "MW",
            DetectorKind::TimeDifference => "TD",
            DetectorKind::Lli => "LLI",
        }
    }
}

/// 单个检测器给出的一个整周候选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorCandidate {
    pub detector: DetectorKind,
    pub band: u8,
    /// 建议整周偏移（周）
    pub offset_cycles: i64,
    /// 取整残差（周）
    pub residual_cycles: f64,
}

/// 检测器的判定状态（候选 / 拒绝 / 无数据 / 无异常）。
#[derive(Debug,Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectorVerdict {
    Candidate,
    NoSlip,
    InsufficientData,
    Rejected(String),
    Unknown,
}

/// 一条站-星历元边界上、单频带的检测记录（残差与候选都保留）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BandTransition {
    pub id: String,
    pub epoch_index: usize,
    pub satellite: String,
    pub band: u8,
    pub dt_seconds: Option<f64>,
    pub td_residual_cycles: Option<f64>,
    pub doppler_predicted_delta_cyc: Option<f64>,
    pub observed_delta_cyc: Option<f64>,
    pub candidates: Vec<DetectorCandidate>,
    pub lli_flag: Option<u8>,
    pub status: DetectorVerdict,
    /// 直接来源
    pub source_import: String,
    pub algo_version: String,
    pub param_fingerprint: String,
}

/// 多频组合残差（每个卫星每个边界一条）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComboResidual {
    pub id: String,
    pub epoch_index: usize,
    pub satellite: String,
    /// GF 历元差残差（L1 周）
    pub gf_residual_cycles: Option<f64>,
    pub gf_candidates: Vec<DetectorCandidate>,
    /// GF 合法整数对 (k_b1, k_b2, 取整残差)
    pub gf_pairs: Vec<(i64, i64, f64)>,
    pub gf_status: DetectorVerdict,
    /// MW 宽巷模糊度跳变残差（宽巷周）
    pub mw_residual_wl_cycles: Option<f64>,
    pub mw_candidates: Vec<DetectorCandidate>,
    pub mw_status: DetectorVerdict,
    pub source_import: String,
    pub algo_version: String,
    pub param_fingerprint: String,
}

/// 一个合法的联合修复假设：各频带整周偏移。多解时全部保留（"多个等分整周偏移"）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepairHypothesis {
    pub id: String,
    pub offsets: std::collections::BTreeMap<u8, i64>,
    pub score: f64,
    /// 支撑它的检测器（GF/MW/TD/LLI），不以任一检测器当真值
    pub supporters: Vec<String>,
    /// 各检测器给出的候选与该假设的残差
    pub per_detector: Vec<DetectorCandidate>,
    /// 未被唯一决定的原因（若有）
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupStatus {
    /// 无候选
    Clean,
    /// 多信号一致 -> 整机钟跳（非周跳）
    ClockJump,
    /// 只有 LLI 置位，检测器不支持 -> LLI 误报
    LliFalseAlarm,
    /// 有候选，等待人工裁定
    Pending,
    /// 多解无法唯一决定
    Ambiguous,
    /// 已采纳一个修复假设
    Accepted,
    /// 已否定（非周跳/不修复）
    Rejected,
    /// 已被更新的人工决定取代（冲突历史仍保留）
    Superseded,
    /// 直接来源已变化 -> 结果过期，可查看但不能冒充当前
    Stale,
}

/// 一个站-星历元边界上的候选组：汇聚三路检测器、LLI 与人工状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateGroup {
    pub id: String,
    pub epoch_index: usize,
    pub satellite: String,
    pub status: GroupStatus,
    pub hypotheses: Vec<RepairHypothesis>,
    /// 拒绝原因（一等数据）
    pub reject_reasons: Vec<String>,
    /// 不确定性说明
    pub uncertainty: Option<String>,
    /// 分类为钟跳时的共识跳变量（L1 周）
    pub clock_consensus_cycles: Option<f64>,
    pub source_import: String,
    pub algo_version: String,
    pub param_fingerprint: String,
    /// 被哪次决定采纳/否定
    pub decided_by: Option<String>,
}

/// 弧段（站-星-频带的连续相位段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub satellite: String,
    pub band: u8,
    pub start_epoch: usize,
    pub end_epoch: usize,
    pub start_time: String,
    pub end_time: String,
    /// 边界原因：长间断(不等间隔)/频率缺失/采纳的周跳
    pub split_reasons: Vec<String>,
    pub source_import: String,
}

/// 一次检测运行的完整产物（快照：固定来源版本 + 参数指纹 + 决定集合）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorRun {
    pub id: String,
    pub source_import: String,
    pub algo_version: String,
    pub param_fingerprint: String,
    pub normalized: Vec<Vec<std::collections::BTreeMap<String, std::collections::BTreeMap<u8, NormObs>>>>,
    pub transitions: Vec<BandTransition>,
    pub combos: Vec<ComboResidual>,
    pub groups: Vec<CandidateGroup>,
    pub arcs: Vec<Arc>,
    /// 输入片段与诊断（边界组合触发时留痕）
    pub input_fragments: Vec<String>,
}
