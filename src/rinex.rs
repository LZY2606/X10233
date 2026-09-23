//! 导入层：mini-RINEX 文本解析。
//! 伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号和观测码均按历元原样保存；
//! 原始表示保留在 raw_value，规范化（含单位）只发生在派生字段；
//! 无法转换的值留在原位置并附诊断，不为画图丢弃。

use crate::model::{ClockEvent, ObsKind, RawObs};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ParseError {
    #[error("第 {line} 行: {msg}")]
    Line { line: usize, msg: String },
    #[error("文件为空或不包含任何 EPOCH 块")]
    Empty,
}

#[derive(Debug, Default)]
pub struct ParsedFile {
    pub time_scale: String, // 坐标基准，如 "GPS"
    pub station: String,
    pub obs: Vec<RawObs>,
    pub clock_events: Vec<ClockEvent>,
}

pub fn obs_kind_of(code: &str) -> Option<(ObsKind, u8)> {
    let b = code.as_bytes();
    if b.len() != 3 {
        return None;
    }
    let kind = match b[0] {
        b'C' => ObsKind::Pseudorange,
        b'L' => ObsKind::Carrier,
        b'D' => ObsKind::Doppler,
        b'S' => ObsKind::Snr,
        _ => return None,
    };
    if !b[1].is_ascii_digit() {
        return None;
    }
    Some((kind, b[1] - b'0'))
}

pub fn norm_unit_of(kind: ObsKind) -> &'static str {
    match kind {
        ObsKind::Pseudorange => "m",
        ObsKind::Carrier => "cycle",
        ObsKind::Doppler => "Hz",
        ObsKind::Snr => "dB-Hz",
    }
}

/// 解析 ISO 子集 `YYYY-MM-DDTHH:MM:SS[.sss]Z` 或纯整数毫秒。
pub fn parse_epoch_ms(tok: &str) -> Result<i64, String> {
    if let Ok(v) = tok.parse::<i64>() {
        return Ok(v);
    }
    let b = tok.as_bytes();
    if b.len() < 20 || !tok.ends_with('Z') {
        return Err(format!("无法解析历元时间 '{tok}'（期望 ISO8601 Z 或毫秒整数）"));
    }
    let num = |s: &str| s.parse::<i64>().map_err(|_| format!("时间字段非法: '{s}'"));
    let y = num(&tok[0..4])?;
    let mo = num(&tok[5..7])?;
    let d = num(&tok[8..10])?;
    let h = num(&tok[11..13])?;
    let mi = num(&tok[14..16])?;
    let sec_whole = num(&tok[17..19])?;
    let mut frac_ms: i64 = 0;
    if b.len() > 20 {
        if &tok[19..20] != "." {
            return Err(format!("时间秒小数分隔符非法: '{tok}'"));
        }
        let frac = &tok[20..tok.len() - 1];
        if frac.is_empty() || frac.len() > 3 || !frac.bytes().all(|c| c.is_ascii_digit()) {
            return Err(format!("时间毫秒字段非法: '{tok}'"));
        }
        let mut v = frac.parse::<i64>().map_err(|_| "毫秒非法".to_string())?;
        for _ in frac.len()..3 {
            v *= 10;
        }
        frac_ms = v;
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return Err(format!("日期越界: '{tok}'"));
    }
    // days-from-civil (Howard Hinnant)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Ok(days * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec_whole * 1000 + frac_ms)
}

