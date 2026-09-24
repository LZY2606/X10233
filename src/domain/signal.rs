//! 星座、频率编号与 RINEX-3 观测码。

use serde::{Deserialize, Serialize};

pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Constellation {
    Gps,
    Glonass,
    Galileo,
    Unknown,
}

impl Constellation {
    pub fn from_char(c: char) -> Constellation {
        match c {
            'G' => Constellation::Gps,
            'R' => Constellation::Glonass,
            'E' => Constellation::Galileo,
            _ => Constellation::Unknown,
        }
    }
    pub fn as_char(self) -> char {
        match self {
            Constellation::Gps => 'G',
            Constellation::Glonass => 'R',
            Constellation::Galileo => 'E',
            Constellation::Unknown => '?',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange,
    CarrierPhase,
    Doppler,
    Snr,
}

/// RINEX-3 观测码，例如 `C1C`(伪距 L1 C/A), `L2W`(L2 半无码 P(Y)),
/// 首字母给出量纲，中间字符给出频段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObsCode {
    pub raw: String,
    pub kind: ObsKind,
    pub band: u8,
    pub attr: char,
}

impl ObsCode {
    pub fn parse(token: &str) -> Option<ObsCode> {
        let t = token.trim();
        let mut chars = t.chars();
        let k = chars.next()?;
        let band_c = chars.next()?;
        let attr = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let kind = match k {
            'C' | 'P' => ObsKind::Pseudorange,
            'L' => ObsKind::CarrierPhase,
            'D' => ObsKind::Doppler,
            'S' => ObsKind::Snr,
            _ => return None,
        };
        if !band_c.is_ascii_digit() || band_c == '0' {
            return None;
        }
        Some(ObsCode { raw: t.to_string(), kind, band: band_c as u8 - b'0', attr })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalDef {
    pub band: u8,
    pub attr: char,
    pub frequency_hz: i64,
}

/// 频率（赫兹）。GLONASS 需要频率编号 k：
/// L1: 1_602_000_000 + k*562_500；L2: 1_246_000_000 + k*437_500。
pub fn freq_hz(c: Constellation, band: u8, glo_k: i32) -> Option<f64> {
    match (c, band) {
        (Constellation::Gps, 1) => Some(1_575_420_000.0),
        (Constellation::Gps, 2) => Some(1_227_600_000.0),
        (Constellation::Gps, 5) => Some(1_176_450_000.0),
        (Constellation::Galileo, 1) => Some(1_575_420_000.0),
        (Constellation::Galileo, 5) => Some(1_176_450_000.0),
        (Constellation::Galileo, 7) => Some(1_207_140_000.0),
        (Constellation::Glonass, 1) => Some(1_602_000_000.0 + (glo_k as f64) * 562_500.0),
        (Constellation::Glonass, 2) => Some(1_246_000_000.0 + (glo_k as f64) * 437_500.0),
        _ => None,
    }
}

pub fn wavelength_m(c: Constellation, band: u8, glo_k: i32) -> Option<f64> {
    freq_hz(c, band, glo_k).map(|f| SPEED_OF_LIGHT_M_S / f)
}

/// 头文件中声明的观测码顺序（用于解析固定宽度字段）。
pub fn frequency_table(codes: &[ObsCode], c: Constellation, glo_k: i32) -> Vec<SignalDef> {
    let mut out: Vec<SignalDef> = Vec::new();
    for code in codes {
        if let Some(f) = freq_hz(c, code.band, glo_k) {
            if !out.iter().any(|s| s.band == code.band && s.attr == code.attr) {
                out.push(SignalDef { band: code.band, attr: code.attr, frequency_hz: f as i64 });
            }
        }
    }
    out
}

pub fn signal_for_code(_codes: &[ObsCode], c: Constellation, band: u8, attr: char, glo_k: i32) -> Option<SignalDef> {
    let f = freq_hz(c, band, glo_k)?;
    Some(SignalDef { band, attr, frequency_hz: f as i64 })
}
