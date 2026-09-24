use crate::model::{obs_code_unit, Observation, Quantity, TIME_DATUM};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClockEvent {
    pub epoch: f64,
    pub kind: String,
    pub raw_flag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParseOutput {
    pub station: String,
    pub obs_types: Vec<String>,
    pub epochs: Vec<f64>,
    pub observations: Vec<Observation>,
    pub clock_events: Vec<ClockEvent>,
    pub diagnostics: Vec<String>,
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn gps_seconds(y: i64, mo: i64, d: i64, h: i64, mi: i64, sec: f64) -> f64 {
    let gps_epoch = days_from_civil(1980, 1, 6);
    let day = days_from_civil(y, mo, d);
    ((day - gps_epoch) * 86400 + h * 3600 + mi * 60) as f64 + sec
}

fn label_of(line: &str) -> &str {
    if line.len() > 60 {
        line[60..].trim()
    } else {
        ""
    }
}

pub fn parse_rinex_obs(content: &str) -> Result<ParseOutput, Vec<ParseError>> {
    let lines: Vec<&str> = content.lines().collect();
    let mut errors = Vec::new();
    let mut station = String::from("UNKN");
    let mut obs_types: Vec<String> = Vec::new();
    let mut header_end = None;

    for (idx, line) in lines.iter().enumerate() {
        let label = label_of(line);
        match label {
            "MARKER NAME" => {
                let name = line.get(0..60).unwrap_or(line).trim();
                if !name.is_empty() {
                    station = name.chars().take(8).collect();
                }
            }
            "SYS / # / OBS TYPES" => {
                let body = line.get(0..60).unwrap_or(line);
                let mut it = body.split_whitespace();
                let _sys = it.next();
                if let Some(n) = it.next().and_then(|s| s.parse::<usize>().ok()) {
                    let types: Vec<String> = it.take(n).map(|s| s.to_string()).collect();
                    if types.len() != n {
                        errors.push(ParseError {
                            line: idx + 1,
                            message: format!(
                                "SYS / # / OBS TYPES declares {n} types but {} found",
                                types.len()
                            ),
                        });
                    }
                    obs_types = types;
                } else {
                    errors.push(ParseError {
                        line: idx + 1,
                        message: "SYS / # / OBS TYPES missing type count".into(),
                    });
                }
            }
            "END OF HEADER" => {
                header_end = Some(idx);
                break;
            }
            _ => {}
        }
    }

    let header_end = match header_end {
        Some(h) => h,
        None => {
            errors.push(ParseError {
                line: lines.len(),
                message: "END OF HEADER not found".into(),
            });
            return Err(errors);
        }
    };
    if obs_types.is_empty() {
        errors.push(ParseError {
            line: header_end + 1,
            message: "no SYS / # / OBS TYPES header record".into(),
        });
        return Err(errors);
    }

    let mut epochs = Vec::new();
    let mut observations = Vec::new();
    let mut clock_events = Vec::new();
    let mut diagnostics = Vec::new();
    let mut idx = header_end + 1;

    while idx < lines.len() {
        let line = lines[idx];
        if line.trim().is_empty() {
            idx += 1;
            continue;
        }
        if !line.starts_with('>') {
            errors.push(ParseError {
                line: idx + 1,
                message: format!("expected epoch record starting with '>', got: {}", &line[..line.len().min(40)]),
            });
            idx += 1;
            continue;
        }
        let body = &line[1..];
        let fields: Vec<&str> = body.split_whitespace().collect();
        if fields.len() < 8 {
            errors.push(ParseError {
                line: idx + 1,
                message: format!("epoch record needs 8 fields, got {}", fields.len()),
            });
            idx += 1;
            continue;
        }
        let nums: Result<Vec<f64>, _> = fields[..6].iter().map(|s| s.parse::<f64>()).collect();
        let nums = match nums {
            Ok(n) => n,
            Err(_) => {
                errors.push(ParseError {
                    line: idx + 1,
                    message: "epoch record has non-numeric date/time field".into(),
                });
                idx += 1;
                continue;
            }
        };
        let epoch = gps_seconds(
            nums[0] as i64,
            nums[1] as i64,
            nums[2] as i64,
            nums[3] as i64,
            nums[4] as i64,
            nums[5],
        );
        let flag = fields[6];
        let nsat: usize = match fields[7].parse() {
            Ok(n) => n,
            Err(_) => {
                errors.push(ParseError {
                    line: idx + 1,
                    message: format!("epoch satellite count not a number: {}", fields[7]),
                });
                idx += 1;
                continue;
            }
        };
        epochs.push(epoch);
        if flag == "1" {
            clock_events.push(ClockEvent {
                epoch,
                kind: "receiver_clock_jump".into(),
                raw_flag: flag.to_string(),
            });
        } else if flag != "0" {
            diagnostics.push(format!(
                "epoch {epoch}: unsupported epoch flag {flag}, treated as normal"
            ));
        }
        idx += 1;
        for _ in 0..nsat {
            if idx >= lines.len() {
                errors.push(ParseError {
                    line: lines.len(),
                    message: format!("epoch {epoch}: expected {nsat} satellite lines, file ended"),
                });
                break;
            }
            let sat_line = lines[idx];
            if sat_line.len() < 3 {
                errors.push(ParseError {
                    line: idx + 1,
                    message: "satellite line too short for satellite id".into(),
                });
                idx += 1;
                continue;
            }
            let sat: String = sat_line[..3].trim().to_string();
            if sat.is_empty() {
                errors.push(ParseError {
                    line: idx + 1,
                    message: "satellite line missing satellite id".into(),
                });
                idx += 1;
                continue;
            }
            let padded = format!("{:<width$}", sat_line, width = 3 + 16 * obs_types.len());
            let bytes = padded.as_bytes();
            for (t_idx, code) in obs_types.iter().enumerate() {
                let start = 3 + 16 * t_idx;
                let field = &bytes[start..start + 16];
                let value_str = String::from_utf8_lossy(&field[0..14]).trim().to_string();
                if value_str.is_empty() {
                    continue;
                }
                let lli = (field[14] as char).to_digit(10).map(|d| d as u8);
                let ssi = (field[15] as char).to_digit(10).map(|d| d as u8);
                let mut diagnostics = Vec::new();
                let unit = match obs_code_unit(code) {
                    Some(u) => u,
                    None => {
                        diagnostics.push(format!("unknown observation code class: {code}"));
                        crate::model::Unit::Dimensionless
                    }
                };
                let quantity = match value_str.parse::<f64>() {
                    Ok(v) => Some(Quantity {
                        value: v,
                        unit,
                        datum: Some(TIME_DATUM.to_string()),
                    }),
                    Err(_) => {
                        diagnostics.push(format!(
                            "unconvertible value '{value_str}' kept in raw form"
                        ));
                        None
                    }
                };
                let mut obs = Observation {
                    id: String::new(),
                    station: station.clone(),
                    sat: sat.clone(),
                    epoch,
                    epoch_datum: TIME_DATUM.to_string(),
                    code: code.clone(),
                    freq: crate::model::obs_band(code),
                    raw: value_str,
                    quantity,
                    lli,
                    ssi,
                    diagnostics,
                };
                obs.compute_id();
                observations.push(obs);
            }
            idx += 1;
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ParseOutput {
        station,
        obs_types,
        epochs,
        observations,
        clock_events,
        diagnostics,
    })
}