pub fn parse_mini_rinex(text: &str) -> Result<ParsedFile, ParseError> {
    let mut out = ParsedFile::default();
    let mut cur_epoch: Option<i64> = None;
    let mut saw_epoch = false;
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let err = |msg: &str| ParseError::Line { line: line_no, msg: msg.to_string() };
        match toks[0] {
            "TIME_SCALE" => {
                if toks.len() != 2 {
                    return Err(err("TIME_SCALE 需要 1 个参数（坐标基准，如 GPS）"));
                }
                out.time_scale = toks[1].to_string();
            }
            "STATION" => {
                if toks.len() != 2 {
                    return Err(err("STATION 需要 1 个参数"));
                }
                out.station = toks[1].to_string();
            }
            "EPOCH" => {
                if toks.len() != 2 {
                    return Err(err("EPOCH 需要 1 个时间参数"));
                }
                cur_epoch = Some(parse_epoch_ms(toks[1]).map_err(|m| err(&m))?);
                saw_epoch = true;
            }
            "OBS" => {
                if toks.len() < 4 {
                    return Err(err("OBS 需要: <sat> <code> <value> [LLI=n]"));
                }
                let epoch = cur_epoch.ok_or_else(|| err("OBS 出现在任何 EPOCH 之前"))?;
                let sat = toks[1];
                let code = toks[2];
                let (kind, freq_no) = obs_kind_of(code)
                    .ok_or_else(|| err(&format!("观测码 '{code}' 非法（期望如 L1C/C2W/D1C/S2W）")))?;
                let raw_value = toks[3].to_string();
                let mut lli = 0u8;
                for extra in &toks[4..] {
                    if let Some(v) = extra.strip_prefix("LLI=") {
                        lli = v.parse::<u8>().map_err(|_| err("LLI 需为整数"))?;
                    } else {
                        return Err(err(&format!("未知 OBS 附加字段 '{extra}'")));
                    }
                }
                // 原样保存；规范化失败不丢值，只附诊断。
                let (norm_value, norm_unit, diag) = match raw_value.parse::<f64>() {
                    Ok(v) => (Some(v), Some(norm_unit_of(kind).to_string()), None),
                    Err(_) => (None, None, Some(format!("值 '{raw_value}' 无法转换为数值，保留原始表示"))),
                };
                out.obs.push(RawObs {
                    station: out.station.clone(),
                    sat: sat.to_string(),
                    epoch_ms: epoch,
                    code: code.to_string(),
                    kind,
                    freq_no,
                    raw_value,
                    lli,
                    norm_value,
                    norm_unit,
                    diag,
                });
            }
            "CLOCK" => {
                if toks.len() != 2 {
                    return Err(err("CLOCK 需要 1 个参数，如 +1ms"));
                }
                let epoch = cur_epoch.ok_or_else(|| err("CLOCK 出现在任何 EPOCH 之前"))?;
                let raw = toks[1].to_string();
                let (offset_ms, diag) = match raw.strip_suffix("ms").and_then(|s| s.parse::<f64>().ok()) {
                    Some(v) => (Some(v), None),
                    None => (None, Some(format!("钟跳量 '{raw}' 无法解析，保留原始表示"))),
                };
                out.clock_events.push(ClockEvent {
                    station: out.station.clone(),
                    epoch_ms: epoch,
                    kind: "receiver_clock_jump".to_string(),
                    raw_value: raw,
                    offset_ms,
                    diag,
                });
            }
            "END" => {
                cur_epoch = None;
            }
            other => {
                return Err(err(&format!("未知记录类型 '{other}'")));
            }
        }
    }
    if !saw_epoch {
        return Err(ParseError::Empty);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_iso_and_ms() {
        assert_eq!(parse_epoch_ms("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(parse_epoch_ms("1970-01-02T00:00:00.500Z").unwrap(), 86_400_500);
        assert_eq!(parse_epoch_ms("12345").unwrap(), 12345);
        assert!(parse_epoch_ms("2026-13-01T00:00:00Z").is_err());
    }

    #[test]
    fn bad_format_reports_line() {
        let r = parse_mini_rinex("STATION S1\nEPOCH 0\nOBS G05 L1C\n");
        match r {
            Err(ParseError::Line { line, .. }) => assert_eq!(line, 3),
            _ => panic!("期望行号错误"),
        }
    }

    #[test]
    fn unconvertible_value_kept_with_diag() {
        let r = parse_mini_rinex("STATION S1\nEPOCH 0\nOBS G05 L1C ***\n").unwrap();
        assert_eq!(r.obs[0].raw_value, "***");
        assert!(r.obs[0].norm_value.is_none());
        assert!(r.obs[0].diag.is_some());
    }
}
