//! 检测引擎: 时间差分(TD)、电离层组合(GF)、Melbourne-Wubbena(MW)
//! 各自给出候选; 融合层只做一致性求交, 不以任一检测器当真值。
//! 整机钟跳单独建模, 周跳修复只存偏移层, 不改原始载波。
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::id::content_id;
use crate::model::{standard_freq_hz, Epoch, ObsRow, Params};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    pub at: Epoch,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arc {
    pub id: String,
    pub sat: String,
    pub code: String,
    pub start: Epoch,
    pub end: Epoch,
    pub epoch_count: i64,
    pub boundaries: Vec<Boundary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub sat: String,
    pub from: Epoch,
    pub to: Epoch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub params_id: String,
    pub epoch: Epoch,
    pub kind: String,
    pub offset_m: f64,
    pub sat_count: i64,
    pub inputs_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateSet {
    pub id: String,
    pub edge_id: String,
    pub sat: String,
    pub from: Epoch,
    pub to: Epoch,
    pub detector: String,
    pub params_id: String,
    pub status: String, // unique | ambiguous | none | disputed | unavailable
    pub status_reason: Option<String>,
    pub inputs_fingerprint: String,
    pub engine_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub set_id: String,
    pub kind: String, // none | slip
    pub offsets: BTreeMap<String, i64>,
    pub score: f64,
    pub estimate: f64,
    pub residual: f64,
    pub residual_unit: String,
    pub tie: bool,
    pub rejected: bool,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rejection {
    pub id: String,
    pub candidate_id: String,
    pub reason_code: String,
    pub detail: String,
}

#[derive(Default, Debug, Clone)]
pub struct EngineOutput {
    pub arcs: Vec<Arc>,
    pub edges: Vec<Edge>,
    pub clock_events: Vec<ClockEvent>,
    pub sets: Vec<CandidateSet>,
    pub candidates: Vec<Candidate>,
    pub rejections: Vec<Rejection>,
}

type Dismissed = BTreeSet<(String, String, i64, i64)>;

pub struct EngineCtx<'a> {
    pub params: &'a Params,
    pub params_id: &'a str,
    pub params_fp: &'a str,
    pub dismissed_lli: &'a Dismissed,
}

#[derive(Clone)]
struct RowRef<'a> {
    row: &'a ObsRow,
}

