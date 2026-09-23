//! 导入层：解析 RINEX 2.x 观测文件子集、fragment_v1 源数据修订、
//! 钟跳事件清单、卫星状态清单。原文逐字段保留；无法转换的值留 null + 诊断。

use crate::ids::hid;
use crate::model::*;
use crate::units::{calendar_to_gps, gps_axis, Datum, EpochTime, Qty, Unit};

#[derive(Debug, Clone)]
pub struct ParseOutcome {
    pub epochs: Vec<RawEpoch>,
    pub observations: Vec<RawObservation>,
    pub clock_events: Vec<ClockEventRow>,
    pub sat_states: Vec<SatStateRow>,
    pub diagnostics: Vec<DiagnosticRow>,
    /// 头中声明的观测类型序列（决定每颗星每历元的字段顺序）
    pub obs_types: Vec<(String, ObsKind, i32)>,
    pub site_marker: String,
}

pub fn epoch_id(batch_id: &str, idx: i64) -> String {
    hid("epoch", &[batch_id, &idx.to_string()])
}
pub fn obs_id(batch_id: &str, idx: i64, sat: &str, code: &str) -> String {
    hid("rawobs", &[batch_id, &idx.to_string(), sat, code])
}
fn clock_id(batch_id: &str, n: i64) -> String {
    hid("clockevt", &[batch_id, &n.to_string()])
}
fn satstate_id(batch_id: &str, n: i64) -> String {
    hid("satstate", &[batch_id, &n.to_string()])
}

fn kind_of(code: &str) -> Option<ObsKind> {
    match code.chars().next() {
        Some('C') => Some(ObsKind::Pseudorange),
        Some('L') => Some(ObsKind::Carrier),
        Some('D') => Some(ObsKind::Doppler),
        Some('S') => Some(ObsKind::Snr),
        _ => None,
    }
}
fn freq_of(code: &str) -> i32 {
    code.chars().nth(1).and_then(|c| c.to_digit(10)).unwrap_or(0) as i32
}

fn parse_qty_field(chunk: &str, unit: Unit) -> Qty {
    let raw = chunk.to_string();
    let body: String = chunk
        .chars()
        .take(14)
        .collect::<String>()
        .trim()
        .to_string();
    if body.is_empty() {
        let mut q = Qty::unparsable(unit, raw.clone(), "missing_value:字段为空白（频率/观测量缺失）");
        q.value = None;
        return q;
    }
    match body.parse::<f64>() {
        Ok(v) => Qty::parsed(v, unit, raw),
        Err(_) => Qty::unparsable(unit, raw, format!("not_a_number:{body}")),
    }
}

fn make_time(year: i32, month: u32, day: u32, hh: u32, mm: u32, ss: f64, raw: String) -> EpochTime {
    let (week, sow) = calendar_to_gps(year, month, day, hh, mm, ss);
    EpochTime {
        year, month, day, hour: hh, minute: mm, second: ss,
        gps_week: week, gps_sow: sow, axis_seconds: gps_axis(week, sow), raw,
    }
}

