//! 稳定身份：全部身份由规范化 JSON 内容的 SHA-256 截断派生，
//! 因此导出→删除本地库→再导入后身份、顺序和来源关系保持不变。

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 规范化序列化：递归按键排序，使哈希不依赖结构体字段插入顺序。
pub fn canonical_json<T: Serialize>(v: &T) -> String {
    let value = serde_json::to_value(v).expect("serializable");
    canonical_value(value)
}

fn canonical_value(v: serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => canonical_number(n),
        serde_json::Value::String(s) => serde_json::to_string(&s).unwrap(),
        serde_json::Value::Array(a) => {
            let mut out = String::from("[");
            for (i, item) in a.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&canonical_value(item));
            }
            out.push(']');
            out
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = String::from("{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push(':');
                out.push_str(&canonical_value(map.get(*k).unwrap().clone()));
            }
            out.push('}');
            out
        }
    }
}

/// JSON 的 f64 输出会经过 ryu；NaN/Infinity 不应出现在身份载荷中（用 None 表示未知）。
fn canonical_number(n: serde_json::Number) -> String {
    n.to_string()
}

pub fn content_uid(parts: Vec<serde_json::Value>) -> String {
    let mut hasher = Sha256::new();
    for p in &parts {
        hasher.update(canonical_value(p.clone()));
        hasher.update(b"\x1f");
    }
    let digest = hasher.finalize();
    // 16 hex 字符（64 位），对本地离线案例规模足够。
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}
