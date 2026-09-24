use crate::ids::{fmt_tow, stable_id};
use crate::model::{ClockJump, JumpOrigin, JumpStatus, ObsPoint};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct RinexError {
    pub line: usize,
    pub message: String,
    pub snippet: String,
}

pub struct ParsedRinex {
    pub station: String,
    pub obs: Vec<ObsPoint>,
    pub diagnostics: Vec<String>,
}

/// 民用日期 -> GPS 周/周内秒（GPST 基准）。
pub fn gps_week_tow(y: i32, m: u32, d: u32, h: u32, mi: u32, s: f64) -> (u32, f64) {
    let a = (14 - m as i32) / 12;
    let y2 = y + 4800 - a;
    let m2 = m as i32 + 12 * a - 3;
    let jdn = d as i32 + (153 * m2 + 2) / 5 + 365 * y2 + y2 / 4 - y2 / 100 + y2 / 400 - 32045;
    let jdn_gps_epoch = 2_444_245; // 1980-01-06
    let days = jdn - jdn_gps_epoch;
    let week = (days.div_euclid(7)) as u32;
    let tow = (days.rem_euclid(7)) as f64 * 86400.0
        + h as f64 * 3600.0
        + mi as f64 * 60.0
        + s;
    (week, tow)
}

fn err(line: usize, message: &str, snippet: &str) -> RinexError {
    RinexError {
        line,
        message: message.to_string(),
        snippet: snippet.chars().take(120).collect(),
    }
}

