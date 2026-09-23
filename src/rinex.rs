//! RINEX 3 观测文件子集解析。原始字段按历元原样保存; 结构性错误整批拒绝,
//! 值级错误保留原值并附诊断, 不为派生计算丢掉任何字段。
use crate::id::content_id;
use crate::model::{standard_freq_hz, unit_of, Diag, EpochRecord, ObsRow};

pub struct ParsedFile {
    pub marker: String,
    pub epochs: Vec<EpochRecord>,
    pub obs: Vec<ObsRow>,
    pub diagnostics: Vec<Diag>,
}

fn diag(batch: &str, line_no: usize, severity: &str, code: &str, message: &str, raw: &str) -> Diag {
    let id = content_id("dgn", &(batch, line_no as i64, code, message));
    Diag {
        id,
        batch_id: batch.into(),
        line_no: line_no as i64,
        severity: severity.into(),
        code: code.into(),
        message: message.into(),
        raw_line: raw.chars().take(120).collect(),
    }
}

/// 儒略历无关的民用日期 -> GPS (week, tow)。确定性好算, 不依赖外部库。
fn civil_to_gps(y: i64, m: i64, d: i64, h: i64, mi: i64, s: f64) -> Option<(i64, f64)> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 24 || mi > 59 || !(0.0..=61.0).contains(&s) {
        return None;
    }
    // days-from-civil (Howard Hinnant)
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    // 1980-01-06 的 days-from-civil:
    let gps0 = {
        let y_adj = 1979_i64;
        let era = y_adj / 400;
        let yoe = y_adj - era * 400;
        let mp = (1 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + 6 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    };
    let gps_days = days - gps0;
    if gps_days < 0 {
        return None;
    }
    let week = gps_days / 7;
    let tow = (gps_days % 7) as f64 * 86_400.0 + h as f64 * 3600.0 + mi as f64 * 60.0 + s;
    Some((week, tow))
}

