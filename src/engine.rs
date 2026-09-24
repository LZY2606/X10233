//! 派生引擎：多频组合、三检测器候选、钟跳/周跳分离、弧段、局部过期。
//!
//! 不变量：
//! - 电离层组合(GF)、Melbourne-Wubbena(MW)、时间差分(TD/DOP) 各自独立给出候选，
//!   不以任一检测器当真值；
//! - 整机钟跳与单星周跳分开（钟跳是所有卫星同步的码+相台阶）；
//! - 修复只写 offsets 偏移层，绝不回写原始载波。

use crate::db::Db;
use crate::domain::{wavelength, EffObs, Params, Snapshot, ALGO_VERSION};
use crate::id::id;
use rusqlite::params;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MISSING_PAIR: f64 = 0.0;

#[derive(Debug, Clone)]
struct Pt {
    epoch_micros: i64,
    obs: EffObs,
    pseudo_m: f64,
    phase_c: f64,
    doppler_hz: f64,
    l1: f64,
    l2: f64,
    f1: f64,
    f2: f64,
}

struct SatSeries {
    sat: String,
    /// band -> epoch -> 点
    by_band: BTreeMap<i64, BTreeMap<i64, Pt>>,
    epochs: Vec<i64>,
}

#[derive(Debug, Clone, Default)]
struct DetVote {
    preferred: Option<(i64, i64)>,
    residual: Option<f64>,
    unit: String,
    available: bool,
    detail: String,
}

#[derive(Debug, Clone)]
struct CandidateOut {
    epoch_micros: i64,
    n1: i64,
    n2: i64,
    kind: &'static str,
    score: f64,
    consensus: String,
    detectors_ok: Vec<&'static str>,
    votes: HashMap<&'static str, DetVote>,
}

#[derive(Debug, Clone)]
struct JudgmentOut {
    sat: String,
    epoch_micros: Option<i64>,
    code: String,
    message: String,
    detail: serde_json::Value,
}

pub struct RunOutputs {
    pub combos: Vec<ComboRow>,
    pub arcs: Vec<ArcRow>,
    pub candidates: Vec<CandidateRow>,
    pub judgments: Vec<JudgmentOut>,
}

#[derive(Debug, Clone)]
pub struct ComboRow {
    pub epoch_micros: i64,
    pub sat: String,
    pub combo: &'static str,
    pub value: Option<f64>,
    pub unit: &'static str,
    pub status: String,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ArcRow {
    pub sat: String,
    pub band: i64,
    pub start_micros: i64,
    pub end_micros: i64,
    pub end_reason: String,
}

#[derive(Debug, Clone)]
pub struct CandidateRow {
    pub epoch_micros: i64,
    pub sat: String,
    pub n1: Option<i64>,
    pub n2: Option<i64>,
    pub kind: String,
    pub score: f64,
    pub consensus: String,
    pub detectors_ok: Vec<String>,
    pub votes: HashMap<String, DetVote>,
    pub reject_reason: String,
}

fn build_series(snap: &Snapshot) -> BTreeMap<String, SatSeries> {
    let mut out: BTreeMap<String, SatSeries> = BTreeMap::new();
    let mut epochs_all: BTreeSet<i64> = BTreeSet::new();
    for o in &snap.obs {
        let (p, ph, d) = match (o.pseudo_m, o.phase_c, o.doppler_hz) {
            (Some(p), Some(ph), Some(d)) => (p, ph, d),
            _ => continue,
        };
        let f = match snap.frequency(&o.sat, o.band) {
            Some(f) => f,
            None => continue,
        };
        let lam = wavelength(f);
        let s = out.entry(o.sat.clone()).or_insert_with(|| SatSeries {
            sat: o.sat.clone(),
            by_band: BTreeMap::new(),
            epochs: Vec::new(),
        });
        s.by_band.entry(o.band).or_default().insert(
            o.epoch_micros,
            Pt {
                epoch_micros: o.epoch_micros,
                obs: o.clone(),
                pseudo_m: p,
                phase_c: ph,
                doppler_hz: d,
                l1: 0.0,
                l2: 0.0,
                f1: 0.0,
                f2: 0.0,
            },
        );
        epochs_all.insert(o.epoch_micros);
        // 在点上记录该星两个主频率
        let _ = lam;
    }
    for s in out.values_mut() {
        s.epochs = epochs_all.iter().copied().collect();
    }
    out
}

fn annotate_freqs(series: &mut BTreeMap<String, SatSeries>, snap: &Snapshot) {
    for s in series.values_mut() {
        let f1 = snap.frequency(&s.sat, 1).unwrap_or(crate::domain::GPS_F1);
        let f2 = snap.frequency(&s.sat, 2).unwrap_or(crate::domain::GPS_F2);
        let (l1, l2) = (wavelength(f1), wavelength(f2));
        for map in s.by_band.values_mut() {
            for pt in map.values_mut() {
                pt.f1 = f1;
                pt.f2 = f2;
                pt.l1 = l1;
                pt.l2 = l2;
            }
        }
    }
}
