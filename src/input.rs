//! 导入层：解析可复制的小型 v1 样例。
//!
//! 原始片段逐行原样保留；可解析的值派生为带单位的量，无法转换的值留在原位置并附诊断。
//! 解析结果用于原样入库，领域规范化只发生在派生层。

use crate::units::{Datum, Qty, Unit};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: &str = "gsb-raw-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line_no: i64,
    pub code: String,
    pub message: String,
    pub raw_line: String,
    pub fatal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RawBatch {
    pub format_version: String,
    pub marker_name: String,
    pub marker_number: String,
    pub receiver: String,
    pub obs_records: Vec<RawObs>,
    pub clock_events: Vec<RawClockEvent>,
    pub sat_states: Vec<RawSatState>,
    pub freqs: Vec<RawFreq>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObs {
    pub line_no: i64,
    pub epoch_iso: String,
    pub sat: String,
    pub code: String,
    pub band: i64,
    pub pseudo: Qty,
    pub phase: Qty,
    pub doppler: Qty,
    pub lli: Option<i64>,
    pub lli_raw: String,
    pub snr: Qty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEvent {
    pub line_no: i64,
    pub epoch_iso: String,
    pub kind: String,
    pub delta: Qty,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSatState {
    pub line_no: i64,
    pub epoch_iso: String,
    pub sat: String,
    pub x: Qty,
    pub y: Qty,
    pub z: Qty,
    pub clock_bias: Qty,
    pub datum: Datum,
    pub health: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFreq {
    pub line_no: i64,
    pub sat: String,
    pub band: i64,
    pub code: String,
    pub frequency_hz: Qty,
}

pub fn parse(text: &str) -> RawBatch {
    let mut batch = RawBatch { format_version: FORMAT_VERSION.to_string(), ..Default::default() };
    let mut current_epoch: Option<String> = None;
    let mut seen_header = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx as i64 + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
        let kind = fields[0];

        match kind {
            "META" if fields.len() >= 2 => {
                seen_header = true;
                if fields.get(1).copied() != Some(FORMAT_VERSION) {
                    fatal(&mut batch, line_no, raw_line, "BAD_FORMAT_VERSION", &format!(
                        "格式版本应为 {FORMAT_VERSION}，读到 {:?}", fields.get(1).copied().unwrap_or("")));
                }
            }
            "STATION" if fields.len() >= 4 => {
                seen_header = true;
                batch.marker_name = fields[1].to_string();
                batch.marker_number = fields[2].to_string();
                batch.receiver = fields[3].to_string();
            }
            "SATELLITE" if fields.len() >= 3 => {
                seen_header = true;
                match fields[2].parse::<i64>() {
                    Ok(_) => {}
                    Err(_) => nonfatal(&mut batch, line_no, raw_line, "BAD_BAND", "频率编号必须是整数"),
                }
            }
            "FREQ" if fields.len() >= 5 => {
                if let Some(band) = fields[2].parse::<i64>().ok() {
                    batch.freqs.push(RawFreq {
                        line_no, sat: fields[1].to_string(), band, code: fields[3].to_string(),
                        frequency_hz: Qty::parsed(fields[4], Unit::Hertz),
                    });
                } else {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_BAND", "FREQ 频率编号非法");
                }
            }
            "EPOCH" if fields.len() >= 2 => {
                if parse_epoch(fields[1]).is_some() {
                    current_epoch = Some(fields[1].to_string());
                } else {
                    fatal(&mut batch, line_no, raw_line, "BAD_EPOCH", &format!("历元时间戳非法：{:?}", fields[1]));
                }
            }
            "OBS" => {
                if !seen_header {
                    fatal(&mut batch, line_no, raw_line, "NO_HEADER", "缺少 META/STATION 头部");
                    continue;
                }
                if fields.len() < 9 {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_OBS", "OBS 需要 sat|code|band|pseudo|phase|doppler|lli|snr 共 8 列");
                    continue;
                }
                let epoch = match &current_epoch {
                    Some(e) => e.clone(),
                    None => { nonfatal(&mut batch, line_no, raw_line, "ORPHAN_OBS", "OBS 之前没有 EPOCH"); continue; }
                };
                let band = match fields[3].parse::<i64>() {
                    Ok(b) => b,
                    Err(_) => { nonfatal(&mut batch, line_no, raw_line, "BAD_BAND", "OBS 频率编号非法"); continue; }
                };
                let lli_raw = fields[7].to_string();
                let lli = fields[7].parse::<i64>().ok();
                if lli.is_none() {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_LLI", &format!("LLI 非整数：{:?}", fields[7]));
                }
                batch.obs_records.push(RawObs {
                    line_no, epoch_iso: epoch, sat: fields[1].to_string(), code: fields[2].to_string(), band,
                    pseudo: Qty::parsed(fields[4], Unit::Meter),
                    phase: Qty::parsed(fields[5], Unit::Cycle),
                    doppler: Qty::parsed(fields[6], Unit::Hertz),
                    lli, lli_raw,
                    snr: Qty::parsed(fields[8], Unit::DbHz),
                });
            }
            "CLOCK" => {
                if fields.len() < 5 {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_CLOCK", "CLOCK 需要 epoch|kind|delta_seconds|note");
                    continue;
                }
                if parse_epoch(fields[1]).is_none() {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_EPOCH", &format!("CLOCK 时间戳非法：{:?}", fields[1]));
                    continue;
                }
                batch.clock_events.push(RawClockEvent {
                    line_no, epoch_iso: fields[1].to_string(), kind: fields[2].to_string(),
                    delta: Qty::parsed(fields[3], Unit::Second), note: fields[4].to_string(),
                });
            }
            "STATE" => {
                if fields.len() < 8 {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_STATE", "STATE 需要 epoch|sat|x|y|z|clock_bias|health");
                    continue;
                }
                if parse_epoch(fields[1]).is_none() {
                    nonfatal(&mut batch, line_no, raw_line, "BAD_EPOCH", &format!("STATE 时间戳非法：{:?}", fields[1]));
                    continue;
                }
                batch.sat_states.push(RawSatState {
                    line_no, epoch_iso: fields[1].to_string(), sat: fields[2].to_string(),
                    x: Qty::parsed(fields[3], Unit::Meter),
                    y: Qty::parsed(fields[4], Unit::Meter),
                    z: Qty::parsed(fields[5], Unit::Meter),
                    clock_bias: Qty::parsed(fields[6], Unit::Second),
                    datum: Datum::default(),
                    health: fields[7].to_string(),
                });
            }
            _ => {
                nonfatal(&mut batch, line_no, raw_line, "UNKNOWN_LINE", &format!("无法识别的记录类型 {:?}", kind));
            }
        }
    }

    if !seen_header {
        fatal(&mut batch, 0, "", "NO_HEADER", "样例缺少 META/STATION 头部");
    }
    batch
}

fn nonfatal(b: &mut RawBatch, line_no: i64, raw_line: &str, code: &str, message: &str) {
    b.diagnostics.push(Diagnostic { line_no, code: code.to_string(), message: message.to_string(), raw_line: raw_line.to_string(), fatal: false });
}

fn fatal(b: &mut RawBatch, line_no: i64, raw_line: &str, code: &str, message: &str) {
    b.diagnostics.push(Diagnostic { line_no, code: code.to_string(), message: message.to_string(), raw_line: raw_line.to_string(), fatal: true });
}

/// 严格 UTC 解析：`YYYY-MM-DDTHH:MM:SSZ`，可选小数秒。
pub fn parse_epoch(s: &str) -> Option<i64> {
    let b = s.strip_suffix('Z')?;
    let (date, time) = b.split_once('T')?;
    let d: Vec<&str> = date.split('-').collect();
    if d.len() != 3 { return None; }
    let year: i64 = d[0].parse().ok()?;
    let mon: i64 = d[1].parse().ok()?;
    let day: i64 = d[2].parse().ok()?;
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, f),
        None => (time, "0"),
    };
    let t: Vec<&str> = hms.split(':').collect();
    if t.len() != 3 { return None; }
    let hour: i64 = t[0].parse().ok()?;
    let min: i64 = t[1].parse().ok()?;
    let sec: i64 = t[2].parse().ok()?;
    let micros: i64 = format!("{:0<6}", frac).get(0..6)?.parse().ok()?;
    if !(1..=12).contains(&mon) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(year as i32, mon as u32, day as u32) as i64;
    Some(days * 86_400 + hour * 3600 + min * 60 + sec)
        .and_then(|v| v.checked_mul(1_000_000))
        .map(|v| v + micros)
}

/// Howard Hinnant civil-from-days 的 days_from_civil。
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn format_epoch(micros: i64) -> String {
    let secs = micros.div_euclid(1_000_000);
    let micros = micros.rem_euclid(1_000_000);
    let days = secs.div_euclid(86_400);
    let mut rem = secs.rem_euclid(86_400);
    let hour = rem / 3600; rem %= 3600;
    let min = rem / 60;
    let sec = rem % 60;
    let (year, month, day) = civil_from_days(days as i32);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z", year, month, day, hour, min, sec, micros)
}

fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_roundtrip() {
        let s = "2026-09-20T00:00:10Z";
        let us = parse_epoch(s).unwrap();
        assert_eq!(format_epoch(us), "2026-09-20T00:00:10.000000Z");
        assert!(parse_epoch("2026-13-40T99:00:00Z").is_none());
    }

    #[test]
    fn parses_sample_and_keeps_unconvertible() {
        let text = "META|gsb-raw-v1\nSTATION|TEST|S1|RX1\nEPOCH|2026-09-20T00:00:00Z\nOBS|G07|L1W|1|2.1e7|1.1e8|1234.5|0|42.0\nSTATE|2026-09-20T00:00:00Z|G07|1.0e7|2.0e7|3.0e7|1.2e-7|OK\nBOGUS\n";
        let b = parse(text);
        assert_eq!(b.diagnostics.iter().filter(|d| !d.fatal).count(), 1);
        assert_eq!(b.obs_records.len(), 1);
        assert_eq!(b.sat_states.len(), 1);
        assert_eq!(b.obs_records[0].phase.unit.as_str(), "cycle");
        assert!(b.obs_records[0].snr.is_usable());
    }

    #[test]
    fn unconvertible_qty_stays_in_place() {
        let q = Qty::parsed("BAD", Unit::Meter);
        assert!(q.value.is_none());
        assert!(q.diagnostic.is_some());
        assert_eq!(q.raw, "BAD");
    }

    #[test]
    fn fatal_on_bad_format_version() {
        let b = parse("META|something-else\n");
        assert!(b.diagnostics.iter().any(|d| d.fatal));
    }
}
