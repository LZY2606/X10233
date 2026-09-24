use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ALGO_VERSION: &str = "gnss-detect/1.0.0";
pub const ARCHIVE_FORMAT: &str = "gnss-slip-archive/1";

/// 领域量的单位。所有派生序列在接口层都标注单位；时间基准一律为 GPST（周/周内秒）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Cycles,
    Meters,
    Hertz,
    DbHz,
    Seconds,
}

impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Unit::Cycles => "cyc",
            Unit::Meters => "m",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Seconds => "s",
        }
    }
}

/// 单历元单频观测点。原始表示（raw 行、观测码）原样保留；
/// 无法转换的值留在原位置（None）并附 diagnostics。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsPoint {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub week: u32,
    pub tow: f64,
    pub freq: u8,
    pub code_l: Option<String>,
    pub code_p: Option<String>,
    pub code_d: Option<String>,
    pub code_s: Option<String>,
    pub l_cycles: Option<f64>,
    pub p_m: Option<f64>,
    pub d_hz: Option<f64>,
    pub snr_dbhz: Option<f64>,
    pub lli: u8,
    pub raw: String,
    pub diagnostics: Vec<String>,
    pub input_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Scope {
    pub station: String,
    pub sat: String,
    pub tow0: f64,
    pub tow1: f64,
}

