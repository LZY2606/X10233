//! RINEX 3 观测文件子集解析 + 钟跳事件 / 卫星状态附属文件。
//! 原则:伪距、载波、Doppler、LLI、信噪比、钟跳事件、频率编号和观测码均按历元原样保存;
//! 解析失败只产生诊断,原始行不丢。
use crate::model::*;

#[derive(Debug, Default)]
pub struct RinexParse {
    pub station: String,
    pub obs_types: Vec<(char, Vec<String>)>, // sys -> codes
    pub obs: Vec<NormObs>,
    pub diagnostics: Vec<Diagnostic>,
}

fn parse_epoch_line(line: &str) -> Option<(i64, u8, usize)> {
    // "> 2024 03 01 00 00  0.0000000  0 12"
    let f: Vec<&str> = line[1..].split_whitespace().collect();
    if f.len() < 7 {
        return None;
    }
    let y: i64 = f[0].parse().ok()?;
    let mo: i64 = f[1].parse().ok()?;
    let d: i64 = f[2].parse().ok()?;
    let h: i64 = f[3].parse().ok()?;
    let mi: i64 = f[4].parse().ok()?;
    let sec: f64 = f[5].parse().ok()?;
    let s = sec.trunc() as i64;
    let ms = ((sec - s as f64) * 1000.0).round() as i64;
    let flag: u8 = f[6].parse().ok()?;
    let nsat: usize = if f.len() > 7 { f[7].parse().unwrap_or(0) } else { 0 };
    Some((ms_of_iso(y, mo, d, h, mi, s, ms), flag, nsat))
}

/// 解析 RINEX 3 观测文本。header 需要 `SYS / # / OBS TYPES` 与 `MARKER NAME`。
pub fn parse_rinex_obs(text: &str, default_station: &str) -> RinexParse {
    let mut out = RinexParse::default();
    out.station = default_station.to_string();
    let mut in_header = true;
    let mut cur_epoch: i64 = 0;
    let mut cur_flag: u8 = 0;
    let mut expect_sats: usize = 0;

    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let lineno = i + 1;
        i += 1;
        if in_header {
            let label = line.get(60..).map(|s| s.trim()).unwrap_or("");
            match label {
                "MARKER NAME" => {
                    let name = line.get(0..60).unwrap_or("").trim();
                    if !name.is_empty() {
                        out.station = name.to_string();
                    }
                }
                "SYS / # / OBS TYPES" => {
                    let sys = line.chars().next().unwrap_or('G');
                    let n: usize = line.get(1..6).unwrap_or("").trim().parse().unwrap_or(0);
                    let mut codes: Vec<String> = Vec::new();
                    let mut chunk = line.get(7..60).unwrap_or("");
                    for k in 0..13 {
                        let c = chunk.get(k * 4..k * 4 + 3).unwrap_or("").trim();
                        if !c.is_empty() {
                            codes.push(c.to_string());
                        }
                    }
                    // 续行
                    while codes.len() < n {
                        if i >= lines.len() {
                            break;
                        }
                        let cont = lines[i];
                        if cont.get(60..).map(|s| s.trim()) != Some("SYS / # / OBS TYPES") {
                            break;
                        }
                        i += 1;
                        chunk = cont.get(7..60).unwrap_or("");
                        for k in 0..13 {
                            let c = chunk.get(k * 4..k * 4 + 3).unwrap_or("").trim();
                            if !c.is_empty() {
                                codes.push(c.to_string());
                            }
                        }
                    }
                    if codes.len() != n {
                        out.diagnostics.push(Diagnostic {
                            line: lineno,
                            severity: "warning".into(),
                            message: format!("观测码声明 {} 个,实际解析 {} 个", n, codes.len()),
                        });
                    }
                    out.obs_types.push((sys, codes));
                }
                "END OF HEADER" => in_header = false,
                _ => {}
            }
            continue;
        }

        // 数据区
        if line.starts_with('>') {
            match parse_epoch_line(line) {
                Some((ms, flag, nsat)) => {
                    cur_epoch = ms;
                    cur_flag = flag;
                    expect_sats = nsat;
                    if cur_flag > 1 {
                        out.diagnostics.push(Diagnostic {
                            line: lineno,
                            severity: "warning".into(),
                            message: format!("历元标志 {} 非普通观测历元,仍按原样保存", cur_flag),
                        });
                    }
                }
                None => {
                    out.diagnostics.push(Diagnostic {
                        line: lineno,
                        severity: "error".into(),
                        message: format!("无法解析历元行: {}", line),
                    });
                }
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let sat = line.get(0..3).unwrap_or("").trim().to_string();
        if sat.len() < 3 || !sat.chars().next().unwrap().is_ascii_alphabetic() {
            out.diagnostics.push(Diagnostic {
                line: lineno,
                severity: "error".into(),
                message: format!("无法识别卫星编号行: {}", line),
            });
            continue;
        }
        if expect_sats > 0 {
            expect_sats -= 1;
        }
        let sys = sat.chars().next().unwrap();
        let codes = out
            .obs_types
            .iter()
            .find(|(s, _)| *s == sys)
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        for (k, code) in codes.iter().enumerate() {
            let start = 3 + k * 16;
            let field = line.get(start..start + 16).unwrap_or("");
            if field.trim().is_empty() {
                // 缺测(如频率缺失):不产出记录,缺测本身由弧段层按历元对齐发现
                continue;
            }
            let raw = field.to_string();
            let valstr = field.get(0..14).unwrap_or("").trim();
            let lli = field.get(14..15).and_then(|s| s.trim().parse::<u8>().ok());
            let ssi = field.get(15..16).and_then(|s| s.trim().parse::<u8>().ok());
            let (value, diag) = match valstr.parse::<f64>() {
                Ok(v) => (Some(v), None),
                Err(_) => (None, Some(format!("值 {:?} 无法转换为浮点,保留原始表示", valstr))),
            };
            if diag.is_some() {
                out.diagnostics.push(Diagnostic {
                    line: lineno,
                    severity: "error".into(),
                    message: format!("{} {} {}: {}", sat, code, iso_of_ms(cur_epoch), diag.clone().unwrap()),
                });
            }
            out.obs.push(NormObs {
                station: out.station.clone(),
                epoch_ms: cur_epoch,
                sat: sat.clone(),
                code: code.clone(),
                freq_slot: freq_info(sys, code).map(|f| f.slot),
                raw,
                value,
                lli,
                ssi,
                diag,
            });
        }
    }
    out
}

/// 钟跳事件文件: 每行 `CLK <station> <yyyy-mm-ddThh:mm:ss.sssZ> <jump_ms>`
pub fn parse_clock_events(text: &str) -> (Vec<ClockEvent>, Vec<Diagnostic>) {
    let mut evs = Vec::new();
    let mut diags = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 4 || f[0] != "CLK" {
            diags.push(Diagnostic {
                line: idx + 1,
                severity: "error".into(),
                message: format!("钟跳事件行格式错误: {}", line),
            });
            continue;
        }
        match (parse_iso(f[2]), f[3].parse::<f64>()) {
            (Some(ms), Ok(j)) => evs.push(ClockEvent {
                station: f[1].to_string(),
                epoch_ms: ms,
                jump_ms: j,
                raw: line.to_string(),
            }),
            _ => diags.push(Diagnostic {
                line: idx + 1,
                severity: "error".into(),
                message: format!("钟跳事件时间或跳变量无法解析: {}", line),
            }),
        }
    }
    (evs, diags)
}

