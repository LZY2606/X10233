//! 固定 seed 的虚拟时间与紧凑 ISO8601 换算（确定性，测试可重放）。

pub const SEED_UNIX: i64 = 1_767_225_600; // 2026-01-01T00:00Z

pub fn format_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub struct VirtualClock {
    cur: i64,
    step: i64,
    calls: u64,
}

impl VirtualClock {
    pub fn seeded(step_secs: i64) -> Self {
        Self { cur: SEED_UNIX, step: step_secs, calls: 0 }
    }
    pub fn tick(&mut self) -> String {
        let t = self.cur + self.calls as i64 * self.step;
        self.calls += 1;
        format_unix(t)
    }
}

/// 解析紧凑 ISO8601（可带 Z）为 Unix 秒。
pub fn parse_iso(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("+00:00")).unwrap_or(s);
    let (date, time) = s.split_once('T')?;
    let d: Vec<i64> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    let t: Vec<i64> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    if d.len() != 3 || t.len() != 3 {
        return None;
    }
    Some(civil_to_days(d[0], d[1] as u32, d[2] as u32) * 86_400 + t[0] * 3600 + t[1] * 60 + t[2])
}

fn civil_to_days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}
