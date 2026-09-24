//! OBS-CARDS v1 导入解析器（小型、可复制、RINEX 观测卡片子集）。
//!
//! 设计原则：
//! * 伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号、观测码按历元原样保存；
//! * 解析器不做等间隔假设；
//! * 频率缺失即缺卡（warning 级诊断，不补零）；
//! * 无法转换的值留在原 Quant.raw 位置并附 diagnostic；
//! * 结构性错误以 `ImportError { code, .. }` 返回，错误信息带行号。

use crate::domain::*;

const C: f64 = 299_792_458.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Head,
    Frequencies,
    States,
    Epochs,
}

pub fn parse_obs_cards(text: &str) -> RawBatch {
    let mut b = RawBatch { format_version: "obs-cards/v1".into(), ..Default::default() };
    let mut section = Section::Head;
    let mut current_epoch: Option<usize> = None;
    let mut seen_epoch: Vec<(usize, String)> = Vec::new();
    let mut freq_seen = std::collections::BTreeSet::new();
    let mut meas_seen = std::collections::BTreeSet::new();

    for (i, raw_line) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match &line[1..line.len() - 1] {
                "head" => Section::Head,
                "frequencies" => Section::Frequencies,
                "states" => Section::States,
                "epochs" => Section::Epochs,
                other => {
                    push_err(&mut b, line_no, "E100", format!("未知段落 [{other}]"), raw_line);
                    continue;
                }
            };
            continue;
        }
        match section {
            Section::Head => parse_head(&mut b, line_no, line, raw_line),
            Section::Frequencies => {
                if let Some(f) = parse_frequency(&mut b, line_no, line, raw_line) {
                    if !freq_seen.insert(f.id.clone()) {
                        push_err(&mut b, line_no, "E110", format!("频率编号重复: {}", f.id), raw_line);
                    } else {
                        b.frequencies.push(f);
                    }
                }
            }
            Section::States => parse_state(&mut b, line_no, line, raw_line),
            Section::Epochs => {
                if let Some(rest) = line.strip_prefix("EPOCH ") {
                    match parse_epoch(&mut b, line_no, rest, raw_line, &mut seen_epoch) {
                        Some(idx) => current_epoch = Some(idx),
                        None => current_epoch = None,
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("CLOCK_JUMP ") {
                    match &current_epoch {
                        Some(idx) => parse_clock_event(&mut b, line_no, rest, raw_line, *idx),
                        None => push_err(&mut b, line_no, "E120", "CLOCK_JUMP 必须位于 EPOCH 块内", raw_line),
                    }
                    continue;
                }
                match &current_epoch {
                    Some(idx) => {
                        if let Some(m) = parse_meas(&mut b, line_no, line, raw_line, *idx) {
                            let key = (m.sv.clone(), m.epoch_idx, m.freq_id.clone());
                            if !meas_seen.insert(key) {
                                push_err(
                                    &mut b,
                                    line_no,
                                    "E130",
                                    format!("{}/{} 历元{} 的观测卡重复", m.sv, m.freq_id, m.epoch_idx),
                                    raw_line,
                                );
                            } else {
                                let known = b.frequencies.iter().any(|f| f.id == m.freq_id && f.code == m.code);
                                if !known {
                                    b.diagnostics.push(ImportDiagnostic {
                                        line_no,
                                        code: "W130".into(),
                                        message: format!(
                                            "观测码 {}/{} 未在 [frequencies] 声明，仍原样保留",
                                            m.freq_id, m.code
                                        ),
                                        raw_line: raw_line.to_string(),
                                    });
                                }
                                b.meas.push(m);
                            }
                        }
                    }
                    None => push_err(&mut b, line_no, "E120", "观测卡必须位于 EPOCH 块内", raw_line),
                }
            }
        }
    }

    if b.station_id.is_empty() {
        b.errors.push(ImportError {
            line_no: 0,
            code: "E101".into(),
            message: "缺少 STATION 声明".into(),
            raw_line: String::new(),
        });
    }
    if b.frequencies.is_empty() {
        b.errors.push(ImportError {
            line_no: 0,
            code: "E102".into(),
            message: "缺少 [frequencies] 频率声明".into(),
            raw_line: String::new(),
        });
    }
    if b.epochs.is_empty() {
        b.errors.push(ImportError {
            line_no: 0,
            code: "E103".into(),
            message: "缺少历元数据".into(),
            raw_line: String::new(),
        });
    }
    fill_intervals(&mut b);
    b
}

