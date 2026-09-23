//! Unit- and datum-tagged quantities. Values that cannot be converted stay at
//! their original position as raw tokens with an attached diagnostic; they are
//! never dropped for plotting.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Qty {
    pub v: f64,
    pub u: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
}

impl Qty {
    pub fn new(v: f64, u: &str) -> Self {
        Qty { v, u: u.to_string(), datum: None }
    }
    pub fn with_datum(mut self, d: &str) -> Self {
        self.datum = Some(d.to_string());
        self
    }
}

/// A field as preserved from the RINEX record: raw token plus, when convertible,
/// a normalized value with its physical unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawField {
    pub raw: String,
    pub v: Option<f64>,
    pub u: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lli: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}
