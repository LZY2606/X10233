//! 不可变事件：原始输入一经提交不可覆盖，一切修订以新版本 + 父版本表达。

use crate::domain::DecisionKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// 初始或追加导入一个不可变批次（OBX 或 SAT）。
    ImportBatch {
        batch_id: String,
        kind: String,
        file_name: String,
        raw_text: String,
    },
    /// 局部源数据修订：新版本指向父批次，只重放依赖片段。
    AmendSource {
        parent_batch_id: String,
        new_batch_id: String,
        kind: String,
        file_name: String,
        raw_text: String,
        /// 被删除的观测键：station|epoch|sat|band|code
        delete_obs: Vec<String>,
        scope_note: String,
    },
    /// 参数修订：生成新算法版本 + 参数指纹。
    ParameterRevision {
        rules_version: String,
        params: crate::params::DetectorParams,
        note: String,
    },
    /// 人工对候选/钟跳/LLI 的裁定；冲突由多个 head 自然分叉表达。
    Decision {
        kind: DecisionKind,
        target_group_id: Option<String>,
        target_candidate_id: Option<String>,
        station: Option<String>,
        epoch_token: Option<String>,
        note: String,
        /// ResolveConflict 使用：选中的规范 head event id。
        chosen_head_event_id: Option<String>,
        /// 采纳的偏移假设（修复只存偏移层，不改原始载波）。
        offset_cycles: BTreeMap<String, i64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub seq: i64,
    pub parent_event_id: Option<String>,
    pub at_ms: u64,
    pub actor: String,
    pub payload: EventPayload,
}

/// 参数指纹：对规范化 JSON 做稳定哈希，同样参数 → 同样指纹。
pub fn fingerprint_canonical(value: &serde_json::Value) -> String {
    let canonical = canonicalize(value);
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

/// 对象键按字典序排序，消除字段顺序影响。
pub fn canonicalize(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let inner: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), canonicalize(v)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            format!("[{}]", arr.iter().map(canonicalize).collect::<Vec<_>>().join(","))
        }
        other => other.to_string(),
    }
}
