//! 派生层：电离层组合(GF)、Melbourne-Wubbena(MW)、时间差分(TD) 三个独立检测器。
//! 任一检测器都不是真值；全部合法候选、拒绝原因、未知态作为一等数据保留。

use super::model::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// 算法版本：检测器规则与 RINEX 频率表的版本标签。
pub const ALGORITHM_VERSION: &str = "cycle-slip-arc-detectors-1.0.0";
pub const CODE_TABLE_VERSION: &str = "rinex3-code-table-1";
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

/// GPS L1/L2 中心频率（Hz），用于波长。
pub const GPS_F: [f64; 2] = [1_575_420_000.0, 1_227_600_000.0];

/// 检测器参数。每个参数版本经指纹化后写入派生结果；参数修订只让引用它的检测器结果过期。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// 电离层组合历元间跳变门限（米）。
    pub gf_jump_threshold_m: f64,
    /// MW 宽巷模糊度跳变门限（周）。
    pub mw_jump_threshold_cycle: f64,
    /// TD 残差门限（周）；不等间隔历元按 sqrt(dt/dt0) 放大。
    pub td_jump_threshold_cycle: f64,
    /// 名义采样间隔（秒）。
    pub td_nominal_dt_s: f64,
    /// 允许做 TD 的最大间隔（秒）；超过则该相邻对只给“未知”。
    pub td_max_gap_s: f64,
    /// 视为弧段断裂的最大无观测间隔（秒）。
    pub arc_break_gap_s: f64,
    /// 多候选并列阈值：与最佳整数候选差距（周）不超过该值则全部保留。
    pub tie_tolerance_cycle: f64,
    /// 候选枚举范围（每个频点 ±N 周）。
    pub hypothesis_search_radius: i64,
    /// 判定“整星钟跳（公共模式）”所需最少卫星数。
    pub clock_common_min_sats: usize,
    /// 公共模式幅度门限（周）。
    pub clock_common_threshold_cycle: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            gf_jump_threshold_m: 0.05,
            mw_jump_threshold_cycle: 1.0,
            td_jump_threshold_cycle: 0.25,
            td_nominal_dt_s: 60.0,
            td_max_gap_s: 600.0,
            arc_break_gap_s: 120.0,
            tie_tolerance_cycle: 0.25,
            hypothesis_search_radius: 10,
            clock_common_min_sats: 3,
            clock_common_threshold_cycle: 0.5,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let s = serde_json::to_string(self).unwrap_or_default();
        crate::infra::hash::short_hash(&s)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    Gf,
    Mw,
    Td,
    ClockCommon,
    Lli,
}

impl Detector {
    pub fn code(&self) -> &'static str {
        match self {
            Detector::Gf => "gf",
            Detector::Mw => "mw",
            Detector::Td => "td",
            Detector::ClockCommon => "clock_common",
            Detector::Lli => "lli",
        }
    }
    /// 该检测器依赖哪些参数键（参数修订时只让相关检测器过期）。
    pub fn param_keys(&self) -> &'static [&'static str] {
        match self {
            Detector::Gf => &["gf_jump_threshold_m", "hypothesis_search_radius", "tie_tolerance_cycle"],
            Detector::Mw => &["mw_jump_threshold_cycle", "hypothesis_search_radius", "tie_tolerance_cycle"],
            Detector::Td => &[
                "td_jump_threshold_cycle",
                "td_nominal_dt_s",
                "td_max_gap_s",
                "hypothesis_search_radius",
                "tie_tolerance_cycle",
            ],
            Detector::ClockCommon => &["clock_common_min_sats", "clock_common_threshold_cycle", "td_nominal_dt_s"],
            Detector::Lli => &[],
        }
    }
}

