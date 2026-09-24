//! 领域模型：所有周跳工作流中出现的数据都是一等公民。
//!
//! 约定：凡是物理量都携带单位或坐标基准；原始层保留 RINEX 原样表示，
//! 派生层才规范化到 SI（米 / 周 / 秒 / Hz）。

use serde::{Deserialize, Serialize};

/// 算法版本。任何判定逻辑变化都必须提升该版本，旧上下文将被标记过期。
pub const ALGO_VERSION: &str = "cb-algo-1.0.0";

/// GPS 主控时钟采用的名义频率（Hz）。
pub const F0_HZ: f64 = 1.48e9;
/// 真空中光速（m/s）。
pub const C_MPS: f64 = 2.99792458e8;
/// GPS L1 名义频率（Hz）。
pub const F1_HZ: f64 = 1.57542e9;
/// GPS L2 名义频率（Hz）。
pub const F2_HZ: f64 = 1.22760e9;

/// 带单位标注的标量。规范层使用 SI 单位；`raw_repr` 保留原始字面量。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WithUnit {
    /// 规范化后的 SI 数值；无法转换时为 None，原值保留在 raw_repr。
    pub value: Option<f64>,
    /// 规范化单位，例如 "cycle"、"m"、"m/s"、"Hz"、"s"。
    pub unit: String,
    /// 导入文件中的原始字面表示（按历元原样保存）。
    pub raw_repr: String,
    /// 无法转换时的诊断信息；正常为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

/// 某一频率上的单条观测量（RINEX 3 观测记录三元组 + 多普勒）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreqObs {
    /// 观测码列表，按 RINEX 头文件顺序，例如 ["C1C","L1C","D1C","S1C"]。
    pub codes: Vec<String>,
    /// 伪距（米），原始码观测如 C1C；缺失为 None。
    pub code_m: Option<WithUnit>,
    /// 载波相位（周），如 L1C；缺失为 None。
    pub phase_cy: Option<WithUnit>,
    /// 多普勒（Hz），RINEX 约定正值为趋近（视距减小）；缺失为 None。
    pub doppler_hz: Option<WithUnit>,
    /// 信噪比（dBHz），如 S1C；缺失为 None。
    pub cnr_dbhz: Option<WithUnit>,
    /// 失锁指示符原始字符（LLI 位域原样保存），如 "1"；无标志为 ""。
    pub lli_raw: String,
}

/// 单站单星单历元的观测集合。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    /// 稳定身份：obs:<station>:<sat>:<epoch_gpst_seconds，四舍五入到纳秒>
    pub id: String,
    pub station: String,
    pub sat: String,
    /// GPS 周内秒（秒），由 RINEX 历元时间戳规范化；保留原始行于 raw_repr。
    pub epoch_s: f64,
    pub epoch_raw: String,
    /// 频率编号 -> 观测。键为规范频率编号 "1"/"2"（RINEX 频率号）。
    pub freqs: std::collections::BTreeMap<String, FreqObs>,
}

/// 卫星状态（星历/星位快照，保留坐标基准）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SatState {
    pub id: String,
    pub sat: String,
    pub epoch_s: f64,
    /// ECEF 坐标（米），ITRF 基准。
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
    /// 卫星钟偏差（秒）。
    pub clock_bias_s: f64,
    pub frame: String,
    pub raw_repr: String,
}

/// 钟跳事件类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClockKind {
    /// 接收机整机钟跳：影响该站该历元全部在视卫星。
    Receiver,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClockEvent {
    /// 稳定身份：clk:<station>:<epoch_s ns>:<delta_ns>
    pub id: String,
    pub station: String,
    pub epoch_s: f64,
    /// 名义跳变量（秒），正为钟向前跳。
    pub delta_s: WithUnit,
    pub kind: ClockKind,
    pub raw_repr: String,
}

/// 检测器名称。三种检测器互为佐证，任一都不是真值。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    /// 电离层（无几何）相位组合 I = L1 - L2（周）。
    Ionosphere,
    /// Melbourne-Wubbena 宽巷组合（周）。
    MelbourneWubbena,
    /// 载波时间差分（含多普勒预测）残差（周）。
    TimeDifference,
    /// LLI 失锁标志（原始指示，不代表真实周跳）。
    Lli,
}

/// 候选的当前状态（相对当前上下文）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    /// 当前上下文下的现行候选。
    Current,
    /// 由更早上下文产生；源或参数已变化，仅存档，不能冒充当前结果。
    Stale,
}

/// 候选种类。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// 单星整周偏移假设（修复层，仅偏移，不改原始相位）。
    Slip,
    /// 整机钟跳解释（非单星周跳）。
    ClockJump,
    /// LLI 误报：有失锁标志但各检测器残差未越限。
    LliFalseAlarm,
    /// 资料不足，无法唯一决定（保留未知状态）。
    Unknown,
}

