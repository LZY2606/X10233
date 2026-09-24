//! “载波断点台：GNSS 周跳候选、多频组合与弧段修订史”离线案例包。
//!
//! 纯确定性生成（无随机数；所谓固定 seed 即案例标识常量），
//! 重放必然经过：不等间隔历元、频率缺失、整机钟跳、单星周跳、
//! LLI 误报、多个等分整周偏移、弧段局部过期。

use crate::domain::{BandDef, FORMAT_VERSION};

pub const CASE_STATION: &str = "ST01";
/// 不等间隔历元（秒偏移）：30 / 30 / 60 / 60 / 60 / 60 / 60。
pub const EPOCHS: [i64; 8] = [0, 30, 60, 120, 180, 240, 300, 360];
pub const SATS: [&str; 4] = ["G01", "G02", "G03", "G04"];
pub const CASE_SEED: i64 = 20260924;

struct BandCtx {
    band: BandDef,
    ambiguity: f64,
}

fn bands_for(sat: &str) -> Vec<BandCtx> {
    let all = BandDef::gps_bands();
    let pick = |num: &str, code: &str, n: f64| BandCtx {
        band: all
            .iter()
            .find(|b| b.freq_num == num && b.code == code)
            .unwrap()
            .clone(),
        ambiguity: n,
    };
    match sat {
        "G01" => vec![pick("L1", "1C", 121000.0), pick("L2", "2W", 94500.0)],
        "G02" => vec![pick("L1", "1C", 122000.0), pick("L2", "2W", 95300.0)],
        "G03" => vec![pick("L1", "1C", 123000.0), pick("L2", "2W", 96100.0)],
        _ => vec![pick("L1", "1C", 124000.0), pick("L2", "2W", 96900.0)],
    }
}

fn sat_phase(si: usize) -> f64 {
    0.7 * si as f64
}

/// 平滑码相位（米）：恒定径向速度 + 确定性微小振荡（不使用随机数）。
fn range_m(sat_idx: usize, t: i64, clock_m: f64) -> f64 {
    20_000_000.0 + 17_000.0 * sat_idx as f64 - 40.0 * t as f64 + clock_m
}

fn code_m(sat_idx: usize, t: i64, clock_m: f64) -> f64 {
    range_m(sat_idx, t, clock_m) + 0.61 + 0.012 * ((t as f64) / 200.0 + sat_phase(sat_idx)).sin()
}

fn smooth_phase_cycles(sat_idx: usize, t: i64, clock_m: f64, wavelength: f64, ambiguity: f64) -> f64 {
    range_m(sat_idx, t, clock_m) / wavelength
        + ambiguity
        + 0.0020 * ((t as f64) / 150.0 + sat_phase(sat_idx)).sin()
}

fn doppler_hz(sat_idx: usize, wavelength: f64) -> f64 {
    -40.0 / wavelength
}

fn epoch_tag(t: i64) -> String {
    format!("+{t}s")
}

