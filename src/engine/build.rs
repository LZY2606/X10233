//! Arc state machine, residual timeline and candidate enumeration.

use super::combo;
use super::*;
use crate::domain::{CandidateKind, ObsField, F_CARRIER, F_DOPPLER, F_PSEUDORANGE};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Prev {
    order: i64,
    epoch_id: String,
    gps: f64,
    lli1: i64,
    lli2: Option<i64>,
    p1: f64,
    l1: f64,
    d1: Option<f64>,
    p2: Option<f64>,
    l2: Option<f64>,
    d2: Option<f64>,
    gf: Option<f64>,
    mw: Option<f64>,
}

struct SatRun {
    arcs: Vec<ArcOut>,
    points: Vec<ResidualPoint>,
    raw_bounds: Vec<RawBound>,
    cur_arc: Option<ArcOut>,
    prev: Option<Prev>,
}

struct RawBound {
    epoch_id: String,
    kinds: BTreeSet<String>,
    triggers: BTreeSet<String>,
    jump: JumpMeas,
}

#[derive(Default, Clone)]
struct JumpMeas {
    dt_s: f64,
    gf_jump_m: Option<f64>,
    mw_jump_cycle: Option<f64>,
    gf_pred_m: f64,
    mw_pred_cycle: f64,
    td1_cycle: Option<f64>,
    td2_cycle: Option<f64>,
    d1_now: Option<f64>,
    d2_now: Option<f64>,
}

fn cell<'a>(b: &'a BandObs, key: &str) -> Option<f64> {
    b.cells.get(key).and_then(|c| c.value)
}

pub fn build(input: &EngineInput) -> BuildReport {
    let sig1 = input.signals.get(&1).expect("band 1 required");
    let sig2 = input.signals.get(&2);
    let lam1 = sig1.wavelength_m;
    let lam2 = sig2.map(|s| s.wavelength_m);

    // Median epoch spacing drives the irregular-gap test.
    let med_dt = median_spacing(&input.epochs);
    let gap_limit = med_dt * input.params.gap_factor;

    let clock_epochs: BTreeSet<i64> =
        input.clocks.iter().filter(|c| c.has_clock_event).map(|c| c.epoch_order).collect();

    let mut runs: BTreeMap<String, SatRun> = BTreeMap::new();
    let epoch_by_order: BTreeMap<i64, &EpochMeta> =
        input.epochs.iter().map(|e| (e.order, e)).collect();

    // rows are processed in epoch, then satellite, then band order.
    let mut by_sat: BTreeMap<String, Vec<&BandObs>> = BTreeMap::new();
    for r in &input.rows {
        by_sat.entry(r.sat.clone()).or_default().push(r);
    }

    for (sat, band_rows) in &mut by_sat {
        let mut by_epoch: BTreeMap<i64, BTreeMap<i64, &&BandObs>> = BTreeMap::new();
        for r in band_rows {
            by_epoch.entry(r.epoch_order).or_default().insert(r.band, r);
        }
        let mut run = SatRun {
            arcs: Vec::new(),
            points: Vec::new(),
            raw_bounds: Vec::new(),
            cur_arc: None,
            prev: None,
        };

        for e in &input.epochs {
            let bands_present: BTreeMap<_, _> = by_epoch.get(&e.order).cloned().unwrap_or_default();
            let b1 = bands_present.get(&1).copied();
            let b2 = bands_present.get(&2).copied();
            process_epoch(
                input,
                &mut run,
                sat,
                e,
                b1,
                b2,
                lam1,
                lam2,
                sig1,
                sig2,
                med_dt,
                gap_limit,
                &clock_epochs,
            );
        }
        close_arc(&mut run, &epoch_by_order);
        runs.insert(sat.clone(), run);
    }

    assemble(input, runs)
}

