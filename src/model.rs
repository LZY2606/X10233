//! 领域模型：GNSS 载波相位周跳审阅。
//! 所有领域量携带单位；原始输入按历元原样保存；派生量带算法版本、参数指纹与直接来源。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Id = String;

/// 由内容哈希生成稳定身份：同一内容在任何机器、任何数据库中得到同一 ID。
pub fn content_id(prefix: &str, canonical: &str) -> Id {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b"\0");
    h.update(canonical.as_bytes());
    let hex = format!("{:x}", h.finalize());
    format!("{}_{}", prefix, &hex[..16])
}

pub fn canonical<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("canonical serialization")
}

// ---------------------------------------------------------------- 单位与量

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meter,
    Cycle,
    Hz,
    DbHz,
    Second,
    Dimensionless,
}

impl Unit {
    pub fn symbol(&self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cyc",
            Unit::Hz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Second => "s",
            Unit::Dimensionless => "1",
        }
    }
}

/// 带单位（和可选坐标/时间基准）的领域量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    pub value: f64,
    pub unit: Unit,
    /// 例如 "GPS"（时系）、"ITRF2020"（坐标基准）；无基准概念时为 None。
    pub datum: Option<String>,
}

impl Quantity {
    pub fn new(value: f64, unit: Unit) -> Self {
        Quantity { value, unit, datum: None }
    }
    pub fn with_datum(value: f64, unit: Unit, datum: &str) -> Self {
        Quantity { value, unit, datum: Some(datum.to_string()) }
    }
    pub fn display(&self) -> String {
        match &self.datum {
            Some(d) => format!("{:.6} {} [{}]", self.value, self.unit.symbol(), d),
            None => format!("{:.6} {}", self.value, self.unit.symbol()),
        }
    }
}

// ---------------------------------------------------------------- 诊断

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// 发生位置，例如 "epoch=2026-03-01T00:04:00Z sat=G25 code=L2W"。
    pub path: String,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Warning,
    Error,
}

// ---------------------------------------------------------------- 原始层（按历元原样保存）

/// 单条原始观测：值以导入时的字符串原样保存，绝不在导入层规范化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawObs {
    /// 观测码，例如 C1C / L1C / D1C / S1C / C2W / L2W。
    pub code: String,
    /// 原始字符串；缺测（空白字段）为 None。
    pub value: Option<String>,
    /// LLI 原始字符（0-7），原样保存，不当真值。
    pub lli: Option<String>,
    /// 信号强度原始字符（0-9）。
    pub ssi: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawSatObs {
    pub sat: String,
    pub obs: Vec<RawObs>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEpoch {
    /// 历元时刻的原始表示（导入层不做时系换算）。
    pub epoch: String,
    /// RINEX 历元标志原始字符（钟跳/事件线索，非真值）。
    pub epoch_flag: Option<String>,
    /// 接收机钟差原始字段（秒，字符串原样保存）。
    pub recv_clock_offset: Option<String>,
    pub sats: Vec<RawSatObs>,
}

/// 一次导入产生一个不可变版本；修订以 parent 链接新版本表达。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawImport {
    pub id: Id,
    pub version: u32,
    pub parent: Option<Id>,
    pub source_name: String,
    pub station: String,
    /// 头部声明的观测码序列（含频率编号，如 G: C1C L1C D1C S1C C2W L2W D2W S2W）。
    pub obs_types: Vec<String>,
    pub epochs: Vec<RawEpoch>,
    /// 导入期诊断（无法转换的值留在原位置并在此附诊断）。
    pub diagnostics: Vec<Diagnostic>,
}

// ---------------------------------------------------------------- 派生层

/// 规范化后的单条观测（派生层产物；原始字符串仍在 RawObs 中）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormObs {
    pub code: String,
    pub freq: Option<u8>,
    pub value: Option<Quantity>,
    pub lli: u8,
    pub ssi: u8,
    pub diag: Option<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DerivedStatus {
    Current,
    /// 局部输入变化使本结果过期；仍可查看，但不能冒充当前结果。
    Stale { reason: String },
    /// 计算失败（错误是一等数据）。
    Error { message: String },
    /// 输入不足，无法判定（未知是一等数据）。
    Unknown { reason: String },
}

