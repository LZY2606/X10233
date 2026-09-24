//! Multi-detector cycle-slip physics.
//!
//! Three independent detectors each propose integer hypotheses; by design
//! none is treated as truth:
//!
//! * **GF** geometry-free ionospheric combination `L1*l1 - L2*l2`
//! * **MW** Melbourne-Wubbena wide-lane (phase WL vs narrow-lane code)
//! * **TD** time difference of carrier phase, checked against Doppler linear
//!   prediction; intentionally weak over irregular gaps with acceleration
//!
//! Receiver clock jumps are measured on the common code component and are
//! reported separately. GF is clock-immune; MW and TD subtract the estimated
//! common clock before slip testing.
use serde::Serialize;

pub const C_LIGHT: f64 = 299_792_458.0;

#[derive(Debug, Clone, Copy)]
pub struct Bands {
    pub l1: f64,
    pub l2: f64,
}

impl Bands {
    pub fn f1(&self) -> f64 {
        C_LIGHT / self.l1
    }
    pub fn f2(&self) -> f64 {
        C_LIGHT / self.l2
    }
    pub fn wl_lambda(&self) -> f64 {
        C_LIGHT / (self.f1() - self.f2())
    }
}

#[derive(Debug, Clone, Default, Copy)]
pub struct BandSample {
    pub code_m: Option<f64>,
    pub phase_cyc: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub lli: Option<i64>,
}

#[derive(Debug, Clone, Default, Copy)]
pub struct PairSample {
    pub a: BandSample,
    pub b: BandSample,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PairResiduals {
    pub gf_jump_m: Option<f64>,
    pub mw_jump_m: Option<f64>,
    pub td_cycles_l1: Option<f64>,
    pub td_cycles_l2: Option<f64>,
    pub code_common_jump_m: Option<f64>,
    pub clock_corrected_m: f64,
    pub irregular: bool,
    pub dt_s: f64,
    pub usable: bool,
    pub unusable_reasons: Vec<String>,
    pub gf_series_m: Option<f64>,
    pub mw_series_m: Option<f64>,
}

pub fn gf_series(l1_cyc: f64, l2_cyc: f64, b: &Bands) -> f64 {
    l1_cyc * b.l1 - l2_cyc * b.l2
}

/// Melbourne-Wubbena wide-lane observable: WL phase minus narrow-lane code.
pub fn mw_value(s: &PairSample, bands: &Bands) -> Option<f64> {
    let p1 = s.a.code_m?;
    let p2 = s.b.code_m?;
    let l1 = s.a.phase_cyc?;
    let l2 = s.b.phase_cyc?;
    let (f1, f2) = (bands.f1(), bands.f2());
    let phase_wl = bands.wl_lambda() * (l1 - l2);
    let code_nl = (f1 * p1 + f2 * p2) / (f1 + f2);
    Some(phase_wl - code_nl)
}

/// Residuals for one adjacent-epoch pair of the same satellite.
///
/// `clock_estimate_m` is the station-wide common clock jump at this boundary
/// (0.0 when none). MW narrow-lane code contains it, so it is subtracted.
pub fn residuals(
    prev: &PairSample,
    cur: &PairSample,
    dt_s: f64,
    irregular: bool,
    bands: &Bands,
    clock_estimate_m: f64,
) -> PairResiduals {
    let mut r = PairResiduals {
        dt_s,
        irregular,
        clock_corrected_m: clock_estimate_m,
        ..Default::default()
    };
    let mut reasons: Vec<String> = vec![];

    let gf_cur = pair_phases(cur).map(|(l1, l2)| gf_series(l1, l2, bands));
    let gf_prev = pair_phases(prev).map(|(l1, l2)| gf_series(l1, l2, bands));
    r.gf_series_m = gf_cur;
    r.gf_jump_m = match (gf_cur, gf_prev) {
        (Some(c), Some(p)) => Some(c - p),
        _ => {
            reasons.push("missing_frequency".into());
            None
        }
    };

    let mw_cur = mw_value(cur, bands);
    let mw_prev = mw_value(prev, bands);
    r.mw_series_m = mw_cur;
    r.mw_jump_m = match (mw_cur, mw_prev) {
        (Some(c), Some(p)) => Some(c - p + clock_estimate_m),
        _ => {
            reasons.push("mw_unavailable".into());
            None
        }
    };

    // A receiver clock jump adds clock_m/lambda cycles to the phase increment;
    // subtract it. GF is immune and MW cancels code+phase internally, so the
    // clock correction only enters the Doppler time difference.
    r.td_cycles_l1 = td_cycles(prev.a, cur.a, dt_s).map(|v| v - clock_estimate_m / bands.l1);
    r.td_cycles_l2 = td_cycles(prev.b, cur.b, dt_s).map(|v| v - clock_estimate_m / bands.l2);
    if r.td_cycles_l1.is_none() {
        reasons.push("td_l1_unavailable".into());
    }
    if r.td_cycles_l2.is_none() {
        reasons.push("td_l2_unavailable".into());
    }

    let code_now = code_common(cur);
    let code_pre = code_common(prev);
    r.code_common_jump_m = match (code_now, code_pre) {
        (Some(n), Some(p)) => Some(n - p),
        _ => None,
    };

    r.usable = r.gf_jump_m.is_some()
        || r.mw_jump_m.is_some()
        || r.td_cycles_l1.is_some()
        || r.td_cycles_l2.is_some();
    r.unusable_reasons = reasons;
    r
}

fn pair_phases(s: &PairSample) -> Option<(f64, f64)> {
    Some((s.a.phase_cyc?, s.b.phase_cyc?))
}

fn code_common(s: &PairSample) -> Option<f64> {
    Some((s.a.code_m? + s.b.code_m?) / 2.0)
}

/// Carrier cycles accumulated between epochs, predicted from mean Doppler.
fn td_cycles(prev: BandSample, cur: BandSample, dt: f64) -> Option<f64> {
    let before = prev.phase_cyc?;
    let after = cur.phase_cyc?;
    let d1 = prev.doppler_hz?;
    let d2 = cur.doppler_hz?;
    if dt <= 0.0 {
        return None;
    }
    // Doppler and accumulated carrier cycles share the same sign: a positive
    // Doppler (range decreasing) grows the accumulated phase.
    let predicted = before + (d1 + d2) * 0.5 * dt;
    Some(after - predicted)
}

#[derive(Debug, Clone, Serialize)]
pub struct Support {
    pub residual: f64,
    pub within: bool,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hypothesis {
    pub n1: i64,
    pub n2: i64,
    pub score: f64,
    pub gf: Option<Support>,
    pub mw: Option<Support>,
    pub td1: Option<Support>,
    pub td2: Option<Support>,
}

impl Hypothesis {
    /// Legal integer pair: accepted by BOTH code/phase combinations.
    /// MW fixes the wide-lane difference n1-n2; GF then keeps the equally
    /// spaced (n1+k,n2+k) offsets. Neither detector alone is truth: TD is
    /// kept separately and can disagree.
    pub fn is_legal(&self) -> bool {
        self.gf.as_ref().is_some_and(|s| s.within) && self.mw.as_ref().is_some_and(|s| s.within)
    }