fn band_of(code: &str) -> Option<char> {
    code.chars().nth(1)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// 引擎主入口(纯函数, 无 IO)。
pub fn run(rows: &[ObsRow], ctx: &EngineCtx) -> EngineOutput {
    let p = ctx.params;
    let mut out = EngineOutput::default();

    // 按 (卫星, 历元, 码) 组织
    let mut sats: BTreeSet<String> = BTreeSet::new();
    let mut by_sat: BTreeMap<String, Vec<&ObsRow>> = BTreeMap::new();
    for r in rows {
        if r.value.is_some() || r.diag.is_some() {
            sats.insert(r.sat.clone());
            by_sat.entry(r.sat.clone()).or_default().push(r);
        }
    }

    // ---------- 1) 弧段(仅载波码) ----------
    let mut arcs_by_sat_code: BTreeMap<(String, String), Vec<Arc>> = BTreeMap::new();
    for sat in &sats {
        let srows = by_sat.get(sat).cloned().unwrap_or_default();
        let mut codes: BTreeSet<String> = srows.iter().map(|r| r.code.clone()).collect();
        codes.retain(|c| c.starts_with('L'));
        for code in &codes {
            let mut series: Vec<(Epoch, i64)> = srows
                .iter()
                .filter(|r| &r.code == code && r.value.is_some())
                .map(|r| (r.epoch(), r.lli.unwrap_or(0)))
                .collect();
            series.sort_by_key(|(e, _)| e.key());
            series.dedup_by_key(|(e, _)| e.key());
            if series.is_empty() {
                continue;
            }
            let mut arcs: Vec<Arc> = Vec::new();
            let mut cur_start = 0usize;
            let mut boundaries: Vec<Boundary> = Vec::new();
            for k in 1..series.len() {
                let (e_prev, lli) = series[k];
                let dt = e_prev.minus(&series[k - 1].0);
                let mut reasons = Vec::new();
                if dt > p.max_gap_s + 1e-9 {
                    reasons.push("gap".to_string());
                }
                if lli & 1 == 1
                    && !ctx.dismissed_lli.contains(&(
                        sat.clone(),
                        code.clone(),
                        e_prev.week,
                        (e_prev.tow * 1000.0).round() as i64,
                    ))
                {
                    reasons.push("lli_flag".to_string());
                }
                if !reasons.is_empty() {
                    let seg = &series[cur_start..k];
                    arcs.push(Arc {
                        id: content_id("arc", &(sat, code, seg.first().unwrap().0.key(), seg.last().unwrap().0.key())),
                        sat: sat.clone(),
                        code: code.clone(),
                        start: seg.first().unwrap().0,
                        end: seg.last().unwrap().0,
                        epoch_count: seg.len() as i64,
                        boundaries: boundaries.clone(),
                    });
                    boundaries.push(Boundary { at: e_prev, reasons });
                    cur_start = k;
                }
            }
            let seg = &series[cur_start..];
            arcs.push(Arc {
                id: content_id("arc", &(sat, code, seg.first().unwrap().0.key(), seg.last().unwrap().0.key())),
                sat: sat.clone(),
                code: code.clone(),
                start: seg.first().unwrap().0,
                end: seg.last().unwrap().0,
                epoch_count: seg.len() as i64,
                boundaries: boundaries.clone(),
            });
            arcs_by_sat_code.insert((sat.clone(), code.clone()), arcs);
        }
    }
    for arcs in arcs_by_sat_code.values() {
        out.arcs.extend(arcs.iter().cloned());
    }

    // ---------- 2) 整机钟跳 ----------
    let mut global_epochs: BTreeSet<(i64, i64)> = BTreeSet::new();
    for r in rows {
        global_epochs.insert((r.week, (r.tow * 1000.0).round() as i64));
    }
    let global_epochs: Vec<(i64, f64)> =
        global_epochs.into_iter().map(|(w, ms)| (w, ms as f64 / 1000.0)).collect();
    let mut clock_at: BTreeMap<(i64, i64), ClockEvent> = BTreeMap::new();
    for pair in global_epochs.windows(2) {
        let (w0, t0) = pair[0];
        let (w1, t1) = pair[1];
        let dt = (w1 - w0) as f64 * 604_800.0 + (t1 - t0);
        let mut jumps: Vec<f64> = Vec::new();
        let mut used_rows: BTreeSet<String> = BTreeSet::new();
        for sat in &sats {
            let srows = by_sat.get(sat).cloned().unwrap_or_default();
            let find = |epoch_key: (i64, i64), pred: fn(&str) -> bool| -> Option<&ObsRow> {
                srows.iter().find(|r| {
                    r.epoch().key() == epoch_key && pred(&r.code) && r.value.is_some()
                }).copied()
            };
            let c0 = find((w0, (t0 * 1000.0).round() as i64), |c| {
                c.starts_with('C') && band_of(c) == Some('1')
            });
            let c1 = find((w1, (t1 * 1000.0).round() as i64), |c| {
                c.starts_with('C') && band_of(c) == Some('1')
            });
            let d0 = find((w0, (t0 * 1000.0).round() as i64), |c| {
                c.starts_with('D') && band_of(c) == Some('1')
            });
            let d1 = find((w1, (t1 * 1000.0).round() as i64), |c| {
                c.starts_with('D') && band_of(c) == Some('1')
            });
            if let (Some(c0), Some(c1), Some(d0), Some(d1)) = (c0, c1, d0, d1) {
                let (f1, _) = standard_freq_hz(sat, "L1C").unwrap_or((crate::model::F1_HZ, ""));
                let lam = crate::model::C_M_PER_S / f1;
                let dbar = (d0.value.unwrap() + d1.value.unwrap()) * 0.5;
                let jump = (c1.value.unwrap() - c0.value.unwrap()) + dbar * lam * dt;
                jumps.push(jump);
                used_rows.insert(c0.id.clone());
                used_rows.insert(c1.id.clone());
                used_rows.insert(d0.id.clone());
                used_rows.insert(d1.id.clone());
            }
        }
        if jumps.len() as i64 >= p.clock_min_sats {
            let m = median(jumps);
            if m.abs() >= p.clock_min_m {
                let fp = content_id("clkin", &(used_rows.iter().collect::<Vec<_>>(), ctx.params_fp));
                let e = Epoch { week: w1, tow: t1 };
                let ev = ClockEvent {
                    id: content_id("clk", &(e.key(), m, ctx.params_fp, "receiver")),
                    params_id: ctx.params_id.to_string(),
                    epoch: e,
                    kind: "receiver".to_string(),
                    offset_m: m,
                    sat_count: jumps.len() as i64,
                    inputs_fingerprint: fp,
                };
                clock_at.insert(e.key(), ev);
            }
        }
    }
    out.clock_events.extend(clock_at.values().cloned());

    // ---------- 3) 边与每边检测器 ----------
    for sat in &sats {
        let srows = by_sat.get(sat).cloned().unwrap_or_default();
        let l_codes: Vec<String> = {
            let mut cs: BTreeSet<String> =
                srows.iter().filter(|r| r.code.starts_with('L')).map(|r| r.code.clone()).collect();
            let mut v: Vec<String> = cs.drain().collect();
            v.sort_by_key(|c| (band_of(c).unwrap_or('Z'), c.clone()));
            v
        };
        let primary = match l_codes.iter().find(|c| band_of(c) == Some('1')).or_else(|| l_codes.first()) {
            Some(c) => c.clone(),
            None => continue,
        };
        let primary_arcs = arcs_by_sat_code.get(&(sat.clone(), primary.clone())).cloned().unwrap_or_default();
        let mut canonical_edges: Vec<Edge> = Vec::new();
        for arc in &primary_arcs {
            let mut epochs: Vec<Epoch> = srows
                .iter()
                .filter(|r| r.code == primary && r.value.is_some())
                .map(|r| r.epoch())
                .collect();
            epochs.sort_by_key(Epoch::key);
            epochs.dedup_by_key(Epoch::key);
            let eps: Vec<Epoch> = epochs
                .into_iter()
                .filter(|e| e.minus(&arc.start) >= -1e-9 && arc.end.minus(e) >= -1e-9)
                .collect();
            for w in eps.windows(2) {
                let (e0, e1) = (w[0], w[1]);
                let id = content_id("edge", &(sat, e0.key(), e1.key()));
                canonical_edges.push(Edge { id, sat: sat.clone(), from: e0, to: e1 });
            }
        }
        out.edges.extend(canonical_edges.iter().cloned());

        for edge in &canonical_edges {
            run_edge(edge, &primary, &l_codes, &srows, &arcs_by_sat_code, &clock_at, ctx, &mut out);
        }
    }
    out
}

fn row_at<'a>(
    srows: &'a [&'a ObsRow],
    e: Epoch,
    pred: impl Fn(&str) -> bool,
) -> Option<&'a ObsRow> {
    srows.iter().copied().find(|r| r.epoch().key() == e.key() && pred(&r.code))
}

