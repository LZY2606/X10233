use crate::model::ObsKind;

/// RINEX-mini：可复制的白空格容错 RINEX 观测子集。
///
/// 格式（逐行）：
///   `# ...`                          注释
///   `STATION <站名>`                  站点声明（缺省 UNKN 并记录诊断）
///   `FREQ <编号> <MHz>MHz <波段>`     频率编号声明（原样保留）
///   `!CLOCK <站> idx=<n> <+x.x>ms`    接收机钟跳事件（原样保留）
///   `> <ISO历元> t=<秒>s`             历元头；t 缺省时 epoch_s 为空并记录诊断
///   `<卫星> <频率编号> <码>=<值><单位> ... [LLI=<n>]`  观测行
///
/// 单位：m（伪距）、cyc（载波，周）、hz（Doppler）、dbhz（信噪比）。
/// 无法转换的值保留在 value_raw 原位置并附诊断，不为派生计算所丢弃。
#[derive(Clone, Debug)]
pub struct ParsedObs {
    pub station: String,
    pub epoch_idx: i64,
    pub epoch_raw: String,
    pub epoch_s: Option<f64>,
    pub sat: String,
    pub freq_no: i64,
    pub obs_code: String,
    pub kind: ObsKind,
    pub value_raw: String,
    pub value: Option<f64>,
    pub unit: String,
    pub lli: i64,
    pub diag: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ParsedClock {
    pub station: String,
    pub epoch_idx: i64,
    pub offset_raw: String,
    pub offset_ms: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedImport {
    pub obs: Vec<ParsedObs>,
    pub clocks: Vec<ParsedClock>,
    pub diagnostics: Vec<String>,
}

fn split_value_unit(token: &str) -> (String, String) {
    let pos = token
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(token.len());
    (token[..pos].to_string(), token[pos..].to_string())
}

pub fn parse_rinex_mini(text: &str) -> ParsedImport {
    let mut out = ParsedImport::default();
    let mut station = String::new();
    let mut epoch_idx: i64 = -1;
    let mut epoch_raw = String::new();
    let mut epoch_s: Option<f64> = None;

    for (lineno, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks[0] {
            "STATION" => {
                if toks.len() >= 2 {
                    station = toks[1].to_string();
                } else {
                    out.diagnostics
                        .push(format!("行{}: STATION 缺少站名", lineno + 1));
                }
            }
            "FREQ" => { /* 频率编号声明：元信息，原样保留在 raw_text 中 */ }
            "!CLOCK" => {
                let mut idx: Option<i64> = None;
                let mut offset_raw = String::new();
                for t in &toks[1..] {
                    if let Some(v) = t.strip_prefix("idx=") {
                        idx = v.parse().ok();
                    } else if t.ends_with("ms") {
                        offset_raw = t.to_string();
                    }
                }
                let st = toks.get(1).unwrap_or(&"UNKN").to_string();
                match (idx, offset_raw.is_empty()) {
                    (Some(i), false) => {
                        let ms = offset_raw.trim_end_matches("ms").parse::<f64>().ok();
                        if ms.is_none() {
                            out.diagnostics.push(format!(
                                "行{}: 钟跳量无法转换，保留原值 {offset_raw}",
                                lineno + 1
                            ));
                        }
                        out.clocks.push(ParsedClock { station: st, epoch_idx: i, offset_raw, offset_ms: ms });
                    }
                    _ => out
                        .diagnostics
                        .push(format!("行{}: 钟跳事件格式错误: {line}", lineno + 1)),
                }
            }
            ">" => {
                epoch_idx += 1;
                epoch_raw = toks.get(1).unwrap_or(&"").to_string();
                epoch_s = None;
                for t in &toks[2..] {
                    if let Some(v) = t.strip_prefix("t=") {
                        epoch_s = v.trim_end_matches('s').parse::<f64>().ok();
                    }
                }
                if epoch_raw.is_empty() {
                    out.diagnostics
                        .push(format!("行{}: 历元头缺少时间戳", lineno + 1));
                }
                if epoch_s.is_none() {
                    out.diagnostics.push(format!(
                        "行{}: 历元秒无法转换（缺 t= 或非法），保留原始表示 {epoch_raw}",
                        lineno + 1
                    ));
                }
            }
            _ => {
                if toks.len() < 2 {
                    out.diagnostics
                        .push(format!("行{}: 无法识别的行: {line}", lineno + 1));
                    continue;
                }
                if epoch_idx < 0 {
                    out.diagnostics
                        .push(format!("行{}: 观测行出现在任何历元头之前，已跳过", lineno + 1));
                    continue;
                }
                if station.is_empty() {
                    station = "UNKN".to_string();
                    out.diagnostics
                        .push(format!("行{}: 未声明 STATION，使用 UNKN", lineno + 1));
                }
                let sat = toks[0].to_string();
                let freq_no: i64 = match toks[1].parse() {
                    Ok(f) => f,
                    Err(_) => {
                        out.diagnostics.push(format!(
                            "行{}: 频率编号无法转换: {}，整行保留为诊断",
                            lineno + 1,
                            toks[1]
                        ));
                        continue;
                    }
                };
                let mut lli: i64 = 0;
                for t in &toks[2..] {
                    if let Some(v) = t.strip_prefix("LLI=") {
                        lli = v.parse().unwrap_or_else(|_| {
                            out.diagnostics
                                .push(format!("行{}: LLI 无法转换: {t}", lineno + 1));
                            0
                        });
                        continue;
                    }
                    let Some(eq) = t.find('=') else {
                        out.diagnostics
                            .push(format!("行{}: 无法识别的令牌: {t}", lineno + 1));
                        continue;
                    };
                    let code = &t[..eq];
                    let (num, unit) = split_value_unit(&t[eq + 1..]);
                    let value = num.parse::<f64>().ok();
                    let diag = if value.is_none() {
                        let d = format!(
                            "行{}: 值无法转换，保留原值 {}（{} {}）",
                            lineno + 1,
                            &t[eq + 1..],
                            sat,
                            code
                        );
                        out.diagnostics.push(d.clone());
                        Some(d)
                    } else {
                        None
                    };
                    out.obs.push(ParsedObs {
                        station: station.clone(),
                        epoch_idx,
                        epoch_raw: epoch_raw.clone(),
                        epoch_s,
                        sat: sat.clone(),
                        freq_no,
                        obs_code: code.to_string(),
                        kind: ObsKind::from_code(code),
                        value_raw: t[eq + 1..].to_string(),
                        value,
                        unit,
                        lli,
                        diag,
                    });
                }
            }
        }
    }
    out
}
