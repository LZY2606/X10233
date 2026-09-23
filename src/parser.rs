//! OBX/SAT 导入解析。
//!
//! 设计原则：
//! - 结构性格式错误（缺字段、未知波段、重复定义等）整体拒绝，返回行号诊断；
//! - 数值无法转换不是结构错误：量留在原位置（raw + diagnostic），
//!   派生层跳过它，但界面/导出仍能看到原值，绝不为画图丢值；
//! - 每个派生不发生在这里：本层只逐字保存原始表示。

use crate::domain::*;
use sha2::{Digest, Sha256};

fn batch_id(kind: &str, file_name: &str, raw_text: &str, at_ms: u64) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"|");
    h.update(file_name.as_bytes());
    h.update(b"|");
    h.update(raw_text.as_bytes());
    h.update(b"|");
    h.update(at_ms.to_string().as_bytes());
    format!("batch_{}", &hex::encode(h.finalize())[..20])
}

fn err(file: &str, line_no: usize, msg: impl Into<String>, raw: &str) -> ParseError {
    ParseError { file_name: file.to_string(), line_no, message: msg.into(), raw_line: raw.to_string() }
}

fn parse_time(token: &str) -> Option<f64> {
    if let Ok(v) = token.trim().parse::<f64>() {
        return Some(v);
    }
    // HH:MM:SS（可选小数）
    let parts: Vec<&str> = token.trim().split(':').collect();
    if parts.len() == 3 {
        let h: f64 = parts[0].parse().ok()?;
        let m: f64 = parts[1].parse().ok()?;
        let s: f64 = parts[2].parse().ok()?;
        return Some(h * 3600.0 + m * 60.0 + s);
    }
    None
}

fn known_band(system: &str, band: &str) -> bool {
    matches!(
        (system, band),
        ("G", "1") | ("G", "2") | ("G", "5") | ("R", "1") | ("R", "2")
            | ("E", "1") | ("E", "5") | ("E", "6") | ("C", "1")
            | ("C", "2") | ("C", "5") | ("C", "6") | ("J", "1") | ("J", "2") | ("J", "5")
    )
}

