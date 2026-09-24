//! 虚拟时间：所有演示与测试使用固定起点、固定步长，不读取系统时钟。

use std::sync::Mutex;

pub trait Clock {
    fn now_rfc3339(&self) -> String;
    fn tick(&self) -> String;
}

/// 固定 seed 的虚拟时钟。起点固定为 2026-01-01T00:00:00Z，
/// 每次事件提交步进固定秒数，保证重放与测试确定性。
#[derive(Debug)]
pub struct DeterministicClock {
    inner: Mutex<State>,
    step_seconds: u64,
}

#[derive(Debug, Clone, Copy)]
struct State {
    epoch_seconds: i64,
}

const BASE_EPOCH_SECONDS: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z

impl DeterministicClock {
    pub fn new(step_seconds: u64) -> Self {
        Self {
            inner: Mutex::new(State { epoch_seconds: BASE_EPOCH_SECONDS }),
            step_seconds,
        }
    }

    pub fn at(epoch_seconds: i64) -> String {
        format_rfc3339(epoch_seconds)
    }
}

impl Clock for DeterministicClock {
    fn now_rfc3339(&self) -> String {
        let s = self.inner.lock().unwrap();
        format_rfc3339(s.epoch_seconds)
    }

    fn tick(&self) -> String {
        let mut s = self.inner.lock().unwrap();
        let out = format_rfc3339(s.epoch_seconds);
        s.epoch_seconds += self.step_seconds as i64;
        out
    }
}

/// 仅支持 UTC 整秒 RFC3339 输出，演示数据不需要亚秒墙钟。
pub fn format_rfc3339(epoch_seconds: i64) -> String {
    let (days, rem) = (epoch_seconds.div_euclid(86_400), epoch_seconds.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Howard Hinnant 的 days->civil 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// RFC3339（整秒，Z 或数字偏移）-> UTC epoch 秒；导入时用于规范化，
/// 失败时原始字符串留在原处并附诊断，而不是丢数据。
pub fn parse_rfc3339(s: &str) -> Result<i64, String> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return Err(format!("时间戳不是 RFC3339 形式: {s:?}"));
    }
    let year: i64 = s[0..4].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let month: u32 = s[5..7].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let day: u32 = s[8..10].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let hour: u32 = s[11..13].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let min: u32 = s[14..16].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let sec_tail = &s[17..];
    let (sec_str, tz_off) = match sec_tail.find(['Z', '+', '-']) {
        Some(idx) => {
            let sec: u32 = sec_tail[..idx].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let tz = &sec_tail[idx..];
            let off = if tz == "Z" {
                0i64
            } else {
                if tz.len() != 6 || tz.as_bytes()[3] != b':' {
                    return Err(format!("无法识别的时区偏移: {tz:?}"));
                }
                let sign = if tz.starts_with('+') { 1i64 } else { -1i64 };
                let oh: i64 = tz[1..3].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
                let om: i64 = tz[4..6].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
                sign * (oh * 3600 + om * 60)
            };
            (sec, off)
        }
        None => return Err("时间戳缺少时区标识".into()),
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return Err("时间分量越界".into());
    }
    let days = days_from_civil(year, month as i64, day as i64);
    Ok(days * 86_400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64 - tz_off)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_base_epoch() {
        assert_eq!(format_rfc3339(BASE_EPOCH_SECONDS), "2026-01-01T00:00:00Z");
        assert_eq!(parse_rfc3339("2026-01-01T00:00:00Z").unwrap(), BASE_EPOCH_SECONDS);
    }

    #[test]
    fn offset_is_normalized() {
        assert_eq!(
            parse_rfc3339("2026-01-01T09:00:00+09:00").unwrap(),
            parse_rfc3339("2026-01-01T00:00:00Z").unwrap()
        );
    }
}
