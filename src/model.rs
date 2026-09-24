//! Domain model: stable identities, physical constants, detector parameters.
//!
//! All domain quantities carry units. Raw import values are preserved as text;
//! only the derived layer normalizes. Logical time is the event sequence
//! number (`seq`) — no wall clock is used anywhere in domain logic.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const C_M_PER_S: f64 = 299_792_458.0;
pub const F1_HZ: f64 = 1_575_420_000.0;
pub const F2_HZ: f64 = 1_227_600_000.0;
pub const LAMBDA1_M: f64 = C_M_PER_S / F1_HZ;
pub const LAMBDA2_M: f64 = C_M_PER_S / F2_HZ;
pub const LAMBDA_WL_M: f64 = C_M_PER_S / (F1_HZ - F2_HZ);

/// Algorithm version stamped on every derived row.
pub const ALGO_VERSION: &str = "gnss-detect/1.0.0";

/// Content-addressed, stable identity. Re-importing identical content yields
/// the identical id, which is what keeps identity stable across export /
/// delete / re-import.
pub fn content_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0x1f]);
    }
    let digest = h.finalize();
    let mut out = String::with_capacity(prefix.len() + 21);
    out.push_str(prefix);
    out.push('_');
    for b in &digest[..10] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// Arc break when the gap between consecutive epochs exceeds this (seconds).
    pub max_epoch_gap_s: f64,
    /// Geometry-free (ionosphere) combination jump threshold, meters.
    pub gf_threshold_m: f64,
    /// Tolerance when matching integer (n1, n2) pairs to a GF jump, meters.
    pub gf_hypothesis_tol_m: f64,
    /// Melbourne-Wubbena jump threshold, wide-lane cycles.
    pub mw_threshold_cycles: f64,
    /// Tolerance when rounding a MW jump to wide-lane integers, cycles.
    pub mw_hypothesis_tol_cycles: f64,
    /// Time-differenced code-minus-carrier threshold, meters.
    pub td_threshold_m: f64,
    /// Fraction of satellites sharing a jump that classifies it as a
    /// whole-receiver clock event instead of per-satellite slips.
    pub clock_common_ratio: f64,
    /// Spread allowed among satellites sharing a clock jump, meters.
    pub clock_tolerance_m: f64,
    /// Search bound for integer hypotheses, cycles.
    pub hypothesis_max_cycles: i32,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            max_epoch_gap_s: 120.0,
            gf_threshold_m: 0.15,
            gf_hypothesis_tol_m: 0.03,
            mw_threshold_cycles: 0.5,
            mw_hypothesis_tol_cycles: 0.5,
            td_threshold_m: 0.8,
            clock_common_ratio: 0.6,
            clock_tolerance_m: 50.0,
            hypothesis_max_cycles: 10,
        }
    }
}

impl DetectorParams {
    /// Parameter fingerprint carried by every derived candidate.
    pub fn fingerprint(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        content_id("par", &[&json])
    }
}

/// One repair hypothesis: whole-cycle offsets per frequency band.
/// Repairs are stored as an offset layer only; raw observations are never
/// rewritten.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Hypothesis {
    pub label: String,
    /// (frequency band, whole cycles)
    pub offsets: Vec<(i32, i64)>,
}

impl Hypothesis {
    pub fn label_for(offsets: &[(i32, i64)]) -> String {
        offsets
            .iter()
            .map(|(f, n)| format!("L{f}{n:+}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}
