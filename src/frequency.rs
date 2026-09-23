use crate::error::{AppError, AppResult};
use crate::types::FrequencyMeta;

/// GPS nominal frequencies.
pub const GPS_L1_FREQ_HZ: f64 = 1.57542e9;
pub const GPS_L2_FREQ_HZ: f64 = 1.22760e9;
pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

pub fn wavelength(freq_hz: f64) -> f64 {
    SPEED_OF_LIGHT_M_S / freq_hz
}

pub fn default_frequencies() -> Vec<FrequencyMeta> {
    vec![
        FrequencyMeta {
            system: "G".into(),
            obs_code: "L1".into(),
            frequency_no: 1,
            frequency_hz: GPS_L1_FREQ_HZ,
            wavelength_m: wavelength(GPS_L1_FREQ_HZ),
        },
        FrequencyMeta {
            system: "G".into(),
            obs_code: "L2".into(),
            frequency_no: 2,
            frequency_hz: GPS_L2_FREQ_HZ,
            wavelength_m: wavelength(GPS_L2_FREQ_HZ),
        },
    ]
}

/// Validate declared frequencies and return lookup errors early.
pub fn validate_frequencies(freqs: &[FrequencyMeta]) -> AppResult<()> {
    if freqs.is_empty() {
        return Err(AppError::format("frequencies must not be empty"));
    }
    for f in freqs {
        if f.frequency_hz <= 0.0 {
            return Err(AppError::format(format!(
                "frequency {} has non-positive frequency_hz",
                f.obs_code
            )));
        }
        if (f.wavelength_m - wavelength(f.frequency_hz)).abs() > 1e-9 {
            return Err(AppError::format(format!(
                "frequency {} wavelength_m is inconsistent with frequency_hz",
                f.obs_code
            )));
        }
        if f.frequency_no < 1 {
            return Err(AppError::format(format!(
                "frequency {} has invalid frequency_no",
                f.obs_code
            )));
        }
    }
    Ok(())
}
