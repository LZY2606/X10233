//! 领域模型：原始历元、观测码/频率、弧段、检测器候选、裁定事件。
//! 原始输入不可变；候选、拒绝原因、误差/未知状态都是一等数据。
use crate::units::Quantity;
use serde::{Deserialize, Serialize};

pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;
/// GPS L1/L2 基准频率比：f1 = 154 f0，f2 = 120 f0（f0 = 10.23 MHz）。
pub const GPS_F0_MHZ: f64 = 10.23;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObsKind {
    C, // 伪距 (m)
    L, // 载波相位 (cyc)
    D, // Doppler (Hz)
    S, // 信噪比 (dB-Hz)
}

impl ObsKind {
    pub fn from_char(c: char) -> Option<ObsKind> {
        match c {
            'C' => Some(ObsKind::C),
            'L' => Some(ObsKind::L),
            'D' => Some(ObsKind::D),
            'S' => Some(ObsKind::S),
            _ => None,
        }
    }
}

/// 观测码（如 L1C、C1W、D5X），按历元原样保存。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObsCode {
    pub raw: String,
    pub kind: ObsKind,
    /// 频率编号位（RINEX 频段字符：'1','2','5','7' ...），未知频率时保留原字符。
    pub band_char: char,
}

impl ObsCode {
    pub fn parse(raw: &str) -> Result<ObsCode, String> {
        let raw = raw.trim().to_string();
        let mut chars = raw.chars();
        let first = chars
            .next()
            .ok_or_else(|| format!("观测码为空：{raw:?}"))?;
        let kind = ObsKind::from_char(first)
            .ok_or_else(|| format!("观测码首字符必须是 C/L/D/S：{raw:?}"))?;
        let band_char = chars
            .next()
            .ok_or_else(|| format!("观测码缺少频率编号：{raw:?}"))?;
        Ok(ObsCode { raw, kind, band_char })
    }
}

/// 频率编号 -> 频率（MHz）。无法识别的频段返回 None，派生层附诊断而不是猜测。
pub fn band_frequency_mhz(system: char, band: char) -> Option<f64> {
    let f0 = GPS_F0_MHZ;
    match (system, band) {
        ('G', '1') => Some(154.0 * f0),
        ('G', '2') => Some(120.0 * f0),
        ('G', '5') => Some(115.0 * f0),
        ('E', '1') => Some(154.0 * f0),
        ('E', '7') => Some(118.0 * f0),
        ('E', '5') => Some(115.0 * f0),
        ('C', '2') => Some(1561.098),
        ('C', '7') => Some(1207.140),
        ('C', '6') => Some(1278.750),
        ('R', '1') => Some(1602.0),
        ('R', '2') => Some(1246.0),
        _ => None,
    }
}

pub fn wavelength_m(system: char, band: char) -> Option<f64> {
    band_frequency_mhz(system, band).map(|f_mhz| SPEED_OF_LIGHT_M_S / (f_mhz * 1e6))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observable {
    pub code: ObsCode,
    pub q: Quantity,
    /// RINEX LLI 字符（如 '1'），原样保存；无则 None。
    pub lli: Option<char>,
    /// RINEX 信号强度字符，原样保存。
    pub ssi: Option<char>,
}

/// 历元时间：原始表头字符串保留，规范化为 GPS 连续秒（派生层使用）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EpochTime {
    pub raw: String,
    /// 自 2024-01-01 00:00:00 GPS 起的连续秒（虚拟案例统一基准）。
    pub gps_sec: Option<i64>,
    pub diagnostic: Option<String>,
}

impl EpochTime {
    pub fn key(&self) -> String {
        match self.gps_sec {
            Some(s) => format!("t{s}"),
            None => format!("raw:{}", self.raw),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatEpoch {
    pub sat: String, // 如 G01
    pub obs: Vec<Observable>,
}

impl SatEpoch {
    pub fn find(&self, code_raw: &str) -> Option<&Observable> {
        self.obs.iter().find(|o| o.code.raw == code_raw)
    }
    pub fn band_obs(&self, kind: ObsKind, band: char) -> Vec<&Observable> {
        self.obs
            .iter()
            .filter(|o| o.code.kind == kind && o.code.band_char == band)
            .collect()
    }
}

/// 整机钟跳事件（源数据里显式给出的事件，与观测历元分开保存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockJumpEvent {
    pub at: EpochTime,
    pub step: Quantity, // s
}

/// 卫星状态（高度角等），原始 key=value 原样保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    pub at: EpochTime,
    pub raw_fields: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub line_no: usize,
    pub line: String,
    pub message: String,
}

/// 一次提交的原始批次：伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号、观测码
/// 全部按历元原样保存。`replaces` 给出局部修订覆盖的历元键区间（起含止含）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBatch {
    pub id: String,
    pub stream: String,
    pub station: String,
    pub submitted_event: i64,
    pub epochs: Vec<RawEpoch>,
    pub clock_events: Vec<ClockJumpEvent>,
    pub sat_states: Vec<SatState>,
    pub diagnostics: Vec<ParseDiagnostic>,
    /// 父原始版本 id（修订链），初始导入为 None。
    pub parent_version: Option<String>,
    /// 局部修订：None 表示全量；否则只覆盖这些历元键。
    pub replaces: Option<Vec<String>>,
    pub content_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEpoch {
    pub time: EpochTime,
    pub sats: Vec<SatEpoch>,
    pub rcvr_clock_raw: Option<String>,
}

// ---------- 检测器产物 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Detector {
    /// 几何无关（电离层）组合 L1−L2
    IonosphereGeometryFree,
    /// Melbourne-Wubbena 宽巷组合
    MelbourneWubbena,
    /// 载波时间差分（−Doppler 积分）
    TimeDifference,
}