/// 解析 RINEX 2.x OBSERVATION DATA 文件（本地离线子集）。
/// 结构性错误（无法识别的历元头）返回 Err；单元级坏值保留为诊断。
pub fn parse_rinex_o(content: &str, batch_id: &str, project_id: &str) -> Result<ParseOutcome, String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut diags: Vec<DiagnosticRow> = Vec::new();
    let mut dcount = 0i64;
    let mut diag = |line_no: Option<i64>, code: &str, ctx: &str, msg: String, severity: &str| {
        dcount += 1;
        diags.push(DiagnosticRow {
            id: hid("diag", &[batch_id, &dcount.to_string()]),
            batch_id: batch_id.to_string(),
            project_id: project_id.to_string(),
            severity: severity.to_string(),
            code: code.to_string(),
            line_no,
            context: ctx.to_string(),
            message: msg,
        });
    };

    let mut site = String::new();
    let mut obs_types: Vec<(String, ObsKind, i32)> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if line.contains("MARKER NAME") {
            site = line.chars().take(60).collect::<String>().trim().to_string();
        }
        if line.contains("# / TYPES OF OBSERV") {
            let head: String = line.chars().take(60).collect();
            let n: i64 = head.get(..6).unwrap_or("  ").trim().parse().unwrap_or(0);
            let mut codes = Vec::new();
            for k in 0..9 {
                let start = 10 + k * 6;
                if start + 6 <= head.len() {
                    let c = head.get(start..start + 6).unwrap_or("").trim().to_string();
                    if !c.is_empty() { codes.push(c); }
                }
            }
            // 支持续行（第二行起前 6 列留空）
            let mut consumed = 1usize;
            while (codes.len() as i64) < n && i + consumed < lines.len() {
                let cont = lines[i + consumed];
                if cont.contains("TYPES OF OBSERV") {
                    let h2: String = cont.chars().take(60).collect();
                    for k in 0..9 {
                        let start = 10 + k * 6;
                        if start + 6 <= h2.len() {
                            let c = h2.get(start..start + 6).unwrap_or("").trim().to_string();
                            if !c.is_empty() { codes.push(c); }
                        }
                    }
                    consumed += 1;
                } else { break; }
            }
            for code in codes {
                match kind_of(&code) {
                    Some(kind) => obs_types.push((code, kind, freq_of(&code))),
                    None => diag(Some(i as i64 + 1), "unknown_obs_type", &code,
                                 format!("观测码 {code} 不在本解析器支持集(C/L/D/S)中，已跳过"), "warning"),
                }
            }
            i += consumed;
        }
        if line.contains("END OF HEADER") { i += 1; break; }
        i += 1;
    }
    if site.is_empty() { site = "SITE".to_string(); }
    if obs_types.is_empty() {
        return Err("format_error:缺少 RINEX 头 '# / TYPES OF OBSERV' 或观测类型为空".to_string());
    }

    let mut epochs = Vec::new();
    let mut observations = Vec::new();
    let mut epoch_index = 0i64;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() { i += 1; continue; }
        if line.len() < 32 {
            return Err(format!("format_error:第{}行不是合法历元头(长度不足)", i + 1));
        }
        // 历元头：1X,I2.2,4I3.2,F10.7, 2I3, F12.9
        let raw = line.to_string();
        let year2: i32 = line.get(1..3).unwrap_or("").trim().parse().map_err(|_| {
            format!("format_error:第{}行年份不是整数: {:?}", i + 1, line.get(1..3))
        })?;
        let month: u32 = line.get(3..6).unwrap_or("").trim().parse().map_err(|_| format!("format_error:第{}行月份", i + 1))?;
        let day: u32 = line.get(6..9).unwrap_or("").trim().parse().map_err(|_| format!("format_error:第{}行日期", i + 1))?;
        let hh: u32 = line.get(9..12).unwrap_or("").trim().parse().map_err(|_| format!("format_error:第{}行小时", i + 1))?;
        let mm: u32 = line.get(12..15).unwrap_or("").trim().parse().map_err(|_| format!("format_error:第{}行分钟", i + 1))?;
        let ss: f64 = line.get(15..26).unwrap_or("0").trim().parse().map_err(|_| format!("format_error:第{}行秒", i + 1))?;
        let flag: i32 = line.get(26..29).unwrap_or("0").trim().parse().unwrap_or(0);
        let nsat: i64 = line.get(29..32).unwrap_or("0").trim().parse().map_err(|_| format!("format_error:第{}行卫星数", i + 1))?;
        let clock_off = line.get(41..53).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        // 卫星列表：列 33.. 每 3 字符；超过 12 颗时在续行（本样例最多 9 颗，保持简单并显式报错）
        if nsat > 12 {
            return Err(format!("format_error:第{}行卫星数 {nsat}>12，本离线子集不支持卫星续行", i + 1));
        }
        let mut sats = Vec::new();
        for k in 0..nsat as usize {
            let start = 32 + k * 3;
            let s = line.get(start..start + 3).unwrap_or("").trim().to_string();
            if s.is_empty() {
                return Err(format!("format_error:第{}行第{}个卫星标识为空", i + 1, k + 1));
            }
            sats.push(normalize_sat(&s));
        }

        let year = if year2 < 80 { 2000 + year2 } else { 1900 + year2 };
        let etime = make_time(year, month, day, hh, mm, ss, raw.clone());
        let eid = epoch_id(batch_id, epoch_index);
        epochs.push(RawEpoch {
            id: eid.clone(), batch_id: batch_id.to_string(), site: site.clone(),
            index_in_batch: epoch_index, time: etime, epoch_flag: flag,
            sats: sats.clone(), raw_header: raw.clone(),
            receiver_clock_offset_raw: clock_off,
        });
        i += 1;

        // 每颗卫星的观测行：每 16 字符一单元
        for sat in &sats {
            // 一颗星一行即可容纳 5 个类型；超过 5 个需要续行处理
            let mut data_line = String::new();
            let mut need = obs_types.len();
            while need > 0 {
                if i >= lines.len() {
                    return Err(format!("format_error:历元#{epoch_index} 卫星 {sat} 观测数据提前结束"));
                }
                data_line.push_str(lines[i]);
                data_line.push('\n');
                let cap = lines[i].len() / 16;
                need = need.saturating_sub(cap);
                i += 1;
            }
            for (ti, (code, kind, fnum)) in obs_types.iter().enumerate() {
                let start = ti * 16;
                let chunk: String = data_line.chars().skip(start).take(16).collect();
                let raw_field = chunk.clone();
                let unit = match kind {
                    ObsKind::Pseudorange => Unit::Meter,
                    ObsKind::Carrier => Unit::Cycle,
                    ObsKind::Doppler => Unit::MeterPerSecond,
                    ObsKind::Snr => Unit::DbHz,
                };
                let mut qty = parse_qty_field(&chunk, unit);
                let lli = chunk.chars().nth(14);
                let snr = chunk.chars().nth(15);
                let lli_v = lli.and_then(|c| c.to_digit(10)).map(|v| v as i32);
                let snr_v = snr.and_then(|c| c.to_digit(10)).map(|v| v as i32);
                // LLI 与值缺失的组合诊断
                if qty.value.is_none() && qty.diagnostics.iter().any(|d| d.starts_with("missing_value")) {
                    diag(None, "missing_frequency",
                         &format!("{sat} {code} @epoch#{epoch_index}"),
                         "该历元此频率/观测码缺失：参与弧段断开判定，原始空格保留".into(), "warning");
                }
                if qty.value.is_none() && !qty.diagnostics.is_empty() {
                    diag(None, "unparsable_value", &raw_field,
                         qty.diagnostics.join("; "), "error");
                }
                if matches!(kind, ObsKind::Doppler) && qty.value.is_none() {
                    qty.diagnostics.push("doppler_unavailable".into());
                }
                observations.push(RawObservation {
                    id: obs_id(batch_id, epoch_index, sat, code),
                    epoch_id: eid.clone(), batch_id: batch_id.to_string(),
                    site: site.clone(), sat: sat.clone(), obs_code: code.clone(),
                    kind: *kind, frequency_number: *fnum, qty,
                    lli: lli_v, signal_strength: snr_v, raw_field,
                });
            }
        }
        epoch_index += 1;
    }

    Ok(ParseOutcome {
        epochs, observations, clock_events: vec![], sat_states: vec![],
        diagnostics: diags, obs_types, site_marker: site,
    })
}

