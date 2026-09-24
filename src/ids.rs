use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 稳定身份：由内容确定性推导，导出/删除/再导入后保持一致。
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    let digest = hex(&h.finalize());
    format!("{}_{}", prefix, &digest[..16])
}

pub fn fmt_tow(tow: f64) -> String {
    format!("{:.3}", tow)
}