fn in_same_arc(
    arcs: &BTreeMap<(String, String), Vec<Arc>>,
    sat: &str,
    code: &str,
    e0: Epoch,
    e1: Epoch,
) -> bool {
    match arcs.get(&(sat.to_string(), code.to_string())) {
        Some(list) => list
            .iter()
            .any(|a| e0.minus(&a.start) >= -1e-9 && a.end.minus(&e1) >= -1e-9),
        None => false,
    }
}

struct SetAcc {
    set: CandidateSet,
    candidates: Vec<Candidate>,
    rejections: Vec<Rejection>,
}

fn make_set(
    edge: &Edge,
    detector: &str,
    status: &str,
    reason: Option<String>,
    digest: &str,
    ctx: &EngineCtx,
) -> CandidateSet {
    CandidateSet {
        id: content_id("set", &(&edge.id, detector, digest)),
        edge_id: edge.id.clone(),
        sat: edge.sat.clone(),
        from: edge.from,
        to: edge.to,
        detector: detector.to_string(),
        params_id: ctx.params_id.to_string(),
        status: status.to_string(),
        status_reason: reason,
        inputs_fingerprint: digest.to_string(),
        engine_version: ctx.params.engine_version.clone(),
    }
}

fn make_candidate(
    set_id: &str,
    kind: &str,
    offsets: BTreeMap<String, i64>,
    score: f64,
    estimate: f64,
    residual: f64,
    unit: &str,
    tie: bool,
    rejected: bool,
    sources: Vec<String>,
) -> Candidate {
    let id = content_id("cnd", &(set_id, &offsets, kind));
    Candidate {
        id,
        set_id: set_id.to_string(),
        kind: kind.to_string(),
        offsets,
        score,
        estimate,
        residual,
        residual_unit: unit.to_string(),
        tie,
        rejected,
        sources,
    }
}

