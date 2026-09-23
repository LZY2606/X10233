//! 领域模型：GNSS 载波相位周跳审阅。
//! 所有领域量携带单位；原始层按历元原样保存，派生层带算法版本与参数指纹。
use serde::{Deserialize, Serialize};

pub const ALGO_VERSION: &str = "derive/1";
pub const ARCHIVE_FORMAT: &str = "breakpoint-bench-archive/1";

/// GPS 载波频率 (Hz)
pub const F1: f64 = 1575.42e6;
pub const F2: f64 = 1227.60e6;
pub const C_LIGHT: f64 = 299792458.0;

pub fn wavelength(freq: i64) -> Option<f64> {
    match freq {
        1 => Some(C_LIGHT / F1),
        2 => Some(C_LIGHT / F2),
        _ => None,
    }
}

/// 观测码 -> (频率编号, 单位)。未知码返回 None（导入时保留原样并附诊断）。
pub fn code_meta(code: &str) -> Option<(i64, &'static str)> {
    let b = code.as_bytes();
    if b.len() != 3 {
        return None;
    }
    let freq = match b[1] {
        b'1' => 1,
        b'2' => 2,
        _ => return None,
    };
    let unit = match b[0] {
        b'C' => "m",
        b'L' => "cyc",
        b'D' => "Hz",
        b'S' => "dB-Hz",
        _ => return None,
    };
    Some((freq, unit))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// 弧段断裂阈值：相邻历元间隔超过该秒数则开新弧段
    pub gap_s: f64,
    /// 时间差分周跳阈值 (周)
    pub td_threshold_cycles: f64,
    /// 电离层(几何无关)组合阈值 (周)
    pub gf_threshold_cycles: f64,
    /// Melbourne-Wubbena 组合阈值 (宽巷周)
    pub mw_threshold_cycles: f64,
    /// 整机钟跳判定阈值 (周, L1)
    pub clock_threshold_cycles: f64,
    /// 判定整机钟跳所需的最少卫星数
    pub clock_min_sats: i64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            gap_s: 60.0,
            td_threshold_cycles: 1.0,
            gf_threshold_cycles: 1.0,
            mw_threshold_cycles: 1.0,
            clock_threshold_cycles: 1000.0,
            clock_min_sats: 3,
        }
    }
}

impl Params {
    /// 参数指纹：规范化 JSON 的 FNV-1a 哈希，随参数一并存储。
    pub fn fingerprint(&self) -> String {
        let canon = format!(
            "gap_s={};td={};gf={};mw={};clk={};clkn={}",
            self.gap_s,
            self.td_threshold_cycles,
            self.gf_threshold_cycles,
            self.mw_threshold_cycles,
            self.clock_threshold_cycles,
            self.clock_min_sats
        );
        fnv_hex(canon.as_bytes())
    }
}

pub fn fnv_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// 候选状态机：proposed -> accepted / rejected；分歧决定 -> conflicted；resolve 落定。
pub fn fold_status(decisions: &[(&str, &str)]) -> String {
    // decisions: (verdict, chosen_for_resolve) 按 seq 升序
    let mut status = "proposed".to_string();
    for (verdict, chosen) in decisions {
        match *verdict {
            "accept" => {
                if status == "proposed" {
                    status = "accepted".into();
                } else if status != "accepted" {
                    status = "conflicted".into();
                }
            }
            "reject" => {
                if status == "proposed" {
                    status = "rejected".into();
                } else if status != "rejected" {
                    status = "conflicted".into();
                }
            }
            "resolve" => {
                status = chosen.to_string();
            }
            _ => {}
        }
    }
    status
}
