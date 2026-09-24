//! RINEX 3 观测文件子集解析器。
//! 设计目标：接受可复制的小型样例，给出明确的格式错误；
//! 值字段原样保留为字符串，无法转换的值留在原位置并附诊断。

use crate::model::{Diagnostic, RawEpoch, RawImport, RawObs, RawSatObs, Severity, content_id, canonical};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "第 {} 行: {}", self.line, self.message)
    }
}

/// 解析 RINEX 3 观测文件（子集）为原始导入版本。
/// `parent`/`version` 由调用方（引擎）决定；此处 version 先填 0。
pub fn parse_rinex(source_name: &str, text: &str) -> Result<RawImport, ParseError> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Err(ParseError { line: 0, message: "空文件".into() });
    }
    let mut i = 0usize;
    let mut station = String::new();
    let mut obs_types: Vec<String> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut header_closed = false;

    // ---- 头部 ----
    while i < lines.len() {
        let line = lines[i];
        let lineno = i + 1;
        if line.len() < 60 {
            return Err(ParseError { line: lineno, message: format!("头部行不足 60 列: {:?}", line) });
        }
        let label = line.get(60..).map(str::trim).unwrap_or("");
        let content = &line[..60];
        match label {
            "RINEX VERSION / TYPE" => {
                let ver: f64 = content.get(0..9).and_then(|s| s.trim().parse().ok())
                    .ok_or(ParseError { line: lineno, message: "无法解析 RINEX 版本号".into() })?;
                if ver < 3.0 {
                    return Err(ParseError { line: lineno, message: format!("仅支持 RINEX 3.x，得到 {}", ver) });
                }
                let typ = content.get(20..21).unwrap_or(" ");
                if typ != "O" {
                    return Err(ParseError { line: lineno, message: format!("仅支持观测文件(O)，得到类型 {:?}", typ) });
                }
            }
            "MARKER NAME" => {
                station = content.trim().to_string();
            }
            "SYS / # / OBS TYPES" => {
                let sys = content.get(0..1).unwrap_or(" ").to_string();
                if sys != "G" {
                    return Err(ParseError { line: lineno, message: format!("本工作台仅处理 GPS(G)，得到 {:?}", sys) });
                }
                let n: usize = content.get(1..7).and_then(|s| s.trim().parse().ok())
                    .ok_or(ParseError { line: lineno, message: "无法解析观测码数量".into() })?;
                let mut types: Vec<String> = Vec::new();
                for k in 0..n.min(13) {
                    let a = 7 + k * 4;
                    if let Some(t) = content.get(a..a + 3) {
                        types.push(t.trim().to_string());
                    }
                }
                // 续行（超过 13 个观测码）
                let mut remaining = n.saturating_sub(13);
                while remaining > 0 {
                    i += 1;
                    let cont = lines.get(i).ok_or(ParseError { line: lineno, message: "观测码续行缺失".into() })?;
                    let c60: String = cont.chars().take(60).collect();
                    for k in 0..remaining.min(13) {
                        let a = 7 + k * 4;
                        if let Some(t) = c60.get(a..a + 3) {
                            types.push(t.trim().to_string());
                        }
                    }
                    remaining = remaining.saturating_sub(13);
                }
                if types.len() != n {
                    return Err(ParseError { line: lineno, message: format!("声明 {} 个观测码，解析到 {} 个", n, types.len()) });
                }
                obs_types = types;
            }
            "END OF HEADER" => {
                header_closed = true;
                i += 1;
                break;
            }
            _ => {}
        }
        i += 1;
    }
    if !header_closed {
        return Err(ParseError { line: lines.len(), message: "缺少 END OF HEADER".into() });
    }
    if station.is_empty() {
        return Err(ParseError { line: 0, message: "头部缺少 MARKER NAME".into() });
    }
    if obs_types.is_empty() {
        return Err(ParseError { line: 0, message: "头部缺少 SYS / # / OBS TYPES".into() });
    }

    // ---- 数据段 ----
    let mut epochs: Vec<RawEpoch> = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let lineno = i + 1;
        i += 1;
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('>') {
            return Err(ParseError { line: lineno, message: format!("期望历元行('>' 起始)，得到 {:?}", line) });
        }
        let toks: Vec<&str> = line[1..].split_whitespace().collect();
        if toks.len() < 8 {
            return Err(ParseError { line: lineno, message: "历元行字段不足（需要 年 月 日 时 分 秒 标志 卫星数 [钟差]）".into() });
        }
        let (y, mo, d, h, mi) = (
            toks[0].parse::<i64>(), toks[1].parse::<u32>(), toks[2].parse::<u32>(),
            toks[3].parse::<u32>(), toks[4].parse::<u32>(),
        );
        let sec = toks[5].parse::<f64>();
        let (y, mo, d, h, mi, sec) = match (y, mo, d, h, mi, sec) {
            (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(s)) => (y, mo, d, h, mi, s),
            _ => return Err(ParseError { line: lineno, message: "历元时间字段无法解析".into() }),
        };
        let epoch_iso = format_iso(y, mo, d, h, mi, sec);
        let epoch_flag = Some(toks[6].to_string());
        let nsat: usize = toks[7].parse().map_err(|_| ParseError { line: lineno, message: "卫星数无法解析".into() })?;
        let recv_clock_offset = toks.get(8).map(|s| s.to_string());

        let mut sats: Vec<RawSatObs> = Vec::new();
        for _ in 0..nsat {
            let sline = lines.get(i).ok_or(ParseError { line: lineno, message: format!("卫星观测行不足（声明 {} 颗）", nsat) })?;
            let slineno = i + 1;
            i += 1;
            if sline.starts_with('>') {
                return Err(ParseError { line: slineno, message: "卫星观测行数不足（遇到下一历元）".into() });
            }
            if sline.len() < 3 {
                return Err(ParseError { line: slineno, message: "卫星行过短".into() });
            }
            let sat = sline[0..3].trim().to_string();
            let mut obs: Vec<RawObs> = Vec::new();
            for (k, code) in obs_types.iter().enumerate() {
                let a = 3 + k * 16;
                let field = sline.get(a..a + 16).unwrap_or("");
                let value = field.get(0..14).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
                let lli = field.get(14..15).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
                let ssi = field.get(15..16).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
                // 无法转换的值：留在原位置并附诊断。
                if let Some(v) = &value {
                    if v.parse::<f64>().is_err() {
                        diagnostics.push(Diagnostic {
                            path: format!("epoch={} sat={} code={}", epoch_iso, sat, code),
                            message: format!("值 {:?} 无法转换为数值，保留原始表示", v),
                            severity: Severity::Warning,
                        });
                    }
                }
                obs.push(RawObs { code: code.clone(), value, lli, ssi });
            }
            sats.push(RawSatObs { sat, obs });
        }
        epochs.push(RawEpoch { epoch: epoch_iso, epoch_flag, recv_clock_offset, sats });
    }
    if epochs.is_empty() {
        return Err(ParseError { line: lines.len(), message: "数据段没有任何历元".into() });
    }

    let mut import = RawImport {
        id: String::new(),
        version: 0,
        parent: None,
        source_name: source_name.to_string(),
        station,
        obs_types,
        epochs,
        diagnostics,
    };
    import.id = content_id("imp", &canonical(&(&import.source_name, &import.station, &import.obs_types, &import.epochs)));
    Ok(import)
}

