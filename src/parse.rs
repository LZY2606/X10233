//! Parser for the copyable observation sample format.
//!
//! Format (line based, `#` comments):
//!   EPOCH <epoch_id> <iso_label> <t_seconds>
//!   FRQNUM <sat> <channel>                 (GLONASS frequency number)
//!   CLKJUMP <epoch_id> <milliseconds>      (receiver-recorded clock jump)
//!   SATSTATE <epoch_id> <sat> <x> <y> <z> <datum>
//!   OBS <epoch_id> <sat> <code> <value> [lli]
//!
//! Structural problems (unknown keyword, wrong field count, reference to an
//! undeclared epoch) are fatal import errors. A single unconvertible value
//! is NOT fatal: the raw text stays in place and a diagnostic is attached.

use crate::model::*;

#[derive(Debug, Default)]
pub struct ParsedFile {
    pub epochs: Vec<(String, String, f64)>, // id, raw label, t seconds
    pub obs: Vec<NormObs>,
    pub states: Vec<SatState>,
    pub freq_numbers: Vec<(String, i32)>,
    pub clock_events: Vec<ClockEventRaw>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct ImportError {
    pub line: usize,
    pub message: String,
    pub raw: String,
}

pub fn parse_obs_file(text: &str) -> Result<ParsedFile, Vec<ImportError>> {
    let mut out = ParsedFile::default();
    let mut errors = Vec::new();
    let mut epoch_t: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let fail = |errors: &mut Vec<ImportError>, msg: &str| {
            errors.push(ImportError {
                line: line_no,
                message: msg.to_string(),
                raw: raw_line.to_string(),
            });
        };
        match parts[0] {
            "EPOCH" => {
                if parts.len() != 4 {
                    fail(&mut errors, "EPOCH expects: EPOCH <id> <iso_label> <t_seconds>");
                    continue;
                }
                match parts[3].parse::<f64>() {
                    Ok(t) => {
                        epoch_t.insert(parts[1].to_string(), t);
                        out.epochs.push((parts[1].to_string(), parts[2].to_string(), t));
                    }
                    Err(_) => fail(&mut errors, "EPOCH t_seconds is not a number"),
                }
            }
            "FRQNUM" => {
                if parts.len() != 3 {
                    fail(&mut errors, "FRQNUM expects: FRQNUM <sat> <channel>");
                    continue;
                }
                match parts[2].parse::<i32>() {
                    Ok(k) => out.freq_numbers.push((parts[1].to_string(), k)),
                    Err(_) => fail(&mut errors, "FRQNUM channel is not an integer"),
                }
            }
            "CLKJUMP" => {
                if parts.len() != 3 {
                    fail(&mut errors, "CLKJUMP expects: CLKJUMP <epoch_id> <ms>");
                    continue;
                }
                let Some(&t) = epoch_t.get(parts[1]) else {
                    fail(&mut errors, "CLKJUMP references undeclared epoch");
                    continue;
                };
                match parts[2].parse::<f64>() {
                    Ok(ms) => out.clock_events.push(ClockEventRaw {
                        epoch_id: parts[1].to_string(),
                        t,
                        ms,
                        raw: raw_line.to_string(),
                    }),
                    Err(_) => fail(&mut errors, "CLKJUMP milliseconds is not a number"),
                }
            }
            "SATSTATE" => {
                if parts.len() != 7 {
                    fail(&mut errors, "SATSTATE expects: SATSTATE <epoch> <sat> <x> <y> <z> <datum>");
                    continue;
                }
                let (x, y, z) = (
                    parts[3].parse::<f64>(),
                    parts[4].parse::<f64>(),
                    parts[5].parse::<f64>(),
                );
                match (x, y, z) {
                    (Ok(x), Ok(y), Ok(z)) => out.states.push(SatState {
                        epoch_id: parts[1].to_string(),
                        sat: parts[2].to_string(),
                        x,
                        y,
                        z,
                        datum: parts[6].to_string(),
                    }),
                    _ => fail(&mut errors, "SATSTATE coordinate is not a number"),
                }
            }
            "OBS" => {
                if parts.len() != 5 && parts.len() != 6 {
                    fail(&mut errors, "OBS expects: OBS <epoch> <sat> <code> <value> [lli]");
                    continue;
                }
                let Some(&t) = epoch_t.get(parts[1]) else {
                    fail(&mut errors, "OBS references undeclared epoch");
                    continue;
                };
                let Some(code) = ObsCode::parse(parts[3]) else {
                    fail(&mut errors, "OBS code not understood (want C/L/D/S + band 1/2)");
                    continue;
                };
                let value = match parts[4].parse::<f64>() {
                    Ok(v) => Some(v),
                    Err(_) => {
                        // Unconvertible value: keep raw in place, attach diagnostic.
                        out.diagnostics.push(Diagnostic {
                            scope: format!("obs:{}/{}/{}", parts[1], parts[2], parts[3]),
                            message: "unconvertible value kept raw; excluded from normalization"
                                .to_string(),
                            raw: raw_line.to_string(),
                        });
                        None
                    }
                };
                let lli = if parts.len() == 6 {
                    match parts[5].parse::<i64>() {
                        Ok(v) => v,
                        Err(_) => {
                            out.diagnostics.push(Diagnostic {
                                scope: format!("lli:{}/{}/{}", parts[1], parts[2], parts[3]),
                                message: "unconvertible LLI flag; stored as 0".to_string(),
                                raw: raw_line.to_string(),
                            });
                            0
                        }
                    }
                } else {
                    0
                };
                out.obs.push(NormObs {
                    epoch_id: parts[1].to_string(),
                    t,
                    sat: parts[2].to_string(),
                    code: code.raw.clone(),
                    field: code.field,
                    band: code.band,
                    value,
                    unit: code.field.unit().to_string(),
                    lli,
                    raw: raw_line.to_string(),
                });
            }
            other => fail(&mut errors, &format!("unknown keyword '{other}'")),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    if out.epochs.is_empty() {
        return Err(vec![ImportError {
            line: 0,
            message: "file declares no EPOCH lines".to_string(),
            raw: String::new(),
        }]);
    }
    Ok(out)
}
