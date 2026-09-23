//! 检测器参数与规则版本。参数修订 = 新版本 + 指纹。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    pub rules_version: String,
    /// 时间差分残差阈值（cycle），超过才给候选。
    pub td_threshold_cycles: f64,
    /// 几何无关（电离层）组合阈值（m）。
    pub gf_threshold_m: f64,
    /// Melbourne-Wubbena 阈值（wide-lane cycle）。
    pub mw_threshold_cycles: f64,
    /// 整机钟跳公共模式阈值（m，基于所有星残差中位数）。
    pub clock_common_mode_m: f64,
    /// 最大弧段间隔（s）：超过即新弧段（不等间隔历元）。
    pub max_gap_s: f64,
    /// LLI bit 置位但残差低于此倍数阈值时，可判误报。
    pub lli_false_alarm_sigma: f64,
    /// MW 初始化所需历元数。
    pub mw_warmup_epochs: usize,
    /// 整数候选枚举范围（cycle）。
    pub search_radius_cycles: i64,
    /// 频率缺失后恢复是否强制开新弧段。
    pub split_on_frequency_gap: bool,
}

impl Default for DetectorParams {
    fn default() -> Self {
        DetectorParams {
            rules_version: "rules-2026.09.1".to_string(),
            td_threshold_cycles: 0.5,
            gf_threshold_m: 0.05,
            mw_threshold_cycles: 0.5,
            clock_common_mode_m: 50.0,
            max_gap_s: 31.0,
            lli_false_alarm_sigma: 0.25,
            mw_warmup_epochs: 3,
            search_radius_cycles: 12,
            split_on_frequency_gap: true,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let v = serde_json::to_value(self).expect("params serializable");
        crate::events::fingerprint_canonical(&v)
    }
}
