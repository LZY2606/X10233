//! 导入层：解析可复制的小型 RINEX 风格观测文本。
//! 原始输入一经提交不可覆盖；解析失败的行/值保留原样并附诊断。
//!
//! 格式（# 开头为注释）：
//!   STATION CBPT
//!   EPOCH 2026-09-19T00:00:00.000Z
//!   SAT G05 F1 C1C=22123456.789 L1C=-116123456.12345 D1C=-1234.5 S1C=45.2 LLI=0
//!   CLOCK 2026-09-19T00:02:40.000Z +0.000500 receiver-log

use crate::model::*;

pub struct ParsedSource {
    pub station: String,
    pub obs: Vec<Obs>,
    pub clocks: Vec<ClockEvent>,
    pub rejects: Vec<Reject>,
}

pub fn parse_source(content: &str) -> ParsedSource {
    let mut station = String::from("UNKNOWN");
    let mut cur_t: Option<f64> = None;
    let mut obs = Vec::new();
    let mut clocks = Vec::new();
    let mut rejects = Vec::new();

    for (idx, line0) in content.lines().enumerate() {
        let line = line0.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks[0] {
            "STATION" if toks.len() >= 2 => station = toks[1].to_string(),
            "EPOCH" if toks.len() >= 2 => match parse_epoch_time(toks[1]) {
                Some(t) => cur_t = Some(t),
                None => rejects.push(Reject {
                    line_no: idx + 1,
                    raw: line0.to_string(),
                    diag: format!("bad epoch time '{}'", toks[1]),
                }),
            },
            "CLOCK" if toks.len() >= 3 => {
                match (parse_epoch_time(toks[1]), toks[2].parse::<f64>()) {
                    (Some(t), Ok(off)) => clocks.push(ClockEvent {
                        t,
                        offset_s: off,
                        note: toks[3..].join(" "),
                        raw: line0.to_string(),
                    }),
                    _ => rejects.push(Reject {
                        line_no: idx + 1,
                        raw: line0.to_string(),
                        diag: "bad CLOCK line".into(),
                    }),
                }
            }
            "SAT" if toks.len() >= 3 => {
                let Some(t) = cur_t else {
                    rejects.push(Reject {
                        line_no: idx + 1,
                        raw: line0.to_string(),
                        diag: "SAT line before any EPOCH".into(),
                    });
                    continue;
                };
                let sat = toks[1].to_string();
                let Some(freq) = toks[2].strip_prefix('F').and_then(|s| s.parse::<i64>().ok())
                else {
                    rejects.push(Reject {
                        line_no: idx + 1,
                        raw: line0.to_string(),
                        diag: format!("bad freq token '{}'", toks[2]),
                    });
                    continue;
                };
                let mut o = Obs {
                    t,
                    station: station.clone(),
                    sat,
                    freq,
                    code_p: None,
                    pseudo: None,
                    code_l: None,
                    carrier: None,
                    lli: None,
                    snr: None,
                    code_d: None,
                    doppler: None,
                    raw: line0.to_string(),
                    diag: None,
                };
                let mut diags = Vec::new();
                for tok in &toks[3..] {
                    let Some(eq) = tok.find('=') else {
                        diags.push(format!("token '{tok}' not key=value"));
                        continue;
                    };
                    let (k, v) = (&tok[..eq], &tok[eq + 1..]);
                    if k == "LLI" {
                        match v.parse::<i64>() {
                            Ok(x) => o.lli = Some(x),
                            Err(_) => diags.push(format!("unparseable LLI '{v}'")),
                        }
                        continue;
                    }
                    let parsed = v.parse::<f64>();
                    match k.chars().next() {
                        Some('C') => {
                            o.code_p = Some(k.to_string());
                            match parsed {
                                Ok(x) => o.pseudo = Some(x),
                                Err(_) => diags.push(format!("unparseable {k}='{v}'")),
                            }
                        }
                        Some('L') => {
                            o.code_l = Some(k.to_string());
                            match parsed {
                                Ok(x) => o.carrier = Some(x),
                                Err(_) => diags.push(format!("unparseable {k}='{v}'")),
                            }
                        }
                        Some('D') => {
                            o.code_d = Some(k.to_string());
                            match parsed {
                                Ok(x) => o.doppler = Some(x),
                                Err(_) => diags.push(format!("unparseable {k}='{v}'")),
                            }
                        }
                        Some('S') => match parsed {
                            Ok(x) => o.snr = Some(x),
                            Err(_) => diags.push(format!("unparseable {k}='{v}'")),
                        },
                        _ => diags.push(format!("unknown key '{k}'")),
                    }
                }
                if !diags.is_empty() {
                    o.diag = Some(diags.join("; "));
                }
                obs.push(o);
            }
            _ => rejects.push(Reject {
                line_no: idx + 1,
                raw: line0.to_string(),
                diag: "unrecognized line".into(),
            }),
        }
    }
    ParsedSource {
        station,
        obs,
        clocks,
        rejects,
    }
}
