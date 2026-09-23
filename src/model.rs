use serde::{Deserialize, Serialize};

pub const C_MPS: f64 = 299_792_458.0;
pub const F1_HZ: f64 = 1575.42e6;
pub const F2_HZ: f64 = 1227.60e6;
pub const LAM1_M: f64 = C_MPS / F1_HZ;
pub const LAM2_M: f64 = C_MPS / F2_HZ;
pub const LAM_W_M: f64 = C_MPS / (F1_HZ - F2_HZ);

pub const ALGO_ARCSEG: &str = "arcseg/1";
pub const ALGO_DETECT: &str = "slipdetect/1";

/// 检测参数（可修订，修订产生新版本与父版本关系）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    /// 弧段断裂阈值：相邻历元间隔超过该秒数则开新弧段
    pub max_gap_s: f64,
    /// 时间差分检测阈值（周）
    pub td_threshold_cycles: f64,
    /// 电离层（几何自由）组合阈值（米）
    pub gf_threshold_m: f64,
    /// Melbourne-Wubbena 宽巷阈值（周）
    pub mw_threshold_cycles: f64,
    /// 判定整机钟跳所需的最少卫星数
    pub clock_min_sats: i64,
    /// 各星钟跳量一致性容差（米）
    pub clock_tolerance_m: f64,
    /// 最小钟跳量（米），低于此值不判为钟跳
    pub clock_min_jump_m: f64,
    /// 等分整周偏移区间：小数部分落在 [lo, hi] 内则保留全部合法候选
    pub ambig_lo: f64,
    pub ambig_hi: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            max_gap_s: 2.0,
            td_threshold_cycles: 0.5,
            gf_threshold_m: 0.2,
            mw_threshold_cycles: 0.5,
            clock_min_sats: 3,
            clock_tolerance_m: 30_000.0,
            clock_min_jump_m: 1_000.0,
            ambig_lo: 0.35,
            ambig_hi: 0.65,
        }
    }
}

impl Params {
    /// 参数指纹：对规范化 JSON 做 FNV-1a，随任何字段变化而变化。
    pub fn fingerprint(&self) -> String {
        let canon = serde_json::to_string(self).unwrap_or_default();
        fnv1a_hex(canon.as_bytes())
    }

    /// 用 JSON patch（扁平键值）生成新参数；未知键返回错误。
    pub fn patched(&self, patch: &serde_json::Value) -> Result<Self, String> {
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        let obj = v.as_object_mut().ok_or("params not object")?;
        let p = patch.as_object().ok_or("patch must be a JSON object")?;
        for (k, val) in p {
            if !obj.contains_key(k) {
                return Err(format!("未知参数键: {k}"));
            }
            obj.insert(k.clone(), val.clone());
        }
        serde_json::from_value(v).map_err(|e| format!("参数类型错误: {e}"))
    }
}

pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// 观测值类别（由观测码首字母派生）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObsKind {
    Pseudorange,
    Carrier,
    Doppler,
    Snr,
    Other,
}

impl ObsKind {
    pub fn from_code(code: &str) -> Self {
        match code.chars().next() {
            Some('C') | Some('P') => ObsKind::Pseudorange,
            Some('L') => ObsKind::Carrier,
            Some('D') => ObsKind::Doppler,
            Some('S') => ObsKind::Snr,
            _ => ObsKind::Other,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ObsKind::Pseudorange => "pseudorange",
            ObsKind::Carrier => "carrier",
            ObsKind::Doppler => "doppler",
            ObsKind::Snr => "snr",
            ObsKind::Other => "other",
        }
    }
}

/// 站-星-频率键，弧段与局部过期的最小作用域。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub station: String,
    pub sat: String,
    pub freq_no: i64,
}

impl Key {
    pub fn label(&self) -> String {
        format!("{}/{}/F{}", self.station, self.sat, self.freq_no)
    }
}
