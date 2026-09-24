//! Import parser for the repo's RINEX-like observation text.
//!
//! Line grammar (whitespace separated, `#` starts a comment):
//!   OBS <epoch> <station> <sat> <freq> <code> <C_m> <L_cyc> <D_Hz> <LLI> <S_dBHz>
//!   CLK <epoch> <station> <clock_jump_s>
//!
//! Every field is stored exactly as written (`*_raw`). Conversion to numbers
//! happens here but failures never drop data: the raw token stays in place
//! and a diagnostic is attached to the row.

use crate::model::{format_unix_utc, ObsLine};

#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub lines: Vec<ObsLine>,
    /// Human/machine readable format errors: (line_no, message).
    pub errors: Vec<(i64, String)>,
}

fn parse_f64(tok: &str) -> Option<f64> {
    let t = tok.trim();
    if t.is_empty() || t.chars().all(|c| c == '*') {
        return None;
    }
    t.parse::<f64>().ok()
}

fn parse_i64(tok: &str) -> Option<i64> {
    tok.trim().parse::<i64>().ok()
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` into unix seconds. Returns None for
/// anything else; caller keeps the raw text and attaches a diagnostic.
pub fn parse_epoch(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 20 || !s.ends_with('Z') {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    if s.get(4..5)? != "-" { return None; }
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    if s.get(7..8)? != "-" { return None; }
    let d: i64 = s.get(8..10)?.parse().ok()?;
    if s.get(10..11)? != "T" { return None; }
    let h: i64 = s.get(11..13)?.parse().ok()?;
    if s.get(13..14)? != ":" { return None; }
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    if s.get(16..17)? != ":" { return None; }
    let sec_part = s.get(17..s.len() - 1)?;
    let secf: f64 = sec_part.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 24 || mi > 59 || secf >= 61.0 {
        return None;
    }
    // days-from-civil
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + h * 3600 + mi * 60) as f64 + secf)
}

pub fn parse_input(raw: &str) -> ParseOutcome {
    let mut out = ParseOutcome::default();
    let mut first_epoch: Option<f64> = None;
    for (idx, raw_line) in raw.lines().enumerate() {
        let line_no = (idx + 1) as i64;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let mut row = ObsLine { line_no, ..Default::default() };
        match toks.first().copied() {
            Some("OBS") => {
                if toks.len() != 11 {
                    out.errors.push((line_no, format!(
                        "OBS expects 11 fields, got {}: `{}`", toks.len(), line)));
                    continue;
                }
                row.kind = "obs".into();
                row.epoch_raw = toks[1].to_string();
                row.station = toks[2].to_string();
                row.sat = toks[3].to_string();
                row.freq = parse_i64(toks[4]);
                if row.freq.is_none() {
                    row.diags.push(format!("freq `{}` not an integer", toks[4]));
                }
                row.code = toks[5].to_string();
                row.c_raw = toks[6].to_string();
                row.c_val = parse_f64(toks[6]);
                if row.c_val.is_none() {
                    row.diags.push(format!("pseudorange `{}` kept raw (unconvertible)", toks[6]));
                }
                row.l_raw = toks[7].to_string();
                row.l_val = parse_f64(toks[7]);
                if row.l_val.is_none() {
                    row.diags.push(format!("carrier `{}` kept raw (unconvertible)", toks[7]));
                }
                row.d_raw = toks[8].to_string();
                row.d_val = parse_f64(toks[8]);
                if row.d_val.is_none() {
                    row.diags.push(format!("doppler `{}` kept raw (unconvertible)", toks[8]));
                }
                row.lli = parse_i64(toks[9]);
                if row.lli.is_none() {
                    row.diags.push(format!("lli `{}` not an integer", toks[9]));
                }
                row.s_raw = toks[10].to_string();
                row.s_val = parse_f64(toks[10]);
                if row.s_val.is_none() {
                    row.diags.push(format!("snr `{}` kept raw (unconvertible)", toks[10]));
                }
            }
            Some("CLK") => {
                if toks.len() != 4 {
                    out.errors.push((line_no, format!(
                        "CLK expects 4 fields, got {}: `{}`", toks.len(), line)));
                    continue;
                }
                row.kind = "clk".into();
                row.epoch_raw = toks[1].to_string();
                row.station = toks[2].to_string();
                row.clk_raw = toks[3].to_string();
                row.clk_val = parse_f64(toks[3]);
                if row.clk_val.is_none() {
                    row.diags.push(format!("clock jump `{}` kept raw (unconvertible)", toks[3]));
                }
            }
            Some(other) => {
                out.errors.push((line_no, format!("unknown record kind `{other}`")));
                continue;
            }
            None => continue,
        }
        match parse_epoch(&row.epoch_raw) {
            Some(t) => {
                let base = *first_epoch.get_or_insert(t);
                row.epoch_s = Some(t - base);
            }
            None => row.diags.push(format!("epoch `{}` unparseable, kept raw", row.epoch_raw)),
        }
        out.lines.push(row);
    }
    out
}

/// Render an epoch offset (seconds since file base) back to ISO for display.
pub fn epoch_label(epoch_s: f64, base_unix: i64) -> String {
    format_unix_utc(base_unix + epoch_s.round() as i64)
}
