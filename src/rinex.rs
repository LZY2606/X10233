//! 导入层：RINEX-3 风格观测文本（可复制小样例）。
//! 原始字段一律按历元原样保存；解析失败给出带行号的明确诊断；
//! 无法转换的值留在原位置并附诊断，绝不为了画图丢弃。
use crate::model::*;
use crate::units::{parse_obs_field, Quantity, Unit};

pub struct Parsed {
    pub station: String,
    pub epochs: Vec<RawEpoch>,
    pub clock_events: Vec<ClockJumpEvent>,
    pub sat_states: Vec<SatState>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

fn diag(line_no: usize, line: &str, msg: impl Into<String>) -> ParseDiagnostic {
    ParseDiagnostic { line_no, line: line.to_string(), message: msg.into() }
}

/// 民用日期 -> 自 2024-01-01 的天数（Howard Hinnant 算法，GPS 连续时，不计闰秒）。
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe;
    days - days_from_civil_epoch()
}
fn days_from_civil_epoch() -> i64 {
    // 2024-01-01 相对 1970-01-01 的天数
    19723
}

pub fn civil_to_gps_sec(y: i64, mo: i64, d: i64, h: i64, mi: i64, sec: f64) -> Option<i64> {
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || !(0.0..61.0).contains(&sec) {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec.round() as i64)
}

fn parse_epoch_time(tokens: &[&str], line_no: usize, line: &str) -> (EpochTime, Option<ParseDiagnostic>) {
    let raw = tokens.join(" ");
    if tokens.len() < 6 {
        return (
            EpochTime { raw, gps_sec: None, diagnostic: Some("历元时间字段不足 6 个".into()) },
            Some(diag(line_no, line, "历元时间字段不足 6 个")),
        );
    }
    let nums: Vec<Result<f64, _>> = tokens[..6].iter().map(|t| t.parse::<f64>()).collect();
    if nums.iter().any(|n| n.is_err()) {
        return (
            EpochTime { raw, gps_sec: None, diagnostic: Some("历元时间含非数字字段".into()) },
            Some(diag(line_no, line, "历元时间含非数字字段")),
        );
    }
    let v: Vec<f64> = nums.into_iter().map(|n| n.unwrap()).collect();
    match civil_to_gps_sec(v[0] as i64, v[1] as i64, v[2] as i64, v[3] as i64, v[4] as i64, v[5]) {
        Some(s) => (EpochTime { raw, gps_sec: Some(s), diagnostic: None }, None),
        None => (
            EpochTime { raw, gps_sec: None, diagnostic: Some("历元时间超出合法范围".into()) },
            Some(diag(line_no, line, "历元时间超出合法范围")),
        ),
    }
}

