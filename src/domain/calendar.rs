//! 公历/GNSS 时间：RINEX 历元（年月日时分秒）<-> UNIX 毫秒。
//!
//! 此处把 RINEX 时间标度视作与 UTC 对齐（案例为演示，不含闰秒表）。
//! 所有派生时间一律走此模块，便于固定 seed 重放。

pub const fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

pub fn days_in_month(y: i64, m: u32) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 0,
    }
}

/// 公历日期 -> 相对 1970-01-01 的天数（Howard Hinnant 算法）。
pub fn days_from_civil(y_in: i64, m_in: u32, d: i64) -> i64 {
    let y = if m_in <= 2 { y_in - 1 } else { y_in };
    let era = if y >= 0 { y } else { 0 } / 400;
    let yoe = (y - era * 400) as i64;
    let m = m_in;
    let doy = ((153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d - 1) as i64;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m as u32, d)
}

pub fn to_unix_ms(y: i64, mo: u32, d: i64, hh: u32, mm: u32, sec: f64) -> i64 {
    let days = days_from_civil(y, mo, d);
    let whole_sec = sec.floor() as i64;
    let milli = ((sec - whole_sec as f64) * 1000000.0).round() as i64 / 1000;
    (days * 86_400 + hh as i64 * 3600 + mm as i64 * 60 + whole_sec) * 1000 + milli
}

pub fn from_unix_ms(ms: i64) -> (i64, u32, i64, u32, u32, u32, i64) {
    let day = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(day);
    let milli = rem % 1000;
    let secs = rem / 1000;
    let hh = (secs / 3600) as u32;
    let mm = ((secs % 3600) / 60) as u32;
    let ss = (secs % 60) as u32;
    (y, m, d, hh, mm, ss, milli)
}