fn push_err(b: &mut RawBatch, line_no: usize, code: &str, message: String, raw_line: &str) {
    b.errors.push(ImportError { line_no, code: code.into(), message, raw_line: raw_line.to_string() });
}

fn parse_head(b: &mut RawBatch, line_no: usize, line: &str, raw_line: &str) {
    let Some((key, val)) = line.split_once('=') else {
        push_err(b, line_no, "E104", format!("头部行应为 KEY=VALUE: {line}"), raw_line);
        return;
    };
    let val = val.trim();
    match key.trim() {
        "STATION" => b.station_id = val.into(),
        "MARKER_NAME" => b.marker_name = val.into(),
        "RECEIVER" => b.receiver = val.into(),
        "FORMAT" => {
            if val != "OBS-CARDS/v1" {
                push_err(b, line_no, "E105", format!("不支持的 FORMAT: {val}"), raw_line);
            }
        }
        other => {
            b.diagnostics.push(ImportDiagnostic {
                line_no,
                code: "W100".into(),
                message: format!("忽略未知头部键 {other}"),
                raw_line: raw_line.to_string(),
            });
        }
    }
}

fn parse_frequency(b: &mut RawBatch, line_no: usize, line: &str, raw_line: &str) -> Option<Frequency> {
    // FREQ L1 C1W 1575420000 Hz
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 || parts[0] != "FREQ" {
        push_err(b, line_no, "E111", "频率行应为: FREQ <id> <code> <hz> Hz", raw_line);
        return None;
    }
    if parts[4] != "Hz" {
        push_err(b, line_no, "E112", "频率单位必须声明为 Hz", raw_line);
        return None;
    }
    let hz_raw = parts[3];
    let hz = match hz_raw.parse::<f64>() {
        Ok(v) if v > 0.0 => Some(v),
        _ => None,
    };
    if hz.is_none() {
        push_err(b, line_no, "E113", format!("频率无法解析为正的 Hz 值: {hz_raw}"), raw_line);
        return None;
    }
    let hz = hz.unwrap();
    Some(Frequency {
        id: parts[1].into(),
        code: parts[2].into(),
        freq_hz: hz,
        wavelength_m: C / hz,
    })
}

fn parse_epoch(
    b: &mut RawBatch,
    line_no: usize,
    rest: &str,
    raw_line: &str,
    seen: &mut Vec<(usize, String)>,
) -> Option<usize> {
    // EPOCH 0 2026-01-01T00:00:00Z
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 2 {
        push_err(b, line_no, "E121", "历元行应为: EPOCH <idx> <RFC3339>", raw_line);
        return None;
    }
    let idx = match parts[0].parse::<usize>() {
        Ok(v) => v,
        Err(_) => {
            push_err(b, line_no, "E122", format!("历元序号不是非负整数: {}", parts[0]), raw_line);
            return None;
        }
    };
    if let Some((_, prior)) = seen.iter().find(|(i, _)| *i == idx) {
        push_err(b, line_no, "E123", format!("历元序号 {idx} 重复（先前 {prior}）"), raw_line);
        return None;
    }
    let ts_raw = parts[1].to_string();
    let (epoch_s, diag) = match crate::clock::parse_rfc3339(&ts_raw) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    seen.push((idx, ts_raw.clone()));
    b.epochs.push(RawEpoch {
        idx,
        timestamp_raw: ts_raw,
        timestamp_epoch_s: epoch_s,
        time_diagnostic: diag,
        interval_s_from_prev: None,
    });
    Some(idx)
}

