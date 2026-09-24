//! 极小但忠实的 RINEX 2.x/3.x 观测文件解析器（离线、无外部依赖）。
//! 支持本案例包所需结构：# / TYPES OF OBSERV、SYS / # / OBS TYPES、
//! 历元行（v3 以 '>' 起始；v2 以两字符年份起始）、16 列字段、LLI/SSI、
//! 头部注释中的整机钟跳标记 "CB-CLOCK-JUMP"。
use crate::model::{
    ClockJumpIn, CommitIn, Diagnostic, EpochIn, ObservationIn, ParsedImport, StationIn,
};
use crate::units::Quant;

pub fn parse_rinex(text: &str) -> ParsedImport {
    let mut out = ParsedImport { format: "rinex".into(), ..Default::default() };
    let mut lines = text.lines();
    let mut header_end = false;
    let mut version: f64 = 0.0;
    let mut types_v3: Vec<(char, String)> = Vec::new();
    let mut types_v2: Vec<String> = Vec::new();
    let mut marker = String::new();
    let mut receiver = String::new();
    let mut antenna = String::new();
    let mut station_id = String::new();
    let mut clock_jumps: Vec<ClockJumpIn> = Vec::new();
    let mut body: Vec<String> = Vec::new();

    while let Some(line) = lines.next() {
        if header_end {
            body.push(line.to_string());
            continue;
        }
        let label = if line.len() >= 60 { line[60..].trim() } else { line.trim() };
        let head = |a: usize, b: usize| line.get(a..b).unwrap_or("").trim();
        match label {
            "RINEX VERSION / TYPE" => {
                version = head(0, 9).parse().unwrap_or(0.0);
            }
            "MARKER NAME" => marker = head(0, 4).to_string(),
            "STATION NUMBER / NAME" => {
                station_id = head(0, 4).to_string();
                let m = head(5, 25).to_string();
                if marker.is_empty() { marker = m; }
            }
            "RECEIVER NUMBER / TYPE / VERS" => receiver = head(0, 20).to_string(),
            "ANT # / TYPE" => antenna = head(0, 20).to_string(),
            "COMMENT" => {
                let c = head(0, 60).trim();
                if let Some(cj) = parse_clock_comment(c) {
                    clock_jumps.push(cj);
                }
            }
            "SYS / # / OBS TYPES" => {
                let sys = head(0, 1).chars().next().unwrap_or(' ');
                let n: usize = head(3, 6).parse().unwrap_or(0);
                let mut codes: Vec<String> = Vec::new();
                for i in 0..((n + 12) / 12).max(1) {
                    let segment = if i == 0 {
                        head(7, 60).to_string()
                    } else {
                        match lines.next() {
                            Some(l) => l.get(0..60).unwrap_or("").trim().to_string(),
                            None => break,
                        }
                    };
                    push_codes(&segment, &mut codes);
                    if codes.len() >= n { break; }
                }
                for c in codes.into_iter().take(n) {
                    types_v3.push((sys, c));
                }
            }
            "# / TYPES OF OBSERV" => {
                let n: usize = head(0, 6).parse().unwrap_or(0);
                let mut codes: Vec<String> = Vec::new();
                push_codes(head(10, 60), &mut codes);
                if n > 9 {
                    if let Some(cont) = lines.next() {
                        push_codes(cont.get(6..60).unwrap_or(""), &mut codes);
                    }
                }
                for c in codes.into_iter().take(n) {
                    types_v2.push(c);
                }
            }
            "END OF HEADER" => header_end = true,
            _ => {}
        }
    }

    if !header_end {
        out.diagnostics.push(Diagnostic {
            severity: "error".into(),
            code: "rinex-no-header-end".into(),
            message: "未找到 END OF HEADER".into(),
            location: String::new(),
            raw_snippet: String::new(),
        });
    }

    out.commit.station = Some(StationIn {
        id: station_id,
        marker,
        receiver,
        antenna,
        ..Default::default()
    });
    out.commit.clock_jumps = clock_jumps;

    if version >= 3.0 {
        parse_body_v3(&body, &types_v3, &mut out);
    } else {
        parse_body_v2(&body, &types_v2, &mut out);
    }
    crate::parser::parse_input_supplement(&mut out);
    out
}

