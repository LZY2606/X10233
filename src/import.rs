//! Import layer: accepts copyable small samples, reports explicit format
//! errors, and stores pseudorange / carrier / Doppler / LLI / SNR /
//! clock-jump events / frequency numbers / observation codes exactly as
//! submitted, per epoch. Raw submissions are immutable; revisions are new
//! versions linked by parent_id. Only derived results that depend on
//! changed source rows are marked stale.
use crate::db::Store;
use crate::model::{content_id, parse_epoch_secs, Diagnostic};
use rusqlite::params;
use serde_json::json;

#[derive(Debug)]
pub struct ParsedObs {
    pub station: String,
    pub sat: String,
    pub epoch_label: String,
    pub epoch_secs: Option<f64>,
    pub freq_number: i64,
    pub obs_code: String,
    pub pr_raw: String,
    pub pr_m: Option<f64>,
    pub cp_raw: String,
    pub cp_cycles: Option<f64>,
    pub dp_raw: String,
    pub dp_hz: Option<f64>,
    pub lli_raw: String,
    pub lli: Option<i64>,
    pub snr_raw: String,
    pub snr: Option<f64>,
    pub clock_jump_raw: Option<String>,
    pub clock_jump_s: Option<f64>,
    pub diag: Vec<Diagnostic>,
}

pub struct ParsedImport {
    pub station: String,
    pub obs: Vec<ParsedObs>,
    pub warnings: Vec<Diagnostic>,
}

fn parse_f64(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Parse the copyable sample format. Structural problems (bad header,
/// unknown directive, wrong field count) reject the whole import with
/// explicit diagnostics; unconvertible *values* stay in place with a
/// per-row diagnostic and never abort the import.
pub fn parse_obs_text(text: &str) -> Result<ParsedImport, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut station: Option<String> = None;
    let mut epoch_label: Option<String> = None;
    let mut epoch_secs: Option<f64> = None;
    let mut clock_jump_raw: Option<String> = None;
    let mut clock_jump_s: Option<f64> = None;
    let mut obs = Vec::new();
    let mut saw_header = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') && !line.starts_with("# PAIRWISE-GSB") {
            continue;
        }
        if !saw_header {
            if line == "# PAIRWISE-GSB OBS 1" {
                saw_header = true;
                continue;
            }
            errors.push(
                Diagnostic::new("bad_header", "first record must be '# PAIRWISE-GSB OBS 1'")
                    .at_line(line_no),
            );
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        match toks.first().copied() {
            Some("STATION") => {
                if toks.len() != 2 {
                    errors.push(Diagnostic::new("arity", "STATION takes 1 argument").at_line(line_no));
                } else {
                    station = Some(toks[1].to_string());
                }
            }
            Some("EPOCH") => {
                if toks.len() != 2 {
                    errors.push(Diagnostic::new("arity", "EPOCH takes 1 argument").at_line(line_no));
                } else {
                    epoch_label = Some(toks[1].to_string());
                    epoch_secs = parse_epoch_secs(toks[1]);
                    if epoch_secs.is_none() {
                        warnings.push(
                            Diagnostic::new(
                                "epoch_unconvertible",
                                format!("epoch label '{}' kept raw; cannot derive seconds", toks[1]),
                            )
                            .at_line(line_no),
                        );
                    }
                    clock_jump_raw = None;
                    clock_jump_s = None;
                }
            }
            Some("CLOCKJUMP") => {
                if toks.len() != 2 {
                    errors.push(Diagnostic::new("arity", "CLOCKJUMP takes 1 argument (seconds)").at_line(line_no));
                } else if epoch_label.is_none() {
                    errors.push(Diagnostic::new("order", "CLOCKJUMP before any EPOCH").at_line(line_no));
                } else {
                    clock_jump_raw = Some(toks[1].to_string());
                    clock_jump_s = parse_f64(toks[1]);
                    if clock_jump_s.is_none() {
                        warnings.push(
                            Diagnostic::new(
                                "value_unconvertible",
                                format!("clock jump '{}' kept raw; expected seconds", toks[1]),
                            )
                            .at_line(line_no),
                        );
                    }
                }
            }
            Some("OBS") => {
                if toks.len() != 9 {
                    errors.push(
                        Diagnostic::new(
                            "arity",
                            format!("OBS takes 8 fields (sat freq obs_code pr cp dp lli snr), got {}", toks.len() - 1),
                        )
                        .at_line(line_no),
                    );
                    continue;
                }
                let (Some(st), Some(ep)) = (station.clone(), epoch_label.clone()) else {
                    errors.push(Diagnostic::new("order", "OBS before STATION/EPOCH").at_line(line_no));
                    continue;
                };
                let mut diag = Vec::new();
                let freq_number: i64 = match toks[2].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        errors.push(
                            Diagnostic::new("format", format!("frequency number '{}' is not an integer", toks[2]))
                                .at_line(line_no),
                        );
                        continue;
                    }
                };
                let pr_m = parse_f64(toks[4]);
                if pr_m.is_none() {
                    diag.push(Diagnostic::new("value_unconvertible", format!("pseudorange '{}' kept raw; expected meters", toks[4])).at_line(line_no));
                }
                let cp_cycles = parse_f64(toks[5]);
                if cp_cycles.is_none() {
                    diag.push(Diagnostic::new("value_unconvertible", format!("carrier '{}' kept raw; expected cycles", toks[5])).at_line(line_no));
                }
                let dp_hz = parse_f64(toks[6]);
                if dp_hz.is_none() {
                    diag.push(Diagnostic::new("value_unconvertible", format!("doppler '{}' kept raw; expected Hz", toks[6])).at_line(line_no));
                }
                let lli: Option<i64> = toks[7].parse().ok();
                if lli.is_none() {
                    diag.push(Diagnostic::new("value_unconvertible", format!("LLI '{}' kept raw; expected integer flag", toks[7])).at_line(line_no));
                }
                let snr = parse_f64(toks[8]);
                if snr.is_none() {
                    diag.push(Diagnostic::new("value_unconvertible", format!("SNR '{}' kept raw; expected dB-Hz", toks[8])).at_line(line_no));
                }
                if crate::model::freq_hz(freq_number).is_none() {
                    diag.push(Diagnostic::new(
                        "unknown_frequency",
                        format!("frequency number {} has no known wavelength; normalization skipped", freq_number),
                    ).at_line(line_no));
                }
                obs.push(ParsedObs {
                    station: st,
                    sat: toks[1].to_string(),
                    epoch_label: ep,
                    epoch_secs,
                    freq_number,
                    obs_code: toks[3].to_string(),
                    pr_raw: toks[4].to_string(),
                    pr_m,
                    cp_raw: toks[5].to_string(),
                    cp_cycles,
                    dp_raw: toks[6].to_string(),
                    dp_hz,
                    lli_raw: toks[7].to_string(),
                    lli,
                    snr_raw: toks[8].to_string(),
                    snr,
                    clock_jump_raw: clock_jump_raw.clone(),
                    clock_jump_s,
                    diag,
                });
            }
            Some("END") => break,
            Some(other) => errors.push(
                Diagnostic::new("unknown_directive", format!("unknown directive '{}'", other)).at_line(line_no),
            ),
            None => {}
        }
    }
    if !saw_header {
        errors.push(Diagnostic::new("bad_header", "missing '# PAIRWISE-GSB OBS 1' header"));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let station = station.ok_or_else(|| vec![Diagnostic::new("missing", "no STATION record")])?;
    if obs.is_empty() {
        return Err(vec![Diagnostic::new("missing", "no OBS records")]);
    }
    Ok(ParsedImport { station, obs, warnings })
}

