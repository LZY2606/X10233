//! GOB v1 案例格式解析器（GNSS Observation Bundle）。
//!
//! 面向“可复制的小型样例 + 明确的格式错误”。结构错误立即拒绝；
//! 数值无法转换时保留原始 token 并产出诊断，绝不丢数据。
//!
//! 行语法（`#` 起始为注释）：
//!   CASE <id> | <title> | seed=<u64>
//!   STATION <id> | <marker> | datum=<ITRF2014> | x=<m> y=<m> z=<m>
//!   FREQDEF <constellation> <band> <code> | freq=<Hz>
//!   EPOCH <station> | <seconds> | interval_hint=<s>
//!   OBS <station> | <seconds> | <sat> | <band> <code> |
//!       C1C=<pseudorange,m> L1C=<carrier,cycles> D1C=<doppler,Hz>
//!       LLI=<0|1|2> S1C=<snr,dBHz>
//!   CLOCK <station> | <seconds> | jump=<ns> | source=<name>
//!   SATSTATE <sat> | <seconds> | x=<m> y=<m> z=<m> clock=<s> | datum=ITRF2014

use crate::model::*;

pub struct ParseOutcome {
    pub bundle: ParsedBundle,
    pub fatal: Option<String>,
}

fn diag(line_no: u32, code: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic { code: code.to_string(), line_no: Some(line_no), message: message.into() }
}

fn parse_f64(token: Option<&str>, line_no: u32, field: &str, diags: &mut Vec<Diagnostic>) -> Option<f64> {
    let raw = token?.trim();
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => {
            diags.push(diag(line_no, "UNCONVERTIBLE_VALUE",
                format!("{field} 无法转换为数值，保留原始表示 `{raw}`")));
            None
        }
    }
}

fn fields(line: &str) -> Vec<String> {
    line.split('|').map(|s| s.trim().to_string()).collect()
}

fn kv<'a>(segment: &'a str, key: &str) -> Option<&'a str> {
    segment.split_whitespace().find_map(|tok| tok.strip_prefix(&format!("{key}=")))
}

