//! Stateless detector math.
//!
//! No detector is treated as ground truth: time-difference, geometry-free
//! (ionosphere) and Melbourne-Wubbena each emit their own integer candidates
//! with residuals and scores. Common-mode receiver clock jumps are detected
//! separately from single-satellite cycle slips.

use crate::domain::*;
use serde::{Deserialize, Serialize};

pub const SPEED_OF_LIGHT: f64 = 299_792_458.0;
pub const F_L1: f64 = 1_575_420_000.0;
pub const F_L2: f64 = 1_227_600_000.0;
pub const F_L5: f64 = 1_176_450_000.0;

pub fn wavelength(band: u8) -> Option<f64> {
    let f = match band {
        1 => F_L1,
        2 => F_L2,
        5 => F_L5,
        _ => return None,
    };
    Some(SPEED_OF_LIGHT / f)
}

/// One normalized point of one band at one epoch (derived layer only).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Point {
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub seconds: f64,
    pub code_m: Option<f64>,
    pub phase_cycles: Option<f64>,
    pub doppler_hz: Option<f64>,
    pub lli: Option<u8>,
    pub snr_dbhz: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandSeries {
    pub sat: String,
    pub band: u8,
    /// indexed like [`SeriesSet::epochs`]; None = frequency absent at epoch
    pub points: Vec<Option<Point>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesSet {
    pub station: String,
    /// all epochs in order, with uneven spacing allowed
    pub epochs: Vec<EpochTime>,
    pub bands: Vec<BandSeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochTime {
    pub epoch_no: u32,
    pub gpst_iso: String,
    pub seconds: f64,
    pub recv_clk_s: Option<f64>,
}

/// Integer candidate emitted by one detector for one boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntCandidate {
    /// offset in cycles, indexed by the two bands used by the detector
    pub bands: Vec<u8>,
    pub offset_cycles: Vec<i64>,
    pub residual: f64,
    pub unit: Unit,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorOutput {
    pub detector: Detector,
    pub fired: bool,
    pub unknown: bool,
    pub residual: f64,
    pub unit: Unit,
    pub reason: String,
    pub candidates: Vec<IntCandidate>,
}

// ---------------------------------------------------------------------------
// time conversion
// ---------------------------------------------------------------------------

pub fn iso_to_seconds(s: &str) -> f64 {
    let core = s.split_whitespace().next().unwrap_or(s);
    let (date, time) = core.split_once('T').unwrap_or((core, "00:00:00"));
    let d: Vec<i64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    let t: Vec<f64> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    let y = d.first().copied().unwrap_or(0);
    let m = d.get(1).copied().unwrap_or(1) as u32;
    let dd = d.get(2).copied().unwrap_or(1) as u32;
    let days = days_from_civil(y, m, dd) as f64;
    let secs = 3600.0 * t.first().copied().unwrap_or(0.0)
        + 60.0 * t.get(1).copied().unwrap_or(0.0)
        + t.get(2).copied().unwrap_or(0.0);
    days * 86_400.0 + secs
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------------------------------------------------------------------------
// normalization (raw -> series); only the derived layer normalizes
// ---------------------------------------------------------------------------

pub fn build_series(station: &str, epochs: &[RawEpoch]) -> SeriesSet {
    let times: Vec<EpochTime> = epochs
        .iter()
        .map(|e| EpochTime {
            epoch_no: e.epoch_no,
            gpst_iso: e.gpst_iso.clone(),
            seconds: iso_to_seconds(&e.gpst_iso),
            recv_clk_s: e.recv_clk_offset_raw.value,
        })
        .collect();

    let mut keys: Vec<(String, u8)> = Vec::new();
    for e in epochs {
        for m in &e.measurements {
            let k = (m.sat.clone(), m.freq_no);
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys.sort();

    let mut bands = Vec::new();
    for (sat, band) in keys {
        let mut points = vec![None; epochs.len()];
        for (ei, e) in epochs.iter().enumerate() {
            let mut p = Point {
                epoch_no: e.epoch_no,
                gpst_iso: e.gpst_iso.clone(),
                seconds: times[ei].seconds,
                ..Default::default()
            };
            let mut present = false;
            for m in e.measurements.iter().filter(|m| m.sat == sat && m.freq_no == band) {
                let v = |x: &Option<RawValue>| x.as_ref().and_then(|r| r.value);
                if m.obs_code.starts_with('C') {
                    p.code_m = p.code_m.or(v(&m.range));
                } else if m.obs_code.starts_with('L') {
                    p.phase_cycles = p.phase_cycles.or(v(&m.phase));
                } else if m.obs_code.starts_with('D') {
                    p.doppler_hz = p.doppler_hz.or(v(&m.doppler));
                }
                if m.lli.is_some() {
                    p.lli = m.lli;
                }
                if let Some(snr) = v(&m.snr) {
                    p.snr_dbhz = Some(snr);
                }
                present = true;
            }
            if present {
                points[ei] = Some(p);
            }
        }
        bands.push(BandSeries { sat, band, points });
    }

    SeriesSet { station: station.into(), epochs: times, bands }
}

fn phase(bs: &BandSeries, idx: usize) -> Option<f64> {
    bs.points[idx].as_ref().and_then(|p| p.phase_cycles)
}
fn code(bs: &BandSeries, idx: usize) -> Option<f64> {
    bs.points[idx].as_ref().and_then(|p| p.code_m)
}

// ---------------------------------------------------------------------------
// time-difference detector on a single band
// ---------------------------------------------------------------------------

pub fn time_difference(set: &SeriesSet, bs: &BandSeries, idx: usize, gate_cycles: f64) -> DetectorOutput {
    let det = Detector::TimeDifference;
    if idx == 0 {
        return unknown(det, Unit::Cycle, "no previous epoch");
    }
    let dt = set.epochs[idx].seconds - set.epochs[idx - 1].seconds;
    let (Some(p_now), Some(p_prev)) = (&bs.points[idx], &bs.points[idx - 1]) else {
        return unknown(det, Unit::Cycle, reasons::FREQUENCY_MISSING);
    };
    let (Some(l_now), Some(l_prev)) = (p_now.phase_cycles, p_prev.phase_cycles) else {
        return unknown(det, Unit::Cycle, "phase not convertible or absent");
    };
    let mut residual = l_now - l_prev;
    let mut reason = String::new();
    if dt > 0.0 {
        // remove the smooth Doppler-predicted trend on both sides of the gap
        let d_after = p_now.doppler_hz;
        let d_before = p_prev.doppler_hz;
        match (d_after, d_before) {
            (Some(a), Some(b)) => {
                let predicted = -0.5 * (a + b) * dt;
                residual -= predicted;
            }
            (Some(a), None) => {
                residual -= -a * dt;
            }
            _ => {
                reason = "doppler_unavailable:raw_delta";
            }
        }
    }
    if (dt - 1.0).abs() > 1e-6 {
        reason = if reason.is_empty() {
            reasons::UNEVEN_EPOCHS.to_string()
        } else {
            format!("{reason};{}", reasons::UNEVEN_EPOCHS)
        };
    }
    let fired = residual.abs() > gate_cycles;
    let candidates = if fired {
        nearest_offset(residual, 0, det, Unit::Cycle, 1.0)
    } else {
        vec![zero_candidate(&[bs.band], Unit::Cycle)]
    };
    DetectorOutput {
        detector: det,
        fired,
        unknown: false,
        residual,
        unit: Unit::Cycle,
        reason,
        candidates,
    }
}

// ---------------------------------------------------------------------------
// geometry-free ionosphere combination between two bands
// ---------------------------------------------------------------------------

pub fn band_pair<'a>(set: &'a SeriesSet, sat: &str, b1: u8, b2: u8) -> Option<(&'a BandSeries, &'a BandSeries)> {
    let a = set.bands.iter().find(|x| x.sat == sat && x.band == b1)?;
    let b = set.bands.iter().find(|x| x.sat == sat && x.band == b2)?;
    Some((a, b))
}

fn gf_value(set: &SeriesSet, a: &BandSeries, b: &BandSeries, idx: usize) -> Option<f64> {
    let l1 = phase(a, idx)?;
    let l2 = phase(b, idx)?;
    let w1 = wavelength(a.band)?;
    let w2 = wavelength(b.band)?;
    Some(l1 * w1 - l2 * w2)
}

pub fn ionosphere_gf(
    set: &SeriesSet,
    sat: &str,
    b1: u8,
    b2: u8,
    idx: usize,
    p: &Params,
) -> DetectorOutput {
    let det = Detector::IonosphereFree;
    let Some((a, b)) = band_pair(set, sat, b1, b2) else {
        return unknown(det, Unit::Meter, reasons::FREQUENCY_MISSING);
    };
    if idx == 0 {
        return unknown(det, Unit::Meter, "no previous epoch");
    }
    if a.points[idx].is_none() || b.points[idx].is_none() {
        return unknown(det, Unit::Meter, reasons::FREQUENCY_MISSING);
    }
    let dt = set.epochs[idx].seconds - set.epochs[idx - 1].seconds;
    let Some(now) = gf_value(set, a, b, idx) else {
        return unknown(det, Unit::Meter, "phase not convertible");
    };
    let Some(prev) = gf_value(set, a, b, idx - 1) else {
        return unknown(det, Unit::Meter, "previous geometry-free unavailable");
    };
    // linear iono drift extrapolation using the two epochs before the gap
    let mut predicted = prev;
    let mut gap_widened = false;
    if let (Some(t0), Some(t1)) = (
        idx.checked_sub(2).and_then(|k| gf_value(set, a, b, k)),
        Some(prev),
    ) {
        let dt1 = set.epochs[idx - 1].seconds - set.epochs[idx - 2].seconds;
        if dt1 > 0.0 {
            predicted = t1 + (t1 - t0) / dt1 * dt;
        }
    }
    let residual = now - predicted;
    let gate = if dt >= p.gap_seconds {
        gap_widened = true;
        p.gap_gf_tol_m
    } else {
        p.gf_gate_m
    };
    let w1 = wavelength(b1).unwrap_or(1.0);
    let w2 = wavelength(b2).unwrap_or(1.0);
    let fired = residual.abs() > gate;
    let mut reason = if gap_widened { reasons::GAP_IONO_DRIFT.to_string() } else { String::new() };
    let candidates = if fired {
        // integer pairs (n1,n2) satisfying n1*w1 - n2*w2 ~= residual
        let mut cands = integer_pairs_near(residual, b1, b2, w1, w2, gate, p.search_radius);
        if cands.len() > 1 {
            if !reason.is_empty() {
                reason.push(';');
            }
            reason.push_str(reasons::MULTIWAY_TIE);
        }
        cands
    } else {
        vec![zero_candidate(&[b1, b2], Unit::Meter)]
    };
    DetectorOutput { detector: det, fired, unknown: false, residual, unit: Unit::Meter, reason, candidates }
}

// ---------------------------------------------------------------------------
// Melbourne-Wubbena
// ---------------------------------------------------------------------------

fn mw_value(a: &BandSeries, b: &BandSeries, idx: usize) -> Option<f64> {
    // N_wl = (f1 L1 - f2 L2)/(f1-f2) - (f1 P1 + f2 P2)/(c*(f1-f2))  [cycles WL]
    let f1 = freq_of(a.band)?;
    let f2 = freq_of(b.band)?;
    let l1 = phase(a, idx)?;
    let l2 = phase(b, idx)?;
    let p1 = code(a, idx)?;
    let p2 = code(b, idx)?;
    let phase_wl = (f1 * l1 - f2 * l2) / (f1 - f2);
    let code_nl_m = (f1 * p1 + f2 * p2) / (f1 + f2);
    let w_wl = SPEED_OF_LIGHT / (f1 - f2);
    Some(phase_wl - code_nl_m / w_wl)
}

fn freq_of(band: u8) -> Option<f64> {
    match band {
        1 => Some(F_L1),
        2 => Some(F_L2),
        5 => Some(F_L5),
        _ => None,
    }
}

pub fn melbourne_wubbena(
    set: &SeriesSet,
    sat: &str,
    b1: u8,
    b2: u8,
    idx: usize,
    p: &Params,
) -> DetectorOutput {
    let det = Detector::MelbourneWubbena;
    let Some((a, b)) = band_pair(set, sat, b1, b2) else {
        return unknown(det, Unit::Cycle, reasons::FREQUENCY_MISSING);
    };
    let Some(now) = mw_value(a, b, idx) else {
        return unknown(det, Unit::Cycle, reasons::FREQUENCY_MISSING);
    };
    // mean of previous available MW values (simple window)
    let mut history = Vec::new();
    for k in (0..idx).rev() {
        if let Some(v) = mw_value(a, b, k) {
            history.push(v);
            if history.len() >= p.mw_window {
                break;
            }
        }
    }
    if history.is_empty() {
        return unknown(det, Unit::Cycle, "mw_window_empty");
    }
    let mean = history.iter().sum::<f64>() / history.len() as f64;
    let residual = now - mean;
    let fired = residual.abs() > p.mw_gate_wl_cycles;
    // N_wl change n1 - n2 is the directly observable integer.
    let candidates = if fired {
        let n_wl = residual.round() as i64;
        let mut out = Vec::new();
        for n1 in -p.search_radius..=p.search_radius {
            let n2 = n1 - n_wl;
            if n2.abs() <= p.search_radius {
                let res = residual - n_wl as f64;
                out.push(IntCandidate {
                    bands: vec![b1, b2],
                    offset_cycles: vec![n1, n2],
                    residual: res,
                    unit: Unit::Cycle,
                    note: Some(format!("wl_integer={n_wl}")),
                });
            }
        }
        out
    } else {
        vec![zero_candidate(&[b1, b2], Unit::Cycle)]
    };
    DetectorOutput {
        detector: det,
        fired,
        unknown: false,
        residual,
        unit: Unit::Cycle,
        reason: String::new(),
        candidates,
    }
}

// ---------------------------------------------------------------------------
// LLI flag: evidence only, never ground truth
// ---------------------------------------------------------------------------

pub fn lli_flag(bs: &BandSeries, idx: usize, td: &DetectorOutput) -> DetectorOutput {
    let det = Detector::Lli;
    let Some(point) = bs.points[idx].as_ref() else {
        return unknown(det, Unit::Dimensionless, reasons::FREQUENCY_MISSING);
    };
    match point.lli {
        Some(bits) if bits & 1 == 1 => {
            // If the independent phase detector did not fire, this is a
            // suspected false alarm; zero offset is a legal candidate.
            let false_alarm = !td.fired;
            let mut cands = if false_alarm {
                vec![zero_candidate(&[bs.band], Unit::Dimensionless)]
            } else {
                td.candidates
                    .iter()
                    .map(|c| IntCandidate {
                        bands: c.bands.clone(),
                        offset_cycles: c.offset_cycles.clone(),
                        residual: c.residual,
                        unit: Unit::Dimensionless,
                        note: c.note.clone(),
                    })
                    .collect()
            };
            if false_alarm {
                cands[0].note = Some(reasons::LLI_FALSE_ALARM.into());
            }
            DetectorOutput {
                detector: det,
                fired: true,
                unknown: false,
                residual: 1.0,
                unit: Unit::Dimensionless,
                reason: if false_alarm { reasons::LLI_FALSE_ALARM.into() } else { "lli_confirmed_by_phase".into() },
                candidates: cands,
            }
        }
        _ => DetectorOutput {
            detector: det,
            fired: false,
            unknown: false,
            residual: 0.0,
            unit: Unit::Dimensionless,
            reason: reasons::NO_BREAK.into(),
            candidates: vec![zero_candidate(&[bs.band], Unit::Dimensionless)],
        },
    }
}

// ---------------------------------------------------------------------------
// common-mode receiver clock jump detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockJumpVerdict {
    pub at_epoch_no: u32,
    pub is_clock_jump: bool,
    pub median_step_cycles: f64,
    pub agreeing_sats: usize,
    pub total_sats: usize,
    pub reason: String,
}

/// A jump common to all satellites in the same sense (L1 band) is a receiver
/// clock event, not independent per-satellite cycle slips.
pub fn detect_clock_jump(set: &SeriesSet, idx: usize, p: &Params) -> ClockJumpVerdict {
    let epoch_no = set.epochs[idx].epoch_no;
    if idx == 0 {
        return ClockJumpVerdict {
            at_epoch_no: epoch_no,
            is_clock_jump: false,
            median_step_cycles: 0.0,
            agreeing_sats: 0,
            total_sats: 0,
            reason: "no previous epoch".into(),
        };
    }
    let dt = set.epochs[idx].seconds - set.epochs[idx - 1].seconds;
    let mut steps = Vec::new();
    let sats: Vec<String> = {
        let mut s = set.bands.iter().map(|b| b.sat.clone()).collect::<Vec<_>>();
        s.sort();
        s.dedup();
        s
    };
    for sat in sats {
        if let Some(bs) = set.bands.iter().find(|b| b.sat == sat && b.band == 1) {
            if let (Some(now), Some(prev)) = (phase(bs, idx), phase(bs, idx - 1)) {
                // Doppler-trend removed step
                let mut d = now - prev;
                let pred = match (
                    bs.points[idx].as_ref().and_then(|q| q.doppler_hz),
                    bs.points[idx - 1].as_ref().and_then(|q| q.doppler_hz),
                ) {
                    (Some(a), Some(b)) => -0.5 * (a + b) * dt,
                    (Some(a), _) => -a * dt,
                    _ => continue,
                };
                d -= pred;
                if d.abs() > p.clock_phase_step_cycles {
                    steps.push(d);
                }
            }
        }
    }
    let total = set
        .bands
        .iter()
        .filter(|b| b.band == 1 && b.points[idx].is_some())
        .count();
    if steps.is_empty() {
        return ClockJumpVerdict {
            at_epoch_no: epoch_no,
            is_clock_jump: false,
            median_step_cycles: 0.0,
            agreeing_sats: 0,
            total_sats: total,
            reason: reasons::NO_BREAK.into(),
        };
    }
    let mut sorted = steps.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let agreeing = steps.iter().filter(|s| (*s - median).abs() < 1.0).count();
    let consensus_ok = total >= p.clock_min_sats
        && agreeing as f64 / total.max(1) as f64 >= p.clock_consensus;
    ClockJumpVerdict {
        at_epoch_no: epoch_no,
        is_clock_jump: consensus_ok,
        median_step_cycles: median,
        agreeing_sats: agreeing,
        total_sats: total,
        reason: if consensus_ok {
            "common_mode_step:receiver_clock_jump".into()
        } else {
            "large_steps_without_consensus:per_satellite".into()
        },
    }
}

// ---------------------------------------------------------------------------
// integer candidate enumeration
// ---------------------------------------------------------------------------

fn zero_candidate(bands: &[u8], unit: Unit) -> IntCandidate {
    IntCandidate {
        bands: bands.to_vec(),
        offset_cycles: vec![0; bands.len()],
        residual: 0.0,
        unit,
        note: None,
    }
}

fn nearest_offset(residual: f64, band: u8, _det: Detector, unit: Unit, _scale: f64) -> Vec<IntCandidate> {
    let n = residual.round() as i64;
    vec![IntCandidate {
        bands: vec![band],
        offset_cycles: vec![n],
        residual: residual - n as f64,
        unit,
        note: None,
    }]
}

/// Enumerate (n1,n2) integer pairs whose GF signature n1*w1 - n2*w2 matches
/// `target` within `tol`. Multiple surviving pairs = underdetermined split.
fn integer_pairs_near(
    target: f64,
    b1: u8,
    b2: u8,
    w1: f64,
    w2: f64,
    tol: f64,
    radius: i64,
) -> Vec<IntCandidate> {
    let mut out = Vec::new();
    for n1 in -radius..=radius {
        for n2 in -radius..=radius {
            let sig = n1 as f64 * w1 - n2 as f64 * w2;
            let r = target - sig;
            if r.abs() <= tol {
                out.push(IntCandidate {
                    bands: vec![b1, b2],
                    offset_cycles: vec![n1, n2],
                    residual: r,
                    unit: Unit::Meter,
                    note: None,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.residual
            .abs()
            .partial_cmp(&b.residual.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn unknown(det: Detector, unit: Unit, reason: &str) -> DetectorOutput {
    DetectorOutput {
        detector: det,
        fired: false,
        unknown: true,
        residual: f64::NAN,
        unit,
        reason: reason.into(),
        candidates: Vec::new(),
    }
}
