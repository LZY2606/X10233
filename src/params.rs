//! 检测器参数：参数以版本存在，带指纹；派生值记录所依据的算法版本与参数指纹。

use crate::ids::canonical_json;
use serde::{Deserialize, Serialize};

pub const ALGORITHM_VERSION: &str = "cb-core-1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectorParams {
    /// 历元间隔的标称值（秒）；与实际间隔偏差超阈值即标记“不等间隔”。
    pub nominal_interval_s: f64,
    pub interval_tolerance_s: f64,
    /// 电离层组合 GF 跳变阈值（米）。
    pub gf_sigma_m: f64,
    /// MW 宽巷周数跳变阈值（周）。
    pub mw_sigma_cycles: f64,
    /// 时间差分（码约束）残差阈值（米）。
    pub td_sigma_m: f64,
    /// Doppler 预测不一致阈值（米）。
    pub doppler_sigma_m: f64,
    /// 判为“整机钟跳”所需的同向跳变卫星占比。
    pub clock_satellite_fraction: f64,
    /// 钟跳最小幅度（米）。
    pub clock_min_m: f64,
    /// LLI 若与任何检测器不一致，则视为误报的残差阈值（米）。
    pub lli_false_alarm_m: f64,
    /// 等分整周偏移族枚举的最大 |k|。
    pub family_max_k: i64,
    /// GLO 频分通道号（卫星 -> k），RINEX 导入可覆盖。
    pub glo_frequency_number: std::collections::BTreeMap<String, i32>,
}

impl Default for DetectorParams {
    fn default() -> Self {
        let mut glo = std::collections::BTreeMap::new();
        glo.insert("R07".to_string(), 2);
        Self {
            nominal_interval_s: 30.0,
            interval_tolerance_s: 0.5,
            gf_sigma_m: 0.05,
            mw_sigma_cycles: 0.5,
            td_sigma_m: 0.6,
            doppler_sigma_m: 0.25,
            clock_satellite_fraction: 0.8,
            clock_min_m: 5.0,
            lli_false_alarm_m: 0.15,
            family_max_k: 2,
            glo_frequency_number: glo,
        }
    }
}

impl DetectorParams {
    pub fn fingerprint(&self) -> String {
        let v = serde_json::to_value(self).expect("params serialize");
        crate::ids::content_id("params", &canonical_json(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_with_revision() {
        let p1 = DetectorParams::default();
        let mut p2 = p1.clone();
        assert_eq!(p1.fingerprint(), p2.fingerprint());
        p2.gf_sigma_m = 0.08;
        assert_ne!(p1.fingerprint(), p2.fingerprint());
    }
}
