//! 领域模型：原始输入层（不可变）与事件负载。
//! 伪距/载波/Doppler/LLI/信噪比/钟跳事件/频率编号/观测码均按历元原样保存。
use serde::{Deserialize, Serialize};

use crate::units::{Datum, Quant};

/// 一次提交的观测历元。`epoch_time_raw` 是文件里的原始时间表示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochIn {
    /// 例如 "2026-09-24T00:00:00"（秒，允许小数）。解析失败保留原文 + 诊断。
    pub time: String,
    /// 原始时间表示（如 RINEX 的年 月 日 时 分 秒 字段拼接），原样保留。
    #[serde(default)]
    pub time_raw: String,
    /// 历元标志（RINEX flag），原样保留。
    #[serde(default)]
    pub epoch_flag: String,
    #[serde(default)]
    pub sat_states: Vec<SatStateIn>,
    /// 每颗星每个观测码一条观测。顺序即文件顺序（用于顺序身份）。
    #[serde(default)]
    pub observations: Vec<ObservationIn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SatStateIn {
    /// 卫星号原样，如 "G01" / "E11" / "C06"。
    pub sat: String,
    /// 星座，由卫星号首字母或显式给出；无法判定保留原文。
    #[serde(default)]
    pub system: String,
    /// 频率编号/波段描述原样（可空）。
    #[serde(default)]
    pub freq_number: String,
    #[serde(default)]
    pub position: Option<Vec3>,
    #[serde(default)]
    pub clock_bias: Option<Quant>,
    #[serde(default)]
    pub datum: Option<Datum>,
    /// 未识别的卫星状态字段原样保留。
    #[serde(default)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: Quant,
    pub y: Quant,
    pub z: Quant,
    pub datum: Datum,
}

/// 单条观测：观测码、频率编号与各观测量都按原样保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationIn {
    pub sat: String,
    /// RINEX 观测码原样，如 "C1C" / "L1C" / "D5X" / "S2W"。
    pub code: String,
    /// 频率编号（派生自码或文件头映射），保留其来源说明。
    #[serde(default)]
    pub freq_band: String,
    #[serde(default)]
    pub freq_source: String,
    #[serde(default)]
    pub pseudorange: Option<Quant>,
    #[serde(default)]
    pub carrier: Option<Quant>,
    #[serde(default)]
    pub doppler: Option<Quant>,
    /// LLI 原始标志（0/1/2/4 组合），原样字符串。
    #[serde(default)]
    pub lli: Option<String>,
    #[serde(default)]
    pub snr: Option<Quant>,
    /// 字段原文（如 RINEX 14 列字段），无法解析时整段留此并出诊断。
    #[serde(default)]
    pub raw_field: Option<String>,
}

/// 整机钟跳事件（可由导入显式提供或注释解析）。
#[derive(Debug,Clone, Serialize, Deserialize)]
pub struct ClockJumpIn {
    pub time: String,
    #[serde(default)]
    pub time_raw: String,
    /// 钟跳量，带单位（通常 "s" 或 "ns"/"us"）。
    pub jump: Quant,
    /// 来源："explicit"（文件事件）/ "header-comment" / "manual"。
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub note: String,
}

/// 提交载荷：一批历元 + 可选钟跳事件 + 可选站信息。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommitIn {
    #[serde(default)]
    pub epochs: Vec<EpochIn>,
    #[serde(default)]
    pub clock_jumps: Vec<ClockJumpIn>,
    #[serde(default)]
    pub station: Option<StationIn>,
    #[serde(default)]
    pub source_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StationIn {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub marker: String,
    #[serde(default)]
    pub approx_position: Option<Vec3>,
    #[serde(default)]
    pub receiver: String,
    #[serde(default)]
    pub antenna: String,
}

/// 解析诊断：无法转换/识别的片段留在原位置，这里给出原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String, // error | warning
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub raw_snippet: String,
}

/// 原始导入解析结果（尚未提交即带诊断）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParsedImport {
    pub format: String,
    pub commit: CommitIn,
    pub diagnostics: Vec<Diagnostic>,
}

/// 检测器参数（参数修订针对这一层）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorParams {
    /// GF/电离层组合判据阈值（周）。
    pub gf_cycles: f64,
    /// MW/WL 判据阈值（周）。
    pub mw_cycles: f64,
    /// 历元间隔异常倍数（相对中位间隔）。
    pub gap_factor: f64,
    /// 钟跳判定：参与星占比。
    pub clock_sat_fraction: f64,
    /// 钟跳判定：码跳变最小幅度（m）。
    pub clock_code_jump_m: f64,
    /// LLI 是否仅作证据而非自动断周跳。
    pub lli_advisory_only: bool,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            gf_cycles: 0.5,
            mw_cycles: 0.5,
            gap_factor: 3.0,
            clock_sat_fraction: 0.8,
            clock_code_jump_m: 50.0,
            lli_advisory_only: true,
        }
    }
}

/// 周跳整周候选（某检测器给出的 (n1,n2) 及得分）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OffsetHypothesis {
    pub n1: i32,
    pub n2: i32,
    /// 修复后残差（周），越小越好；同分时全部保留。
    pub residual_cycles: f64,
}