#[allow(clippy::too_many_arguments)]
fn process_epoch(
    input: &EngineInput,
    run: &mut SatRun,
    sat: &str,
    e: &EpochMeta,
    b1: Option<&BandObs>,
    b2: Option<&BandObs>,
    lam1: f64,
    lam2: Option<f64>,
    sig1: &SignalMeta,
    sig2: Option<&SignalMeta>,
    med_dt: f64,
    gap_limit: f64,
    clock_epochs: &BTreeSet<i64>,
) {
    let lli1 = b1.map(|b| b.lli).unwrap_or(0);
    let lli2 = b2.map(|b| b.lli);
    let p1 = b1.and_then(|b| cell(b, F_PSEUDORANGE));
    let l1 = b1.and_then(|b| cell(b, F_CARRIER));
    let d1 = b1.and_then(|b| cell(b, F_DOPPLER));
    let p2 = b2.and_then(|b| cell(b, F_PSEUDORANGE));
    let l2 = b2.and_then(|b| cell(b, F_CARRIER));
    let d2 = b2.and_then(|b| cell(b, F_DOPPLER));

    let (gf, mw) = match (l1, l2, lam2, sig2, p1, p2) {
        (Some(l1v), Some(l2v), Some(lam2v), Some(s2), Some(p1v), Some(p2v)) => {
            let gf = combo::gf_m(lam1, l1v, lam2v, l2v);
            let mw = combo::mw_wl_cycle(sig1, l1v, p1v, s2, l2v, p2v);
            (Some(gf), Some(mw))
        }
        _ => (None, None),
    };

    // Arc boundary bookkeeping requires a previous point.
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut triggers: BTreeSet<String> = BTreeSet::new();
    let mut jump = JumpMeas::default();

    if let Some(prev) = run.prev.clone() {
        let dt = e.gps_seconds - prev.gps;
        jump.dt_s = dt;
        if dt > gap_limit + 1e-9 && med_dt > 0.0 {
            kinds.insert("gap".to_string());
            triggers.insert("irregular_gap".to_string());
        }
        if clock_epochs.contains(&e.order) {
            kinds.insert("clock_jump".to_string());
            triggers.insert("receiver_clock_jump".to_string());
        }
        // Band present previously but missing now => local frequency gap.
        if prev.l2.is_some() && l2.is_none() {
            kinds.insert("frequency_missing".to_string());
            triggers.insert("frequency_missing".to_string());
        }
        if prev.lli2.is_some() && l2.is_some() && lli2.is_some_and(|v| v >= input.params.lli_flag) {
            triggers.insert("lli".to_string());
        }
        if lli1 >= input.params.lli_flag {
            triggers.insert("lli".to_string());
        }

        // Combination jumps vs zero ionosphere-change prediction.
        if let (Some(g_now), Some(g_prev), Some(lam2v)) = (gf, prev.gf, lam2) {
            let raw = g_now - g_prev;
            // Slow ionosphere drift prediction (zero-order, explicitly
            // recorded as the chosen prediction) = 0 over the baseline.
            jump.gf_pred_m = 0.0;
            jump.gf_jump_m = Some(raw);
        }
        if let (Some(m_now), Some(m_prev)) = (mw, prev.mw) {
            jump.mw_pred_cycle = 0.0;
            jump.mw_jump_cycle = Some(m_now - m_prev);
        }
        if let (Some(ln), Some(lp), Some(dn)) = (l1, prev.l1, d1) {
            let pred = prev.d1.map(|dp| combo::td_predicted_cycles(dp, dn, dt));
            if let Some(pred) = pred {
                jump.td1_cycle = Some(ln - lp - pred);
            }
        }
        if let (Some(ln), Some(lp), Some(dn)) = (l2, prev.l2, d2) {
            let pred = prev.d2.and_then(|dp| {
                Some(combo::td_predicted_cycles(dp, dn, dt))
            }).flatten();
            if let Some(pred) = pred {
                jump.td2_cycle = Some(ln - lp - pred);
            }
        }
        jump.d1_now = d1;
        jump.d2_now = d2;

        let gf_alarm = jump
            .gf_jump_m
            .is_some_and(|j| j.abs() > input.params.gf_sigma_m * 3.0);
        let mw_alarm = jump
            .mw_jump_cycle
            .is_some_and(|j| j.abs() > input.params.mw_sigma_cycle);
        let td_alarm = jump
            .td1_cycle
            .is_some_and(|j| j.abs() > input.params.td_sigma_cycle)
            || jump
                .td2_cycle
                .is_some_and(|j| j.abs() > input.params.td_sigma_cycle);
        if gf_alarm {
            triggers.insert("gf".to_string());
        }
        if mw_alarm {
            triggers.insert("mw".to_string());
        }
        if td_alarm {
            triggers.insert("td".to_string());
        }
        if !kinds.is_empty() || !triggers.is_empty() {
            run.raw_bounds.push(RawBound {
                epoch_id: e.id.clone(),
                kinds: kinds.clone(),
                triggers: triggers.clone(),
                jump: jump.clone(),
            });
        }
    }

    // Split/start arcs when the observable scope changes.
    let scope = if b2.is_some() { "1+2" } else { "1" };
    let need_split = match &run.cur_arc {
        None => true,
        Some(a) => {
            a.band_scope != scope
                || kinds.contains("gap")
                || kinds.contains("clock_jump")
                || kinds.contains("frequency_missing")
                || run.prev.as_ref().is_some_and(|p| {
                    p.l2.is_none() && l2.is_some()
                })
        }
    };
    if need_split {
        close_arc(run, &collect_orders(input));
        let arc_id = format!("{}-AR{}", sat, run.arcs.len() as i64 + 1);
        run.cur_arc = Some(ArcOut {
            arc_id,
            sat: sat.to_string(),
            band_scope: scope.to_string(),
            start_epoch: e.id.clone(),
            end_epoch: e.id.clone(),
            length: 0,
            boundaries: Vec::new(),
        });
    }
    if let Some(a) = run.cur_arc.as_mut() {
        a.end_epoch = e.id.clone();
        a.length += 1;
        if !kinds.is_empty() || !triggers.is_empty() {
            a.boundaries.push(ArcBoundary {
                epoch_id: e.id.clone(),
                kinds: kinds.iter().cloned().collect(),
                candidate_id: None,
            });
        }
    }

    let arc_id = run.cur_arc.as_ref().map(|a| a.arc_id.clone()).unwrap_or_default();
    let _ = med_dt;
    run.points.push(ResidualPoint {
        epoch_id: e.id.clone(),
        sat: sat.to_string(),
        gf_m: gf,
        gf_delta_m: run.prev.clone().zip(gf).and_then(|(p, g)| p.gf.map(|pg| g - pg)),
        mw_wl_cycle: mw,
        mw_delta_cycle: run.prev.clone().zip(mw).and_then(|(p, m)| p.mw.map(|pm| m - pm)),
        td_l1_cycle: jump.td1_cycle,
        td_l2_cycle: jump.td2_cycle,
        doppler_l1_hz: d1,
        doppler_l2_hz: d2,
        arc_id,
    });

    if let (Some(l1v),) = (l1,) {
        run.prev = Some(Prev {
            order: e.order,
            epoch_id: e.id.clone(),
            gps: e.gps_seconds,
            lli1,
            lli2,
            p1: p1.unwrap_or(0.0),
            l1: l1v,
            d1,
            p2,
            l2,
            d2,
            gf,
            mw,
        });
    }
}

