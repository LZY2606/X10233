//! 导入层：接受可复制的小型行格式样例，也产出明确的格式错误。
//! 原始行逐字保存；不可转换的标量留在原位置并附诊断，绝不丢弃。

use crate::ids::{clock_id, epoch_id, obs_id};
use crate::model::{FrequencySpec, Qty, ClockEvent, RawEpoch, RawObservation};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct ParseOutput {
    pub frequencies: Vec<FrequencySpec>,
    pub epochs: Vec<RawEpoch>,
    pub observations: Vec<RawObservation>,
    pub clocks: Vec<ClockEvent>,
    /// (epoch_tag, sat, band) 的频点缺失声明
    pub missing: Vec<(String, String, String)>,
}

#[derive(Debug, Clone)]
pub struct ImportError {
    pub line_no: usize,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {} [{}]: {}", self.line_no, self.code, self.message)
    }
}
impl std::error::Error for ImportError {}

/// 数值 token：可解析则规范化；不可解析保留原文与诊断，不中断整批导入。
fn qty(unit: &str, token: &str, code: &str) -> Qty {
    match token.parse::<f64>() {
        Ok(v) if v.is_finite() => Qty {
            value: v,
            unit: unit.into(),
            raw: token.into(),
            diagnostic: None,
        },
        _ => Qty::broken(unit, token, code),
    }
}

pub fn parse(project: &str, text: &str) -> Result<ParseOutput, ImportError> {
    let mut out = ParseOutput::default();
    let mut epoch_tags: HashSet<String> = HashSet::new();
    let mut seen_obs: HashSet<(String, String, String, String)> = HashSet::new();

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let err = |code: &str, msg: String| ImportError {
            line_no,
            code: code.into(),
            message: msg,
        };

        match f[0] {
            "FREQ" => {
                if f.len() != 7 {
                    return Err(err("FREQ_FIELD_COUNT", format!("FREQ 需要 6 个字段，得到 {}", f.len() - 1)));
                }
                let wavelength: f64 = f[6].parse().map_err(|_| {
                    err("FREQ_WAVELENGTH_NONNUMERIC", format!("波长无法解析: {}", f[6]))
                })?;
                out.frequencies.push(FrequencySpec {
                    constellation: f[1].into(),
                    band: f[2].into(),
                    code: f[3].into(),
                    freq_number: f[4].into(),
                    wavelength_m: wavelength,
                    datum: "RINEX-frequency-band".into(),
                });
            }
            "EPOCH" => {
                if f.len() != 3 {
                    return Err(err("EPOCH_FIELD_COUNT", format!("EPOCH 需要 2 个字段，得到 {}", f.len() - 1)));
                }
                let tag = f[1].to_string();
                let time: f64 = f[2].parse().map_err(|_| {
                    err("EPOCH_TIME_NONNUMERIC", format!("历元时间无法解析: {}", f[2]))
                })?;
                if !epoch_tags.insert(tag.clone()) {
                    return Err(err("EPOCH_DUPLICATE_TAG", format!("重复历元标签: {}", tag)));
                }
                out.epochs.push(RawEpoch {
                    id: epoch_id(project, &tag),
                    tag,
                    time_s: Qty::seconds(time, f[2]),
                    time_raw: f[2].into(),
                    interval_from_prev_s: None,
                    regular_spacing: false,
                    raw_header: raw_line.to_string(),
                    source_version: String::new(),
                    missing: vec![],
                });
            }
            "OBS" => {
                if f.len() != 11 {
                    return Err(err("OBS_FIELD_COUNT", format!("OBS 需要 10 个字段（标签 卫星 频带 观测码 频点号 伪距 载波 Doppler LLI 信噪比），得到 {}", f.len() - 1)));
                }
                let (tag, sat, band, code_tok, fnum) =
                    (f[1].to_string(), f[2].to_string(), f[3].to_string(), f[4].to_string(), f[5].to_string());
                if !epoch_tags.contains(&tag) {
                    return Err(err("OBS_UNKNOWN_EPOCH", format!("观测引用了未声明历元: {}", tag)));
                }
                let key = (tag.clone(), sat.clone(), band.clone(), code_tok.clone());
                if !seen_obs.insert(key) {
                    return Err(err("OBS_DUPLICATE", format!("重复观测: {}/{}/{}/{}", tag, sat, band, code_tok)));
                }
                let pseudorange = qty("m", f[6], "pseudorange_non_numeric");
                let carrier = qty("cycle", f[7], "carrier_non_numeric");
                let doppler = qty("Hz", f[8], "doppler_non_numeric");
                let lli = qty("flag", f[9], "lli_non_numeric");
                let cnr = qty("dB-Hz", f[10], "cnr_non_numeric");
                let epoch = epoch_id(project, &tag);
                out.observations.push(RawObservation {
                    id: obs_id(project, raw_line),
                    epoch_id: epoch,
                    epoch_tag: tag,
                    sat,
                    band,
                    code: code_tok,
                    freq_number: fnum,
                    pseudorange,
                    carrier,
                    doppler,
                    lli,
                    cnr,
                    raw_line: raw_line.to_string(),
                    source_version: String::new(),
                });
            }
            "CLOCK" => {
                if f.len() != 4 {
                    return Err(err("CLOCK_FIELD_COUNT", format!("CLOCK 需要 3 个字段，得到 {}", f.len() - 1)));
                }
                let tag = f[1].to_string();
                if !epoch_tags.contains(&tag) {
                    return Err(err("CLOCK_UNKNOWN_EPOCH", format!("钟跳引用了未声明历元: {}", tag)));
                }
                out.clocks.push(ClockEvent {
                    id: clock_id(project, raw_line),
                    epoch_tag: tag,
                    raw: raw_line.to_string(),
                    kind: f[2].into(),
                    shift_s: qty("s", f[3], "clock_shift_non_numeric"),
                    source_version: String::new(),
                });
            }
            "MISSING" => {
                if f.len() != 4 {
                    return Err(err("MISSING_FIELD_COUNT", format!("MISSING 需要 3 个字段，得到 {}", f.len() - 1)));
                }
                if !epoch_tags.contains(f[1]) {
                    return Err(err("MISSING_UNKNOWN_EPOCH", format!("缺失声明引用了未声明历元: {}", f[1])));
                }
                out.missing.push((f[1].to_string(), f[2].to_string(), f[3].to_string()));
            }
            other => {
                return Err(err("UNKNOWN_RECORD", format!("未知记录类型: {}", other)));
            }
        }
    }
    Ok(out)
}
