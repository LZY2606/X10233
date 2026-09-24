//! RINEX 3 观测文件解析（导入层）。
//! 伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号和观测码均按历元原样保存；
//! 原始文本不可覆盖，解析失败的值保留在原位置并附诊断。

use crate::domain::*;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RinexError {
    #[error("missing RINEX VERSION / TYPE header")]
    MissingVersion,
    #[error("unsupported RINEX version: {0}")]
    UnsupportedVersion(String),
    #[error("missing END OF HEADER")]
    MissingEndOfHeader,
    #[error("bad epoch line at record {0}: {1}")]
    BadEpoch(usize, String),
    #[error("bad observation line at record {0}: {1}")]
    BadObsLine(usize, String),
    #[error("empty dataset: no epochs")]
    Empty,
}

/// 解析 14 字符宽度的观测字段（RINEX 3 定宽），保留原始文本。
fn parse_field(raw: &str, unit: &str) -> ObsValue {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ObsValue { raw: raw.to_string(), value: None, unit: unit.into(), lli: None, ssi: None, diag: None };
    }
    // 取前 14 字符为数值部分，后两位为 LLI/SSI。
    let num_part: String = trimmed.chars().take(14).collect();
    let value = num_part.trim().parse::<f64>().ok();
    let chars: Vec<char> = trimmed.chars().collect();
    let lli = chars.get(14).and_then(|c| c.to_digit(10)).map(|d| d as u8);
    let ssi = chars.get(15).and_then(|c| c.to_digit(10)).map(|d| d as u8);
    let diag = if value.is_none() { Some("unparseable: kept raw in place".to_string()) } else { None };
    ObsValue { raw: raw.to_string(), value, unit: unit.into(), lli, ssi, diag }
}

fn unit_for(obs_type: char) -> &'static str {
    match obs_type {
        'C' => "m",
        'L' => "cycles",
        'D' => "Hz",
        'S' => "dB-Hz",
        _ => "1",
    }
}

