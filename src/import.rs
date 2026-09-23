//! 导入层：RINEX-lite 文本（OBS/CLK/NAV 行）。
//! - 伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号、观测码均按历元原样保存（raw_line）。
//! - 结构性错误（字段数、未知类型、坏历元）=> 整个导入拒绝并给出带行号的诊断。
//! - 值级不可转换（如载波为 `?`）=> 原值留在 raw_line，解析列为 NULL 并附诊断。

use crate::model::{parse_epoch_ms, Diag};

#[derive(Debug, Clone)]
pub struct ParsedEpoch {
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub code: String,
    pub freq: i64,
    pub pr_m: Option<f64>,
    pub carrier: Option<f64>,
    pub doppler: Option<f64>,
    pub lli: i64,
    pub snr: Option<f64>,
    pub diag: Option<String>,
    pub raw_line: String,
}

#[derive(Debug, Clone)]
pub struct ParsedClock {
    pub station: String,
    pub epoch_ms: i64,
    pub jump_ms: f64,
    pub raw_line: String,
}

#[derive(Debug, Clone)]
pub struct ParsedNav {
    pub sat: String,
    pub epoch_ms: i64,
    pub x_m: Option<f64>,
    pub y_m: Option<f64>,
    pub z_m: Option<f64>,
    pub health: i64,
    pub diag: Option<String>,
    pub raw_line: String,
}

#[derive(Debug, Default)]
pub struct ParsedSource {
    pub epochs: Vec<ParsedEpoch>,
    pub clocks: Vec<ParsedClock>,
    pub navs: Vec<ParsedNav>,
    pub diagnostics: Vec<Diag>,
}

fn parse_f64(tok: &str, line_no: usize, field: &str, diags: &mut Vec<String>) -> Option<f64> {
    match tok.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => {
            diags.push(format!(
                "line {}: field {} value {:?} 不可转换，原值保留并附诊断",
                line_no, field, tok
            ));
            None
        }
    }
}