/// 卫星状态文件: 每行 `SAT <sat> <iso> <state>`
pub fn parse_sat_states(text: &str) -> (Vec<SatState>, Vec<Diagnostic>) {
    let mut evs = Vec::new();
    let mut diags = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 4 || f[0] != "SAT" {
            diags.push(Diagnostic {
                line: idx + 1,
                severity: "error".into(),
                message: format!("卫星状态行格式错误: {}", line),
            });
            continue;
        }
        match parse_iso(f[2]) {
            Some(ms) => evs.push(SatState {
                sat: f[1].to_string(),
                epoch_ms: ms,
                state: f[3].to_string(),
                raw: line.to_string(),
            }),
            None => diags.push(Diagnostic {
                line: idx + 1,
                severity: "error".into(),
                message: format!("卫星状态时间无法解析: {}", line),
            }),
        }
    }
    (evs, diags)
}

pub fn parse_iso(s: &str) -> Option<i64> {
    // 2024-03-01T00:00:00.000Z
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let sec: f64 = s.get(17..)?.trim_end_matches('Z').parse().ok()?;
    let si = sec.trunc() as i64;
    let ms = ((sec - si as f64) * 1000.0).round() as i64;
    Some(ms_of_iso(y, mo, d, h, mi, si, ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        let mut s = String::new();
        s.push_str("     3.04           OBSERVATION DATA    G                   RINEX VERSION / TYPE\n");
        s.push_str("BENCH                                                       MARKER NAME\n");
        s.push_str("G    8 C1C L1C D1C S1C C2W L2W D2W S2W                      SYS / # / OBS TYPES\n");
        s.push_str("                                                            END OF HEADER\n");
        s.push_str("> 2024 03 01 00 00  0.0000000  0  2\n");
        s.push_str("G05  22222222.222 1  116753861.105 1     -2100.000      45.000    22222444.111     90707007.007      -1600.000      42.000  \n");
        s.push_str("G07  23333333.333    122000000.000        -1800.000      44.000    \n");
        s
    }

    #[test]
    fn parses_epochs_and_codes() {
        let p = parse_rinex_obs(&sample(), "X");
        assert_eq!(p.station, "BENCH");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        let l1 = p.obs.iter().find(|o| o.sat == "G05" && o.code == "L1C").unwrap();
        assert_eq!(l1.lli, Some(1));
        assert_eq!(l1.freq_slot, Some(1));
        assert!((l1.value.unwrap() - 116753861.105).abs() < 1e-9);
        // G07 只有 4 个字段,其余缺测
        assert_eq!(p.obs.iter().filter(|o| o.sat == "G07").count(), 4);
    }

    #[test]
    fn bad_value_kept_with_diag() {
        let mut s = sample();
        s.push_str("> 2024 03 01 00 00  1.0000000  0  1\n");
        s.push_str("G05  **************  116753861.105       -2100.000      45.000    \n");
        let p = parse_rinex_obs(&s, "X");
        let bad = p.obs.iter().find(|o| o.code == "C1C" && o.epoch_ms > 0).unwrap();
        assert!(bad.value.is_none());
        assert!(bad.diag.is_some());
        assert!(bad.raw.contains('*'));
        assert!(!p.diagnostics.is_empty());
    }

    #[test]
    fn clock_and_state_files() {
        let (c, d1) = parse_clock_events("CLK BENCH 2024-03-01T00:00:05.000Z 1.0\nbad line\n");
        assert_eq!(c.len(), 1);
        assert_eq!(d1.len(), 1);
        let (s, _) = parse_sat_states("SAT G05 2024-03-01T00:00:00.000Z HEALTHY\n");
        assert_eq!(s.len(), 1);
    }
}
