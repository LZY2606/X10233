//! Units, coordinate datums and the `Quantity` newtype.
//!
//! Every domain value derived from an observation is rendered as a
//! `Quantity { value, unit, datum }`. Raw textual representations are kept
//! verbatim in the import layer; conversion happens only in the derived layer.

use serde::{Deserialize, Serialize};

/// Physical unit tag carried alongside every numeric domain value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    /// Pseudorange in metres.
    Meter,
    /// Carrier phase in whole cycles of the signal band.
    Cycle,
    /// Doppler in hertz (cycles/second).
    Hertz,
    /// Signal strength in dB-Hz.
    DbHz,
    /// Time interval in seconds.
    Second,
    /// Wavelength in metres per cycle.
    MeterPerCycle,
    /// Frequency in megahertz.
    Megahertz,
    /// Dimensionless ratio / combination.
    Dimensionless,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cycle",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dBHz",
            Unit::Second => "s",
            Unit::MeterPerCycle => "m/cycle",
            Unit::Megahertz => "MHz",
            Unit::Dimensionless => "1",
        }
    }
}

/// Reference frame / clock datum tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Datum {
    /// Coordinate reference frame used by the station metadata.
    Itrf2020,
    /// Receiver clock datum (clock jumps are measured on it).
    ReceiverClock,
    /// Satellite clock / broadcast ephemeris datum.
    Broadcast,
    /// No spatial datum (pure signal-domain quantity).
    None,
}

impl Datum {
    pub fn as_str(self) -> &'static str {
        match self {
            Datum::Itrf2020 => "ITRF2020",
            Datum::ReceiverClock => "receiver_clock",
            Datum::Broadcast => "broadcast",
            Datum::None => "none",
        }
    }
}

impl Default for Datum {
    fn default() -> Self {
        Datum::None
    }
}

/// A numeric value that always carries its physical unit and datum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    pub value: f64,
    pub unit: Unit,
    #[serde(default)]
    pub datum: Datum,
}

impl Quantity {
    pub fn new(value: f64, unit: Unit) -> Self {
        Quantity {
            value,
            unit,
            datum: Datum::None,
        }
    }

    pub fn with_datum(mut self, datum: Datum) -> Self {
        self.datum = datum;
        self
    }

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "value": round9(self.value),
            "unit": self.unit.as_str(),
            "datum": self.datum.as_str(),
        })
    }
}

/// Round to 9 decimal places so persisted JSON is stable and diff-friendly,
/// while keeping sub-micrometre precision for phase combinations.
pub fn round9(x: f64) -> f64 {
    if x.is_finite() {
        (x * 1e9).round() / 1e9
    } else {
        x
    }
}

/// Canonical RINEX observation kinds this project understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange,
    CarrierPhase,
    Doppler,
    /// Carrier-to-noise density ratio.
    Snr,
}

impl ObsKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "C",
            ObsKind::CarrierPhase => "L",
            ObsKind::Doppler => "D",
            ObsKind::Snr => "S",
        }
    }

    pub fn unit(self) -> Unit {
        match self {
            ObsKind::Pseudorange => Unit::Meter,
            ObsKind::CarrierPhase => Unit::Cycle,
            ObsKind::Doppler => Unit::Hertz,
            ObsKind::Snr => Unit::DbHz,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "C" => Some(ObsKind::Pseudorange),
            "L" => Some(ObsKind::CarrierPhase),
            "D" => Some(ObsKind::Doppler),
            "S" => Some(ObsKind::Snr),
            _ => None,
        }
    }
}
