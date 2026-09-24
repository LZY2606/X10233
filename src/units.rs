//! 领域量：所有数值都携带单位/坐标基准；无法转换的值留在原位置并附诊断。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meter,
    Cycle,
    Hertz,
    DbHz,
    Second,
    MeterPerSecond,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Meter => "m",
            Unit::Cycle => "cycle",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dBHz",
            Unit::Second => "s",
            Unit::MeterPerSecond => "m/s",
        }
    }
}

/// 带原始表示与单位的量。`value` 为 None 表示无法转换（保留 raw + 诊断）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Qty {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: Unit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl Qty {
    pub fn parsed(raw: &str, unit: Unit) -> Qty {
        let trimmed = raw.trim();
        match trimmed.parse::<f64>() {
            Ok(v) if v.is_finite() => Qty { raw: trimmed.to_string(), value: Some(v), unit, diagnostic: None },
            _ => Qty {
                raw: trimmed.to_string(),
                value: None,
                unit,
                diagnostic: Some(format!("UNCONVERTIBLE: 无法把 {:?} 转换为 {}", trimmed, unit.as_str())),
            },
        }
    }

    pub fn finite(v: f64, unit: Unit) -> Qty {
        Qty { raw: format!("{}", v), value: Some(v), unit, diagnostic: None }
    }

    pub fn is_usable(&self) -> bool {
        self.value.map(|v| v.is_finite()).unwrap_or(false)
    }
}

/// 坐标基准（卫星状态/测站坐标）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Datum(pub String);

impl Default for Datum {
    fn default() -> Self {
        Datum("ITRF2014".to_string())
    }
}
