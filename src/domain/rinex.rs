//! RINEX 3 观测文件解析。仅把原始表示规范化为带单位的领域量；无法转换的值留在原位置并附诊断。

use super::model::*;
use chrono::NaiveDateTime;

/// RINEX 观测码 -> (类型, 频点)。表版本随算法版本记录。
pub fn classify_code(code: &str) -> Option<(ObsKind, u8)> {
    let c = code.trim();
    if c.len() != 3 {
        return None;
    }
    let mut bytes = c.bytes();
    let first = bytes.next()?;
    let band = bytes.next()?;
    if !(b'1'..=b'9').contains(&band) {
        return None;
    }
    let band = band - b'0';
    let kind = match first {
        b'C' | b'P' => ObsKind::Pseudorange,
        b'L' => ObsKind::Phase,
        b'D' => ObsKind::Doppler,
        _ => return None,
    };
    Some((kind, band))
}

fn err(code: &str, msg: impl Into<String>, line_no: usize, ctx: impl Into<String>) -> Diagnostic {
    Diagnostic {
        level: DiagLevel::Error,
        code: code.to_string(),
        message: msg.into(),
        line_no: Some(line_no),
        context: Some(ctx.into()),
    }
}
fn warn(code: &str, msg: impl Into<String>, line_no: usize, ctx: impl Into<String>) -> Diagnostic {
    Diagnostic {
        level: DiagLevel::Warning,
        code: code.to_string(),
        message: msg.into(),
        line_no: Some(line_no),
        context: Some(ctx.into()),
    }
}

/// 解析一个 14 字符的 RINEX3 观测字段（14,1,1：值 / LLI / SNR）。
fn parse_obs_field(raw16: &str, code: &str, line_no: usize, diags: &mut Vec<Diagnostic>) -> Option<ObsField> {
    let raw = raw16.to_string();
    let value_part = raw16.get(0..14).unwrap_or("").trim().to_string();
    if value_part.is_empty() {
        return None; // 缺失字段（合法）
    }
    let (kind, band_opt) = match classify_code(code) {
        Some((k, b)) => (k, Some(b)),
        None => (ObsKind::Pseudorange, None),
    };
    let mut field = ObsField {
        code: code.to_string(),
        kind,
        band: band_opt,
        raw_value: value_part.clone(),
        value_si: None,
        lli: None,
        snr_flag: None,
        diagnostics: vec![],
    };
    if band_opt.is_none() {
        field.diagnostics.push(format!("unknown_obs_code:{code}"));
        diags.push(warn("OBS_CODE_UNKNOWN", format!("观测码 {code} 不在频率表中，原始值保留"), line_no, raw.clone()));
    }
    let lli_ch = raw16.get(14..15).unwrap_or(" ").trim();
    let snr_ch = raw16.get(15..16).unwrap_or(" ").trim();
    if let Ok(v) = lli_ch.parse::<u8>() {
        field.lli = Some(v);
    }
    if let Ok(v) = snr_ch.parse::<u8>() {
        field.snr_flag = Some(v);
    }
    match value_part.parse::<f64>() {
        Ok(v) if v.is_finite() => field.value_si = Some(v),
        _ => {
            field.diagnostics.push("unparseable_numeric".into());
            diags.push(err(
                "OBS_VALUE_BAD",
                format!("观测码 {code} 的值 `{value_part}` 无法解析，保留原始表示"),
                line_no,
                raw,
            ));
        }
    }
    Some(field)
}

fn epoch_from_line(line: &str, line_no: usize, diags: &mut Vec<Diagnostic>) -> Option<(NaiveDateTime, String, String, usize)> {
    // > yyyy mm dd hh mm ss.fff  flag num  [rcv clock offset]
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 {
        diags.push(err("EPOCH_LINE_SHORT", "历元行字段不足", line_no, line.to_string()));
        return None;
    }
    let nums: Vec<f64> = parts[1..7].iter().filter_map(|p| p.parse::<f64>().ok()).collect();
    if nums.len() != 6 {
        diags.push(err("EPOCH_TIME_BAD", "历元时间分量无法解析", line_no, line.to_string()));
        return None;
    }
    let sec_int = nums[5] as u32;
    let millis = ((nums[5] - sec_int as f64) * 1000.0).round() as u32;
    let date = chrono::NaiveDate::from_ymd_opt(nums[0] as i32, nums[1] as u32, nums[2] as u32);
    let time = chrono::NaiveTime::from_hms_milli_opt(nums[3] as u32, nums[4] as u32, sec_int, millis);
    let ndt = match (date, time) {
        (Some(d), Some(t)) => d.and_time(t),
        _ => {
            diags.push(err("EPOCH_TIME_BAD", "历元时间越界", line_no, line.to_string()));
            return None;
        }
    };
    let nsat: usize = match parts[7].parse() {
        Ok(n) => n,
        Err(_) => {
            diags.push(err("EPOCH_NSAT_BAD", "卫星数无法解析", line_no, line.to_string()));
            return None;
        }
    };
    let clock_raw = if parts.len() >= 9 {
        line.split_whitespace().nth(8).unwrap_or("").to_string()
    } else {
        String::new()
    };
    Some((ndt, parts[6].to_string(), clock_raw, nsat))
}