impl DerivedStatus {
    pub fn label(&self) -> String {
        match self {
            DerivedStatus::Current => "current".into(),
            DerivedStatus::Stale { reason } => format!("stale: {}", reason),
            DerivedStatus::Error { message } => format!("error: {}", message),
            DerivedStatus::Unknown { reason } => format!("unknown: {}", reason),
        }
    }
    pub fn is_current(&self) -> bool {
        matches!(self, DerivedStatus::Current)
    }
}

/// 派生结果：一律携带算法版本、参数指纹与直接来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedResult {
    pub id: Id,
    /// 例如 "arc" / "gf_series" / "mw_series" / "td_series" / "clock_scan"。
    pub kind: String,
    /// 作用对象（弧段 ID 或站 ID）。
    pub target: Id,
    pub algo_version: String,
    pub param_fp: String,
    pub status: DerivedStatus,
    /// 直接来源（导入版本 ID、弧段 ID 等）。
    pub sources: Vec<Id>,
    pub payload: serde_json::Value,
}

// ---------------------------------------------------------------- 弧段

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arc {
    pub id: Id,
    pub station: String,
    pub sat: String,
    pub freq: u8,
    pub start_epoch: String,
    pub end_epoch: String,
    pub epoch_count: usize,
    pub import_id: Id,
    pub import_version: u32,
}

// ---------------------------------------------------------------- 候选与裁定

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectorKind {
    /// 电离层（几何自由）组合。
    GeometryFree,
    /// Melbourne-Wubbena 组合。
    MelbourneWubbena,
    /// 历元间时间差分（载波 vs Doppler）。
    TimeDifference,
    /// 整机钟跳扫描（与单星周跳严格分开）。
    ClockScan,
    /// LLI 标志（原始数据线索，非检测器真值）。
    LliFlag,
}

