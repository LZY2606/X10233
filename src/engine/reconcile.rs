//! Candidate reconciliation.
//!
//! Per edge we combine TD (per band), GF (ionosphere combination) and MW
//! (Melbourne-Wubbena wide lane) proposals. The rules deliberately do not
//! elect any detector as truth:
//! * TD-only integer offset hypotheses survive as band candidates.
//! * GF pairs survive when GF is the only two-band witness.
//! * Joint hypotheses require BOTH a GF pair and the matching MW wide-lane
//!   integer (n = na - nb). They form one ambiguity group; when several pairs
//!   tie (e.g. because one frequency is missing on the edge) every legal
//!   candidate is kept with equal score.
//! * An LLI flag with no slip witnesses is proposed with unknown offset and a
//!   reviewer reject hint (LLI false-positive case), but is never auto-rejected.

use super::compute::EdgeCand;
use crate::ids;
use crate::model::*;
use crate::params::{DetectorParams, ALG_VERSION};
use std::collections::BTreeMap;

pub fn reconcile_edges(
    edges: &BTreeMap<(String, String, i64), EdgeCand>,
    params: &DetectorParams,
    param_fp: &str,
    project: &str,
) -> Vec<CycleCandidate> {
    let _ = project;
    // Merge edges keyed by (sat, at_seq): band A carries GF/MW, band B edges
    // carry LLI/TD.
    let mut by_sat_edge: BTreeMap<(String, i64), Vec<(&(String, String, i64), &EdgeCand)>> =
        BTreeMap::new();
    for (k, v) in edges {
        by_sat_edge.entry((k.0.clone(), k.2)).or_default().push((k, v));
    }

    let mut out = Vec::new();
    for ((sat, seq), group) in by_sat_edge {
        let epoch_raw = group
            .iter()
            .map(|(_, e)| e.epoch_raw.clone())
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let band_a = params.gf_band_a.clone();
        let band_b = params.gf_band_b.clone();
        let ec_a = group
            .iter()
            .find(|(k, _)| k.1 == band_a)
            .map(|x| x.1);
        let ec_b = group
            .iter()
            .find(|(k, _)| k.1 == band_b)
            .map(|x| x.1);

        let mut proposals: Vec<String> = Vec::new();
        for e in [ec_a, ec_b].into_iter().flatten() {
            proposals.extend(e.proposals.clone());
        }

        let clock_edge = ec_a.map(|e| e.clock_declared).unwrap_or(false)
            || ec_b.map(|e| e.clock_declared).unwrap_or(false);
        let irregular = ec_a.map(|e| e.irregular).unwrap_or(false)
            || ec_b.map(|e| e.irregular).unwrap_or(false);
        let missing_b = ec_a.map(|e| e.missing_b).unwrap_or(false);

        // ---- two-band joint candidates ------------------------------------
        let mut joint: Vec<(i64, i64, f64, f64)> = Vec::new(); // na,nb,gf_res,score
        if let Some(ec) = ec_a {
            if !clock_edge {
                for &(na, nb, gfr) in &ec.gf {
                    let mw_ok = ec
                        .mw
                        .iter()
                        .any(|(n, _)| *n == na - nb);
                    let mw_absent = ec.mw.is_empty();
                    if mw_ok || mw_absent {
                        // Score prefers agreement; GF-only is weaker.
                        let base = (1.0 - gfr.abs().min(1.0) / params.gf_tol_m.max(1e-9)).max(0.0);
                        let score = if mw_ok { 0.90 + 0.09 * base } else { 0.60 + 0.10 * base };
                        joint.push((na, nb, gfr, score));
                    }
                }
            }
        }
        // Deterministic ordering: score desc, then na asc, nb asc.
        joint.sort_by(|a, b| {
            b.3.partial_cmp(&a.3)
                .unwrap()
                .then(a.0.cmp(&b.0))
                .then(a.1.cmp(&b.1))
        });

        // Detect tied equal-score ambiguity: multiple pairs with identical
        // score are "multiple equally dividing integer offsets".
        let group_key = format!("{sat}@{seq}");
        let n_joint = joint.len();
        if n_joint > 0 {
            // If TD band candidates directly contradict all joint pairs, keep
            // them anyway: disagreement is first-class information.
            for (idx, (na, nb, gfr, score)) in joint.iter().enumerate() {
                let sources = candidate_sources(ec_a, ec_b);
                let id = ids::candidate_id(
                    "GFMW",
                    &sat,
                    &format!("{band_a}+{band_b}"),
                    &epoch_raw,
                    &format!("{na},{nb}"),
                    &sources,
                );
                let tied = joint.iter().filter(|x| (x.3 - score).abs() < 1e-9).count();
                out.push(CycleCandidate {
                    id,
                    ambiguity_group: group_key.clone(),
                    ambiguity_n: n_joint,
                    detector: if ec_a.map(|e| !e.mw.is_empty()).unwrap_or(false) {
                        "GF+MW".to_string()
                    } else {
                        "GF".to_string()
                    },
                    alg_version: ALG_VERSION.to_string(),
                    param_fp: param_fp.to_string(),
                    sat: sat.clone(),
                    band: band_a.clone(),
                    band_b: Some(band_b.clone()),
                    at_seq: seq,
                    at_epoch: epoch_raw.clone(),
                    offset_cycles: *na as f64,
                    offset_b_cycles: Some(*nb as f64),
                    offset_unknown: false,
                    score: *score,
                    evidence: vec![
                        format!("gf_resid={gfr:.4}m"),
                        format!("joint_hypotheses={n_joint}"),
                        format!("equal_scored={tied}"),
                        if missing_b {
                            "frequency_missing_at_edge: GF/MW underdetermined".to_string()
                        } else {
                            String::new()
                        },
                    ]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect(),
                    detector_proposals: proposals.clone(),
                    suggest_reject: false,
                    status: CandidateStatus::Proposed,
                    status_reason: None,
                    offset_ids: vec![],
                    sources,
                });
                let _ = idx;
            }
        }

        // ---- TD-only per-band candidates (kept even when joint candidates
        // exist; disagreement is visible) -----------------------------------
        for (bname, ec) in [(&band_a, ec_a), (&band_b, ec_b)] {
            let Some(ec) = ec else { continue };
            if clock_edge {
                continue;
            }
            for &(n, resid) in &ec.td {
                // If a joint hypothesis already contains this band offset,
                // still emit the TD candidate as independent evidence.
                let sources = ec
                    .stat_ids
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>();
                let id = ids::candidate_id(
                    "TD",
                    &sat,
                    bname,
                    &epoch_raw,
                    &n.to_string(),
                    &sources,
                );
                out.push(CycleCandidate {
                    id,
                    ambiguity_group: format!("{sat}@{seq}:TD:{bname}"),
                    ambiguity_n: ec.td.len(),
                    detector: "TD".to_string(),
                    alg_version: ALG_VERSION.to_string(),
                    param_fp: param_fp.to_string(),
                    sat: sat.clone(),
                    band: bname.clone(),
                    band_b: None,
                    at_seq: seq,
                    at_epoch: epoch_raw.clone(),
                    offset_cycles: n as f64,
                    offset_b_cycles: None,
                    offset_unknown: false,
                    score: 0.75 + 0.20 * (1.0 - resid.abs() / params.td_tol_cycles.max(1e-9)).clamp01(),
                    evidence: vec![
                        format!("td_resid={resid:.4}cyc"),
                        if irregular { "irregular_gap".to_string() } else { String::new() },
                    ]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect(),
                    detector_proposals: ec.proposals.clone(),
                    suggest_reject: false,
                    status: CandidateStatus::Proposed,
                    status_reason: None,
                    offset_ids: vec![],
                    sources,
                });
            }

            // ---- LLI-only candidate with unknown offset -------------------
            if ec.li_only() {
                let sources = ec.stat_ids.iter().cloned().collect::<Vec<_>>();
                let id = ids::candidate_id("LLI", &sat, bname, &epoch_raw, "unknown", &sources);
                let contradicted =
                    !ec.td.is_empty() || ec_a.map(|e| !e.gf.is_empty()).unwrap_or(false);
                out.push(CycleCandidate {
                    id,
                    ambiguity_group: format!("{sat}@{seq}:LLI:{bname}"),
                    ambiguity_n: 1,
                    detector: "LLI".to_string(),
                    alg_version: ALG_VERSION.to_string(),
                    param_fp: param_fp.to_string(),
                    sat: sat.clone(),
                    band: bname.clone(),
                    band_b: None,
                    at_seq: seq,
                    at_epoch: epoch_raw.clone(),
                    offset_cycles: 0.0,
                    offset_b_cycles: None,
                    offset_unknown: true,
                    score: if contradicted { 0.15 } else { 0.35 },
                    evidence: vec![
                        "lli_bit0_set".to_string(),
                        if contradicted {
                            "contradicted_by_phase_detectors: treat_as_false_positive".to_string()
                        } else {
                            "no_numeric_witness".to_string()
                        },
                    ],
                    detector_proposals: ec.proposals.clone(),
                    suggest_reject: contradicted,
                    status: CandidateStatus::Proposed,
                    status_reason: None,
                    offset_ids: vec![],
                    sources,
                });
            }
        }
    }
    out
}

trait Clamp {
    fn clamp01(self) -> f64;
}
impl Clamp for f64 {
    fn clamp01(self) -> f64 {
        self.clamp(0.0, 1.0)
    }
}

impl EdgeCand {
    fn li_only(&self) -> bool {
        self.lli && self.td.is_empty() && self.gf.is_empty() && self.mw.is_empty()
    }
}

fn candidate_sources(
    ec_a: Option<&EdgeCand>,
    ec_b: Option<&EdgeCand>,
) -> Vec<String> {
    let mut s = BTreeMap::new();
    for e in [ec_a, ec_b].into_iter().flatten() {
        for id in &e.stat_ids {
            s.insert(id.clone(), ());
        }
    }
    s.into_keys().collect()
}
