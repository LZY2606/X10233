use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const TIME_DATUM: &str = "GPST";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Unit {
    Meters,
    Cycles,
    Hertz,
    DbHz,
    Dimensionless,
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Meters => "m",
            Unit::Cycles => "cyc",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Dimensionless => "1",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quantity {
    pub value: f64,
    pub unit: Unit,
    pub datum: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub station: String,
    pub sat: String,
    pub epoch: f64,
    pub epoch_datum: String,
    pub code: String,
    pub freq: Option<u8>,
    pub raw: String,
    pub quantity: Option<Quantity>,
    pub lli: Option<u8>,
    pub ssi: Option<u8>,
    pub diagnostics: Vec<String>,
}

impl Observation {
    pub fn compute_id(&mut self) {
        self.id = stable_id(
            "obs",
            &serde_json::json!({
                "station": self.station,
                "sat": self.sat,
                "epoch": self.epoch,
                "code": self.code,
                "raw": self.raw,
                "lli": self.lli,
                "ssi": self.ssi,
            }),
        );
    }
}

pub fn stable_id(prefix: &str, canonical: &serde_json::Value) -> String {
    let text = serde_json::to_string(canonical).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{hex}")
}

pub fn obs_code_unit(code: &str) -> Option<Unit> {
    match code.chars().next()? {
        'C' => Some(Unit::Meters),
        'L' => Some(Unit::Cycles),
        'D' => Some(Unit::Hertz),
        'S' => Some(Unit::DbHz),
        _ => None,
    }
}

pub fn obs_band(code: &str) -> Option<u8> {
    code.chars().nth(1)?.to_digit(10).map(|d| d as u8)
}

pub fn carrier_freq_hz(sat: &str, band: u8, freq_number: Option<u8>) -> Option<f64> {
    let system = sat.chars().next()?;
    match (system, band) {
        ('G', 1) => Some(1575.42e6),
        ('G', 2) => Some(1227.60e6),
        ('G', 5) => Some(1176.45e6),
        ('R', 1) => {
            let k = freq_number.unwrap_or(0) as f64;
            Some(1602.0e6 + k * 0.5625e6)
        }
        ('R', 2) => {
            let k = freq_number.unwrap_or(0) as f64;
            Some(1246.0e6 + k * 0.4375e6)
        }
        _ => None,
    }
}

pub const C_LIGHT: f64 = 299_792_458.0;

pub fn wavelength_m(sat: &str, band: u8, freq_number: Option<u8>) -> Option<f64> {
    carrier_freq_hz(sat, band, freq_number).map(|f| C_LIGHT / f)
}