pub fn parse_rinex3(text: &str) -> ParsedImport {
    let mut diags = Vec::new();
    let mut meta = ImportMeta {
        rinex_version: String::new(),
        ..Default::default()
    };
    let mut type_map: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    let mut epochs: Vec<EpochRecord> = Vec::new();
    let lines: Vec<(usize, &str)> = text.lines().enumerate().map(|(i, l)| (i + 1, l)).collect();
    let mut i = 0;
    while i < lines.len() {
        let (ln, line) = lines[i];
        if line.len() >= 60 {
            let label = line[60..].trim();
            match label {
                "RINEX VERSION / TYPE" => meta.rinex_version = line[0..20].trim().to_string(),
                "MARKER NAME" => meta.marker_name = line[0..60].trim().to_string(),
                "OBSERVER / AGENCY" => meta.observer_agency = line[0..60].trim().to_string(),
                "REC # / TYPE / VERS" => meta.receiver = line[0..60].trim().to_string(),
                "ANT # / TYPE" => meta.antenna = line[0..60].trim().to_string(),
                "APPROX POSITION XYZ" => {
                    meta.approx_xyz_raw = [
                        line[0..14].trim().to_string(),
                        line[14..28].trim().to_string(),
                        line[28..42].trim().to_string(),
                    ];
                    for r in meta.approx_xyz_raw.iter() {
                        match r.parse::<f64>() {
                            Ok(v) => meta.approx_xyz_m.push(v),
                            Err(_) => diags.push(warn("XYZ_BAD", format!("站坐标 `{r}` 无法换算为米"), ln, r.clone())),
                        }
                    }
                    meta.approx_datum = "ITRF-like/ECEF".into();
                }
                "SYS / # / OBS TYPES" => {
                    let sys = line[0..1].trim().to_string();
                    let n: usize = line[4..6].trim().parse().unwrap_or(0);
                    let mut codes = Vec::new();
                    for k in 0..n {
                        let start = 7 + k * 4;
                        if start + 3 <= line.len() {
                            codes.push(line[start..start + 3].trim().to_string());
                        }
                    }
                    type_map.entry(sys).or_default().extend(codes);
                }
                "END OF HEADER" => {
                    let all: Vec<String> = type_map.values().flat_map(|v| v.iter().cloned()).collect();
                    meta.types_observed_raw = all;
                    i += 1;
                    break;
                }
                _ => {}
            }
        }
        i += 1;
    }

    let mut file_idx = 0usize;
    let t0 = loop {
        if i >= lines.len() {
            if epochs.is_empty() {
                diags.push(err("NO_EPOCH", "文件没有任何历元", 0, String::new()));
            }
            return ParsedImport { meta, epochs, diagnostics: diags };
        }
        let line = lines[i].1;
        if line.starts_with('>') {
            match epoch_from_line(line, lines[i].0, &mut diags) {
                Some((ndt, _flag, _clk, _n)) => break ndt,
                None => {
                    i += 1;
                    continue;
                }
            }
        }
        i += 1;
    };

    while i < lines.len() {
        let (eln, eline) = lines[i];
        if !eline.starts_with('>') {
            i += 1;
            continue;
        }
        let (ndt, flag_raw, clock_raw, nsat) = match epoch_from_line(eline, eln, &mut diags) {
            Some(v) => v,
            None => {
                i += 1;
                continue;
            }
        };
        if flag_raw != "0" {
            diags.push(Diagnostic {
                level: DiagLevel::Info,
                code: "EPOCH_FLAG_NONZERO".into(),
                message: format!("历元标志={flag_raw}，按原样保留标志并继续解析观测"),
                line_no: Some(eln),
                context: Some(eline.to_string()),
            });
        }
        let mut clock_offset_s = None;
        if !clock_raw.trim().is_empty() {
            match clock_raw.trim().parse::<f64>() {
                Ok(v) => clock_offset_s = Some(v),
                Err(_) => diags.push(warn("CLOCK_BAD", "历元钟差无法换算为秒", eln, clock_raw.clone())),
            }
        }
        let mut sats: Vec<SatEpoch> = Vec::with_capacity(nsat);
        i += 1;
        for _ in 0..nsat {
            if i >= lines.len() {
                diags.push(err("SAT_BLOCK_SHORT", "卫星观测块提前结束", eln, eline.to_string()));
                break;
            }
            let (sln, sline) = lines[i];
            i += 1;
            if sline.len() < 3 {
                diags.push(err("SAT_LINE_SHORT", "卫星观测行过短", sln, sline.to_string()));
                continue;
            }
            let sat = sline[0..3].trim().to_string();
            let sys = sat.chars().next().unwrap_or(' ').to_string();
            let codes = type_map.get(&sys).cloned().unwrap_or_default();
            let mut fields = Vec::new();
            for (k, code) in codes.iter().enumerate() {
                let start = 3 + k * 16;
                let chunk = if start + 16 <= sline.len() {
                    &sline[start..start + 16]
                } else if start < sline.len() {
                    &sline[start..]
                } else {
                    ""
                };
                if let Some(f) = parse_obs_field(chunk, code, sln, &mut diags) {
                    fields.push(f);
                }
            }
            sats.push(SatEpoch { sat, fields });
        }
        let epoch = EpochRecord {
            epoch_index: epochs.len(),
            index_in_file: file_idx,
            iso: ndt.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            t_seconds: Quantity::new((ndt - t0).num_milliseconds() as f64 / 1000.0, "second"),
            epoch_flag_raw: flag_raw,
            clock_offset_raw: clock_raw,
            clock_offset_s,
            sats,
        };
        epochs.push(epoch);
        file_idx += 1;
    }

    if epochs.is_empty() {
        diags.push(err("NO_EPOCH", "文件没有任何有效历元", 0, String::new()));
    }
    ParsedImport { meta, epochs, diagnostics: diags }
}
