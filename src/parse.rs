//! 导入层:行式文本格式。原始行一经提交不可覆盖;解析失败保留原文并附诊断。
//!
//! 格式(空白分隔,# 为注释):
//!   STATION <名>
//!   EPOCH <ISO8601>                                   (仅声明,可省略)
//!   SATSTATE <epoch> <sat> <elev_deg> <health>
//!   CLOCKJUMP <epoch> <秒>
//!   OBS <epoch> <sat> F<频> PR=<码>:<值> PH=<码>:<值> DP=<值> LLI=<n> SNR=<值> CH=<n>

use crate::model::*;

#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub obs: Vec<NormObs>,
    pub sat_states: Vec<SatState>,
    pub clock_events: Vec<ClockEvent>,
    pub diagnostics: Vec<serde_json::Value>,
}

fn nv(raw: &str, unit: &str) -> NormValue {
    match raw.parse::<f64>() {
        Ok(v) => NormValue { raw: raw.to_string(), value: Some(v), unit: unit.into(), diag: None },
        Err(_) => NormValue {
            raw: raw.to_string(),
            value: None,
            unit: unit.into(),
            diag: Some(format!("无法转换为数值: '{raw}'")),
        },
    }
}

/// ISO8601 (UTC, 可带毫秒) -> epoch ms。自实现,避免外部时钟依赖。
pub fn parse_epoch_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || !s.ends_with('Z') {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    let mut ms: i64 = 0;
    if let Some(dot) = s.find('.') {
        let frac: String = s[dot + 1..s.len() - 1].chars().take(3).collect();
        let pad = format!("{:0<3}", frac);
        ms = pad.parse().ok()?;
    }
    // days-from-civil (Howard Hinnant)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86_400_000 + h * 3_600_000 + mi * 60_000 + sec * 1000 + ms)
}

pub fn format_epoch_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    // civil-from-days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yy = if mo <= 2 { y + 1 } else { y };
    let h = rem / 3_600_000;
    let mi = (rem % 3_600_000) / 60_000;
    let s = (rem % 60_000) / 1000;
    let msec = rem % 1000;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", yy, mo, d, h, mi, s, msec)
}

pub fn parse_import(text: &str) -> ParseOutcome {
    let mut out = ParseOutcome::default();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = t.split_whitespace().collect();
        let diag = |msg: String| {
            out.diagnostics.push(serde_json::json!({
                "line": line_no, "raw": line, "diag": msg,
            }));
        };
        match parts[0] {
            "STATION" | "EPOCH" | "PARAM" => { /* 声明行,内容已保留在原文中 */ }
            "SATSTATE" => {
                if parts.len() < 5 {
                    diag("SATSTATE 需要 <epoch> <sat> <elev> <health>".into());
                    continue;
                }
                match parse_epoch_ms(parts[1]) {
                    Some(ep) => out.sat_states.push(SatState {
                        sat: parts[2].into(),
                        epoch_ms: ep,
                        elev_deg: parts[3].parse().ok(),
                        health: parts[4].into(),
                    }),
                    None => diag(format!("无法解析历元 '{}'", parts[1])),
                }
            }
            "CLOCKJUMP" => {
                if parts.len() < 3 {
                    diag("CLOCKJUMP 需要 <epoch> <秒>".into());
                    continue;
                }
                match (parse_epoch_ms(parts[1]), parts[2].parse::<f64>()) {
                    (Some(ep), Ok(j)) => out.clock_events.push(ClockEvent { epoch_ms: ep, jump_s: j }),
                    _ => diag("CLOCKJUMP 历元或跳变秒数无法解析".into()),
                }
            }
            "OBS" => {
                if parts.len() < 4 {
                    diag("OBS 需要 <epoch> <sat> F<频> [字段...]".into());
                    continue;
                }
                let ep = match parse_epoch_ms(parts[1]) {
                    Some(e) => e,
                    None => {
                        diag(format!("无法解析历元 '{}'", parts[1]));
                        continue;
                    }
                };
                let freq: u8 = match parts[3].strip_prefix('F').and_then(|f| f.parse().ok()) {
                    Some(f) => f,
                    None => {
                        diag(format!("无法解析频率号 '{}'", parts[3]));
                        continue;
                    }
                };
                let mut o = NormObs {
                    station: "DUAN".into(),
                    sat: parts[2].into(),
                    epoch_ms: ep,
                    freq,
                    pr: None, ph: None, dp: None, lli: 0, snr: None,
                    channel: None, pr_code: None, ph_code: None,
                    source_line: line_no,
                };
                for f in &parts[4..] {
                    if let Some(v) = f.strip_prefix("PR=") {
                        let (code, val) = v.split_once(':').unwrap_or(("C?", v));
                        o.pr_code = Some(code.into());
                        o.pr = Some(nv(val, "m"));
                    } else if let Some(v) = f.strip_prefix("PH=") {
                        let (code, val) = v.split_once(':').unwrap_or(("L?", v));
                        o.ph_code = Some(code.into());
                        o.ph = Some(nv(val, "cyc"));
                    } else if let Some(v) = f.strip_prefix("DP=") {
                        o.dp = Some(nv(v, "Hz"));
                    } else if let Some(v) = f.strip_prefix("LLI=") {
                        o.lli = v.parse().unwrap_or_else(|_| {
                            diag(format!("LLI 无法解析 '{v}',按 0 处理"));
                            0
                        });
                    } else if let Some(v) = f.strip_prefix("SNR=") {
                        o.snr = Some(nv(v, "dB-Hz"));
                    } else if let Some(v) = f.strip_prefix("CH=") {
                        match v.parse::<u32>() {
                            Ok(c) => o.channel = Some(c),
                            Err(_) => diag(format!("频率编号(通道)无法解析 '{v}'")),
                        }
                    } else {
                        diag(format!("未知字段 '{f}',原样保留于输入"));
                    }
                }
                out.obs.push(o);
            }
            other => diag(format!("未知记录类型 '{other}',原样保留于输入")),
        }
    }
    out
}