/// 解析成功返回 ParsedSource；存在结构性错误时返回 Err(诊断列表)。
pub fn parse_source(raw: &str) -> Result<ParsedSource, Vec<Diag>> {
    let mut out = ParsedSource::default();
    let mut structural: Vec<Diag> = Vec::new();

    for (idx, line) in raw.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = trimmed.split_whitespace().collect();
        match toks[0] {
            "OBS" => {
                if toks.len() != 11 {
                    structural.push(Diag {
                        line: Some(line_no),
                        code: "obs_field_count".into(),
                        message: format!(
                            "OBS 行需要 11 个字段（OBS 站 星 历元 观测码 频率编号 伪距m 载波周 DopplerHz LLI SNR），实际 {}",
                            toks.len()
                        ),
                    });
                    continue;
                }
                let epoch_ms = match parse_epoch_ms(toks[3]) {
                    Some(v) => v,
                    None => {
                        structural.push(Diag {
                            line: Some(line_no),
                            code: "bad_epoch".into(),
                            message: format!("历元 {:?} 不是合法 UTC ISO-8601", toks[3]),
                        });
                        continue;
                    }
                };
                let freq = match toks[5].parse::<i64>() {
                    Ok(v) => v,
                    Err(_) => {
                        structural.push(Diag {
                            line: Some(line_no),
                            code: "bad_freq".into(),
                            message: format!("频率编号 {:?} 不是整数", toks[5]),
                        });
                        continue;
                    }
                };
                let lli = match toks[9].parse::<i64>() {
                    Ok(v) => v,
                    Err(_) => {
                        structural.push(Diag {
                            line: Some(line_no),
                            code: "bad_lli".into(),
                            message: format!("LLI {:?} 不是整数", toks[9]),
                        });
                        continue;
                    }
                };
                let mut value_diags: Vec<String> = Vec::new();
                let pr_m = parse_f64(toks[6], line_no, "伪距(m)", &mut value_diags);
                let carrier = parse_f64(toks[7], line_no, "载波(周)", &mut value_diags);
                let doppler = parse_f64(toks[8], line_no, "Doppler(Hz)", &mut value_diags);
                let snr = parse_f64(toks[10], line_no, "SNR(dB-Hz)", &mut value_diags);
                let diag = if value_diags.is_empty() {
                    None
                } else {
                    out.diagnostics.extend(value_diags.iter().map(|m| Diag {
                        line: Some(line_no),
                        code: "value_unconvertible".into(),
                        message: m.clone(),
                    }));
                    Some(value_diags.join("; "))
                };
                out.epochs.push(ParsedEpoch {
                    station: toks[1].to_string(),
                    sat: toks[2].to_string(),
                    epoch_ms,
                    code: toks[4].to_string(),
                    freq,
                    pr_m,
                    carrier,
                    doppler,
                    lli,
                    snr,
                    diag,
                    raw_line: trimmed.to_string(),
                });
            }
            "CLK" => {
                if toks.len() != 4 {
                    structural.push(Diag {
                        line: Some(line_no),
                        code: "clk_field_count".into(),
                        message: format!("CLK 行需要 4 个字段（CLK 站 历元 跳变ms），实际 {}", toks.len()),
                    });
                    continue;
                }
                let epoch_ms = match parse_epoch_ms(toks[2]) {
                    Some(v) => v,
                    None => {
                        structural.push(Diag {
                            line: Some(line_no),
                            code: "bad_epoch".into(),
                            message: format!("历元 {:?} 不是合法 UTC ISO-8601", toks[2]),
                        });
                        continue;
                    }
                };
                match toks[3].parse::<f64>() {
                    Ok(j) if j.is_finite() => out.clocks.push(ParsedClock {
                        station: toks[1].to_string(),
                        epoch_ms,
                        jump_ms: j,
                        raw_line: trimmed.to_string(),
                    }),
                    _ => structural.push(Diag {
                        line: Some(line_no),
                        code: "bad_clock_jump".into(),
                        message: format!("钟跳量 {:?} 不是有限数值(ms)", toks[3]),
                    }),
                }
            }
            "NAV" => {
                if toks.len() != 7 {
                    structural.push(Diag {
                        line: Some(line_no),
                        code: "nav_field_count".into(),
                        message: format!(
                            "NAV 行需要 7 个字段（NAV 星 历元 x_m y_m z_m health，坐标基准 ECEF/WGS84），实际 {}",
                            toks.len()
                        ),
                    });
                    continue;
                }
                let epoch_ms = match parse_epoch_ms(toks[2]) {
                    Some(v) => v,
                    None => {
                        structural.push(Diag {
                            line: Some(line_no),
                            code: "bad_epoch".into(),
                            message: format!("历元 {:?} 不是合法 UTC ISO-8601", toks[2]),
                        });
                        continue;
                    }
                };
                let mut value_diags: Vec<String> = Vec::new();
                let x_m = parse_f64(toks[3], line_no, "x(m,ECEF)", &mut value_diags);
                let y_m = parse_f64(toks[4], line_no, "y(m,ECEF)", &mut value_diags);
                let z_m = parse_f64(toks[5], line_no, "z(m,ECEF)", &mut value_diags);
                let health = toks[6].parse::<i64>().unwrap_or_else(|_| {
                    value_diags.push(format!("line {}: health {:?} 不可转换，按 0 存", line_no, toks[6]));
                    0
                });
                let diag = if value_diags.is_empty() {
                    None
                } else {
                    out.diagnostics.extend(value_diags.iter().map(|m| Diag {
                        line: Some(line_no),
                        code: "value_unconvertible".into(),
                        message: m.clone(),
                    }));
                    Some(value_diags.join("; "))
                };
                out.navs.push(ParsedNav {
                    sat: toks[1].to_string(),
                    epoch_ms,
                    x_m,
                    y_m,
                    z_m,
                    health,
                    diag,
                    raw_line: trimmed.to_string(),
                });
            }
            other => structural.push(Diag {
                line: Some(line_no),
                code: "unknown_record".into(),
                message: format!("未知记录类型 {:?}（支持 OBS/CLK/NAV）", other),
            }),
        }
    }

    if structural.is_empty() {
        Ok(out)
    } else {
        Err(structural)
    }
}
