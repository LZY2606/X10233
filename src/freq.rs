//! 观测码 -> 标称频率/波长。无法从观测码单独确定的频率（如 GLONASS 信道号缺失）
//! 返回 None，由上层保留原值并附诊断，绝不静默丢弃。

pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

/// 返回 (频率 Hz, 基准说明)。
pub fn nominal_frequency_hz(code: &str) -> Option<(f64, &'static str)> {
    let sys = code.chars().next()?;
    let band = code.chars().nth(1)?;
    let hz = match (sys, band) {
        ('G', '1') | ('E', '1') => 1_575.42e6,
        ('G', '2') => 1_227.60e6,
        ('G', '5') | ('E', '5') => 1_176.45e6,
        ('E', '7') => 1_207.140e6,
        ('E', '6') => 1_278.750e6,
        ('C', '1') => 1_561.098e6,
        ('C', '2') | ('C', '7') => 1_207.140e6,
        ('C', '5') => 1_176.45e6,
        ('R', _) => return None, // 需要频率信道号，案例不内置
        _ => return None,
    };
    Some((hz, "nominal_band_frequency"))
}

pub fn wavelength_m(code: &str) -> Option<f64> {
    nominal_frequency_hz(code).map(|(f, _)| SPEED_OF_LIGHT_M_S / f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_wavelengths() {
        let l1 = wavelength_m("L1C").unwrap();
        let l2 = wavelength_m("L2W").unwrap();
        assert!((l1 - 0.190293672798).abs() < 1e-7);
        assert!((l2 - 0.244210213425).abs() < 1e-7);
    }

    #[test]
    fn glonass_requires_channel() {
        assert!(nominal_frequency_hz("L1C".chars().next().map(|_| "R1C").unwrap_or("R1C")).is_none());
        assert!(wavelength_m("R1C").is_none());
    }
}
