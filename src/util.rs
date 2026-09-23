/// Deterministic FNV-1a so identities are stable across machines, runs and
/// export/import cycles.
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn hid(prefix: &str, material: &str) -> String {
    format!("{}{:016x}", prefix, fnv1a64(material.as_bytes()))
}

/// Deterministic xorshift64* RNG used wherever a tie must be broken with a
/// fixed seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next_u64() as usize) % (i + 1);
            items.swap(i, j);
        }
    }
}

pub fn canonical_json(v: &serde_json::Value) -> String {
    let mut s = Vec::new();
    canon_into(v, &mut s);
    String::from_utf8(s).unwrap()
}

fn canon_into(v: &serde_json::Value, out: &mut Vec<u8>) {
    use serde_json::Value;
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => {
            out.push(b'"');
            out.extend(serde_json::to_vec(s).unwrap());
        }
        Value::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 { out.push(b','); }
                canon_into(x, out);
            }
            out.push(b']');
        }
        Value::Object(o) => {
            let mut keys: Vec<_> = o.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 { out.push(b','); }
                out.extend(serde_json::to_vec(k).unwrap());
                out.push(b':');
                canon_into(&o[*k], out);
            }
            out.push(b'}');
        }
    }
}
