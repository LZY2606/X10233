//! 虚拟时钟：固定起点 + 固定步进，重放结果与墙钟无关。

#[derive(Clone, Debug)]
pub struct VirtualClock {
    micros: i64,
    tick_us: i64,
    pub ticks: u64,
}

/// 固定起点：2023-03-12T01:00:00Z（与内置案例的 GPS 历元一致）。
pub const ANCHOR_MICROS_UTC: i64 = 1_678_592_400_000_000;
/// 固定 seed：任何需要确定次序的抖动都来自它。
pub const FIXED_SEED: u64 = 0x0C_B5_17_2A_12_04_20_23;

impl VirtualClock {
    pub fn new() -> Self {
        VirtualClock { micros: ANCHOR_MICROS_UTC, tick_us: 37_000, ticks: 0 }
    }
    pub fn now_micros(&mut self) -> i64 {
        let t = self.micros;
        self.micros += self.tick_us;
        self.ticks += 1;
        t
    }
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new()
    }
}

/// SplitMix64：固定 seed 的确定性伪随机，仅用于稳定的同分次序键。
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Howard Hinnant days_from_civil，1970-01-01 起算的天数。
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let d = d as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