pub fn parse_obx(file_name: &str, raw_text: &str, at_ms: u64) -> Result<ParsedBatch, Vec<ParseError>> {
    let id = batch_id("obx", file_name, raw_text, at_ms);
    let mut errors = Vec::new();
    let mut observations = Vec::new();
    let mut epoch_events = Vec::new();
    let mut frequencies = Vec::new();

    let mut station: Option<String> = None;
    let mut seen_freq = std::collections::HashSet::new();

    for (idx, raw_line) in raw_text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let sref = |raw: &str| SourceRef {
            batch_id: id.clone(),
            file_name: file_name.to_string(),
            line_no,
            raw_line: raw.to_string(),
        };
        let mut fields = line.split_whitespace();
        let tag = fields.next().unwrap_or("");
        match tag {
            "FMT" => {
                let v = fields.next().unwrap_or("");
                if v != "OBX/1" {
                    errors.push(err(file_name, line_no, format!("不支持的格式版本 {v}"), raw_line));
                }
            }
            "STATION" => match fields.next() {
                Some(s) if !s.is_empty() => station = Some(s.to_string()),
                _ => errors.push(err(file_name, line_no, "STATION 缺少站标识", raw_line)),
            },
            "FREQ" => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 4 {
                    errors.push(err(file_name, line_no, "FREQ 需要: FREQ <系统> <波段> <码> <Hz> [K]", raw_line));
                } else {
                    let (sys, band, code, hz) = (f[1], f[2], f[3], f[4]);
                    if !known_band(sys, band) {
                        errors.push(err(file_name, line_no, format!("未知频率 {sys}{band}"), raw_line));
                    }
                    match hz.parse::<f64>() {
                        Ok(hzv) if hzv > 0.0 => {
                            let key = format!("{sys}|{band}|{code}");
                            if !seen_freq.insert(key.clone()) {
                                errors.push(err(file_name, line_no, format!("频率重复定义 {key}"), raw_line));
                            }
                            let k = f.get(5).and_then(|t| t.parse::<i32>().ok());
                            frequencies.push(FrequencyDef {
                                system: sys.into(),
                                band: band.into(),
                                code: code.into(),
                                frequency_hz: hzv,
                                glonass_k: k,
                                source_ref: sref(raw_line),
                            });
                        }
                        _ => errors.push(err(file_name, line_no, format!("FREQ 频率必须为正的 Hz: {hz}"), raw_line)),
                    }
                }
            }
            "EPOCH" => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 2 {
                    errors.push(err(file_name, line_no, "EPOCH 需要时间标记", raw_line));
                }
            }
            "EVENT" => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 3 {
                    errors.push(err(file_name, line_no, "EVENT 需要: EVENT <epoch> <kind> [fields...]", raw_line));
                    continue;
                }
                let st = station.clone().unwrap_or_default();
                if st.is_empty() {
                    errors.push(err(file_name, line_no, "EVENT 之前必须先声明 STATION", raw_line));
                }
                epoch_events.push(EpochEvent {
                    station: st,
                    epoch_token: f[1].to_string(),
                    time_s: parse_time(f[1]),
                    kind: f[2].to_string(),
                    fields: f[3..].iter().map(|s| s.to_string()).collect(),
                    source_ref: sref(raw_line),
                });
            }
            "OBS" => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 7 {
                    errors.push(err(
                        file_name,
                        line_no,
                        "OBS 需要: OBS <epoch> <sat> <band> <code>(P|L|D|S) <value> <unit> [lli] [snr]",
                        raw_line,
                    ));
                    continue;
                }
                let st = match &station {
                    Some(s) => s.clone(),
                    None => {
                        errors.push(err(file_name, line_no, "OBS 之前必须先声明 STATION", raw_line));
                        continue;
                    }
                };
                let (epoch, sat, band, code, value_tok, unit_tok) =
                    (f[1], f[2], f[3], f[4], f[5], f[6]);
                let sys: String = sat.chars().take(1).collect();
                if !known_band(&sys, band) {
                    errors.push(err(file_name, line_no, format!("OBS 未知波段 {sat}/{band}"), raw_line));
                    continue;
                }
                let kind = match code.chars().next() {
                    Some('P') => CodeKind::PseudoRange,
                    Some('L') => CodeKind::CarrierPhase,
                    Some('D') => CodeKind::Doppler,
                    Some('S') => CodeKind::Snr,
                    _ => {
                        errors.push(err(file_name, line_no, format!("OBS 观测码必须以 P/L/D/S 开头: {code}"), raw_line));
                        continue;
                    }
                };
                let unit = Unit::from_label(unit_tok);
                let measure = if value_tok == "-" || value_tok.is_empty() {
                    Measure::raw_only(value_tok.to_string(), unit.clone(), "缺测（missing）".to_string())
                } else if let Ok(v) = value_tok.parse::<f64>() {
                    Measure::numeric(v, value_tok.to_string(), unit.clone())
                } else {
                    Measure::raw_only(
                        value_tok.to_string(),
                        unit.clone(),
                        format!("无法转换为 f64，原始值保留在原位置"),
                    )
                };
                let lli = f.get(7).map(|s| s.to_string());
                let snr_tok = f.get(8).map(|s| s.to_string());
                let mut m = measure;
                if let Some(bit) = lli {
                    if bit != "0" && kind == CodeKind::CarrierPhase {
                        m.diagnostic = Some(format!("LLI={bit}（按历元原样保存，是否误报由审阅判定）"));
                    }
                }
                let _ = snr_tok;
                observations.push(Observation {
                    station: st,
                    epoch_token: epoch.to_string(),
                    time_s: parse_time(epoch),
                    sat: sat.to_string(),
                    band: band.to_string(),
                    code: code.to_string(),
                    kind,
                    measure: m,
                    source_ref: sref(raw_line),
                });
            }
            other => errors.push(err(file_name, line_no, format!("无法识别的行类型 {other}"), raw_line)),
        }
    }

    if observations.is_empty() && epoch_events.is_empty() {
        errors.push(err(file_name, 0, "文件中没有任何 OBS/EVENT 记录", ""));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ParsedBatch {
        batch_id: id,
        kind: "obx".into(),
        file_name: file_name.to_string(),
        received_at_ms: at_ms,
        observations,
        epoch_events,
        satellite_states: vec![],
        frequencies,
        raw_text: raw_text.to_string(),
    })
}