pub struct ImportOutcome {
    pub import_id: String,
    pub n_observations: usize,
    pub warnings: Vec<Diagnostic>,
    pub staled_groups: Vec<String>,
}

fn obs_identity(import_id: &str, o: &ParsedObs) -> String {
    content_id(
        "obs",
        &format!(
            "{}|{}|{}|{}|{}|{}",
            import_id, o.station, o.sat, o.epoch_label, o.freq_number, o.obs_code
        ),
    )
}

/// Fingerprint of one (station, sat, freq) group's values, independent of
/// the import version, so unchanged groups survive a revision untouched.
fn group_fingerprint(rows: &[&ParsedObs]) -> String {
    let mut parts: Vec<String> = rows
        .iter()
        .map(|o| {
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                o.epoch_label, o.obs_code, o.pr_raw, o.cp_raw, o.dp_raw, o.lli_raw, o.snr_raw,
                o.clock_jump_raw.clone().unwrap_or_default(),
                o.epoch_secs.map(|v| v.to_string()).unwrap_or_default()
            )
        })
        .collect();
    parts.sort();
    content_id("grp", &parts.join("\n"))
}

/// Commit a parsed import as an immutable new version, then mark stale
/// exactly those derived results whose source groups changed.
pub fn commit_import(
    store: &mut Store,
    label: &str,
    text: &str,
    parent_id: Option<&str>,
) -> Result<ImportOutcome, Vec<Diagnostic>> {
    let parsed = parse_obs_text(text)?;
    let import_id = content_id("imp", &format!("{}|{}", label, text));
    let seq = store.next_seq()?;
    let warnings_json = serde_json::to_string(&parsed.warnings).unwrap();
    store.conn.execute(
        "INSERT INTO imports(id, parent_id, label, raw_text, seq, diag) VALUES(?1,?2,?3,?4,?5,?6)",
        params![import_id, parent_id, label, text, seq, warnings_json],
    )?;
    let tx = store.conn.transaction()?;
    let mut obs_ids_by_group: std::collections::BTreeMap<(String, String, i64), Vec<String>> =
        Default::default();
    let mut group_rows: std::collections::BTreeMap<(String, String, i64), Vec<&ParsedObs>> =
        Default::default();
    for o in &parsed.obs {
        let id = obs_identity(&import_id, o);
        let diag_json = serde_json::to_string(&o.diag).unwrap();
        tx.execute(
            "INSERT INTO observations(id, import_id, station, sat, epoch_label, epoch_secs,
             freq_number, obs_code, pr_raw, pr_m, cp_raw, cp_cycles, dp_raw, dp_hz,
             lli_raw, lli, snr_raw, snr, clock_jump_raw, clock_jump_s, diag)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
            params![
                id, import_id, o.station, o.sat, o.epoch_label, o.epoch_secs,
                o.freq_number, o.obs_code, o.pr_raw, o.pr_m, o.cp_raw, o.cp_cycles,
                o.dp_raw, o.dp_hz, o.lli_raw, o.lli, o.snr_raw, o.snr,
                o.clock_jump_raw, o.clock_jump_s, diag_json
            ],
        )?;
        let key = (o.station.clone(), o.sat.clone(), o.freq_number);
        obs_ids_by_group.entry(key.clone()).or_default().push(id);
        group_rows.entry(key).or_default().push(o);
    }
    tx.commit()?;

    // Local staleness: compare each group's value fingerprint with the
    // fingerprint recorded for the currently-valid arcs of that group.
    let mut staled_groups = Vec::new();
    let mut groups_to_compute = Vec::new();
    for (key, rows) in &group_rows {
        let fp = group_fingerprint(rows);
        let existing: Option<String> = store.conn.query_row(
            "SELECT param_fp FROM arcs WHERE station=?1 AND sat=?2 AND freq_number=?3 AND stale=0 LIMIT 1",
            params![key.0, key.1, key.2],
            |r| r.get(0),
        ).ok();
        // arcs store the *parameter* fingerprint; the group value fingerprint
        // is kept in the diag channel of the first arc of the group.
        let existing_fp: Option<String> = store.conn.query_row(
            "SELECT json_extract(diag,'$[0].group_fp') FROM arcs
             WHERE station=?1 AND sat=?2 AND freq_number=?3 AND stale=0 LIMIT 1",
            params![key.0, key.1, key.2],
            |r| r.get(0),
        ).ok().flatten();
        let _ = existing;
        match existing_fp {
            Some(old) if old == fp => { /* unchanged: derived results stay valid */ }
            _ => {
                if existing_fp.is_some() {
                    staled_groups.push(format!("{}/{}/F{}", key.0, key.1, key.2));
                }
                groups_to_compute.push((key.clone(), fp));
            }
        }
    }
    // Groups that disappeared entirely in this revision also go stale.
    if parent_id.is_some() {
        let mut stmt = store.conn.prepare(
            "SELECT DISTINCT station, sat, freq_number FROM arcs WHERE stale=0",
        )?;
        let live: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        for g in live {
            if !group_rows.contains_key(&g) {
                staled_groups.push(format!("{}/{}/F{}", g.0, g.1, g.2));
                mark_group_stale(store, &g.0, &g.1, g.2)?;
            }
        }
    }
    for (key, _fp) in &groups_to_compute {
        mark_group_stale(store, &key.0, &key.1, key.2)?;
    }

    store.emit(
        "import_committed",
        &json!({
            "import_id": import_id,
            "parent_id": parent_id,
            "label": label,
            "observations": parsed.obs.len(),
            "staled_groups": staled_groups,
        }),
    )?;
    Ok(ImportOutcome {
        import_id,
        n_observations: parsed.obs.len(),
        warnings: parsed.warnings,
        staled_groups,
    })
}