fn normalize_sat(s: &str) -> String {
    // "G09" / " 9" 风格 -> "G09"；本样例直接给出 Gnn
    let t = s.trim();
    if t.len() == 3 { t.to_string() } else { format!("G{t:0>2}") }
}

fn empty_outcome(site: String) -> ParseOutcome {
    ParseOutcome {
        epochs: vec![], observations: vec![], clock_events: vec![],
        sat_states: vec![], diagnostics: vec![], obs_types: vec![], site_marker: site,
    }
}

fn push_diag(diags: &mut Vec<DiagnosticRow>, batch_id: &str, project_id: &str,
             line_no: Option<i64>, code: &str, ctx: &str, msg: String, severity: &str) {
    let n = diags.len() as i64 + 1;
    diags.push(DiagnosticRow {
        id: hid("diag", &[batch_id, &n.to_string()]),
        batch_id: batch_id.to_string(), project_id: project_id.to_string(),
        severity: severity.to_string(), code: code.to_string(),
        line_no, context: ctx.to_string(), message: msg,
    });
}

/// fragment_v1：源数据修订。每行
/// `# epoch_index sat obs_code new_field(<=16字符，16空格表示缺失)  reason`
/// 行式 `7  G03  C1   21345678.123   接收机钟漂后重标`
pub fn parse_fragment_v1(content: &str, batch_id: &str, project_id: &str,
                        root_batch_id: &str) -> Result<(ParseOutcome, Vec<FragmentReplacement>)>, String> {
    let mut outcome = empty_outcome("SITE".to_string());
    let mut replaces = Vec::new();
    for (ln0, line) in content.lines().enumerate() {
        let ln = ln0 as i64 + 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            if t.starts_with("# fragment_v1") && outcome.site_marker == "SITE" {
                outcome.site_marker = "SITE".to_string();
            }
            continue;
        }
        let cols: Vec<&str> = t.split_whitespace().collect();
        if cols.len() < 4 {
            push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                      "bad_fragment_line", line, "需要至少4列: epoch sat code new_field [reason]".into(), "error");
            continue;
        }
        let epoch_index: i64 = match cols[0].parse() {
            Ok(v) => v,
            Err(_) => {
                push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                          "bad_epoch_index", cols[0], "epoch_index 不是整数".into(), "error");
                continue;
            }
        };
        let sat = normalize_sat(cols[1]);
        let code = cols[2].to_uppercase();
        if kind_of(&code).is_none() {
            push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                      "unknown_obs_type", &code, "观测码不支持".into(), "error");
            continue;
        }
        // new_field 取第4列，左侧补空到 16 列（保持 RINEX 单元表示；缺测约定 16 空格）
        let token = cols[3];
        if token == "BLANK16" {
            let _ = token;
        }
        let new_field_raw = if token == "BLANK16" {
            " ".repeat(16)
        } else {
            format!("{:>16}", token)
        };
        let reason = cols.get(4..).map(|r| r.join(" ")).unwrap_or_default();
        let target = obs_id(root_batch_id, epoch_index, &sat, &code);
        replaces.push(FragmentReplacement {
            epoch_index, sat: sat.clone(), obs_code: code.clone(),
            new_field_raw: new_field_raw.clone(), target_obs_id: target.clone(),
            reason: reason.clone(),
        });
        // 片段本身不重新保存观测行；身份关系通过 target 保留。
        let _ = (ln, new_field_raw);
    }
    Ok((outcome, replaces))
}

