// GNSS frequency / wavelength knowledge (L-band GPS-compatible subset).
//
// Frequency numbers are the band digit carried by the RINEX observation code
// (1, 2, 5...). Frequencies are explicit SI quantities so every derived number
// can be rendered with its unit.

pub const C_LIGHT: f64 = 299_792_458.0; // m/s

#[derive(Debug, Clone, Copy)]
pub struct Band {
    pub number: i64,
    pub frequency_hz: f64,
    pub name: &'static str,
}

pub fn band_for(obs_code: &str) -> Option<Band> {
    let digit = obs_code.chars().find(|c| c.is_ascii_digit())?;
    let number = (digit as u8 - b'0') as i64;
    Some(match number {
        1 => Band { number, frequency_hz: 1_575_420_000.0, name: "GPS L1" },
        2 => Band { number, frequency_hz: 1_227_600_000.0, name: "GPS L2" },
        5 => Band { number, frequency_hz: 1_176_450_000.0, name: "GPS L5" },
        _ => return None,
    })
}

pub fn wavelength_m(obs_code: &str) -> Option<f64> {
    Some(C_LIGHT / band_for(obs_code)?.frequency_hz)
}

/// Frequency number exactly as encoded in the observation code, kept for raw
/// traceability ("frequency number preserved per epoch").
pub fn frequency_number(obs_code: &str) -> Option<i64> {
    obs_code.chars().find(|c| c.is_ascii_digit()).map(|c| (c as u8 - b'0') as i64)
}
