//! 领域模型：GNSS 载波相位 / 周跳候选 / 弧段。所有领域量携带单位或坐标基准。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const L1_HZ: f64 = 1575.42e6;
pub const L2_HZ: f64 = 1227.60e6;
pub const C_MPS: f64 = 299_792_458.0;

/// 频率编号 -> 载波频率 (Hz)。原始文件中频率编号按历元原样保存，这里仅用于派生层。
pub fn freq_hz(n: i64) -> Option<f64> {
    match n {
        1 => Some(L1_HZ),
        2 => Some(L2_HZ),
        _ => None,
    }
}

pub fn lambda_m(freq_no: i64) -> Option<f64> {
    freq_hz(freq_no).map(|f| C_MPS / f)
}

/// 检测参数。任何派生值都带算法版本、参数指纹与直接来源。
pub const DETECTOR_ALGO_VERSION: &str = "detector/1.0.0";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// 时间差分检测阈值（周）
    pub td_threshold_cycles: f64,
    /// 电离层（几何无关）组合阈值（周）
    pub gf_threshold_cycles: f64,
    /// Melbourne-Wubbena 阈值（宽巷周）
    pub mw_threshold_cycles: f64,
    /// 弧段断裂间隔因子：dt > factor * 中位 dt 即断弧
    pub max_gap_factor: f64,
    /// 判定整机钟跳所需的最少卫星数
    pub clock_min_sats: i64,
    /// 钟跳聚合容差（毫秒）
    pub clock_tol_ms: f64,
    /// 等分整周偏移模糊度窗口：|frac(est)| 距 0.5 小于该值时保留全部合法候选
    pub ambig_eps: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            td_threshold_cycles: 0.5,
            gf_threshold_cycles: 0.5,
            mw_threshold_cycles: 0.5,
            max_gap_factor: 2.5,
            clock_min_sats: 3,
            clock_tol_ms: 0.001,
            ambig_eps: 0.02,
        }
    }
}

impl Params {
    pub fn fingerprint(&self) -> String {
        // 结构体序列化字段顺序固定，指纹稳定
        let s = serde_json::to_string(self).unwrap_or_default();
        sha256_hex(s.as_bytes())
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// ISO-8601 (UTC, `YYYY-MM-DDTHH:MM:SS[.sss]Z`) -> 毫秒。失败返回 None（导入层据此给出诊断）。
pub fn parse_epoch_ms(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let time = time.strip_suffix('Z').unwrap_or(time);
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: i64 = dp.next()?.parse().ok()?;
    let d: i64 = dp.next()?.parse().ok()?;
    let mut tp = time.split(':');
    let hh: i64 = tp.next()?.parse().ok()?;
    let mm: i64 = tp.next()?.parse().ok()?;
    let ss_raw = tp.next()?;
    let (ss, frac_ms) = match ss_raw.split_once('.') {
        Some((a, b)) => (a.parse::<i64>().ok()?, frac_to_ms(b)?),
        None => (ss_raw.parse::<i64>().ok()?, 0),
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m as u32, d as u32);
    Some((((days * 24 + hh) * 60 + mm) * 60 + ss) * 1000 + frac_ms)
}

fn frac_to_ms(frac: &str) -> Option<i64> {
    if frac.is_empty() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut v: i64 = frac.parse().ok()?;
    for _ in frac.len()..3 {
        v *= 10;
    }
    for _ in 3..frac.len() {
        v /= 10;
    }
    Some(v)
}

/// Howard Hinnant 的 civil->days 算法，1970-01-01 为 0。
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn fmt_epoch_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3_600_000;
    let mm = (rem % 3_600_000) / 60_000;
    let ss = (rem % 60_000) / 1000;
    let mss = rem % 1000;
    if mss == 0 {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hh, mm, ss)
    } else {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, m, d, hh, mm, ss, mss)
    }
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 诊断：一等数据，随导入/检测持久化。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diag {
    pub line: Option<usize>,
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_roundtrip() {
        let ms = parse_epoch_ms("2026-09-19T00:00:00Z").unwrap();
        assert_eq!(fmt_epoch_ms(ms), "2026-09-19T00:00:00Z");
        let ms2 = parse_epoch_ms("2026-09-19T00:10:15.250Z").unwrap();
        assert_eq!(ms2 - ms, 615_000 + 250);
        assert_eq!(fmt_epoch_ms(ms2), "2026-09-19T00:10:15.250Z");
        assert!(parse_epoch_ms("2026-13-01T00:00:00Z").is_none());
        assert!(parse_epoch_ms("not-a-time").is_none());
    }
}