/// batch_id 参与行 id, 保证同文件在不同批次版本中身份独立且稳定。
pub fn parse(text: &str, batch_id: &str) -> Result<ParsedFile, Vec<Diag>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut diagnostics = Vec::new();
    let mut marker = String::new();
    let mut obs_types: Vec<String> = Vec::new();
    let mut header_end = None;

    for (i, line) in lines.iter().enumerate() {
        let label = if line.len() > 60 { line[60..].trim().to_string() } else { String::new() };
        let content: String = line.chars().take(60).collect();
        if label.starts_with("RINEX VERSION / TYPE") {
            let ver: f64 = content[0..9].trim().parse().unwrap_or(0.0);
            if ver < 3.0 {
                diagnostics.push(diag(batch_id, i + 1, "error", "unsupported_rinex_version",
                    "仅支持 RINEX 3.x 观测文件", line));
            }
        } else if label.starts_with("MARKER NAME") {
            marker = content.trim().to_string();
        } else if label.starts_with("SYS / # / OBS TYPES") {
            let tokens: Vec<&str> = content.split_whitespace().collect();
            if tokens.len() >= 2 {
                let n: usize = tokens[1].parse().unwrap_or(0);
                for t in tokens.iter().skip(2).take(n) {
                    obs_types.push(t.to_string());
                }
            }
        } else if label.starts_with("END OF HEADER") {
            header_end = Some(i);
            break;
        }
    }

    let mut structural = |diagnostics: &mut Vec<Diag>| {
        if header_end.is_none() {
            diagnostics.push(diag(batch_id, 0, "error", "missing_end_of_header",
                "缺少 END OF HEADER 记录", ""));
        }
        if obs_types.is_empty() {
            diagnostics.push(diag(batch_id, 0, "error", "missing_obs_types",
                "缺少 SYS / # / OBS TYPES 记录", ""));
        }
    };
    structural(&mut diagnostics);
    if diagnostics.iter().any(|d| d.severity == "error") {
        return Err(diagnostics);
    }
    let header_end = header_end.unwrap();

    let mut epochs: Vec<EpochRecord> = Vec::new();
    let mut obs: Vec<ObsRow> = Vec::new();
    let mut i = header_end + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if !line.starts_with('>') {
            return Err(vec![diag(batch_id, i + 1, "error", "expected_epoch_record",
                "期望历元记录行(以 > 开头)", line)]);
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 8 {
            return Err(vec![diag(batch_id, i + 1, "error", "malformed_epoch_record",
                "历元记录字段不足", line)]);
        }
        let parsed = (|| {
            let y: i64 = tokens[1].parse().ok()?;
            let mo: i64 = tokens[2].parse().ok()?;
            let d: i64 = tokens[3].parse().ok()?;
            let h: i64 = tokens[4].parse().ok()?;
            let mi: i64 = tokens[5].parse().ok()?;
            let s: f64 = tokens[6].parse().ok()?;
            let flag: i64 = tokens[7].parse().ok()?;
            let nsat: i64 = tokens.get(8).and_then(|t| t.parse().ok()).unwrap_or(0);
            let clk_raw = tokens.get(9).map(|t| t.to_string());
            let clk_s = tokens.get(9).and_then(|t| t.parse::<f64>().ok());
            Some((y, mo, d, h, mi, s, flag, nsat, clk_raw, clk_s))
        })();
        let (y, mo, d, h, mi, s, flag, nsat, clk_raw, clk_s) = match parsed {
            Some(v) => v,
            None => {
                return Err(vec![diag(batch_id, i + 1, "error", "malformed_epoch_record",
                    "历元记录日期/标志字段不可解析", line)]);
            }
        };
        let (week, tow) = match civil_to_gps(y, mo, d, h, mi, s) {
            Some(v) => v,
            None => {
                return Err(vec![diag(batch_id, i + 1, "error", "invalid_epoch_datetime",
                    "历元日期时间非法", line)]);
            }
        };
        let epoch_id = content_id("epo", &(batch_id, week, (tow * 1000.0).round() as i64));
        epochs.push(EpochRecord {
            id: epoch_id,
            batch_id: batch_id.into(),
            week,
            tow,
            flag,
            num_sats: nsat,
            rcv_clk_offset_raw: clk_raw,
            rcv_clk_offset_s: clk_s,
        });
        i += 1;
        for _ in 0..nsat {
            if i >= lines.len() {
                return Err(vec![diag(batch_id, i, "error", "truncated_satellite_block",
                    "卫星观测行不足, 文件被截断", "")]);
            }
            let sline = lines[i];
            if sline.starts_with('>') || sline.len() < 3 {
                return Err(vec![diag(batch_id, i + 1, "error", "malformed_satellite_record",
                    "卫星观测行非法", sline)]);
            }
            let sat: String = sline.chars().take(3).collect();
            let field_area: String = sline.chars().skip(3).collect();
            let nfields = field_area.len() / 16;
            if nfields < obs_types.len() {
                diagnostics.push(diag(batch_id, i + 1, "warning", "short_satellite_record",
                    "卫星观测行字段数少于头部声明, 缺失字段按空处理", sline));
            }
            for (k, code) in obs_types.iter().enumerate() {
                let start = k * 16;
                if start + 16 > field_area.len() {
                    break;
                }
                let field = &field_area[start..start + 16];
                let value_str = field[0..14].trim();
                if value_str.is_empty() {
                    continue; // 频率缺失等: 该字段本就不存在, 不生成行
                }
                let lli = field[14..15].trim().parse::<i64>().ok();
                let ssi = field[15..16].trim().parse::<i64>().ok();
                let value = value_str.parse::<f64>().ok();
                let mut row_diag: Option<String> = None;
                if value.is_none() {
                    row_diag = Some("unparseable_value".into());
                    diagnostics.push(diag(batch_id, i + 1, "warning", "unparseable_value",
                        "观测值不可解析, 原值保留在 raw 字段", sline));
                }
                let unit = unit_of(code).to_string();
                let mut row_diag = row_diag;
                if unit.is_empty() && row_diag.is_none() {
                    row_diag = Some("unknown_obs_code".into());
                }
                let (freq_hz, freq_origin) = match standard_freq_hz(&sat, code) {
                    Some((f, o)) => (Some(f), Some(o)),
                    None => {
                        if code.starts_with('L') && row_diag.is_none() {
                            row_diag = Some("unknown_frequency_band".into());
                        }
                        (None, None)
                    }
                };
                let row_id = content_id("obs", &(
                    batch_id, week, (tow * 1000.0).round() as i64, &sat, code, value_str));
                obs.push(ObsRow {
                    id: row_id,
                    batch_id: batch_id.into(),
                    week,
                    tow,
                    sat: sat.clone(),
                    code: code.clone(),
                    raw: value_str.to_string(),
                    lli,
                    ssi,
                    value,
                    unit,
                    freq_hz,
                    freq_origin,
                    diag: row_diag,
                });
            }
            i += 1;
        }
    }
    Ok(ParsedFile { marker, epochs, obs, diagnostics })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch_mapping() {
        let (week, tow) = civil_to_gps(2026, 12, 24, 0, 0, 0.0).unwrap();
        assert_eq!((week, tow), (2450, 345600.0));
    }
}