/// 一个合法修复假设：两个频点的整周偏移（只存偏移层）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hypothesis {
    pub offset_b1_cycle: i64,
    pub offset_b2_cycle: i64,
    /// 相对该检测器残差的失配（周，越小越好）。
    pub mismatch_cycle: f64,
    pub score: f64,
    pub tied: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResidualPoint {
    pub epoch_index: usize,
    pub t_seconds: f64,
    pub value: f64,
    pub unit: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    /// 有至少一个修复候选。
    Candidate,
    /// 检测器发现异常但无法唯一/无法形成合法候选（如间隔过长、单频）。
    Unknown,
    /// 被规则拒绝（典型：LLI 置位但无组合证据）。
    Rejected,
    /// 无异常。
    Clean,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Pending,
    Accepted,
    RejectedByHuman,
    RejectedByRule,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    /// 跨重算/导入导出稳定身份。
    pub id: String,
    pub detector: Detector,
    pub sat: String,
    pub band: u8,
    /// 异常所在“之后”历元（即 from->to 的 to）。
    pub epoch_index: usize,
    pub from_epoch_index: usize,
    pub state: CandidateState,
    pub resolution: Resolution,
    pub hypotheses: Vec<Hypothesis>,
    /// 残差跳变幅度。
    pub jump: Quantity,
    pub threshold: Quantity,
    pub dt_seconds: Quantity,
    pub lli_set: bool,
    pub reject_reasons: Vec<String>,
    pub diagnostics: Vec<String>,
    /// 直接来源槽位 id 集合（局部过期用）。
    pub source_slot_ids: Vec<String>,
    /// 计算该候选的算法版本。
    pub algorithm_version: String,
    /// 参数指纹。
    pub param_fingerprint: String,
    /// 弧段 id。
    pub arc_id: String,
    /// 是否已被更新源数据/参数修订淘汰（旧候选仍可查看但不能冒充当前）。
    pub stale: bool,
    /// 淘汰原因。
    pub stale_reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArcBoundaryKind {
    Start,
    DataGap,
    Slip,
    End,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArcBoundary {
    pub epoch_index: usize,
    pub kind: ArcBoundaryKind,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Arc {
    pub id: String,
    pub sat: String,
    pub band: u8,
    pub start_epoch_index: usize,
    pub end_epoch_index: usize,
    pub boundaries: Vec<ArcBoundary>,
    /// 弧段内历元（槽位 id 形式的直接来源）。
    pub slot_ids: Vec<String>,
}

/// 与单星周跳严格分开的整机钟跳事件。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClockJump {
    pub id: String,
    pub epoch_index: usize,
    pub from_epoch_index: usize,
    /// 公共模式偏移（周，在 L1 尺度）。
    pub common_shift_cycle: f64,
    pub affected_sats: Vec<String>,
    pub sources: Vec<String>, // external event id 或 "td_common_mode"
    pub state: CandidateState,
}

/// 单频点序列上的规范化观测。
#[derive(Clone, Debug)]
struct Series {
    sat: String,
    band: u8,
    epochs: Vec<usize>,
    t: Vec<f64>,
    p: Vec<f64>,
    l: Vec<f64>,
    d: Vec<Option<f64>>,
    lli: Vec<bool>,
    slot_ids: Vec<String>,
}

#[derive(Clone, Debug)]
struct DataSet {
    station: String,
    times: BTreeMap<usize, f64>,
    /// (sat,band) -> 序列
    series: BTreeMap<(String, u8), Series>,
    sats_per_epoch: BTreeMap<usize, Vec<String>>,
    /// 外部钟跳事件：epoch_index -> events
    clock_external: BTreeMap<usize, Vec<ClockEventInput>>,
    /// 槽位 id：(epoch_index, sat)
    slot_id: BTreeMap<(usize, String), String>,
}

pub fn slot_id(station: &str, epoch_index: usize, sat: &str) -> String {
    format!("slot:{}.{}@{}", station, epoch_index, sat)
}

/// 一次完整派生计算的结果。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunResult {
    pub run_id: String,
    pub algorithm_version: String,
    pub param_fingerprint: String,
    pub station: String,
    pub candidates: Vec<Candidate>,
    pub arcs: Vec<Arc>,
    pub clock_jumps: Vec<ClockJump>,
    /// GF / MW 组合残差时间序列（界面多频组合残差图）。
    pub gf_series: BTreeMap<String, Vec<ResidualPoint>>,
    pub mw_series: BTreeMap<String, Vec<ResidualPoint>>,
}

fn wavelength_hz(f: f64) -> f64 {
    SPEED_OF_LIGHT_M_S / f
}

/// 合并一个站点全部观测提交后的规范化数据集（按 (卫星,频点) 组织）。
pub fn build_dataset(
    station: &str,
    commits: &[(String, ParsedImport)],
    clock_events: &[ClockEventInput],
) -> DataSet {
    let mut ds = DataSet {
        station: station.to_string(),
        times: BTreeMap::new(),
        series: BTreeMap::new(),
        sats_per_epoch: BTreeMap::new(),
        clock_external: BTreeMap::new(),
        slot_id: BTreeMap::new(),
    };
    // 提交不可覆盖；后续提交只追加新槽位。槽位身份由 (站, 全局历元, 卫星) 决定。
    for (_commit_id, parsed in commits {
        for ep in &parsed.epochs {
            ds.times.entry(ep.epoch_index).or_insert(ep.t_seconds.value);
            for se in &ep.sats {
                ds.slot_id
                    .entry((ep.epoch_index, se.sat.clone()))
                    .or_insert_with(|| slot_id(station, ep.epoch_index, &se.sat));
                ds.sats_per_epoch.entry(ep.epoch_index).or_default().push(se.sat.clone());
                let mut per_band: BTreeMap<u8, (Option<f64>, Option<f64>, Option<f64>, Option<u8>)> = BTreeMap::new();
                for f in &se.fields {
                    if let Some(b) = f.band {
                        let entry = per_band.entry(b).or_default();
                        match f.kind {
                            ObsKind::Pseudorange => entry.0 = f.value_si,
                            ObsKind::Phase => {
                                entry.1 = f.value_si;
                                if f.lli_lost_lock() {
                                    entry.3 = Some(f.lli.unwrap_or(1));
                                }
                            }
                            ObsKind::Doppler => entry.2 = f.value_si,
                        }
                    }
                }
                for (band, (p, l, d, lli)) in per_band {
                    let key = (se.sat.clone(), band);
                    let s = ds.series.entry(key).or_insert_with(|| Series {
                        sat: se.sat.clone(),
                        band,
                        epochs: vec![],
                        t: vec![],
                        p: vec![],
                        l: vec![],
                        d: vec![],
                        lli: vec![],
                        slot_ids: vec![],
                    });
                    s.epochs.push(ep.epoch_index);
                    s.t.push(ep.t_seconds.value);
                    s.p.push(p.unwrap_or(f64::NAN));
                    s.l.push(l.unwrap_or(f64::NAN));
                    s.d.push(d);
                    s.lli.push(lli.is_some());
                    s.slot_ids.push(slot_id(station, ep.epoch_index, &se.sat));
                }
            }
        }
    }
    for v in ds.series.values_mut() {
        // 同历元同频点理论上只有一条；按历元排序保证顺序稳定。
        let mut idx: Vec<usize> = (0..v.epochs.len()).collect();
        idx.sort_by_key(|&k| (v.epochs[k], v.slot_ids[k].clone()));
        let mut s = Series {
            sat: v.sat.clone(),
            band: v.band,
            epochs: vec![],
            t: vec![],
            p: vec![],
            l: vec![],
            d: vec![],
            lli: vec![],
            slot_ids: vec![],
        };
        for k in idx {
            s.epochs.push(v.epochs[k]);
            s.t.push(v.t[k]);
            s.p.push(v.p[k]);
            s.l.push(v.l[k]);
            s.d.push(v.d[k]);
            s.lli.push(v.lli[k]);
            s.slot_ids.push(v.slot_ids[k].clone());
        }
        *v = s;
    }
    for e in clock_events {
        ds.clock_external.entry(e.epoch_index).or_default().push(e.clone());
    }
    ds
}

fn score_of(mismatch_cycle: f64, tol: f64) -> f64 {
    let x = mismatch_cycle.abs() / tol.max(1e-9);
    1.0 / (1.0 + x * x)
}

fn build_arcs(ds: &DataSet, params: &DetectorParams) -> Vec<Arc> {
    let mut arcs = Vec::new();
    for ((sat, band), s) in &ds.series {
        let mut start = 0usize;
        let mut boundaries = vec![ArcBoundary {
            epoch_index: s.epochs[0],
            kind: ArcBoundaryKind::Start,
            reason: "series_start".into(),
        }];
        for k in 1..s.epochs.len() {
            let dt = s.t[k] - s.t[k - 1];
            if dt > params.arc_break_gap_s {
                boundaries.push(ArcBoundary {
                    epoch_index: s.epochs[k - 1],
                    kind: ArcBoundaryKind::End,
                    reason: format!("data_gap:{dt:.0}s"),
                });
                boundaries.push(ArcBoundary {
                    epoch_index: s.epochs[k],
                    kind: ArcBoundaryKind::DataGap,
                    reason: format!("data_gap:{dt:.0}s"),
                });
                let arc_id = format!("arc:{}:{}:B{}:e{}", ds.station, sat, band, s.epochs[start]);
                arcs.push(Arc {
                    id: arc_id,
                    sat: sat.clone(),
                    band: *band,
                    start_epoch_index: s.epochs[start],
                    end_epoch_index: s.epochs[k - 1],
                    boundaries: std::mem::take(&mut boundaries),
                    slot_ids: s.slot_ids[start..k].to_vec(),
                });
                start = k;
                boundaries.push(ArcBoundary {
                    epoch_index: s.epochs[k],
                    kind: ArcBoundaryKind::Start,
                    reason: format!("data_gap:{dt:.0}s"),
                });
            }
        }
        boundaries.push(ArcBoundary {
            epoch_index: s.epochs[s.epochs.len() - 1],
            kind: ArcBoundaryKind::End,
            reason: "series_end".into(),
        });
        let arc_id = format!("arc:{}:{}:B{}:e{}", ds.station, sat, band, s.epochs[start]);
        arcs.push(Arc {
            id: arc_id,
            sat: sat.clone(),
            band: *band,
            start_epoch_index: s.epochs[start],
            end_epoch_index: s.epochs[s.epochs.len() - 1],
            boundaries,
            slot_ids: s.slot_ids[start..].to_vec(),
        });
    }
    arcs
}

fn arc_id_for(station: &str, sat: &str, band: u8, epoch: usize) -> String {
    format!("arc:{station}:{sat}:B{band}:e{epoch}")
}

fn cand_id(station: &str, det: Detector, sat: &str, band: u8, epoch: usize) -> String {
    format!("cand:{station}:{}:{}:B{}:e{}", det.code(), sat, band, epoch)
}

fn empty_candidate(
    id: String,
    det: Detector,
    sat: String,
    band: u8,
    epoch_index: usize,
    from_epoch_index: usize,
    state: CandidateState,
    jump: Quantity,
    threshold: Quantity,
    dt: Quantity,
    lli_set: bool,
    source_slot_ids: Vec<String>,
    arc_id: String,
    pf: &str,
) -> Candidate {
    Candidate {
        id,
        detector: det,
        sat,
        band,
        epoch_index,
        from_epoch_index,
        state,
        resolution: Resolution::Pending,
        hypotheses: vec![],
        jump,
        threshold,
        dt_seconds: dt,
        lli_set,
        reject_reasons: vec![],
        diagnostics: vec![],
        source_slot_ids,
        algorithm_version: ALGORITHM_VERSION.to_string(),
        param_fingerprint: pf.to_string(),
        arc_id,
        stale: false,
        stale_reasons: vec![],
    }
}


#[allow(clippy::too_many_arguments)]
fn make_hypotheses(mismatches: Vec<(i64, i64, f64)>, params: &DetectorParams) -> Vec<Hypothesis> {
    if mismatches.is_empty() {
        return vec![];
    }
    let best = mismatches.iter().map(|m| m.2.abs()).fold(f64::INFINITY, f64::min);
    mismatches
        .into_iter()
        .map(|(n1, n2, mm)| {
            let tied = (mm.abs() - best).abs() <= params.tie_tolerance_cycle + 1e-9
                || (best <= params.tie_tolerance_cycle && mm.abs() <= params.tie_tolerance_cycle);
            Hypothesis {
                offset_b1_cycle: n1,
                offset_b2_cycle: n2,
                mismatch_cycle: mm,
                score: score_of(mm, params.tie_tolerance_cycle),
                tied,
            }
        })
        .collect()
}

struct DualPoint {
    epoch: usize,
    t: f64,
    p1: f64,
    p2: f64,
    l1: f64,
    l2: f64,
    lli: bool,
    slot: String,
}

fn align_dual(s1: &Series, s2: &Series) -> Vec<DualPoint> {
    let mut out = Vec::new();
    let mut j = 0usize;
    for k in 0..s1.epochs.len() {
        while j < s2.epochs.len() && s2.epochs[j] < s1.epochs[k] {
            j += 1;
        }
        if j < s2.epochs.len() && s2.epochs[j] == s1.epochs[k] {
            if s1.l[k].is_finite()
                && s2.l[j].is_finite()
                && s1.p[k].is_finite()
                && s2.p[j].is_finite()
            {
                out.push(DualPoint {
                    epoch: s1.epochs[k],
                    t: s1.t[k],
                    p1: s1.p[k],
                    p2: s2.p[j],
                    l1: s1.l[k],
                    l2: s2.l[j],
                    lli: s1.lli[k] || s2.lli[j],
                    slot: s1.slot_ids[k].clone(),
                });
            }
        }
    }
    out
}

/// 枚举满足 GF 整周关系 n1*lam1 - n2*lam2 ≈ -gf_delta 的全部候选。
fn gf_hypotheses(gf_delta_m: f64, params: &DetectorParams) -> Vec<(i64, i64, f64)> {
    let lam1 = wavelength_hz(GPS_F[0]);
    let lam2 = wavelength_hz(GPS_F[1]);
    let mut out = Vec::new();
    let r = params.hypothesis_search_radius;
    for n1 in -r..=r {
        for n2 in -r..=r {
            if n1 == 0 && n2 == 0 {
                continue;
            }
            let predicted = n1 as f64 * lam1 - n2 as f64 * lam2;
            let mm = (-gf_delta_m - predicted) / lam1; // 用 L1 周归一
            if mm.abs() <= params.tie_tolerance_cycle + 0.5 {
                out.push((n1, n2, mm));
            }
        }
    }
    out
}

fn mw_nw_cycles(pt: &DualPoint) -> f64 {
    let lam1 = wavelength_hz(GPS_F[0]);
    let lam2 = wavelength_hz(GPS_F[1]);
    let ln = lam2 / lam1;
    // 宽巷组合：以宽巷波长 lam_w = lam1*lam2/(lam1-lam2) 归一的整周模糊度。
    let lam_w = lam1 * lam2 / (lam1 - lam2);
    let wide_metres = (pt.l1 - ln * pt.l2) * lam1 - (pt.p1 - ln * pt.p2);
    wide_metres / lam_w
}

/// 对每颗同时有 B1/B2 的卫星跑 GF 与 MW。
fn run_gf_mw(
    ds: &DataSet,
    params: &DetectorParams,
    pf: &str,
    candidates: &mut Vec<Candidate>,
    gf_series: &mut BTreeMap<String, Vec<ResidualPoint>>,
    mw_series: &mut BTreeMap<String, Vec<ResidualPoint>>,
) {
    let sats: BTreeSet<String> = ds
        .series
        .keys()
        .filter(|(_, b)| *b == 1)
        .map(|(s, _)| s.clone())
        .collect();
    for sat in sats {
        let k1 = (sat.clone(), 1u8);
        let k2 = (sat.clone(), 2u8);
        let (s1, s2) = match (ds.series.get(&k1), ds.series.get(&k2)) {
            (Some(a), Some(b)) => (a, b),
            _ => continue, // 单频卫星不跑 GF/MW
        };
        let dual = align_dual(s1, s2);
        let mut gf_pts = Vec::new();
        let mut mw_pts = Vec::new();
        for pt in &dual {
            let lam1 = wavelength_hz(GPS_F[0]);
            let lam2 = wavelength_hz(GPS_F[1]);
            let gf = (pt.l1 * lam1 - pt.p1) - (pt.l2 * lam2 - pt.p2);
            gf_pts.push(ResidualPoint {
                epoch_index: pt.epoch,
                t_seconds: pt.t,
                value: gf,
                unit: "metre".into(),
            });
            mw_pts.push(ResidualPoint {
                epoch_index: pt.epoch,
                t_seconds: pt.t,
                value: mw_nw_cycles(pt),
                unit: "cycle_widelane".into(),
            });
        }
        gf_series.insert(sat.clone(), gf_pts);
        mw_series.insert(sat.clone(), mw_pts.clone());

        // GF 历元间跳变（不等间隔不放大：电离层残差随时间缓慢，跳变与间隔无关）。
        for k in 1..dual.len() {
            let a = &dual[k - 1];
            let b = &dual[k];
            let dt = b.t - a.t;
            let gf_a = (a.l1 * wavelength_hz(GPS_F[0]) - a.p1)
                - (a.l2 * wavelength_hz(GPS_F[1]) - a.p2);
            let gf_b = (b.l1 * wavelength_hz(GPS_F[0]) - b.p1)
                - (b.l2 * wavelength_hz(GPS_F[1]) - b.p2);
            let delta = gf_b - gf_a;
            let id = cand_id(&ds.station, Detector::Gf, &sat, 1, b.epoch);
            let mut c = empty_candidate(
                id,
                Detector::Gf,
                sat.clone(),
                1,
                b.epoch,
                a.epoch,
                CandidateState::Clean,
                Quantity::new(delta, "metre"),
                Quantity::new(params.gf_jump_threshold_m, "metre"),
                Quantity::new(dt, "second"),
                b.lli,
                vec![a.slot.clone(), b.slot.clone()],
                arc_id_for(&ds.station, &sat, 1, b.epoch),
                pf,
            );
            if delta.abs() > params.gf_jump_threshold_m {
                let hyps = gf_hypotheses(delta, params);
                c.hypotheses = make_hypotheses(hyps, params);
                c.state = CandidateState::Candidate;
            }
            candidates.push(c);
        }

        // MW：对宽巷模糊度做递推均值，超过门限则给候选。
        if mw_pts.len() >= 2 {
            let mut mean = mw_pts[0].value;
            let mut count = 1usize;
            for k in 1..mw_pts.len() {
                let a = &dual[k - 1];
                let b = &dual[k];
                let dt = b.t - a.t;
                let delta = mw_pts[k].value - mean;
                let id = cand_id(&ds.station, Detector::Mw, &sat, 1, b.epoch);
                let mut c = empty_candidate(
                    id,
                    Detector::Mw,
                    sat.clone(),
                    1,
                    b.epoch,
                    a.epoch,
                    CandidateState::Clean,
                    Quantity::new(delta, "cycle_widelane"),
                    Quantity::new(params.mw_jump_threshold_cycle, "cycle_widelane"),
                    Quantity::new(dt, "second"),
                    b.lli,
                    vec![a.slot.clone(), b.slot.clone()],
                    arc_id_for(&ds.station, &sat, 1, b.epoch),
                    pf,
                );
                if delta.abs() > params.mw_jump_threshold_cycle {
                    let r = params.hypothesis_search_radius;
                    let mut hyps = Vec::new();
                    for n in 1..=r {
                        for &sgn in &[-1i64, 1] {
                            let nn = sgn * n;
                            hyps.push((nn, 0, delta - nn as f64));
                        }
                    }
                    c.hypotheses = make_hypotheses(hyps, params);
                    c.state = CandidateState::Candidate;
                }
                candidates.push(c);
                mean += (mw_pts[k].value - mean) / (count + 1) as f64;
                count += 1;
            }
        }
    }
}

/// TD：逐频点载波历元差分。不等间隔按 sqrt(dt/dt0) 放大门限；
/// 间隔超过 td_max_gap_s 时不猜整数，只给“未知”；钟跳历元上的公共模式候选被拒绝并归并到 ClockJump。
fn run_td(
    ds: &DataSet,
    params: &DetectorParams,
    pf: &str,
    candidates: &mut Vec<Candidate>,
    clock_jumps: &mut Vec<ClockJump>,
    clock_epochs: &BTreeSet<usize>,
) {
    // 1) 收集每个相邻对的公共模式残差（L1 周），判定整机钟跳。
    let mut common: BTreeMap<(usize, usize), Vec<(String, f64, f64, String, String)>> = BTreeMap::new();
    for ((sat, band), s) in &ds.series {
        if *band != 1 {
            continue;
        }
        for k in 1..s.epochs.len() {
            let dt = s.t[k] - s.t[k - 1];
            if !s.l[k].is_finite() || !s.l[k - 1].is_finite() || dt > params.td_max_gap_s + 1e-9 {
                continue;
            }
            let resid = s.l[k] - s.l[k - 1] - (s.d[k].unwrap_or(0.0) * dt);
            common
                .entry((s.epochs[k - 1], s.epochs[k]))
                .or_default()
                .push((sat.clone(), resid, dt, s.slot_ids[k - 1].clone(), s.slot_ids[k].clone()));
        }
    }
    for ((e0, e1), vals) in common {
        if vals.len() < params.clock_common_min_sats {
            continue;
        }
        let dt = vals[0].2;
        let scale = (dt / params.td_nominal_dt_s).sqrt();
        let thresh = params.clock_common_threshold_cycle * scale;
        // 外部记录的钟跳直接锚定；否则用多数卫星同号大残差判定。
        let external = ds.clock_external.get(&e1).cloned().unwrap_or_default();
        let big: Vec<&(String, f64, f64, String, String)> = vals
            .iter()
            .filter(|(_, r, _, _, _)| r.abs() > thresh)
            .collect();
        let is_clock = !external.is_empty()
            || (big.len() >= params.clock_common_min_sats
                && {
                    let pos = big.iter().filter(|(_, r, _, _, _)| *r > 0.0).count();
                    let neg = big.len() - pos;
                    pos.min(neg) == 0 || pos.max(neg) * 10 >= big.len() * 8
                });
        if !is_clock {
            continue;
        }
        let mean = if big.is_empty() {
            external
                .first()
                .and_then(|ev| {
                    if ev.shift.unit == "second" {
                        Some(ev.shift.value * GPS_F[0])
                    } else if ev.shift.unit == "cycle" {
                        Some(ev.shift.value)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0)
        } else {
            big.iter().map(|(_, r, _, _, _)| *r).sum::<f64>() / big.len() as f64
        };
        let affected: Vec<String> = vals.iter().map(|(s, _, _, _, _)| s.clone()).collect();
        let mut sources: Vec<String> = external.iter().map(|e| e.event_id.clone()).collect();
        if big.len() >= params.clock_common_min_sats {
            sources.push("td_common_mode".into());
        }
        clock_jumps.push(ClockJump {
            id: format!("clock:{}:e{}", ds.station, e1),
            epoch_index: e1,
            from_epoch_index: e0,
            common_shift_cycle: mean,
            affected_sats: affected,
            sources,
            state: CandidateState::Candidate,
        });
        clock_epochs.insert(e1);
    }

    // 2) 逐频点 TD 候选。
    for ((sat, band), s) in &ds.series {
        for k in 1..s.epochs.len() {
            let dt = s.t[k] - s.t[k - 1];
            let id = cand_id(&ds.station, Detector::Td, sat, *band, s.epochs[k]);
            let sources = vec![s.slot_ids[k - 1].clone(), s.slot_ids[k].clone()];
            let aid = arc_id_for(&ds.station, sat, *band, s.epochs[k]);
            let base = |state: CandidateState, jump: f64, thresh: f64, unit: &str| {
                let mut c = empty_candidate(
                    id.clone(),
                    Detector::Td,
                    sat.clone(),
                    *band,
                    s.epochs[k],
                    s.epochs[k - 1],
                    state,
                    Quantity::new(jump, unit),
                    Quantity::new(thresh, unit),
                    Quantity::new(dt, "second"),
                    s.lli[k],
                    sources.clone(),
                    aid.clone(),
                    pf,
                );
                c
            };
            if !s.l[k].is_finite() || !s.l[k - 1].is_finite() {
                let mut c = base(CandidateState::Unknown, f64::NAN, params.td_jump_threshold_cycle, "cycle");
                c.diagnostics.push("phase_missing".into());
                candidates.push(c);
                continue;
            }
            if dt > params.td_max_gap_s + 1e-9 {
                let mut c = base(CandidateState::Unknown, f64::NAN, params.td_jump_threshold_cycle, "cycle");
                c.diagnostics.push(format!("gap_exceeds_max:{dt:.0}s"));
                candidates.push(c);
                continue;
            }
            let scale = (dt / params.td_nominal_dt_s).sqrt();
            let thresh = params.td_jump_threshold_cycle * scale;
            let resid = s.l[k] - s.l[k - 1] - (s.d[k].unwrap_or(0.0) * dt);
            let mut c = base(
                if resid.abs() > thresh { CandidateState::Candidate } else { CandidateState::Clean },
                resid,
                thresh,
                "cycle",
            );
            if resid.abs() > thresh {
                // 钟跳历元：该星该频点的 TD 不归为单星周跳。
                if clock_epochs.contains(&s.epochs[k]) {
                    c.state = CandidateState::Rejected;
                    c.resolution = Resolution::RejectedByRule;
                    c.reject_reasons.push("common_mode_receiver_clock_jump".into());
                } else {
                    let r = params.hypothesis_search_radius;
                    let mut hyps = Vec::new();
                    let (self_n, other_n) = if *band == 1 { (1u8, 2u8) } else { (2u8, 1u8) };
                    for n in 1..=r {
                        for &sgn in &[-1i64, 1] {
                            let nn = sgn * n;
                            let mm = resid - nn as f64;
                            let (n1, n2) =
                                if self_n == 1 { (nn, 0) } else { (0, nn) };
                            let _ = other_n;
                            hyps.push((n1, n2, mm));
                        }
                    }
                    c.hypotheses = make_hypotheses(hyps, params);
                    if c.hypotheses.is_empty() {
                        c.state = CandidateState::Unknown;
                    }
                }
            }
            candidates.push(c);
        }
    }
}

/// LLI 审阅位：置位但没有任何组合/TD 证据 => 误报拒绝；有证据则证据已体现在候选里。
fn run_lli(ds: &DataSet, params: &DetectorParams, pf: &str, candidates: &[Candidate], out: &mut Vec<Candidate>) {
    for ((sat, band), s) in &ds.series {
        for k in 0..s.epochs.len() {
            if !s.lli[k] {
                continue;
            }
            let id = format!("cand:{}:lli:{}:B{}:e{}", ds.station, sat, band, s.epochs[k]);
            let evidence = candidates
                .iter()
                .filter(|c| c.sat == *sat && c.band == *band && c.epoch_index == s.epochs[k])
                .any(|c| c.state == CandidateState::Candidate);
            let mut c = empty_candidate(
                id,
                Detector::Lli,
                sat.clone(),
                *band,
                s.epochs[k],
                if k > 0 { s.epochs[k - 1] } else { s.epochs[k] },
                if evidence { CandidateState::Clean } else { CandidateState::Rejected },
                Quantity::new(1.0, "flag"),
                Quantity::new(0.0, "flag"),
                Quantity::new(if k > 0 { s.t[k] - s.t[k - 1] } else { 0.0 }, "second"),
                true,
                vec![s.slot_ids[k].clone()],
                arc_id_for(&ds.station, sat, *band, s.epochs[k]),
                pf,
            );
            if evidence {
                c.diagnostics.push("lli_confirmed_by_combo".into());
            } else {
                c.resolution = Resolution::RejectedByRule;
                c.reject_reasons.push("lli_false_alarm_no_combo_evidence".into());
            }
            let _ = params;
            out.push(c);
        }
    }
}

/// 从规范化数据集计算一次完整派生。
pub fn compute_run(run_id: &str, station: &str, commits: &[(String, ParsedImport)], clock_events: &[ClockEventInput], params: &DetectorParams) -> RunResult {
    let ds = build_dataset(station, commits, clock_events);
    let pf = params.fingerprint();
    let arcs = build_arcs(&ds, params);
    let mut candidates = Vec::new();
    let mut gf_series = BTreeMap::new();
    let mut mw_series = BTreeMap::new();
    let mut clock_jumps = Vec::new();
    let mut clock_epochs = BTreeSet::new();
    run_gf_mw(&ds, params, &pf, &mut candidates, &mut gf_series, &mut mw_series);
    run_td(&ds, params, &pf, &mut candidates, &mut clock_jumps, &mut clock_epochs);
    run_lli(&ds, params, &pf, &candidates, &mut candidates);
    // 稳定排序：检测器、卫星、频点、历元。
    candidates.sort_by(|a, b| {
        a.detector
            .code()
            .cmp(b.detector.code())
            .then(a.sat.cmp(&b.sat))
            .then(a.band.cmp(&b.band))
            .then(a.epoch_index.cmp(&b.epoch_index))
    });
    RunResult {
        run_id: run_id.to_string(),
        algorithm_version: ALGORITHM_VERSION.to_string(),
        param_fingerprint: pf,
        station: station.to_string(),
        candidates,
        arcs,
        clock_jumps,
        gf_series,
        mw_series,
    }
}
