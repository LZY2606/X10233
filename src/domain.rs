//! 领域核心类型、稳定身份与单位基准。
//!
//! 分层原则：
//! - 原始层（raw）：按历元原样保存，token 不可被覆盖；无法转换的值留在原位置并附诊断。
//! - 派生层（derived）：组合、弧段、候选等，携带算法版本与参数指纹、直接来源。
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ALGO_VERSION: &str = "gsb-detector-1.0.0";
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;
pub const DATUM: &str = "EPSG:4978"; // WGS-84 三维地心地固坐标（X,Y,Z，单位 m）

/// 12 位内容身份：同一输入在任意数据库中、导出再导入后保持一致。
pub fn hid(prefix: &str, material: &str) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b"|");
    h.update(material.as_bytes());
    let d = hex::encode(h.finalize());
    d[..12].to_string()
}

/// 原始数值：保留原始表示、单位与转换诊断；转换失败不丢弃。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawNum {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub ok: bool,
    pub diagnostic: Option<String>,
}

impl RawNum {
    pub fn parse(raw: &str, want_unit: &str) -> Self {
        let raw = raw.trim().to_string();
        // 形如 12.3m / 45.0Hz / 1.2：分离数值与单位后缀。
        let split_at = raw
            .find(|c: char| c.is_alphabetic() || c == '%')
            .unwrap_or(raw.len());
        let (num_part, unit_part) = raw.split_at(split_at);
        let num_part = num_part.trim();
        let unit_part = unit_part.trim();
        match num_part.parse::<f64>() {
            Ok(v) if v.is_finite() => {
                let unit = if unit_part.is_empty() {
                    Some(want_unit.to_string())
                } else {
                    Some(unit_part.to_string())
                };
                let (conv, diag) = if unit.as_deref() == Some(want_unit) {
                    (Some(v), None)
                } else {
                    match convert(v, unit.as_deref().unwrap_or(""), want_unit) {
                        Some(cv) => (Some(cv), Some(format!("unit {unit_part} -> {want_unit}"))),
                        None => (
                            None,
                            Some(format!("unconvertible unit {unit_part:?}, kept in place")),
                        ),
                    }
                };
                RawNum {
                    raw,
                    value: conv,
                    unit,
                    ok: diag.is_none(),
                    diagnostic: diag,
                }
            }
            _ => RawNum {
                raw,
                value: None,
                unit: None,
                ok: false,
                diagnostic: Some(format!("not a number, raw token retained")),
            },
        }
    }

    pub fn missing() -> Self {
        RawNum {
            raw: String::new(),
            value: None,
            unit: None,
            ok: false,
            diagnostic: Some("frequency absent at this epoch".to_string()),
        }
    }
}

fn convert(v: f64, from: &str, to: &str) -> Option<f64> {
    if from == to {
        return Some(v);
    }
    // 仅接受与周跳演示相关的明确换算，避免静默猜测。
    match (from, to) {
        ("km", "m") => Some(v * 1000.0),
        ("m", "km") => Some(v / 1000.0),
        ("kHz", "Hz") => Some(v * 1000.0),
        ("MHz", "Hz") => Some(v * 1e6),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SigKind {
    Code,
    Phase,
    Doppler,
    Snr,
}

impl SigKind {
    pub fn tag(self) -> &'static str {
        match self {
            SigKind::Code => "C",
            SigKind::Phase => "L",
            SigKind::Doppler => "D",
            SigKind::Snr => "S",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyDef {
    pub band: String, // L1 / L2
    pub codes: Vec<String>,
    pub freq_hz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationDef {
    pub name: String,
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
    pub datum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epoch {
    pub id: String,
    pub doc_id: String,
    pub idx: i64,
    pub t_raw: String,
    pub dt_s: f64, // 距上一历元的间隔（首历元 0），可能不等间隔
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub doc_id: String,
    pub epoch_idx: i64,
    pub sat: String,
    pub code: String,
    pub kind: String, // C/L/D/S
    pub num: RawNum,
    pub lli: Option<String>, // RINEX LLI 原样（如 "1"）
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub id: String,
    pub doc_id: String,
    pub epoch_idx: i64,
    pub event: String, // clock_jump
    pub offset_ns: RawNum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub id: String,
    pub doc_id: String,
    pub epoch_idx: i64,
    pub sat: String,
    pub x: RawNum,
    pub y: RawNum,
    pub z: RawNum,
    pub datum: String,
}

/// 历元上的频点视图；缺失频点对应 `ObsCell::Absent`。
#[derive(Debug, Clone, Default)]
pub struct CellSet {
    pub code: Option<Observation>,
    pub phase: Option<Observation>,
    pub doppler: Option<Observation>,
    pub snr: Option<Observation>,
}

/// 虚拟时间：所有时间戳由固定起点加确定性步进得到，测试不接触系统时钟。
#[derive(Debug, Clone)]
pub struct VirtualClock {
    base_iso: String,
    step_s: u64,
    n: u64,
}

impl VirtualClock {
    pub fn new() -> Self {
        VirtualClock {
            base_iso: "2026-09-20T00:00:00.000Z".to_string(),
            step_s: 60,
            n: 0,
        }
    }
    pub fn tick(&mut self) -> String {
        // base + n*step，简单 ISO 拼接（分钟进位）。
        let total_sec = self.n * self.step_s;
        self.n += 1;
        let h = total_sec / 3600;
        let m = (total_sec % 3600) / 60;
        let s = total_sec % 60;
        format!(
            "{}+{:02}:{:02}:{:02}",
            self.base_iso.trim_end_matches(".000Z"),
            h,
            m,
            s
        )
    }
}
