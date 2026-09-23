//! Analysis rules and parameters.
//!
//! Rule revisions are first-class: every parameter set has a semantic
//! fingerprint independent of who submitted it and when. Derived results record
//! the rule version + fingerprint, so a replayed analysis at an old event uses
//! exactly the rules visible then.

use serde::{Deserialize, Serialize};

pub const ALGORITHM_VERSION: &str = "cb-gnss-rules-1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// Minimum epoch spacing gap (s) that creates an arc time boundary.
    pub gap_tolerance_s: f64,
    /// GF (geometry-free ionosphere combination) jump threshold, cycles.
    pub gf_threshold_cycles: f64,
    /// GF residual accepted within this distance of an integer pair, cycles.
    pub gf_round_tol_cycles: f64,
    /// MW widelane jump threshold, wide-lane cycles.
    pub mw_threshold_cycles: f64,
    /// MW deviation from running mean that declares a jump, cycles.
    pub mw_jump_tol_cycles: f64,
    /// TD predicted-minus-observed phase threshold, cycles per band.
    pub td_threshold_cycles: f64,
    /// Common-mode shift (fraction of an L1 cycle) declared a receiver clock
    /// jump rather than per-satellite slips.
    pub clock_common_fraction: f64,
    /// Minimum satellites contributing to the common-mode clock estimator.
    pub clock_min_sats: usize,
    /// |n1| bound when enumerating equivalent integer pairs.
    pub equal_pair_bound: i64,
    /// LLI flag is a hint only; this is its evidence weight.
    pub lli_weight: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            gap_tolerance_s: 30.0,
            gf_threshold_cycles: 0.25,
            gf_round_tol_cycles: 0.15,
            mw_threshold_cycles: 0.25,
            mw_jump_tol_cycles: 0.5,
            td_threshold_cycles: 0.5,
            clock_common_fraction: 0.5,
            clock_min_sats: 3,
            equal_pair_bound: 12,
            lli_weight: 1.0,
        }
    }
}

impl Params {
    /// Semantic fingerprint: canonical JSON (sorted keys) over stable fields.
    pub fn fingerprint(&self) -> String {
        let canon = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            ALGORITHM_VERSION,
            self.gap_tolerance_s,
            self.gf_threshold_cycles,
            self.gf_round_tol_cycles,
            self.mw_threshold_cycles,
            self.mw_jump_tol_cycles,
            self.td_threshold_cycles,
            self.clock_common_fraction,
            self.clock_min_sats,
            self.equal_pair_bound,
            self.lli_weight,
        );
        crate::id::content_fp(&canon)
    }
}
