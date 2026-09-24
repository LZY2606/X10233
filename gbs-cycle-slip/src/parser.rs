//! GBSV1 样例格式解析器（可复制的小型 RINEX 观测子集）。
//!
//! 行语法（`#` 起始为注释，空行忽略）：
//! ```text
//! FORMAT GBSV1
//! REFERENCE 2026-01-01T00:00:00Z
//! OBS station sat freq code epoch P phase doppler LLI SNR
//! SATSTATE sat epoch health ...
//! CLOCK ALL epoch jump_ms
//! ```
//! 缺失值令牌为 `*`；无法转换的数值令牌不致整批失败，而是留 None + WARNING。
//! 结构/语法错误产生 ERROR，整批不予提交。

use crate::domain::*;
use crate::time::epoch_to_offset;

pub fn parse_batch(input: &str) -> ParsedBatch {
    parse_batch_with_reference(input, DEFAULT_REFERENCE)
}

pub fn parse_batch_with_reference(input: &str, declared: &str) -> ParsedBatch {
    let mut batch = ParsedBatch {
        format_version: String::new(),
        reference_epoch: declared.to_string(),
        observations: Vec::new(),
        sat_states: Vec::new(),
        clock_events: Vec::new(),
        diagnostics: Vec::new(),
    };

    let mut ordinal: i64 = 0;
    for (idx, raw) in input.lines().enumerate() {
        let line_no = (idx + 1) as i64;
        let line = raw.trim_end();
        let stripped = line.split('#').next().unwrap_or("").trim();
        if stripped.is_empty() {
            continue;
        }
        let mut tokens = stripped.split_whitespace();
        let head = tokens.next().unwrap();
        let rest: Vec<&str> = tokens.collect();
        match head {
            "FORMAT" => {
                if rest.len() != 1 {
                    err(&mut batch, line_no, "FORMAT_SYNTAX", "FORMAT 行需要恰好一个参数", line);
                } else if rest[0] != FORMAT_VERSION {
                    err(
                        &mut batch,
                        line_no,
                        "UNSUPPORTED_FORMAT",
                        &format!("只支持 {}，收到 {}", FORMAT_VERSION, rest[0]),
                        line,
                    );
                } else {
                    batch.format_version = rest[0].to_string();
                }
            }
            "REFERENCE" => {
                if rest.len() != 1 {
                    err(
                        &mut batch,
                        line_no,
                        "REFERENCE_SYNTAX",
                        "REFERENCE 行需要恰好一个 RFC3339 历元",
                        line,
                    );
                } else if epoch_to_offset(rest[0], DEFAULT_REFERENCE).is_none() {
                    err(
                        &mut batch,
                        line_no,
                        "BAD_REFERENCE_EPOCH",
                        "参考历元必须为 YYYY-MM-DDTHH:MM:SSZ",
                        line,
                    );
                } else {
                    batch.reference_epoch = rest[0].to_string();
                }
            }
            "OBS" => parse_obs(&mut batch, line_no, line, &rest, &mut ordinal),
            "SATSTATE" => parse_satstate(&mut batch, line_no, line, &rest, &mut ordinal),
            "CLOCK" => parse_clock(&mut batch, line_no, line, &rest, &mut ordinal),
            other => err(
                &mut batch,
                line_no,
                "UNKNOWN_RECORD",
                &format!("未知记录类型: {other}"),
                line,
            ),
        }
    }

    if batch.format_version.is_empty() && !batch.diagnostics.iter().any(|d| d.code == "UNSUPPORTED_FORMAT") {
        err(
            &mut batch,
            0,
            "MISSING_FORMAT",
            &format!("首行必须声明 FORMAT {FORMAT_VERSION}"),
            "",
        );
    }
    batch
}

fn parse_obs(
    batch: &mut ParsedBatch,
    line_no: i64,
    line: &str,
    rest: &[&str],
    ordinal: &mut i64,
) {
    // OBS station sat freq code epoch P phase doppler LLI SNR  → 10 个参数。
    if rest.len() != 10 {
        return err(
            batch,
            line_no,
            "OBS_SYNTAX",
            "OBS 需要 10 个字段: station sat freq code epoch P phase doppler LLI SNR",
            line,
        );
    }
    let (station, sat, freq, code, epoch) =
        (rest[0], rest[1], rest[2], rest[3], rest[4]);
    let system = sat.chars().next().unwrap_or('?').to_string();
    match BandDef::lookup(&system, freq, code) {
        None => err(
            batch,
            line_no,
            "UNKNOWN_BAND",
            &format!("未知系统/频率编号/观测码组合: {system} {freq} {code}"),
            line,
        ),
        Some(_band) => {
            if epoch_to_offset(epoch, &batch.reference_epoch).is_none() {
                err(
                    batch,
                    line_no,
                    "BAD_EPOCH",
                    &format!("无法解析历元令牌: {epoch}"),
                    line,
                );
                return;
            }
            *ordinal += 1;
            let p = value_field(batch, line_no, rest[5], "pseudorange", "m");
            let phase = value_field(batch, line_no, rest[6], "phase", "cycles");
            let dop = value_field(batch, line_no, rest[7], "doppler", "Hz");
            let lli = int_field(batch, line_no, rest[8], "LLI");
            let snr = value_field(batch, line_no, rest[9], "snr", "dB-Hz");
            batch.observations.push(RawObservation {
                ordinal: *ordinal,
                station: station.to_string(),
                sat: sat.to_string(),
                system,
                freq_num: freq.to_string(),
                code: code.to_string(),
                epoch_tag: epoch.to_string(),
                epoch_offset_s: epoch_to_offset(epoch, &batch.reference_epoch),
                epoch_datum: EPOCH_DATUM.to_string(),
                reference_epoch: batch.reference_epoch.clone(),
                pseudorange_m: p,
                pseudorange_raw: rest[5].to_string(),
                phase_cycles: phase,
                phase_raw: rest[6].to_string(),
                doppler_hz: dop,
                doppler_raw: rest[7].to_string(),
                lli,
                lli_raw: rest[8].to_string(),
                snr_db_hz: snr,
                snr_raw: rest[9].to_string(),
                raw_line: line.to_string(),
            });
        }
    }
}