/// 单个检测器给出的证据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Clone)]
pub struct Evidence {
    pub detector: Detector,
    /// 检测量在候选历元处的跳变（检测器自身单位，见 unit）。
    pub jump: f64,
    pub unit: String,
    /// 与该候选整周偏移的拟合残差（同 unit）。
    pub residual: f64,
    /// 是否越过该检测器门限。
    pub over_threshold: bool,
    /// 门限数值（同 unit）。
    pub threshold: f64,
}

/// 一个合法的整周偏移假设（周），修复只写在偏移层。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OffsetHypothesis {
    /// 频率编号 -> 整周数，例如 {"1": 5, "2": 4}。
    pub cycles: std::collections::BTreeMap<String, i64>,
    /// 证据列表（哪些检测器支持/反对）。
    pub evidences: Vec<Evidence>,
    /// 0..=1 的综合得分；等分候选得分相同。
    pub score: f64,
}

/// 周跳/钟跳候选（一等数据：采纳与否都保留）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    /// 稳定身份（算法版本+上下文+站星历元+候选键的哈希）。
    pub id: String,
    pub station: String,
    pub sat: String,
    /// 候选所在弧段边界历元（跳变后的第一个历元）。
    pub epoch_s: f64,
    pub kind: CandidateKind,
    /// 触发该候选的检测器。
    pub triggered_by: Vec<Detector>,
    /// 全部合法整周假设（无法唯一决定时保留多个等分候选）。
    pub hypotheses: Vec<OffsetHypothesis>,
    /// 拒绝原因（未采纳/被否时必须给出；正常为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<String>,
    /// 误差或未知说明（例如缺频、资料不足）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<String>,
    pub status: CandidateStatus,
    /// 产生它的计算上下文 id。
    pub context_id: String,
    /// 直接来源观测 id 列表（血缘）。
    pub source_obs: Vec<String>,
}

/// 弧段：单站单星某频率上的连续载波段。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Arc {
    /// 稳定身份：arc:<ctx>:<station>:<sat>:<freq>:<start_ns>
    pub id: String,
    pub station: String,
    pub sat: String,
    pub freq: String,
    pub start_epoch_s: f64,
    pub end_epoch_s: f64,
    /// 弧段内历元（秒，保持导入顺序）。
    pub epochs: Vec<f64>,
    /// 边界产生原因："start" | "gap" | "lli" | "slip"
    pub boundary: String,
    pub context_id: String,
    pub status: CandidateStatus,
}

/// 多频组合残差序列（供复核界面绘制）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComboSeries {
    pub station: String,
    pub sat: String,
    /// "ionosphere_gf_cycles" | "melbourne_wubbena_cycles" | "td_l1_cycles" | "td_l2_cycles"
    pub combo: String,
    pub unit: String,
    /// (epoch_s, value 或 null)，缺频处为 null 而不是删点。
    pub points: Vec<(f64, Option<f64>)>,
    pub context_id: String,
    pub status: CandidateStatus,
}

/// 计算参数（可修订；指纹决定派生层是否过期）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// 允许的最大历元间隔（秒），超过即断弧（应对不等间隔）。
    pub max_epoch_gap_s: f64,
    /// 电离层组合跳变门限（周）。
    pub gf_threshold_cy: f64,
    /// MW 宽巷跳变门限（周）。
    pub mw_threshold_cy: f64,
    /// 时间差分残差门限（周）。
    pub td_threshold_cy: f64,
    /// 整周候选枚举范围（周）。
    pub search_window_cy: i64,
    /// 接收机钟跳最小量级（微秒）。
    pub clock_jump_min_us: f64,
    /// 钟跳共现卫星占比门限（>= 该比例判定为整机钟跳）。
    pub clock_consensus_ratio: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            max_epoch_gap_s: 35.0,
            gf_threshold_cy: 0.25,
            mw_threshold_cy: 0.5,
            td_threshold_cy: 0.5,
            search_window_cy: 12,
            clock_jump_min_us: 100.0,
            clock_consensus_ratio: 0.8,
        }
    }
}

/// 人工裁定结果。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// 采纳某整周偏移假设（写偏移层）。
    Accept,
    /// 否定候选（拒绝原因留档）。
    Reject,
    /// 参数修订。
    ReviseParams,
    /// 标记无法唯一决定（保留全部合法候选）。
    Defer,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavelengths_match_rinex_nominal() {
        let w1 = C_MPS / F1_HZ;
        let w2 = C_MPS / F2_HZ;
        assert!((w1 - 0.190_293_652).abs() < 1e-6);
        assert!((w2 - 0.244_213_441).abs() < 1e-6);
    }
}
