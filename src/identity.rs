//! 稳定身份：对规范化 JSON 做 SHA-256，取前 16 hex 字符，并加类型前缀。
//! 身份只由内容决定，因此导出→删除→导入后身份、顺序与来源关系保持。
use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn digest16<T: Serialize>(content: &T) -> String {
    let v = serde_json::to_value(content).unwrap_or(serde_json::Value::Null);
    digest16_json(&v)
}

pub fn digest16_json(v: &serde_json::Value) -> String {
    let canonical = canonical_json(v);
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    let out = hex::encode(h.finalize());
    out[..16].to_string()
}

pub fn id(kind: &str, parts: &[serde_json::Value]) -> String {
    let v = serde_json::json!({"k": kind, "p": parts});
    format!("{kind}:{}", digest16_json(&v))
}

/// 递归排序对象键，得到确定性 JSON 文本（无空格）。
pub fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap(),
        serde_json::Value::Array(a) => {
            let mut s = String::from("[");
            for (i, x) in a.iter().enumerate() {
                if i > 0 { s.push(','); }
                s.push_str(&canonical_json(x));
            }
            s.push(']');
            s
        }
        serde_json::Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let mut s = String::from("{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 { s.push(','); }
                s.push_str(&serde_json::to_string(k).unwrap());
                s.push(':');
                s.push_str(&canonical_json(&o[*k]));
            }
            s.push('}');
            s
        }
    }
}