impl Detector {
    pub fn code(&self) -> &'static str {
        match self {
            Detector::IonosphereGeometryFree => "GF",
            Detector::MelbourneWubbena => "MW",
            Detector::TimeDifference => "TD",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FindingKind {
    /// 整机钟跳（所有卫星共模）
    ClockJump,
    /// 单星周跳
    CycleSlip,
    /// LLI 标记本身（可能误报）
    LliFlag,
    /// 无法给出结论（频率缺失等）
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlipHypothesis {
    pub band: char,
    pub offset_cycles: i64,
}

/// 一个合法修复假设：可同时涉及多个频段（如等分整周偏移族）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hypothesis {
    pub offsets: Vec<SlipHypothesis>,
    /// 该假设与证据的残差（米），越小越自洽。
    pub residual_m: f64,
}

/// 候选来源：检测器只是来源之一，任一检测器都不是真值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CandidateSource {
    Detector(Detector),
    /// 接收机 LLI 标记
    ReceiverLli,
    /// 跨卫星共模检测出的整机钟跳
    ClockCommonMode,
    /// 源数据中显式声明的钟跳事件
    DeclaredClockEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub arc_id: String,
    pub sat: String,
    pub source: CandidateSource,
    pub kind: FindingKind,
    /// 弧段边界历元键（跳变发生在该历元，相对前一历元）。
    pub at_epoch: String,
    pub prev_epoch: Option<String>,
    /// 全部合法假设；无法唯一决定时保留多个（如等分整周偏移族）。
    pub hypotheses: Vec<Hypothesis>,
    /// 0..=1 检测器得分；检测器之间可能不一致，得分不是真值。
    pub score: f64,
    /// 多频组合残差证据（米 / 周 / Hz·s），带单位。
    pub evidence: Vec<EvidencePoint>,
    /// 误差或未知状态诊断；候选本身保留，不丢弃。
    pub diagnostic: Option<String>,
    /// 与显式钟跳事件一致。
    pub matches_clock_event: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePoint {
    pub label: String,
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArcInfo {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub band: char,
    pub code_l: String,
    pub start_epoch: String,
    pub end_epoch: String,
    pub n_epochs: usize,
    /// 弧段边界成因（不等间隔、LLI、频率缺失、钟跳…）。
    pub boundary_reasons: Vec<String>,
    /// 该弧段依赖的 (历元键 -> 原始批次 id)；局部过期的判定依据。
    pub deps: Vec<(String, String)>,
}

/// 组合残差序列中的一个点：每个显示值都有稳定身份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub id: String,
    pub epoch: String,
    pub value: Option<f64>,
    pub unit: String,
    pub reason_absent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualSeries {
    pub id: String,
    pub arc_id: String,
    pub detector: Detector,
    pub points: Vec<SeriesPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    pub arcs: Vec<ArcInfo>,
    pub series: Vec<ResidualSeries>,
    pub candidates: Vec<Candidate>,
    /// 原始层诊断（未知频率、无法转换的值等）随结果一起可见。
    pub raw_diagnostics: Vec<(String, ParseDiagnostic)>,
}

// ---------- 参数与裁定 ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    pub gf_sigma_cycles: f64,
    pub mw_sigma_wl: f64,
    pub td_sigma_cycles: f64,
    pub clock_sigma_m: f64,
    pub max_gap_s: i64,
    pub min_snr_db_hz: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            gf_sigma_cycles: 0.25,
            mw_sigma_wl: 0.5,
            td_sigma_cycles: 0.75,
            clock_sigma_m: 100.0,
            max_gap_s: 60,
            min_snr_db_hz: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Verdict {
    /// 采纳（修复层写入整周偏移）
    Accept,
    /// 否定（不是周跳）
    Reject,
    /// LLI 误报
    LliFalseAlarm,
    /// 挂起，候选保留
    Defer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub event_id: i64,
    pub candidate_id: String,
    pub verdict: Verdict,
    /// 采纳时选择的假设下标（多候选必须人工选定）。
    pub chosen: Option<usize>,
    pub note: String,
    /// 决定时看到的检测器/参数/来源版本，供反查。
    pub detector: Detector,
    pub params_fp: String,
    pub at_epoch: String,
}

/// 修复只存偏移层：弧段内从某历元起的整周偏移（周），绝不回写原始载波。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OffsetEntry {
    pub arc_id: String,
    pub band: char,
    pub from_epoch: String,
    pub offset_cycles: i64,
    pub decision_event: i64,
    pub candidate_id: String,
}
