//! 多检测器数学。任何一个检测器都不是真值：各给候选与置信度，由综合层与人工裁定。
//!
//! 约定（案例包与 UI 中均显式标注）：
//! - 伪距 P 单位 m；载波 L 单位 cycles；Doppler D 单位 Hz（正号对应距离增加的频移约定，
//!   样例按 d(phase)/dt = -Doppler 构造，符号约定写进算法版本与导出文档）。

use crate::identity::q9;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ALGO_VERSION: &str = "detect-1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameters {
    /// 弧段内最大采样间隔（含），超过则开新弧段（s）
    pub max_gap_s: f64,
    /// GF/时间差分阈值（周）
    pub gf_sigma: f64,
    pub td_sigma: f64,
    /// MW 平滑阈值（宽巷周）
    pub mw_sigma: f64,
    /// 候选保留的最大 |n|
    pub candidate_radius: i64,
    /// 至少需要几个历元才做平滑
    pub mw_min_epochs: usize,
}

impl Default for Parameters {
    fn default() -> Self {
        Parameters {
            max_gap_s: 30.0,
            gf_sigma: 0.10,
            td_sigma: 0.05,
            mw_sigma: 0.25,
            candidate_radius: 3,
            mw_min_epochs: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandInfo {
    pub band: String,
    pub wavelength_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EpochSample {
    pub epoch: f64,
    pub p: Option<f64>,
    pub l: Option<f64>,
    pub doppler: Option<f64>,
    pub lli: Option<i64>,
}

/// 检测器在一条边界上的原始输出。残差单位统一为“周”（对应波段见 vector 字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorResult {
    pub detector: String,
    pub status: String, // ok | masked_clock | insufficient | missing_frequency
    pub residual: Option<f64>,
    pub sigma: f64,
    pub unit: String,
    pub detail: serde_json::Value,
    /// 检测器自身枚举的合法整周候选（波段 -> 整数）
    pub native_candidates: Vec<NativeCandidate>,
    pub reject_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeCandidate {
    pub vector: BTreeMap<String, i64>,
    pub score: f64,
}

pub fn candidates_around(residual: f64, sigma: f64, radius: i64, band: &str) -> Vec<NativeCandidate> {
    let nearest = residual.round() as i64;
    let mut out = Vec::new();
    for n in (nearest - radius)..=(nearest + radius) {
        let dist = (residual - n as f64).abs();
        if dist <= 3.0 * sigma {
            out.push(NativeCandidate {
                vector: BTreeMap::from([(band.to_string(), n)]),
                score: q9(dist / sigma),
            });
        }
    }
    out.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 时间差分相位：(L_b - L_a) - Doppler 积分周数。返回残差（周）。
pub fn td_residual(prev: &EpochSample, cur: &EpochSample) -> Option<f64> {
    let dl = q9(cur.l? - prev.l?);
    let d = prev.doppler.or(cur.doppler)?;
    let dt = cur.epoch - prev.epoch;
    if dt <= 0.0 {
        return None;
    }
    // 约定：dL/dt = -Doppler（Hz=s^-1，乘以秒即周）
    Some(q9(dl + d * dt))
}

/// 电离层无关之外的几何无关（GF）组合：L1*lam1 - L2*lam2（米），再换算到 band1 周。
/// 时间差分后几何项抵消，剩余主要是电离层变化 + 周跳。
pub fn gf_residual_cycles(
    prev1: Option<f64>, cur1: Option<f64>,
    prev2: Option<f64>, cur2: Option<f64>,
    b1: &BandInfo, b2: &BandInfo,
) -> Option<f64> {
    let d_meters = ((cur1? - prev1?) * b1.wavelength_m) - ((cur2? - prev2?) * b2.wavelength_m);
    Some(q9(d_meters / b1.wavelength_m))
}

/// Melbourne-Wubbena 宽巷组合（周）。
/// N_w = (f1 L1 - f2 L2)/(f1-f2) - (f1 P1 + f2 P2)/((f1+f2) c) * (f1-f2)
/// 等价常用形式：N_w = (L1-L2) - (P1/lam_w) * f1/(f1+f2) ... 这里采用显式频率形式。
pub fn mw_widelane_cycles(
    l1: f64, l2: f64, p1: f64, p2: f64, b1: &BandInfo, b2: &BandInfo,
) -> f64 {
    let f1 = crate::engine::C / b1.wavelength_m;
    let f2 = crate::engine::C / b2.wavelength_m;
    let wide_phase = (f1 * l1 - f2 * l2) / (f1 - f2);
    let wide_code = ((f1 * p1 + f2 * p2) / (f1 + f2)) * (f1 - f2) / crate::engine::C;
    q9(wide_phase - wide_code)
}
