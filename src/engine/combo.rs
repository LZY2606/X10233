//! Multi-frequency combination mathematics.
//!
//! Sign convention (shared with the offline case generator): the recorded
//! accumulated carrier phase follows
//!
//! ```text
//! L = rho / lambda - I / lambda - N
//! ```
//!
//! A cycle slip adds integer cycles to the recorded phase, so a slip
//! `(n1, n2)` produces a positive jump in the formulas below.

use crate::engine::SignalMeta;

/// Ionosphere (geometry-free) carrier combination, metres:
/// `GF = lambda1*L1 - lambda2*L2`. Common geometry and receiver-clock terms
/// cancel; a `(n1,n2)` slip jumps by `lambda1*n1 - lambda2*n2`.
pub fn gf_m(lambda1: f64, l1: f64, lambda2: f64, l2: f64) -> f64 {
    lambda1 * l1 - lambda2 * l2
}

pub fn gf_jump_m(lambda1: f64, lambda2: f64, n1: i64, n2: Option<i64>) -> f64 {
    lambda1 * n1 as f64 - lambda2 * n2.unwrap_or(0) as f64
}

/// Wide-lane wavelength from the two carrier frequencies.
pub fn wl_wavelength_m(sig1: &SignalMeta, sig2: &SignalMeta) -> f64 {
    let f1 = sig1.frequency_hz;
    let f2 = sig2.frequency_hz;
    crate::consts::C_M_S * (f1 - f2).abs() / (f1 * f2)
}

/// Wide-lane phase combination (WL cycles):
/// `LW = (f1*L1 - f2*L2)/(f1-f2)`. A slip contributes `n1 - n2` cycles.
pub fn lw_wl_cycle(sig1: &SignalMeta, l1: f64, sig2: &SignalMeta, l2: f64) -> f64 {
    let f1 = sig1.frequency_hz;
    let f2 = sig2.frequency_hz;
    (f1 * l1 - f2 * l2) / (f1 - f2)
}

/// Narrow-lane code combination, metres:
/// `PN = (f1*P1 + f2*P2)/(f1+f2)`.
pub fn pn_nl_m(sig1: &SignalMeta, p1: f64, sig2: &SignalMeta, p2: f64) -> f64 {
    let f1 = sig1.frequency_hz;
    let f2 = sig2.frequency_hz;
    (f1 * p1 + f2 * p2) / (f1 + f2)
}

/// Melbourne-Wubbena combination in wide-lane cycles:
/// `MW = (LW - PN/lambda_w)`. A slip jumps by the WL integer `n1 - n2`.
pub fn mw_wl_cycle(
    sig1: &SignalMeta,
    l1: f64,
    p1: f64,
    sig2: &SignalMeta,
    l2: f64,
    p2: f64,
) -> f64 {
    let lw = lw_wl_cycle(sig1, l1, sig2, l2);
    let pn = pn_nl_m(sig1, p1, sig2, p2);
    let lam_w = wl_wavelength_m(sig1, sig2);
    lw - pn / lam_w
}

pub fn mw_jump_cycle(n1: i64, n2: Option<i64>) -> f64 {
    n1 as f64 - n2.unwrap_or(0) as f64
}

/// Time-difference (Doppler aided) expected accumulated-phase change in
/// cycles. Doppler is recorded as `Hz = -dL/dt`, so the expected increment of
/// the accumulated phase over `dt` seconds is `-mean(Doppler)*dt`.
pub fn td_predicted_cycles(doppler_prev_hz: f64, doppler_hz: f64, dt_s: f64) -> f64 {
    -0.5 * (doppler_prev_hz + doppler_hz) * dt_s
}

/// Expected phase change from the code rate, in cycles (fallback detector).
pub fn code_predicted_cycles(p_now_m: f64, p_prev_m: f64, wavelength_m: f64) -> f64 {
    (p_now_m - p_prev_m) / wavelength_m
}
