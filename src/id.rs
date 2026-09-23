//! 稳定身份: 所有实体 id 均为内容寻址哈希, 不含时间戳/随机数,
//! 因此导出-删除-再导入后身份、顺序与来源关系保持不变。
use serde::Serialize;
use sha2::{Digest, Sha256};

/// serde_json 默认 Map 为 BTreeMap(键有序), 序列化结果即规范化 JSON。
pub fn canonical_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("canonical json serialization")
}

pub fn content_id<T: Serialize>(prefix: &str, value: &T) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(b"|");
    hasher.update(canonical_json(value).as_bytes());
    let digest = hasher.finalize();
    format!("{}_{}", prefix, hex::encode(&digest[..16]))
}