/// 解析完整文本。结构性错误（缺头部、无历元）返回 Err；字段级问题进 diagnostics。
pub fn parse(text: &str) -> Result<Parsed, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut diagnostics = Vec::new();
    let mut obs_types: Vec<(char, Vec<String>)> = Vec::new();
    let mut station = "UNKNOWN".to_string();
    let mut header_end = None;

    for (i, line) in lines.iter().enumerate() {
        let ln = i + 1;
        if line.contains("RINEX VERSION / TYPE") {
            let v: f64 = line
                .get(..20)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0.0);
            if v < 3.0 {
                diagnostics.push(diag(ln, line, "仅支持 RINEX 3 风格观测文件（版本 < 3）"));
            }
        } else if line.contains("MARKER NAME") {
            let name = line.get(..60).unwrap_or(line).trim();
            if !name.is_empty() {
                station = name.split_whitespace().next().unwrap_or("UNKNOWN").to_string();
            }
        } else if line.contains("SYS / # / OBS TYPES") {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 3 {
                diagnostics.push(diag(ln, line, "SYS / # / OBS TYPES 行字段不足"));
                continue;
            }
            let sys = toks[0].chars().next().unwrap_or('G');
            let n: usize = toks[1].parse().unwrap_or(0);
            let codes: Vec<String> = toks[2..]
                .iter()
                .take(n)
                .filter(|t| !t.contains('/'))
                .map(|s| s.to_string())
                .collect();
            if codes.len() != n {
                diagnostics.push(diag(
                    ln,
                    line,
                    format!("声明 {n} 个观测码但只读到 {} 个（多行续行不支持，请写在一行）", codes.len()),
                ));
            }
            for c in &codes {
                if let Err(e) = ObsCode::parse(c) {
                    diagnostics.push(diag(ln, line, e));
                }
            }
            obs_types.push((sys, codes));
        } else if line.contains("END OF HEADER") {
            header_end = Some(i);
            break;
        }
    }
    let header_end = match header_end {
        Some(h) => h,
        None => return Err("缺少 END OF HEADER：不是可识别的 RINEX 观测文本".to_string()),
    };
    if obs_types.is_empty() {
        return Err("头部缺少 SYS / # / OBS TYPES：无法解释观测字段".to_string());
    }

    let mut epochs: Vec<RawEpoch> = Vec::new();
    let mut clock_events = Vec::new();
    let mut sat_states = Vec::new();
    let mut i = header_end + 1;
    while i < lines.len() {
        let line = lines[i];
        let ln = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# CLKJMP") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if toks.len() < 8 {
                diagnostics.push(diag(ln, line, "钟跳事件字段不足：# CLKJMP <sys> y m d h m s step_s"));
            } else {
                let (t, d) = parse_epoch_time(&toks[1..7], ln, line);
                if let Some(dd) = d {
                    diagnostics.push(dd);
                }
                let step = match toks[7].parse::<f64>() {
                    Ok(v) => Quantity::ok(toks[7], v, Unit::Seconds),
                    Err(_) => Quantity::bad(toks[7], Unit::Seconds, "钟跳步长无法解析"),
                };
                clock_events.push(ClockJumpEvent { at: t, step });
            }
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# SATSTATE") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if toks.len() < 8 {
                diagnostics.push(diag(ln, line, "卫星状态字段不足：# SATSTATE <sat> y m d h m s k=v ..."));
            } else {
                let (t, d) = parse_epoch_time(&toks[1..7], ln, line);
                if let Some(dd) = d {
                    diagnostics.push(dd);
                }
                let fields: Vec<(String, String)> = toks[7..]
                    .iter()
                    .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                    .collect();
                sat_states.push(SatState { sat: toks[0].to_string(), at: t, raw_fields: fields });
            }
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if toks.len() < 8 {
                diagnostics.push(diag(ln, line, "历元行字段不足：> y m d h m s flag nsat"));
                i += 1;
                continue;
            }
            let (time, d) = parse_epoch_time(&toks[..6], ln, line);
            if let Some(dd) = d {
                diagnostics.push(dd);
            }
            let nsat: usize = toks[7].parse().unwrap_or(0);
            let rcvr_clock_raw = toks.get(8).map(|s| s.to_string());
            let mut sats = Vec::new();
            for _ in 0..nsat {
                i += 1;
                if i >= lines.len() {
                    diagnostics.push(diag(ln, line, format!("历元声明 {nsat} 颗卫星但文件提前结束")));
                    break;
                }
                let sline = lines[i];
                let sln = i + 1;
                if sline.len() < 3 {
                    diagnostics.push(diag(sln, sline, "卫星行过短，缺少卫星编号"));
                    continue;
                }
                let sat: String = sline.chars().take(3).collect();
                let sys = sat.chars().next().unwrap_or('G');
                let codes = obs_types
                    .iter()
                    .find(|(s, _)| *s == sys)
                    .map(|(_, c)| c.clone())
                    .unwrap_or_else(|| obs_types[0].1.clone());
                let mut obs = Vec::new();
                let body = &sline[3..];
                for (k, code_raw) in codes.iter().enumerate() {
                    let start = k * 16;
                    if start >= body.len() {
                        break; // 频率缺失：该历元没有此观测码，属合法情形
                    }
                    let field = body.get(start..(start + 16).min(body.len())).unwrap_or("");
                    let value_raw: String = field.chars().take(14).collect();
                    let lli = field.chars().nth(14).filter(|c| !c.is_whitespace());
                    let ssi = field.chars().nth(15).filter(|c| !c.is_whitespace());
                    let code = match ObsCode::parse(code_raw) {
                        Ok(c) => c,
                        Err(e) => {
                            diagnostics.push(diag(sln, sline, e));
                            continue;
                        }
                    };
                    let unit = match code.kind {
                        ObsKind::C => Unit::Meters,
                        ObsKind::L => Unit::Cycles,
                        ObsKind::D => Unit::Hz,
                        ObsKind::S => Unit::DbHz,
                    };
                    let q = parse_obs_field(&value_raw, unit);
                    if let Some(dg) = &q.diagnostic {
                        diagnostics.push(diag(sln, sline, format!("{} {}：{}", sat, code_raw, dg)));
                    }
                    obs.push(Observable { code, q, lli, ssi });
                }
                let _ = sat;
                sats.push(SatEpoch { sat: sat.trim().to_string(), obs });
            }
            epochs.push(RawEpoch { time, sats, rcvr_clock_raw });
            i += 1;
            continue;
        }
        diagnostics.push(diag(ln, line, "无法识别的行（既不是历元、卫星、事件，也不是注释）"));
        i += 1;
    }
    if epochs.is_empty() {
        return Err("头部之后没有任何观测历元".to_string());
    }
    Ok(Parsed { station, epochs, clock_events, sat_states, diagnostics })
}
