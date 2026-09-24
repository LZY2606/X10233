//! 领域模型：GNSS 载波相位周跳审阅的一等数据。
//!
//! 设计约束：
//! - 所有领域量携带单位或坐标基准（[`Qty`]）；
//! - 原始表示按历元原样保存（[`Observation`]），派生层才做规范化；
//! - 候选、拒绝原因、误差/未知状态都是一等数据（不静默丢弃）；
//! - 原始输入不可覆盖，修订以“新版本 + 父版本”关系表达。

use serde::{Deserialize, Serialize};

/// 算法版本。检测器/组合公式变化时必须递增，旧派生行不会冒充当前结果。
pub const ALGO_VERSION: &str = "algo-1.0.0";
/// 导入解析器版本。
pub const PARSER_VERSION: &str = "obs-parser-1.0.0";
/// 固定 seed：整个系统不使用任何随机源，身份均为内容/关系的确定性哈希。
pub const FIXED_SEED: &str = "gnss-cycle-break-desk-seed-fixed";
/// 速度 of light，米/秒。
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

/// 领域量单位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Meter,
    Cycle,
    Hertz,
    DbHz,
    Second,
    Degree,
    MeterSquared,
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cy",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dBHz",
            Unit::Second => "s",
            Unit::Degree => "deg",
            Unit::MeterSquared => "m^2",
        }
    }
    pub fn parse(token: &str) -> Option<Unit> {
        match token {
            "m" | "meter" | "meters" => Some(Unit::Meter),
            "cy" | "cycle" | "cycles" => Some(Unit::Cycle),
            "Hz" | "hz" | "hertz" => Some(Unit::Hertz),
            "dBHz" | "dbhz" => Some(Unit::DbHz),
            "s" | "sec" | "second" | "seconds" => Some(Unit::Second),
            "deg" | "degree" => Some(Unit::Degree),
            "m^2" | "m2" => Some(Unit::MeterSquared),
            _ => None,
        }
    }
}

/// 携带单位与（可选）坐标基准的领域量。`raw_repr` 保留导入时的原始表示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Qty {
    pub value: f64,
    pub unit: Unit,
    /// 坐标/时间基准，如 `ITRF2014`、`GPST`。
    pub datum: Option<String>,
    /// 导入文件中的原始字符串（如 `1575420000`、`-3080.000`）。
    pub raw_repr: String,
}

impl Qty {
    pub fn new(value: f64, unit: Unit, raw_repr: impl Into<String>) -> Self {
        Qty { value, unit, datum: None, raw_repr: raw_repr.into() }
    }
    pub fn with_datum(mut self, datum: impl Into<String>) -> Self {
        self.datum = Some(datum.into());
        self
    }
}

/// 观测类型：伪距 / 载波相位 / Doppler / 信噪比（与 RINEX obs type 对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObsKind {
    PseudoRange,
    CarrierPhase,
    Doppler,
    Snr,
}

impl ObsKind {
    pub fn from_code(code: &str) -> Result<ObsKind, String> {
        match code.chars().next() {
            Some('C' | 'P') => Ok(ObsKind::PseudoRange),
            Some('L') => Ok(ObsKind::CarrierPhase),
            Some('D') => Ok(ObsKind::Doppler),
            Some('S') => Ok(ObsKind::Snr),
            _ => Err(format!("无法从观测码 {code:?} 识别观测类型（首字符须为 C/P/L/D/S）")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ObsKind::PseudoRange => "pseudorange",
            ObsKind::CarrierPhase => "carrier_phase",
            ObsKind::Doppler => "doppler",
            ObsKind::Snr => "snr",
        }
    }
}

/// 信号声明：系统、频率编号、观测码、标称频率（含单位）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalDef {
    pub gnss: String,
    pub band: String,
    pub attribute: String,
    pub code: String,
    pub frequency: Qty,
}

impl SignalDef {
    pub fn kind(&self) -> Result<ObsKind, String> {
        ObsKind::from_code(&self.code)
    }
    /// 波长（米）= c / f。
    pub fn wavelength_m(&self) -> f64 {
        SPEED_OF_LIGHT_M_S / self.frequency.value
    }
}

/// LLI（Loss of Lock Indicator）原始标记，按历元原样保存。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LliFlag {
    pub raw: String,
    /// RINEX 语义位：1=可能周跳/失锁, 2=周跳/半周可能, 3=半周未知, ...
    pub slip_suspect: bool,
}

/// 一条原始观测（站 × 星 × 码 × 历元）。不可变，属于某次导入批次。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub sat: String,
    pub code: String,
    pub kind: ObsKind,
    pub qty: Qty,
    pub lli: Option<LliFlag>,
    pub ssi: Option<String>,
    /// 文件行号，供原始表示反查。
    pub line_no: u32,
}

/// 卫星状态行（健康标志、坐标 + 坐标基准）。字段缺失时为 [`None`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    pub health: Option<String>,
    pub pos: Option<[Qty; 3]>,
    pub frame: Option<String>,
    pub line_no: u32,
}

/// 历元内的原始钟跳标记（EVENT 行），与计算检测出的钟跳分开保存。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    pub kind: String,
    pub target: String,
    pub amount: Option<Qty>,
    pub note: Option<String>,
    pub line_no: u32,
}

