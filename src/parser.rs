//! Parser for the `.cbx` (carrier-break exchange) sample format.
//!
//! The format is line oriented and deliberately close to a flattened RINEX
//! import so each epoch carries pseudorange, carrier, Doppler, LLI, SNR,
//! frequency number, observation code and clock events verbatim:
//!
//! ```text
//! CBX 1
//! STATION XM01 X  -2400000.000 m ... IGS08
//! FREQUENCY XM01 G L1 1C 1575420000 Hz
//! SATSTATE 0 G01 -24100000.000 m ... IGS08 12.5 ns
//! CLOCK 180 XM01 100.0 ns manual_insert
//! OBS epoch station sat band code frq P(m) L(cyc) D(m/s) LLI SNR(dBHz)
//! ```
//!
//! Invalid tokens do not abort the import: the value stays at its position
//! with `value=None` and a diagnostic carrying the line number is recorded.
//! Nothing is dropped "to make the plot work".

use crate::domain::{
    ClockLine, FrequencyDef, LLI, ObsLine, ParseDiagnostic, Qty, SatStateLine, SourceBundle,
    StationDef, Unit,
};

struct Cursor<'a> {
    tokens: Vec<&'a str>,
    pos: usize,
    line_no: i64,
    raw: &'a str,
    diags: &'a mut Vec<ParseDiagnostic>,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, what: &str) -> Option<&'a str> {
        if self.pos < self.tokens.len() {
            let t = self.tokens[self.pos];
            self.pos += 1;
            Some(t)
        } else {
            self.diag("missing_token", format!("expected {what}"));
            None
        }
    }

    fn rest(&self) -> &[&'a str] {
        &self.tokens[self.pos..]
    }

    fn diag(&mut self, code: &str, message: String) {
        self.diags.push(ParseDiagnostic {
            line_no: self.line_no,
            code: code.into(),
            message,
            raw_line: self.raw.to_string(),
            severity: "error".into(),
        });
    }

    fn warn(&mut self, code: &str, message: String) {
        self.diags.push(ParseDiagnostic {
            line_no: self.line_no,
            code: code.into(),
            message,
            raw_line: self.raw.to_string(),
            severity: "warning".into(),
        });
    }

    /// Consume `value + unit`, keeping the raw token. Unconvertible values
    /// remain in place as `Qty::unconvertible`.
    fn qty(&mut self, unit: Unit, what: &str) -> Qty {
        let Some(raw) = self.take(what) else {
            return Qty::unconvertible("", unit);
        };
        match raw.parse::<f64>() {
            Ok(v) => Qty::parsed(raw, v, unit),
            Err(_) => {
                self.diag("unconvertible_value", format!("{what}: `{raw}` is not numeric"));
                Qty::unconvertible(raw, unit)
            }
        }
    }

    fn optional_qty(&mut self, unit: Unit, what: &str) -> Option<Qty> {
        let raw = *self.rest().first()?;
        if raw == "-" {
            self.pos += 1;
            return None;
        }
        Some(self.qty(unit, what))
    }
}

/// Parse a `.cbx` blob into a [`SourceBundle`].
pub fn parse_cbx(input: &str) -> SourceBundle {
    let mut bundle = SourceBundle {
        kind: "cbx".into(),
        format_version: "0".into(),
        ..Default::default()
    };

    for (idx, raw_line) in input.lines().enumerate() {
        let line_no = (idx + 1) as i64;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let mut cur = Cursor {
            tokens,
            pos: 0,
            line_no,
            raw: raw_line,
            diags: &mut bundle.diagnostics,
        };
        let Some(keyword) = cur.take("keyword") else { continue };
        match keyword {
            "CBX" => {
                if let Some(v) = cur.take("format_version") {
                    bundle.format_version = v.to_string();
                    if v != "1" {
                        cur.warn("unknown_format_version", format!("CBX version {v}, expected 1"));
                    }
                }
            }
            "STATION" => parse_station(&mut cur, &mut bundle),
            "FREQUENCY" => parse_frequency(&mut cur, &mut bundle),
            "SATSTATE" => parse_satstate(&mut cur, &mut bundle),
            "CLOCK" => parse_clock(&mut cur, &mut bundle),
            "OBS" => parse_obs(&mut cur, &mut bundle),
            other => cur.diag("unknown_record", format!("unknown record `{other}`")),
        }
    }

    // Deterministic ordering (the file already is, but normalise for replay).
    bundle.stations.sort_by(|a, b| a.marker.cmp(&b.marker));
    bundle
        .frequencies
        .sort_by(|a, b| (&a.station, &a.system, &a.band).cmp(&(&b.station, &b.system, &b.band)));
    bundle.obs.sort_by_key(|o| (o.epoch, o.station.clone(), o.sat.clone(), o.band.clone()));
    bundle.sat_states.sort_by_key(|s| (s.epoch, s.sat.clone()));
    bundle.clocks.sort_by_key(|c| (c.epoch, c.station.clone()));
    bundle
}

