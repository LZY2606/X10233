// Core data structures and deterministic identity/fingerprint helpers.

use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "cycle-slip-algo-1.0.0";

#[derive(Clone, Copy, Debug)]
pub struct DetectorParams {
    pub gf_threshold_m: f64,
    pub mw_threshold_cy: f64,
    pub td_threshold_cy: f64,
    pub clock_threshold_m: f64,
    pub max_lattice_n: i64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            gf_threshold_m: 0.05,
            mw_threshold_cy: 0.25,
            td_threshold_cy: 0.30,
            clock_threshold_m: 100_000.0,
            max_lattice_n: 5,
        }
    }
}

impl DetectorParams {
    pub fn from_map(m: &serde_json::Map<String, serde_json::Value>) -> Self {
        let mut p = Self::default();
        if let Some(v) = m.get("gf_threshold_m").and_then(|x| x.as_f64()) {
            p.gf_threshold_m = v;
        }
        if let Some(v) = m.get("mw_threshold_cy").and_then(|x| x.as_f64()) {
            p.mw_threshold_cy = v;
        }
        if let Some(v) = m.get("td_threshold_cy").and_then(|x| x.as_f64()) {
            p.td_threshold_cy = v;
        }
        if let Some(v) = m.get("clock_threshold_m").and_then(|x| x.as_f64()) {
            p.clock_threshold_m = v;
        }
        if let Some(v) = m.get("max_lattice_n").and_then(|x| x.as_i64()) {
            p.max_lattice_n = v;
        }
        p
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "gf_threshold_m": self.gf_threshold_m,
            "mw_threshold_cy": self.mw_threshold_cy,
            "td_threshold_cy": self.td_threshold_cy,
            "clock_threshold_m": self.clock_threshold_m,
            "max_lattice_n": self.max_lattice_n,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ObsRow {
    pub id: String,
    pub submission_id: String,
    pub line_id: String,
    pub epoch_idx: i64,
    pub sat: String,
    pub gnss: String,
    pub band: String,
    pub code_phase: String,
    pub code_pseudo: Option<String>,
    pub code_doppler: Option<String>,
    pub code_snr: Option<String>,
    pub code_lli: Option<String>,
    pub phase_cy: Option<f64>,
    pub pseudo_m: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub lli_bits: Option<i64>,
    pub value_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochInfo {
    pub idx: i64,
    pub t_seconds: f64,
    pub iso_tag: String,
    pub rcvr_clk_s: String,
    pub clock_event_flag: i64,
}

#[derive(Debug, Clone)]
pub struct FrequencyBasis {
    pub gnss: String,
    pub band: String,
    pub freq_hz: f64,
    pub wavelength_m: f64,
    pub freq_number: Option<i64>,
    pub codes_raw: String,
}

// Deterministic FNV-1a 64-bit hash -> stable short fingerprint. No randomness.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn short_hash(s: &str) -> String {
    format!("{:016x}", fnv1a64(s.as_bytes()))
}

pub fn param_fingerprint(p: &DetectorParams) -> String {
    // Canonical, ordered representation for stable fingerprints.
    let canon = format!(
        "gf={:.6}|mw={:.6}|td={:.6}|clk={:.3}|n={}|algo={}",
        p.gf_threshold_m,
        p.mw_threshold_cy,
        p.td_threshold_cy,
        p.clock_threshold_m,
        p.max_lattice_n,
        ALGO_VERSION
    );
    short_hash(&canon)
}

// Stable identity builders. Identities encode semantic coordinates so they are
// reproducible after export -> delete -> import.
pub fn id_obs(project: &str, sub: &str, sat: &str, epoch: i64, band: &str) -> String {
    format!("obs-{}-{}", short_hash(&format!("{project}|{sub}|{sat}|{epoch}|{band}")), "")
        .trim_end_matches('-')
        .to_string()
}
pub fn id_line(sub: &str, n: i64) -> String {
    format!("ln-{}", short_hash(&format!("{sub}|{n}")))
}
pub fn id_epoch(project: &str, idx: i64) -> String {
    format!("ep-{}-{}", short_hash(project), idx)
}
pub fn id_freq(project: &str, gnss: &str, band: &str) -> String {
    format!("fq-{}", short_hash(&format!("{project}|{gnss}|{band}")))
}
pub fn id_run(branch: &str, seq: i64) -> String {
    format!("run-{}-{}", short_hash(branch), seq)
}
pub fn id_arc(run: &str, sat: &str, band: &str, start: i64) -> String {
    format!("arc-{}", short_hash(&format!("{run}|{sat}|{band}|{start}")))
}
pub fn id_res(run: &str, det: &str, sat: &str, epoch: i64, band: &str) -> String {
    format!("rs-{det}-{}", short_hash(&format!("{run}|{sat}|{epoch}|{band}")))
}
pub fn id_cand(run: &str, sat: &str, epoch: i64, kind: &str, n1: String, n2: String) -> String {
    format!(
        "cd-{}",
        short_hash(&format!("{run}|{sat}|{epoch}|{kind}|{n1}|{n2}"))
    )
}
