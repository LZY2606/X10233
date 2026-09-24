use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 算法版本：任何派生值都记录它。
pub const ALGO_VERSION: &str = "det-v1";
pub const C_LIGHT_MPS: f64 = 299_792_458.0;
pub const F1_HZ: f64 = 1_575_420_000.0;
pub const F2_HZ: f64 = 1_227_600_000.0;

pub fn band_freq_hz(band: usize) -> Option<f64> {
    match band {
        0 => Some(F1_HZ),
        1 => Some(F2_HZ),
        _ => None,
    }
}
pub fn band_name(band: usize) -> &'static str {
    match band {
        0 => "L1",
        1 => "L2",
        _ => "?",
    }
}
pub fn wavelength_m(band: usize) -> f64 {
    C_LIGHT_MPS / band_freq_hz(band).unwrap()
}
pub fn widelane_lambda_m() -> f64 {
    C_LIGHT_MPS / (F1_HZ - F2_HZ)
}

/// 观测码 -> 频段索引（L1=0, L2=1）。其它频段原样保存但不参与双频组合。
pub fn code_band(code: &str) -> Option<usize> {
    let b = code.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() {
        match b[1] {
            b'1' => Some(0),
            b'2' => Some(1),
            _ => None,
        }
    } else {
        None
    }
}
pub fn code_unit(code: &str) -> &'static str {
    match code.as_bytes().first() {
        Some(b'C') => "m",
        Some(b'L') => "cycles",
        Some(b'D') => "Hz",
        Some(b'S') => "dBHz",
        _ => "",
    }
}

/// 历元键：字典序即时间序（GPS 周.周内毫秒，零填充）。
pub fn epoch_key(week: i32, tow_ms: i64) -> String {
    format!("{:04}.{:010}", week, tow_ms)
}
pub fn parse_epoch_key(key: &str) -> Option<(i32, i64)> {
    let (w, t) = key.split_once('.')?;
    Some((w.parse().ok()?, t.parse().ok()?))
}

/// 单条观测量：原始表示原样保存；解析失败时值留空并附诊断，不丢弃。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObsValue {
    pub code: String,
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lli: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssi: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diag: Option<String>,
}

impl ObsValue {
    pub fn parse(code: &str, raw: &str, lli: Option<u8>, ssi: Option<u8>) -> ObsValue {
        let trimmed = raw.trim();
        let (value, diag) = if trimmed.is_empty() {
            (None, Some("empty raw field".to_string()))
        } else {
            match trimmed.parse::<f64>() {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(format!("unparseable value kept in place: {}", e))),
            }
        };
        ObsValue {
            code: code.to_string(),
            raw: raw.to_string(),
            value,
            unit: code_unit(code).to_string(),
            lli,
            ssi,
            diag,
        }
    }
}

