//! RNXL：RINEX 风格的行式观测文本。导入层把伪距、载波、Doppler、LLI、
//! 信噪比、钟跳事件、频率编号和观测码均按历元原样保存；解析失败的值
//! 留在原位置并附诊断，不为派生计算丢掉。

use crate::model::Unit;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObs {
    pub epoch_raw: String,
    pub epoch_ms: i64,
    pub sat: String,
    pub freq: u8,
    pub code: String,
    pub raw_value: String,
    pub value: Option<f64>,
    pub unit: Unit,
    pub diag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawClockEvent {
    pub epoch_raw: String,
    pub epoch_ms: i64,
    pub raw_offset: String,
    pub offset_s: Option<f64>,
    pub diag: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedInput {
    pub station: String,
    pub obs: Vec<RawObs>,
    pub clock_events: Vec<RawClockEvent>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "第 {} 行: {}", self.line, self.message)
    }
}
impl std::error::Error for ParseError {}

/// 解析历元时间戳，保留原始字符串，毫秒为规范化表示。
pub fn parse_epoch(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.fZ", "%Y-%m-%dT%H:%M:%SZ", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(nd) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Ok(Utc.from_utc_datetime(&nd).timestamp_millis());
        }
    }
    Err(format!("无法解析历元 '{}'", raw))
}

pub fn parse_input(text: &str) -> Result<ParsedInput, ParseError> {
    let mut out = ParsedInput::default();
    let mut cur_epoch: Option<(String, i64)> = None;
    let mut header_seen = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kw = it.next().unwrap_or("");
        match kw {
            "RNXL" => {
                let v = it.next().ok_or(ParseError { line: lineno, message: "RNXL 缺少版本号".into() })?;
                if v != "1" {
                    return Err(ParseError { line: lineno, message: format!("不支持的 RNXL 版本 '{}'", v) });
                }
                header_seen = true;
            }
            "STATION" => {
                if !header_seen {
                    return Err(ParseError { line: lineno, message: "STATION 之前缺少 RNXL 头".into() });
                }
                out.station = it.next().ok_or(ParseError { line: lineno, message: "STATION 缺少站名".into() })?.to_string();
            }
            "EPOCH" => {
                let raw = it.next().ok_or(ParseError { line: lineno, message: "EPOCH 缺少时间戳".into() })?;
                let ms = parse_epoch(raw).map_err(|m| ParseError { line: lineno, message: m })?;
                cur_epoch = Some((raw.to_string(), ms));
            }
            "CLOCKJUMP" => {
                let raw = it.next().ok_or(ParseError { line: lineno, message: "CLOCKJUMP 缺少历元".into() })?;
                let ms = parse_epoch(raw).map_err(|m| ParseError { line: lineno, message: m })?;
                let raw_off = it.next().ok_or(ParseError { line: lineno, message: "CLOCKJUMP 缺少偏移量(秒)".into() })?;
                let (offset_s, diag) = match raw_off.parse::<f64>() {
                    Ok(v) => (Some(v), None),
                    Err(_) => (None, Some(format!("钟跳偏移 '{}' 无法解析，原样保留", raw_off))),
                };
                out.clock_events.push(RawClockEvent {
                    epoch_raw: raw.to_string(),
                    epoch_ms: ms,
                    raw_offset: raw_off.to_string(),
                    offset_s,
                    diag,
                });
            }
            "OBS" => {
                let (epoch_raw, epoch_ms) = cur_epoch.clone().ok_or(ParseError {
                    line: lineno,
                    message: "OBS 出现在任何 EPOCH 之前".into(),
                })?;
                let sat = it.next().ok_or(ParseError { line: lineno, message: "OBS 缺少卫星号".into() })?.to_string();
                let freq_raw = it.next().ok_or(ParseError { line: lineno, message: "OBS 缺少频率编号".into() })?;
                let freq: u8 = freq_raw.parse().map_err(|_| ParseError {
                    line: lineno,
                    message: format!("频率编号 '{}' 不是 0-255 整数", freq_raw),
                })?;
                let mut saw_any = false;
                for kv in it {
                    let (code, raw_value) = kv.split_once('=').ok_or(ParseError {
                        line: lineno,
                        message: format!("观测项 '{}' 缺少 '='", kv),
                    })?;
                    saw_any = true;
                    let unit = Unit::from_code(code);
                    let (value, diag) = match raw_value.parse::<f64>() {
                        Ok(v) => (Some(v), None),
                        Err(_) => (
                            None,
                            Some(format!("值 '{}' 无法转换为数值，原样保留于 {} 的 {}", raw_value, sat, code)),
                        ),
                    };
                    let diag = if unit == Unit::Unknown && diag.is_none() {
                        Some(format!("观测码 '{}' 单位未知，原样保留", code))
                    } else {
                        diag
                    };
                    out.obs.push(RawObs {
                        epoch_raw: epoch_raw.clone(),
                        epoch_ms,
                        sat: sat.clone(),
                        freq,
                        code: code.to_string(),
                        raw_value: raw_value.to_string(),
                        value,
                        unit,
                        diag,
                    });
                }
                if !saw_any {
                    return Err(ParseError { line: lineno, message: "OBS 行没有任何 码=值 项".into() });
                }
            }
            other => {
                return Err(ParseError { line: lineno, message: format!("未知指令 '{}'", other) });
            }
        }
    }
    if !header_seen {
        return Err(ParseError { line: 0, message: "缺少 RNXL 1 头".into() });
    }
    if out.station.is_empty() {
        return Err(ParseError { line: 0, message: "缺少 STATION 声明".into() });
    }
    Ok(out)
}

/// 内容指纹（FNV-1a，确定性，不依赖随机状态）。
pub fn fingerprint(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a:{:016x}", h)
}