fn reject(candidate_id: &str, reason: &str, detail: String) -> Rejection {
    Rejection {
        id: content_id("rej", &(candidate_id, reason, &detail)),
        candidate_id: candidate_id.to_string(),
        reason_code: reason.to_string(),
        detail,
    }
}

fn edge_digest(
    mut used: BTreeSet<String>,
    ctx: &EngineCtx,
    clock_id: Option<&str>,
) -> String {
    if let Some(c) = clock_id {
        used.insert(c.to_string());
    }
    content_id("digest", &(used.into_iter().collect::<Vec<_>>(), ctx.params_fp, ctx.params.engine_version))
}

fn code_for(srows: &[&ObsRow], kind: char, band: char) -> Option<String> {
    let mut cs: Vec<String> = srows
        .iter()
        .map(|r| r.code.clone())
        .filter(|c| c.starts_with(kind) && band_of(c) == Some(band))
        .collect();
    cs.sort();
    cs.into_iter().next()
}

fn freq_of(sat: &str, srows: &[&ObsRow], kind: char, band: char) -> Option<(String, f64)> {
    let code = code_for(srows, kind, band)?;
    if kind == 'L' {
        let r = srows.iter().find(|r| r.code == code)?;
        return Some((code, r.freq_hz?));
    }
    let (f, _) = standard_freq_hz(sat, &format!("L{}{}", band, &code[2..]))?;
    Some((code, f))
}

