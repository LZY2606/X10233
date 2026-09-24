//! RINEX 3 观测文件解析（子集）。
//! 原则：伪距、载波、Doppler、LLI、信噪比、观测码均按历元原样保存；
//! 无法转换的值留在原位置并附诊断，绝不为了派生计算而丢弃。

use crate::ids::make_id;
use crate::model::{epoch_to_ns, ns_to_epoch, ObsRow, Unit};

pub struct RinexParse {
    pub station: String,
    pub obs: Vec<ObsRow>,
    pub diagnostics: Vec<String>,
}

fn header_line(content: &str, label: &str) -> String {
    let mut s = content.to_string();
    while s.len() < 60 {
        s.push(' ');
    }
    s.truncate(60);
    format!("{}{}", s, label)
}

/// 供案例包生成器/测试使用：构造一条头部行。
pub fn hdr(content: &str, label: &str) -> String {
    header_line(content, label)
}

/// 解析 RINEX 观测文件。结构性错误返回 Err（导入整体拒绝，不产生部分写入）；
/// 单值错误只记入诊断，原始字段保留。
pub fn parse_rinex_obs(version_id: &str, content: &str) -> Result<RinexParse, String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Err("empty file".into());
    }
    // --- 头部 ---
    let version_label = lines[0].get(60..).map(|s| s.trim().to_string()).unwrap_or_default();
    if !version_label.contains("RINEX VERSION") {
        return Err("line 1: missing 'RINEX VERSION / TYPE' label; not a RINEX file".into());
    }
    if !lines[0].get(20..40).unwrap_or("").contains("OBSERVATION") {
        return Err(format!(
            "line 1: file type is '{}', expected OBSERVATION DATA",
            lines[0].get(20..40).unwrap_or("").trim()
        ));
    }
    let mut station = String::new();
    let mut obs_types: std::collections::HashMap<char, Vec<String>> = Default::default();
    let mut idx = 1;
    let mut diagnostics = Vec::new();
    let mut saw_end = false;
    while idx < lines.len() {
        let line = lines[idx];
        let label = line.get(60..).unwrap_or("").trim().to_string();
        match label.as_str() {
            "MARKER NAME" => station = line.get(0..60).unwrap_or("").trim().to_string(),
            "SYS / # / OBS TYPES" => {
                let sys = line.chars().next().unwrap_or(' ');
                let n: usize = line
                    .get(3..6)
                    .and_then(|s| s.trim().parse().ok())
                    .ok_or_else(|| format!("line {}: bad obs-type count", idx + 1))?;
                let mut types: Vec<String> = Vec::new();
                let mut cur = line.get(7..60).unwrap_or("");
                loop {
                    for i in 0..13 {
                        if let Some(t) = cur.get(i * 4..i * 4 + 3) {
                            let t = t.trim();
                            if !t.is_empty() {
                                types.push(t.to_string());
                            }
                        }
                    }
                    if types.len() >= n {
                        break;
                    }
                    idx += 1;
                    if idx >= lines.len() {
                        return Err("unexpected EOF in SYS / # / OBS TYPES continuation".into());
                    }
                    cur = lines[idx].get(7..60).unwrap_or("");
                }
                types.truncate(n);
                obs_types.insert(sys, types);
            }
            "END OF HEADER" => {
                saw_end = true;
                idx += 1;
                break;
            }
            _ => {}
        }
        idx += 1;
    }
    if !saw_end {
        return Err("missing END OF HEADER".into());
    }
    if station.is_empty() {
        station = "UNKNOWN".into();
        diagnostics.push("no MARKER NAME; station set to UNKNOWN".into());
    }
    if obs_types.is_empty() {
        return Err("no SYS / # / OBS TYPES records".into());
    }

    // --- 数据 ---
    let mut obs = Vec::new();
    while idx < lines.len() {
        let line = lines[idx];
        idx += 1;
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('>') {
            return Err(format!("line {}: expected epoch record starting with '>'", idx));
        }
        let f: Vec<&str> = line[1..].split_whitespace().collect();
        if f.len() < 8 {
            return Err(format!("line {}: malformed epoch record", idx));
        }
        let (y, mo, d, h, mi) = (
            f[0].parse::<i32>(),
            f[1].parse::<u32>(),
            f[2].parse::<u32>(),
            f[3].parse::<u32>(),
            f[4].parse::<u32>(),
        );
        let sec = f[5].parse::<f64>();
        let (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(sec)) = (y, mo, d, h, mi, sec) else {
            return Err(format!("line {}: unparseable epoch fields", idx));
        };
        let flag = f[6].parse::<i64>().unwrap_or(0);
        let nsat: usize = f[7]
            .parse()
            .map_err(|_| format!("line {}: bad satellite count", idx))?;
        let epoch = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:012.9}Z",
            y, mo, d, h, mi, sec
        );
        // 规范化为 RFC3339（小数秒可能带尾随零差异，统一走 ns 往返）
        let epoch_ns = epoch_to_ns(&epoch)
            .ok_or_else(|| format!("line {}: epoch out of range", idx))?;
        let epoch = ns_to_epoch(epoch_ns);
        if flag > 1 {
            diagnostics.push(format!(
                "epoch {}: special event flag {} recorded; epochs kept as-is",
                epoch, flag
            ));
        }
        for _ in 0..nsat {
            if idx >= lines.len() {
                return Err(format!("epoch {}: missing satellite lines", epoch));
            }
            let sline = lines[idx];
            idx += 1;
            if sline.len() < 3 {
                return Err(format!("line {}: satellite line too short", idx));
            }
            let sat = sline[0..3].trim().to_string();
            let sys = sat.chars().next().unwrap_or(' ');
            let Some(types) = obs_types.get(&sys) else {
                return Err(format!("line {}: no obs types declared for system '{}'", idx, sys));
            };
            for (ti, code) in types.iter().enumerate() {
                let start = 3 + ti * 16;
                let end = start + 16;
                if start >= sline.len() {
                    break; // 行尾缺字段 = 该观测缺失（频率缺失场景）
                }
                let field = format!("{:16}", &sline[start..end.min(sline.len())]);
                let raw_val = field[0..14].to_string();
                let lli_s = field[14..15].trim();
                let ssi_s = field[15..16].trim();
                if raw_val.trim().is_empty() {
                    continue; // 空字段：观测缺失，不产生行
                }
                let value = raw_val.trim().parse::<f64>();
                let (value, diag) = match value {
                    Ok(v) => (Some(v), None),
                    Err(_) => (
                        None,
                        Some(format!(
                            "unconvertible value '{}' kept in place",
                            raw_val.trim()
                        )),
                    ),
                };
                let unit = Unit::of_obs_code(code)
                    .map(|u| u.as_str().to_string())
                    .unwrap_or_else(|| "unknown".into());
                let id = make_id(
                    "obs",
                    &format!("{}|{}|{}|{}|{}", version_id, epoch, sat, code, raw_val),
                );
                obs.push(ObsRow {
                    id,
                    version_id: version_id.to_string(),
                    station: station.clone(),
                    sat: sat.clone(),
                    code: code.clone(),
                    epoch: epoch.clone(),
                    epoch_ns,
                    raw: raw_val,
                    value,
                    unit,
                    lli: if lli_s.is_empty() { None } else { lli_s.parse().ok() },
                    ssi: if ssi_s.is_empty() { None } else { ssi_s.parse().ok() },
                    diag,
                });
            }
        }
    }
    if obs.is_empty() {
        return Err("no observations parsed".into());
    }
    Ok(RinexParse { station, obs, diagnostics })
}
