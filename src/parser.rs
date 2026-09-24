//! 导入层：接受可复制的小型样例（Pack v1）与最小 RINEX 2.11 观测文件。
//! 伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号与观测码均按历元原样保存；
//! 规范化只发生在这里的数值转换，原始 token 始终保留；无法转换的值留在原位并附诊断。

use crate::model::*;

const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

/// 解析结果。
pub struct Parsed {
    pub payload: CommitPayload,
    /// 每个 EPOCH 槽位的逻辑键（用于版本化与 @DROP 匹配）
    pub slot_keys: Vec<String>,
}

/// 解析 Pack v1 文本。
pub fn parse_pack(text: &str) -> Result<Parsed, String> {
    let mut p = CommitPayload { format: "gsb-pack-1".to_string(), ..Default::default() };
    let mut slot_keys: Vec<String> = Vec::new();
    let mut freqs_seen = std::collections::BTreeSet::new();

    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
    let mut idx = 0;
    let mut epoch_counter = 0usize;
    while idx < lines.len() {
        let (line_no, raw_line) = lines[idx];
        let line = strip_comment(raw_line).trim_end();
        idx += 1;
        if line.trim().is_empty() {
            continue;
        }
        let head = line.split_whitespace().next().unwrap_or("");
        match head {
            "@FORMAT" | "@FREQ" | "@STATION" => {
                parse_header(head, line, line_no, &mut p).map_err(|e| format!("第 {line_no} 行: {e}"))?;
            }
            "@EPOCH" => {
                let (ep, consumed, key) = parse_epoch_block(line, line_no, &lines[idx..], epoch_counter, &p.frequencies)?;
                // 记录出现的频带顺序
                for b in &ep.bands {
                    freqs_seen.insert(b.band.clone());
                }
                slot_keys.push(key);
                p.epochs.push(ep);
                idx += consumed;
                epoch_counter += 1;
            }
            other => {
                p.orphan_lines.push(OrphanLine {
                    line_no,
                    text: raw_line.to_string(),
                    diag: format!("无法识别的指令 `{other}`（原文保留，不参与计算）"),
                });
            }
        }
    }
    if p.frequencies.is_empty() {
        return Err("缺少 @FREQ 频率定义".to_string());
    }
    Ok(Parsed { payload: p, slot_keys })
}

fn strip_comment(line: &str) -> &str {
    if let Some(i) = line.find('#') {
        &line[..i]
    } else {
        line
    }
}