/// 生成初始导入版本（v1）。
pub fn generate_v1() -> String {
    let mut out = header();
    for &t in &EPOCHS {
        // 整机钟跳：180s 起所有星的码与载波共同平移 1ms（非整周，属接收机钟）。
        let clock_m = if t >= 180 { 0.001 * crate::domain::C_LIGHT } else { 0.0 };
        for (si, sat) in SATS.iter().enumerate() {
            out.push_str(&format!("SATSTATE {sat} {} OK HEALTHY\n", epoch_tag(t)));
            for ctx in bands_for(sat) {
                // 频率缺失：G01 在 300s 历元完全没有 L2 观测（整行缺测，非 `*`）。
                if *sat == "G01" && ctx.band.freq_num == "L2" && t == 300 {
                    continue;
                }
                let p = code_m(si, t, clock_m);
                let mut phase =
                    smooth_phase_cycles(si, t, clock_m, ctx.band.wavelength_m, ctx.ambiguity);
                let mut lli: i64 = 0;

                if *sat == "G02" && ctx.band.freq_num == "L1" {
                    // LLI 误报：120s 置失锁标志，但载波连续无跳变。
                    if t == 120 {
                        lli = 1;
                    }
                    // 单星周跳：240s L1 +2 周（只动载波，码不变）。
                    if t >= 240 {
                        phase += 2.0;
                    }
                }
                if *sat == "G03" {
                    // 多个等分整周偏移：300s 起 L1/L2 同时注入半周物理偏移，
                    // 使 (0,0) 与 (1,1) 两个整周假设在 MW 下严格等分、GF 下相邻，
                    // 且该星 Doppler 在 240/300 缺测 → 时间差分不参与裁决。
                    if t >= 300 {
                        phase += 0.5;
                    }
                }
                if *sat == "G04" && ctx.band.freq_num == "L1" && t >= 120 {
                    // 软不一致：-40mm 量级 GF 阶跃（约 -0.21 L1 周），检测器可分歧。
                    phase -= 0.21;
                }
                if *sat == "G01" && ctx.band.freq_num == "L1" && t >= 300 {
                    // 频率缺失边界上的真实 +1 周 L1 跳变。
                    phase += 1.0;
                }

                let doppler = if *sat == "G03" && (t == 240 || t == 300) {
                    "*".to_string()
                } else if *sat == "G01" && t == 300 {
                    "*".to_string()
                } else {
                    format!("{:.6}", doppler_hz(si, ctx.band.wavelength_m))
                };
                let snr = 42.0 + 3.0 * ((t as f64) / 90.0 + sat_phase(si)).sin();
                out.push_str(&format!(
                    "OBS {CASE_STATION} {sat} {} {} {} {:.6} {:.6} {} {} {:.2}\n",
                    ctx.band.freq_num,
                    ctx.band.code,
                    epoch_tag(t),
                    p,
                    phase,
                    doppler,
                    lli,
                    snr
                ));
            }
        }
    }
    out.push_str("CLOCK ALL +180s 1.0  # 显式钟跳事件记录（ms），派生层同时独立检测\n");
    out
}

/// 生成源数据修订版本（v2）：只改 G02 L1 在 120s 的 LLI 标志 1→0。
/// 仅 60s→120s、G02 相关弧段依赖该材料，因此只有这些结果局部过期。
pub fn generate_v2_amendment() -> String {
    let mut out = header();
    let t = 120;
    let sat = "G02";
    let si = 1;
    let ctx = bands_for(sat)
        .into_iter()
        .find(|c| c.band.freq_num == "L1")
        .unwrap();
    let p = code_m(si, t, 0.0);
    let phase = smooth_phase_cycles(si, t, 0.0, ctx.band.wavelength_m, ctx.ambiguity);
    let doppler = format!("{:.6}", doppler_hz(si, ctx.band.wavelength_m));
    let snr = 42.0 + 3.0 * ((t as f64) / 90.0 + sat_phase(si)).sin();
    out.push_str(&format!(
        "# 修订：撤销 120s 的 LLI 置位（现场复核确认未失锁）\n"
    ));
    out.push_str(&format!(
        "OBS {CASE_STATION} {sat} L1 1C +120s {p:.6} {phase:.6} {doppler} 0 {snr:.2}\n"
    ));
    out
}

fn header() -> String {
    format!(
        "# 载波断点台离线案例（seed={CASE_SEED}）——按历元原样保存的 RINEX 观测子集\n\
         FORMAT {FORMAT_VERSION}\n\
         REFERENCE 2026-01-01T00:00:00Z\n"
    )
}

/// 明确格式错误的样例：用于验证导入层拒收与逐行诊断。
pub fn bad_sample() -> &'static str {
    "# 故意包含多种格式错误的小样本\n\
     FORMAT GBSV9\n\
     REFERENCE 2026-01-01T00:00:00Z\n\
     OBS ST01 G01 L1 1C +30s not_a_number 12.0 -0.3 0 44.0\n\
     FROBNICATE x y\n"
}

/// 可复制的最小正确样例（README 中的微型导入示例）。
pub fn mini_sample() -> &'static str {
    "FORMAT GBSV1\n\
     REFERENCE 2026-01-01T00:00:00Z\n\
     SATSTATE G01 +0s OK HEALTHY\n\
     OBS ST01 G01 L1 1C +0s 20000000.000000 104968944.098901 -0.000000 0 45.00\n\
     OBS ST01 G01 L2 2W +0s 20000000.000000 81949057.179168 -0.000000 0 43.00\n\
     SATSTATE G01 +30s OK HEALTHY\n\
     OBS ST01 G01 L1 1C +30s 19998800.000000 104975237.000000 -202.1022 0 45.00\n\
     OBS ST01 G01 L2 2W +30s 19998800.000000 81953956.000000 -157.7586 0 43.00\n"
}