fn parse_satstate(
    batch: &mut ParsedBatch,
    line_no: i64,
    line: &str,
    rest: &[&str],
    ordinal: &mut i64,
) {
    // SATSTATE sat epoch health [tokens...]
    if rest.len() < 3 {
        return err(
            batch,
            line_no,
            "SATSTATE_SYNTAX",
            "SATSTATE 至少需要 sat epoch health",
            line,
        );
    }
    let epoch = rest[1];
    let off = epoch_to_offset(epoch, &batch.reference_epoch);
    if off.is_none() {
        return err(
            batch,
            line_no,
            "BAD_EPOCH",
            &format!("无法解析历元令牌: {epoch}"),
            line,
        );
    }
    *ordinal += 1;
    batch.sat_states.push(RawSatState {
        ordinal: *ordinal,
        sat: rest[0].to_string(),
        epoch_tag: epoch.to_string(),
        epoch_offset_s: off,
        epoch_datum: EPOCH_DATUM.to_string(),
        reference_epoch: batch.reference_epoch.clone(),
        health_raw: rest[2].to_string(),
        tokens: rest[3..].iter().map(|s| s.to_string()).collect(),
        raw_line: line.to_string(),
    });
}

fn parse_clock(
    batch: &mut ParsedBatch,
    line_no: i64,
    line: &str,
    rest: &[&str],
    ordinal: &mut i64,
) {
    // CLOCK applies_to epoch jump_ms
    if rest.len() != 3 {
        return err(
            batch,
            line_no,
            "CLOCK_SYNTAX",
            "CLOCK 需要 3 个字段: applies_to epoch jump_ms",
            line,
        );
    }
    let epoch = rest[1];
    let off = epoch_to_offset(epoch, &batch.reference_epoch);
    if off.is_none() {
        return err(
            batch,
            line_no,
            "BAD_EPOCH",
            &format!("无法解析历元令牌: {epoch}"),
            line,
        );
    }
    let jump = value_field(batch, line_no, rest[2], "clock jump", "ms");
    *ordinal += 1;
    batch.clock_events.push(RawClockEvent {
        ordinal: *ordinal,
        applies_to: rest[0].to_string(),
        epoch_tag: epoch.to_string(),
        epoch_offset_s: off,
        epoch_datum: EPOCH_DATUM.to_string(),
        reference_epoch: batch.reference_epoch.clone(),
        jump_ms_raw: rest[2].to_string(),
        jump_ms: jump,
        raw_line: line.to_string(),
    });
}

fn value_field(
    batch: &mut ParsedBatch,
    line_no: i64,
    token: &str,
    field: &str,
    unit: &str,
) -> Option<f64> {
    if token == "*" {
        return None;
    }
    match token.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => {
            warn(
                batch,
                line_no,
                "UNCONVERTIBLE_VALUE",
                &format!("{field} 值无法转换为 {unit}，原值保留在原位置: {token}"),
            );
            None
        }
    }
}

fn int_field(batch: &mut ParsedBatch, line_no: i64, token: &str, field: &str) -> Option<i64> {
    if token == "*" {
        return None;
    }
    match token.parse::<i64>() {
        Ok(v) => Some(v),
        Err(_) => {
            warn(
                batch,
                line_no,
                "UNCONVERTIBLE_VALUE",
                &format!("{field} 值无法转换为整数，原值保留在原位置: {token}"),
            );
            None
        }
    }
}

fn err(batch: &mut ParsedBatch, line_no: i64, code: &str, message: &str, line: &str) {
    batch.diagnostics.push(ParseDiagnostic {
        line_no,
        severity: Severity::Error,
        code: code.to_string(),
        message: message.to_string(),
        raw_line: line.to_string(),
    });
}

fn warn(batch: &mut ParsedBatch, line_no: i64, code: &str, message: &str, line: &str = "") {
    batch.diagnostics.push(ParseDiagnostic {
        line_no,
        severity: Severity::Warning,
        code: code.to_string(),
        message: message.to_string(),
        raw_line: line.to_string(),
    });
}
