//! 领域模型：所有 GNSS 领域量都携带单位/坐标基准；原始表示与规范化表示分离。
//! 原始层（raw）一经提交不可覆盖；规范化只发生在派生层。

use serde::{Deserialize, Serialize};

/// 值的可转换状态。无法转换的值留在原位并附诊断，绝不丢弃。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValueStatus {
    /// 正常解析并已规范化
    Ok,
    /// 该历元该码缺失（频率缺失）
    Missing,
    /// 原始表示存在但无法转换；保留原文与诊断
    Unparseable { diag: String },
}

/// 带单位/基准的领域量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    /// 规范化后的 SI 值（距离=米，相位=周，Doppler=周/秒）；不可转换为 None
    pub value: Option<f64>,
    /// 导入时的原始表示（逐字符保留）
    pub raw: String,
    /// 单位标签（"m" / "cycle" / "Hz" / 自定义原始单位）
    pub unit: String,
    pub status: ValueStatus,
}

impl Quantity {
    pub fn ok(value: f64, raw: impl Into<String>, unit: impl Into<String>) -> Self {
        Quantity { value: Some(value), raw: raw.into(), unit: unit.into(), status: ValueStatus::Ok }
    }
    pub fn missing(raw: impl Into<String>) -> Self {
        Quantity { value: None, raw: raw.into(), unit: String::new(), status: ValueStatus::Missing }
    }
    pub fn unparseable(raw: impl Into<String>, diag: impl Into<String>) -> Self {
        Quantity {
            value: None,
            raw: raw.into(),
            unit: String::new(),
            status: ValueStatus::Unparseable { diag: diag.into() },
        }
    }
    pub fn is_ok(&self) -> bool {
        self.value.is_some()
    }
}

/// GNSS 时间。时间基准固定为 GPS 时间（1980-01-06T00:00:00 GPS），秒为连续实数。
/// `raw` 保存历元时间戳的原始表示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpsTime {
    /// 自 GPS 历元起的连续秒（含小数，用于不等间隔历元）
    pub secs: f64,
    /// 规范日历表示，用于展示与稳定身份
    pub iso: String,
    /// 导入时的原始时间戳表示
    pub raw: String,
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// 日历（UTC/GPS 民用日）到自 1970-01-01 的天数（Howard Hinnant 历算）。
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

const GPS_EPOCH_UNIX_DAYS: i64 = days_from_civil(1980, 1, 6);

/// 由 `YYYY MM DD HH MM SS.sssssss`（RINEX 民用历元，时区基准按 GPS 民用时）解析。
pub fn parse_epoch(tokens: &[&str]) -> Result<GpsTime, String> {
    if tokens.len() < 6 {
        return Err(format!("历元需要 6 个字段，得到 {}", tokens.len()));
    }
    let year: i64 = tokens[0].trim().parse().map_err(|_| format!("年份非法: {}", tokens[0]))?;
    let month: u32 = tokens[1].trim().parse().map_err(|_| format!("月份非法: {}", tokens[1]))?;
    let day: u32 = tokens[2].trim().parse().map_err(|_| format!("日非法: {}", tokens[2]))?;
    let hour: u32 = tokens[3].trim().parse().map_err(|_| format!("时非法: {}", tokens[3]))?;
    let minute: u32 = tokens[4].trim().parse().map_err(|_| format!("分非法: {}", tokens[4]))?;
    let secf: f64 = tokens[5].trim().parse().map_err(|_| format!("秒非法: {}", tokens[5]))?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || !(0.0..60.0).contains(&secf) {
        return Err(format!("历元越界: {}", tokens.join(" ")));
    }
    let days = days_from_civil(year, month, day) - GPS_EPOCH_UNIX_DAYS;
    let secs = days as f64 * 86400.0 + hour as f64 * 3600.0 + minute as f64 * 60.0 + secf;
    let whole = secf.floor() as u32;
    let iso = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:0>9.6}", year, month, day, hour, minute, whole as f64 + (secf - whole as f64));
    Ok(GpsTime { secs, iso, raw: tokens.join(" ") })
}

pub fn iso_from_secs(secs: f64) -> String {
    let total_days = (secs.div_euclid(86400.0)) as i64;
    let rem = secs.rem_euclid(86400.0);
    let (y, m, d) = civil_from_days(GPS_EPOCH_UNIX_DAYS + total_days);
    let hh = (rem / 3600.0) as u32;
    let mm = ((rem % 3600.0) / 60.0) as u32;
    let ss = rem % 60.0;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:0>9.6}", y, m, d, hh, mm, ss)
}

