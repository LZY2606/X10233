//! 显式单位/基准枚举。所有领域量在事件中携带单位或坐标基准。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meter,
    Cycles,
    Hertz,
    Second,
    MeterPerSecond,
    Dimensionless,
    Unknown,
}

impl Unit {
    pub fn as_label(self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycles => "cycle",
            Unit::Hertz => "Hz",
            Unit::Second => "s",
            Unit::MeterPerSecond => "m/s",
            Unit::Dimensionless => "-",
            Unit::Unknown => "?",
        }
    }
}

/// 坐标/时间基准。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Datum {
    pub frame: String,
    pub time_scale: String,
}

impl Default for Datum {
    fn default() -> Self {
        Datum { frame: "ITRF".to_string(), time_scale: "GPS".to_string() }
    }
}
