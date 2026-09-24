//! 历元换算：ISO-8601(毫秒, Z) <-> 毫秒计数。领域逻辑只使用历元毫秒与逻辑 tick。

/// Howard Hinnant 的 days_from_civil。
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 解析 "2026-09-19T00:00:00.000Z" -> 毫秒计数。错误信息面向导入层。
pub fn iso_to_ms(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let Some(stripped) = s.strip_suffix('Z') else {
        return Err(format!("历元 '{s}' 缺少 Z 后缀（时间基准必须显式标注）"));
    };
    let Some((date, time)) = stripped.split_once('T') else {
        return Err(format!("历元 '{s}' 缺少 T 分隔符"));
    };
    let dparts: Vec<&str> = date.split('-').collect();
    if dparts.len() != 3 {
        return Err(format!("历元 '{s}' 日期部分应为 YYYY-MM-DD"));
    }
    let y: i64 = dparts[0].parse().map_err(|_| format!("历元 '{s}' 年份无效"))?;
    let mo: i64 = dparts[1].parse().map_err(|_| format!("历元 '{s}' 月份无效"))?;
    let d: i64 = dparts[2].parse().map_err(|_| format!("历元 '{s}' 日无效"))?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return Err(format!("历元 '{s}' 日期越界"));
    }
    let tparts: Vec<&str> = time.split(':').collect();
    if tparts.len() != 3 {
        return Err(format!("历元 '{s}' 时间部分应为 HH:MM:SS.mmm"));
    }
    let h: i64 = tparts[0].parse().map_err(|_| format!("历元 '{s}' 小时无效"))?;
    let mi: i64 = tparts[1].parse().map_err(|_| format!("历元 '{s}' 分钟无效"))?;
    let spart = tparts[2];
    let (sec_s, frac_ms) = match spart.split_once('.') {
        Some((a, b)) => {
            let mut frac = b.to_string();
            while frac.len() < 3 {
                frac.push('0');
            }
            let frac: i64 = frac[..3]
                .parse()
                .map_err(|_| format!("历元 '{s}' 毫秒无效"))?;
            (a.to_string(), frac)
        }
        None => (spart.to_string(), 0),
    };
    let sec: i64 = sec_s.parse().map_err(|_| format!("历元 '{s}' 秒无效"))?;
    if h > 23 || mi > 59 || sec > 60 {
        return Err(format!("历元 '{s}' 时间越界"));
    }
    Ok(days_from_civil(y, mo, d) * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec * 1000 + frac_ms)
}

pub fn ms_to_iso(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, mo, d) = civil_from_days(days);
    let h = rem / 3_600_000;
    let mi = (rem % 3_600_000) / 60_000;
    let sec = (rem % 60_000) / 1000;
    let msec = rem % 1000;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, h, mi, sec, msec)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let ms = iso_to_ms("2026-09-19T00:00:00.000Z").unwrap();
        assert_eq!(ms_to_iso(ms), "2026-09-19T00:00:00.000Z");
        let ms2 = iso_to_ms("2026-09-19T00:03:02.000Z").unwrap();
        assert_eq!(ms2 - ms, 182_000);
    }
}