// ---------------------------------------------------------------- 确定性时间工具（虚拟时间基准）

/// 天数换算（Howard Hinnant 算法），避免依赖系统时区。
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn format_iso(y: i64, mo: u32, d: u32, h: u32, mi: u32, sec: f64) -> String {
    let s = sec.floor() as u32;
    let frac = ((sec - s as f64) * 1e7).round() as u32;
    if frac > 0 {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:07}Z", y, mo, d, h, mi, s, frac)
    } else {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s)
    }
}

/// 解析 ISO 历元为 GPS 时系秒（自 1980-01-06 的确定性格式化秒数；不做闰秒换算，
/// 因为同一项目内只做差分，基准一致即可）。无法解析返回 None。
pub fn parse_epoch_seconds(iso: &str) -> Option<f64> {
    let b = iso.as_bytes();
    if b.len() < 19 { return None; }
    let y: i64 = iso.get(0..4)?.parse().ok()?;
    let mo: u32 = iso.get(5..7)?.parse().ok()?;
    let d: u32 = iso.get(8..10)?.parse().ok()?;
    let h: u32 = iso.get(11..13)?.parse().ok()?;
    let mi: u32 = iso.get(14..16)?.parse().ok()?;
    let sec: f64 = iso.get(17..)?.trim_end_matches('Z').parse().ok()?;
    let days = days_from_civil(y, mo, d);
    Some((days * 86400 + (h as i64) * 3600 + (mi as i64) * 60) as f64 + sec)
}

pub fn iso_from_unix_seconds(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let rem = unix.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format_iso(y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as f64)
}
