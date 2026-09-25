//! 原始领域类型。导入层只负责"按历元原样保存"：
//! 伪距 / 载波 / Doppler / LLI / 信噪比 / 接收机钟偏移 / 频率编号 / 观测码
//! 一律保留原始 token（16 字符字段原文），派生层才做规范化。
//! 无法转换的值留在原位置并附诊断，绝不为了画图丢弃。

use serde::{Deserialize, Serialize};

use crate::time::Civil;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub at: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// 单个观测字段（一颗星一个观测码一格）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawCell {
    /// 观测码，例如 L1C / C1C / D1C / S1C，原样 3 字符
    pub code: String,
    /// RINEX 字段原文（16 字符定宽，含前导空格）；缺测为全空格
    pub token: String,
    /// 数值部分原文（token 前 14 字符），缺测为 None
    pub value_raw: Option<String>,
    pub value: Option<f64>,
    /// LLI 标志原文（单字符，空格=无）
    pub lli_raw: String,
    /// SSI 信噪比原文（单字符，空格=无）
    pub ssi_raw: String,
    /// 该格规范化时产生的诊断（如观测码无法对应频率）
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawSatLine {
    pub sat: String,
    pub cells: Vec<RawCell>,
}

/// 一个观测历元（原始）。所有字段保持提交时的文本表示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEpoch {
    pub epoch: Civil,
    pub epoch_flag: String,
    pub sat_lines: Vec<RawSatLine>,
    /// 历元头接收机钟偏移原文（RINEX 字段；缺测为 None）
    pub clock_offset_raw: Option<String>,
    pub clock_offset_s: Option<f64>,
    /// 该历元头原文行（溯源用）
    pub header_line: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RinexObsFile {
    pub format: String,
    pub marker_name: String,
    pub obs_codes: Vec<String>,
    pub header_lines: Vec<String>,
    pub epochs: Vec<RawEpoch>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 卫星状态历元（位置/钟差）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatStateEpoch {
    pub epoch: Civil,
    pub sat: String,
    /// ECEF X/Y/Z，米
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
    /// 卫星钟差，秒
    pub clock_s: f64,
    pub xyz_datum: String,
    pub source_line: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SatStateFile {
    pub version: String,
    pub xyz_datum: String,
    pub states: Vec<SatStateEpoch>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 一个版本的完整原始输入（不可变）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceVersionData {
    pub label: String,
    pub rinex: RinexObsFile,
    pub sat_states: SatStateFile,
}

/// 派生结果的溯源：算法版本、参数指纹、直接来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub algorithm_version: String,
    pub params_fingerprint: String,
    pub direct_sources: Vec<String>,
}
