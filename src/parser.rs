//! 紧凑 RINEX 观测表（COSV, Carrier Observation Sheet, v1）解析器。
//!
//! 目标是“可复制的小型样例 + 明确的格式错误”：伪距、载波、Doppler、LLI、信噪比、
//! 钟跳事件、频率编号与观测码均按历元原样保存；不合法值留在原位置并附诊断。
use crate::domain::*;
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ParseDiagnostic {
    pub line_no: usize,
    pub level: String, // error | warning
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SheetDoc {
    pub doc_id: String,
    pub kind: String, // obs | sat
    pub frequencies: Vec<FrequencyDef>,
    pub stations: Vec<StationDef>,
    pub epochs: Vec<Epoch>,
    pub observations: Vec<Observation>,
    pub clock_events: Vec<ClockEvent>,
    pub sat_states: Vec<SatState>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub valid: bool,
}

pub fn parse_sheet(kind: &str, text: &str, doc_id: &str) -> SheetDoc {
    let mut d = SheetDoc {
        doc_id: doc_id.to_string(),
        kind: kind.to_string(),
        valid: true,
        ..Default::default()
    };
    let mut times: Vec<(i64, String, f64)> = Vec::new(); // idx, raw, dt
    let mut prev_t: Option<f64> = None;

    for (i, raw_line) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let head = parts.next().unwrap_or("");
        let push_err = |d: &mut SheetDoc, msg: String| {
            d.valid = false;
            d.diagnostics.push(ParseDiagnostic {
                line_no,
                level: "error".into(),
                message: msg,
            });
        };

        match head {
            "@SHEET" => {
                let k = parts.next().unwrap_or("");
                if k != kind {
                    push_err(
                        &mut d,
                        format!("sheet kind {k:?} does not match endpoint {kind:?}"),
                    );
                }
            }
            "@FREQ" => {
                let band = parts.next();
                let codes_s = parts.next();
                let freq_tok = parts.next();
                match (band, codes_s, freq_tok) {
                    (Some(band), Some(codes), Some(freq_tok)) => {
                        let n = RawNum::parse(freq_tok, "Hz");
                        if n.value.is_none() {
                            push_err(&mut d, format!("bad @FREQ frequency {freq_tok:?}"));
                        }
                        d.frequencies.push(FrequencyDef {
                            band: band.into(),
                            codes: codes.split(',').map(|s| s.to_string()).collect(),
                            freq_hz: n.value.unwrap_or(0.0),
                        });
                    }
                    _ => push_err(&mut d, "@FREQ expects <band> <c1,c2> <freq>".into()),
                }
            }
            "@STATION" => {
                let (name, x, y, z) =
                    (parts.next(), parts.next(), parts.next(), parts.next());
                match (name, x, y, z) {
                    (Some(name), Some(x), Some(y), Some(z)) => d.stations.push(StationDef {
                        name: name.into(),
                        x_m: x.parse().unwrap_or(f64::NAN),
                        y_m: y.parse().unwrap_or(f64::NAN),
                        z_m: z.parse().unwrap_or(f64::NAN),
                        datum: DATUM.into(),
                    }),
                    _ => push_err(&mut d, "@STATION expects <name> <x> <y> <z>".into()),
                }
            }
            "@EPOCH" => {
                let idx_tok = parts.next();
                let t = parts.next().unwrap_or("");
                match idx_tok.and_then(|s| s.parse::<i64>().ok()) {
                    Some(idx) => {
                        let tval = parse_epoch_time(t);
                        let dt = match (tval, prev_t) {
                            (Some(tv), Some(p)) => (tv - p).max(0.0),
                            _ => 0.0,
                        };
                        prev_t = tval.or(prev_t);
                        times.push((idx, t.to_string(), dt));
                        d.epochs.push(Epoch {
                            id: hid("ep", &format!("{doc_id}|{idx}|{t}")),
                            doc_id: doc_id.into(),
                            idx,
                            t_raw: t.into(),
                            dt_s: dt,
                        });
                    }
                    None => push_err(&mut d, "@EPOCH expects integer index".into()),
                }
            }
            "@CLOCK" => {
                let (idx_tok, ev, off) =
                    (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                match idx_tok.and_then(|s| s.parse::<i64>().ok()) {
                    Some(idx) => d.clock_events.push(ClockEvent {
                        id: hid("clk", &format!("{doc_id}|{idx}|{ev}|{off}")),
                        doc_id: doc_id.into(),
                        epoch_idx: idx,
                        event: ev.into(),
                        offset_ns: RawNum::parse(off, "ns"),
                    }),
                    None => push_err(&mut d, "@CLOCK expects integer epoch index".into()),
                }
            }
            "@SAT" => {
                if kind != "sat" {
                    push_err(&mut d, "@SAT rows belong in a satellite-state sheet".into());
                    continue;
                }
                let (idx_tok, sat, x, y, z) = (
                    parts.next(),
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or(""),
                );
                match idx_tok.and_then(|s| s.parse::<i64>().ok()) {
                    Some(idx) => d.sat_states.push(SatState {
                        id: hid("sat", &format!("{doc_id}|{idx}|{sat}")),
                        doc_id: doc_id.into(),
                        epoch_idx: idx,
                        sat: sat.into(),
                        x: RawNum::parse(x, "m"),
                        y: RawNum::parse(y, "m"),
                        z: RawNum::parse(z, "m"),
                        datum: DATUM.into(),
                    }),
                    None => push_err(&mut d, "@SAT expects integer epoch index".into()),
                }
            }
            _ if head.starts_with('G') || head.starts_with('E') || head.starts_with('R') => {
                parse_obs_row(&mut d, doc_id, line_no, line, &mut push_err);
            }
            _ => push_err(&mut d, format!("unknown record {head:?}")),
        }
    }

    if d.frequencies.is_empty() && kind == "obs" {
        d.valid = false;
        d.diagnostics.push(ParseDiagnostic {
            line_no: 0,
            level: "error".into(),
            message: "no @FREQ definition".into(),
        });
    }
    let _ = times;
    d
}

fn parse_obs_row<F>(d: &mut SheetDoc, doc_id: &str, line_no: usize, line: &str, push_err: &mut F)
where
    F: FnMut(&mut SheetDoc, String),
{
    let mut it = line.split_whitespace();
    let sat = it.next().unwrap_or("").to_string();
    let idx_tok = it.next().unwrap_or("");
    let code = it.next().unwrap_or("").to_string();
    let idx = match idx_tok.parse::<i64>().ok() {
        Some(v) => v,
        None => {
            push_err(d, format!("satellite row {sat} has non-integer epoch {idx_tok:?}"));
            return;
        }
    };
    let code_known = d.frequencies.iter().any(|f| f.codes.contains(&code));
    if !code_known {
        push_err(d, format!("observation code {code:?} at {sat} epoch {idx} not declared in @FREQ"));
    }
    // 其余标记顺序：C [值] L [值] D [值] S [值]，每个信号可带 LLI=..。
    // 支持两种写法：`L 12345.6 LLI=1`。缺失的信号直接不出现 => 频点/信号缺失。
    // C/L/D/S 为信号标签，紧跟其值；`LLI=..` 附属于上一个信号。信号标签不出现即该信号缺失。
    let mut tokens: Vec<(String, String, Option<String>)> = Vec::new();
    let mut waiting: bool = false;
    for tok in it {
        if let Some(sig) = tok.strip_prefix("LLI=") {
            if let Some(t) = tokens.last_mut() {
                t.2 = Some(sig.to_string());
            } else {
                push_err(d, format!("LLI without signal at {sat} epoch {idx}"));
            }
            waiting = false;
            continue;
        }
        if matches!(tok, "C" | "L" | "D" | "S") {
            tokens.push((tok.to_string(), String::new(), None));
            waiting = true;
        } else if waiting {
            if let Some(t) = tokens.last_mut() {
                t.1 = tok.to_string();
            }
            waiting = false;
        } else {
            push_err(d, format!("unexpected token {tok:?} at {sat} epoch {idx}"));
        }
    }

    for (sig, val, lli) in tokens {
        let (kind, unit) = match sig.as_str() {
            "C" => (SigKind::Code, "m"),
            "L" => (SigKind::Phase, "cycle"),
            "D" => (SigKind::Doppler, "Hz"),
            "S" => (SigKind::Snr, "dBHz"),
            _ => unreachable!(),
        };
        let num = if val.is_empty() {
            let mut n = RawNum::missing();
            n.diagnostic = Some(format!("{sig} value omitted at {sat} {code} epoch {idx}"));
            n
        } else {
            RawNum::parse(&val, unit)
        };
        if !num.ok {
            // 转换失败不是整张表失败，但该行诊断保留，值留在原位置。
            d.diagnostics.push(ParseDiagnostic {
                line_no,
                level: "warning".into(),
                message: format!(
                    "{} {} epoch {} {} token: {}",
                    sat,
                    code,
                    idx,
                    sig,
                    num.diagnostic.clone().unwrap_or_default()
                ),
            });
        }
        d.observations.push(Observation {
            id: hid(
                "obs",
                &format!("{doc_id}|{idx}|{sat}|{code}|{}", kind.tag()),
            ),
            doc_id: doc_id.into(),
            epoch_idx: idx,
            sat: sat.clone(),
            code: code.clone(),
            kind: kind.tag().into(),
            num,
            lli,
        });
    }
}

fn parse_epoch_time(t: &str) -> Option<f64> {
    // 演示格式：GPS 秒，形如 120.0（允许小数，间隔可不等）。
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}