fn push_codes(segment: &str, out: &mut Vec<String>) {
    let bytes = segment.as_bytes();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let c: String = segment[i..i + 3].trim().to_string();
        if !c.is_empty() {
            out.push(c);
        }
        i += 4;
    }
}

fn parse_clock_comment(c: &str) -> Option<ClockJumpIn> {
    let mut it = c.split_whitespace();
    if it.next()? != "CB-CLOCK-JUMP" {
        return None;
    }
    let time = it.next()?.to_string();
    let raw = it.next()?.to_string();
    let unit = it.next().unwrap_or("s").to_string();
    Some(ClockJumpIn {
        time: time.clone(),
        time_raw: time,
        jump: Quant::new(raw, unit),
        source: "header-comment".into(),
        note: c.to_string(),
    })
}

fn code_kind(code: &str) -> (&'static str, &'static str) {
    match code.chars().next() {
        Some('C') | Some('P') => ("pseudorange", "m"),
        Some('L') => ("carrier", "cycle"),
        Some('D') => ("doppler", "Hz"),
        Some('S') => ("snr", "dBHz"),
        _ => ("unknown", "1"),
    }
}

fn parse_body_v3(body: &[String], types: &[(char, String)], out: &mut ParsedImport) {
    let mut i = 0;
    while i < body.len() {
        let line = &body[i];
        if !line.starts_with('>') {
            if line.trim().is_empty() {
                i += 1;
                continue;
            }
            out.diagnostics.push(Diagnostic {
                severity: "warning".into(),
                code: "rinex-stray-line".into(),
                message: "历元外数据行已忽略".into(),
                location: format!("body line {}", i + 1),
                raw_snippet: line.chars().take(60).collect(),
            });
            i += 1;
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 8 {
            out.diagnostics.push(Diagnostic {
                severity: "error".into(),
                code: "rinex-bad-epoch".into(),
                message: "v3 历元行字段不足".into(),
                location: format!("body line {}", i + 1),
                raw_snippet: line.clone(),
            });
            i += 1;
            continue;
        }
        let (time, time_raw) = join_time(&toks[1..7]);
        let flag = toks[7].to_string();
        let nsat: usize = toks[8].parse().unwrap_or(0);
        let mut epoch = EpochIn {
            time,
            time_raw,
            epoch_flag: flag,
            sat_states: vec![],
            observations: vec![],
        };
        i += 1;
        for _ in 0..nsat {
            if i >= body.len() {
                break;
            }
            let sat_line = &body[i];
            if sat_line.len() < 3 {
                i += 1;
                continue;
            }
            let sat = sat_line[0..3].trim().to_string();
            let sys_hint = sat.chars().next().unwrap_or(' ');
            let codes: Vec<String> = types
                .iter()
                .filter(|(s, _)| *s == sys_hint || *s == ' ')
                .map(|(_, c)| c.clone())
                .collect();
            let ntypes = codes.len();
            let mut fields: Vec<String> = Vec::new();
            extract_fields(sat_line.get(3..).unwrap_or(""), &mut fields);
            while fields.len() < ntypes {
                i += 1;
                if i >= body.len() {
                    break;
                }
                let more = &body[i];
                let before = fields.len();
                extract_fields(more, &mut fields);
                if fields.len() == before {
                    break;
                }
            }
            emit_obs(&sat, &codes, &fields, &mut epoch, out);
            i += 1;
        }
        out.commit.epochs.push(epoch);
    }
}

fn parse_body_v2(body: &[String], types: &[String], out: &mut ParsedImport) {
    let mut i = 0;
    while i < body.len() {
        let line = &body[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if line.len() < 32 || !line.get(0..1).is_some_and(|s| s == " ") {
            i += 1;
            continue;
        }
        let year2: i32 = match line.get(1..3).unwrap_or("").trim().parse() {
            Ok(v) => v,
            Err(_) => {
                i += 1;
                continue;
            }
        };
        let month: u32 = line.get(3..5).unwrap_or("").trim().parse().unwrap_or(0);
        if !(1..=12).contains(&month) {
            i += 1;
            continue;
        }
        let day: u32 = line.get(5..8).unwrap_or("").trim().parse().unwrap_or(0);
        let hh: u32 = line.get(8..11).unwrap_or("").trim().parse().unwrap_or(0);
        let mm: u32 = line.get(11..14).unwrap_or("").trim().parse().unwrap_or(0);
        let ss: f64 = line.get(14..26).unwrap_or("0").trim().parse().unwrap_or(0.0);
        let flag = line.get(26..29).unwrap_or("0").trim().to_string();
        let nsat: usize = line.get(29..32).unwrap_or("0").trim().parse().unwrap_or(0);
        let year = if year2 < 80 { 2000 + year2 } else { 1900 + year2 };
        let time = format!("{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}");
        let time_raw = format!("{year2:02} {month} {day} {hh} {mm} {ss}");
        let sats: Vec<String> = (0..nsat)
            .filter_map(|k| {
                let a = 32 + k * 3;
                line.get(a..a + 3).map(|s| {
                    let s = s.trim();
                    if s.is_empty() {
                        String::new()
                    } else if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                        s.to_string()
                    } else {
                        format!("G{}", s.trim())
                    }
                })
            })
            .collect();
        let mut epoch = EpochIn {
            time,
            time_raw,
            epoch_flag: flag,
            sat_states: vec![],
            observations: vec![],
        };
        i += 1;
        let lines_per = types.len().div_ceil(5).max(1);
        for sat in sats.iter().filter(|s| !s.is_empty()) {
            let mut fields: Vec<String> = Vec::new();
            for _ in 0..lines_per {
                if i < body.len() {
                    extract_fields(&body[i], &mut fields);
                    i += 1;
                }
            }
            emit_obs(sat, types, &fields, &mut epoch, out);
        }
        out.commit.epochs.push(epoch);
    }
}

fn extract_fields(s: &str, fields: &mut Vec<String>) {
    let bytes = s.len();
    let mut k = 0;
    while k + 16 <= bytes {
        fields.push(s[k..k + 16].to_string());
        k += 16;
    }
}

fn emit_obs(
    sat: &str,
    codes: &[String],
    fields: &[String],
    epoch: &mut EpochIn,
    out: &mut ParsedImport,
) {
    for (idx, code) in codes.iter().enumerate() {
        let field = match fields.get(idx) {
            Some(f) => f.clone(),
            None => continue,
        };
        let num_raw = field.get(0..14).unwrap_or("").trim().to_string();
        if num_raw.is_empty() {
            continue; // 频率缺失：不造观测，弧段层据此识别缺口。
        }
        let lli = field.get(14..15).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let (kind, unit) = code_kind(code);
        let mut ob = ObservationIn {
            sat: sat.to_string(),
            code: code.clone(),
            freq_band: crate::parser::band_of_code(code),
            freq_source: "code".into(),
            pseudorange: None,
            carrier: None,
            doppler: None,
            lli,
            snr: None,
            raw_field: Some(field.clone()),
        };
        let q = Quant::new(num_raw.clone(), unit);
        if !q.convertible() {
            out.diagnostics.push(Diagnostic {
                severity: "warning".into(),
                code: "unconvertible-field".into(),
                message: format!("{code} 字段无法解析，保留原文"),
                location: format!("sat {sat}"),
                raw_snippet: field.clone(),
            });
        }
        match kind {
            "pseudorange" => ob.pseudorange = Some(q),
            "carrier" => ob.carrier = Some(q),
            "doppler" => ob.doppler = Some(q),
            "snr" => ob.snr = Some(q),
            _ => {}
        }
        epoch.observations.push(ob);
    }
}

fn join_time(t: &[&str]) -> (String, String) {
    let y: i64 = t[0].parse().unwrap_or(0);
    let m: u32 = t[1].parse().unwrap_or(0);
    let d: u32 = t[2].parse().unwrap_or(0);
    let hh: u32 = t[3].parse().unwrap_or(0);
    let mm: u32 = t[4].parse().unwrap_or(0);
    let ss: f64 = t[5].parse().unwrap_or(0.0);
    let iso = format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}");
    (iso, t.join(" "))
}
