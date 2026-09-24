//! Multi-frequency combination math.
//!
//! Sign conventions (applied consistently by the synthetic generator and the
//! detector so a jump of `n` cycles is recovered as `n`):
//! * phase observation L (cycles) grows with propagation time;
//! * Doppler D (Hz) ~ -dL/dt;
//! * pseudorange C (m).

use crate::ingest::gps_frequency_hz;

pub const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

pub fn wavelength_m(band: u8) -> Option<f64> {
    gps_frequency_hz(band).map(|f| SPEED_OF_LIGHT_M_S / f)
}

pub struct Bands {
    pub b1: u8,
    pub b2: u8,
}

impl Bands {
    pub fn f1(&self) -> f64 {
        gps_frequency_hz(self.b1).unwrap()
    }
    pub fn f2(&self) -> f64 {
        gps_frequency_hz(self.b2).unwrap()
    }
    pub fn l1_m(&self) -> f64 {
        wavelength_m(self.b1).unwrap()
    }
    pub fn l2_m(&self) -> f64 {
        wavelength_m(self.b2).unwrap()
    }
    /// Widelane frequency f_w = f1 - f2.
    pub fn fw(&self) -> f64 {
        self.f1() - self.f2()
    }
    /// Widelane wavelength.
    pub fn wl_m(&self) -> f64 {
        SPEED_OF_LIGHT_M_S / self.fw()
    }
    /// Narrowlane code frequency f1 + f2 (pseudorange combination).
    pub fn fn_code(&self) -> f64 {
        self.f1() + self.f2()
    }
    /// Narrowlane pseudorange (m): (f1*P1 + f2*P2)/(f1+f2).
    pub fn narrowlane_code_m(&self, p1_m: f64, p2_m: f64) -> f64 {
        (self.f1() * p1_m + self.f2() * p2_m) / self.fn_code()
    }
    /// Widelane phase (m): lambda_w * (L1 - L2).
    pub fn widelane_phase_m(&self, l1_cyc: f64, l2_cyc: f64) -> f64 {
        self.wl_m() * (l1_cyc - l2_cyc)
    }
    /// Melbourne-Wubbena combination (widelane cycles).
    pub fn mw_cycles(&self, l1_cyc: f64, l2_cyc: f64, p1_m: f64, p2_m: f64) -> f64 {
        (self.widelane_phase_m(l1_cyc, l2_cyc)
            - self.narrowlane_code_m(p1_m, p2_m))
            / self.wl_m()
    }
    /// Geometry-free ionosphere combination L_I = L1*lam1 - L2*lam2 (metres).
    pub fn iono_phase_m(&self, l1_cyc: f64, l2_cyc: f64) -> f64 {
        l1_cyc * self.l1_m() - l2_cyc * self.l2_m()
    }
    /// Convert a GF-ionosphere jump (metres) to an equivalent L1/L2 integer
    /// shift ambiguity; returns residual cycles against the nearest integer
    /// pair consistent with a pure slip.
    pub fn gf_jump_to_cycles(&self, jump_m: f64) -> (f64, f64) {
        (jump_m / self.l1_m(), jump_m / self.l2_m())
    }
}

/// Round to nearest integer with half-to-even avoidance (floor(x+0.5));
/// ties are handled by the caller via the tie band.
pub fn nearest_int(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

pub fn frac(x: f64) -> f64 {
    x - x.floor()
}
