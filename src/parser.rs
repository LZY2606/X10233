//! ROBS/1 parser.
//!
//! ROBS/1 is a small, copy-pasteable RINEX-observation-inspired text format
//! used by the bundled cases. Every value keeps its declared unit; the parser
//! preserves the original representation and attaches diagnostics rather than
//! discarding values that cannot be converted.
//!
//! Grammar:
//!   @FORMAT ROBS/1.0
//!   @STATION <id> <lat> <lon> <height>  datum=<datum>
//!   @COORDSYS datum=<datum>
//!   @FREQ <SYS> <BAND> <freq>[MHz] code=<obs-code>
//!   @BATCH <batch-id>
//!   @PARENT <batch-id>
//!   EPOCH <ISO8601> +<offset>[s]
//!   EVENT <KIND> +<delta>[s] source=<...>
//!   SAT <PRN>  <key>[<unit>]=<value> ...   (two-space indent, 8-space before)

use crate::model::*;

pub struct ParsedFile {
    pub batch_id: String,
    pub parent_batch_id: Option<String>,
    pub station_id: String,
    pub station_datum: Option<String>,
    pub frequencies: Vec<FrequencyDef>,
    pub epochs: Vec<EpochRec>,
    pub sat_rows: Vec<SatObs>,
    pub clock_events: Vec<ClockEventRec>,
    pub diagnostics: Vec<ImportDiagnostic>,
}

const SPEED_OF_LIGHT: f64 = 299792458.0;

