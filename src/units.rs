//! Units and coordinate/measurement datums.
//!
//! Every domain quantity carries its physical unit and, where relevant, the
//! datum it is expressed in. Values that could not be converted are kept in
//! place with a diagnostic instead of being dropped for plotting.

use serde::{Deserialize, Serialize};

/// A physical quantity that always carries its unit and datum/frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Qty {
    /// Canonical numeric value in `unit` (or `None` if not convertible).
    pub value: Option<f64>,
    /// Physical unit of `value`, e.g. "m", "cycle", "Hz", "s", "dB-Hz".
    pub unit: String,
    /// Reference frame / datum, e.g. "WGS84", "GPST", "RAW".
    pub datum: String,
    /// Original textual representation exactly as imported.
    pub raw: String,
    /// Diagnostic when `value` is `None` or the conversion is partial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl Qty {
    /// A fully converted quantity.
    pub fn new(value: f64, unit: impl Into<String>, datum: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            value: Some(value),
            unit: unit.into(),
            datum: datum.into(),
            raw: raw.into(),
            diagnostic: None,
        }
    }

    /// Keep an unconvertible raw token in place with a diagnostic.
    pub fn unconvertible(raw: impl Into<String>, unit: impl Into<String>, diagnostic: impl Into<String>) -> Self {
        Self {
            value: None,
            unit: unit.into(),
            datum: "RAW".to_string(),
            raw: raw.into(),
            diagnostic: Some(diagnostic.into()),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.value.is_some() && self.diagnostic.is_none()
    }
}

/// Standard unit labels used across the project.
pub mod unit {
    pub const METER: &str = "m";
    pub const CYCLE: &str = "cycle";
    pub const HZ: &str = "Hz";
    pub const SECOND: &str = "s";
    pub const DBHZ: &str = "dB-Hz";
    pub const COUNT: &str = "1";
}

/// Standard datums/frames.
pub mod datum {
    pub const RAW: &str = "RAW";
    pub const GPST: &str = "GPST";
    pub const WGS84: &str = "WGS84";
}