fn parse_clock_event(b: &mut RawBatch, line_no: usize, rest: &str, raw_line: &str, idx: usize) {
    // CLOCK_JUMP 1.0 ms source=obs-cards
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() < 2 || parts[1] != "ms" {
        push_err(b, line_no, "E140", "钟跳行应为: CLOCK_JUMP <value> ms source=...", raw_line);
        return;
    }
    let jump_raw = parts[0].to_string();
    let jump_ms = jump_raw.parse::<f64>().ok().filter(|v| v.is_finite());
    if jump_ms.is_none() {
        b.diagnostics.push(ImportDiagnostic {
            line_no,
            code: "W140".into(),
            message: format!("钟跳量级无法解析，原始值保留: {jump_raw}"),
            raw_line: raw_line.to_string(),
        });
    }
    let source = parts
        .iter()
        .find_map(|p| p.strip_prefix("source="))
        .unwrap_or("obs-cards")
        .to_string();
    b.clock_events.push(ClockEvent { epoch_idx: idx, jump_ms_raw: jump_raw, jump_ms, source });
}

fn parse_meas(b: &mut RawBatch, line_no: usize, line: &str, raw_line: &str, epoch_idx: usize) -> Option<RawMeas> {
    // G02 L1 C1W P=21936211.123 L=115649312.78 D=-213.4 LLI=0 S=45.2
    let mut toks = line.split_whitespace();
    let sv = toks.next()?;
    let freq_id = toks.next()?;
    let code = toks.next()?;
    let mut map = std::collections::BTreeMap::new();
    for tok in toks {
        let (k, v) = match tok.split_once('=') {
            Some(kv) => kv,
            None => {
                push_err(b, line_no, "E131", format!("观测字段必须为 K=V: {tok}"), raw_line);
                return None;
            }
        };
        if map.insert(k, v).is_some() {
            push_err(b, line_no, "E132", format!("观测字段 {k} 重复"), raw_line);
            return None;
        }
    }
    let take = |key: &str| -> Option<String> { map.get(key).map(|s| s.to_string()) };
    let (p_raw, l_raw, d_raw, s_raw) = match (take("P"), take("L"), take("D"), take("S")) {
        (Some(p), Some(l), Some(d), Some(s)) => (p, l, d, s),
        missing => {
            push_err(
                b,
                line_no,
                "E133",
                format!("观测卡缺少字段（需要 P L D S；LLI 可缺省）: {missing:?}"),
                raw_line,
            );
            return None;
        }
    };

    let mut pseudorange = Quant::meters(&p_raw);
    if pseudorange.value.is_some_and(|v| v < 0.0) {
        pseudorange = pseudorange.with_diag("伪距为负值，保留原值待人工核对");
    }
    let carrier = match l_raw.parse::<f64>() {
        Ok(v) => Quant::new(l_raw.clone(), Some(v), "cycle"),
        Err(e) => Quant::new(l_raw.clone(), None, "cycle").with_diag(format!("载波无法解析为周: {e}")),
    };
    let doppler = match d_raw.parse::<f64>() {
        Ok(v) => Quant::new(d_raw.clone(), Some(v), "Hz"),
        Err(e) => Quant::new(d_raw.clone(), None, "Hz").with_diag(format!("Doppler 无法解析为 Hz: {e}")),
    };
    let cn0 = match s_raw.parse::<f64>() {
        Ok(v) => Quant::new(s_raw.clone(), Some(v), "dBHz"),
        Err(e) => Quant::new(s_raw.clone(), None, "dBHz").with_diag(format!("信噪比无法解析为 dBHz: {e}")),
    };

    let (lli, lli_raw) = match take("LLI") {
        Some(raw) => {
            let parsed = raw.parse::<u8>().ok().filter(|v| *v <= 7);
            if parsed.is_none() {
                b.diagnostics.push(ImportDiagnostic {
                    line_no,
                    code: "W131".into(),
                    message: format!("LLI 必须是 0..7 的整数，原值保留: {raw}"),
                    raw_line: raw_line.to_string(),
                });
            }
            (parsed, raw)
        }
        None => (None, String::new()),
    };

    Some(RawMeas {
        sv: sv.to_string(),
        epoch_idx,
        freq_id: freq_id.to_string(),
        code: code.to_string(),
        pseudorange,
        carrier_cycles: carrier,
        doppler_hz: doppler,
        lli,
        lli_raw,
        cn0_db_hz: cn0,
    })
}