#[allow(clippy::too_many_arguments)]
fn run_edge(
    edge: &Edge,
    primary: &str,
    l_codes: &[String],
    srows: &[&ObsRow],
    arcs: &BTreeMap<(String, String), Vec<Arc>>,
    clock_at: &BTreeMap<(i64, i64), ClockEvent>,
    ctx: &EngineCtx,
    out: &mut EngineOutput,
) {
    let p = ctx.params;
    let dt = edge.to.minus(&edge.from);
    let clock = clock_at.get(&edge.to.key());
    let clock_id = clock.map(|c| c.id.as_str());
    let mut used: BTreeSet<String> = BTreeSet::new();

    // ---------- TD(逐载波码) ----------
    let mut td_pools: Vec<(String, Vec<Candidate>, String)> = Vec::new(); // (code, cands, set_id)
    let mut bands: Vec<char> = l_codes.iter().filter_map(|c| band_of(c)).collect();
    bands.sort();
    bands.dedup();
    for band in &bands {
        let lcode = match code_for(srows, 'L', *band) {
            Some(c) if l_codes.contains(&c) => c,
            _ => continue,
        };
        let detector = format!("td:{}", lcode);
        let l0 = row_at(srows, edge.from, |c| c == lcode).and_then(|r| r.value.map(|v| (r, v)));
        let l1 = row_at(srows, edge.to, |c| c == lcode).and_then(|r| r.value.map(|v| (r, v)));
        let (l0row, v_l0) = match l0 { Some(x) => x, None => continue };
        let (l1row, v_l1) = match l1 { Some(x) => x, None => continue };
        let same_arc = in_same_arc(arcs, &edge.sat, &lcode, edge.from, edge.to);
        if !same_arc {
            let set = make_set(edge, &detector, "unavailable",
                Some(format!("arc_boundary_between:{}", lcode)),
                &edge_digest(used.clone(), ctx, clock_id), ctx);
            out.sets.push(set);
            continue;
        }
        let (dcode, _f) = match freq_of(&edge.sat, srows, 'D', *band) {
            Some(x) => x,
            None => {
                let digest = edge_digest(used.clone(), ctx, clock_id);
                let set = make_set(edge, &detector, "unavailable",
                    Some(format!("doppler_unavailable:band{}", band)), &digest, ctx);
                out.sets.push(set);
                continue;
            }
        };
        let d0 = row_at(srows, edge.from, |c| c == dcode).and_then(|r| r.value);
        let d1 = row_at(srows, edge.to, |c| c == dcode).and_then(|r| r.value);
        let (d0v, d1v) = match (d0, d1) { (Some(a), Some(b)) => (a, b), _ => {
            let digest = edge_digest(used.clone(), ctx, clock_id);
            let set = make_set(edge, &detector, "unavailable",
                Some(format!("doppler_unavailable:{}", dcode)), &digest, ctx);
            out.sets.push(set);
            continue;
        }};
        for r in [l0row, l1row] {
            used.insert(r.id.clone());
            if let Some(dd) = row_at(srows, edge.from, |c| c == dcode) { used.insert(dd.id.clone()); }
            if let Some(dd) = row_at(srows, edge.to, |c| c == dcode) { used.insert(dd.id.clone()); }
        }
        let digest = edge_digest(used.clone(), ctx, clock_id);
        let estimate = (v_l1 - v_l0) + (d0v + d1v) * 0.5 * dt; // cycles
        let lam = match l0row.freq_hz { Some(f) => crate::model::C_M_PER_S / f, None => continue };
        let mut cands: Vec<Candidate> = Vec::new();
        for n in -64..=64i64 {
            let dist = (estimate - n as f64).abs();
            if dist <= p.td_tol_cycles + 1e-12 {
                let mut off = BTreeMap::new();
                off.insert(lcode.clone(), n);
                let kind = if n == 0 { "none" } else { "slip" };
                let mut cand = make_candidate(
                    &content_id("set", &(&edge.id, &detector, &digest)),
                    kind, off,
                    dist + p.complexity_penalty * n.abs() as f64,
                    estimate, estimate - n as f64, "cycles", false, false, vec![l0row.id.clone(), l1row.id.clone()]);
                if clock.is_some() && n != 0
                    && ((n as f64 * lam) - clock.unwrap().offset_m).abs() <= p.clock_min_m
                {
                    cand.rejected = true;
                }
                cands.push(cand);
            }
        }
        let min_dist = cands.iter().map(|c| (c.estimate - c.offsets[&lcode] as f64).abs())
            .fold(f64::INFINITY, f64::min);
        let tie_count = cands.iter()
            .filter(|c| ((c.estimate - c.offsets[&lcode] as f64).abs() - min_dist).abs() < 1e-9)
            .count();
        for c in cands.iter_mut() {
            let dist = (c.estimate - c.offsets[&lcode] as f64).abs();
            if (dist - min_dist).abs() < 1e-9 && tie_count > 1 { c.tie = true; }
        }
        let set_id = content_id("set", &(&edge.id, &detector, &digest));
        let status = if cands.iter().all(|c| c.rejected) {
            "none"
        } else if cands.iter().filter(|c| !c.rejected).count() > 1 {
            "ambiguous"
        } else { "unique" };
        let mut set = make_set(edge, &detector, status, None, &digest, ctx);
        if cands.is_empty() {
            set.status = "none".into();
            let none = make_candidate(&set_id, "none", BTreeMap::new(),
                1e9, estimate, estimate, "cycles", false, true, vec![]);
            let rej = reject(&none.id, "td_residual_exceeds_tolerance",
                format!("{} = {:.6} cycles, tol {:.3}", lcode, estimate, p.td_tol_cycles));
            out.rejections.push(rej);
            cands.push(none);
        } else if let Some(clk) = clock {
            for c in cands.iter_mut() {
                if c.rejected {
                    let r = reject(&c.id, "explained_by_receiver_clock",
                        format!("clock_event={} offset_m={:.6}", clk.id, clk.offset_m));
                    out.rejections.push(r);
                }
            }
        }
        let pool = cands.iter().filter(|c| !c.rejected).cloned().collect::<Vec<_>>();
        out.candidates.extend(cands);
        td_pools.push((lcode.clone(), pool, set_id.clone()));
        out.sets.push(set);
    }

    // ---------- GF / MW ----------
    let combo_pools = run_gf_mw(edge, primary, srows, arcs, clock, ctx, &mut used, out);

    // ---------- 融合 ----------
    run_fusion(edge, clock, &td_pools, &combo_pools, &used, ctx, out);
}

