//! 导入层：可复制的小型 obsx 文本格式（人能直接读懂、可手改）。
//!
//! 原始层只做“原样保存 + 尽力规范化”，不做业务派生：
//! - 伪距(m)、载波(cycle)、Doppler(Hz)、LLI(原始字符)、信噪比(dBHz)、
//!   钟跳事件、频率编号、观测码 均按历元原样保存；
//! - 无法转换的值留在原位置并附诊断（normalized=None），不会为画图而丢弃。
//!
//! 格式（每行 `#` 为注释，字段以空白分隔）：
//! STATION <id>
//! CODE <band> <pr_code> <ph_code> <dop_code> <snr_code>
//! FREQ <band> <hz>
//! EPOCH <rfc3339> <ord>
//! SAT <sat> <azimuth_deg> <zenith_deg> <health>
//! OBS <sat> <band> <pseudorange_m_token> <carrier_cycle_token>
//!     <doppler_hz_token> <lli_char> <snr_dbhz_token>
//! CLOCK <epoch_rfc3339> <shift_seconds_token>

use crate::model::*;
use crate::model::units::*;

const C: f64 = 299_792_458.0;

/// 致命格式错误：整个文件拒绝（不产生版本）。
#[derive(Debug, Clone)]
pub struct IngestError {
    pub line_no: u32,
    pub message: String,
}
impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line_no, self.message)
    }
}
impl std::error::Error for IngestError {}

fn err(line: u32, msg: impl Into<String>) -> IngestError {
    IngestError { line_no: line, message: msg.into() }
}

fn parse_num(token: &str, want_unit: &'static str) -> RawValue {
    let t = token.trim();
    match t.parse::<f64>() {
        Ok(v) => RawValue::parsed(t.to_string(), v, want_unit),
        Err(_) => {
            if t.is_empty() || t == "-" || t.eq_ignore_ascii_case("nan") || t.eq_ignore_ascii_case("null") {
                let mut v = RawValue::blank(want_unit);
                v.raw = t.to_string();
                v.diagnostics = vec![format!("missing token for value in {}", want_unit)];
                v
            } else {
                RawValue::unparseable(
                    t.to_string(),
                    want_unit,
                    format!("cannot parse `{}` as f64 ({})", t, want_unit),
                )
            }
        }
    }
}

fn valid_epoch(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
}

