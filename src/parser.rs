//! Parser for the documented `csbench-obs/1` format (a RINEX-OBS-derived
//! epoch snapshot). Design rules:
//!
//! * pseudorange, carrier, Doppler, LLI, C/N0, clock-jump events, frequency
//!   numbers and observation codes are stored per epoch exactly as written;
//! * unconvertible values stay in place (raw token + diagnostic), they are
//!   never dropped;
//! * malformed lines produce structured diagnostics rather than aborting.
use crate::model::*;

pub fn parse(input: &str) -> ImportedFile {
    let mut file = ImportedFile {
        station: StationMeta::default(),
        units: Units {
            code: "meter".into(),
            phase: "cycle".into(),
            doppler: "hertz".into(),
            cnr: "dB-Hz".into(),
            jump: "microsecond".into(),
            range: "meter".into(),
        },
        wavelengths: vec![],
        time_scale: "iso8601".into(),
        constellation: String::new(),
        epochs: vec![],
        diagnostics: vec![],
        format: String::new(),
    };

    let mut epoch: Option<EpochRecord> = None;
    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let head = parts.next().unwrap_or("");
        match head {
            "@CASE" => {}
            "@FORMAT" => file.format = parts.collect::<Vec<_>>().join(" "),
            "@TIMEFRAME" | "@TIMESCALE" => file.time_scale = parts.next().unwrap_or("iso8601").into(),
            "@CONSTELLATION" => file.constellation = parts.next().unwrap_or("").into(),
            "@STATION" => file.station = parse_station(line),
            "@UNIT" => parse_units(line, &mut file.units),
            "@WAVELENGTH" => parse_wavelengths(parts.collect(), &mut file),
            "@EPOCH" => {
                if let Some(done) = epoch.take() {
                    file.epochs.push(done);
                }
                let time_raw = parts.next().unwrap_or("").to_string();
                let time_s = iso_to_seconds(&time_raw);
                let time_diagnostic = if time_s.is_none() {
                    Some(format!("unparsed epoch timestamp '{time_raw}'"))
                } else {
                    None
                };
                epoch = Some(EpochRecord {
                    seq: file.epochs.len() as i64,
                    time_raw,
                    time_s,
                    time_unit: "second".into(),
                    time_diagnostic,
                    sats: vec![],
                    frequencies: vec![],
                    clock_events: vec![],
                    sat_states: vec![],
                });
            }
            "FREQ" => {
                if let Some(e) = epoch.as_mut() {
                    parse_freq(parts.collect(), e, line_no, raw_line, &mut file.diagnostics);
                } else {
                    diag(&mut file.diagnostics, line_no, "error", "orphan_frequency", raw_line);
                }
            }
            "OBS" => {
                if let Some(e) = epoch.as_mut() {
                    parse_obs(parts.collect(), e, line_no, raw_line, &mut file.diagnostics, &file.units);
                } else {
                    diag(&mut file.diagnostics, line_no, "error", "orphan_observation", raw_line);
                }
            }
            "SATSTATE" => {
                if let Some(e) = epoch.as_mut() {
                    parse_satstate(parts.collect(), e, line_no, raw_line, &mut file.diagnostics);
                } else {
                    diag(&mut file.diagnostics, line_no, "error", "orphan_satstate", raw_line);
                }
            }
            "@CLOCKJUMP" => {
                if let Some(e) = epoch.as_mut() {
                    e.clock_events.push(parse_clock(parts.collect(), line, line_no, &mut file.diagnostics));
                } else {
                    diag(&mut file.diagnostics, line_no, "error", "orphan_clockjump", raw_line);
                }
            }
            other => diag(
                &mut file.diagnostics,
                line_no,
                "warning",
                "unknown_directive",
                &format!("{other} <- line kept verbatim, ignored"),
            ),
        }
    }
    if let Some(done) = epoch.take() {
        file.epochs.push(done);
    }
    file
}

fn diag(v: &mut Vec<ParseDiagnostic>, line_no: usize, severity: &str, code: &str, raw_line: &str) {
    v.push(ParseDiagnostic {
        line_no,
        severity: severity.into(),
        code: code.into(),
        message: code.replace('_', " "),
        raw_line: raw_line.to_string(),
    });
}

