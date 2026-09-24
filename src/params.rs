//! Detector parameter set with deterministic fingerprints.
//!
//! A parameter revision never mutates old derived rows: the fingerprint is
//! part of every derived identity, so revising a threshold forks the derived
//! layer while leaving the old viewpoint fully replayable.

use crate::ids;
use serde::{Deserialize, Serialize};

pub const ALG_VERSION: &str = "carrier-breakpoint-core-1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorParams {
    /// Nominal epoch interval (s). Gaps beyond `gap_factor` times it are
    /// flagged irregular but do not by themselves break arcs.
    pub nominal_dt_secs: f64,
    pub gap_factor: f64,
    /// Time-difference acceptance to the nearest integer (cycles).
    pub td_tol_cycles: f64,
    /// Geometry-free combination tolerance (metres).
    pub gf_tol_m: f64,
    /// Melbourne-Wubbena wide-lane tolerance (wide-lane cycles).
    pub mw_tol_wl_cycles: f64,
    /// Half-width of integer search.
    pub max_search_cycles: i64,
    /// Minimum number of satellites sharing a common-mode jump for the engine
    /// to corroborate a receiver clock jump.
    pub clock_consensus_min_sats: usize,
    /// Declared clock-event threshold (ns).
    pub clock_event_threshold_ns: f64,
    /// Frequency band numbers used for GF / MW combinations, as raw labels.
    pub gf_band_a: String,
    pub gf_band_b: String,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            nominal_dt_secs: 30.0,
            gap_factor: 1.5,
            td_tol_cycles: 0.25,
            gf_tol_m: 0.03,
            mw_tol_wl_cycles: 0.25,
            max_search_cycles: 6,
            clock_consensus_min_sats: 2,
            clock_event_threshold_ns: 500.0,
            gf_band_a: "1".to_string(),
            gf_band_b: "2".to_string(),
        }
    }
}

impl DetectorParams {
    /// Canonical field-ordered fingerprint material (no map, no floats beyond
    /// fixed rendering).
    pub fn fingerprint(&self) -> String {
        let m = format!(
            "dt={:.6};gap={:.6};td={:.6};gf={:.6};mw={:.6};m={};clk_ns={:.6};cons={};ba={};bb={};alg={}",
            self.nominal_dt_secs,
            self.gap_factor,
            self.td_tol_cycles,
            self.gf_tol_m,
            self.mw_tol_wl_cycles,
            self.max_search_cycles,
            self.clock_event_threshold_ns,
            self.clock_consensus_min_sats,
            self.gf_band_a,
            self.gf_band_b,
            ALG_VERSION
        );
        ids::stable_id("fp", &m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fingerprint_stable_and_sensitive() {
        let a = DetectorParams::default().fingerprint();
        let b = DetectorParams::default().fingerprint();
        assert_eq!(a, b);
        let mut p = DetectorParams::default();
        p.td_tol_cycles += 0.01;
        assert_ne!(a, p.fingerprint());
    }
}
