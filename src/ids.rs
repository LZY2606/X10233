//! Stable identities.
//!
//! Natural identities are RFC 9562 version-5 (SHA-1) UUIDs computed from a
//! fixed project namespace and a domain natural key.  Recomputing the same
//! key after export/delete/import yields the same identifier, while unrelated
//! inputs cannot collide into the same arc or candidate.

use sha1::{Digest, Sha1};

fn namespace() -> [u8; 16] {
    let text = "7f1c9a54-3b2e-4d67-91b4-2e8c5a6d70f1";
    let mut out = [0u8; 16];
    let hex: Vec<u8> = text.bytes().filter(|b| *b != b'-').collect();
    for i in 0..16 {
        let hi = hex_val(hex[i * 2]);
        let lo = hex_val(hex[i * 2 + 1]);
        out[i] = (hi << 4) | lo;
    }
    out
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// RFC 4122/RFC 9562 name-based SHA-1 UUID, returned as a hyphenated string.
pub fn stable_uuid_v5(name: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(namespace());
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid(bytes)
}

fn format_uuid(b: [u8; 16]) -> String {
    let mut s = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

pub fn join(parts: &[&str]) -> String {
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        out.push_str(&escape(part));
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('|', "\\p").replace('\n', "\\n")
}

pub fn measurement_natural(
    project: &str,
    station: &str,
    sat: &str,
    gps_sec: i64,
    code: &str,
) -> String {
    let key = join(&["meas", project, station, sat, &gps_sec.to_string(), code]);
    stable_uuid_v5(&key)
}

pub fn candidate_natural(
    project: &str,
    station: &str,
    sat: &str,
    gps_sec: i64,
    detector: &str,
    algo_version: &str,
    param_fingerprint: &str,
    detail: &str,
) -> String {
    let key = join(&[
        "candidate",
        project,
        station,
        sat,
        &gps_sec.to_string(),
        detector,
        algo_version,
        param_fingerprint,
        detail,
    ]);
    stable_uuid_v5(&key)
}

pub fn arc_natural(project: &str, station: &str, sat: &str, freq: &str, ordinal: i64) -> String {
    stable_uuid_v5(&join(&[
        "arc",
        project,
        station,
        sat,
        freq,
        &ordinal.to_string(),
    ]))
}

pub fn decision_natural(
    project: &str,
    candidate: &str,
    action: &str,
    parent_event: &str,
) -> String {
    stable_uuid_v5(&join(&["decision", project, candidate, action, parent_event]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v5_is_deterministic_and_versioned() {
        let a = stable_uuid_v5("hello");
        let b = stable_uuid_v5("hello");
        assert_eq!(a, b);
        assert_eq!(&a[14..15], "5");
        assert_eq!(a.len(), 36);
        assert_ne!(stable_uuid_v5("a"), stable_uuid_v5("b"));
    }
}
