//! 稳定身份：所有身份都是内容派生的 SHA-256 截断，跨导出/重导入保持不变。
//! 身份里绝不使用自增 rowid 或 wall-clock。

use sha2::{Digest, Sha256};

pub fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0x1f]); // 单元分隔，避免拼接歧义
        }
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    let mut s = String::with_capacity(16);
    for b in d.iter().take(8) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

pub fn id(prefix: &str, parts: &[&str]) -> String {
    format!("{}_{}", prefix, digest(parts))
}

pub fn project_id(seed: &str) -> String {
    id("prj", &["gsb", seed])
}
pub fn epoch_id(project: &str, tag: &str) -> String {
    id("ep", &[project, tag])
}
pub fn obs_id(project: &str, raw_line: &str) -> String {
    id("ob", &[project, raw_line])
}
pub fn clock_id(project: &str, raw_line: &str) -> String {
    id("ck", &[project, raw_line])
}
pub fn source_version_id(project: &str, parent: Option<&str>, content_hash: &str) -> String {
    id("src", &[project, parent.unwrap_or("ROOT"), content_hash])
}
pub fn params_id(project: &str, parent: Option<&str>, fingerprint: &str) -> String {
    id("prm", &[project, parent.unwrap_or("ROOT"), fingerprint])
}
pub fn event_id(project: &str, parent: Option<&str>, kind: &str, payload_canonical: &str) -> String {
    id("evt", &[project, parent.unwrap_or("ROOT"), kind, payload_canonical])
}
pub fn node_id(kind: &str, project: &str, algo: &str, params_fp: &str, deps_key: &str) -> String {
    id(kind, &[project, algo, params_fp, deps_key])
}
pub fn candidate_id(boundary: &str, detector: &str, target: &str, offset: &str) -> String {
    id("cnd", &[boundary, detector, target, offset])
}
pub fn boundary_id(project: &str, left_epoch: &str, right_epoch: &str) -> String {
    id("bnd", &[project, left_epoch, right_epoch])
}
