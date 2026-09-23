//! OBX（Observation eXchange, v1）解析器：可复制的小型 RINEX 风格样例。
//!
//! 设计要点：
//! - 伪距/载波/Doppler/LLI/信噪比/钟跳/频率编号/观测码 全部按历元原样保存；
//! - 每个量带声明单位，能转换则填 normalized（派生层规范化），不能则留在原位置附诊断；
//! - 解析错误是一等结果：硬错误返回 Err（拒绝整批），软问题进入 diagnostics 但不丢数据。

pub mod units;

use crate::domain::{FreqSlot, ObsCode, Qty, RawEpoch, RawObs, RawSatObs};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedInput {
    pub format: String,
    pub station: String,
    pub receiver_clock: String,
    pub coord_datum: String,
    pub freq_table: Vec<FreqSlot>,
    pub epochs: Vec<RawEpoch>,
    pub clock_lines: Vec<ClockLine>,
    pub nav: Vec<NavRow>,
    pub diagnostics: Vec<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClockLine {
    pub epoch_label: String,
    pub jump: Qty,
    pub scope: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavRow {
    pub sat: String,
    pub datum: String,
    pub fields: Map<String, Value>,
    pub raw: String,
}

#[derive(Debug, Clone, Default)]
struct HeaderUnits {
    c: String,
    l: String,
    d: String,
    s: String,
}

pub fn content_hash_of(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_obx(text: &str) -> Result<ParsedInput, String> {
    let mut diags: Vec<String> = Vec::new();
    let mut format = String::from("obx/1");
    let mut station = String::from("STATION");
    let mut receiver_clock = String::from("RXCLK");
    let mut coord_datum = String::from("ITRF2014");
    let mut units = HeaderUnits {
        c: "m".into(),
        l: "cycles".into(),
        d: "Hz".into(),
        s: "dBHz".into(),
    };
    let mut freq_table: Vec<FreqSlot> = Vec::new();
    let mut clock_lines: Vec<ClockLine> = Vec::new();
    let mut nav: Vec<NavRow> = Vec::new();
    let mut epochs: Vec<RawEpoch> = Vec::new();

    let mut section = "";
    let mut current_epoch: Option<RawEpoch> = None;

    for (lineno0, raw_line) in text.lines().enumerate() {
        let lineno = lineno0 + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(ep) = current_epoch.take() {
                epochs.push(ep);
            }
            section = &line[1..line.len() - 1];
            continue;
        }

        match section {
            "HEADER" | "" => {
                let Some((key, rest)) = line.split_once(char::is_whitespace) else {
                    return Err(format!("第 {lineno} 行：HEADER 行缺少值: {line}"));
                };
                let rest = rest.trim();
                match key {
                    "FORMAT" => format = rest.to_string(),
                    "STATION" => station = rest.to_string(),
                    "RECEIVER_CLOCK" => receiver_clock = rest.to_string(),
                    "COORD_DATUM" => coord_datum = rest.to_string(),
                    "UNITS" => {
                        for tok in rest.split_whitespace() {
                            let Some((k, u)) = tok.split_once('=') else {
                                return Err(format!("第 {lineno} 行：UNITS 需要 K=u，得到 {tok}"));
                            };
                            match k {
                                "C" => units.c = u.into(),
                                "L" => units.l = u.into(),
                                "D" => units.d = u.into(),
                                "S" => units.s = u.into(),
                                other => {
                                    return Err(format!(
                                        "第 {lineno} 行：UNITS 未知类别 {other}"
                                    ))
                                }
                            }
                        }
                    }
                    other => diags.push(format!("第 {lineno} 行：忽略未知 HEADER 键 {other}")),
                }
            }
            "FREQ" => {
                let toks: Vec<&str> = line.split_whitespace().collect();
                if toks[0] != "FREQ" || toks.len() < 3 {
                    return Err(format!("第 {lineno} 行：FREQ 行应为 `FREQ <系统频点> <Hz>`"));
                }
                let name = toks[1];
                let mut chars = name.chars();
                let system = chars.next().ok_or_else(|| format!("第 {lineno} 行：频点名非法"))?;
                let band: u8 = chars
                    .collect::<String>()
                    .parse()
                    .map_err(|_| format!("第 {lineno} 行：频点编号非法 {name}"))?;
                let freq_q = units::parse_quantity(toks[2], "Hz");
                let freq_hz = freq_q.normalized.ok_or_else(|| {
                    format!(
                        "第 {lineno} 行：频率无法转换为 Hz：{}（{}）",
                        freq_q.raw,
                        freq_q.diagnostic.unwrap_or_default()
                    )
                })?;
                freq_table.push(FreqSlot { system, band, freq_hz });
            }
            "NAV" => {
                let toks: Vec<&str> = line.split_whitespace().collect();
                if toks[0] != "NAV" || toks.len() < 2 {
                    return Err(format!("第 {lineno} 行：NAV 行应为 `NAV <卫星> k=v ...`"));
                }
                let mut fields = Map::new();
                let mut datum = coord_datum.clone();
                for kv in &toks[2..] {
                    let Some((k, v)) = kv.split_once('=') else {
                        return Err(format!("第 {lineno} 行：NAV 需要 k=v，得到 {kv}"));
                    };
                    if k == "datum" {
                        datum = v.to_string();
                        continue;
                    }
                    fields.insert(
                        k.to_string(),
                        v.parse::<f64>().map(Value::from).unwrap_or(Value::from(v)),
                    );
                }
                nav.push(NavRow {
                    sat: toks[1].to_string(),
                    datum,
                    fields,
                    raw: line.to_string(),
                });
            }
            "CLOCK" => {
                let toks: Vec<&str> = line.split_whitespace().collect();
                if toks[0] != "EVENT" || toks.len() < 4 {
                    return Err(format!(
                        "第 {lineno} 行：CLOCK 行应为 `EVENT <历元标签> <跳变> <scope> <source>`"
                    ));
                }
                clock_lines.push(ClockLine {
                    epoch_label: toks[1].to_string(),
                    jump: units::parse_quantity(toks[2], "s"),
                    scope: toks[3].to_string(),
                    source: toks.get(4).copied().unwrap_or("operator").to_string(),
                });
            }
            "DATA" => {
                let toks: Vec<&str> = line.split_whitespace().collect();
                match toks[0] {
                    "EPOCH" => {
                        if toks.len() < 3 {
                            return Err(format!(
                                "第 {lineno} 行：EPOCH 需要 `<标签> <标志> [钟差]`"
                            ));
                        }
                        if let Some(ep) = current_epoch.take() {
                            epochs.push(ep);
                        }
                        let clock_offset = if toks.len() >= 4 {
                            Some(units::parse_quantity(toks[3], "s"))
                        } else {
                            None
                        };
                        current_epoch = Some(RawEpoch {
                            label: toks[1].to_string(),
                            flag: toks[2].to_string(),
                            clock_offset,
                            sats: Vec::new(),
                        });
                    }
                    "SAT" => {
                        let Some(ep) = current_epoch.as_mut() else {
                            return Err(format!("第 {lineno} 行：SAT 必须位于 EPOCH 之下"));
                        };
                        if toks.len() < 2 {
                            return Err(format!("第 {lineno} 行：SAT 缺少卫星编号"));
                        }
                        let sat = toks[1].to_string();
                        let mut obs = Vec::new();
                        for tok in &toks[2..] {
                            let parsed = parse_obs_token(tok, &units, lineno, &mut diags)?;
                            obs.push(parsed);
                        }
                        ep.sats.push(RawSatObs { sat, obs });
                    }
                    other => {
                        return Err(format!("第 {lineno} 行：DATA 段未知记录类型 {other}"))
                    }
                }
            }
            other => return Err(format!("第 {lineno} 行：未知段 [{other}]")),
        }
    }
    if let Some(ep) = current_epoch.take() {
        epochs.push(ep);
    }

    if format != "obx/1" {
        return Err(format!("仅支持 FORMAT obx/1，得到 {format}"));
    }
    if epochs.is_empty() {
        return Err("没有任何 EPOCH 记录".into());
    }
    let mut labels: Vec<&str> = epochs.iter().map(|e| e.label.as_str()).collect();
    labels.sort();
    labels.dedup();
    if labels.len() != epochs.len() {
        return Err("历元标签必须唯一".into());
    }
    freq_table.sort_by(|a, b| (a.system, a.band).cmp(&(b.system, b.band)));

    let content_hash = content_hash_of(text);
    Ok(ParsedInput {
        format,
        station,
        receiver_clock,
        coord_datum,
        freq_table,
        epochs,
        clock_lines,
        nav,
        diagnostics: diags,
        content_hash,
    })
}

fn parse_obs_token(
    tok: &str,
    units: &HeaderUnits,
    lineno: usize,
    diags: &mut Vec<String>,
) -> Result<RawObs, String> {
    // 形如 CODE:value[/LLI]；信噪比缺失的频点直接省略整个 token。
    let Some((code_s, rest)) = tok.split_once(':') else {
        return Err(format!("第 {lineno} 行：观测量需 `CODE:value[/LLI]`，得到 {tok}"));
    };
    let code = ObsCode::parse(code_s)?;
    let (val_s, lli) = match rest.split_once('/') {
        Some((v, l)) => (v, Some(l.to_string())),
        None => (rest, None),
    };
    let declared = match code.kind {
        'C' => units.c.clone(),
        'L' => units.l.clone(),
        'D' => units.d.clone(),
        'S' => units.s.clone(),
        _ => unreachable!(),
    };
    let mut value = units::parse_quantity(val_s, &declared);
    if value.normalized.is_none() {
        diags.push(format!(
            "第 {lineno} 行：{code_s} 值 `{val_s}` 无法从 {declared} 转换，保留原值（{}）",
            value.diagnostic.clone().unwrap_or_default()
        ));
    }
    // SSI：S 码若为 0..=9 的整数，原始位即信号强度位。
    let ssi = if code.kind == 'S' {
        Some(value.raw.clone())
    } else {
        None
    };
    if value.normalized_unit.is_none() {
        value.normalized_unit = Some(match code.kind {
            'C' => "m",
            'L' => "cycles",
            'D' => "Hz",
            _ => "dBHz",
        }.to_string());
    }
    Ok(RawObs { code, value, lli, ssi })
}
