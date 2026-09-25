//! Melbourne-Wubbena 组合常量。
use crate::model::FreqSpec;

/// 宽巷波长 lambda_w = c / (f1 - f2)，单位 m。
pub fn wl_wavelength(f1: &FreqSpec, f2: &FreqSpec) -> f64 {
    299_792_458.0 / (f1.frequency_hz - f2.frequency_hz).abs()
}