pub fn parse_sat(file_name: &str, raw_text: &str, at_ms: u64) -> Result<ParsedBatch, Vec<ParseError>> {
    let id = batch_id("sat", file_name, raw_text, at_ms);
    let mut errors = Vec::new();
    let mut states = Vec::new();
    let mut frame: Option<String> = None;

    for (idx, raw_line) in raw_text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let sref = SourceRef {
            batch_id: id.clone(),
            file_name: file_name.to_string(),
            line_no,
            raw_line: raw_line.to_string(),
        };
        match f[0] {
            "FMT" => {
                if f.get(1) != Some(&"SAT/1") {
                    errors.push(err(file_name, line_no, "不支持的格式版本（应为 SAT/1）", raw_line));
                }
            }
            "FRAME" => {
                if f.len() < 2 {
                    errors.push(err(file_name, line_no, "FRAME 缺少参考框架标识（如 ITRF2020）", raw_line));
                } else {
                    frame = Some(f[1].to_string());
                }
            }
            "SAT" => {
                if f.len() < 11 {
                    errors.push(err(
                        file_name,
                        line_no,
                        "SAT 需要: SAT <epoch> <sat> <x> <y> <z> <vx> <vy> <vz> <clock_bias> <unit>",
                        raw_line,
                    ));
                    continue;
                }
                let fr = frame.clone().unwrap_or_else(|| "UNFRAMED".to_string());
                let unit = Unit::from_label(f[10]);
                let mut meas = |tok: &str| -> Measure {
                    match tok.parse::<f64>() {
                        Ok(v) => Measure::numeric(v, tok.to_string(), unit.clone()),
                        Err(_) => Measure::raw_only(
                            tok.to_string(),
                            unit.clone(),
                            "无法转换，原始卫星状态值保留".to_string(),
                        ),
                    }
                };
                states.push(SatelliteState {
                    epoch_token: f[1].to_string(),
                    time_s: parse_time(f[1]),
                    sat: f[2].to_string(),
                    frame: fr,
                    x: meas(f[3]),
                    y: meas(f[4]),
                    z: meas(f[5]),
                    vx: meas(f[6]),
                    vy: meas(f[7]),
                    vz: meas(f[8]),
                    clock_bias: meas(f[9]),
                    source_ref: sref,
                });
            }
            other => errors.push(err(file_name, line_no, format!("无法识别的行类型 {other}"), raw_line)),
        }
    }
    if states.is_empty() {
        errors.push(err(file_name, 0, "SAT 文件没有任何 SAT 记录", ""));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ParsedBatch {
        batch_id: id,
        kind: "sat".into(),
        file_name: file_name.to_string(),
        received_at_ms: at_ms,
        observations: vec![],
        epoch_events: vec![],
        satellite_states: states,
        frequencies: vec![],
        raw_text: raw_text.to_string(),
    })
}

pub fn parse_any(file_name: &str, raw_text: &str, at_ms: u64) -> Result<ParsedBatch, Vec<ParseError>> {
    let head: String = raw_text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if head.starts_with("FMT OBX") {
        parse_obx(file_name, raw_text, at_ms)
    } else if head.starts_with("FMT SAT") {
        parse_sat(file_name, raw_text, at_ms)
    } else {
        Err(vec![ParseError {
            file_name: file_name.to_string(),
            line_no: 1,
            message: "缺少 FMT 声明（首行应为 `FMT OBX/1` 或 `FMT SAT/1`）".into(),
            raw_line: head,
        }])
    }
}