/// 解析 RINEX 3 观测文件子集。结构性错误整体拒绝；
/// 单值不可转换时保留原始行并附诊断。
pub fn parse_rinex(text: &str, input_id: &str) -> Result<ParsedRinex, RinexError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut station = String::from("UNKNOWN");
    let mut obs_types: Vec<String> = Vec::new();
    let mut header_end = None;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let label = line.get(60..).unwrap_or("").trim();
        if label == "END OF HEADER" {
            header_end = Some(i);
            break;
        }
        match label {
            "MARKER NAME" => {
                let name = line.get(0..60).unwrap_or(line).trim();
                if !name.is_empty() {
                    station = name.to_string();
                }
            }
            "SYS / # / OBS TYPES" => {
                let count: usize = line
                    .get(3..6)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                let body = line.get(7..60).unwrap_or("");
                for k in 0..count {
                    if let Some(c) = body.get(k * 4..k * 4 + 3) {
                        let c = c.trim();
                        if !c.is_empty() {
                            obs_types.push(c.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    let header_end = match header_end {
        Some(h) => h,
        None => return Err(err(lines.len(), "缺少 END OF HEADER 记录", lines.last().copied().unwrap_or(""))),
    };
    if obs_types.is_empty() {
        return Err(err(header_end + 1, "缺少 SYS / # / OBS TYPES 记录", ""));
    }

    let mut obs: Vec<ObsPoint> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut idx = header_end + 1;
    while idx < lines.len() {
        let line = lines[idx];
        if line.trim().is_empty() {
            idx += 1;
            continue;
        }
        if !line.starts_with('>') {
            return Err(err(idx + 1, "期望历元行（以 '>' 开头）", line));
        }
        let parts: Vec<&str> = line[1..].split_whitespace().collect();
        if parts.len() < 8 {
            return Err(err(idx + 1, "历元行字段不足", line));
        }
        let y: i32 = parts[0].parse().map_err(|_| err(idx + 1, "历元年份不可解析", line))?;
        let mo: u32 = parts[1].parse().map_err(|_| err(idx + 1, "历元月份不可解析", line))?;
        let d: u32 = parts[2].parse().map_err(|_| err(idx + 1, "历元日不可解析", line))?;
        let h: u32 = parts[3].parse().map_err(|_| err(idx + 1, "历元时不可解析", line))?;
        let mi: u32 = parts[4].parse().map_err(|_| err(idx + 1, "历元分不可解析", line))?;
        let sec: f64 = parts[5].parse().map_err(|_| err(idx + 1, "历元秒不可解析", line))?;
        let flag: i32 = parts[6].parse().map_err(|_| err(idx + 1, "历元标志不可解析", line))?;
        if !(0..=1).contains(&flag) {
            return Err(err(idx + 1, "观测历元标志异常（仅支持 0/1）", line));
        }
        let nsv: usize = parts[7].parse().map_err(|_| err(idx + 1, "卫星数不可解析", line))?;
        let (week, tow) = gps_week_tow(y, mo, d, h, mi, sec);
        for _ in 0..nsv {
            idx += 1;
            let ol = match lines.get(idx) {
                Some(l) => *l,
                None => return Err(err(idx + 1, "观测行缺失（文件提前结束）", "")),
            };
            let need = 3 + 16 * obs_types.len();
            if ol.len() < need {
                return Err(err(idx + 1, "观测行长度不足（结构性错误）", ol));
            }
            let sat = ol[0..3].trim().to_string();
            if sat.is_empty() {
                return Err(err(idx + 1, "卫星编号缺失", ol));
            }
            // 按频率聚合：观测码第 2 位为频率编号。
            struct Acc {
                code_l: Option<String>,
                code_p: Option<String>,
                code_d: Option<String>,
                code_s: Option<String>,
                l: Option<f64>,
                p: Option<f64>,
                d: Option<f64>,
                s: Option<f64>,
                lli: u8,
            }
            let mut by_freq: BTreeMap<u8, Acc> = BTreeMap::new();
            for (t, code) in obs_types.iter().enumerate() {
                let field = &ol[3 + t * 16..3 + t * 16 + 16];
                let val_s = field[0..14].trim();
                let lli_ch = field.chars().nth(14).unwrap_or(' ');
                let kind = code.chars().next().unwrap_or(' ');
                let freq: u8 = match code.chars().nth(1).and_then(|c| c.to_digit(10)) {
                    Some(f) => f as u8,
                    None => {
                        diagnostics.push(format!(
                            "行 {}: 观测码 {} 频率编号不可解析，已跳过",
                            idx + 1,
                            code
                        ));
                        continue;
                    }
                };
                let acc = by_freq.entry(freq).or_insert(Acc {
                    code_l: None,
                    code_p: None,
                    code_d: None,
                    code_s: None,
                    l: None,
                    p: None,
                    d: None,
                    s: None,
                    lli: 0,
                });
                let value: Option<f64> = if val_s.is_empty() {
                    None
                } else {
                    match val_s.parse::<f64>() {
                        Ok(v) => Some(v),
                        Err(_) => {
                            diagnostics.push(format!(
                                "行 {}: 值 '{}' 不可转换（{} {}），保留原始表示",
                                idx + 1,
                                val_s,
                                sat,
                                code
                            ));
                            None
                        }
                    }
                };
                match kind {
                    'L' => {
                        acc.code_l = Some(code.clone());
                        acc.l = value;
                        if lli_ch.is_ascii_digit() {
                            acc.lli = lli_ch.to_digit(10).unwrap_or(0) as u8;
                        }
                    }
                    'C' => {
                        acc.code_p = Some(code.clone());
                        acc.p = value;
                    }
                    'D' => {
                        acc.code_d = Some(code.clone());
                        acc.d = value;
                    }
                    'S' => {
                        acc.code_s = Some(code.clone());
                        acc.s = value;
                    }
                    _ => {
                        diagnostics.push(format!(
                            "行 {}: 未知观测码类型 {}，已跳过",
                            idx + 1,
                            code
                        ));
                    }
                }
            }
            for (freq, acc) in by_freq {
                let id = stable_id(
                    "ob",
                    &[&station, &sat, &week.to_string(), &fmt_tow(tow), &freq.to_string()],
                );
                obs.push(ObsPoint {
                    id,
                    station: station.clone(),
                    sat: sat.clone(),
                    week,
                    tow,
                    freq,
                    code_l: acc.code_l,
                    code_p: acc.code_p,
                    code_d: acc.code_d,
                    code_s: acc.code_s,
                    l_cycles: acc.l,
                    p_m: acc.p,
                    d_hz: acc.d,
                    snr_dbhz: acc.s,
                    lli: acc.lli,
                    raw: ol.to_string(),
                    diagnostics: diagnostics.clone(),
                    input_id: input_id.to_string(),
                });
            }
        }
        idx += 1;
    }
    if obs.is_empty() {
        return Err(err(lines.len(), "文件不含任何观测历元", ""));
    }
    Ok(ParsedRinex { station, obs, diagnostics })
}

/// 钟跳事件旁车文件：`CLOCKJUMP <站> <周> <周内秒> <偏移秒>`，`#` 为注释。
pub fn parse_clock_file(text: &str) -> Result<Vec<ClockJump>, String> {
    let mut out = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 5 || parts[0] != "CLOCKJUMP" {
            return Err(format!(
                "行 {}: 期望 'CLOCKJUMP <站> <周> <周内秒> <偏移秒>'，实际 '{}'",
                n + 1,
                line
            ));
        }
        let week: u32 = parts[2]
            .parse()
            .map_err(|_| format!("行 {}: 周不可解析", n + 1))?;
        let tow: f64 = parts[3]
            .parse()
            .map_err(|_| format!("行 {}: 周内秒不可解析", n + 1))?;
        let offset: f64 = parts[4]
            .parse()
            .map_err(|_| format!("行 {}: 偏移不可解析", n + 1))?;
        out.push(ClockJump {
            id: stable_id(
                "cj",
                &["declared", parts[1], &week.to_string(), &fmt_tow(tow)],
            ),
            station: parts[1].to_string(),
            week,
            tow,
            offset_s: offset,
            origin: JumpOrigin::Declared,
            status: JumpStatus::DeclaredPending,
            declared_id: None,
        });
    }
    Ok(out)
}
