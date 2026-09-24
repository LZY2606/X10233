use serde::{Deserialize, Serialize};

pub const F1_HZ: f64 = 1575.42e6;
pub const F2_HZ: f64 = 1227.60e6;
pub const C_MPS: f64 = 299792458.0;
pub const ALGO_VERSION: i64 = 1;

pub fn wide_lane_wavelength_m() -> f64 {
    C_MPS / (F1_HZ - F2_HZ)
}

/// Detection / segmentation parameters. Every derived value records the
/// fingerprint of the parameter set that produced it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub arc_gap_s: f64,
    pub td_threshold_cycles: f64,
    pub gf_threshold_cycles: f64,
    pub mw_threshold_wl: f64,
    pub clock_min_sats: i64,
    pub ambiguity_window: i64,
    pub algo_version: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            arc_gap_s: 60.0,
            td_threshold_cycles: 0.75,
            gf_threshold_cycles: 0.5,
            mw_threshold_wl: 0.5,
            clock_min_sats: 3,
            ambiguity_window: 1,
            algo_version: ALGO_VERSION,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        fnv1a(&format!(
            "arc_gap_s={}/td_thr={}/gf_thr={}/mw_thr={}/clock_min_sats={}/amb_window={}/algo={}",
            self.arc_gap_s,
            self.td_threshold_cycles,
            self.gf_threshold_cycles,
            self.mw_threshold_wl,
            self.clock_min_sats,
            self.ambiguity_window,
            self.algo_version
        ))
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Params> {
        serde_json::from_value(v.clone()).ok()
    }
}

pub fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Virtual-time clock: fixed in tests, system time in the server.
#[derive(Clone, Copy, Debug)]
pub enum Clock {
    Fixed(i64),
    System,
}

impl Clock {
    pub fn now_ms(&self) -> i64 {
        match self {
            Clock::Fixed(t) => *t,
            Clock::System => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        }
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` into milliseconds since Unix epoch.
pub fn parse_epoch(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let (date, time) = s
        .split_once('T')
        .ok_or_else(|| format!("invalid epoch `{s}`: missing 'T'"))?;
    let time = time
        .strip_suffix('Z')
        .ok_or_else(|| format!("invalid epoch `{s}`: must end with 'Z' (UTC)"))?;
    let dparts: Vec<&str> = date.split('-').collect();
    if dparts.len() != 3 {
        return Err(format!("invalid epoch `{s}`: bad date"));
    }
    let y: i64 = dparts[0].parse().map_err(|_| format!("invalid epoch `{s}`: bad year"))?;
    let mo: i64 = dparts[1].parse().map_err(|_| format!("invalid epoch `{s}`: bad month"))?;
    let d: i64 = dparts[2].parse().map_err(|_| format!("invalid epoch `{s}`: bad day"))?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return Err(format!("invalid epoch `{s}`: date out of range"));
    }
    let (hms, frac_ms) = match time.split_once('.') {
        Some((h, f)) => {
            let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                return Err(format!("invalid epoch `{s}`: bad fraction"));
            }
            let scaled = format!("{:0<3}", &digits[..digits.len().min(3)]);
            (h.to_string(), scaled.parse::<i64>().unwrap_or(0))
        }
        None => (time.to_string(), 0),
    };
    let tparts: Vec<&str> = hms.split(':').collect();
    if tparts.len() != 3 {
        return Err(format!("invalid epoch `{s}`: bad time"));
    }
    let h: i64 = tparts[0].parse().map_err(|_| format!("invalid epoch `{s}`: bad hour"))?;
    let mi: i64 = tparts[1].parse().map_err(|_| format!("invalid epoch `{s}`: bad minute"))?;
    let sec: i64 = tparts[2].parse().map_err(|_| format!("invalid epoch `{s}`: bad second"))?;
    if h > 23 || mi > 59 || sec > 60 {
        return Err(format!("invalid epoch `{s}`: time out of range"));
    }
    let days = days_from_civil(y, mo, d);
    Ok((((days * 24 + h) * 60 + mi) * 60 + sec) * 1000 + frac_ms)
}

pub fn fmt_epoch(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, mo, d) = civil_from_days(days);
    let h = rem / 3_600_000;
    let mi = (rem / 60_000) % 60;
    let sec = (rem / 1000) % 60;
    let mmm = rem % 1000;
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.{mmm:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_roundtrip() {
        let ms = parse_epoch("2026-09-19T00:01:00Z").unwrap();
        assert_eq!(fmt_epoch(ms), "2026-09-19T00:01:00.000Z");
        let ms2 = parse_epoch("2026-09-19T00:01:00.250Z").unwrap();
        assert_eq!(ms2 - ms, 250);
        assert!(parse_epoch("2026-09-19 00:01:00").is_err());
        assert!(parse_epoch("not-a-time").is_err());
    }

    #[test]
    fn fingerprint_stable() {
        let p = Params::default();
        assert_eq!(p.fingerprint(), p.fingerprint());
        let mut q = p.clone();
        q.td_threshold_cycles = 0.5;
        assert_ne!(p.fingerprint(), q.fingerprint());
    }
}