fn collect_orders(input: &EngineInput) -> BTreeMap<i64, &EpochMeta> {
    input.epochs.iter().map(|e| (e.order, e)).collect()
}

fn close_arc(run: &mut SatRun, _orders: &BTreeMap<i64, &EpochMeta>) {
    if let Some(a) = run.cur_arc.take() {
        run.arcs.push(a);
    }
}

fn median_spacing(epochs: &[EpochMeta]) -> f64 {
    let mut d: Vec<f64> = epochs.windows(2).map(|w| w[1].gps_seconds - w[0].gps_seconds).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if d.is_empty() {
        30.0
    } else {
        d[d.len() / 2]
    }
}

fn assemble(input: &EngineInput, runs: BTreeMap<String, SatRun>) -> BuildReport {
    let mut arcs = Vec::new();
    let mut residuals = Vec::new();
    let mut bounds_by_sat: BTreeMap<String, Vec<RawBound>> = BTreeMap::new();
    for (sat, run) in runs {
        arcs.extend(run.arcs);
        residuals.extend(run.points);
        bounds_by_sat.insert(sat, run.raw_bounds);
    }

    // Whole-machine clock jumps: all satellites alarm at the same epoch.
    let clock_epochs: BTreeSet<i64> =
        input.clocks.iter().filter(|c| c.has_clock_event).map(|c| c.epoch_order).collect();
    let epoch_order: BTreeMap<&str, i64> =
        input.epochs.iter().map(|e| (e.id.as_str(), e.order)).collect();

    let mut candidates: Vec<CandidateOut> = Vec::new();
    let mut affected_arcs: BTreeSet<String> = BTreeSet::new();

    for order in &clock_epochs {
        let eid = input.epochs.iter().find(|e| e.order == *order).map(|e| e.id.clone()).unwrap();
        let mut members = Vec::new();
        for (sat, bounds) in &bounds_by_sat {
            if bounds.iter().any(|b| b.epoch_id == eid) {
                members.push(sat.clone());
            }
        }
        members.sort();
        let cid = format!("CLK-{eid}");
        candidates.push(CandidateOut {
            candidate_id: cid,
            sat: "STATION".to_string(),
            epoch_id: eid.clone(),
            kind: CandidateKind::ClockJump.as_str().to_string(),
            status: "open".to_string(),
            triggers: vec!["receiver_clock_jump".to_string()],
            gf_jump_m: None,
            mw_jump_cycle: None,
            td_l1_cycle: None,
            td_l2_cycle: None,
            gf_predicted_m: None,
            mw_predicted_cycle: None,
            ambiguity: false,
            ambiguity_tie_count: 0,
            hypotheses: Vec::new(),
            evidence: Vec::new(),
            note: "common-mode whole-machine clock jump; kept separate from per-satellite slips"
                .to_string(),
            members: members.clone(),
        });
        for a in arcs.iter().filter(|a| members.contains(&a.sat) && within(&a, &eid)) {
            affected_arcs.insert(a.arc_id.clone());
        }
    }

    // Per-satellite slip candidates at non-clock boundaries/alarms.
    for (sat, bounds) in &bounds_by_sat {
        for b in bounds {
            let is_clock = epoch_order.get(b.epoch_id.as_str()).is_some_and(|o| clock_epochs.contains(o));
            if is_clock {
                // The common-mode clock explains the jumps; per-satellite slip
                // candidates at this epoch are explicitly not created.
                continue;
            }
            if b.triggers.is_empty() {
                continue;
            }
            let cid = format!("{}-{}", sat, b.epoch_id);
            let cand = build_slip_candidate(input, sat, b, cid);
            for a in arcs.iter().filter(|a| a.sat == *sat && within(a, &b.epoch_id)) {
                affected_arcs.insert(a.arc_id.clone());
            }
            candidates.push(cand);
        }
    }

    // Link candidate ids onto arc boundaries.
    let link: BTreeMap<(String, String), String> = candidates
        .iter()
        .filter(|c| c.kind == CandidateKind::Slip.as_str())
        .map(|c| ((c.sat.clone(), c.epoch_id.clone()), c.candidate_id.clone()))
        .collect();
    for a in arcs.iter_mut() {
        for bd in a.boundaries.iter_mut() {
            if let Some(cid) = link.get(&(a.sat.clone(), bd.epoch_id.clone())) {
                bd.candidate_id = Some(cid.clone());
            } else if clock_epochs
                .iter()
                .any(|o| input.epochs.iter().any(|e| e.order == *o && e.id == bd.epoch_id))
            {
                bd.candidate_id = Some(format!("CLK-{}", bd.epoch_id));
            }
        }
    }

    candidates.sort_by(|a, b| {
        epoch_order
            .get(a.epoch_id.as_str())
            .unwrap_or(&0)
            .cmp(epoch_order.get(b.epoch_id.as_str()).unwrap_or(&0))
            .then(a.candidate_id.cmp(&b.candidate_id))
    });
    residuals.sort_by(|a, b| a.sat.cmp(&b.sat).then(a.epoch_id.cmp(&b.epoch_id)));

    BuildReport {
        arcs,
        residuals,
        candidates,
        affected_arcs: affected_arcs.into_iter().collect(),
    }
}

