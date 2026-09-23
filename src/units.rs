//! 所有领域量都携带单位或坐标基准。导入层保留原始表示，派生层才做规范化；
//! 无法转换的值留在原位置（value=null）并附诊断，绝不为了画图而丢掉。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Meter,
    Cycle,
    MeterPerSecond,
    DbHz,
    Second,
    MeterPerSecondSquared,
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cycle",
            Unit::MeterPerSecond => "m/s",
            Unit::DbHz => "dBHz",
            Unit::Second => "s",
            Unit::MeterPerSecondSquared => "m/s^2",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datum {
    /// 世界大地测量基准，ECEF 直角坐标
    Wgs84Ecef,
    /// GPS 连续周内秒
    GpsTime,
    Unitless,
}

impl Datum {
    pub fn as_str(&self) -> &'static str {
        match self {
            Datum::Wgs84Ecef => "WGS84/ECEF",
            Datum::GpsTime => "GPS:week-seconds",
            Datum::Unitless => "unitless",
        }
    }
}

/// 带单位/基准的领域量。raw 是导入时的原始表示，永不被覆盖。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Qty {
    pub value: Option<f64>,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
    pub raw: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

impl Qty {
    pub fn parsed(v: f64, unit: Unit, raw: impl Into<String>) -> Self {
        Qty {
            value: Some(v),
            unit: unit.as_str().to_string(),
            datum: None,
            raw: raw.into(),
            diagnostics: vec![],
        }
    }
    pub fn parsed_datum(v: f64, unit: Unit, datum: Datum, raw: impl Into<String>) -> Self {
        Qty {
            value: Some(v),
            unit: unit.as_str().to_string(),
            datum: Some(datum.as_str().to_string()),
            raw: raw.into(),
            diagnostics: vec![],
        }
    }
    pub fn unparsable(unit: Unit, raw: impl Into<String>, why: impl Into<String>) -> Self {
        Qty {
            value: None,
            unit: unit.as_str().to_string(),
            datum: None,
            raw: raw.into(),
            diagnostics: vec![why.into()],
        }
    }
    pub fn with_datum(mut self, datum: Datum) -> Self {
        self.datum = Some(datum.as_str().to_string());
        self
    }
}

/// 历元时间：保留 RINEX 原始日历表示，同时给出 GPS 周内秒（派生）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: f64,
    /// GPS 周
    pub gps_week: i32,
    /// GPS 周内连续秒（基准 GPS:week-seconds）
    pub gps_sow: f64,
    /// 单调时间轴上的连续秒（跨周），由 gps_week*604800+sow 组成
    pub axis_seconds: f64,
    pub raw: String,
}

/// 把 (年月日时分秒, 两位/四位年) 转成 GPS 周/周内秒。
/// GPS 起点 1980-01-06T00:00:00Z；样例全部位于 UTC 2026，闰秒固定按 18s 处理
/// （仅用于离线样例的时间轴，不引入外部星历）。
pub fn calendar_to_gps(year: i32, month: u32, day: u32, hh: u32, mm: u32, ss: f64) -> (i32, f64) {
    let days = days_from_civil(year as i64, month as i64, day as i64)
        - days_from_civil(1980, 1, 6);
    let leap_seconds = 18.0_f64;
    let sow = days as f64 * 86_400.0 + hh as f64 * 3600.0 + mm as f64 * 60.0 + ss
        + leap_seconds;
    let week = (sow / 604_800.0).floor() as i32;
    (week, sow - week as f64 * 604_800.0)
}

pub fn gps_axis(week: i32, sow: f64) -> f64 {
    week as f64 * 604_800.0 + sow
}

// Howard Hinnant civil-from-days 算法
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