    pub fn td_passes(&self) -> bool {
        self.td1.as_ref().is_some_and(|s| s.within) && self.td2.as_ref().is_some_and(|s| s.within)
    }

    /// Independent Doppler pick: nearest integer on each band.
    pub fn td_pick(r: &PairResiduals) -> Option<(i64, i64)> {
        let t1 = r.td_cycles_l1?;
        let t2 = r.td_cycles_l2?;
        Some((t1.round() as i64, t2.round() as i64))
    }
}

/// Enumerate integer pairs (n1,n2), retaining only those supported by at
/// least one code/phase combination. Ordered by normalized residual score.
pub fn evaluate_hypotheses(
    r: &PairResiduals,
    bands: &Bands,
    gf_sigma: f64,
    mw_sigma: f64,
    td_sigma: f64,
    max_n: i64,
) -> Vec<Hypothesis> {
    let mut out = vec![];
    for n1 in -max_n..=max_n {
        for n2 in -max_n..=max_n {
            let gf = r.gf_jump_m.map(|jump| {
                let pred = n1 as f64 * bands.l1 - n2 as f64 * bands.l2;
                mk((jump - pred) / gf_sigma, "meter")
            });
            let mw = r.mw_jump_m.map(|jump| {
                let pred = n1 as f64 * bands.l1 - n2 as f64 * bands.l2;
                mk((jump - pred) / mw_sigma, "meter")
            });
            let td1 = r.td_cycles_l1.map(|j| mk((j - n1 as f64) / td_sigma, "cycle"));
            let td2 = r.td_cycles_l2.map(|j| mk((j - n2 as f64) / td_sigma, "cycle"));
            let mut h = Hypothesis { n1, n2, score: 0.0, gf, mw, td1, td2 };
            if !(h.is_legal()
                || (h.td1.as_ref().is_some_and(|s| s.within)
                    && h.td2.as_ref().is_some_and(|s| s.within)))
            {
                continue;
            }
            // TD-only picks are retained for comparison but never enter the
            // legal integer set by themselves.
            let td_only = !(h.gf.as_ref().is_some_and(|x| x.within)
                && h.mw.as_ref().is_some_and(|x| x.within));
            if td_only {
                continue;
            }
            let mut score = 0.0f64;
            for s in [&h.gf, &h.mw, &h.td1, &h.td2].into_iter().flatten() {
                score += s.residual.powi(2);
            }
            h.score = score;
            out.push(h);
        }
    }
    out.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.n1.cmp(&b.n1))
            .then(a.n2.cmp(&b.n2))
    });
    out
}

/// Store normalized residual (sigma units) but keep `within` against |z|<=1.
fn mk(z: f64, unit: &str) -> Support {
    Support { residual: z, within: z.abs() <= 1.0, unit: unit.into() }
}
