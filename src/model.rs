//! 领域模型：单位、频率基准、虚拟时钟、检测参数。
//! 原则：所有领域量携带单位或坐标基准；原始表示在导入层保留，规范化只发生在派生层。

use serde::{Deserialize, Serialize};

/// 光速 (m/s)，SI 定义值。
pub const C_M_PER_S: f64 = 299_792_458.0;

/// 检测算法版本。算法行为变化时必须升级，派生值借此与旧结果区分。
pub const DETECT_ALGO_VERSION: &str = "csb-detect/1.0.0";

/// 导出格式版本。
pub const EXPORT_FORMAT: &str = "gnss-csb-export/1";

/// 虚拟时钟：生产用系统时间，测试用固定时间。逻辑顺序一律以 seq 为准。
#[derive(Clone, Debug)]
pub enum Clock {
    System,
    Fixed(String),
}

impl Clock {
    pub fn now(&self) -> String {
        match self {
            Clock::System => chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            Clock::Fixed(t) => t.clone(),
        }
    }
}

/// 观测值单位。原始层按 RINEX 约定保存，派生层换算时显式记录。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Meters,   // 伪距 C*
    Cycles,   // 载波 L*
    Hertz,    // 多普勒 D*
    DbHz,     // 信噪比 S*
    Flag,     // LLI / 事件标志
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Unit::Meters => "m",
            Unit::Cycles => "cycles",
            Unit::Hertz => "Hz",
            Unit::DbHz => "dB-Hz",
            Unit::Flag => "flag",
        }
    }
    pub fn of_obs_code(code: &str) -> Option<Unit> {
        match code.as_bytes().first()? {
            b'C' => Some(Unit::Meters),
            b'L' => Some(Unit::Cycles),
            b'D' => Some(Unit::Hertz),
            b'S' => Some(Unit::DbHz),
            _ => None,
        }
    }
}

/// 载波频率 (Hz)。GLONASS 为 FDMA，需要频率编号 k（来自卫星状态导入）。
pub fn frequency_hz(sys: char, band: char, glo_k: Option<i32>) -> Option<f64> {
    match (sys, band) {
        ('G', '1') => Some(1575.42e6),
        ('G', '2') => Some(1227.60e6),
        ('E', '1') => Some(1575.42e6),
        ('E', '5') => Some(1176.45e6),
        ('C', '2') => Some(1561.098e6),
        ('R', '1') => glo_k.map(|k| (1602.0 + 0.5625 * k as f64) * 1e6),
        ('R', '2') => glo_k.map(|k| (1246.0 + 0.4375 * k as f64) * 1e6),
        _ => None,
    }
}

pub fn wavelength_m(sys: char, band: char, glo_k: Option<i32>) -> Option<f64> {
    frequency_hz(sys, band, glo_k).map(|f| C_M_PER_S / f)
}

/// 每系统用于双频组合的第二波段（派生层内部统一为索引 1）。
pub fn band2_of(sys: char) -> char {
    match sys {
        'E' => '5',
        'C' => '7',
        _ => '2',
    }
}

/// 观测码 -> (类别, 波段)。如 "L2W" -> ('L','2')。
pub fn parse_obs_code(code: &str) -> Option<(char, char)> {
    let b = code.as_bytes();
    if b.len() >= 2 {
        Some((b[0] as char, b[1] as char))
    } else {
        None
    }
}

/// 检测参数。完整参与参数指纹；任何变化都会产生新的派生身份。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DetectParams {
    /// GF 组合跳变基础阈值 (m)
    pub gf_base_m: f64,
    /// GF 阈值随间隔增长 (m/s)
    pub gf_rate_m_per_s: f64,
    /// MW 组合阈值 (宽巷周)
    pub mw_thr_cycles: f64,
    /// TD 基础阈值 (周)
    pub td_base_cycles: f64,
    /// TD 阈值随间隔增长 (周/s)
    pub td_rate_cycles_per_s: f64,
    /// TD 小数部分距 0.5 多近视为“等分整周”平局 (周)
    pub td_tie_halfwidth: f64,
    /// 弧段断裂间隔 (s)
    pub max_gap_s: f64,
    /// 接收机钟跳最小共模量 (m)
    pub clock_min_m: f64,
    /// 钟跳共模一致性偏差 (m)
    pub clock_dev_m: f64,
    /// GF 整周对搜索范围 (周)
    pub pair_search_range: i32,
}

impl Default for DetectParams {
    fn default() -> Self {
        DetectParams {
            gf_base_m: 0.10,
            gf_rate_m_per_s: 0.002,
            mw_thr_cycles: 0.8,
            td_base_cycles: 1.0,
            td_rate_cycles_per_s: 0.01,
            td_tie_halfwidth: 0.06,
            max_gap_s: 120.0,
            clock_min_m: 1.0,
            clock_dev_m: 0.10,
            pair_search_range: 20,
        }
    }
}

impl DetectParams {
    /// 参数指纹：规范 JSON 的短摘要，随任何参数变化而变化。
    pub fn fingerprint(&self) -> String {
        let canon = serde_json::to_string(self).unwrap_or_default();
        crate::ids::fingerprint(&canon)
    }
}

/// 依赖作用域的版本指针：派生值记录它计算时所依赖的每个作用域版本。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScopeDep {
    pub scope: String,
    pub version: String,
}

/// 解析后的单条观测（原始层行）。raw 永远保留提交时的原样字段。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsRow {
    pub id: String,
    pub version_id: String,
    pub station: String,
    pub sat: String,
    pub code: String,
    pub epoch: String,
    pub epoch_ns: i64,
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String,
    pub lli: Option<i64>,
    pub ssi: Option<i64>,
    pub diag: Option<String>,
}

/// 把 ISO-8601 (UTC, 允许小数秒) 解析为纳秒时间戳。
pub fn epoch_to_ns(epoch: &str) -> Option<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(epoch).ok()?;
    Some(dt.timestamp_nanos_opt().unwrap_or(dt.timestamp() * 1_000_000_000))
}

pub fn ns_to_epoch(ns: i64) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    let nanos = ns.rem_euclid(1_000_000_000);
    let dt = chrono::DateTime::from_timestamp(secs, nanos as u32)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    dt.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}
