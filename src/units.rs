//! Units and coordinate datums for every domain quantity.
//!
//! Raw imports preserve the RINEX representation exactly.  Derived layers
//! normalize to the units declared here; values that cannot be converted stay
//! in place with a diagnostic instead of being dropped.

use serde::{Deserialize, Serialize};

pub const GPS_EPOCH_UNIX_SECONDS: i64 = 315_964_800; // 1980-01-06T00:00:00Z
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainUnit {
    Meter,
    Cycle,
    MeterPerSecond,
    Hertz,
    SecondSinceGps,
    Second,
    IntegerIndex,
    DecibelHertz,
    LliFlag,
}

impl DomainUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            DomainUnit::Meter => "m",
            DomainUnit::Cycle => "cycle",
            DomainUnit::MeterPerSecond => "m/s",
            DomainUnit::Hertz => "Hz",
            DomainUnit::SecondSinceGps => "s(GPS)",
            DomainUnit::Second => "s",
            DomainUnit::IntegerIndex => "1",
            DomainUnit::DecibelHertz => "dBHz",
            DomainUnit::LliFlag => "LLI",
        }
    }
}

/// Reference frame / time scale carried by every position or timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Datum {
    GpsTime,
    Itrf,
    RinexRaw,
    None,
}

impl Datum {
    pub fn as_str(&self) -> &'static str {
        match self {
            Datum::GpsTime => "GPS time",
            Datum::Itrf => "ITRF (case-declared)",
            Datum::RinexRaw => "RINEX raw representation",
            Datum::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RinexObsClass {
    PseudoRange,
    CarrierPhase,
    Doppler,
    SignalStrength,
}

/// RINEX 3 observation codes are exactly three characters: band + tracking
/// code.  The observation type (`C/L/D/S`) is carried separately.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObsCode {
    pub code: String,
}

impl ObsCode {
    pub fn new(code: &str) -> Self {
        ObsCode { code: code.to_string() }
    }
    pub fn band(&self) -> Option<char> {
        self.code.chars().nth(1)
    }
    pub fn tracking(&self) -> &str {
        if self.code.len() >= 3 {
            &self.code[2..]
        } else {
            ""
        }
    }
}

/// Convert civil GPS date-time (UTC calendar as written in a RINEX header) to
/// seconds since the GPS epoch on the same nominal time scale (case samples do
/// not apply leap-second transforms).
pub fn civil_to_gps_seconds(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: f64,
) -> i64 {
    civil_to_unix_seconds(year, month, day, hour, minute, second) - GPS_EPOCH_UNIX_SECONDS
}

pub fn civil_to_unix_seconds(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: f64,
) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i64;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 + hour as i64 * 3600 + minute as i64 * 60) + second.round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_epoch_and_known_offset() {
        assert_eq!(civil_to_gps_seconds(1980, 1, 6, 0, 0, 0.0), 0);
        // One GPS week.
        assert_eq!(civil_to_gps_seconds(1980, 1, 13, 0, 0, 0.0), 604_800);
        // 2024-01-01 is GPS week 2296 day 1 -> 1,387,987,200 s.
        assert_eq!(civil_to_gps_seconds(2024, 1, 1, 0, 0, 0.0), 1_387_987_200);
    }
}