fn n_wl(e: Epoch, srows: &[&ObsRow]) -> Option<(f64, Vec<String>)> {
    let sat = srows.first()?.sat.clone();
    let (l1c, f1) = freq_of(&sat, srows, 'L', '1')?;
    let (l2c, f2) = freq_of(&sat, srows, 'L', '2')?;
    let (c1c, _) = freq_of(&sat, srows, 'C', '1')?;
    let (c2c, _) = freq_of(&sat, srows, 'C', '2')?;
    let l1 = row_at(srows, e, |c| c == l1c)?.value?;
    let l2 = row_at(srows, e, |c| c == l2c)?.value?;
    let c1 = row_at(srows, e, |c| c == c1c)?.value?;
    let c2 = row_at(srows, e, |c| c == c2c)?.value?;
    let lam1 = crate::model::C_M_PER_S / f1;
    let lam2 = crate::model::C_M_PER_S / f2;
    let lam_wl = crate::model::C_M_PER_S / (f1 - f2);
    let phase = (f1 * l1 * lam1 - f2 * l2 * lam2) / (f1 - f2);
    let code = (f1 * c1 + f2 * c2) / (f1 + f2);
    let mut ids = Vec::new();
    for code_name in [&l1c, &l2c, &c1c, &c2c] {
        if let Some(r) = row_at(srows, e, |c| c == *code_name) {
            ids.push(r.id.clone());
        }
    }
    Some(((phase - code) / lam_wl, ids))
}

