//! 所有领域量都显式携带单位/坐标基准。原始层只保存“原始表示 + 声明单位”，
//! 无法转换的值留在原位置并附诊断，派生层才做规范化。

use serde::{Deserialize, Serialize};

/// 带单位的数值。`raw` 是导入时的原始表示，`value_si` 仅在可转换时存在。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quant {
    /// 导入时的原始文本表示（RINEX 字段或 JSON 值原样）。
    pub raw: String,
    /// 原始声明单位，例如 "m" / "cycle" / "Hz" / "s" / "TECU"。
    pub unit: String,
    /// 解析后的 SI 规范化值；无法转换时为 None，诊断另行保留。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_si: Option<f64>,
}

impl Quant {
    pub fn new(raw: impl Into<String>, unit: impl Into<String>) -> Self {
        let raw = raw.into();
        let unit = unit.into();
        let value_si = raw.trim().parse::<f64>().ok().and_then(|v| to_si(&unit, v));
        Quant { raw, unit, value_si }
    }

    pub fn from_value(v: f64, unit: &str) -> Self {
        Quant {
            raw: format!("{v}"),
            unit: unit.to_string(),
            value_si: to_si(unit, v),
        }
    }

    pub fn convertible(&self) -> bool {
        self.value_si.is_some()
    }
}

fn to_si(unit: &str, v: f64) -> Option<f64> {
    if !v.is_finite() {
        return None;
    }
    Some(match unit {
        "m" | "s" | "Hz" | "cycle" | "rad" | "1" | "TECU" | "dBHz" | "ms" => match unit {
            "ms" => v / 1000.0,
            _ => v,
        },
        "km" => v * 1000.0,
        "us" => v * 1e-6,
        "ns" => v * 1e-9,
        _ => return None,
    })
}

/// 坐标基准。卫星状态、站坐标等必须说明所在基准；无法判定时保留原文。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Datum {
    pub frame: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Datum {
    pub fn unknown(raw: impl Into<String>) -> Self {
        Datum { frame: "UNKNOWN".into(), note: Some(raw.into()) }
    }
    pub fn itrf() -> Self {
        Datum { frame: "ITRF/本地约定".into(), note: Some("样例本地约定，无外部基准".into()) }
    }
}
