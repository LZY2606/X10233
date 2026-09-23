//! Frequency numbers, wavelengths and linear combinations.
//! Units and datums are explicit. Only frequencies needed by the lab case are
//! mapped; GLONASS FDMA channel numbers come from imported GLO NAV records.

#![allow(clippy::too_many_arguments)]

pub const C_M_S: f64 = 299_792_456.0;
pub const GPS_L1_HZ: f64 = 1.575_42e9;
pub const GPS_L2_HZ: f64 = 1.227_60e9;
pub const GLO_DFAC_HZ: f64 = 0.5625e6;
pub const GLO_L1_BASE_HZ: f64 = 1.602e9;
pub const GLO_L2_BASE_HZ: f64 = 1.246e9;
pub const RATIO: f64 = 9.0 / 7.0; // f1/f2 for GPS-like frequencies

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    L1,
    L2,
}

#[derive(Debug, Clone)]
pub struct SatFreq {
    pub sat: String,
    pub band: Band,
    pub freq_hz: f64,
    pub frequency_number: Option<i32>,
    pub source: String, // "standard-bureau:RINEX-convention" | "glo-nav:line4-field2"
}

pub fn band_for_code(code: &str) -> Option<Band> {
    let b = code.as_bytes().get(1)?;
    match b {
        b'1' | b'5' => Some(Band::L1),
        b'2' | b'6' | b'7' => Some(Band::L2),
        _ => None,
    }
}

pub fn band_name(b: Band) -> &'static str {
    match b { Band::L1 => "L1", Band::L2 => "L2" }
}

/// Frequency for a satellite/band. GLONASS needs the FDMA frequency number from
/// GLO NAV; if it is unavailable the result is an unknown datum (None), never a
/// silently guessed value.
pub fn frequency(sat: &str, band: Band, glo_k: Option<i32>) -> Option<SatFreq> {
    let c = sat.chars().next().unwrap_or('G');
    match (c, band) {
        ('G', Band::L1) | ('E', Band::L1) => Some(SatFreq { sat: sat.into(), band, freq_hz: GPS_L1_HZ, frequency_number: None, source: "standard:RINEX/GPS-L1".into() }),
        ('G', Band::L2) | ('E', Band::L2) => Some(SatFreq { sat: sat.into(), band, freq_hz: GPS_L2_HZ, frequency_number: None, source: "standard:RINEX/GPS-L2".into() }),
        ('R', _) => {
            let k = glo_k?;
            let base = if matches!(band, Band::L1) { GLO_L1_BASE_HZ } else { GLO_L2_BASE_HZ };
            Some(SatFreq { sat: sat.into(), band, freq_hz: base + (k as f64) * GLO_DFAC_HZ,
                frequency_number: Some(k), source: "glo-nav:line4-field2".into() })
        }
        _ => None,
    }
}

pub fn wavelength_m(sat: &str, band: Band, glo_k: Option<i32>) -> Option<f64> {
    frequency(sat, band, glo_k).map(|f| C_M_S / f.freq_hz)
}

/// Geometry-free / ionosphere phase combination in metres:
/// Lgf = L1_lambda*phi1 - L2_lambda*phi2.
pub fn gf_m(l1_cyc: f64, l2_cyc: f64, lam1: f64, lam2: f64) -> f64 {
    l1_cyc * lam1 - l2_cyc * lam2
}

/// Melbourne-Wubbena wide-lane in *wide-lane cycles*:
/// N_wl = phi1 - phi2 - (f1*P1 + f2*P2)/(c*(f1-f2))  (equivalent form).
pub fn mw_wl_cycle(l1: f64, l2: f64, p1: f64, p2: f64, lam1: f64, lam2: f64) -> f64 {
    let wl = RATIO;
    let wl_lam = wl / (wl - 1.0) * lam2; // c/(f1-f2)
    let phase_wl = l1 - l2;
    let code_nl_m = (wl * p1 + p2) / (wl + 1.0);
    phase_wl - code_nl_m / wl_lam
}

/// Change of the MW wide-lane in cycles for integer slips (n1, n2).
pub fn mw_wl_from_slips(n1: i64, n2: i64) -> f64 {
    n1 as f64 - n2 as f64
}

/// Change of geometry-free combination in metres for integer slips.
pub fn gf_from_slips(n1: i64, n2: i64, lam1: f64, lam2: f64) -> f64 {
    n1 as f64 * lam1 - n2 as f64 * lam2
}

/// Predicted phase change in metres between epochs using Doppler (Hz==cycle/s):
/// dL_m = -0.5*(D_a+D_b)*dt*lambda. Unequal epoch spacing enters through dt.
pub fn doppler_predicted_m(d_prev: f64, d_now: f64, dt_s: f64, lam: f64) -> f64 {
    -0.5 * (d_prev + d_now) * dt_s * lam
}
