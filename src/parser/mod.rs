//! 导入层：接受可复制的小型样例（自有 JSON）与 RINEX 2.x/3.x OBS 文件。
//! 所有字段按历元原样保存；无法转换的值留在原位置并附诊断，不丢弃。
use crate::clock::civil_to_unix;
use crate::model::{Diagnostic, ParsedImport};

pub mod rinex;

pub fn parse_input(text: &str, filename: &str) -> ParsedImport {
    let lower = filename.to_ascii_lowercase();
    let looks_json = lower.ends_with(".json")
        || text.trim_start().starts_with('{')
        || text.trim_start().starts_with('[');
    if looks_json {
        parse_json(text)
    } else {
        rinex::parse_rinex(text)
    }
}

fn parse_json(text: &str) -> ParsedImport {
    let mut out = ParsedImport { format: "json".into(), ..Default::default() };
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            out.diagnostics.push(Diagnostic {
                severity: "error".into(),
                code: "json-syntax".into(),
                message: format!("JSON 解析失败：{e}"),
                location: String::new(),
                raw_snippet: snippet(text, e.line(), e.column()),
            });
            return out;
        }
    };
    // 允许两种形态：直接 CommitIn，或 { "commit": {...} }
    let commit_v = if v.get("epochs").is_some() || v.get("clock_jumps").is_some() {
        v
    } else if let Some(c) = v.get("commit").cloned() {
        c
    } else {
        out.diagnostics.push(Diagnostic {
            severity: "error".into(),
            code: "json-shape".into(),
            message: "缺少 epochs 或 clock_jumps，无法识别为提交载荷".into(),
            location: String::new(),
            raw_snippet: String::new(),
        });
        return out;
    };
    match serde_json::from_value::<crate::model::CommitIn>(commit_v) {
        Ok(commit) => {
            out.commit = commit;
            validate(&mut out);
        }
        Err(e) => out.diagnostics.push(Diagnostic {
            severity: "error".into(),
            code: "json-field".into(),
            message: format!("字段结构不符：{e}"),
            location: String::new(),
            raw_snippet: String::new(),
        }),
    }
    out
}

fn snippet(text: &str, line: usize, col: usize) -> String {
    text.lines()
        .nth(line.saturating_sub(1))
        .map(|l| {
            let start = col.saturating_sub(20).min(l.len());
            l.get(start..).unwrap_or(l).chars().take(40).collect()
        })
        .unwrap_or_default()
}

/// 跨格式的轻量校验：时间可解析性、单位可转换性；不可转换只出诊断，不丢值。
fn validate(out: &mut ParsedImport) {
    for (ei, ep) in out.commit.epochs.iter().enumerate() {
        if parse_time(&ep.time).is_none() {
            out.diagnostics.push(Diagnostic {
                severity: "error".into(),
                code: "bad-time".into(),
                message: format!("历元时间无法解析：{}", ep.time),
                location: format!("epochs[{ei}].time"),
                raw_snippet: ep.time_raw.clone(),
            });
        }
        for (oi, ob) in ep.observations.iter().enumerate() {
            let loc = |q: &crate::units::Quant| {
                if !q.raw.trim().is_empty() && !q.convertible() {
                    Some(format!("无法把 {:?} 转换为 SI（单位 {}）", q.raw, q.unit))
                } else {
                    None
                }
            };
            for (field, q) in [
                ("pseudorange", ob.pseudorange.as_ref()),
                ("carrier", ob.carrier.as_ref()),
                ("doppler", ob.doppler.as_ref()),
                ("snr", ob.snr.as_ref()),
            ]
            .into_iter()
            .filter_map(|(f, q)| q.map(|q| (f, q)))
            {
                if let Some(msg) = loc(q) {
                    out.diagnostics.push(Diagnostic {
                        severity: "warning".into(),
                        code: "unconvertible-value".into(),
                        message: msg,
                        location: format!("epochs[{ei}].observations[{oi}].{field}"),
                        raw_snippet: q.raw.clone(),
                    });
                }
            }
            if ob.code.len() < 2 {
                out.diagnostics.push(Diagnostic {
                    severity: "warning".into(),
                    code: "odd-code".into(),
                    message: format!("观测码过短，保留原样：{}", ob.code),
                    location: format!("epochs[{ei}].observations[{oi}].code"),
                    raw_snippet: ob.code.clone(),
                });
            }
        }
    }
}

/// 解析 ISO 风格时间为 Unix 秒（UTC）。允许 "YYYY-MM-DDTHH:MM:SS[.frac][Z]"。
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('Z').trim_end_matches('z');
    let (date, time) = if let Some((d, t)) = s.split_once(['T', ' ']) {
        (d, Some(t))
    } else {
        (s, None)
    };
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let (hh, mm, ss) = match time {
        Some(t) => {
            let mut tp = t.split(':');
            let hh: u32 = tp.next()?.parse().ok()?;
            let mm: u32 = tp.next()?.parse().ok()?;
            let ss: f64 = tp.next().unwrap_or("0").parse().ok()?;
            (hh, mm, ss)
        }
        None => (0, 0, 0.0),
    };
    Some(civil_to_unix(y, m, d, hh, mm, ss))
}

/// 由观测码推断频率编号/波段字符；无法判定返回空（调用方保留原文）。
pub fn band_of_code(code: &str) -> String {
    // RINEX3: C1C/L2W/D5X；RINEX2: C1/L2/P2。第二位即波段。
    let c = code.trim();
    if c.len() >= 2 && c.chars().nth(1).is_some_and(|ch| ch.is_ascii_alphanumeric()) {
        c.chars().nth(1).unwrap().to_string()
    } else {
        String::new()
    }
}

/// RINEX 解析完成后的统一校验入口。
pub fn parse_input_supplement(out: &mut ParsedImport) {
    validate(out);
}
