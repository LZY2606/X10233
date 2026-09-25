//! 民用历元 <-> GPS 周内秒。纯确定性换算，不接触系统时钟。

use serde::{Deserialize, Serialize};

/// 民用 UTC 历元（案例与 RINEX 中以 UTC/GPS 民用表示保存；虚拟时间）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Civil {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub nanos: u32,
}

impl Civil {
    pub const fn new(year: i32, month: u8, day: u8, hour: u8, minute: u8, nanos: u32) -> Self {
        Self { year, month, day, hour, minute, nanos }
    }

    /// ISO-8601（秒精度以下用 9 位小数），示例 2026-01-01T00:00:30.500000000Z
    pub fn to_iso(&self) -> String {
        let sec = self.nanos / 1_000_000_000;
        let frac = self.nanos % 1_000_000_000;
        if frac == 0 {
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                self.year, self.month, self.day, self.hour, self.minute, sec
            )
        } else {
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
                self.year, self.month, self.day, self.hour, self.minute, sec, frac
            )
        }
    }

    pub fn parse_iso(s: &str) -> Option<Self> {
        let s = s.strip_suffix('Z').unwrap_or(s);
        let (date, time) = s.split_once('T')?;
        let mut d = date.split('-');
        let mut t = time.split(':');
        let year = d.next()?.parse().ok()?;
        let month = d.next()?.parse().ok()?;
        let day = d.next()?.parse().ok()?;
        let hour = t.next()?.parse().ok()?;
        let minute = t.next()?.parse().ok()?;
        let sec_tok = t.next()?;
        let (sec_tok, frac_tok) = match sec_tok.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (sec_tok, None),
        };
        let sec: u32 = sec_tok.parse().ok()?;
        let mut nanos = sec * 1_000_000_000;
        if let Some(f) = frac_tok {
            let mut p = String::from(f);
            while p.len() < 9 {
                p.push('0');
            }
            p.truncate(9);
            nanos += p.parse::<u32>().ok()?;
        }
        Some(Self::new(year, month, day, hour, minute, nanos))
    }

    /// RINEX 历元头使用的浮点秒
    pub fn rinex_seconds(&self) -> f64 {
        (self.nanos as f64) / 1e9
    }
}

impl std::fmt::Display for Civil {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_iso())
    }
}

/// Howard Hinnant days_from_civil（proleptic Gregorian）。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u8, d as u8)
}

/// GPS 历元：周数 + 周内秒
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GpsEpoch {
    pub week: i32,
    pub tow_s: f64,
}

const GPS_EPOCH_DAYS: i64 = 3657; // 1980-01-06 相对 1970-01-01

impl Civil {
    pub fn to_gps(&self) -> GpsEpoch {
        let days = days_from_civil(self.year, self.month as u32, self.day as u32) - GPS_EPOCH_DAYS;
        let sod = self.hour as i64 * 3600 + self.minute as i64 * 60
            + (self.nanos / 1_000_000_000) as i64;
        let mut total = days * 86_400 + sod;
        let mut week = (total / 604_800) as i32;
        total %= 604_800;
        if total < 0 {
            week -= 1;
            total += 604_800;
        }
        GpsEpoch { week, tow_s: total as f64 + (self.nanos % 1_000_000_000) as f64 / 1e9 }
    }

    pub fn from_gps(g: GpsEpoch) -> Self {
        let total = g.week as i64 * 604_800 + g.tow_s.floor() as i64;
        let days = total.div_euclid(86_400) + GPS_EPOCH_DAYS;
        let sod = total.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        let frac = ((g.tow_s.fract()) * 1e9).round() as u32;
        Civil::new(
            y,
            m,
            d,
            (sod / 3600) as u8,
            ((sod % 3600) / 60) as u8,
            (sod % 60) as u32 * 1_000_000_000 + frac,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_roundtrip_known_epoch() {
        let c = Civil::new(2026, 1, 1, 0, 0, 0);
        let g = c.to_gps();
        assert_eq!(g.week, 2400);
        assert_eq!(g.tow_s, 0.0);
        let back = Civil::from_gps(g);
        assert_eq!(back, c);
    }

    #[test]
    fn iso_roundtrip() {
        let c = Civil::new(2026, 1, 1, 2, 0, 30_500_000_000);
        let s = c.to_iso();
        assert_eq!(Civil::parse_iso(&s), Some(c));
    }
}