pub fn parse_gob(text: &str, allow_patch_header: bool) -> ParseOutcome {
    let mut b = ParsedBundle {
        format: "GOB".into(), format_version: "1".into(),
        case_id: String::new(), title: String::new(), seed: 0,
        stations: Vec::new(), freqs: Vec::new(), observations: Vec::new(),
        clock_events: Vec::new(), sat_states: Vec::new(),
        diagnostics: Vec::new(), replaces: false,
    };
    let mut fatal = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();
        let fail = |msg: &str| -> Option<()> {
            fatal = Some(format!("第 {line_no} 行格式错误：{msg}（原文：{line}）"));
            None
        };
        match head {
            "GOB" => {
                let f = fields(rest);
                let ver = f.first().map(|s| s.trim()).unwrap_or("");
                if ver != "1" {
                    fatal = Some(format!("第 {line_no} 行：仅支持 GOB 版本 1，收到 `{ver}`"));
                }
            }
            "CASE" => {
                let f = fields(rest);
                if f.len() < 2 { let _ = fail("CASE 需要 id | title | seed="); continue; }
                b.case_id = f[0].clone();
                b.title = f[1].clone();
                if let Some(s) = f.get(2).and_then(|x| kv(x, "seed")) {
                    b.seed = s.parse().unwrap_or(0);
                }
            }
            "PATCH" if allow_patch_header => {
                let f = fields(rest);
                if f.len() < 3 { let _ = fail("PATCH 需要 parent=<id> | case=<id> | replaces=OBS"); continue; }
                b.case_id = f.get(1).cloned().unwrap_or_default();
                b.replaces = f.iter().any(|x| x.contains("replaces=OBS"));
            }
            "STATION" => {
                let f = fields(rest);
                if f.len() < 3 { let _ = fail("STATION 需要 id | marker | datum= ..."); continue; }
                let datum = kv(&f[2], "datum").unwrap_or("LOCAL").to_string();
                let coord = if let (Some(x), Some(y), Some(z)) =
                    (kv(&f[2], "x"), kv(&f[2], "y"), kv(&f[2], "z")) {
                    match (x.parse::<f64>(), y.parse::<f64>(), z.parse::<f64>()) {
                        (Ok(x), Ok(y), Ok(z)) => Some([x, y, z]),
                        _ => { b.diagnostics.push(diag(line_no, "UNCONVERTIBLE_VALUE",
                            "站坐标分量无法转换，坐标留空但站点保留")); None }
                    }
                } else { None };
                b.stations.push(StationDef {
                    station: f[0].clone(), marker_name: f[1].clone(), coord, datum,
                });
            }
            "FREQDEF" => {
                let f = fields(rest);
                if f.len() < 2 { let _ = fail("FREQDEF 需要 'GNSS band code' | freq="); continue; }
                let id: Vec<&str> = f[0].split_whitespace().collect();
                if id.len() < 3 { let _ = fail("FREQDEF 段需要 星座 频率编号 观测码"); continue; }
                let freq_raw = kv(&f[1], "freq").unwrap_or("").to_string();
                let freq_hz = parse_f64(Some(&freq_raw), line_no, "freq", &mut b.diagnostics);
                b.freqs.push(FreqDef {
                    constellation: id[0].into(), band: id[1].into(), code: id[2].into(),
                    freq_raw, freq_hz, unit: "Hz".into(),
                });
            }
            "CLOCK" => {
                let f = fields(rest);
                if f.len() < 3 { let _ = fail("CLOCK 需要 station | seconds | jump="); continue; }
                let epoch_raw = f[1].clone();
                let epoch = parse_f64(Some(&epoch_raw), line_no, "epoch", &mut b.diagnostics)
                    .unwrap_or(f64::NAN);
                let jump_raw = kv(&f[2], "jump").unwrap_or("").to_string();
                let jump_ns = parse_f64(Some(&jump_raw), line_no, "jump", &mut b.diagnostics);
                b.clock_events.push(ClockEvent {
                    station: f[0].clone(), epoch, epoch_raw,
                    jump_ns_raw: jump_raw, jump_ns,
                    source: f.get(3).and_then(|x| kv(x, "source")).unwrap_or("log").into(),
                    unit: "ns".into(),
                });
            }
            "SATSTATE" => {
                let f = fields(rest);
                if f.len() < 3 { let _ = fail("SATSTATE 需要 sat | seconds | x= y= z= clock="); continue; }
                let epoch_raw = f[1].clone();
                let epoch = parse_f64(Some(&epoch_raw), line_no, "epoch", &mut b.diagnostics)
                    .unwrap_or(f64::NAN);
                let geo = &f[2];
                let x_raw = kv(geo, "x").unwrap_or("").to_string();
                let y_raw = kv(geo, "y").unwrap_or("").to_string();
                let z_raw = kv(geo, "z").unwrap_or("").to_string();
                let clock_raw = kv(geo, "clock").unwrap_or("").to_string();
                b.sat_states.push(SatState {
                    sat: f[0].clone(), epoch, epoch_raw,
                    x_raw: x_raw.clone(), y_raw: y_raw.clone(),
                    z_raw: z_raw.clone(), clock_raw: clock_raw.clone(),
                    x_m: parse_f64(Some(&x_raw), line_no, "x", &mut b.diagnostics),
                    y_m: parse_f64(Some(&y_raw), line_no, "y", &mut b.diagnostics),
                    z_m: parse_f64(Some(&z_raw), line_no, "z", &mut b.diagnostics),
                    clock_s: parse_f64(Some(&clock_raw), line_no, "clock", &mut b.diagnostics),
                    datum: kv(geo, "datum").unwrap_or("ITRF2014").into(),
                });
            }
            "OBS" => {
                let f = fields(rest);
                if f.len() < 3 { let _ = fail("OBS 需要 station | seconds | sat | band code | 量..."); continue; }
                let id: Vec<&str> = f[2].split_whitespace().collect();
                if id.len() < 2 { let _ = fail("OBS 第三段需要 '卫星 观测码'（如 G01 L1C）"); continue; }
                let code_full = id[1].to_string();
                let band_digit = code_full.chars().nth(1).unwrap_or('1').to_string();
                let attr: String = code_full.chars().skip(2).collect();
                let pr_key = format!("C{band_digit}{attr}");
                let cp_key = format!("L{band_digit}{attr}");
                let do_key = format!("D{band_digit}{attr}");
                let sn_key = format!("S{band_digit}{attr}");
                let epoch_raw = f[1].clone();
                let epoch = parse_f64(Some(&epoch_raw), line_no, "epoch", &mut b.diagnostics)
                    .unwrap_or(f64::NAN);
                let measures = if f.len() >= 5 { f[4].clone() } else { f.get(3).cloned().unwrap_or_default() };
                let token = |name: &str| kv(&measures, name).map(|s| s.to_string());
                let pr_raw = token(&pr_key);
                let cp_raw = token(&cp_key);
                let do_raw = token(&do_key);
                let sn_raw = token(&sn_key);
                let lli_raw = token("LLI");
                let mut obs = RawObservation {
                    station: f[0].clone(), sat: id[0].into(), epoch, epoch_raw,
                    band: band_digit,
                    code: code_full,
                    pseudorange_raw: pr_raw.clone(), carrier_raw: cp_raw.clone(),
                    doppler_raw: do_raw.clone(), lli_raw: lli_raw.clone(), snr_raw: sn_raw.clone(),
                    pseudorange_m: pr_raw.as_deref().and_then(|v| parse_f64(Some(v), line_no, "pseudorange", &mut b.diagnostics)),
                    carrier_cycles: cp_raw.as_deref().and_then(|v| parse_f64(Some(v), line_no, "carrier", &mut b.diagnostics)),
                    doppler_hz: do_raw.as_deref().and_then(|v| parse_f64(Some(v), line_no, "doppler", &mut b.diagnostics)),
                    lli_flag: lli_raw.as_deref().and_then(|v| v.parse::<i64>().ok()),
                    snr_db_hz: sn_raw.as_deref().and_then(|v| parse_f64(Some(v), line_no, "snr", &mut b.diagnostics)),
                    units: RawUnits {
                        pseudorange: "m".into(), carrier: "cycles".into(), doppler: "Hz".into(),
                        lli: "flag".into(), snr: "dBHz".into(),
                    },
                };
                b.observations.push(obs);
            }
            // EPOCH 行可选，主要是导入层显式保留不等间隔历元声明；观测历元本身仍以 OBS 为准。
            "EPOCH" => {
                let f = fields(rest);
                if f.len() < 2 { let _ = fail("EPOCH 需要 station | seconds"); continue; }
                let _ = parse_f64(Some(&f[1]), line_no, "epoch", &mut b.diagnostics);
            }
            _ => {
                if fatal.is_none() {
                    fatal = Some(format!("第 {line_no} 行：未知记录类型 `{head}`"));
                }
            }
        }
        if fatal.is_some() {
            break;
        }
    }

    if fatal.is_none() {
        if b.case_id.is_empty() {
            fatal = Some("缺少 CASE 声明".into());
        } else if b.freqs.is_empty() {
            fatal = Some("缺少 FREQDEF 频率定义".into());
        } else if b.observations.is_empty() {
            fatal = Some("案例中没有任何 OBS 观测记录".into());
        }
    }
    ParseOutcome { bundle: b, fatal }
}
