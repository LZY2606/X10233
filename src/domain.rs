//! 领域数据模型。
//!
//! 分层约束：
//! * `Raw*` 结构是原始层：按历元原样保存（含原始字符串表示），一经提交不可覆盖。
//! * `Derived*` 结构是派生层：携带算法版本、参数指纹、直接来源与单位/坐标基准。
//! * 修复只落在 `RepairOffset`（偏移层），绝不回写载波原始观测。

use serde::{Deserialize, Serialize};

/// 带单位的标量。原始字符串始终保留；无法转换时 value 为 None 且带诊断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quant {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String,
    pub diagnostic: Option<String>,
}

impl Quant {
    pub fn new(raw: impl Into<String>, value: Option<f64>, unit: impl Into<String>) -> Self {
        Self { raw: raw.into(), value, unit: unit.into(), diagnostic: None }
    }

    pub fn with_diag(mut self, msg: impl Into<String>) -> Self {
        self.diagnostic = Some(msg.into());
        self
    }

    pub fn meters(raw: &str) -> Self {
        match raw.trim().parse::<f64>() {
            Ok(v) => Quant::new(raw, Some(v), "m"),
            Err(e) => Quant::new(raw, None, "m").with_diag(format!("伪距/载波无法解析为米: {e}")),
        }
    }
}

/// 频率定义（单位与观测码原样保留）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Frequency {
    /// 稳定频率编号，例如 "L1"。
    pub id: String,
    pub code: String,
    pub freq_hz: f64,
    pub wavelength_m: f64,
}

/// 单颗卫星单个频率、单个历元的原始观测。
/// 字段缺失（频率缺失）时整个条目不出现，而不是填零。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawMeas {
    pub sv: String,
    pub epoch_idx: usize,
    pub freq_id: String,
    pub code: String,
    pub pseudorange: Quant,
    pub carrier_cycles: Quant,
    pub doppler_hz: Quant,
    pub lli: Option<u8>,
    pub lli_raw: String,
    pub cn0_db_hz: Quant,
}

/// 卫星状态（坐标带坐标基准）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatState {
    pub sv: String,
    pub epoch_idx: usize,
    pub datum: String,
    pub x_m: Quant,
    pub y_m: Quant,
    pub z_m: Quant,
    pub clock_bias_s: Quant,
}

/// 原始历元头。间隔不做等间隔假设。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEpoch {
    pub idx: usize,
    pub timestamp_raw: String,
    pub timestamp_epoch_s: Option<i64>,
    pub time_diagnostic: Option<String>,
    pub interval_s_from_prev: Option<f64>,
}

/// 整机钟跳事件（原始声明）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEvent {
    pub epoch_idx: usize,
    /// 原始声明的钟跳量级（毫秒），原样保存。
    pub jump_ms_raw: String,
    pub jump_ms: Option<f64>,
    pub source: String,
}

/// 一次导入批次的完整原始卡片集。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RawBatch {
    pub format_version: String,
    pub station_id: String,
    pub marker_name: String,
    pub receiver: String,
    pub frequencies: Vec<Frequency>,
    pub epochs: Vec<RawEpoch>,
    pub meas: Vec<RawMeas>,
    pub sat_states: Vec<SatState>,
    pub clock_events: Vec<ClockEvent>,
    /// 导入期诊断（致命错误与非致命诊断分开）。
    pub errors: Vec<ImportError>,
    pub diagnostics: Vec<ImportDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportError {
    pub line_no: usize,
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportDiagnostic {
    pub line_no: usize,
    pub code: String,
    pub message: String,
    pub raw_line: String,
}

/// 修订补丁：只修改一个直接来源，局部依赖才会过期。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Patch {
    /// 被替换测量的 (sv, epoch_idx, freq_id)。
    pub target_sv: String,
    pub target_epoch: usize,
    pub target_freq: String,
    pub replacement: RawMeas,
    pub reason: String,
}

/// 检测器/算法参数。参数修订产生新版本与指纹。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    pub schema_version: String,
    pub td_sigma_m: f64,
    pub gf_sigma_m: f64,
    pub mw_sigma_cycles: f64,
    pub doppler_sigma_hz: f64,
    pub lli_forces_candidate: bool,
    pub int_search_halfwidth: i64,
    /// 钟跳认定：同一历元达到该卫星数的共因跳变即归为整机钟跳。
    pub clock_common_sv_count: usize,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            schema_version: "detector-params/v1".into(),
            td_sigma_m: 0.02,
            gf_sigma_m: 0.025,
            mw_sigma_cycles: 0.25,
            doppler_sigma_hz: 0.05,
            lli_forces_candidate: true,
            int_search_halfwidth: 20,
            clock_common_sv_count: 2,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let body = serde_json::to_string(self).unwrap_or_default();
        format!("fp_{:016x}", fnv1a64(body.as_bytes()))
    }
}

pub(crate) fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 事件类型。事件流只追加，状态由重放得到。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    ProjectCreated {
        project_id: String,
        name: String,
    },
    RawCommitted {
        commit_id: String,
        parent_commit_id: Option<String>,
        batch: RawBatch,
    },
    ParamsRevised {
        from_fingerprint: String,
        params: DetectorParams,
        reason: String,
    },
    SourcePatched {
        patch: Patch,
    },
    CandidateAdjudicated {
        candidate_id: String,
        decision: Decision,
        note: String,
    },
    /// 冲突的人工决定：与既有采纳决定冲突，旧决定保留，新决定覆盖当前视图。
    DecisionConflict {
        prior_candidate_id: String,
        prior_decision: Decision,
        new_candidate_id: String,
        new_decision: Decision,
        note: String,
    },
    /// 决策撤销也写入历史（不删除旧事件）。
    DecisionReverted {
        reverted_candidate_id: String,
        reverted_decision: Decision,
        note: String,
    },
    /// 派生层重算事件（每次提交/修订/补丁/裁定后生成）。
    DerivedRegenerated {
        derivation_id: String,
        basis_event_seq: u64,
        algorithm_version: String,
        params_fingerprint: String,
        invalidated: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    Rejected,
    Withdrawn,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Accepted => "accepted",
            Decision::Rejected => "rejected",
            Decision::Withdrawn => "withdrawn",
        }
    }
}

/// 持久化事件信封。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    pub seq: u64,
    pub event_id: String,
    pub parent_event_id: Option<String>,
    pub occurred_at: String,
    pub actor: String,
    pub payload: DomainEvent,
}