fn parse_header(head: &str, line: &str, line_no: usize, p: &mut CommitPayload) -> Result<(), String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    match head {
        "@FORMAT" => {
            if toks.len() < 2 || toks[1] != "gsb-pack-1" {
                return Err(format!("@FORMAT 期望 gsb-pack-1，得到 `{}`", toks.get(1).copied().unwrap_or("")));
            }
        }
        "@FREQ" => {
            // @FREQ <band> <freq_number_raw> <frequency_MHz> <code1,code2,...>
            if toks.len() < 5 {
                return Err("@FREQ 需要: <band> <freq编号> <频率MHz> <观测码列表>".to_string());
            }
            let band = toks[1].to_string();
            let freq_number_raw = toks[2].to_string();
            let mhz: f64 = toks[3].parse().map_err(|_| format!("频率 MHz 非法: {}", toks[3]))?;
            let codes = toks[4].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>();
            if codes.is_empty() {
                return Err("@FREQ 观测码列表为空".to_string());
            }
            let hz = mhz * 1_000_000.0;
            p.frequencies.push(FrequencyDef {
                band,
                freq_number_raw,
                codes,
                frequency_hz: hz,
                wavelength_m: SPEED_OF_LIGHT_M_S / hz,
            });
        }
        "@STATION" => {
            // @STATION <id> <marker> <datum> <xraw> <yraw> <zraw>
            if toks.len() < 7 {
                return Err("@STATION 需要: <id> <marker> <datum> <X> <Y> <Z>（坐标原文保留）".to_string());
            }
            let xyz_raw = [toks[4].to_string(), toks[5].to_string(), toks[6].to_string()];
            let mut xyz = [None, None, None];
            for (i, t) in xyz_raw.iter().enumerate() {
                match t.parse::<f64>() {
                    Ok(v) => xyz[i] = Some(v),
                    Err(_) => p.diagnostics.push(ImportDiagnostic {
                        line_no,
                        severity: "warn".to_string(),
                        code: "unparseable_station_coord".to_string(),
                        message: format!("站点 {} 坐标分量无法转换，保留原文 `{t}`", toks[1]),
                    }),
                }
            }
            p.stations.push(StationMeta {
                station: toks[1].to_string(),
                marker_name: toks[2].to_string(),
                datum: toks[3].to_string(),
                xyz_m: xyz,
                xyz_raw,
            });
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// 解析 BAND 行：单位（m|cyc|hz）与值（可能缺失/不可转换）。
fn parse_band_value(raw_tok: &str, unit: &str) -> Quantity {
    let t = raw_tok.trim();
    if t.is_empty() || t == "-" || t.eq_ignore_ascii_case("MISSING") {
        Quantity::missing(t)
    } else {
        match t.parse::<f64>() {
            Ok(v) => {
                let (si, si_unit) = match unit {
                    "m" => (v, "m"),
                    "km" => (v * 1000.0, "m"),
                    "cyc" | "cycle" => (v, "cycle"),
                    "hz" => (v, "Hz"),
                    "khz" => (v * 1000.0, "Hz"),
                    other => (v, other),
                };
                Quantity::ok(si, t, si_unit)
            }
            Err(_) => Quantity::unparseable(t, format!("无法按单位 `{unit}` 转换为数值")),
        }
    }
}

fn parse_epoch_block(
    head_line: &str,
    head_line_no: usize,
    rest: &[(usize, &str)],
    epoch_counter: usize,
    freqs: &[FrequencyDef],
) -> Result<(EpochObs, usize, String), String> {
    let _ = epoch_counter;
    // @EPOCH <station> <sat> <sat_status> YYYY MM DD HH MM SS.S
    let htoks: Vec<&str> = head_line.split_whitespace().collect();
    if htoks.len() < 10 {
        return Err(format!("@EPOCH 头需要 站点 卫星 状态 + 6 个时间字段，得到 {} 个 token", htoks.len()));
    }
    let station = htoks[1].to_string();
    let sat = htoks[2].to_string();
    let sat_status_raw = htoks[3].to_string();
    let time = parse_epoch(&htoks[4..10]).map_err(|e| format!("@EPOCH 时间: {e}"))?;

    let mut bands: Vec<BandObs> = Vec::new();
    let mut clock_event: Option<ClockEvent> = None;
    let mut consumed = 0usize;
    let mut source_lines = vec![head_line.to_string()];
    let mut bands_present = std::collections::BTreeSet::new();

    for (i, (lno, lraw)) in rest.iter().enumerate() {
        let l = strip_comment(lraw).trim_end();
        if l.trim().is_empty() {
            consumed = i + 1;
            continue;
        }
        let t0 = l.split_whitespace().next().unwrap_or("");
        if t0.starts_with('@') {
            break;
        }
        consumed = i + 1;
        source_lines.push(lraw.to_string());
        let parts: Vec<&str> = l.split_whitespace().collect();
        match parts[0] {
            "BAND" => {
                if parts.len() < 7 {
                    return Err(format!("第 {lno} 行: BAND 需要 频带 P L D LLI SNR [单位]"));
                }
                let band = parts[1].to_string();
                if !freqs.iter().any(|f| f.band == band) {
                    return Err(format!("第 {lno} 行: 频带 `{band}` 未在 @FREQ 中定义"));
                }
                if !bands_present.insert(band.clone()) {
                    return Err(format!("第 {lno} 行: 历元内频带 `{band}` 重复"));
                }
                let unit = parts.get(7).copied().unwrap_or("m");
                let missing_all =
                    is_missing_tok(parts[2]) && is_missing_tok(parts[3]) && is_missing_tok(parts[4]) && is_missing_tok(parts[6]);
                bands.push(BandObs {
                    band,
                    pseudorange: Some(parse_band_value(parts[2], "m")),
                    carrier: Some(parse_band_value(parts[3], "cyc")),
                    doppler: Some(parse_band_value(parts[4], "hz")),
                    lli_raw: parts[5].to_string(),
                    snr: Some(parse_band_value(parts[6], "dBHz")),
                    missing: missing_all,
                });
            }
            "CLOCK" => {
                // CLOCK <kind_raw> <amount_raw> <unit_raw>
                if parts.len() < 4 {
                    return Err(format!("第 {lno} 行: CLOCK 需要 <类型> <幅度> <单位>"));
                }
                let amount_raw = parts[2].to_string();
                let unit_raw = parts[3].to_string();
                let (amount_ns, status) = match (amount_raw.parse::<f64>(), unit_raw.as_str()) {
                    (Ok(v), "ns") => (Some(v), ValueStatus::Ok),
                    (Ok(v), "us") => (Some(v * 1000.0), ValueStatus::Ok),
                    (Ok(v), "ms") => (Some(v * 1_000_000.0), ValueStatus::Ok),
                    (Ok(_), u) => (None, ValueStatus::Unparseable { diag: format!("未知钟跳单位 `{u}`，保留原文") }),
                    (Err(_), _) => (None, ValueStatus::Unparseable { diag: format!("钟跳幅度无法转换: `{amount_raw}`") }),
                };
                clock_event = Some(ClockEvent {
                    epoch_index: epoch_counter,
                    kind_raw: parts[1].to_string(),
                    amount_raw,
                    unit_raw,
                    amount_ns,
                    status,
                    source_line: lraw.to_string(),
                });
            }
            other => {
                return Err(format!("第 {lno} 行: @EPOCH 块内无法识别 `{other}`"));
            }
        }
    }

    let key = epoch_slot_key(&station, &sat, &time.iso);
    Ok((
        EpochObs {
            epoch_index: epoch_counter,
            station,
            sat,
            time,
            bands,
            clock_event,
            sat_status_raw,
            source_lines,
        },
        consumed,
        key,
    ))
}

fn is_missing_tok(t: &str) -> bool {
    t.trim().is_empty() || t == "-" || t.eq_ignore_ascii_case("MISSING")
}

pub fn epoch_slot_key(station: &str, sat: &str, epoch_iso: &str) -> String {
    format!("{station}|{sat}|{epoch_iso}")
}

// ============================ RINEX 2.11（最小子集） ============================
//
// 支持：
//   头部 `# / TYPES OF OBSERV`、`APPROX POSITION XYZ`、`MARKER NAME`、`SYS / # / OBS TYPES` 行原文保留；
//   历元记录首行 `year mon day hour min sec epoch-flag numsat [sat ids...]`；
//   观测按每 16 列读取 `value LLI SSI`。
// 只接受 GPS（G）频带 1/2/5，未覆盖的系统/类型保留为孤立行与诊断，绝不丢弃。

#[derive(Debug, Clone)]
struct RinType {
    band: String,
    kind: RinKind,
    code: String,
}

#[derive(Debug, Clone, Copy)]
enum RinKind {
    P,
    L,
    D,
    S,
}

/// 解析 RINEX 2.11 观测文件（最小子集）。
pub fn parse_rinex(text: &str) -> Result<Parsed, String> {
    let mut p = CommitPayload { format: "rinex-2.11".to_string(), ..Default::default() };
    let mut slot_keys: Vec<String> = Vec::new();
    let mut obs_types: Vec<RinType> = Vec::new();
    let mut station = String::from("SITE");
    let mut marker = String::new();
    let mut xyz = [None, None, None];
    let mut xyz_raw = [String::new(), String::new(), String::new()];

    let lines: Vec<(usize, String)> = text.lines().map(|l| (0usize, l.to_string())).collect();
    let mut i = 0;
    let mut in_header = true;
    let mut epoch_counter = 0usize;

    while i < lines.len() {
        let (_, line) = &lines[i];
        let lineno = i + 1;
        if in_header {
            if line.contains("END OF HEADER") {
                in_header = false;
                i += 1;
                continue;
            }
            if line.contains("TYPES OF OBSERV") {
                // # / TYPES OF OBSERV 或 SYS / # / OBS TYPES（2.11 GPS 常见无系统前缀）
                let data = line.get(..60).unwrap_or(line);
                let n_tok = data.get(..6).unwrap_or("  ").trim();
                let mut n: usize = n_tok.parse().unwrap_or(0);
                let mut chunks: Vec<String> = Vec::new();
                for c in (10..60).step_by(6) {
                    if let Some(s) = data.get(c..c + 4) {
                        let t = s.trim().to_string();
                        if !t.is_empty() {
                            chunks.push(t);
                        }
                    }
                }
                // 头部跨多行时，后续行以续接形式出现（本最小实现把所有 TYPES 行拼接）
                if obs_types.is_empty() && n == 0 {
                    n = chunks.len();
                }
                for t in chunks.into_iter().take(n.max(obs_types.len()) + 16) {
                    if let Some(rt) = decode_obs_type(&t) {
                        obs_types.push(rt);
                    } else {
                        p.diagnostics.push(ImportDiagnostic {
                            line_no: lineno,
                            severity: "info".to_string(),
                            code: "unsupported_obs_type".to_string(),
                            message: format!("观测码 `{t}` 不在最小子集（C/L/D/S 1/2/5），列位保留"),
                        });
                    }
                }
                // 去重并保留顺序
                obs_types.dedup_by(|a, b| a.code == b.code);
            }
            if line.contains("APPROX POSITION XYZ") {
                for (k, c) in (0..60).step_by(14).enumerate() {
                    if k >= 3 {
                        break;
                    }
                    let raw = line.get(c..c + 14).unwrap_or("").trim().to_string();
                    xyz_raw[k] = raw.clone();
                    xyz[k] = raw.parse::<f64>().ok();
                }
            }
            if line.contains("MARKER NAME") {
                marker = line.get(..60).unwrap_or("").trim().to_string();
            }
            i += 1;
            continue;
        }

        // 历元首行：固定列 1X,I5 等，采用更宽松的空白解析但卫星 ID 用 3 字符列
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            i += 1;
            continue;
        }
        let header_toks: Vec<&str> = trimmed.split_whitespace().collect();
        if header_toks.len() < 8 {
            p.orphan_lines.push(OrphanLine { line_no: lineno, text: line.clone(), diag: "历元首行字段不足，原文保留".to_string() });
            i += 1;
            continue;
        }
        let epoch_tokens = &header_toks[0..6];
        let time = parse_epoch(epoch_tokens).map_err(|e| format!("第 {lineno} 行 历元: {e}"))?;
        let eflag = header_toks[6].to_string();
        let n_sat: usize = header_toks[7].parse().unwrap_or(0);
        let mut sat_ids: Vec<String> = Vec::new();
        let mut hdr_consumed = 1usize;
        // 卫星 ID：首行从列 32 起每列 3 字符
        if let Some(satfield) = trimmed.get(32..) {
            for c in (0..(n_sat * 3)).step_by(3) {
                if let Some(s) = satfield.get(c..c + 3) {
                    let s = s.trim();
                    if !s.is_empty() {
                        sat_ids.push(normalize_sat_id(s));
                    }
                }
            }
        }
        // 卫星 ID 可能续行
        while sat_ids.len() < n_sat {
            if let Some((_, next)) = lines.get(i + hdr_consumed) {
                if let Some(satfield) = next.get(32..) {
                    for c in (0..(n_sat * 3)).step_by(3) {
                        if let Some(s) = satfield.get(c..c + 3) {
                            let s = s.trim();
                            if !s.is_empty() {
                                sat_ids.push(normalize_sat_id(s));
                            }
                        }
                    }
                }
                hdr_consumed += 1;
            } else {
                break;
            }
        }

        // 非普通历元标志（2..5）在最小子集中原地保留为孤立行并跳过后续对应行
        if eflag != "0" && eflag != "1" {
            p.orphan_lines.push(OrphanLine {
                line_no: lineno,
                text: line.clone(),
                diag: format!("RINEX 历元标志 `{eflag}`（特殊事件），最小子集不展开，原文保留"),
            });
            // 跳过该历元观测行（尽力估计）
            let obs_lines = (n_sat * obs_types.len() + 4) / 5;
            i += hdr_consumed + obs_lines;
            continue;
        }

        // 每个卫星 ceil(n_types/5) 行、每行 5 个观测、每个观测 16 列
        let lines_per_sat = (obs_types.len() + 4) / 5;
        for (si, sat) in sat_ids.iter().enumerate().take(n_sat) {
            let mut per_band: std::collections::BTreeMap<String, BandObs> = std::collections::BTreeMap::new();
            let mut src = vec![line.clone()];
            for li in 0..lines_per_sat {
                let abs = i + hdr_consumed + si * lines_per_sat + li;
                let obs_line = match lines.get(abs) {
                    Some((_, l)) => l.clone(),
                    None => break,
                };
                src.push(obs_line.clone());
                for oi in 0..5 {
                    let type_index = li * 5 + oi;
                    if type_index >= obs_types.len() {
                        break;
                    }
                    let col = oi * 16;
                    let field = obs_line.get(col..col + 16).unwrap_or("");
                    let val_raw = field.get(..14).unwrap_or("").trim().to_string();
                    let lli = field.get(14..15).unwrap_or("").trim().to_string();
                    let ssi = field.get(15..16).unwrap_or("").trim().to_string();
                    let rt = &obs_types[type_index];
                    let bo = per_band.entry(rt.band.clone()).or_insert_with(|| BandObs {
                        band: rt.band.clone(),
                        ..Default::default()
                    });
                    match rt.kind {
                        RinKind::P => bo.pseudorange = Some(rin_quantity(&val_raw, "m")),
                        RinKind::L => bo.carrier = Some(rin_quantity(&val_raw, "cycle")),
                        RinKind::D => bo.doppler = Some(rin_quantity(&val_raw, "Hz")),
                        RinKind::S => bo.snr = Some(rin_quantity(&val_raw, "dBHz")),
                    }
                    if !lli.is_empty() {
                        bo.lli_raw = lli;
                    }
                    let _ = ssi;
                }
            }
            if per_band.is_empty() {
                continue;
            }
            let bands: Vec<BandObs> = per_band
                .into_values()
                .map(|mut b| {
                    b.missing = b.pseudorange.as_ref().map_or(true, |q| q.status == ValueStatus::Missing)
                        && b.carrier.as_ref().map_or(true, |q| q.status == ValueStatus::Missing);
                    b
                })
                .collect();
            let key = epoch_slot_key(&station, sat, &time.iso);
            slot_keys.push(key);
            p.epochs.push(EpochObs {
                epoch_index: epoch_counter,
                station: station.clone(),
                sat: sat.clone(),
                time: time.clone(),
                bands,
                clock_event: None,
                sat_status_raw: eflag.clone(),
                source_lines: src,
            });
            epoch_counter += 1;
        }
        i += hdr_consumed + n_sat * lines_per_sat;
    }

    if obs_types.is_empty() {
        return Err("RINEX 头部缺少 TYPES OF OBSERV".to_string());
    }
    // 由观测码反推频率定义（频率编号：RINEX 频带字符）
    let mut seen = std::collections::BTreeSet::new();
    for rt in &obs_types {
        if seen.insert(rt.band.clone()) {
            if let Some((mhz, codes_seed)) = gps_band_meta(&rt.band) {
                let hz = mhz * 1_000_000.0;
                p.frequencies.push(FrequencyDef {
                    band: rt.band.clone(),
                    freq_number_raw: rt.band.chars().last().map(|c| c.to_string()).unwrap_or_default(),
                    codes: codes_seed,
                    frequency_hz: hz,
                    wavelength_m: SPEED_OF_LIGHT_M_S / hz,
                });
            }
        }
    }
    p.stations.push(StationMeta {
        station: station.clone(),
        marker_name: marker,
        xyz_m: xyz,
        datum: "ITRF/RINEX-APPROX".to_string(),
        xyz_raw,
    });
    p.raw_blob = text.to_string();
    Ok(Parsed { payload: p, slot_keys })
}

fn normalize_sat_id(s: &str) -> String {
    // RINEX：系统字符 + 两位 PRN（GPS 可省略 G）
    let s = s.trim();
    if let Some(c) = s.chars().next() {
        if c.is_ascii_alphabetic() {
            return s.to_string();
        }
    }
    format!("G{s}")
}

fn rin_quantity(raw: &str, unit: &str) -> Quantity {
    if raw.is_empty() {
        Quantity::missing(raw)
    } else {
        match raw.parse::<f64>() {
            Ok(v) => Quantity::ok(v, raw, unit),
            Err(_) => Quantity::unparseable(raw, format!("RINEX 观测量无法按 `{unit}` 转换")),
        }
    }
}

fn decode_obs_type(t: &str) -> Option<RinType> {
    let mut chars = t.chars();
    let kind_c = chars.next()?;
    let band_c = chars.next()?;
    let code = t.to_string();
    let kind = match kind_c {
        'C' | 'P' => RinKind::P,
        'L' => RinKind::L,
        'D' => RinKind::D,
        'S' => RinKind::S,
        _ => return None,
    };
    let band = match band_c {
        '1' => "L1",
        '2' => "L2",
        '5' => "L5",
        _ => return None,
    };
    Some(RinType { band: band.to_string(), kind, code })
}

fn gps_band_meta(band: &str) -> Option<(f64, Vec<String>)> {
    match band {
        "L1" => Some((1575.42, vec!["C1C".into(), "L1C".into(), "D1C".into(), "S1C".into()])),
        "L2" => Some((1227.60, vec!["C2W".into(), "L2W".into(), "D2W".into(), "S2W".into()])),
        "L5" => Some((1176.45, vec!["C5X".into(), "L5X".into(), "D5X".into(), "S5X".into()])),
        _ => None,
    }
}
