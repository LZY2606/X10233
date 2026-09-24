//! 领域模型：原始观测按历元原样保存；派生层携带算法版本、参数指纹与直接来源。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SPEED_OF_LIGHT_MPS: f64 = 299_792_458.0;

pub const ALGO_NORM: &str = "norm/v1";
pub const ALGO_ARC: &str = "arc/v1";
pub const ALGO_COMBO: &str = "combo/v1";
pub const ALGO_DETECT: &str = "detect/v1";
pub const ALGO_REPAIR: &str = "repair/v1";

pub const MAIN_BRANCH: &str = "main";
pub const GLOBAL_BRANCH: &str = "*";

/// 机器可读取的判断依据（写入事件与载荷）。
pub const BASIS_DETECTOR_NOT_TRUTH: &str = "detector_candidates_are_not_truth";
pub const BASIS_CLOCK_SEPARATED: &str = "clock_jump_separated_from_single_sat_slip";
pub const BASIS_REPAIR_OFFSET_LAYER: &str = "repair_stored_as_offset_layer_only";
pub const BASIS_HUMAN_ADJUDICATION: &str = "human_adjudication_recorded";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Diag {
    pub code: String,
    pub message: String,
}

impl Diag {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Diag { code: code.to_string(), message: message.into() }
    }
}

/// 检测器参数集。任何修订生成新版本（父版本关系），指纹 = 规范化 JSON 的哈希。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// "G:F1" -> Hz。频率表缺失时对应观测量保留原值并附诊断。
    pub freq_table_hz: BTreeMap<String, f64>,
    pub max_gap_s: f64,
    pub gf_threshold_m: f64,
    pub mw_threshold_cyc: f64,
    pub td_threshold_cyc: f64,
    pub clock_min_sats: u32,
    pub clock_min_jump_m: f64,
    pub clock_consistency_m: f64,
    pub split_max_parts: u32,
    pub integer_tolerance_cyc: f64,
    pub snr_min_dbhz: f64,
}

impl Default for Params {
    fn default() -> Self {
        let mut freq = BTreeMap::new();
        freq.insert("G:F1".to_string(), 1_575.42e6);
        freq.insert("G:F2".to_string(), 1_227.60e6);
        Params {
            freq_table_hz: freq,
            max_gap_s: 60.0,
            gf_threshold_m: 0.2,
            mw_threshold_cyc: 0.7,
            td_threshold_cyc: 0.5,
            clock_min_sats: 3,
            clock_min_jump_m: 1000.0,
            clock_consistency_m: 1.0,
            split_max_parts: 3,
            integer_tolerance_cyc: 0.25,
            snr_min_dbhz: 30.0,
        }
    }
}

/// 参数修订的影响范围：只让依赖被改参数的结果过期。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Affected {
    pub norm: bool,
    pub arc: bool,
    pub combo: bool,
    /// None = 全部检测器；Some = 只影响这些检测器的候选。
    pub detectors: Option<Vec<String>>,
}

