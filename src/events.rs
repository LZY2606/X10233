//! Append-only event log. The log is the single source of truth; derived
//! tables are projections of it and may be rebuilt or invalidated.

use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "carrier-break-station/algo@1.0.0";

/// Virtual clock: deterministic integer ticks, never wall-clock time.
#[derive(Debug, Clone, Copy)]
pub struct VirtualClock {
    tick: i64,
    base: i64,
}

impl VirtualClock {
    pub fn new(seed: u64) -> Self {
        VirtualClock {
            tick: 0,
            base: 1_700_000_000i64 + (seed as i64 % 100_000),
        }
    }
    pub fn now_unix(&mut self) -> i64 {
        self.tick += 1;
        self.base + self.tick
    }
    pub fn now_iso(&mut self) -> String {
        let t = self.now_unix();
        format!("1970-01-01T00:00:{:04}Z", t.rem_euclid(10_000))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    ProjectCreated,
    /// A batch of raw observations committed (immutable thereafter).
    SourceCommitted,
    /// Detector parameters revised; produces a new parameter version.
    ParamsRevised,
    /// Derived computation (arcs/candidates) materialised for a version.
    DerivedComputed,
    /// Derived results marked stale because an input they depend on changed.
    DerivedStale,
    /// Human accepted a candidate / repair offset layer.
    CandidateAccepted,
    /// Human rejected a candidate, with a reason.
    CandidateRejected,
    /// Human flagged a candidate as ambiguous/undecidable.
    CandidateDeferred,
    /// Receiver clock jump classification (kept separate from slips).
    ClockClassified,
    /// LLI flag confirmed as false alarm.
    LliFalseAlarm,
    /// Export package produced.
    Exported,
    /// Export package imported into a fresh project.
    Imported,
    /// Reviewer revisited an old version (read-only time travel).
    Revisited,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::ProjectCreated => "project_created",
            EventKind::SourceCommitted => "source_committed",
            EventKind::ParamsRevised => "params_revised",
            EventKind::DerivedComputed => "derived_computed",
            EventKind::DerivedStale => "derived_stale",
            EventKind::CandidateAccepted => "candidate_accepted",
            EventKind::CandidateRejected => "candidate_rejected",
            EventKind::CandidateDeferred => "candidate_deferred",
            EventKind::ClockClassified => "clock_classified",
            EventKind::LliFalseAlarm => "lli_false_alarm",
            EventKind::Exported => "exported",
            EventKind::Imported => "imported",
            EventKind::Revisited => "revisited",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "project_created" => EventKind::ProjectCreated,
            "source_committed" => EventKind::SourceCommitted,
            "params_revised" => EventKind::ParamsRevised,
            "derived_computed" => EventKind::DerivedComputed,
            "derived_stale" => EventKind::DerivedStale,
            "candidate_accepted" => EventKind::CandidateAccepted,
            "candidate_rejected" => EventKind::CandidateRejected,
            "candidate_deferred" => EventKind::CandidateDeferred,
            "clock_classified" => EventKind::ClockClassified,
            "lli_false_alarm" => EventKind::LliFalseAlarm,
            "exported" => EventKind::Exported,
            "imported" => EventKind::Imported,
            "revisited" => EventKind::Revisited,
            _ => return None,
        })
    }
}

/// One immutable log entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    /// Version (content hash) created *by* this event, when applicable.
    pub version_id: String,
    /// Parent version this event forks from.
    pub parent_version_id: String,
    /// Version this event supersedes (for revisions), else "".
    pub supersedes: String,
    /// Deterministic virtual timestamp.
    pub at_virtual: String,
    /// Free-form machine-readable payload.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Direct source object ids this event refers to.
    #[serde(default)]
    pub sources: Vec<String>,
    pub actor: String,
    pub note: String,
}

/// Tunable detector parameters. Each revision is content-addressed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorParams {
    /// Threshold on geometry-free jump, in metres.
    pub if_threshold_m: f64,
    /// Threshold on MW wide-lane jump, in wide-lane cycles.
    pub mw_threshold_cycles: f64,
    /// Threshold on time-difference residual, in carrier cycles.
    pub td_threshold_cycles: f64,
    /// Minimum gap (seconds) at which Doppler extrapolation is still trusted.
    pub td_max_gap_s: f64,
    /// SNR floor in dB-Hz below which evidence is marked weak.
    pub snr_floor_dbhz: f64,
    /// Cycles within which integer hypotheses are considered tied.
    pub tie_tolerance_cycles: f64,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            if_threshold_m: 0.05,
            mw_threshold_cycles: 0.75,
            td_threshold_cycles: 0.75,
            td_max_gap_s: 35.0,
            snr_floor_dbhz: 30.0,
            tie_tolerance_cycles: 0.25,
        }
    }
}

impl DetectorParams {
    /// Deterministic fingerprint of the parameter set.
    pub fn fingerprint(&self) -> String {
        let canonical = format!(
            "if={:.6}|mw={:.6}|td={:.6}|gap={:.6}|snr={:.6}|tie={:.6}",
            self.if_threshold_m,
            self.mw_threshold_cycles,
            self.td_threshold_cycles,
            self.td_max_gap_s,
            self.snr_floor_dbhz,
            self.tie_tolerance_cycles
        );
        crate::ids::short_hash(canonical.as_bytes())
    }

    /// Apply one revision expressed as a JSON object; unchanged fields keep
    /// their values. Returns an error string on invalid values.
    pub fn revise(&mut self, patch: &serde_json::Value) -> Result<(), String> {
        let mut next = self.clone();
        if let Some(v) = patch.get("if_threshold_m") {
            next.if_threshold_m = positive(v, "if_threshold_m")?;
        }
        if let Some(v) = patch.get("mw_threshold_cycles") {
            next.mw_threshold_cycles = positive(v, "mw_threshold_cycles")?;
        }
        if let Some(v) = patch.get("td_threshold_cycles") {
            next.td_threshold_cycles = positive(v, "td_threshold_cycles")?;
        }
        if let Some(v) = patch.get("td_max_gap_s") {
            next.td_max_gap_s = positive(v, "td_max_gap_s")?;
        }
        if let Some(v) = patch.get("snr_floor_dbhz") {
            next.snr_floor_dbhz = positive(v, "snr_floor_dbhz")?;
        }
        if let Some(v) = patch.get("tie_tolerance_cycles") {
            next.tie_tolerance_cycles = positive(v, "tie_tolerance_cycles")?;
        }
        *self = next;
        Ok(())
    }
}

fn positive(v: &serde_json::Value, name: &str) -> Result<f64, String> {
    let x = v
        .as_f64()
        .ok_or_else(|| format!("parameter {name} must be a number"))?;
    if !x.is_finite() || x <= 0.0 {
        return Err(format!("parameter {name} must be positive"));
    }
    Ok(x)
}
