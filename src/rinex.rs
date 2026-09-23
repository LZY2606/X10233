//! 简化 RINEX 3 观测文件解析：伪距/载波/Doppler/信噪比/LLI/观测码按历元原样保留。
//! 解析失败的值不丢弃：留在原位置并附诊断。

#[derive(Clone, Debug)]
pub struct ParsedObs {
    pub code: String,
    pub raw_text: String, // 16 字符原始字段
    pub value: Option<f64>,
    pub lli: Option<i64>,
    pub ssi: Option<i64>,
    pub diag: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ParsedSat {
    pub satellite: String,
    pub obs: Vec<ParsedObs>,
}

#[derive(Clone, Debug)]
pub struct ParsedEpoch {
    pub time: String, // ISO 文本，原样保存
    pub t_s: f64,      // 连续秒（派生层规范化用）
    pub clock_offset_s: Option<f64>, // 历元行声明的接收机钟偏移（钟跳事件原样保存）
    pub raw_line: String,
    pub sats: Vec<ParsedSat>,
}

#[derive(Clone, Debug)]
pub struct ParsedFile {
    pub station: String,
    pub obs_types: Vec<String>,
    pub epochs: Vec<ParsedEpoch>,
    pub diagnostics: Vec<(String, String)>, // (scope, message)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn parse(content: &str) -> Result<ParsedFile, String> {
    let mut lines = content.lines().peekable();
    let mut station = String::new();
    let mut obs_types: Vec<String> = Vec::new();
    let mut diagnostics: Vec<(String, String)> = Vec::new();
    let mut header_ok = false;

    // ---- 头 ----
    while let Some(line) = lines.next() {
        let label = if line.len() > 60 { line[60..].trim() } else { "" };
        if label == "MARKER NAME" {
            station = line[..60].trim().to_string();
        } else if label == "SYS / # / OBS TYPES" {
            let body = &line[..60.min(line.len())];
            if body.starts_with(' ') {
                // 续行
                for c in body.split_whitespace() {
                    obs_types.push(c.to_string());
                }
            } else {
                for c in body[1..].split_whitespace().skip(1) {
                    obs_types.push(c.to_string());
                }
            }
        } else if label == "END OF HEADER" {
            header_ok = true;
            break;
        }
    }
    if !header_ok {
        return Err("缺少 END OF HEADER，不是有效的 RINEX 观测文件".into());
    }
    if obs_types.is_empty() {
        diagnostics.push(("header".into(), "未声明 SYS / # / OBS TYPES，历元行将按无观测码处理".into()));
    }
    if station.is_empty() {
        station = "UNKNOWN".into();
        diagnostics.push(("header".into(), "缺少 MARKER NAME，站名记为 UNKNOWN".into()));
    }

    // ---- 历元 ----
    let mut epochs: Vec<ParsedEpoch> = Vec::new();
    let ntypes = obs_types.len();
    while let Some(line) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('>') {
            diagnostics.push(("epoch".into(), format!("无法识别的行（已跳过）: {}", line.trim())));
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 9 {
            diagnostics.push(("epoch".into(), format!("历元行字段不足（已跳过）: {}", line.trim())));
            continue;
        }
        let (y, mo, d, h, mi) = (
            f[1].parse::<i64>(), f[2].parse::<i64>(), f[3].parse::<i64>(),
            f[4].parse::<i64>(), f[5].parse::<i64>(),
        );
        let sec = f[6].parse::<f64>();
        let (y, mo, d, h, mi, sec) = match (y, mo, d, h, mi, sec) {
            (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(sec)) => (y, mo, d, h, mi, sec),
            _ => {
                diagnostics.push(("epoch".into(), format!("历元时间无法解析（已跳过）: {}", line.trim())));
                continue;
            }
        };
        let nsat = f[8].parse::<usize>().unwrap_or(0);
        // 历元行可选尾部字段：接收机钟偏移（秒）——钟跳事件按历元原样保存
        let clock_offset_s = if f.len() > 9 { f[9].parse::<f64>().ok() } else { None };
        let t_s = (days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60) as f64 + sec;
        let time = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:07.4}Z", y, mo, d, h, mi, sec);
        let mut sats = Vec::new();
        for _ in 0..nsat {
            let Some(sline) = lines.next() else {
                diagnostics.push((time.clone(), "历元卫星行缺失（文件提前结束）".into()));
                break;
            };
            if sline.starts_with('>') {
                diagnostics.push((time.clone(), "历元卫星行数不足（遇到下一历元）".into()));
                break;
            }
            if sline.len() < 3 {
                diagnostics.push((time.clone(), format!("卫星行过短（已跳过）: {}", sline.trim())));
                continue;
            }
            let satellite = sline[..3].trim().to_string();
            let mut obs = Vec::new();
            for k in 0..ntypes {
                let start = 3 + k * 16;
                let field: String = if start + 16 <= sline.len() {
                    sline[start..start + 16].to_string()
                } else if start < sline.len() {
                    format!("{:<16}", &sline[start..])
                } else {
                    " ".repeat(16)
                };
                if field.trim().is_empty() {
                    continue; // 缺测：不生成记录（频率缺失情形）
                }
                let vtxt = field[..14].trim();
                let value = if vtxt.is_empty() {
                    None
                } else {
                    match vtxt.parse::<f64>() {
                        Ok(v) => Some(v),
                        Err(_) => None,
                    }
                };
                let lli = field[14..15].trim().parse::<i64>().ok();
                let ssi = field[15..16].trim().parse::<i64>().ok();
                let diag = if value.is_none() && !vtxt.is_empty() {
                    Some(format!("观测值无法解析为浮点: '{}'", vtxt))
                } else {
                    None
                };
                obs.push(ParsedObs {
                    code: obs_types[k].clone(),
                    raw_text: field,
                    value,
                    lli,
                    ssi,
                    diag,
                });
            }
            sats.push(ParsedSat { satellite, obs });
        }
        epochs.push(ParsedEpoch { time, t_s, clock_offset_s, raw_line: line.to_string(), sats });
    }
    if epochs.is_empty() {
        return Err("文件中没有可用历元".into());
    }
    Ok(ParsedFile { station, obs_types, epochs, diagnostics })
}