/// 导入文档（JSON 形态；文本形态由 rinex 模块转成此结构）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsIn {
    pub code: String,
    pub raw: String,
    #[serde(default)]
    pub lli: Option<u8>,
    #[serde(default)]
    pub ssi: Option<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SatIn {
    pub sat: String,
    #[serde(default)]
    pub freq_num: Option<i32>,
    #[serde(default)]
    pub obs: Vec<ObsIn>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochIn {
    pub week: i32,
    pub tow_ms: i64,
    #[serde(default)]
    pub clock_event: Option<String>,
    #[serde(default)]
    pub sats: Vec<SatIn>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsDoc {
    pub station: String,
    #[serde(default)]
    pub epochs: Vec<EpochIn>,
}

/// 检测参数（版本化，指纹参与派生值身份）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Params {
    pub td_threshold_cycles: f64,
    pub gf_threshold_cycles: f64,
    pub mw_threshold_wl: f64,
    pub clock_threshold_m: f64,
    pub clock_min_sats: usize,
    pub max_gap_s: f64,
    pub max_fix: i32,
}
impl Default for Params {
    fn default() -> Self {
        Params {
            td_threshold_cycles: 0.5,
            gf_threshold_cycles: 0.5,
            mw_threshold_wl: 0.5,
            clock_threshold_m: 1000.0,
            clock_min_sats: 3,
            max_gap_s: 120.0,
            max_fix: 20,
        }
    }
}
impl Params {
    pub fn fingerprint(&self) -> String {
        crate::ids::cid("par", &[&serde_json::to_string(self).unwrap_or_default()])
    }
}

// ---------- 持久化行 ----------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub branch: String,
    pub kind: String,
    pub label: String,
    pub payload: Value,
    pub vtime: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub branch_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub fork_seq: u64,
    pub created_seq: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputVersion {
    pub version_id: String,
    pub parent: Option<String>,
    pub label: String,
    pub kind: String, // initial | patch
    pub touches: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochHeaderRow {
    pub version_id: String,
    pub epoch_key: String,
    pub week: i32,
    pub tow_ms: i64,
    pub station: String,
    pub clock_event: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochObsRow {
    pub version_id: String,
    pub epoch_key: String,
    pub sat: String,
    pub freq_num: Option<i32>,
    pub obs: Vec<ObsValue>, // 空 Vec = 墓碑（该版本删除此历元此星）
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamsRow {
    pub param_fp: String,
    pub params: Params,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DerivedRow {
    pub row_id: String,
    pub station: String,
    pub sat: String,
    pub arc_id: String,
    pub kind: String, // candidate | residuals | boundaries | clock
    pub detector: String,
    pub algo_version: String,
    pub param_fp: String,
    pub src_map: Vec<(String, String)>, // (station|sat, version_id) 直接来源
    pub status: String,                 // proposed|ambiguous|rejected|unknown|ok|confirmed
    pub payload: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Presence {
    pub row_id: String,
    pub branch: String,
    pub seq: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub decision_id: String,
    pub branch: String,
    pub seq: u64,
    pub action: String, // accept | reject
    pub candidate_id: String,
    pub arc_id: String,
    pub epoch_key: String,
    pub reviewer: String,
    pub reason: String,
    pub payload: Value,
}
/// 修复只存偏移层：永不改写原始观测量。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offset {
    pub offset_id: String,
    pub branch: String,
    pub seq: u64,
    pub arc_id: String,
    pub from_epoch_key: String,
    pub band: String,
    pub cycles: f64,
    pub decision_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Db {
    pub events: Vec<Event>,
    pub branches: Vec<Branch>,
    pub versions: Vec<InputVersion>,
    pub epoch_headers: Vec<EpochHeaderRow>,
    pub epoch_obs: Vec<EpochObsRow>,
    pub params: Vec<ParamsRow>,
    pub derived: Vec<DerivedRow>,
    pub presence: Vec<Presence>,
    pub decisions: Vec<Decision>,
    pub offsets: Vec<Offset>,
}

impl Db {
    /// 导出前确定性排序：保证导出/导入/再导出字节一致。
    pub fn sort_canonical(&mut self) {
        self.events.sort_by_key(|e| e.seq);
        self.branches.sort_by(|a, b| a.branch_id.cmp(&b.branch_id));
        self.versions.sort_by(|a, b| a.version_id.cmp(&b.version_id));
        self.epoch_headers
            .sort_by(|a, b| (&a.version_id, &a.epoch_key).cmp(&(&b.version_id, &b.epoch_key)));
        self.epoch_obs.sort_by(|a, b| {
            (&a.version_id, &a.epoch_key, &a.sat).cmp(&(&b.version_id, &b.epoch_key, &b.sat))
        });
        self.params.sort_by(|a, b| a.param_fp.cmp(&b.param_fp));
        self.derived.sort_by(|a, b| a.row_id.cmp(&b.row_id));
        self.presence.sort_by(|a, b| (&a.row_id, &a.branch).cmp(&(&b.row_id, &b.branch)));
        self.decisions.sort_by(|a, b| a.decision_id.cmp(&b.decision_id));
        self.offsets.sort_by(|a, b| a.offset_id.cmp(&b.offset_id));
    }
}
