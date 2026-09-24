use crate::model::parse_epoch;

/// One raw observation row, exactly as submitted. Raw text fields are always
/// preserved; normalized numeric columns are filled only when conversion
/// succeeds, otherwise the value stays in place with a diagnostic.
#[derive(Clone, Debug)]
pub struct ParsedObs {
    pub station: String,
    pub sat: String,
    pub epoch_ms: i64,
    pub freq: i64,
    pub code: String,
    pub p_raw: String,
    pub l_raw: String,
    pub d_raw: String,
    pub lli_raw: String,
    pub s_raw: String,
    pub p: Option<f64>,
    pub l: Option<f64>,
    pub d: Option<f64>,
    pub lli: Option<i64>,
    pub s: Option<f64>,
    pub diag: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ParsedClk {
    pub station: String,
    pub epoch_ms: i64,
    pub offset_raw: String,
    pub offset_s: Option<f64>,
    pub diag: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedInput {
    pub obs: Vec<ParsedObs>,
    pub clks: Vec<ParsedClk>,
}

fn parse_f64(raw: &str, what: &str, diags: &mut Vec<String>) -> Option<f64> {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => {
            diags.push(format!(
                "unconvertible {what} value `{raw}` kept raw at origin (unit unknown)"
            ));
            None
        }
    }
}

fn parse_i64(raw: &str, what: &str, diags: &mut Vec<String>) -> Option<i64> {
    match raw.parse::<i64>() {
        Ok(v) => Some(v),
        _ => {
            diags.push(format!("unconvertible {what} value `{raw}` kept raw at origin"));
            None
        }
    }
}

/// Parse the mini-RINEX text format:
///
/// ```text
/// MINIRINEX 1
/// # comment
/// OBS <station> <sat> <epoch> F<freq> <code> P=<m> L=<cycles> D=<Hz> LLI=<flags> S=<dB-Hz>
/// CLKJUMP <station> <epoch> <offset_s>
/// ```
///
/// Units: P in meters, L in carrier cycles, D in Hz, S in dB-Hz, epochs UTC.
/// Format errors are rejected with a line number; value-level conversion
/// failures are kept raw with diagnostics instead of failing the import.
pub fn parse_input(raw: &str) -> Result<ParsedInput, String> {
    let mut out = ParsedInput::default();
    let mut saw_header = false;
    for (idx, line) in raw.lines().enumerate() {
        let lineno = idx + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !saw_header {
            if line == "MINIRINEX 1" {
                saw_header = true;
                continue;
            }
            return Err(format!(
                "line {lineno}: expected header `MINIRINEX 1`, found `{line}`"
            ));
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks.first().copied() {
            Some("OBS") => {
                if toks.len() != 11 {
                    return Err(format!(
                        "line {lineno}: OBS expects 11 fields `OBS <station> <sat> <epoch> F<freq> <code> P=.. L=.. D=.. LLI=.. S=..`, got {}",
                        toks.len()
                    ));
                }
                let station = toks[1].to_string();
                let sat = toks[2].to_string();
                let epoch_ms = parse_epoch(toks[3])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let freq = toks[4]
                    .strip_prefix('F')
                    .and_then(|f| f.parse::<i64>().ok())
                    .ok_or_else(|| {
                        format!("line {lineno}: bad frequency designator `{}`", toks[4])
                    })?;
                let code = toks[5].to_string();
                let mut kv = std::collections::HashMap::new();
                for t in &toks[6..] {
                    let (k, v) = t.split_once('=').ok_or_else(|| {
                        format!("line {lineno}: bad field `{t}`, expected KEY=VALUE")
                    })?;
                    if kv.insert(k.to_string(), v.to_string()).is_some() {
                        return Err(format!("line {lineno}: duplicate key `{k}`"));
                    }
                }
                let take = |k: &str| -> Result<String, String> {
                    kv.remove(k).ok_or_else(|| format!("line {lineno}: missing key `{k}=`"))
                };
                let p_raw = take("P")?;
                let l_raw = take("L")?;
                let d_raw = take("D")?;
                let lli_raw = take("LLI")?;
                let s_raw = take("S")?;
                if let Some(extra) = kv.keys().next() {
                    return Err(format!("line {lineno}: unknown key `{extra}`"));
                }
                let mut diags = Vec::new();
                let p = parse_f64(&p_raw, "pseudorange P (m)", &mut diags);
                let l = parse_f64(&l_raw, "carrier L (cycles)", &mut diags);
                let d = parse_f64(&d_raw, "doppler D (Hz)", &mut diags);
                let lli = parse_i64(&lli_raw, "LLI", &mut diags);
                let s = parse_f64(&s_raw, "SNR S (dB-Hz)", &mut diags);
                out.obs.push(ParsedObs {
                    station,
                    sat,
                    epoch_ms,
                    freq,
                    code,
                    p_raw,
                    l_raw,
                    d_raw,
                    lli_raw,
                    s_raw,
                    p,
                    l,
                    d,
                    lli,
                    s,
                    diag: if diags.is_empty() { None } else { Some(diags.join("; ")) },
                });
            }
            Some("CLKJUMP") => {
                if toks.len() != 4 {
                    return Err(format!(
                        "line {lineno}: CLKJUMP expects `CLKJUMP <station> <epoch> <offset_s>`"
                    ));
                }
                let epoch_ms = parse_epoch(toks[2])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let mut diags = Vec::new();
                let offset_s = parse_f64(toks[3], "clock jump offset (s)", &mut diags);
                out.clks.push(ParsedClk {
                    station: toks[1].to_string(),
                    epoch_ms,
                    offset_raw: toks[3].to_string(),
                    offset_s,
                    diag: if diags.is_empty() { None } else { Some(diags.join("; ")) },
                });
            }
            Some(other) => {
                return Err(format!("line {lineno}: unknown record kind `{other}`"));
            }
            None => {}
        }
    }
    if !saw_header {
        return Err("empty input: missing header `MINIRINEX 1`".to_string());
    }
    if out.obs.is_empty() && out.clks.is_empty() {
        return Err("input contains no OBS or CLKJUMP records".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_obs_and_keeps_raw() {
        let raw = "MINIRINEX 1\nOBS BPTA G05 2026-09-19T00:00:00Z F1 C1C P=20200000.000 L=100000000.000 D=-2000.0 LLI=0 S=45.0\n";
        let p = parse_input(raw).unwrap();
        assert_eq!(p.obs.len(), 1);
        assert_eq!(p.obs[0].l_raw, "100000000.000");
        assert_eq!(p.obs[0].l, Some(100000000.0));
        assert!(p.obs[0].diag.is_none());
    }

    #[test]
    fn unconvertible_values_stay_with_diag() {
        let raw = "MINIRINEX 1\nOBS BPTA G06 2026-09-19T00:00:02Z F1 C1C P=20200100.000 L=--- D=-2000.0 LLI=0 S=45.0\n";
        let p = parse_input(raw).unwrap();
        assert_eq!(p.obs[0].l, None);
        assert_eq!(p.obs[0].l_raw, "---");
        assert!(p.obs[0].diag.as_ref().unwrap().contains("unconvertible carrier"));
    }

    #[test]
    fn format_errors_have_line_numbers() {
        assert!(parse_input("OBS X").unwrap_err().contains("line 1"));
        let bad = "MINIRINEX 1\nOBS BPTA G05 not-a-time F1 C1C P=1 L=2 D=3 LLI=0 S=4\n";
        assert!(parse_input(bad).unwrap_err().contains("line 2"));
        let unknown = "MINIRINEX 1\nWAT BPTA\n";
        assert!(parse_input(unknown).unwrap_err().contains("unknown record kind"));
    }
}
