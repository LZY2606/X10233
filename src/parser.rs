//! 可复制小型样例格式解析器（"mini-RINEX"）。
//!
//! 语法（每行一个记录，`#` 起始为注释）：
//! ```text
//! STATION <site>
//! FREQ <编号> <观测码> <频率Hz>
//! @ <ISO8601 时间戳>                 # 开始一个历元（允许不等间隔）
//! SAT <卫星> EL <deg> AZ <deg> HEALTH <OK|...>
//! <卫星> <频点编号> C <伪距[m|km]> L <载波[cyc]> D <Doppler[Hz]> S <SNR[dBHz]> [LLI <n>]
//! CLOCK between=<历元下标> jump=<秒[s|ms|us]> source=<文本>
//! ```
//! * 某卫星某历元缺某频点行 => 频率缺失（正常缺测，不是格式错误）。
//! * 数值不可转换 => 留在原位置并附诊断，不阻断导入。
//! * 结构性错误（未知频点编号、重复字段、时间倒序等）=> 带行号的 [`ParseError`]，整次导入拒绝。

use crate::model::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(line: usize, msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError { line, message: msg.into() })
}

/// 解析数值并按单位后缀换算到规范单位。无法转换时返回带诊断的 RawValue。
fn parse_raw(token: &str, canonical: &str, conversions: &[(&str, f64)], line: usize) -> RawValue {
    let unit_hint = canonical.to_string();
    // 取末尾单位字母（最多 3 个），其余作为数值。
    let split_at = token
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+' || *c == 'e' || *c == 'E')
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(token.len());
    let (num_part, suffix) = token.split_at(split_at);
    let factor = if suffix.is_empty() {
        1.0
    } else if let Some((_, f)) = conversions.iter().find(|(u, _)| *u == suffix) {
        *f
    } else {
        return RawValue {
            raw_repr: token.to_string(),
            unit_hint,
            normalized: None,
            diagnostic: Some(format!("line {line}: unknown unit suffix '{suffix}' (canonical unit '{canonical}')")),
        };
    };
    match num_part.parse::<f64>() {
        Ok(v) if v.is_finite() => RawValue {
            raw_repr: token.to_string(),
            unit_hint: canonical.to_string(),
            normalized: Some(v * factor),
            diagnostic: None,
        },
        _ => RawValue {
            raw_repr: token.to_string(),
            unit_hint,
            normalized: None,
            diagnostic: Some(format!("line {line}: cannot parse '{num_part}' as {canonical}")),
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub fn parse_import(
    text: &str,
    id: String,
    version: u32,
    parent_id: Option<String>,
    revision_note: Option<String>,
    committed_at: String,
) -> Result<RawImport, ParseError> {
    let mut station = String::new();
    let mut freq_table: BTreeMap<u8, FreqSpec> = BTreeMap::new();
    let mut epoch_times: Vec<String> = vec![];
    let mut dt_seconds: Vec<Option<f64>> = vec![];
    let mut observations: Vec<BTreeMap<String, BTreeMap<u8, RawObservation>>> = vec![];
    let mut sat_states: Vec<BTreeMap<String, SatState>> = vec![];
    let mut clock_events: Vec<ClockEvent> = vec![];
    let mut diagnostics: Vec<String> = vec![];
    let mut cur_epoch: Option<usize> = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "STATION" => {
                if parts.len() != 2 {
                    return err(lineno, "STATION expects exactly one site id");
                }
                station = parts[1].to_string();
            }
            "FREQ" => {
                if parts.len() != 4 {
                    return err(lineno, "FREQ expects: FREQ <number> <code> <frequency_hz>");
                }
                let num: u8 = parts[1].parse().map_err(|_| ParseError {
                    line: lineno,
                    message: format!("frequency number '{}' not an integer", parts[1]),
                })?;
                let hz: f64 = parts[3].parse().map_err(|_| ParseError {
                    line: lineno,
                    message: format!("frequency '{}' not a number", parts[3]),
                })?;
                if !(hz.is_finite() && hz > 0.0) {
                    return err(lineno, "frequency must be a positive finite number");
                }
                if freq_table.insert(num, FreqSpec { code: parts[2].to_string(), frequency_hz: hz }).is_some() {
                    return err(lineno, format!("duplicate FREQ number {num}"));
                }
            }
            "@" => {
                if parts.len() != 2 {
                    return err(lineno, "epoch header expects '@' <ISO8601 timestamp>");
                }
                if let Some(prev) = epoch_times.last() {
                    if parts[1] <= prev.as_str() {
                        return err(lineno, "epoch timestamps must be strictly increasing");
                    }
                }
                let dt = if epoch_times.is_empty() {
                    None
                } else {
                    Some(seconds_between(epoch_times.last().unwrap(), parts[1]).ok_or_else(|| ParseError {
                        line: lineno,
                        message: format!("timestamp '{}' is not ISO8601 (YYYY-MM-DDTHH:MM:SS)", parts[1]),
                    })?)
                };
                epoch_times.push(parts[1].to_string());
                dt_seconds.push(dt);
                observations.push(BTreeMap::new());
                sat_states.push(BTreeMap::new());
                cur_epoch = Some(epoch_times.len() - 1);
            }
            "SAT" => {
                let e = match cur_epoch {
                    Some(e) => e,
                    None => return err(lineno, "SAT line outside an epoch (missing '@')"),
                };
                if parts.len() < 2 {
                    return err(lineno, "SAT expects satellite id");
                }
                let sat = parts[1].to_string();
                let mut state = SatState::default();
                let mut i = 2;
                while i < parts.len() {
                    let set: &mut Option<Measure> = match parts[i] {
                        "EL" => &mut state.elevation,
                        "AZ" => &mut state.azimuth,
                        "HEALTH" => {
                            if i + 1 >= parts.len() {
                                return err(lineno, "HEALTH requires a value");
                            }
                            state.health = Some(parts[i + 1].to_string());
                            i += 2;
                            continue;
                        }
                        other => return err(lineno, format!("unknown SAT field '{other}'")),
                    };
                    if i + 1 >= parts.len() {
                        return err(lineno, format!("{} requires a value", parts[i]));
                    }
                    let v: f64 = parts[i + 1].parse().map_err(|_| ParseError {
                        line: lineno,
                        message: format!("{} value '{}' not a number", parts[i], parts[i + 1]),
                    })?;
                    *set = Some(Measure::new(v, "deg"));
                    i += 2;
                }
                sat_states[e].insert(sat, state);
            }
            "CLOCK" => {
                let mut between: Option<usize> = None;
                let mut jump: Option<f64> = None;
                let mut source = String::from("explicit");
                let mut i = 1;
                while i < parts.len() {
                    let (key, val) = (parts[i], parts.get(i + 1));
                    match key {
                        "between" => {
                            let v = val.and_then(|s| s.parse::<usize>().ok()).ok_or_else(|| ParseError {
                                line: lineno,
                                message: "CLOCK between requires integer".into(),
                            })?;
                            between = Some(v);
                        }
                        "jump" => {
                            let tok = val.ok_or_else(|| ParseError { line: lineno, message: "CLOCK jump requires value".into() })?;
                            let rv = parse_raw(tok, "s", &[("ms", 1e-3), ("us", 1e-6)], lineno);
                            match rv.normalized {
                                Some(v) => jump = Some(v),
                                None => {
                                    diagnostics.push(rv.diagnostic.clone().unwrap());
                                    jump = Some(f64::NAN);
                                }
                            }
                        }
                        "source" => source = val.unwrap_or("explicit").to_string(),
                        other => return err(lineno, format!("unknown CLOCK field '{other}'")),
                    }
                    i += 2;
                }
                let between = between.ok_or_else(|| ParseError { line: lineno, message: "CLOCK missing between=".into() })?;
                let j = jump.ok_or_else(|| ParseError { line: lineno, message: "CLOCK missing jump=".into() })?;
                if between == 0 || between > epoch_times.len() {
                    return err(lineno, format!("CLOCK between={between} out of range for {} epochs", epoch_times.len()));
                }
                clock_events.push(ClockEvent { between_epoch: between, jump: Measure::new(j, "s"), source });
            }
            first => {
                // 观测行：<sat> <freq> C .. L .. D .. S .. [LLI n]
                let e = match cur_epoch {
                    Some(e) => e,
                    None => return err(lineno, "observation line outside an epoch (missing '@')"),
                };
                if parts.len() < 3 {
                    return err(lineno, "observation line expects: <sat> <freq> C <v> [L <v>] ...");
                }
                let sat = first.to_string();
                let freq: u8 = parts[1].parse().map_err(|_| ParseError {
                    line: lineno,
                    message: format!("frequency number '{}' not an integer", parts[1]),
                })?;
                if !freq_table.contains_key(&freq) {
                    return err(lineno, format!("frequency {freq} used without a FREQ header declaration"));
                }
                if observations[e].get(&sat).and_then(|m| m.get(&freq)).is_some() {
                    return err(lineno, format!("duplicate observation for {sat} frequency {freq} in epoch {}", e + 1));
                }
                let mut obs = RawObservation::default();
                let mut i = 2;
                while i < parts.len() {
                    let tag = parts[i];
                    match tag {
                        "LLI" => {
                            let v = parts.get(i + 1).and_then(|s| s.parse::<u8>().ok()).ok_or_else(|| ParseError {
                                line: lineno,
                                message: "LLI requires integer flag".into(),
                            })?;
                            obs.lli = Some(v);
                            i += 2;
                        }
                        field @ ("C" | "L" | "D" | "S") => {
                            let tok = parts.get(i + 1).ok_or_else(|| ParseError {
                                line: lineno,
                                message: format!("{field} requires a value"),
                            })?;
                            let rv = match field {
                                "C" => parse_raw(tok, "m", &[("km", 1000.0)], lineno),
                                "L" => parse_raw(tok, "cyc", &[], lineno),
                                "D" => parse_raw(tok, "Hz", &[("kHz", 1000.0)], lineno),
                                "S" => parse_raw(tok, "dBHz", &[], lineno),
                                _ => unreachable!(),
                            };
                            if let Some(d) = &rv.diagnostic {
                                diagnostics.push(d.clone());
                            }
                            let slot = match field {
                                "C" => &mut obs.pseudo_range,
                                "L" => &mut obs.carrier_phase,
                                "D" => &mut obs.doppler,
                                "S" => &mut obs.snr,
                                _ => unreachable!(),
                            };
                            *slot = Some(rv);
                            i += 2;
                        }
                        other => return err(lineno, format!("unknown observation field '{other}'")),
                    }
                }
                observations[e].entry(sat).or_default().insert(freq, obs);
            }
        }
    }

    if station.is_empty() {
        return err(0, "missing STATION header");
    }
    if epoch_times.is_empty() {
        return err(0, "import contains no epochs");
    }

    Ok(RawImport {
        id,
        version,
        parent_id,
        revision_note,
        committed_at,
        station,
        epoch_times,
        dt_seconds,
        observations,
        sat_states,
        clock_events,
        diagnostics,
        freq_table,
    })
}

/// 仅支持紧凑 ISO8601 `YYYY-MM-DDTHH:MM:SS`（可含小数秒），返回秒差。
fn seconds_between(a: &str, b: &str) -> Option<f64> {
    let parse = |s: &str| -> Option<f64> {
        let (date, time) = s.split_once('T')?;
        let d: Vec<u32> = date.split('-').map(|x| x.parse().ok()).collect::<Option<Vec<_>>>()?;
        let t: Vec<f64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<Vec<_>>>()?;
        if d.len() != 3 || t.len() != 3 {
            return None;
        }
        let days = civil_to_days(d[0] as i64, d[1] as u32, d[2] as u32);
        Some(days as f64 * 86_400.0 + t[0] * 3600.0 + t[1] * 60.0 + t[2])
    };
    Some(parse(b)? - parse(a)?)
}

/// Howard Hinnant 的公历换算，保证日期差正确。
fn civil_to_days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe as i64 * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