fn parse_state(b: &mut RawBatch, line_no: usize, line: &str, raw_line: &str) {
    // STATE G01 datum=ITRF2014 x=13234567.1 y=-? z=... clk=0.000012 s
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 || parts[0] != "STATE" {
        push_err(b, line_no, "E150", "STATE 行字段不足", raw_line);
        return;
    }
    let sv = parts[1].to_string();
    let mut epoch = None;
    let mut datum = None;
    let mut vals: std::collections::BTreeMap<&str, String> = std::collections::BTreeMap::new();
    for p in &parts[2..] {
        if let Some(v) = p.strip_prefix("epoch=") {
            epoch = v.parse::<usize>().ok();
        } else if let Some(v) = p.strip_prefix("datum=") {
            datum = Some(v.to_string());
        } else if let Some((k, v)) = p.split_once('=') {
            vals.insert(k, v.to_string());
        }
    }
    let (Some(epoch_idx), Some(datum)) = (epoch, datum) else {
        push_err(b, line_no, "E151", "STATE 需要 epoch=<idx> datum=<name>", raw_line);
        return;
    };
    let qm = |key: &str, unit: &str, diag: &str| match vals.get(key) {
        Some(raw) => match raw.parse::<f64>() {
            Ok(v) => Quant::new(raw.clone(), Some(v), unit),
            Err(e) => Quant::new(raw.clone(), None, unit).with_diag(format!("{diag}: {e}")),
        },
        None => Quant::new("", None, unit).with_diag(format!("{diag}: 字段缺失")),
    };
    b.sat_states.push(SatState {
        sv,
        epoch_idx,
        datum,
        x_m: qm("x", "m", "X 坐标无法解析"),
        y_m: qm("y", "m", "Y 坐标无法解析"),
        z_m: qm("z", "m", "Z 坐标无法解析"),
        clock_bias_s: qm("clk", "s", "卫星钟差无法解析"),
    });
}

fn fill_intervals(b: &mut RawBatch) {
    // 明确计算真实间隔：不等间隔是一等情形，绝不假设均匀采样。
    let mut by_idx: Vec<&RawEpoch> = b.epochs.iter().collect();
    by_idx.sort_by_key(|e| e.idx);
    for pair in by_idx.windows(2) {
        let (prev, cur) = (pair[0], pair[1]);
        let interval = match (prev.timestamp_epoch_s, cur.timestamp_epoch_s) {
            (Some(a), Some(c)) => Some((c - a) as f64),
            _ => None,
        };
        if let Some(e) = b.epochs.iter_mut().find(|e| e.idx == cur.idx) {
            e.interval_s_from_prev = interval;
        }
        if interval.is_some_and(|dt| dt <= 0.0) {
            b.errors.push(ImportError {
                line_no: 0,
                code: "E124".into(),
                message: format!("历元 {} 相对上一历元时间戳未严格递增", cur.idx),
                raw_line: cur.timestamp_raw.clone(),
            });
        }
    }
    // 缺频诊断：只对已出现的 (sv, epoch) 组合检查声明频率是否缺卡。
    let mut combos: std::collections::BTreeSet<(String, usize)> =
        b.meas.iter().map(|m| (m.sv.clone(), m.epoch_idx)).collect();
    let declared: Vec<String> = b.frequencies.iter().map(|f| f.id.clone()).collect();
    let present: std::collections::BTreeSet<(String, usize, String)> =
        b.meas.iter().map(|m| (m.sv.clone(), m.epoch_idx, m.freq_id.clone())).collect();
    for (sv, ep) in combos.iter().cloned() {
        for f in &declared {
            if !present.contains(&(sv.clone(), ep, f.clone())) {
                b.diagnostics.push(ImportDiagnostic {
                    line_no: 0,
                    code: "W120".into(),
                    message: format!("频率缺失：{sv} 历元{ep} 缺 {f}（按缺卡处理，不补零）"),
                    raw_line: String::new(),
                });
            }
        }
    }
}
