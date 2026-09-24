pub fn stable_id(parts: &[&str]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58b;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_01b3;
    let mut hash = OFFSET;
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hash ^= 0x1f;
            hash = hash.wrapping_mul(PRIME);
        }
        for byte in part.as_bytes() {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{hash:032x}")
}

pub fn source_hash(text: &str) -> String {
    stable_id(&["source-v1", text])
}
