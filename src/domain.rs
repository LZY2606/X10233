//! 领域核心：从“某一时刻的事件序列”投影出当前可见的领域状态。
//!
//! 纯函数式重放（replay），不依赖数据库：
//! - 三个并列检测器：IONO（几何无关/电离层组合）、MW（Melbourne-Wübbena 宽巷）、
//!   TDI（带 Doppler 辅助的时间差分载波）。另用 CODE_TD 识别整机钟跳。
//! - 任何检测器都不是真值；它们各自提出候选，候选带得分与来源，无法唯一决定时
//!   保留全部合法候选（多个等分整周偏移）。
//! - 钟跳（receiver 级）与单星周跳严格分开；钟跳边不产生周跳弧段修订。
//! - 弧段按 站×星×频率 的连续历元组织；超过最大间隔即断开。
//! - 人工裁定只产生偏移层（offset overlay），不回写观测。
//!
//! 局部过期通过“自然键复用 + 指纹比对”实现，见 [`project_head`]。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::events::{DecisionPayload, Event, EventKind};
use crate::ids::stable_id;
use crate::ingest;
use crate::model::{FreqDef, Observation, SatState, VersionPayload};
use crate::ALGO_VERSION;

const C: f64 = 299_792_458.0;

/// 检测器/计算步骤的可判定状态。UNKNOWN 是一等状态，不强行二值化。
pub mod status {
    pub const OK: &str = "ok";
    pub const FIRED: &str = "fired";
    pub const CLOCK: &str = "receiver_clock";
    pub const BELOW_THRESHOLD: &str = "below_threshold";
    pub const GAP_TOO_LONG: &str = "gap_too_long";
    pub const FREQUENCY_MISSING: &str = "frequency_missing";
    pub const VALUE_UNUSABLE: &str = "value_unusable";
    pub const FIRST_IN_ARC: &str = "first_in_arc";
    pub const UNKNOWN: &str = "unknown";
}

/// 参数集合。每次参数修订都会生成参数指纹。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// 弧段最大历元间隔（秒），超过则断开弧段。
    pub max_gap_s: f64,
    /// 电离层组合残差阈值（m）。
    pub iono_threshold_m: f64,
    /// MW 宽巷残差阈值（cycle）。
    pub mw_threshold_cycle: f64,
    /// 时间差分载波残差阈值（cycle）。
    pub td_threshold_cycle: f64,
    /// 伪距时间差分（钟跳检测）阈值（m）。
    pub code_clock_threshold_m: f64,
    /// 钟跳相关判定：认为“同步”的时间窗（秒）。
    pub clock_sync_window_s: f64,
    /// 钟跳相关判定：至少多少条星频流同向跳变才算整机钟跳。
    pub clock_min_streams: usize,
    /// MW-only 等分支时，单频整周偏移的搜索半径（cycles）。
    pub mw_single_band_radius: i64,
    /// 判为周跳候选的最小支持检测器数。
    pub min_voting_detectors: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_gap_s: 30.0,
            iono_threshold_m: 0.30,
            mw_threshold_cycle: 1.0,
            td_threshold_cycle: 1.0,
            code_clock_threshold_m: 100.0,
            clock_sync_window_s: 0.001,
            min_voting_detectors: 1,
            clock_min_streams: 4,
            mw_single_band_radius: 2,
        }
    }
}

impl Params {
    /// 稳定、规范（键排序）的参数指纹。
    pub fn fingerprint(&self) -> String {
        let v = serde_json::to_value(self).expect("params serialize");
        let canon = canonical_json(&v);
        stable_id("pfp", &[ALGO_VERSION, &canon])
    }
    /// 各检测器关心的参数子集指纹 —— 用于“只使依赖它的结果过期”。
    pub fn fingerprint_for(&self, detector: &str) -> String {
        let keys: &[&str] = match detector {
            "arc" => &["max_gap_s"],
            "iono" => &["max_gap_s", "iono_threshold_m"],
            "mw" => &["max_gap_s", "mw_threshold_cycle", "mw_single_band_radius"],
            "tdi" => &["max_gap_s", "td_threshold_cycle"],
            "clock" => &["max_gap_s", "code_clock_threshold_m", "clock_sync_window_s", "clock_min_streams"],
            _ => &[],
        };
        let full = serde_json::to_value(self).unwrap();
        let mut obj = serde_json::Map::new();
        if let serde_json::Value::Object(m) = full {
            for k in keys {
                if let Some(v) = m.get(*k) {
                    obj.insert((*k).to_string(), v.clone());
                }
            }
        }
        stable_id(&format!("pfp-{}", detector), &[ALGO_VERSION, &canonical_json(&serde_json::Value::Object(obj))])
    }
}

/// 规范 JSON：对象键排序、无空白，保证指纹跨进程稳定。
pub fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let keys: BTreeSet<&String> = m.keys().collect();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        other => serde_json::to_string(other).unwrap(),
    }
}

/// 一条导入记录（replay 用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRecord {
    pub event_seq: i64,
    pub version_id: String,
    pub parent_version_id: Option<String>,
    pub payload: VersionPayload,
}

/// 原始观测（带稳定身份与版本/行号溯源）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsRow {
    pub id: String,
    pub version_id: String,
    pub line_no: u32,
    pub obs: Observation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatRow {
    pub id: String,
    pub version_id: String,
    pub state: SatState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockRow {
    pub id: String,
    pub version_id: String,
    pub epoch: String,
    pub raw: String,
    pub shift_seconds: Option<f64>,
    pub diagnostics: Vec<String>,
    pub line_no: u32,
}

/// 历元（全局排序）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochRow {
    pub id: String,
    pub epoch: String,
    pub ord: i64,
    pub dt_from_prev_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreqRow {
    pub band: String,
    pub def: FreqDef,
}