fn within(a: &ArcOut, epoch_id: &str) -> bool {
    a.start_epoch <= epoch_id.to_string() && epoch_id.to_string() <= a.end_epoch
}

fn build_slip_candidate(input: &EngineInput, sat: &str, b: &RawBound, cid: String) -> CandidateOut {
    let sig1 = input.signals.get(&1).unwrap();
    let sig2 = input.signals.get(&2);
    let lam1 = sig1.wavelength_m;
    let lam2 = sig2.map(|s| s.wavelength_m);
    let dual = b.jump.gf_jump_m.is_some() || b.jump.mw_jump_cycle.is_some();

    let p = &input.params;
    let gf_tol = (p.gf_sigma_m * 3.0).max(p.gf_cycle_threshold);
    let mw_tol = p.mw_sigma_cycle;
    let td_tol = p.td_sigma_cycle;

    let j_gf = b.jump.gf_jump_m;
    let j_mw = b.jump.mw_jump_cycle;
    let j_td1 = b.jump.td1_cycle;
    let j_td2 = b.jump.td2_cycle;

    let mut hyps: Vec<HypothesisOut> = Vec::new();
    let r = p.search_radius_cycle;

    let n2_vals: Vec<Option<i64>> = if dual && lam2.is_some() {
        (-r..=r).map(Some).collect()
    } else {
        vec![None]
    };

    for n1 in -r..=r {
        for &n2 in &n2_vals {
            let mut rejections = Vec::new();

            let gf_resid = match (j_gf, lam2) {
                (Some(j), Some(l2)) => {
                    let pred = combo::gf_jump_m(lam1, l2, n1, n2);
                    let res = j - pred;
                    if res.abs() > gf_tol {
                        rejections.push(RejectionOut {
                            detector: "gf".to_string(),
                            reason: format!(
                                "GF residual {:.4} m exceeds tolerance {:.4} m",
                                res.abs(),
                                gf_tol
                            ),
                        });
                    }
                    Some(res)
                }
                _ => None,
            };

            let mw_resid = match (j_mw, n2) {
                (Some(j), Some(n2v)) => {
                    let pred = combo::mw_jump_cycle(n1, Some(n2v));
                    let res = j - pred;
                    if res.abs() > mw_tol {
                        rejections.push(RejectionOut {
                            detector: "mw".to_string(),
                            reason: format!(
                                "MW residual {:.4} WL cycle exceeds tolerance {:.4}",
                                res.abs(),
                                mw_tol
                            ),
                        });
                    }
                    Some(res)
                }
                _ => None,
            };

            let td1_resid = match j_td1 {
                Some(j) => {
                    let res = j - n1 as f64;
                    if res.abs() > td_tol {
                        rejections.push(RejectionOut {
                            detector: "td".to_string(),
                            reason: format!(
                                "TD/L1 residual {:.4} cycle exceeds tolerance {:.4}",
                                res.abs(),
                                td_tol
                            ),
                        });
                    }
                    Some(res)
                }
                None => None,
            };

            let td2_resid = match (j_td2, n2) {
                (Some(j), Some(n2v)) => {
                    let res = j - n2v as f64;
                    if res.abs() > td_tol {
                        rejections.push(RejectionOut {
                            detector: "td".to_string(),
                            reason: format!(
                                "TD/L2 residual {:.4} cycle exceeds tolerance {:.4}",
                                res.abs(),
                                td_tol
                            ),
                        });
                    }
                    Some(res)
                }
                _ => None,
            };

            let admissible = rejections.is_empty();
            let mut score = 0.0;
            if let Some(v) = gf_resid {
                score += (v / p.gf_sigma_m).powi(2);
            }
            if let Some(v) = mw_resid {
                score += (v / p.mw_sigma_cycle).powi(2);
            }
            if let Some(v) = td1_resid {
                score += (v / p.td_sigma_cycle).powi(2);
            }
            if let Some(v) = td2_resid {
                score += (v / p.td_sigma_cycle).powi(2);
            }

            hyps.push(HypothesisOut {
                n1,
                n2,
                score,
                adopted: false,
                rank: 0,
                admissible,
                rejections,
                gf_residual_m: gf_resid,
                mw_residual_cycle: mw_resid,
                td_residual_l1_cycle: td1_resid,
                td_residual_l2_cycle: td2_resid,
            });
        }
    }

    hyps.sort_by(|a, z| {
        z.admissible
            .cmp(&a.admissible)
            .then_with(|| a.score.partial_cmp(&z.score).unwrap())
            .then_with(|| a.n1.cmp(&z.n1))
            .then_with(|| a.n2.unwrap_or(0).cmp(&z.n2.unwrap_or(0)))
    });

    let admissible: Vec<&mut HypothesisOut> = hyps.iter_mut().filter(|h| h.admissible).collect();
    let best_score = admissible.first().map(|h| h.score);
    let tie_count = if let Some(best) = best_score {
        admissible.iter().filter(|h| (h.score - best).abs() < 1e-6).count() as i64
    } else {
        0
    };
    for (i, h) in admissible.iter_mut().enumerate() {
        h.rank = i as i64 + 1;
        if i == 0 && tie_count == 1 {
            h.adopted = true;
        }
    }

    let ambiguous = tie_count > 1;
    let evidence = build_evidence(
        j_gf,
        j_mw,
        j_td1,
        j_td2,
        gf_tol,
        mw_tol,
        td_tol,
        dual,
    );

    let only_lli = b.triggers.iter().all(|t| t == "lli");
    let no_slip_unique = !ambiguous
        && hyps
            .iter()
            .find(|h| h.adopted)
            .is_some_and(|h| h.n1 == 0 && h.n2.unwrap_or(0) == 0);
    let status = if ambiguous {
        "ambiguous"
    } else if no_slip_unique {
        "open"
    } else {
        "open"
    };
    let note = if only_lli && no_slip_unique {
        "LLI flag raised but GF/MW/TD show no jump: recorded as LLI false-positive candidate".to_string()
    } else if ambiguous {
        format!("{tie_count} integer hypotheses tie; all legal candidates retained, no detector is truth")
    } else {
        "slip candidate from independent GF/MW/TD evidence".to_string()
    };

    CandidateOut {
        candidate_id: cid,
        sat: sat.to_string(),
        epoch_id: b.epoch_id.clone(),
        kind: CandidateKind::Slip.as_str().to_string(),
        status: status.to_string(),
        triggers: b.triggers.iter().cloned().collect(),
        gf_jump_m: j_gf,
        mw_jump_cycle: j_mw,
        td_l1_cycle: j_td1,
        td_l2_cycle: j_td2,
        gf_predicted_m: j_gf.map(|_| b.jump.gf_pred_m),
        mw_predicted_cycle: j_mw.map(|_| b.jump.mw_pred_cycle),
        ambiguity,
        ambiguity_tie_count: tie_count,
        hypotheses: hyps,
        evidence,
        note,
        members: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_evidence(
    j_gf: Option<f64>,
    j_mw: Option<f64>,
    j_td1: Option<f64>,
    j_td2: Option<f64>,
    gf_tol: f64,
    mw_tol: f64,
    td_tol: f64,
    dual: bool,
) -> Vec<EvidenceOut> {
    let mut out = Vec::new();
    match j_gf {
        Some(j) => out.push(EvidenceOut {
            detector: "gf".to_string(),
            residual: Some(j),
            unit: "m".to_string(),
            predicted: Some(0.0),
            observed: Some(j),
            threshold: gf_tol,
            status: if j.abs() > gf_tol { "alarm".to_string() } else { "ok".to_string() },
            note: "geometry-free/ionosphere carrier combination epoch jump".to_string(),
        }),
        None => out.push(EvidenceOut {
            detector: "gf".to_string(),
            residual: None,
            unit: "m".to_string(),
            predicted: None,
            observed: None,
            threshold: gf_tol,
            status: "unavailable".to_string(),
            note: if dual { "missing numeric carrier on a band" } else { "single-frequency arc: GF combination not defined" }.to_string(),
        }),
    }
    match j_mw {
        Some(j) => out.push(EvidenceOut {
            detector: "mw".to_string(),
            residual: Some(j),
            unit: "WL-cycle".to_string(),
            predicted: Some(0.0),
            observed: Some(j),
            threshold: mw_tol,
            status: if j.abs() > mw_tol { "alarm".to_string() } else { "ok".to_string() },
            note: "Melbourne-Wubbena wide-lane epoch jump".to_string(),
        }),
        None => out.push(EvidenceOut {
            detector: "mw".to_string(),
            residual: None,
            unit: "WL-cycle".to_string(),
            predicted: None,
            observed: None,
            threshold: mw_tol,
            status: "unavailable".to_string(),
            note: "MW needs dual-frequency code+carrier".to_string(),
        }),
    }
    for (det, j, band) in [("td", j_td1, 1), ("td", j_td2, 2)] {
        match j {
            Some(v) => out.push(EvidenceOut {
                detector: format!("{det}_l{band}"),
                residual: Some(v),
                unit: "cycle".to_string(),
                predicted: Some(0.0),
                observed: Some(v),
                threshold: td_tol,
                status: if v.abs() > td_tol { "alarm".to_string() } else { "ok".to_string() },
                note: format!("time-difference carrier vs Doppler prediction on L{band}"),
            }),
            None => out.push(EvidenceOut {
                detector: format!("{det}_l{band}"),
                residual: None,
                unit: "cycle".to_string(),
                predicted: None,
                observed: None,
                threshold: td_tol,
                status: "unavailable".to_string(),
                note: format!("Doppler/carrier unavailable on L{band}"),
            }),
        }
    }
    out
}
