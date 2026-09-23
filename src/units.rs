//! 领域量：所有量携带单位或坐标基准；原始表示保留，派生层才规范化；
//! 无法转换的值留在原位置并附诊断。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Cycles,
    Meters,
    Hz,
    DbHz,
    Seconds,
    Dimensionless,
    Unknown,
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Cycles => "cyc",
            Unit::Meters => "m",
            Unit::Hz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Seconds => "s",
            Unit::Dimensionless => "1",
            Unit::Unknown => "?",
        }
    }
}

/// 坐标/时间基准，例如 GPS 时、接收机钟参考。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basis {
    GpsTime,
    ReceiverClock,
    Unspecified(String),
}

/// 一个带单位与基准的量。`raw` 为导入时的原始字符串表示，永不丢失。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantity {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: Unit,
    pub basis: Option<Basis>,
    /// 无法解析 / 转换时的诊断；为 None 表示该量可用。
    pub diagnostic: Option<String>,
}

impl Quantity {
    pub fn ok(raw: &str, value: f64, unit: Unit) -> Self {
        Quantity { raw: raw.to_string(), value: Some(value), unit, basis: None, diagnostic: None }
    }
    pub fn bad(raw: &str, unit: Unit, diag: &str) -> Self {
        Quantity {
            raw: raw.to_string(),
            value: None,
            unit,
            basis: None,
            diagnostic: Some(diag.to_string()),
        }
    }
}

/// 把 RINEX 观测字段的原始字符串解析为带单位量；空字段 / 非法字段留在原位置并附诊断。
pub fn parse_obs_field(raw: &str, unit: Unit) -> Quantity {
    let t = raw.trim();
    if t.is_empty() {
        return Quantity::bad(raw, unit, "空观测字段：该历元此观测码缺失");
    }
    match t.parse::<f64>() {
        Ok(v) => Quantity::ok(raw, v, unit),
        Err(_) => Quantity::bad(raw, unit, "无法解析为浮点数，保留原始表示"),
    }
}
