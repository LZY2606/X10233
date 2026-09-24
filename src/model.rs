//! 核心数据模型:稳定身份、单位、原始表示保留。
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Id = String;

/// GPS 纪元起点 1980-01-06T00:00:00Z,所有历元用相对它的毫秒整数表示(虚拟时间友好)。
pub const GPS_EPOCH_UNIX_MS: i64 = 315_964_800_000;

pub fn sha256_hex(data: &str) -> String {
    let mut h = Sha256::new();
    h.update(data.as_bytes());
    hex(&h.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 内容寻址身份:同一 (kind, 内容) 永远得到同一 Id。
/// 导出、删库、再导入后身份保持不变,这是稳定身份的根基。
pub fn content_id(kind: &str, canonical: &str) -> Id {
    let h = sha256_hex(canonical);
    format!("{}_{}", kind, &h[..24])
}

/// 领域量单位。所有派生量都必须带单位;原始层保留输入原样。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Cycles,
    Meters,
    Hertz,
    DbHz,
    Milliseconds,
    Dimensionless,
}

impl Unit {
    pub fn label(&self) -> &'static str {
        match self {
            Unit::Cycles => "cyc",
            Unit::Meters => "m",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Milliseconds => "ms",
            Unit::Dimensionless => "1",
        }
    }
}

/// 观测码 -> 频率编号与载波频率(Hz)。坐标基准:载波相位以周为单位,频率基准 GPS ICD。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreqInfo {
    pub slot: u8,
    pub carrier_hz: f64,
    pub band: &'static str,
}

pub fn freq_info(sys: char, code: &str) -> Option<FreqInfo> {
    let bytes = code.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    let band_ch = bytes[1] as char;
    let (slot, carrier_hz, band) = match (sys, band_ch) {
        ('G', '1') => (1, 1_575_420_000.0, "L1"),
        ('G', '2') => (2, 1_227_600_000.0, "L2"),
        ('G', '5') => (5, 1_176_450_000.0, "L5"),
        ('C', '1') => (1, 1_561_098_000.0, "B1"),
        ('C', '2') => (2, 1_207_140_000.0, "B2"),
        ('E', '1') => (1, 1_575_420_000.0, "E1"),
        ('E', '5') => (5, 1_176_450_000.0, "E5a"),
        ('R', '1') => (1, 1_602_000_000.0, "G1"),
        ('R', '2') => (2, 1_246_000_000.0, "G2"),
        _ => return None,
    };
    Some(FreqInfo { slot, carrier_hz, band })
}

pub const C_M_PER_S: f64 = 299_792_458.0;

/// 观测码类别:伪距 C / 载波 L / Doppler D / 信噪比 S。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsClass {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
}

pub fn obs_class(code: &str) -> Option<ObsClass> {
    match code.as_bytes().first()? {
        b'C' => Some(ObsClass::Pseudorange),
        b'L' => Some(ObsClass::Carrier),
        b'D' => Some(ObsClass::Doppler),
        b'S' => Some(ObsClass::Snr),
        _ => None,
    }
}

/// 诊断:无法转换的值留在原位置并附诊断,不为画图丢弃。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line: usize,
    pub severity: String, // "error" | "warning"
    pub message: String,
}

/// 解析后的单条观测(派生第 0 层:规范化)。raw 字段永远保留原始表示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormObs {
    pub station: String,
    pub epoch_ms: i64,
    pub sat: String,
    pub code: String,
    pub freq_slot: Option<u8>,
    pub raw: String,          // 原始 16 字符字段,原样保存
    pub value: Option<f64>,   // 无法转换时为 None,诊断挂在 diag
    pub lli: Option<u8>,
    pub ssi: Option<u8>,
    pub diag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockEvent {
    pub station: String,
    pub epoch_ms: i64,
    pub jump_ms: f64,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatState {
    pub sat: String,
    pub epoch_ms: i64,
    pub state: String,
    pub raw: String,
}

/// ISO8601 格式化(虚拟时间可复现,不依赖系统时区)。
pub fn iso_of_ms(epoch_ms: i64) -> String {
    let unix_ms = GPS_EPOCH_UNIX_MS + epoch_ms;
    let secs = unix_ms.div_euclid(1000);
    let ms = unix_ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        tod / 3600,
        (tod / 60) % 60,
        tod % 60,
        ms
    )
}

pub fn ms_of_iso(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64, ms: i64) -> i64 {
    let days = days_from_civil(y, mo as u32, d as u32);
    (days * 86_400 + h * 3600 + mi * 60 + s) * 1000 + ms - GPS_EPOCH_UNIX_MS
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_roundtrip() {
        let ms = ms_of_iso(2024, 3, 1, 12, 30, 15, 250);
        assert_eq!(iso_of_ms(ms), "2024-03-01T12:30:15.250Z");
        assert_eq!(iso_of_ms(0), "1980-01-06T00:00:00.000Z");
    }

    #[test]
    fn content_id_stable() {
        assert_eq!(content_id("arc", "a|b"), content_id("arc", "a|b"));
        assert_ne!(content_id("arc", "a|b"), content_id("arc", "a|c"));
    }
}