impl Scope {
    pub fn overlaps(&self, other: &Scope) -> bool {
        self.station == other.station
            && self.sat == other.sat
            && self.tow0 <= other.tow1
            && other.tow0 <= self.tow1
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DetParams {
    pub max_gap_s: f64,
    pub gf_threshold_cyc: f64,
    pub mw_threshold_cyc: f64,
    pub td_threshold_cyc: f64,
    pub clock_min_sats: usize,
    pub clock_common_ratio: f64,
}

impl Default for DetParams {
    fn default() -> Self {
        DetParams {
            max_gap_s: 60.0,
            gf_threshold_cyc: 0.5,
            mw_threshold_cyc: 0.5,
            td_threshold_cyc: 0.5,
            clock_min_sats: 3,
            clock_common_ratio: 0.6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detector {
    GeometryFree,
    MelbourneWubbena,
    TimeDifference,
}

impl Detector {
    pub fn label(self) -> &'static str {
        match self {
            Detector::GeometryFree => "电离层组合(GF)",
            Detector::MelbourneWubbena => "Melbourne-Wubbena",
            Detector::TimeDifference => "时间差分(TD)",
        }
    }
}

/// 检测器投票：触发 / 安静 / 不可用（缺频率、缺伪距等，原因是一等数据）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VoteStatus {
    Triggered { delta: String, score: f64 },
    Quiet { value: f64 },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DetectorVote {
    pub detector: Detector,
    pub status: VoteStatus,
}

/// 合法整周偏移组合。无法唯一决定时保留全部合法候选。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegalOffsets {
    pub combos: Vec<BTreeMap<u8, i64>>,
    pub open_ended: bool,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Pending,
    Accepted,
    Rejected,
    Stale,
    Superseded,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub run_id: String,
    pub station: String,
    pub sat: String,
    pub tow: f64,
    pub arc_id: String,
    pub votes: Vec<DetectorVote>,
    pub offsets: LegalOffsets,
    pub score: f64,
    pub status: CandidateStatus,
    pub rejection: Option<String>,
    pub scope: Scope,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JumpOrigin {
    Declared,
    Detected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JumpStatus {
    Confirmed,
    UndeclaredDetected,
    DeclaredNotDetected,
    DeclaredPending,
}

/// 接收机整机钟跳：与单星周跳严格分开记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockJump {
    pub id: String,
    pub station: String,
    pub week: u32,
    pub tow: f64,
    pub offset_s: f64,
    pub origin: JumpOrigin,
    pub status: JumpStatus,
    pub declared_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LliAssessment {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub tow: f64,
    pub freq: u8,
    pub confirmed: bool,
    pub reason: String,
}

/// 修复只存偏移层：原始观测永不改写。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OffsetLayer {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub freq: u8,
    pub from_tow: f64,
    pub cycles: i64,
    pub decision_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Accept,
    Reject,
    Adjust,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArcBoundary {
    pub tow: f64,
    pub cause: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub tow0: f64,
    pub tow1: f64,
    pub epochs: Vec<f64>,
    pub freqs: Vec<u8>,
    pub boundaries: Vec<ArcBoundary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    RawInputSubmitted {
        input_id: String,
        input_kind: String,
        version: u32,
        parent: Option<String>,
        hash: String,
        affected: Vec<Scope>,
        obs_count: usize,
    },
    ImportRejected {
        reason: String,
        snippet: String,
    },
    ParamsRevised {
        version: u32,
        parent: Option<u32>,
        params: DetParams,
        fingerprint: String,
    },
    DetectionRun {
        run_id: String,
        algo: String,
        fingerprint: String,
        params: DetParams,
        input_ids: Vec<String>,
        candidates: Vec<Candidate>,
        clock_jumps: Vec<ClockJump>,
        lli: Vec<LliAssessment>,
    },
    StalenessMarked {
        cause: String,
        candidate_ids: Vec<String>,
        scopes: Vec<Scope>,
    },
    DecisionMade {
        decision_id: String,
        candidate_id: String,
        verdict: Verdict,
        note: String,
        offsets: BTreeMap<u8, i64>,
    },
    OffsetLayerWritten {
        layer: OffsetLayer,
    },
    ForkCreated {
        branch: String,
        label: String,
        from_event: String,
    },
    ArchiveImported {
        archive_hash: String,
        events: usize,
    },
}

impl EventPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            EventPayload::RawInputSubmitted { .. } => "raw_input_submitted",
            EventPayload::ImportRejected { .. } => "import_rejected",
            EventPayload::ParamsRevised { .. } => "params_revised",
            EventPayload::DetectionRun { .. } => "detection_run",
            EventPayload::StalenessMarked { .. } => "staleness_marked",
            EventPayload::DecisionMade { .. } => "decision_made",
            EventPayload::OffsetLayerWritten { .. } => "offset_layer_written",
            EventPayload::ForkCreated { .. } => "fork_created",
            EventPayload::ArchiveImported { .. } => "archive_imported",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub branch: String,
    pub parent: Option<String>,
    pub lamport: i64,
    pub ts: f64,
    pub payload: EventPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchInfo {
    pub id: String,
    pub label: String,
    pub parent_event: Option<String>,
    pub tip: Option<String>,
    pub created_ts: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputMeta {
    pub id: String,
    pub kind: String,
    pub version: u32,
    pub parent: Option<String>,
    pub hash: String,
    pub obs_count: usize,
    pub ts: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventInfo {
    pub id: String,
    pub lamport: i64,
    pub ts: f64,
    pub branch: String,
    pub kind: String,
    pub summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunInfo {
    pub run_id: String,
    pub algo: String,
    pub fingerprint: String,
    pub ts: f64,
    pub candidates: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RejectionInfo {
    pub event_id: String,
    pub reason: String,
    pub snippet: String,
    pub ts: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimelineRow {
    pub station: String,
    pub sat: String,
    pub freq: u8,
    pub points: Vec<ObsPoint>,
}

/// 某一事件视角下的完整可见状态（重放得到，历史视图不会被新结果覆盖）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct View {
    pub branch: String,
    pub branches: Vec<BranchInfo>,
    pub tip: Option<String>,
    pub view_event: String,
    pub historical: bool,
    pub algo: String,
    pub params: DetParams,
    pub params_version: u32,
    pub fingerprint: String,
    pub inputs: Vec<InputMeta>,
    pub current_inputs: BTreeMap<String, String>,
    pub rejections: Vec<RejectionInfo>,
    pub candidates: Vec<Candidate>,
    pub clock_jumps: Vec<ClockJump>,
    pub declared_jumps: Vec<ClockJump>,
    pub lli: Vec<LliAssessment>,
    pub offsets: Vec<OffsetLayer>,
    pub arcs: Vec<Arc>,
    pub timeline: Vec<TimelineRow>,
    pub observations: Vec<ObsPoint>,
    pub events: Vec<EventInfo>,
    pub runs: Vec<RunInfo>,
    pub diagnostics: Vec<String>,
    pub units: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRow {
    pub id: String,
    pub kind: String,
    pub version: u32,
    pub parent: Option<String>,
    pub hash: String,
    pub content: String,
    pub ts: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Archive {
    pub format: String,
    pub algo: String,
    pub inputs: Vec<InputRow>,
    pub events: Vec<Event>,
    pub branches: Vec<BranchInfo>,
}