/// clock_events_v1：整机钟跳事件清单（台站级，区别于单星周跳）。
/// 列：epoch_index  jump_seconds(带符号)  kind  note...
pub fn parse_clock_events(content: &str, batch_id: &str, project_id: &str,
                         site: &str) -> Result<ParseOutcome, String> {
    let mut outcome = empty_outcome(site.to_string());
    let mut n = 0i64;
    for (ln0, line) in content.lines().enumerate() {
        let ln = ln0 as i64 + 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let cols: Vec<&str> = t.split_whitespace().collect();
        if cols.len() < 3 {
            push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                      "bad_clock_line", line, "需要: epoch_index jump_seconds kind [note]".into(), "error");
            continue;
        }
        let eidx: i64 = cols[0].parse().map_err(|_| format!("format_error:钟跳第{ln}行 epoch_index"))?;
        let jump = match cols[1].parse::<f64>() {
            Ok(v) => Qty::parsed(v, Unit::Second, cols[1]),
            Err(_) => Qty::unparsable(Unit::Second, cols[1], "not_a_number"),
        };
        n += 1;
        outcome.clock_events.push(ClockEventRow {
            id: clock_id(batch_id, n), batch_id: batch_id.to_string(),
            site: site.to_string(), epoch_id: Some(epoch_id(batch_id, eidx)),
            at_raw: format!("#{eidx}"), at_axis_seconds: None,
            jump, event_kind: cols[2].to_string(), raw_line: line.to_string(),
        });
    }
    Ok(outcome)
}

/// sat_states_v1：卫星状态（ECEF，米，WGS84）。
/// 列：epoch_index sat x y z
pub fn parse_sat_states(content: &str, batch_id: &str, project_id: &str,
                        site: &str) -> Result<ParseOutcome, String> {
    let mut outcome = empty_outcome(site.to_string());
    let mut n = 0i64;
    for (ln0, line) in content.lines().enumerate() {
        let ln = ln0 as i64 + 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') { continue; }
        let cols: Vec<&str> = t.split_whitespace().collect();
        if cols.len() < 5 {
            push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                      "bad_satstate_line", line, "需要: epoch_index sat x y z".into(), "error");
            continue;
        }
        let eidx: i64 = match cols[0].parse() {
            Ok(v) => v,
            Err(_) => {
                push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                          "bad_epoch_index", cols[0], "epoch_index 不是整数".into(), "error");
                continue;
            }
        };
        let mk = |tok: &str| -> Qty {
            match tok.parse::<f64>() {
                Ok(v) => Qty::parsed_datum(v, Unit::Meter, Datum::Wgs84Ecef, tok),
                Err(_) => Qty::unparsable(Unit::Meter, tok, "not_a_number").with_datum(Datum::Wgs84Ecef),
            }
        };
        let (x, y, z) = (mk(cols[2]), mk(cols[3]), mk(cols[4]));
        if x.value.is_none() || y.value.is_none() || z.value.is_none() {
            push_diag(&mut outcome.diagnostics, batch_id, project_id, Some(ln),
                      "unparsable_value", line, "卫星状态坐标无法解析，保留原文与诊断".into(), "error");
        }
        n += 1;
        outcome.sat_states.push(SatStateRow {
            id: satstate_id(batch_id, n), batch_id: batch_id.to_string(),
            site: site.to_string(), sat: normalize_sat(cols[1]),
            epoch_id: Some(epoch_id(batch_id, eidx)), at_axis_seconds: None,
            x_m: x, y_m: y, z_m: z, datum: Datum::Wgs84Ecef.as_str().to_string(),
            raw_line: line.to_string(),
        });
    }
    Ok(outcome)
}
