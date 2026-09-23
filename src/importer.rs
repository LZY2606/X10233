//! Parser for the offline, RINEX-like small case format.
//!
//! Design rules:
//! * pseudorange, carrier, Doppler, LLI, SNR, clock events, frequency numbers
//!   and observation codes are all stored as imported, epoch by epoch;
//! * a malformed file is rejected transactionally by the store, but the parser
//!   itself reports precise `line: ...` errors;
//! * a value that is syntactically present but cannot be converted stays in
//!   place as an invalid [`CellV`] with a diagnostic (it is not dropped to make
//!   plotting easier).

use crate::domain::*;
use crate::util::ymdhms_to_gps_seconds;
use std::collections::BTreeMap;

pub fn parse_case(text: &str) -> Result<CaseBundle, String> {
    let mut case_id = String::new();
    let mut title = String::new();
    let mut station: Option<Station> = None;
    let mut signals: BTreeMap<Band, Signal> = BTreeMap::new();
    let mut params = Params::default();
    let mut epochs: Vec<EpochRecord> = Vec::new();

    let mut cur: Option<EpochRecord> = None;
    let mut seen_epochs: BTreeMap<String, ()> = BTreeMap::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let tag = parts.next().ok_or_else(|| format!("line {lineno}: empty"))?;
        let rest: Vec<&str> = parts.collect();

        let err = |msg: String| -> String { format!("line {lineno}: {msg}") };

        match tag {
            "CASE" => {
                require(&rest, 2, lineno)?;
                case_id = rest[0].to_string();
                title = rest[1..].join(" ");
            }
            "STN" => {
                require(&rest, 6, lineno)?;
                station = Some(Station {
                    code: rest[0].to_string(),
                    name: rest[1].to_string(),
                    marker: rest[2].to_string(),
                    x_m: fnum(rest[3]).map_err(err)?,
                    y_m: fnum(rest[4]).map_err(err)?,
                    z_m: fnum(rest[5]).map_err(err)?,
                    coord_datum: "WGS84/ECEF-m".to_string(),
                });
            }
            "SIG" => {
                require(&rest, 8, lineno)?;
                let band = inum(rest[0]).map_err(err)?;
                let freq_number = rest[1].to_string();
                let f = fnum(rest[2]).map_err(err)?;
                let lam = fnum(rest[3]).map_err(err)?;
                if band < 1 {
                    return Err(err("band must be >= 1".into()));
                }
                if signals
                    .insert(
                        band,
                        Signal {
                            band,
                            freq_number,
                            code_p: rest[4].to_string(),
                            code_l: rest[5].to_string(),
                            code_d: rest[6].to_string(),
                            code_s: rest[7].to_string(),
                            frequency_hz: f,
                            wavelength_m: lam,
                        },
                    )
                    .is_some()
                {
                    return Err(err(format!("duplicate signal band {band}")));
                }
            }
            "PAR" => {
                for kv in &rest {
                    let (k, v) = kv.split_once('=').ok_or_else(|| {
                        err(format!("parameter '{kv}' must be key=value"))
                    })?;
                    let v = fnum(v).map_err(err)?;
                    match k {
                        "gf_cycle_threshold" => params.gf_cycle_threshold = v,
                        "gf_sigma_m" => params.gf_sigma_m = v,
                        "mw_sigma_cycle" => params.mw_sigma_cycle = v,
                        "td_sigma_cycle" => params.td_sigma_cycle = v,
                        "search_radius_cycle" => params.search_radius_cycle = v as i64,
                        "gap_factor" => params.gap_factor = v,
                        "lli_flag" => params.lli_flag = v as i64,
                        other => return Err(err(format!("unknown parameter '{other}'"))),
                    }
                }
            }
            "EPOCH" => {
                if let Some(e) = cur.take() {
                    epochs.push(e);
                }
                require(&rest, 8, lineno)?;
                let eid = rest[0].to_string();
                let datum = rest[1].to_string();
                let gps_sec = fnum(rest[2]).map_err(err)?;
                let ymd = rest[3].to_string();
                let hms = rest[4].to_string();
                let _week = inum(rest[5]).map_err(err)?;
                let _dow = fnum(rest[6]).map_err(err)?;
                let _flag = rest[7];
                if seen_epochs.insert(eid.clone(), ()).is_some() {
                    return Err(err(format!("duplicate epoch id {eid}")));
                }
                let (y, mo, d) = parse_ymd(&ymd).map_err(err)?;
                let (h, mi, s) = parse_hms(&hms).map_err(err)?;
                let gps = if datum == "GPS" {
                    ymdhms_to_gps_seconds(y, mo, d, h, mi, s)
                } else {
                    gps_sec
                };
                cur = Some(EpochRecord {
                    order: epochs.len() as i64 + 1,
                    id: eid,
                    time_datum: datum,
                    gps_seconds: gps,
                    ymdhms: format!("{ymd} {hms}"),
                    receiver_clock_offset_s: CellV::numeric("0", 0.0, "s"),
                    clock_event: None,
                    sats: BTreeMap::new(),
                    sat_states: BTreeMap::new(),
                });
            }
            "CLK" => {
                let e = cur.as_mut().ok_or_else(|| err("CLK before EPOCH".into()))?;
                require(&rest, 1, lineno)?;
                e.receiver_clock_offset_s = parse_cell(rest[0], "s", "receiver clock offset");
                let mut jump = None;
                for kv in &rest[1..] {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| err(format!("clock option '{kv}' must be key=value")))?;
                    match k {
                        "jump_s" => jump = Some(fnum(v).map_err(err)?),
                        other => return Err(err(format!("unknown clock option '{other}'"))),
                    }
                }
                if let Some(mag) = jump {
                    e.clock_event = Some(ClockEvent {
                        kind: "clock_jump".to_string(),
                        magnitude_s: Some(mag),
                        note: "whole-machine receiver clock jump".to_string(),
                    });
                }
            }
            "SAT" => {
                let e = cur.as_mut().ok_or_else(|| err("SAT before EPOCH".into()))?;
                require(&rest, 6, lineno)?;
                let sat = rest[0].to_string();
                if e.sat_states
                    .insert(
                        sat.clone(),
                        SatState {
                            sat,
                            x_m: fnum(rest[1]).map_err(err)?,
                            y_m: fnum(rest[2]).map_err(err)?,
                            z_m: fnum(rest[3]).map_err(err)?,
                            coord_datum: "WGS84/ECEF-m".to_string(),
                            sat_clock_bias_s: fnum(rest[4]).ok(),
                            health: rest[5..].join(" "),
                        },
                    )
                    .is_some()
                {
                    return Err(err(format!("duplicate SAT for {sat}")));
                }
            }
            "OBS" => {
                let e = cur.as_mut().ok_or_else(|| err("OBS before EPOCH".into()))?;
                require(&rest, 6, lineno)?;
                let sat = rest[0].to_string();
                let band = inum(rest[1]).map_err(err)?;
                if !signals.contains_key(&band) {
                    return Err(err(format!("OBS references undeclared band {band}")));
                }
                let lli = inum(rest[2]).map_err(err)?;
                if lli < 0 {
                    return Err(err("LLI flag must be >= 0"));
                }
                let so = e.sats.entry(sat.clone()).or_insert_with(|| SatObs {
                    sat: sat.clone(),
                    lli: BTreeMap::new(),
                    obs: BTreeMap::new(),
                });
                if so.obs.contains_key(&band) {
                    return Err(err(format!("duplicate OBS {sat} band {band}")));
                }
                so.lli.insert(band, lli);
                let mut cells = BTreeMap::new();
                let fields = [
                    (ObsField::Pseudorange, rest[3]),
                    (ObsField::Carrier, rest[4]),
                    (ObsField::Doppler, rest[5]),
                ];
                for (f, tok) in fields {
                    cells.insert(f, parse_cell(tok, f.unit(), f.key()));
                }
                let snr_tok = rest.get(6).copied().unwrap_or("X");
                cells.insert(
                    ObsField::Snr,
                    parse_cell(snr_tok, ObsField::Snr.unit(), ObsField::Snr.key()),
                );
                so.obs.insert(band, cells);
            }
            other => return Err(err(format!("unknown record tag '{other}'"))),
        }
    }
    if let Some(e) = cur {
        epochs.push(e);
    }

    let station = station.ok_or_else(|| "missing STN record".to_string())?;
    if case_id.is_empty() {
        return Err("missing CASE record".into());
    }
    if signals.is_empty() {
        return Err("at least one SIG record is required".into());
    }
    if epochs.is_empty() {
        return Err("at least one EPOCH is required".into());
    }
    epochs.sort_by_key(|e| e.order);

    Ok(CaseBundle {
        case_id,
        title,
        station,
        signals,
        params,
        epochs,
    })
}

