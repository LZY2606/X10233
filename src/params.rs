//! 检测器参数集：每次修订产生新的算法版本与参数指纹。

use serde::{Deserialize, Serialize};

/// 周跳检测器的可调参数（固定默认值即"基线规则"）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectorParams {
    pub algo_version: String,
    /// 时间差分相位残差判为候选的阈值（周）。
    pub td_threshold_cycles: f64,
    /// 电离层组合历元差阈值（L1 周）。
    pub gf_threshold_cycles: f64,
    /// Melbourne-Wubbena 宽巷模糊度跳变阈值（宽巷周）。
    pub mw_threshold_cycles: f64,
    /// LLI 置位仍要求检测器交叉验证（LLI 可误报）。
    pub lli_requires_corroboration: bool,
    /// 整数搜索半径（周）。
    pub search_radius_cycles: i64,
    /// 不等间隔：超过该秒数视为弧段长间断（不等间隔历元）。
    pub max_gap_seconds: f64,
    /// MW 滑动平均的最小历史历元数。
    pub mw_min_history: usize,
    /// 多少个站-星信号同时出现一致跳变才归为整机钟跳。
    pub clock_consensus_min_signals: usize,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            algo_version: "rule-set-v1",
            td_threshold_cycles: 0.75,
            gf_threshold_cycles: 0.75,
            mw_threshold_cycles: 0.75,
            lli_requires_corroboration: true,
            search_radius_cycles: 20,
            max_gap_seconds: 120.0,
            mw_min_history: 3,
            clock_consensus_min_signals: 3,
        }
    }
}

impl DetectorParams {
    /// 参数指纹：由算法版本与全部参数值确定性生成。
    pub fn fingerprint(&self) -> String {
        let material = serde_json::to_string(self).expect("params serialize");
        crate::ids::content_id("par", &material)
    }
}
