//! Unit-tagged quantities.
//!
//! Imported values always keep their *raw representation* ([`RawValue`]).
//! Normalised SI values live on the derived layer only. Values that cannot be
//! converted stay in place with a diagnostic instead of being dropped.

use serde::{Deserialize, Serialize};

/// How a value was written in the source file, before any normalisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawValue {
    /// Exact token as it appeared in the source.
    pub text: String,
    /// Unit/representation declared by the source ("m", "cyc", "Hz", "dbHz", ...).
    pub unit: String,
}

/// A quantity with a normalised SI value plus the verbatim source form.
///
/// `value` is `None` when the raw representation could not be converted; the
/// raw value is still retained and `diagnostic` explains the failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Si {
    /// SI-normalised value (`m`, `m/s`, `Hz`, ...), `None` if unconvertible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// SI unit of [`Si::value`] ("m", "m/s", "Hz", "dB-Hz", "dimensionless").
    pub si_unit: &'static str,
    pub raw: RawValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl Si {
    pub fn new(value: Option<f64>, si_unit: &'static str, raw_text: &str, raw_unit: &str) -> Self {
        Si {
            value,
            si_unit,
            raw: RawValue { text: raw_text.to_string(), unit: raw_unit.to_string() },
            diagnostic: None,
        }
    }

    pub fn ok(value: f64, si_unit: &'static str, raw_text: impl Into<String>, raw_unit: impl Into<String>) -> Self {
        Si { value: Some(value), si_unit, raw: RawValue { text: raw_text.into(), unit: raw_unit.into() }, diagnostic: None }
    }

    pub fn bad(si_unit: &'static str, raw_text: impl Into<String>, raw_unit: impl Into<String>, why: impl Into<String>) -> Self {
        Si {
            value: None,
            si_unit,
            raw: RawValue { text: raw_text.into(), unit: raw_unit.into() },
            diagnostic: Some(why.into()),
        }
    }

    pub fn is_converted(&self) -> bool {
        self.value.is_some()
    }
}

/// Unit conversions understood by the import layer.
///
/// The import layer never silently drops an unknown unit.
pub fn convert_to_si(text: &str, unit: &str) -> Result<f64, String> {
    let v: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("numeric value not parseable: {:?}", text))?;
    let factor = match unit {
        "m" | "meter" | "meters" | "metre" | "metres" => 1.0,
        "km" => 1_000.0,
        "cyc" | "cycle" | "cycles" => 1.0, // cycles are dimensionless counts
        "Hz" | "hz" => 1.0,
        "kHz" => 1_000.0,
        "MHz" => 1_000_000.0,
        "GHz" => 1_000_000_000.0,
        "m/s" | "mps" => 1.0,
        "km/s" | "kms" => 1_000.0,
        "dbHz" | "dB-Hz" | "dBHz" | "dbhz" => 1.0,
        "" => 1.0,
        other => return Err(format!("unknown unit/representation {:?}", other)),
    };
    Ok(v * factor)
}

/// Coordinate reference frame carried by every geometric quantity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Datum {
    pub frame: String,
}

impl Default for Datum {
    fn default() -> Self {
        Datum { frame: "ITRF-like synthetic (metres)".to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_form_is_retained_on_success_and_failure() {
        let ok = Si::new(convert_to_si("1.2", "km").ok(), "m", "1.2", "km");
        assert_eq!(ok.value, Some(1200.0));
        assert_eq!(ok.raw.text, "1.2");
        let bad = convert_to_si("n/a", "m");
        assert!(bad.is_err());
        let kept = Si::bad("m", "n/a", "m", bad.unwrap_err());
        assert!(kept.value.is_none());
        assert!(kept.diagnostic.is_some());
        assert_eq!(kept.raw.text, "n/a");
    }
}