fn parse_station(line: &str) -> StationMeta {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let mut s = StationMeta { raw: line.into(), ..Default::default() };
    s.id = toks.get(1).copied().unwrap_or("ST01").to_string();
    let mut i = 2;
    while i < toks.len() {
        let kv = toks[i];
        if let Some((k, val)) = kv.split_once('=') {
            match k {
                "lat" => s.lat = val.parse().ok(),
                "lon" => s.lon = val.parse().ok(),
                "height" => s.height = val.parse().ok(),
                "datum" => s.datum = val.into(),
                "receiver" => s.receiver = val.into(),
                "antenna" => s.antenna = val.into(),
                _ => {}
            }
        }
        i += 1;
    }
    s
}

fn parse_units(line: &str, u: &mut Units) {
    for tok in line.split_whitespace().skip(1) {
        if let Some((k, v)) = tok.split_once('=') {
            match k {
                "code" => u.code = v.into(),
                "phase" => u.phase = v.into(),
                "doppler" => u.doppler = v.into(),
                "cnr" => u.cnr = v.into(),
                "jump" => u.jump = v.into(),
                "range" => u.range = v.into(),
                _ => {}
            }
        }
    }
}

fn parse_wavelengths(toks: Vec<&str>, file: &mut ImportedFile) {
    let mut band: Option<String> = None;
    for tok in toks {
        if tok.starts_with('L') {
            band = Some(tok.to_string());
        } else if let (Some(b), Ok(v)) = (band.take(), tok.parse::<f64>()) {
            file.wavelengths.push((b, v));
        }
    }
}

fn parse_freq(
    toks: Vec<&str>,
    e: &mut EpochRecord,
    line_no: usize,
    raw_line: &str,
    diags: &mut Vec<ParseDiagnostic>,
) {
    let mut it = toks.into_iter();
    let Some(sat) = it.next() else {
        diag(diags, line_no, "error", "frequency_missing_satellite", raw_line);
        return;
    };
    let mut rec = FrequencyRecord { satellite: sat.into(), entries: vec![], source: String::new() };
    for tok in it {
        if let Some((k, v)) = tok.split_once('=') {
            if k == "source" {
                rec.source = v.into();
            } else {
                rec.entries.push((k.into(), v.into()));
                if v.parse::<f64>().is_err() {
                    diag(
                        diags,
                        line_no,
                        "warning",
                        "frequency_unconvertible",
                        &format!("{sat} {k}={v} kept raw"),
                    );
                }
            }
        }
    }
    e.frequencies.push(rec);
}

fn parse_obs(
    toks: Vec<&str>,
    e: &mut EpochRecord,
    line_no: usize,
    raw_line: &str,
    diags: &mut Vec<ParseDiagnostic>,
    units: &Units,
) {
    let mut it = toks.into_iter();
    let Some(sat) = it.next() else {
        diag(diags, line_no, "error", "observation_missing_satellite", raw_line);
        return;
    };
    let mut so = SatObs { satellite: sat.into(), measurements: vec![], lli_flags: vec![] };
    for tok in it {
        if tok == "|" {
            continue;
        }
        let Some((code, raw_val)) = tok.split_once('=') else {
            diag(diags, line_no, "warning", "observation_token_ignored", &format!("{sat} {tok}"));
            continue;
        };
        if code.starts_with("LLI") {
            match raw_val.parse::<i64>() {
                Ok(v) => so.lli_flags.push((code.to_string(), v)),
                Err(_) => diag(
                    diags,
                    line_no,
                    "warning",
                    "lli_unconvertible",
                    &format!("{sat} {code}={raw_val} kept raw"),
                ),
            }
            // LLI is also retained as a first-class measurement-like token.
            let (value_int, diagnostic) = match raw_val.parse::<i64>() {
                Ok(v) => (Some(v), None),
                Err(_) => (None, Some(format!("LLI '{raw_val}' not an integer"))),
            };
            so.measurements.push(Measurement {
                obs_code: code.into(),
                kind: MeasKind::Lli,
                band: band_of(code).to_string(),
                value_f64: value_int.map(|v| v as f64),
                value_int,
                missing: false,
                raw: raw_val.into(),
                unit: MeasKind::Lli.unit().into(),
                diagnostic,
            });
            continue;
        }
        let Some(kind) = MeasKind::from_code(code) else {
            diag(diags, line_no, "warning", "unknown_observation_code", &format!("{sat} {tok} kept raw"));
            continue;
        };
        let missing = raw_val == "MISSING" || raw_val.is_empty();
        let (value_f64, diagnostic) = if missing {
            (None, Some(format!("{code} missing at epoch {}", e.time_raw)))
        } else {
            match raw_val.parse::<f64>() {
                Ok(v) => (Some(v), None),
                Err(_) => (None, Some(format!("{code}='{raw_val}' not numeric, kept in place"))),
            }
        };
        let unit = match kind {
            MeasKind::Code => units.code.clone(),
            MeasKind::Phase => units.phase.clone(),
            MeasKind::Doppler => units.doppler.clone(),
            MeasKind::Cnr => units.cnr.clone(),
            MeasKind::Lli => "flag".into(),
        };
        so.measurements.push(Measurement {
            obs_code: code.into(),
            kind,
            band: band_of(code).to_string(),
            value_f64,
            value_int: None,
            missing,
            raw: raw_val.into(),
            unit,
            diagnostic,
        });
    }
    e.sats.push(so);
}

