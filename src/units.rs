use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meters,
    Cycles,
    CyclesPerSecond,
    DbHz,
    Seconds,
    Nanoseconds,
    Hertz,
    Megahertz,
    Dimensionless,
}

impl Unit {
    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Meters => "m",
            Unit::Cycles => "cycle",
            Unit::CyclesPerSecond => "cycle/s",
            Unit::DbHz => "dB-Hz",
            Unit::Seconds => "s",
            Unit::Nanoseconds => "ns",
            Unit::Hertz => "Hz",
            Unit::Megahertz => "MHz",
            Unit::Dimensionless => "1",
        }
    }
}

/// Every domain value moves through the system with its unit / coordinate
/// datum and original textual representation. Unconvertible input keeps
/// `value = None` while `raw` stays in place and gets a diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    pub value: Option<f64>,
    pub unit: Unit,
    pub datum: Option<String>,
    pub raw: String,
}

impl Quantity {
    pub fn parsed(value: f64, unit: Unit, raw: impl Into<String>) -> Self {
        Self { value: Some(value), unit, datum: None, raw: raw.into() }
    }
    pub fn raw_only(unit: Unit, raw: impl Into<String>) -> Self {
        Self { value: None, unit, datum: None, raw: raw.into() }
    }
    pub fn with_datum(mut self, datum: impl Into<String>) -> Self {
        self.datum = Some(datum.into());
        self
    }
}