fn parse_station(cur: &mut Cursor, bundle: &mut SourceBundle) {
    let Some(marker) = cur.take("station") else { return };
    let Some(x_lab) = cur.take("x_label") else { return };
    if x_lab != "X" {
        cur.diag("bad_station", format!("expected X, got `{x_lab}`"));
        return;
    }
    let x = cur.qty(Unit::Meter, "x");
    let Some(y_lab) = cur.take("y_label") else { return };
    let y = if y_lab == "Y" {
        cur.qty(Unit::Meter, "y")
    } else {
        cur.diag("bad_station", format!("expected Y, got `{y_lab}`"));
        Qty::unconvertible(y_lab, Unit::Meter)
    };
    let Some(z_lab) = cur.take("z_label") else { return };
    let z = if z_lab == "Z" {
        cur.qty(Unit::Meter, "z")
    } else {
        cur.diag("bad_station", format!("expected Z, got `{z_lab}`"));
        Qty::unconvertible(z_lab, Unit::Meter)
    };
    let datum = cur.take("datum").unwrap_or("UNKNOWN").to_string();
    bundle.stations.push(StationDef {
        marker: marker.to_string(),
        name: None,
        x,
        y,
        z,
        datum,
    });
}

fn parse_frequency(cur: &mut Cursor, bundle: &mut SourceBundle) {
    let Some(station) = cur.take("station") else { return };
    let Some(system) = cur.take("system") else { return };
    let Some(band) = cur.take("band") else { return };
    let Some(code) = cur.take("code") else { return };
    let freq = cur.qty(Unit::Hertz, "frequency");
    bundle.frequencies.push(FrequencyDef {
        station: station.to_string(),
        system: system.to_string(),
        band: band.to_string(),
        code: code.to_string(),
        freq_hz: freq,
    });
}

fn parse_satstate(cur: &mut Cursor, bundle: &mut SourceBundle) {
    let Some(epoch_tok) = cur.take("epoch") else { return };
    let epoch = match epoch_tok.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            cur.diag("bad_epoch", format!("satstate epoch `{epoch_tok}` not integer"));
            return;
        }
    };
    let Some(sat) = cur.take("sat") else { return };
    let Some(x_lab) = cur.take("x_label") else { return };
    let x = if x_lab == "X" {
        cur.qty(Unit::Meter, "x")
    } else {
        cur.diag("bad_satstate", format!("expected X, got `{x_lab}`"));
        Qty::unconvertible(x_lab, Unit::Meter)
    };
    let Some(y_lab) = cur.take("y_label") else { return };
    let y = if y_lab == "Y" {
        cur.qty(Unit::Meter, "y")
    } else {
        cur.diag("bad_satstate", format!("expected Y, got `{y_lab}`"));
        Qty::unconvertible(y_lab, Unit::Meter)
    };
    let Some(z_lab) = cur.take("z_label") else { return };
    let z = if z_lab == "Z" {
        cur.qty(Unit::Meter, "z")
    } else {
        cur.diag("bad_satstate", format!("expected Z, got `{z_lab}`"));
        Qty::unconvertible(z_lab, Unit::Meter)
    };
    let datum = cur.take("datum").unwrap_or("UNKNOWN").to_string();
    let clock_bias = cur.qty(Unit::Nanosecond, "clock_bias");
    bundle.sat_states.push(SatStateLine {
        line_no: cur.line_no,
        epoch,
        sat: sat.to_string(),
        x,
        y,
        z,
        datum,
        clock_bias,
    });
}

fn parse_clock(cur: &mut Cursor, bundle: &mut SourceBundle) {
    let Some(epoch_tok) = cur.take("epoch") else { return };
    let epoch = match epoch_tok.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            cur.diag("bad_epoch", format!("clock epoch `{epoch_tok}` not integer"));
            return;
        }
    };
    let Some(station) = cur.take("station") else { return };
    let jump_ns = cur.qty(Unit::Nanosecond, "jump_ns");
    let cause = cur.take("cause").map(|c| c.to_string());
    bundle.clocks.push(ClockLine {
        line_no: cur.line_no,
        epoch,
        station: station.to_string(),
        jump_ns,
        cause,
    });
}