/// 一个观测历元（按文件顺序，时间戳毫秒 + 序号）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Epoch {
    pub ordinal: u32,
    /// UTC 毫秒（规范化，仅派生层使用）。
    pub time_ms: i64,
    /// 原始时间表示。
    pub time_raw: String,
    pub flag: Option<String>,
    pub clock_offset: Option<Qty>,
    pub observations: Vec<Observation>,
    pub sat_states: Vec<SatState>,
    pub clock_events: Vec<ClockEvent>,
    pub line_no: u32,
}

/// 解析/转换诊断。无法转换的值留在原位置并附带诊断，绝不为画图丢弃。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: DiagLevel,
    pub line_no: Option<u32>,
    pub code: String,
    pub message: String,
    pub raw_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagLevel {
    Error,
    Warning,
    Info,
}

/// 台站元数据（按原始表示保存，坐标带基准）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StationMeta {
    pub id: String,
    pub marker: Option<String>,
    pub marker_type: Option<String>,
    pub timescale: Option<String>,
    pub coord: Option<[Qty; 3]>,
    pub datum: Option<String>,
}

/// 一次解析结果（可能 `committable=false`：存在结构性错误，拒绝提交）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedFile {
    pub case_id: Option<String>,
    pub station: StationMeta,
    pub signals: Vec<SignalDef>,
    pub epochs: Vec<Epoch>,
    pub diagnostics: Vec<Diagnostic>,
    pub parent_note: Option<String>,
}

impl ParsedFile {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.level == DiagLevel::Error)
    }
}

// ------------------------- 派生层：检测器与候选 -------------------------

/// 检测器身份。三者相互独立，任何一个都不被当作真值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    /// 电离层（geometry-free）相位组合：L1·λ1 − L2·λ2
    GeometryFree,
    /// Melbourne–Wübbena：宽巷相位 − 窄巷伪距
    MelbourneWubbena,
    /// 时间差分（Doppler 辅助）：ΔL − D̄·Δt
    TimeDifference,
    /// LLI 原始标记（仅线索，非真值）
    Lli,
}

impl Detector {
    pub fn as_str(self) -> &'static str {
        match self {
            Detector::GeometryFree => "geometry_free",
            Detector::MelbourneWubbena => "melbourne_wubbena",
            Detector::TimeDifference => "time_difference",
            Detector::Lli => "lli",
        }
    }
}

/// 候选的性质：单星周跳 与 整机钟跳 必须分开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    CycleSlip,
    ReceiverClockJump,
}

/// 候选状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    /// 未经人工处理。
    Open,
    /// 已采纳（写入偏移层）。
    Accepted,
    /// 已否定（LLI 误报即落此状态）。
    Rejected,
    /// 多检测器不一致 / 多整数解等距，无法唯一决定。
    Ambiguous,
}

/// 单频整周偏移假设（周）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegerHypothesis {
    pub band: String,
    pub code: String,
    pub cycles: i64,
    pub cycles_raw: String,
}

/// 候选被拒绝/扣分的结构化原因（一等数据）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RejectReason {
    pub code: String,
    pub detail: String,
}

/// 单条检测器证据。残差/跳变量都带单位；无法计算时 `unknown` 给出原因。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorEvidence {
    pub detector: Detector,
    pub kind: CandidateKind,
    /// 标准化残差/跳变量（米或周，取决于组合）；无法计算为 None。
    pub value: Option<f64>,
    pub unit: Unit,
    /// 原始组合值序列（米或周），供残差面板按单位绘图。
    pub raw_value: Option<String>,
    /// 该检测器支持的整数假设；无法唯一决定时可有多个等距候选。
    pub hypotheses: Vec<IntegerHypothesis>,
    /// 0..=1，1 表示证据最强。
    pub score: f64,
    pub unknown_reason: Option<String>,
    pub reject_reasons: Vec<RejectReason>,
}

/// 周跳/钟跳候选（一个站星边界一份；采纳与未采纳都保留）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlipCandidate {
    /// 稳定主体键：(sat, 前历元时间ms, 后历元时间ms)，跨重算保持不变。
    pub subject_key: String,
    pub sat: String,
    pub kind: CandidateKind,
    pub status: CandidateStatus,
    pub evidence: Vec<DetectorEvidence>,
    /// 跨检测器整合得分（任何检测器都不是真值，仅作排序）。
    pub combined_score: f64,
    pub reject_reasons: Vec<RejectReason>,
    /// 当多个整数假设严格等距时全部保留。
    pub tied: bool,
}

/// 弧段边界类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcBoundary {
    Start,
    End,
    FrequencyMissing,
    Gap,
    LliBreak,
    Slip,
}

/// 站星频率弧段（原始弧段：仅由输入决定；修复弧段在偏移层另行表达）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arc {
    pub sat: String,
    pub code: String,
    pub band: String,
    pub ordinal_start: u32,
    pub ordinal_end: u32,
    pub boundaries: Vec<(u32, ArcBoundary, String)>,
}

/// 偏移层记录：修复只存偏移，绝不改动原始载波。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffsetEntry {
    pub sat: String,
    pub code: String,
    pub from_ordinal: u32,
    pub cycles: i64,
    pub source_candidate_key: String,
    pub decision_event_id: String,
}