/// 历元行："> yyyy mm dd hh mm ss.sssssss  flag nsat [clock_offset]"
fn parse_epoch_line(line: &str, rec: usize) -> Result<(String, f64, u8, usize, Option<f64>), RinexError> {
    let toks: Vec<&str> = line[1..].split_whitespace().collect();
    if toks.len() < 8 {
        return Err(RinexError::BadEpoch(rec, format!("too few fields: '{}'", line)));
    }
    let y: i64 = toks[0].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let mo: i64 = toks[1].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let d: i64 = toks[2].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let h: i64 = toks[3].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let mi: i64 = toks[4].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let s: f64 = toks[5].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let flag: u8 = toks[6].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let nsat: usize = toks[7].parse().map_err(|_| RinexError::BadEpoch(rec, line.into()))?;
    let clock_offset = if toks.len() > 8 { toks[8].parse::<f64>().ok() } else { None };
    let epoch_text = format!("{:04} {:02} {:02} {:02} {:02} {:011.7}", y, mo, d, h, mi, s);
    // 民用历 -> 相对 2020-01-01 的秒（确定性，不考虑闰秒）。
    let mut days: i64 = 0;
    for yy in 2020..y {
        days += if (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0 { 366 } else { 365 };
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(mo - 1) as usize {
        days += mdays[m] as i64;
    }
    days += d - 1;
    let secs = days as f64 * 86400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + s;
    Ok((epoch_text, secs, flag, nsat, clock_offset))
}

pub fn parse_rinex(text: &str) -> Result<Dataset, RinexError> {
    let mut lines = text.lines().enumerate().peekable();
    let mut station = String::from("UNKNOWN");
    let mut marker = String::new();
    let mut rinex_version = String::new();
    let mut time_system = String::from("GPS");
    let mut sys_obs_types: Vec<(char, Vec<String>)> = Vec::new();
    let mut sat_states = Vec::new();
    let mut clock_jumps = Vec::new();
    let mut diagnostics = Vec::new();

    // ---- 头 ----
    let first = lines.next().ok_or(RinexError::MissingVersion)?;
    let first_line = first.1;
    if !first_line.contains("RINEX VERSION / TYPE") {
        return Err(RinexError::MissingVersion);
    }
    rinex_version = first_line.get(..9).unwrap_or("").trim().to_string();
    if !rinex_version.starts_with('3') && !rinex_version.starts_with('4') {
        return Err(RinexError::UnsupportedVersion(rinex_version.clone()));
    }
    let mut saw_end = false;
    for (i, line) in lines.by_ref() {
        let label = line.get(60..).unwrap_or("").trim();
        match label {
            "MARKER NAME" => {
                station = line.get(..60).unwrap_or("").trim().to_string();
                marker = station.clone();
            }
            "TIME OF FIRST OBS" => {
                if let Some(ts) = line.get(48..60).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    time_system = ts.to_string();
                }
            }
            "SYS / # / OBS TYPES" => {
                let sys = line.chars().next().unwrap_or('G');
                let n: usize = line.get(3..6).unwrap_or("").trim().parse().unwrap_or(0);
                let mut codes = Vec::new();
                let mut rest = line.get(7..60).unwrap_or("").to_string();
                // 续行
                let mut remaining = n;
                loop {
                    for chunk in rest.as_str().split_whitespace() {
                        if remaining == 0 { break; }
                        codes.push(chunk.to_string());
                        remaining -= 1;
                    }
                    if remaining == 0 { break; }
                    match lines.next() {
                        Some((_, cont)) => rest = cont.get(7..60).unwrap_or("").to_string(),
                        None => return Err(RinexError::MissingEndOfHeader),
                    }
                }
                sys_obs_types.push((sys, codes));
            }
            "CLK_JUMP" | "CLOCK JUMP" => {
                // 自定义头行（案例包使用）：epoch_seconds jump_ms
                let body = line.get(..60).unwrap_or("");
                let toks: Vec<&str> = body.split_whitespace().collect();
                if toks.len() >= 2 {
                    if let (Ok(es), Ok(j)) = (toks[0].parse::<f64>(), toks[1].parse::<f64>()) {
                        clock_jumps.push(ClockJumpEvent { epoch_seconds: es, jump_ms: j, source: "rinex-header".into() });
                    }
                }
            }
            "SAT_STATE" => {
                // 自定义头行：sat epoch_seconds x_km y_km z_km healthy
                let body = line.get(..60).unwrap_or("");
                let toks: Vec<&str> = body.split_whitespace().collect();
                if toks.len() >= 6 {
                    let parsed = (
                        toks[1].parse::<f64>(), toks[2].parse::<f64>(),
                        toks[3].parse::<f64>(), toks[4].parse::<f64>(),
                    );
                    if let (Ok(es), Ok(x), Ok(y), Ok(z)) = parsed {
                        sat_states.push(SatState {
                            sat: toks[0].to_string(), epoch_seconds: es,
                            x_km: x, y_km: y, z_km: z,
                            healthy: toks[5] == "1", frame: "ITRF2020".into(),
                        });
                    }
                }
            }
            "END OF HEADER" => { saw_end = true; break; }
            _ => {}
        }
        let _ = i;
    }
    if !saw_end {
        return Err(RinexError::MissingEndOfHeader);
    }

    // ---- 数据体 ----
    let mut epochs = Vec::new();
    let mut rec = 0usize;
    while let Some((_, line)) = lines.next() {
        let line = line.trim_end();
        if line.is_empty() { continue; }
        if !line.starts_with('>') {
            return Err(RinexError::BadEpoch(rec, format!("expected '>' epoch line, got '{}'", line)));
        }
        let (epoch_text, epoch_secs, flag, nsat, clock_offset) = parse_epoch_line(line, rec)?;
        let mut sats = Vec::new();
        for _ in 0..nsat {
            let (li, obs_line) = lines.next().ok_or(RinexError::BadObsLine(rec, "unexpected EOF".into()))?;
            let obs_line = obs_line.trim_end();
            if obs_line.len() < 3 {
                return Err(RinexError::BadObsLine(li, obs_line.to_string()));
            }
            let sat = obs_line[..3].to_string();
            let sys = sat.chars().next().unwrap_or('G');
            let codes = sys_obs_types.iter().find(|(s, _)| *s == sys).map(|(_, c)| c.clone()).unwrap_or_default();
            let mut records = Vec::new();
            let body = &obs_line[3..];
            for (ci, code) in codes.iter().enumerate() {
                let start = ci * 16;
                if start >= body.len() { break; }
                let end = (start + 16).min(body.len());
                let raw = &body[start..end];
                let meta = parse_code(code);
                let unit = meta.map(|m| unit_for(m.obs_type)).unwrap_or("1");
                let mut obs = parse_field(raw, unit);
                // LLI 误报与真实周跳都按原样保留在 LLI 位上。
                if let Some(d) = &obs.diag {
                    diagnostics.push(format!("epoch {} sat {} code {}: {}", epoch_text, sat, code, d));
                    obs.diag = None; // 诊断归集到数据集级，值本身保留原样
                }
                records.push(ObsRecord { code: code.clone(), obs });
            }
            sats.push(SatObs { sat, records });
        }
        if flag == 1 {
            // flag=1 表示历元间出现接收机钟跳（整机，区别于单星周跳）。
            clock_jumps.push(ClockJumpEvent { epoch_seconds: epoch_secs, jump_ms: clock_offset.unwrap_or(0.0) * 1000.0, source: "receiver-flag".into() });
        }
        epochs.push(EpochObs { epoch: epoch_text, epoch_seconds: epoch_secs, clock_offset, sats });
        rec += 1;
    }
    if epochs.is_empty() {
        return Err(RinexError::Empty);
    }
    Ok(Dataset { station, marker, rinex_version, time_system, epochs, sat_states, clock_jumps, diagnostics })
}