fn parse_obs(cur: &mut Cursor, bundle: &mut SourceBundle) {
    let Some(epoch_tok) = cur.take("epoch") else { return };
    let epoch = match epoch_tok.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            cur.diag("bad_epoch", format!("obs epoch `{epoch_tok}` not integer"));
            return;
        }
    };
    let Some(station) = cur.take("station") else { return };
    let Some(sat) = cur.take("sat") else { return };
    let Some(band) = cur.take("band") else { return };
    let Some(code) = cur.take("code") else { return };
    let Some(frq_tok) = cur.take("freq_no") else { return };
    let freq_no = if frq_tok == "-" {
        None
    } else {
        match frq_tok.parse::<i64>() {
            Ok(v) => Some(v),
            Err(_) => {
                cur.diag("bad_freq_no", format!("frequency number `{frq_tok}` not integer"));
                None
            }
        }
    };
    let pseudorange = cur.optional_qty(Unit::Meter, "pseudorange");
    let carrier = cur.optional_qty(Unit::Cycle, "carrier");
    let doppler = cur.optional_qty(Unit::MetersPerSecond, "doppler");
    // LLI is an integer bit flag with raw text retained.
    let lli_raw = cur.rest().first().copied();
    let lli = match lli_raw {
        None | Some("-") => {
            if lli_raw.is_some() {
                cur.pos += 1;
            }
            None
        }
        Some(tok) => {
            cur.pos += 1;
            match tok.parse::<i64>() {
                Ok(flag) => Some(LLI { raw: tok.to_string(), flag }),
                Err(_) => {
                    cur.diag("bad_lli", format!("LLI `{tok}` not integer"));
                    Some(LLI { raw: tok.to_string(), flag: -1 })
                }
            }
        }
    };
    let snr = cur.optional_qty(Unit::Unitless, "snr");
    bundle.obs.push(ObsLine {
        line_no: cur.line_no,
        epoch,
        station: station.to_string(),
        sat: sat.to_string(),
        band: band.to_string(),
        code: code.to_string(),
        freq_no,
        pseudorange,
        carrier,
        doppler,
        lli,
        snr,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# header comment
CBX 1
STATION XM01 X -2400000.123 m Y 4600000.456 m Z 3700000.789 m IGS08
FREQUENCY XM01 G L1 1C 1575420000 Hz
SATSTATE 0 G01 X -24100000.000 m Y 14600000.000 m Z 13700000.000 m IGS08 12.5 ns
CLOCK 180 XM01 100.0 ns manual_insert
OBS 0 XM01 G01 L1 1C 1 21000000.000 m 110000000.000 cyc 1200.0 m/s 0 42
OBS 30 XM01 G01 L1 1C 1 - - - 1 -
";

    #[test]
    fn parses_verbatim_epoch_fields() {
        let b = parse_cbx(SAMPLE);
        assert!(b.diagnostics.is_empty(), "{:?}", b.diagnostics);
        assert_eq!(b.format_version, "1");
        assert_eq!(b.stations[0].marker, "XM01");
        assert_eq!(b.stations[0].datum, "IGS08");
        assert_eq!(b.frequencies[0].freq_hz.value, Some(1575420000.0));
        assert_eq!(b.clocks[0].jump_ns.value, Some(100.0));
        assert_eq!(b.obs.len(), 2);
        let o0 = &b.obs[0];
        assert_eq!(o0.carrier.as_ref().unwrap().raw, "110000000.000");
        assert_eq!(o0.lli.as_ref().unwrap().flag, 0);
        assert_eq!(o0.snr.as_ref().unwrap().value, Some(42.0));
        assert_eq!(o0.freq_no, Some(1));
        assert_eq!(o0.code, "1C");
        // Missing values kept as None, LLI false-alarm flag retained.
        let o1 = &b.obs[1];
        assert!(o1.carrier.is_none());
        assert_eq!(o1.lli.as_ref().unwrap().flag, 1);
        assert!(o1.snr.is_none());
    }

    #[test]
    fn invalid_tokens_stay_in_place_with_diagnostics() {
        let bad = "CBX 1\nOBS 0 XM01 G01 L1 1C 1 nope m 110000000.000 cyc 1200 m/s x 42\n";
        let b = parse_cbx(bad);
        assert_eq!(b.obs.len(), 1);
        let pr = b.obs[0].pseudorange.as_ref().unwrap();
        assert_eq!(pr.value, None);
        assert_eq!(pr.raw, "nope");
        assert!(b.diagnostics.iter().any(|d| d.code == "unconvertible_value"));
        assert!(b.diagnostics.iter().any(|d| d.code == "bad_lli"));
        // Carrier and snr are still captured despite earlier bad tokens.
        assert_eq!(b.obs[0].carrier.as_ref().unwrap().value, Some(110000000.0));
        assert_eq!(b.obs[0].snr.as_ref().unwrap().value, Some(42.0));
    }

    #[test]
    fn explicit_format_errors_are_reported() {
        let bad = "CBX 1\nOBS notanint XM01 G01 L1 1C 1\nBOGUS 1 2 3\n";
        let b = parse_cbx(bad);
        assert!(b.diagnostics.iter().any(|d| d.code == "bad_epoch"));
        assert!(b.diagnostics.iter().any(|d| d.code == "unknown_record"));
    }
}