fn band_of(code: &str) -> &str {
    match code.chars().nth(1) {
        Some('1') => "L1",
        Some('2') => "L2",
        Some('5') => "L5",
        _ => "L?",
    }
}

fn parse_satstate(
    toks: Vec<&str>,
    e: &mut EpochRecord,
    line_no: usize,
    raw_line: &str,
    diags: &mut Vec<ParseDiagnostic>,
) {
    let mut it = toks.into_iter();
    let Some(sat) = it.next() else {
        diag(diags, line_no, "error", "satstate_missing_satellite", raw_line);
        return;
    };
    let mut st = SatState { satellite: sat.into(), entries: vec![], iono_l1_m: None, clk_bias_s: None };
    for tok in it {
        if let Some((k, v)) = tok.split_once('=') {
            st.entries.push((k.into(), v.into()));
            if let Ok(num) = v.parse::<f64>() {
                match k {
                    "iono_l1_m" => st.iono_l1_m = Some(num),
                    "clk_bias_s" => st.clk_bias_s = Some(num),
                    _ => {}
                }
            }
        }
    }
    e.sat_states.push(st);
}

fn parse_clock(
    toks: Vec<&str>,
    line: &str,
    line_no: usize,
    diags: &mut Vec<ParseDiagnostic>,
) -> ClockEvent {
    let mut ev = ClockEvent { source: String::new(), raw: line.into(), unit: "microsecond".into(), jump_us: None, diagnostic: None };
    let mut first = true;
    for tok in toks {
        if let Some((k, v)) = tok.split_once('=') {
            match k {
                "source" => ev.source = v.into(),
                "raw" => {}
                "unit" => ev.unit = v.into(),
                _ => {}
            }
        } else if first {
            first = false;
            match tok.parse::<f64>() {
                Ok(v) => ev.jump_us = Some(v),
                Err(_) => {
                    ev.diagnostic = Some(format!("clock jump '{tok}' not numeric; kept raw"));
                    diag(diags, line_no, "warning", "clockjump_unconvertible", line);
                }
            }
        }
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_preserves_raw_tokens() {
        let f = parse(
            "@EPOCH 2026-01-04T00:00:00\n\
             FREQ G01 L1=1575420000 L2=1227600000 source=RINEX\n\
             OBS G01 | C1C=100.5 L1C=12.0 D1C=0.0003 S1C=46 LLI1=0 | C2W=N/A L2W=MISSING D2W=0.1 S2W=40 LLI2=1\n\
             @CLOCKJUMP 1000 source=receiver raw=\"x\"\n",
        );
        let e = &f.epochs[0];
        let g = e.sat("G01").unwrap();
        assert_eq!(g.find("C1C").unwrap().raw, "100.5");
        let c2 = g.find("C2W").unwrap();
        assert!(c2.diagnostic.is_some());
        assert_eq!(c2.raw, "N/A");
        assert!(c2.value_f64.is_none());
        let l2 = g.find("L2W").unwrap();
        assert!(l2.missing && l2.diagnostic.is_some());
        assert_eq!(e.frequencies[0].entries[0], ("L1".to_string(), "1575420000".to_string()));
        assert_eq!(e.clock_events[0].jump_us, Some(1000.0));
        assert_eq!(e.sat("G01").unwrap().lli_flags[0].1, 0);
    }
}
