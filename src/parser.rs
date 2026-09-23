// RINEX-observation-subset parser used by the offline case pack.
//
// Contract:
//   * Every input line is stored verbatim (including comments).
//   * Raw token representations (phase/pseudorange/Doppler/SNR/LLI) are
//     preserved on the observation; normalised numeric values live on derived
//     columns. A value that cannot be converted stays in place with
//     value_status='unconvertible' and a diagnostic; it is never dropped.
//   * Frequency basis and observation codes are kept as declared (frequency
//     number for GLONASS, datum/units from the header).

use crate::models::{id_freq, id_line, id_obs};

#[derive(Debug, Clone, Default)]
pub struct ParsedHeader {
    pub station: Vec<(String, String)>,
    pub datum: Vec<(String, String)>,
    pub freqs: Vec<FreqDecl>,
}

#[derive(Debug, Clone)]
pub struct FreqDecl {
    pub gnss: String,
    pub band: String,
    pub freq_hz: f64,
    pub freq_number: Option<i64>,
    pub codes_raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct EpochDecl {
    pub idx: i64,
    pub t_seconds: f64,
    pub iso_tag: String,
    pub rcvr_clk_s: String,
    pub clock_event_flag: i64,
}

#[derive(Debug, Clone)]
pub struct ObsDecl {
    pub epoch_idx: i64,
    pub sat: String,
    pub gnss: String,
    pub band: String,
    pub code_phase: String,
    pub code_pseudo: Option<String>,
    pub code_doppler: Option<String>,
    pub code_snr: Option<String>,
    pub code_lli: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ClockDecl {
    pub epoch_idx: i64,
    pub delta_s_raw: String,
    pub scope_raw: String,
    pub cause_raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct AmendDecl {
    pub target_sat: String,
    pub epoch_idx: i64,
    pub mode: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum Directive {
    Station(String),
    Datum(String),
    Freq(FreqDecl),
    Epoch(EpochDecl),
    Obs(ObsDecl),
    Clock(ClockDecl),
    Amend(AmendDecl),
    Comment,
}

#[derive(Debug, Clone)]
pub struct ParsedLine {
    pub line_no: i64,
    pub raw_text: String,
    pub directive_name: String,
    pub directive: Option<Directive>,
    pub status: String,
    pub diagnostic: Option<String>,
    pub stable_line_id: String,
}

pub struct ParseOutput {
    pub lines: Vec<ParsedLine>,
    pub header: ParsedHeader,
}

fn kv(rest: &str) -> Vec<(String, String)> {
    rest.split_whitespace()
        .filter_map(|tok| {
            let mut it = tok.splitn(2, '=');
            let k = it.next()?.to_string();
            let v = it.next().unwrap_or("").to_string();
            Some((k, v))
        })
        .collect()
}

fn band_prefix_for_sat(sat: &str) -> &'static str {
    match sat.chars().next() {
        Some('R') => "R",
        Some('E') => "E",
        Some('C') => "C",
        _ => "G",
    }
}

// Parse one OBS line. `cur_epoch` associates it with an epoch.
pub fn parse_obs(tokens: &[&str], cur_epoch: i64) -> Result<ObsDecl, String> {
    // OBS <sat> <band> P2=.. L2=.. D2=.. S2=.. LLI2=..
    if tokens.len() < 4 {
        return Err("OBS requires at least OBS <sat> <band> <phase=...>".to_string());
    }
    let sat = tokens[1].to_string();
    let band = tokens[2].to_string();
    let map = kv(&tokens[3..].join(" "));
    let get = |key: &str| map.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let phase = get(&band).ok_or_else(|| format!("missing carrier token {band}"))?;
    let pkey = format!("P{}", &band[1..]);
    let dkey = format!("D{}", &band[1..]);
    let skey = format!("S{}", &band[1..]);
    let lkey = format!("LLI{}", &band[1..]);
    Ok(ObsDecl {
        epoch_idx: cur_epoch,
        gnss: band_prefix_for_sat(&sat).to_string(),
        sat,
        band,
        code_phase: phase,
        code_pseudo: get(&pkey),
        code_doppler: get(&dkey),
        code_snr: get(&skey),
        code_lli: get(&lkey),
    })
}

pub fn parse(content: &str, submission_id: &str) -> ParseOutput {
    let mut header = ParsedHeader::default();
    let mut lines = Vec::new();
    let mut cur_epoch: i64 = 0;

    for (idx, raw) in content.lines().enumerate() {
        let line_no = (idx + 1) as i64;
        let trimmed = raw.trim();
        let stable_line_id = id_line(submission_id, line_no);
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            lines.push(ParsedLine {
                line_no, raw_text: raw.to_string(), directive_name: "#".to_string(),
                directive: Some(Directive::Comment), status: "ok".to_string(),
                diagnostic: None, stable_line_id,
            });
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        let head = tokens[0];
        let rest = tokens.get(1..).unwrap_or(&[]).join(" ");
        let mut status = "ok".to_string();
        let mut diagnostic = None;
        let directive = match head {
            "STATION" => {
                for (k, v) in kv(&rest) {
                    header.station.push((k, v));
                }
                Some(Directive::Station(rest.clone()))
            }
            "DATUM" => {
                for (k, v) in kv(&rest) {
                    header.datum.push((k, v));
                }
                Some(Directive::Datum(rest.clone()))
            }
            "FREQ" => parse_freq(tokens.get(1..).unwrap_or(&[]))
                .map(|f| {
                    header.freqs.push(f.clone());
                    Directive::Freq(f)
                })
                .unwrap_or_else(|e| {
                    status = "error".to_string();
                    diagnostic = Some(e);
                    Directive::Comment
                })
                .into(),
            "EPOCH" => match parse_epoch(&rest) {
                Ok(e) => {
                    cur_epoch = e.idx;
                    Some(Directive::Epoch(e))
                }
                Err(e) => {
                    status = "error".to_string();
                    diagnostic = Some(e);
                    Some(Directive::Comment)
                }
            },
            "OBS" => match parse_obs(&tokens, cur_epoch) {
                Ok(o) => Some(Directive::Obs(o)),
                Err(e) => {
                    status = "error".to_string();
                    diagnostic = Some(e);
                    Some(Directive::Comment)
                }
            },
            "CLOCKJUMP" => match parse_clock(&rest, cur_epoch) {
                Ok(c) => Some(Directive::Clock(c)),
                Err(e) => {
                    status = "error".to_string();
                    diagnostic = Some(e);
                    Some(Directive::Comment)
                }
            },
            "AMEND" => match parse_amend(&rest) {
                Ok(a) => {
                    if a.epoch_idx > 0 {
                        cur_epoch = a.epoch_idx;
                    }
                    Some(Directive::Amend(a))
                }
                Err(e) => {
                    status = "error".to_string();
                    diagnostic = Some(e);
                    Some(Directive::Comment)
                }
            },
            other => {
                status = "error".to_string();
                diagnostic = Some(format!("unknown directive '{other}'"));
                Some(Directive::Comment)
            }
        };

        lines.push(ParsedLine {
            line_no,
            raw_text: raw.to_string(),
            directive_name: head.to_string(),
            directive,
            status,
            diagnostic,
            stable_line_id: id_line(submission_id, line_no),
        });
    }

    ParseOutput { lines, header }
}

fn parse_freq(tokens: &[&str]) -> Result<FreqDecl, String> {
    if tokens.len() < 2 {
        return Err("FREQ requires <gnss> <band> f=.. codes=..".into());
    }
    let gnss = tokens[0].to_string();
    let band = tokens[1].to_string();
    let map = kv(&tokens[2..].join(" "));
    let get = |k: &str| map.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let f_raw = get("f").ok_or("FREQ missing f=")?;
    let f: f64 = f_raw.parse().map_err(|_| format!("bad frequency '{f_raw}'"))?;
    let k = get("k").and_then(|v| v.parse::<i64>().ok());
    let codes = get("codes").unwrap_or_default();
    Ok(FreqDecl {
        gnss,
        band,
        freq_hz: f,
        freq_number: k,
        codes_raw: codes,
    })
}

fn parse_epoch(rest: &str) -> Result<EpochDecl, String> {
    let map = kv(rest);
    let get = |k: &str| map.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let idx: i64 = get("idx")
        .ok_or("EPOCH missing idx")?
        .parse()
        .map_err(|_| "bad idx".to_string())?;
    let t: f64 = get("t")
        .ok_or("EPOCH missing t")?
        .parse()
        .map_err(|_| "bad t".to_string())?;
    Ok(EpochDecl {
        idx,
        t_seconds: t,
        iso_tag: get("iso").unwrap_or_default(),
        rcvr_clk_s: get("rcvr_clk_s").unwrap_or_else(|| "0".to_string()),
        clock_event_flag: get("clock_event_flag")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    })
}

fn parse_clock(rest: &str, cur_epoch: i64) -> Result<ClockDecl, String> {
    let map = kv(rest);
    let get = |k: &str| map.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    let idx = get("t")
        .and_then(|v| v.parse::<i64>().ok())
        .map(|_| cur_epoch)
        .unwrap_or(cur_epoch);
    Ok(ClockDecl {
        epoch_idx: idx,
        delta_s_raw: get("delta_s").unwrap_or_default(),
        scope_raw: get("scope").unwrap_or_else(|| "receiver".to_string()),
        cause_raw: get("cause").unwrap_or_default(),
    })
}

fn parse_amend(rest: &str) -> Result<AmendDecl, String> {
    let map = kv(rest);
    let get = |k: &str| map.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
    Ok(AmendDecl {
        target_sat: get("target").unwrap_or_default(),
        epoch_idx: get("epoch_idx").and_then(|v| v.parse().ok()).unwrap_or(0),
        mode: get("mode").unwrap_or_default(),
        reason: get("reason").unwrap_or_default(),
    })
}

// Try to normalise a raw token; failure is reported, never silently dropped.
pub fn normalise_f64(raw: &str) -> Result<f64, String> {
    raw.trim()
        .parse::<f64>()
        .map_err(|_| format!("unconvertible numeric value '{raw}'"))
}
pub fn normalise_int(raw: &str) -> Result<i64, String> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| format!("unconvertible integer value '{raw}'"))
}

// Suppress unused warnings for helpers retained for clarity of the identity API.
#[allow(dead_code)]
fn _ids(project: &str, sub: &str, sat: &str, ep: i64, band: &str, gnss: &str) {
    let _ = id_obs(project, sub, sat, ep, band);
    let _ = id_freq(project, gnss, band);
}