/// 频率定义：频率编号（如 GLONASS 通道号 -7..+6）与观测码均按原样保存。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrequencyDef {
    /// 逻辑频带键，如 "L1"
    pub band: String,
    /// 频率编号原文（RINEX 频带字符或 GLONASS 槽位号）
    pub freq_number_raw: String,
    /// 观测码原文（如 "C1C" / "L1W" / "D1C" / "S1C"）
    pub codes: Vec<String>,
    /// 频率 (Hz)
    pub frequency_hz: f64,
    /// 波长 (m/cycle)
    pub wavelength_m: f64,
}

/// 某一频带上的一个历元观测量集合（原始 + 规范化）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BandObs {
    pub band: String,
    pub pseudorange: Option<Quantity>,
    pub carrier: Option<Quantity>,
    pub doppler: Option<Quantity>,
    /// LLI 原始字符（RINEX 失锁指示，逐字符保留）
    pub lli_raw: String,
    /// 信噪比原文
    pub snr: Option<Quantity>,
    /// 该历元该频带是否整体缺失（频率缺失）
    pub missing: bool,
}

/// 钟跳事件（整机钟跳），独立于单星周跳保存。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockEvent {
    pub epoch_index: usize,
    /// 事件类型原文（如 "JUMP_NS"）
    pub kind_raw: String,
    /// 钟跳幅度原始表示
    pub amount_raw: String,
    /// 单位原文（"ns"）
    pub unit_raw: String,
    /// 规范化为纳秒；不可转换为 None 并附诊断
    pub amount_ns: Option<f64>,
    pub status: ValueStatus,
    /// 来源原文行
    pub source_line: String,
}

/// 一个站-星-历元的原始观测记录。一经提交不可覆盖。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpochObs {
    pub epoch_index: usize,
    pub station: String,
    pub sat: String,
    pub time: GpsTime,
    pub bands: Vec<BandObs>,
    pub clock_event: Option<ClockEvent>,
    /// 卫星状态原文（RINEX epoch flag / 健康状态）
    pub sat_status_raw: String,
    /// 提交此记录的原始文本片段（输入片段，供反查）
    pub source_lines: Vec<String>,
}

/// 站点元数据（坐标基准随坐标保存）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StationMeta {
    pub station: String,
    pub marker_name: String,
    /// 近似坐标 XYZ（ITRF/地心基准标签随附）
    pub xyz_m: [Option<f64>; 3],
    pub datum: String,
    pub xyz_raw: [String; 3],
}

/// 提交载荷：一次不可变输入提交的全部内容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CommitPayload {
    pub format: String,
    pub frequencies: Vec<FrequencyDef>,
    pub stations: Vec<StationMeta>,
    pub epochs: Vec<EpochObs>,
    /// 无法解析为结构化记录但保留下来的原始行（不丢弃）
    pub orphan_lines: Vec<OrphanLine>,
    /// 导入诊断（格式错误等）
    pub diagnostics: Vec<ImportDiagnostic>,
    /// 提交时的原始全文（用于审计/反查）
    pub raw_blob: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrphanLine {
    pub line_no: usize,
    pub text: String,
    pub diag: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportDiagnostic {
    pub line_no: usize,
    pub severity: String,
    pub code: String,
    pub message: String,
}

/// 检测/修复参数。指纹由其确定性 JSON 计算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Params {
    pub algo_version: String,
    /// 历元间隔标称秒数；超过 gap_factor 倍视为弧段边界（不等间隔历元）
    pub nominal_dt_s: f64,
    pub gap_factor: f64,
    /// 各检测器的模型噪声（周/米），与稳健 MAD 取大者
    pub gf_sigma_m: f64,
    pub wl_sigma_cycle: f64,
    pub td_sigma_cycle: f64,
    /// 候选整数枚举半径（周）
    pub search_radius_cycle: i32,
    /// 判定阈值（sigma 倍数）
    pub n_sigma: f64,
    /// 钟跳共模判定：同符号同幅度比例阈值
    pub clock_common_ratio: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            algo_version: "gsb-algo-1.0.0".to_string(),
            nominal_dt_s: 1.0,
            gap_factor: 1.5,
            gf_sigma_m: 0.02,
            wl_sigma_cycle: 0.15,
            td_sigma_cycle: 0.20,
            search_radius_cycle: 12,
            n_sigma: 3.0,
            clock_common_ratio: 0.8,
        }
    }
}

/// 检测器身份（不以任一检测器当真值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    /// 电离层（几何无关）组合
    GeometryFree,
    /// Melbourne-Wubbena 宽巷组合
    MelbourneWubbena,
    /// 时间差分（载波 vs Doppler 预测）
    TimeDifference,
}

impl Detector {
    pub fn key(self) -> &'static str {
        match self {
            Detector::GeometryFree => "gf",
            Detector::MelbourneWubbena => "mw",
            Detector::TimeDifference => "td",
        }
    }
    pub fn all() -> [Detector; 3] {
        [Detector::GeometryFree, Detector::MelbourneWubbena, Detector::TimeDifference]
    }
}