/// 解析一个 obsx 文件。成功返回版本负载；致命错误返回错误列表。
pub fn parse_obsx(text: &str) -> Result<VersionPayload, Vec<IngestError>> {
    let mut errors: Vec<IngestError> = vec![];
    let mut station: Option<String> = None;
    let mut freqs: std::collections::BTreeMap<String, FreqDef> = std::collections::BTreeMap::new();
    let mut sats: Vec<SatState> = vec![];
    let mut observations: Vec<Observation> = vec![];
    let mut clocks: Vec<ClockEvent> = vec![];
    let mut warnings: Vec<String> = vec![];

    let mut cur_epoch: Option<String> = None;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let head = toks[0];
        let push_err = |errors: &mut Vec<IngestError>, m: String| errors.push(err(line_no, m));

        match head {
            "STATION" => {
                if toks.len() != 2 {
                    push_err(&mut errors, "STATION requires exactly one id".into());
                } else {
                    station = Some(toks[1].to_string());
                }
            }
            "CODE" => {
                if toks.len() != 6 {
                    push_err(&mut errors, "CODE requires <band> <pr> <ph> <dop> <snr>".into());
                } else if let Some(f) = freqs.get_mut(toks[1]) {
                    f.code_pr = toks[2].into();
                    f.code_ph = toks[3].into();
                    f.code_dop = toks[4].into();
                    f.code_snr = toks[5].into();
                } else {
                    push_err(
                        &mut errors,
                        format!("CODE for band {} before its FREQ declaration", toks[1]),
                    );
                }
            }
            "FREQ" => {
                if toks.len() != 3 {
                    push_err(&mut errors, "FREQ requires <band> <hz>".into());
                } else {
                    let hz = match toks[2].parse::<f64>() {
                        Ok(v) if v > 0.0 => v,
                        _ => {
                            push_err(&mut errors, format!("FREQ hz `{}` not a positive number", toks[2]));
                            continue;
                        }
                    };
                    let band = toks[1].to_string();
                    freqs.entry(band.clone()).or_insert(FreqDef {
                        band,
                        frequency_hz: hz,
                        wavelength: C / hz,
                        code_pr: String::new(),
                        code_ph: String::new(),
                        code_dop: String::new(),
                        code_snr: String::new(),
                    });
                }
            }
            "EPOCH" => {
                if toks.len() != 3 {
                    push_err(&mut errors, "EPOCH requires <rfc3339> <ord>".into());
                } else if !valid_epoch(toks[1]) {
                    push_err(&mut errors, format!("EPOCH timestamp `{}` is not RFC3339", tooks_safe(&toks, 1)));
                } else if toks[2].parse::<i64>().is_err() {
                    push_err(&mut errors, format!("EPOCH ord `{}` not an integer", toks[2]));
                } else {
                    cur_epoch = Some(toks[1].to_string());
                }
            }
            "SAT" => {
                if toks.len() != 5 {
                    push_err(&mut errors, "SAT requires <sat> <az_deg> <zen_deg> <health>".into());
                } else if cur_epoch.is_none() {
                    push_err(&mut errors, "SAT before any EPOCH".into());
                } else {
                    let az = parse_num(toks[2], DEG);
                    let ze = parse_num(toks[3], DEG);
                    if !az.usable() {
                        warnings.push(format!("line {}: SAT {} azimuth left in place: {}", line_no, toks[1], az.diagnostics.join("; ")));
                    }
                    if !ze.usable() {
                        warnings.push(format!("line {}: SAT {} zenith left in place: {}", line_no, toks[1], ze.diagnostics.join("; ")));
                    }
                    sats.push(SatState {
                        sat: toks[1].into(),
                        epoch: cur_epoch.clone().unwrap(),
                        azimuth: az,
                        zenith: ze,
                        health: toks[4].into(),
                        datum: DATUM,
                    });
                }
            }
            "OBS" => {
                // OBS <sat> <band> <code_pr> <code_ph> <code_dop> <code_snr>
                //     <pr_m> <ph_cycle> <dop_hz> <lli> <snr_dbhz>
                if toks.len() != 12 {
                    push_err(
                        &mut errors,
                        "OBS requires 12 fields: sat band code_pr code_ph code_dop code_snr pr ph dop lli snr".into(),
                    );
                } else if station.is_none() {
                    push_err(&mut errors, "OBS before STATION".into());
                } else if cur_epoch.is_none() {
                    push_err(&mut errors, "OBS before any EPOCH".into());
                } else if !freqs.contains_key(toks[2]) {
                    push_err(&mut errors, format!("OBS band {} has no FREQ declaration", toks[2]));
                } else {
                    let pr = parse_num(toks[7], METER);
                    let ph = parse_num(toks[8], CYCLE);
                    let dop = parse_num(toks[9], HERTZ);
                    let lli = toks[10].to_string();
                    let snr = parse_num(toks[11], DBHZ);
                    for (name, v) in [("pseudorange", &pr), ("carrier", &ph), ("doppler", &dop), ("snr", &snr)] {
                        if !v.usable() {
                            warnings.push(format!(
                                "line {}: OBS {} {} {} left in place: {}",
                                line_no, toks[1], toks[2], name,
                                v.diagnostics.join("; ")
                            ));
                        }
                    }
                    observations.push(Observation {
                        station: station.clone().unwrap(),
                        sat: toks[1].into(),
                        epoch: cur_epoch.clone().unwrap(),
                        band: toks[2].into(),
                        code_pr: toks[3].into(),
                        code_ph: toks[4].into(),
                        code_dop: toks[5].into(),
                        code_snr: toks[6].into(),
                        pseudorange: pr,
                        carrier: ph,
                        doppler: dop,
                        lli,
                        snr,
                        line_no,
                    });
                }
            }
            "CLOCK" => {
                if toks.len() != 3 {
                    push_err(&mut errors, "CLOCK requires <epoch_rfc3339> <shift_seconds>".into());
                } else if station.is_none() {
                    push_err(&mut errors, "CLOCK before STATION".into());
                } else if !valid_epoch(toks[1]) {
                    push_err(&mut errors, format!("CLOCK epoch `{}` is not RFC3339", toks[1]));
                } else {
                    let shift = parse_num(toks[2], SECOND);
                    if !shift.usable() {
                        warnings.push(format!("line {}: CLOCK shift left in place: {}", line_no, shift.diagnostics.join("; ")));
                    }
                    clocks.push(ClockEvent {
                        station: station.clone().unwrap(),
                        epoch: toks[1].into(),
                        raw: toks[2].into(),
                        shift_seconds: shift,
                        kind: "receiver_clock_jump",
                        line_no,
                    });
                }
            }
            other => push_err(&mut errors, format!("unknown directive `{}`", other)),
        }
    }

    let station = match station {
        Some(s) => s,
        None => {
            errors.push(err(0, "missing STATION directive".to_string()));
            return Err(errors);
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }

    let mut freqs_v: Vec<FreqDef> = freqs.into_values().collect();
    freqs_v.sort_by(|a, b| a.band.cmp(&b.band));
    sats.sort_by(|a, b| (a.epoch.as_str(), a.sat.as_str()).cmp(&(b.epoch.as_str(), b.sat.as_str())));
    observations.sort_by(|a, b| {
        (a.epoch.as_str(), a.sat.as_str(), a.band.as_str(), a.line_no)
            .cmp(&(b.epoch.as_str(), b.sat.as_str(), b.band.as_str(), b.line_no))
    });
    clocks.sort_by_key(|c| (c.epoch.clone(), c.line_no));

    Ok(VersionPayload {
        station,
        freqs: freqs_v,
        sats,
        observations,
        clocks,
        warnings,
    })
}

fn toks_safe(t: &[&str], i: usize) -> String {
    t.get(i).copied().unwrap_or("?").to_string()
}