impl Params {
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("params serialize")
    }

    pub fn fingerprint(&self) -> String {
        crate::ids::hash_id("pfp", &[&self.canonical_json()])
    }

    pub fn changed_fields(&self, other: &Params) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.freq_table_hz != other.freq_table_hz {
            out.push("freq_table_hz");
        }
        if self.max_gap_s != other.max_gap_s {
            out.push("max_gap_s");
        }
        if self.gf_threshold_m != other.gf_threshold_m {
            out.push("gf_threshold_m");
        }
        if self.mw_threshold_cyc != other.mw_threshold_cyc {
            out.push("mw_threshold_cyc");
        }
        if self.td_threshold_cyc != other.td_threshold_cyc {
            out.push("td_threshold_cyc");
        }
        if self.clock_min_sats != other.clock_min_sats
            || self.clock_min_jump_m != other.clock_min_jump_m
            || self.clock_consistency_m != other.clock_consistency_m
        {
            out.push("clock");
        }
        if self.split_max_parts != other.split_max_parts
            || self.integer_tolerance_cyc != other.integer_tolerance_cyc
        {
            out.push("split");
        }
        if self.snr_min_dbhz != other.snr_min_dbhz {
            out.push("snr_min_dbhz");
        }
        out
    }

    /// old -> self 的影响范围。
    pub fn affected_vs(&self, old: &Params) -> Affected {
        let mut aff = Affected::default();
        let mut dets: Vec<String> = Vec::new();
        let mut all_detectors = false;
        for f in self.changed_fields(old) {
            match f {
                "freq_table_hz" => {
                    aff.norm = true;
                    aff.combo = true;
                    all_detectors = true;
                }
                "max_gap_s" => {
                    aff.arc = true;
                    aff.combo = true;
                    all_detectors = true;
                }
                "gf_threshold_m" => dets.push("gf".into()),
                "mw_threshold_cyc" => dets.push("mw".into()),
                "td_threshold_cyc" => {
                    dets.push("td".into());
                    dets.push("td-split".into());
                }
                "clock" => {
                    dets.push("clock".into());
                    dets.push("td".into());
                    dets.push("td-split".into());
                }
                "split" => {
                    dets.push("td".into());
                    dets.push("td-split".into());
                }
                "snr_min_dbhz" => all_detectors = true,
                _ => {}
            }
        }
        if all_detectors {
            aff.detectors = None;
        } else if !dets.is_empty() {
            dets.sort();
            dets.dedup();
            aff.detectors = Some(dets);
        }
        aff
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub station: String,
    pub timescale: String,
    pub tick_created: i64,
    pub raw_text: String,
    pub n_epochs: i64,
    pub n_obs: i64,
    pub diagnostics: Vec<Diag>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsRow {
    pub id: String,
    pub input_id: String,
    pub line_no: i64,
    pub epoch_ms: i64,
    pub station: String,
    pub sat: String,
    pub freq: String,
    pub code: String,
    pub kind: String, // P|L|D|S|K(钟跳事件)
    pub raw_value: String,
    pub unit: String,
    pub lli: Option<i64>,
    pub ch: Option<i64>,
    pub diags: Vec<Diag>,
    pub tick_created: i64,
}

impl ObsRow {
    pub fn value(&self) -> Option<f64> {
        self.raw_value.parse::<f64>().ok()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamsRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub fp: String,
    pub params: Params,
    pub tick_created: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DerivedRow {
    pub id: String,
    pub layer: String,
    pub algo: String,
    pub param_fp: String,
    pub branch: String,
    pub scope: String, // "station|sat|freq"
    pub nkey: String,
    pub deps: Vec<String>,
    pub payload: serde_json::Value,
    pub tick_created: i64,
    pub tick_stale: Option<i64>,
}

impl DerivedRow {
    pub fn scope_parts(&self) -> (String, String, String) {
        let mut it = self.scope.split('|');
        (
            it.next().unwrap_or("").to_string(),
            it.next().unwrap_or("").to_string(),
            it.next().unwrap_or("").to_string(),
        )
    }
    pub fn is_stale_at(&self, tick: i64) -> bool {
        self.tick_stale.map_or(false, |t| t <= tick)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub id: String,
    pub candidate_id: String,
    pub action: String, // accept|reject|adjust|resolve
    pub note: String,
    pub branch: String,
    pub parent_decision: Option<String>,
    pub payload: serde_json::Value,
    pub tick_created: i64,
    pub tick_undone: Option<i64>,
}

impl DecisionRecord {
    pub fn is_active_at(&self, tick: i64) -> bool {
        self.tick_created <= tick && self.tick_undone.map_or(true, |u| u > tick)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchRecord {
    pub id: String,
    pub parent: Option<String>,
    pub fork_event: Option<String>,
    pub name: String,
    pub tick_created: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub tick: i64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// 作用域 (station, sat, freq)；空串 = 通配。
pub type Scope = (String, String, String);

pub fn scope_key(s: &Scope) -> String {
    format!("{}|{}|{}", s.0, s.1, s.2)
}

pub fn scope_matches(row_scope: &str, target: &Scope) -> bool {
    let mut it = row_scope.split('|');
    let rs = it.next().unwrap_or("");
    let rt = it.next().unwrap_or("");
    let rf = it.next().unwrap_or("");
    (target.0.is_empty() || rs.is_empty() || rs == target.0)
        && (target.1.is_empty() || rt.is_empty() || rt == target.1)
        && (target.2.is_empty() || rf.is_empty() || rf == target.2)
}

// ---------- 派生层载荷 ----------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArcPayload {
    pub station: String,
    pub sat: String,
    pub freq: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub epochs: Vec<i64>,
    pub ends_with_gap: Option<GapBoundary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GapBoundary {
    pub gap_ms: i64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComboPayload {
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub gf_m: Option<f64>,
    pub mw_cyc: Option<f64>,
    pub td_res_cyc: BTreeMap<String, f64>,
    pub diags: Vec<Diag>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SplitInfo {
    pub parts: u32,
    pub each_cycles: i64,
    pub from_ms: i64,
    pub to_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidatePayload {
    pub station: String,
    pub sat: Option<String>,
    pub freq: Option<String>,
    pub epoch_ms: i64,
    pub detector: String, // gf|mw|td|td-split|lli|clock
    pub kind: String,     // slip|clock_jump
    pub size_cycles: f64,
    pub size_display: String,
    pub score: f64,
    pub reasons: Vec<String>,
    pub group: Option<String>,
    pub split: Option<SplitInfo>,
    pub basis: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepairPayload {
    pub station: String,
    pub sat: String,
    pub freq: Option<String>,
    pub from_epoch_ms: i64,
    pub offset_cycles: f64,
    pub decision_id: String,
    pub candidate_id: String,
    pub basis: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CandidateStatus {
    Proposed,
    Accepted { size_cycles: f64 },
    Rejected { reason: String },
}

impl CandidateStatus {
    pub fn label(&self) -> String {
        match self {
            CandidateStatus::Proposed => "待裁定".to_string(),
            CandidateStatus::Accepted { size_cycles } => format!("已采纳({:+.3} cyc)", size_cycles),
            CandidateStatus::Rejected { reason } => format!("已否定({})", reason),
        }
    }
}
