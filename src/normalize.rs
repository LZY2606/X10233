//! Derived-layer normalization. Raw values stay verbatim in `obs_value`;
//! normalization only fills the derived columns. Values that cannot be
//! converted stay in place with a diagnostic instead of being dropped.

/// Canonical unit for an observation code family.
pub fn canonical_unit(code: &str) -> Option<&'static str> {
    match code.chars().next()? {
        'C' => Some("m"),       // pseudorange
        'L' => Some("cycles"),  // carrier phase
        'D' => Some("Hz"),      // Doppler
        'S' => Some("dB-Hz"),   // signal-to-noise ratio
        _ => None,
    }
}

/// Frequency number carried by an observation code (C1C/L2W/... -> 1/2).
pub fn freq_num_of_code(code: &str) -> Option<i32> {
    code.chars().nth(1)?.to_digit(10).map(|d| d as i32)
}

/// Convert a numeric value between units. Returns None when the conversion is
/// not defined; the caller must keep the raw value and attach a diagnostic.
pub fn convert(value: f64, from: &str, to: &str) -> Option<f64> {
    if from == to {
        return Some(value);
    }
    match (from, to) {
        ("km", "m") => Some(value * 1000.0),
        ("mm", "m") => Some(value / 1000.0),
        ("kHz", "Hz") => Some(value * 1000.0),
        _ => None,
    }
}

pub struct Normalized {
    pub norm_value: Option<f64>,
    pub norm_unit: Option<String>,
    pub status: &'static str,
    pub diagnostic: Option<String>,
}

/// Normalize one raw observation. `unit_raw` is the unit as imported (RINEX
/// implies canonical units; JSON imports may state others).
pub fn normalize_obs(code: &str, value_raw: &str, unit_raw: &str) -> Normalized {
    let Some(canon) = canonical_unit(code) else {
        return Normalized {
            norm_value: None,
            norm_unit: None,
            status: "unconvertible",
            diagnostic: Some(format!("unknown observation code family '{}'", code)),
        };
    };
    let Ok(v) = value_raw.trim().parse::<f64>() else {
        return Normalized {
            norm_value: None,
            norm_unit: None,
            status: "unconvertible",
            diagnostic: Some(format!("value '{}' is not numeric", value_raw)),
        };
    };
    match convert(v, unit_raw, canon) {
        Some(x) => Normalized {
            norm_value: Some(x),
            norm_unit: Some(canon.to_string()),
            status: "ok",
            diagnostic: None,
        },
        None => Normalized {
            norm_value: None,
            norm_unit: None,
            status: "unconvertible",
            diagnostic: Some(format!(
                "no conversion from unit '{}' to '{}' for code {}; raw value kept in place",
                unit_raw, canon, code
            )),
        },
    }
}