impl DetectorKind {
    pub fn code(&self) -> &'static str {
        match self {
            DetectorKind::GeometryFree => "gf",
            DetectorKind::MelbourneWubbena => "mw",
            DetectorKind::TimeDifference => "td",
            DetectorKind::ClockScan => "clock",
            DetectorKind::LliFlag => "lli",
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            DetectorKind::GeometryFree => "电离层组合(GF)",
            DetectorKind::MelbourneWubbena => "Melbourne-Wubbena",
            DetectorKind::TimeDifference => "时间差分(TD)",
            DetectorKind::ClockScan => "整机钟跳扫描",
            DetectorKind::LliFlag => "LLI 标志",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateKind {
    CycleSlip,
    ClockJump,
    /// 多个等分整周偏移：无法唯一决定时保留全部合法候选。
    AmbiguousRepair,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Open,
    Accepted,
    /// 拒绝原因是一等数据。
    Rejected { reason: String },
    /// 被新一轮计算取代（旧候选仍可查看）。
    Superseded { by: Id },
}

impl CandidateStatus {
    pub fn label(&self) -> String {
        match self {
            CandidateStatus::Open => "open".into(),
            CandidateStatus::Accepted => "accepted".into(),
            CandidateStatus::Rejected { reason } => format!("rejected: {}", reason),
            CandidateStatus::Superseded { .. } => "superseded".into(),
        }
    }
}

/// 一个整周偏移解：(频率号, 周数)。
pub type SlipVector = Vec<(u8, f64)>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: Id,
    /// 整机钟跳不属于任何弧段，为 None。
    pub arc_id: Option<Id>,
    pub station: String,
    pub sat: Option<String>,
    pub epoch: String,
    pub detector: DetectorKind,
    pub kind: CandidateKind,
    /// 主解（无法唯一决定时为评分最高者）。
    pub slips: SlipVector,
    /// 全部合法候选解（含主解）；唯一解时长度为 1。
    pub alternatives: Vec<SlipVector>,
    pub score: f64,
    /// 机器可读取的判断依据（组合残差、阈值、来源字段等）。
    pub evidence: serde_json::Value,
    pub status: CandidateStatus,
    /// 基于已过期输入时为 true（仍可查看，不能冒充当前结果）。
    pub stale: bool,
    pub algo_version: String,
    pub param_fp: String,
    pub sources: Vec<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionAction {
    Accept,
    Reject,
    /// 选择某个等分解作为修复。
    ChooseAlternative,
    AdjustParams,
    ResolveConflict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: Id,
    pub candidate_id: Option<Id>,
    pub action: DecisionAction,
    pub reviewer: String,
    pub reason: String,
    /// ChooseAlternative 时选中的解。
    pub chosen: Option<SlipVector>,
    pub branch: String,
}

/// 修复只存偏移层：永不修改原始或派生序列，校正视图 = 派生值 + 生效偏移层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffsetLayer {
    pub id: Id,
    pub arc_id: Id,
    pub from_epoch: String,
    pub offsets: SlipVector,
    pub decision_id: Id,
    pub active: bool,
}

// ---------------------------------------------------------------- 参数

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    /// 弧段断裂阈值（秒）：相邻历元间隔超过则开新弧段。
    pub max_gap_s: f64,
    /// GF 组合跳变阈值（米）。
    pub gf_threshold_m: f64,
    /// MW 宽巷跳变阈值（周）。
    pub mw_threshold_cyc: f64,
    /// TD 残差阈值（周）。
    pub td_threshold_cyc: f64,
    /// 整机钟跳阈值（秒，等效伪距公共跳变）。
    pub clock_jump_threshold_s: f64,
    /// 等分解枚举的 GF 残差容差（米）。
    pub gf_solution_tol_m: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 120.0,
            gf_threshold_m: 0.05,
            mw_threshold_cyc: 0.6,
            td_threshold_cyc: 0.3,
            clock_jump_threshold_s: 1.0e-4,
            gf_solution_tol_m: 0.05,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        content_id("pfp", &canonical(self)).replace("pfp_", "")
    }
}

// ---------------------------------------------------------------- 事件（机器可读取的判断依据）

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    CaseLoaded,
    ImportSubmitted,
    ImportRevised,
    ParamsRevised,
    DetectionRun,
    CandidateRaised,
    CandidateRejected,
    DecisionRecorded,
    ConflictDetected,
    ConflictResolved,
    OffsetLayerAdded,
    ResultsInvalidated,
    BranchForked,
    Undo,
    ExportWritten,
    ImportLoaded,
}

impl EventKind {
    pub fn code(&self) -> &'static str {
        match self {
            EventKind::CaseLoaded => "case_loaded",
            EventKind::ImportSubmitted => "import_submitted",
            EventKind::ImportRevised => "import_revised",
            EventKind::ParamsRevised => "params_revised",
            EventKind::DetectionRun => "detection_run",
            EventKind::CandidateRaised => "candidate_raised",
            EventKind::CandidateRejected => "candidate_rejected",
            EventKind::DecisionRecorded => "decision_recorded",
            EventKind::ConflictDetected => "conflict_detected",
            EventKind::ConflictResolved => "conflict_resolved",
            EventKind::OffsetLayerAdded => "offset_layer_added",
            EventKind::ResultsInvalidated => "results_invalidated",
            EventKind::BranchForked => "branch_forked",
            EventKind::Undo => "undo",
            EventKind::ExportWritten => "export_written",
            EventKind::ImportLoaded => "import_loaded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// 全局单调序号（导出/再导入后相对顺序保持）。
    pub seq: u64,
    pub id: Id,
    pub branch: String,
    pub kind: EventKind,
    /// 虚拟时间（固定基准 + 序号，确定性）。
    pub at: String,
    pub payload: serde_json::Value,
    /// 本事件涉及的直接对象（来源关系）。
    pub refs: Vec<Id>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub forked_from: Option<(String, u64)>,
    pub created_seq: u64,
}