#[allow(clippy::too_many_arguments)]
fn run_gf_mw(
    edge: &Edge,
    primary: &str,
    srows: &[&ObsRow],
    arcs: &BTreeMap<(String, String), Vec<Arc>>,
    clock: Option<&ClockEvent>,
    ctx: &EngineCtx,
    used: &mut BTreeSet<String>,
    out: &mut EngineOutput,
) -> Vec<(String, String, Vec<Candidate>)> {
    let p = ctx.params;
    let mut pools = Vec::new();
    let sat = edge.sat.clone();

    // ---------------- GF ----------------
    let mut gf_missing = None;
    let gf_pair = (|| {
        let (l1, _f1) = freq_of(&sat, srows, 'L', '1')?;
        let (l2, _f2) = freq_of(&sat, srows, 'L', '2')?;
        let a0 = row_at(srows, edge.from, |c| c == l1)?.value?;
        let a1 = row_at(srows, edge.to, |c| c == l1)?.value?;
        let b0 = row_at(srows, edge.from, |c| c == l2)?.value?;
        let b1 = row_at(srows, edge.to, |c| c == l2)?.value?;
        Some((l1, l2, a0, a1, b0, b1))
    })();
    let digest0 = edge_digest(used.clone(), ctx, clock.map(|c| c.id.as_str()));
    let gf_set_id = content_id("set", &(&edge.id, "gf", &digest0));
    match gf_pair {
        None => {
            gf_missing = Some(format!(
                "frequency_unavailable: need {}+{} phase at both epochs", "L-band1", "L-band2"));
            let set = make_set(edge, "gf", "unavailable", gf_missing, &digest0, ctx);
            out.sets.push(set);
        }
        Some((l1, l2, a0, a1, b0, b1)) => {
            for (cn, e) in [(&l1, edge.from), (&l1, edge.to), (&l2, edge.from), (&l2, edge.to)] {
                if let Some(r) = row_at(srows, e, |c| c == cn) {
                    used.insert(r.id.clone());
                }
            }
            let jump = (a1 - a0) - (b1 - b0);
            let mut cands = Vec::new();
            for n1 in -8..=8i64 {
                for n2 in -8..=8i64 {
                    let dist = ((n1 - n2) as f64 - jump).abs();
                    if dist <= p.gf_tol_cycles + 1e-12 {
                        let mut off = BTreeMap::new();
                        off.insert(l1.clone(), n1);
                        off.insert(l2.clone(), n2);
                        let kind = if n1 == 0 && n2 == 0 { "none" } else { "slip" };
                        cands.push(make_candidate(&gf_set_id, kind, off,
                            dist + p.complexity_penalty * (n1.abs() + n2.abs()) as f64,
                            jump, jump - (n1 - n2) as f64, "cycles(GF)", false, false, vec![]));
                    }
                }
            }
            let min_dist = cands.iter().map(|c| c.residual.abs()).fold(f64::INFINITY, f64::min);
            let tie_n = cands.iter().filter(|c| (c.residual.abs() - min_dist).abs() < 1e-9).count();
            for c in cands.iter_mut() {
                if (c.residual.abs() - min_dist).abs() < 1e-9 && tie_n > 1 { c.tie = true; }
            }
            let status = if cands.is_empty() { "none" }
                else if cands.iter().all(|c| c.residual.abs() != min_dist) { "unique" }
                else if tie_n > 1 { "ambiguous" } else { "unique" };
            let digest = edge_digest(used.clone(), ctx, clock.map(|c| c.id.as_str()));
            let set = make_set(edge, "gf", status, None, &digest, ctx);
            let set_id = set.id.clone();
            if cands.is_empty() {
                let none = make_candidate(&set_id, "none", BTreeMap::new(), 1e9,
                    jump, jump, "cycles(GF)", false, true, vec![]);
                out.rejections.push(reject(&none.id, "gf_residual_exceeds_tolerance",
                    format!("GF jump {:.6} cycles, tol {:.3}", jump, p.gf_tol_cycles)));
                out.candidates.push(none);
            } else {
                for c in cands.iter_mut() { c.set_id = set_id.clone(); }
                out.candidates.extend(cands.iter().cloned());
                pools.push(("gf".to_string(), set_id.clone(),
                    cands.into_iter().collect::<Vec<_>>()));
            }
            out.sets.push(set);
        }
    }

    // ---------------- MW ----------------
    let arc = arcs.get(&(sat.clone(), primary.to_string())).and_then(|list| {
        list.iter().find(|a| edge.to.minus(&a.start) >= -1e-9 && a.end.minus(&edge.to) >= -1e-9)
    });
    let mut baseline_epochs = Vec::new();
    if let Some(a) = arc {
        let mut eps: Vec<Epoch> = srows.iter().map(|r| r.epoch())
            .filter(|e| e.minus(&a.start) >= -1e-9 && a.end.minus(e) >= -1e-9)
            .collect();
        eps.sort_by_key(Epoch::key);
        eps.dedup_by_key(Epoch::key);
        let mut prev: Vec<Epoch> = eps.into_iter().filter(|e| e.key() < edge.to.key()).collect();
        let take = p.mw_window.max(1) as usize;
        if prev.len() > take { prev.drain(0..prev.len() - take); }
        baseline_epochs = prev;
    }
    let nwl_to = n_wl(edge.to, srows);
    let nwl_base: Vec<Option<(f64, Vec<String>)>> =
        baseline_epochs.iter().map(|e| n_wl(*e, srows)).collect();
    let digest0 = edge_digest(used.clone(), ctx, clock.map(|c| c.id.as_str()));
    let mw_set_id_early = content_id("set", &(&edge.id, "mw", &digest0));
    let _ = mw_set_id_early;
    match (nwl_to, nwl_base.iter().all(|x| x.is_some()) && !nwl_base.is_empty()) {
        (Some((n1v, ids1)), true) => {
            used.extend(ids1);
            let mut base_ids = Vec::new();
            for x in nwl_base.iter().flatten() {
                base_ids.extend(x.1.clone());
            }
            used.extend(base_ids);
            let base = nwl_base.iter().flatten().map(|x| x.0).sum::<f64>() / nwl_base.len() as f64;
            let jump = n1v - base;
            let nwl_round = jump.round() as i64;
            let digest = edge_digest(used.clone(), ctx, clock.map(|c| c.id.as_str()));
            let set_id = content_id("set", &(&edge.id, "mw", &digest));
            let (l1, _) = freq_of(&sat, srows, 'L', '1').unwrap();
            let (l2, _) = freq_of(&sat, srows, 'L', '2').unwrap();
            if (jump - nwl_round as f64).abs() > p.mw_tol_cycles + 1e-12 {
                let none = make_candidate(&set_id, "none", BTreeMap::new(), 1e9,
                    jump, jump, "cycles(WL)", false, true, vec![]);
                out.rejections.push(reject(&none.id, "mw_residual_exceeds_tolerance",
                    format!("MW jump {:.6} WL cycles, tol {:.3}", jump, p.mw_tol_cycles)));
                out.candidates.push(none);
                out.sets.push(make_set(edge, "mw", "none", None, &digest, ctx));
            } else {
                let mut cands = Vec::new();
                for n1 in -8..=8i64 {
                    let n2 = n1 - nwl_round;
                    if !(-8..=8).contains(&n2) { continue; }
                    let dist = (jump - nwl_round as f64).abs();
                    let mut off = BTreeMap::new();
                    off.insert(l1.clone(), n1);
                    off.insert(l2.clone(), n2);
                    let kind = if n1 == 0 && n2 == 0 { "none" } else { "slip" };
                    cands.push(make_candidate(&set_id, kind, off,
                        dist + p.complexity_penalty * (n1.abs() + n2.abs()) as f64,
                        jump, jump - (n1 - n2) as f64, "cycles(WL)", false, false, vec![]));
                }
                let min_dist = cands.iter().map(|c| (c.residual.abs())).fold(f64::INFINITY, f64::min);
                let tie_n = cands.iter().filter(|c| (c.residual.abs() - min_dist).abs() < 1e-9).count();
                for c in cands.iter_mut() {
                    if (c.residual.abs() - min_dist).abs() < 1e-9 && tie_n > 1 { c.tie = true; }
                }
                let status = if tie_n > 1 { "ambiguous" } else { "unique" };
                out.candidates.extend(cands.iter().cloned());
                pools.push(("mw".to_string(), set_id.clone(), cands));
                out.sets.push(make_set(edge, "mw", status, None, &digest, ctx));
            }
        }
        _ => {
            let set = make_set(edge, "mw", "unavailable",
                Some("frequency_unavailable: need C/L band1+band2 at both epochs and baseline".into()),
                &digest0, ctx);
            out.sets.push(set);
        }
    }
    pools
}