/// Mark every derived row of one (station, sat, freq) group stale. Stale
/// rows remain readable but can no longer pass as current.
pub fn mark_group_stale(
    store: &Store,
    station: &str,
    sat: &str,
    freq_number: i64,
) -> rusqlite::Result<()> {
    store.conn.execute(
        "UPDATE arcs SET stale=1 WHERE station=?1 AND sat=?2 AND freq_number=?3 AND stale=0",
        params![station, sat, freq_number],
    )?;
    store.conn.execute(
        "UPDATE candidates SET stale=1 WHERE stale=0 AND arc_id IN
         (SELECT id FROM arcs WHERE station=?1 AND sat=?2 AND freq_number=?3)",
        params![station, sat, freq_number],
    )?;
    store.conn.execute(
        "UPDATE repairs SET stale=1 WHERE stale=0 AND arc_id IN
         (SELECT id FROM arcs WHERE station=?1 AND sat=?2 AND freq_number=?3)",
        params![station, sat, freq_number],
    )?;
    // Clock events are station-level: stale them only if any group of the
    // station changed; the detector re-derives fresh ones.
    store.conn.execute(
        "UPDATE clock_events SET stale=1 WHERE stale=0 AND station=?1",
        params![station],
    )?;
    Ok(())
}

/// Groups whose derived results must be (re)computed for the latest import.
pub fn pending_groups(store: &Store) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = store.conn.prepare(
        "SELECT DISTINCT o.station, o.sat, o.freq_number
         FROM observations o
         JOIN imports i ON i.id = o.import_id
         WHERE i.seq = (SELECT MAX(seq) FROM imports)
           AND NOT EXISTS (
             SELECT 1 FROM arcs a
             WHERE a.station=o.station AND a.sat=o.sat
               AND a.freq_number=o.freq_number AND a.stale=0)",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}