fn require(rest: &[&str], n: usize, lineno: usize) -> Result<(), String> {
    if rest.len() < n {
        return Err(format!(
            "line {lineno}: expected at least {n} fields, got {}",
            rest.len()
        ));
    }
    Ok(())
}

fn fnum(t: &str) -> Result<f64, String> {
    t.parse::<f64>()
        .map_err(|_| format!("'{t}' is not a finite number"))
        .and_then(|v| {
            if v.is_finite() {
                Ok(v)
            } else {
                Err(format!("'{t}' is not finite"))
            }
        })
}

fn inum(t: &str) -> Result<i64, String> {
    t.parse::<i64>().map_err(|_| format!("'{t}' is not an integer"))
}

fn parse_ymd(s: &str) -> Result<(i64, u32, u32), String> {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() != 3 {
        return Err(format!("date '{s}' must be YYYY-MM-DD"));
    }
    Ok((
        inum(p[0])?,
        inum(p[1])? as u32,
        inum(p[2])? as u32,
    ))
}

fn parse_hms(s: &str) -> Result<(u32, u32, u32), String> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() != 3 {
        return Err(format!("time '{s}' must be HH:MM:SS"));
    }
    Ok((inum(p[0])? as u32, inum(p[1])? as u32, inum(p[2])? as u32))
}

/// Parse a measurement token. `X` (case-insensitive) means "present but not
/// convertible" and is retained with a diagnostic; a finite number converts.
fn parse_cell(token: &str, unit: &str, what: &str) -> CellV {
    match token.parse::<f64>() {
        Ok(v) if v.is_finite() => CellV::numeric(token, v, unit),
        _ => CellV::invalid(
            token,
            unit,
            format!("{what}: token '{token}' kept as imported, no numeric conversion"),
        ),
    }
}
