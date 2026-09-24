//! Stable content identities.
//!
//! Every domain entity carries a natural, deterministic identity that does
//! not depend on insertion order or a database rowid. Derived identities fold
//! in the algorithm version and the direct-source identities, so changing one
//! source yields a *new* identity while unrelated results keep theirs.

use sha2::{Digest, Sha256};

/// 128-bit (truncated SHA-256) deterministic hex id with a domain prefix.
pub fn stable_id(prefix: &str, material: &str) -> String {
    let mut h = Sha256::new();
    h.update(prefix.as_bytes());
    h.update(b"\x00");
    h.update(material.as_bytes());
    let d = h.finalize();
    format!("{prefix}_{}", hex16(&d[..16]))
}

/// Full SHA-256 hex used for bundle integrity.
pub fn sha256_hex(material: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(material);
    let d = h.finalize();
    hex16(&d)
}

fn hex16(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for x in b {
        out.push(H[(x >> 4) as usize] as char);
        out.push(H[(x & 0x0f) as usize] as char);
    }
    out
}

pub fn batch_id(project: &str, seq: i64, raw_sha: &str) -> String {
    stable_id("in", &format!("{project}|{seq}|{raw_sha}"))
}

pub fn source_patch_id(project: &str, seq: i64, kind: &str, key: &str, raw_sha: &str) -> String {
    stable_id("src", &format!("{project}|{seq}|{kind}|{key}|{raw_sha}"))
}

pub fn param_version_id(project: &str, fingerprint: &str, seq: i64) -> String {
    stable_id("pf", &format!("{project}|{fingerprint}|{seq}"))
}

pub fn event_id(prev: &str, kind: &str, payload: &str, clock_ms: i64) -> String {
    stable_id("ev", &format!("{prev}|{kind}|{payload}|{clock_ms}"))
}

pub fn obs_natural(batch_id: &str, epoch: &str, sat: &str, code: &str) -> String {
    format!("{batch_id}@{epoch}/{sat}/{code}")
}

pub fn merge_obs_id(project: &str, epoch: &str, sat: &str, code: &str) -> String {
    stable_id("obs", &format!("{project}|{epoch}|{sat}|{code}"))
}

pub fn clock_natural(batch_id: &str, epoch: &str) -> String {
    format!("{batch_id}@{epoch}/#clock")
}

pub fn merge_clock_id(project: &str, epoch: &str) -> String {
    stable_id("clk", &format!("{project}|{epoch}"))
}

pub fn state_natural(batch_id: &str, epoch: &str, sat: &str) -> String {
    format!("{batch_id}@{epoch}/{sat}/#state")
}

pub fn merge_state_id(project: &str, epoch: &str, sat: &str) -> String {
    stable_id("sat", &format!("{project}|{epoch}|{sat}"))
}

/// Identity of a derived edge-level statistic. Folds in:
/// algorithm version, parameter fingerprint, sat/bands, and the two direct
/// merged-observation identities (so a local source change changes only it).
pub fn statistic_id(
    algo: &str,
    alg_ver: &str,
    param_fp: &str,
    sat: &str,
    bands: &str,
    sources: &[String],
) -> String {
    let mut m = String::new();
    m.push_str(alg_ver);
    m.push('|');
    m.push_str(param_fp);
    m.push('|');
    m.push_str(sat);
    m.push('|');
    m.push_str(bands);
    for s in sources {
        m.push('|');
        m.push_str(s);
    }
    stable_id(&format!("stat-{algo}"), &m)
}

pub fn candidate_id(
    detector: &str,
    sat: &str,
    band: &str,
    at_epoch: &str,
    offset_cycles: &str,
    sources: &[String],
) -> String {
    let mut m = format!("{detector}|{sat}|{band}|{at_epoch}|{offset_cycles}");
    for s in sources {
        m.push('|');
        m.push_str(s);
    }
    stable_id("cand", &m)
}

pub fn arc_id(sat: &str, band: &str, start_epoch: &str, start_seq: i64) -> String {
    stable_id("arc", &format!("{sat}|{band}|{start_epoch}|{start_seq}"))
}

pub fn offset_id(sat: &str, band: &str, at_epoch: &str, cycles: f64, decision_id: &str) -> String {
    stable_id(
        "off",
        &format!("{sat}|{band}|{at_epoch}|{cycles:.6}|{decision_id}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_distinct() {
        let a = stable_id("x", "hello");
        let b = stable_id("x", "hello");
        let c = stable_id("x", "hellp");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("x_"));
        assert_eq!(sha256_hex(b"").len(), 64);
    }
}