pub fn parse(project_id: &str, text: &str) -> Result<ParsedFile, Vec<ImportDiagnostic>> {
    let mut batch_id = None;
    let mut parent_batch_id = None;
    let mut station_id = String::new();
    let mut station_datum = None;
    let mut frequencies: Vec<FrequencyDef> = Vec::new();
    let mut epochs: Vec<EpochRec> = Vec::new();
    let mut sat_rows: Vec<SatObs> = Vec::new();
    let mut clock_events: Vec<ClockEventRec> = Vec::new();
    let mut diagnostics: Vec<ImportDiagnostic> = Vec::new();

    let mut format_ok = false;
    let mut current_epoch: Option<EpochRec> = None;
    let mut row_ord: i64 = 0;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        macro_rules! err {
            ($code:expr, $msg:expr) => {{
                diagnostics.push(ImportDiagnostic {
                    line_no,
                    severity: "error".into(),
                    code: $code.into(),
                    message: $msg.into(),
                    raw_line: raw.to_string(),
                });
                continue;
            }};
        }
        if let Some(rest) = line.strip_prefix("@FORMAT") {
            let v = rest.trim();
            if v != "ROBS/1.0" {
                err!("unsupported-format", format!("unsupported format '{}'", v));
            }
            format_ok = true;
        } else if let Some(rest) = line.strip_prefix("@STATION") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() {
                err!("station-missing-id", "@STATION requires an id");
            }
            station_id = parts[0].to_string();
            for p in parts.iter().skip(1) {
                if let Some(d) = p.strip_prefix("datum=") {
                    station_datum = Some(d.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("@COORDSYS") {
            for p in rest.split_whitespace() {
                if let Some(d) = p.strip_prefix("datum=") {
                    station_datum = Some(d.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("@FREQ") {
            let p: Vec<&str> = rest.split_whitespace().collect();
            if p.len() < 4 {
                err!("freq-fields", "@FREQ requires SYS BAND freq[MHz] code=<code>");
            }
            let (system, band) = (p[0].to_string(), p[1].to_string());
            let tok = parse_token("FREQ", p[2]);
            let freq_mhz = match tok.value {
                Some(v) if tok.unit.as_deref() == Some("MHz") => v,
                _ => err!("freq-unit", "frequency must be numeric [MHz]"),
            };
            let code = p[3]
                .strip_prefix("code=")
                .unwrap_or(p[3])
                .to_string();
            let id = sid("frq", &[project_id, &system, &band]);
            frequencies.push(FrequencyDef {
                id,
                system,
                band,
                freq_hz: freq_mhz * 1e6,
                code,
            });
        } else if let Some(rest) = line.strip_prefix("@BATCH") {
            batch_id = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("@PARENT") {
            parent_batch_id = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("EPOCH") {
            let p: Vec<&str> = rest.split_whitespace().collect();
            if p.len() < 2 {
                err!("epoch-fields", "EPOCH requires <iso8601> +<offset>[s]");
            }
            let off = parse_token("OFFSET", p[1]);
            let off_s = match off.value {
                Some(v) if off.unit.as_deref() == Some("s") => v,
                _ => err!("epoch-offset-unit", "epoch offset must be numeric [s]"),
            };
            let ord = epochs.len() as i64;
            let epoch_id = sid("epc", &[project_id, p[0], &format!("{:.3}", off_s)]);
            let rec = EpochRec {
                epoch_id,
                ord,
                t0: p[0].to_string(),
                offset_s: off_s,
            };
            epochs.push(rec.clone());
            current_epoch = Some(rec);
        } else if let Some(rest) = line.strip_prefix("EVENT") {
            let ep = match &current_epoch {
                Some(e) => e.clone(),
                None => err!("event-without-epoch", "EVENT outside of an EPOCH block"),
            };
            let p: Vec<&str> = rest.split_whitespace().collect();
            if p.len() < 2 {
                err!("event-fields", "EVENT requires KIND +<delta>[s]");
            }
            let kind = p[0].to_string();
            let tok = parse_token("EVENT", p[1]);
            let delta_s = match tok.value {
                Some(v) if tok.unit.as_deref() == Some("s") => v,
                _ => err!("event-unit", "event delta must be numeric [s]"),
            };
            let mut source = "inline".to_string();
            for q in p.iter().skip(2) {
                if let Some(s) = q.strip_prefix("source=") {
                    source = s.to_string();
                }
            }
            let id = sid("clk", &[project_id, &ep.epoch_id, &kind, &format!("{:.7}", delta_s)]);
            clock_events.push(ClockEventRec {
                id,
                batch_id: batch_id.clone().unwrap_or_default(),
                epoch_id: ep.epoch_id,
                kind,
                delta_s,
                source,
                raw: raw.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("SAT") {
            let ep = match &current_epoch {
                Some(e) => e.clone(),
                None => err!("sat-without-epoch", "SAT outside of an EPOCH block"),
            };
            let p: Vec<&str> = rest.split_whitespace().collect();
            if p.is_empty() {
                err!("sat-fields", "SAT requires a PRN");
            }
            let satellite = p[0].to_string();
            let mut tokens: Vec<RawToken> = Vec::new();
            for word in p.iter().skip(1) {
                let tok = parse_token(line_no.to_string().as_str(), word);
                if tok.value.is_none() && tok.unit.is_some() {
                    // keep the raw token at its source location with diagnostic
                    diagnostics.push(ImportDiagnostic {
                        line_no,
                        severity: "warning".into(),
                        code: "unconvertible-value".into(),
                        message: format!(
                            "token '{}' kept but cannot be converted to {}: {}",
                            tok.key,
                            tok.unit.clone().unwrap_or_default(),
                            tok.diagnostic.clone().unwrap_or_default()
                        ),
                        raw_line: raw.to_string(),
                    });
                }
                tokens.push(tok);
            }
            let row_id = sid(
                "obs",
                &[project_id, &batch_id.clone().unwrap_or_default(), &ep.epoch_id, &satellite],
            );
            sat_rows.push(SatObs {
                row_id,
                batch_id: batch_id.clone().unwrap_or_default(),
                epoch_id: ep.epoch_id,
                ord: row_ord,
                satellite,
                tokens,
            });
            row_ord += 1;
        } else {
            err!("unknown-record", format!("line starts with no known keyword"));
        }
    }

    let mut structural: Vec<ImportDiagnostic> = Vec::new();
    if !format_ok {
        structural.push(ImportDiagnostic {
            line_no: 0,
            severity: "error".into(),
            code: "missing-format".into(),
            message: "missing @FORMAT ROBS/1.0 header".into(),
            raw_line: String::new(),
        });
    }
    if batch_id.is_none() {
        structural.push(ImportDiagnostic {
            line_no: 0,
            severity: "error".into(),
            code: "missing-batch".into(),
            message: "missing @BATCH declaration".into(),
            raw_line: String::new(),
        });
    }
    if frequencies.is_empty() {
        structural.push(ImportDiagnostic {
            line_no: 0,
            severity: "error".into(),
            code: "missing-frequencies".into(),
            message: "no @FREQ definitions".into(),
            raw_line: String::new(),
        });
    }
    if !structural.is_empty() {
        return Err(structural);
    }

    Ok(ParsedFile {
        batch_id: batch_id.unwrap(),
        parent_batch_id,
        station_id,
        station_datum,
        frequencies,
        epochs,
        sat_rows,
        clock_events,
        diagnostics,
    })
}

/// Parse a `KEY[unit]=value` token. `CLOCK_BIAS[s]=...` style keys keep their
/// underscores. The raw value is always retained.
pub fn parse_token(_ctx: &str, word: &str) -> RawToken {
    let (head, raw_value) = match word.split_once('=') {
        Some(x) => x,
        None => {
            return RawToken {
                key: word.to_string(),
                unit: None,
                raw_value: String::new(),
                value: None,
                diagnostic: Some("expected key=value".into()),
            }
        }
    };
    let (key, unit) = match head.split_once('[') {
        Some((k, u)) => (k.to_string(), Some(u.trim_end_matches(']').to_string())),
        None => (head.to_string(), None),
    };
    // LLI flags are digits without unit or float requirement.
    if key.starts_with("LLI") {
        if raw_value.chars().all(|c| c.is_ascii_digit()) {
            return RawToken {
                key,
                unit: Some("flag".into()),
                raw_value: raw_value.to_string(),
                value: raw_value.parse::<f64>().ok(),
                diagnostic: None,
            };
        }
        return RawToken {
            key,
            unit: Some("flag".into()),
            raw_value: raw_value.to_string(),
            value: None,
            diagnostic: Some("LLI flag must be digits 0-9".into()),
        };
    }
    let raw_num = raw_value.trim().trim_start_matches('+');
    match raw_num.parse::<f64>() {
        Ok(v) => {
            // normalize a few accepted aliases into canonical units
            let (canon_unit, canon_val, diag) = normalize(&key, unit.as_deref(), v);
            RawToken {
                key,
                unit: canon_unit,
                raw_value: raw_value.to_string(),
                value: Some(canon_val),
                diagnostic: diag,
            }
        }
        Err(e) => RawToken {
            key,
            unit,
            raw_value: raw_value.to_string(),
            value: None,
            diagnostic: Some(format!("not a number: {}", e)),
        },
    }
}

fn normalize(key: &str, unit: Option<&str>, v: f64) -> (Option<String>, f64, Option<String>) {
    let u = match unit {
        Some(u) => u,
        None => return (None, v, Some("no unit declared".into())),
    };
    // Accepted units, grouped by physical quantity. Unknown units are retained
    // but flagged so the value is never silently dropped for plotting.
    match u {
        "m" | "meters" => (Some("m".into()), v, None),
        "s" | "sec" | "seconds" => (Some("s".into()), v, None),
        "ms" => (Some("s".into()), v * 1e-3, None),
        "us" => (Some("s".into()), v * 1e-6, None),
        "Hz" => (Some("Hz".into()), v, None),
        "cyc" | "cycle" | "cycles" => (Some("cyc".into()), v, None),
        "MHz" => (Some("MHz".into()), v, None),
        "kHz" => (Some("MHz".into()), v * 1e-3, None),
        "dBHz" | "dB-Hz" => (Some("dBHz".into()), v, None),
        "deg" | "degree" | "degrees" => (Some("deg".into()), v, None),
        "rad" => (Some("deg".into()), v.to_degrees(), None),
        other => {
            // distance aliases to meters?
            let hint = if key.contains("AZ") || key.contains("EL") {
                "expected deg"
            } else if key.contains("L") && !key.contains("LLI") {
                "expected cyc"
            } else if key.starts_with('P') {
                "expected m"
            } else if key.starts_with('D') {
                "expected Hz"
            } else if key.starts_with('S') {
                "expected dBHz"
            } else {
                "unknown unit"
            };
            (Some(other.to_string()), v, Some(format!("unknown unit '{}'; {}", other, hint)))
        }
    }
}

/// Convert a length-domain residual in meters to cycles for a frequency.
pub fn meters_to_cycles(meters: f64, freq_hz: f64) -> f64 {
    meters * freq_hz / SPEED_OF_LIGHT
}

pub fn cycles_to_meters(cycles: f64, freq_hz: f64) -> f64 {
    cycles * SPEED_OF_LIGHT / freq_hz
}
