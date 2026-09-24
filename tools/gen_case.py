#!/usr/bin/env python3
"""Deterministic generator for the offline case pack
"载波断点台：GNSS 周跳候选、多频组合与弧段修订史".

No randomness, no network. Emits full per-version RINEX-OBS-derived text
snapshots plus steps.json with machine-readable expectations.
"""
import json
import os

C = 299792458.0
F1 = 1575420000.0
F2 = 1227600000.0
LAM1 = C / F1
LAM2 = C / F2
GAMMA = (F1 / F2) ** 2

# epoch index -> (ISO time, seconds)
TIMES = [
    ("2026-01-04T00:00:00", 0.0),
    ("2026-01-04T00:00:30", 30.0),
    ("2026-01-04T00:01:00", 60.0),
    ("2026-01-04T00:01:45", 105.0),   # irregular 45 s gap
    ("2026-01-04T00:02:15", 135.0),
    ("2026-01-04T00:03:00", 180.0),   # irregular 45 s gap
    ("2026-01-04T00:03:30", 210.0),
    ("2026-01-04T00:04:00", 240.0),
]

SATS = {
    # sat: (range0 m, range_rate m/s, accel m/s^2, iono0 m, iono_rate m/s, N1, N2, cnr)
    # G01 carries a small constant acceleration: Doppler linear prediction over
    # the 45 s irregular gaps accumulates ~0.5 cycle error, so the time
    # difference detector disagrees with GF/MW at e5 on purpose.
    "G01": (20000000.0, -500.0, 1.0e-4, 8.0,  0.0010, 100000, 77700, 46.0),
    "G02": (20100000.0,  300.0, 0.0,   12.0, -0.0008, 105000, 81000, 44.0),
    "G03": (20200000.0,  750.0, 0.0,    6.0,  0.0004, 99000,  76000, 42.0),
}

CLK_BASE = 1000.0          # constant receiver clock offset (m)
CLK_JUMP_M = 1000e-6 * C   # 1000 us receiver clock jump at e4 (v4+)
SLIP_EPOCH = 4             # clock jump + G02 slip epoch
AMB_EPOCH = 5             # G01 ambiguous slip epoch


def geometry(sat, t):
    r0, vr, acc, i0, ir, _, _, _ = SATS[sat]
    rho = r0 + vr * t + 0.5 * acc * t * t
    rate = vr + acc * t
    iono = i0 + ir * t
    return rho, iono, rate


def clk_offset(version, k):
    if version >= 4 and k >= SLIP_EPOCH:
        return CLK_BASE + CLK_JUMP_M
    return CLK_BASE


def phase_slip(version, sat, k, band):
    """Integer cycle offsets applied to raw phase (single-sat slips)."""
    n = 0
    if version >= 6 and sat == "G02" and k >= SLIP_EPOCH:
        n += 3 if band == 1 else 2
    if version >= 7 and sat == "G01" and k >= AMB_EPOCH:
        n += 4 if band == 1 else 3
    return n


def lli_flag(version, sat, k, band):
    flag = 0
    # real slip flags
    if version >= 6 and sat == "G02" and k == SLIP_EPOCH:
        flag = 1
    # false alarm: G03 band1 at e3, values stay continuous
    if version >= 5 and sat == "G03" and k == 3 and band == 1:
        flag = 1
    return flag


def obs_tuple(version, sat, k, band):
    _, t = TIMES[k]
    rho, iono, rate = geometry(sat, t)
    _, _, _, i0, ir, _, _, cnr = SATS[sat]
    clk = clk_offset(version, k)
    lam = LAM1 if band == 1 else LAM2
    ion = iono if band == 1 else GAMMA * iono
    code = rho + clk + ion
    # RINEX Doppler (Hz): positive when the range to the satellite decreases,
    # equal to the carrier-phase accumulation rate. vr is geometric range
    # rate (negative when approaching); iono term grows the code path.
    # `rate` already includes geometry + acceleration; iono grows the code path.
    # Bench Doppler convention (see README): D = signed code-path rate /
    # lambda; +500 m/s range growth => +2627 Hz L1. Phase advance = +D*dt.
    code_rate = rate + (ir if band == 1 else GAMMA * ir)
    doppler = code_rate / lam
    if version == 3 and sat == "G03" and k == 3 and band == 2:
        return None  # frequency missing: G03 L2 absent at e3
    phase_m = rho + clk - ion + lam * phase_slip(version, sat, k, band)
    n_amb = SATS[sat][5] if band == 1 else SATS[sat][6]  # N1,N2 tuple positions
    phase = phase_m / lam + n_amb
    return code, phase, doppler, cnr


def render(version):
    lines = []
    lines.append("@CASE carrier-break-station")
    lines.append("@FORMAT csbench-obs/1; RINEX-OBS-derived epoch snapshot")
    lines.append("@STATION ST01 lat=35.6812 lon=139.7671 height=40.0 datum=WGS84 receiver=CS-R1 antenna=CS-A1")
    lines.append("@TIMEFRAME UTC")
    lines.append("@TIMESCALE iso8601")
    lines.append("@CONSTELLATION GPS")
    lines.append(f"@WAVELENGTH L1 {LAM1:.12f} L2 {LAM2:.12f}")
    lines.append("@UNIT code=meter phase=cycle doppler=hz cnr=dbhz jump=microsecond range=meter")
    for k, (iso, t) in enumerate(TIMES):
        lines.append(f"@EPOCH {iso}")
        for sat in SATS:
            lines.append(f"FREQ {sat} L1={int(F1)} L2={int(F2)} source=RINEX")
            parts = [f"OBS {sat}"]
            for band, code_lbl, ph_lbl, d_lbl, s_lbl, lli in (
                (1, "C1C", "L1C", "D1C", "S1C", "LLI1"),
                (2, "C2W", "L2W", "D2W", "S2W", "LLI2"),
            ):
                tup = obs_tuple(version, sat, k, band)
                if tup is None:
                    parts.append(f"| {code_lbl}=MISSING {ph_lbl}=MISSING {d_lbl}=MISSING {s_lbl}=MISSING {lli}=0")
                    continue
                code, phase, dop, cnr = tup
                parts.append(
                    f"| {code_lbl}={code:.6f} {ph_lbl}={phase:.6f} "
                    f"{d_lbl}={dop:.6f} {s_lbl}={cnr:.1f} {lli}={lli_flag(version, sat, k, band)}"
                )
            lines.append(" ".join(parts))
            rho_sat, iono, _ = geometry(sat, t)
            lines.append(f"SATSTATE {sat} clk_bias_s=0.00001{ {'G01':2,'G02':3,'G03':4}[sat] } health=OK iono_l1_m={iono:.4f} range_m={rho_sat:.6f}")
        if version >= 4 and k == SLIP_EPOCH:
            lines.append('@CLOCKJUMP 1000 source=receiver raw="1PPS +1000us at epoch boundary"')
    return "\n".join(lines) + "\n"


PARAMS_V1 = {
    "algo_version": "csbench.core/1.0.0",
    "gf_sigma_m": 0.10,
    "mw_sigma_m": 0.10,
    "td_sigma_cycles": 0.5,
    "mw_envelope_m": 0.25,
    "clock_sigma_m": 500.0,
    "gap_factor": 5.0,
    "max_lattice": 3,
}
PARAMS_V2 = {
    "algo_version": "csbench.core/1.0.0",
    "gf_sigma_m": 0.15,
    "mw_sigma_m": 0.15,
    "td_sigma_cycles": 0.5,
    "mw_envelope_m": 0.25,
    "clock_sigma_m": 500.0,
    "gap_factor": 5.0,
    "max_lattice": 3,
}

STEPS = [
    {"id": "s1-import-baseline", "kind": "import_input", "file": "input-v1.txt",
     "title": "初始导入：8 个历元、G01/G02/G03 双频全量，无周跳",
     "expect": {"epochs": 8, "satellites": ["G01", "G02", "G03"], "open_findings": 0,
                "raw_presence": ["code", "phase", "doppler", "lli", "cnr", "clock_events", "frequency_numbers", "obs_codes"]}},
    {"id": "s2-param-revision", "kind": "revise_params", "params": PARAMS_V2,
     "title": "参数修订：GF/MW 容差 GF 0.10→0.15m、MW 0.10→0.15m（父版本保留，等分候选随之增多）",
     "expect": {"param_fingerprint_changes": True, "old_context_still_viewable": True}},
    {"id": "s3-local-source-change", "kind": "import_input", "file": "input-v3.txt",
     "title": "局部源数据改动：G03 在 e3 缺失 L2（频率缺失 + 弧段边界 + 局部过期）",
     "expect": {"missing_key": "G03@e3/L2W",
                "stale_subjects": ["G03"], "current_subjects": ["G01", "G02"],
                "arc_boundary": {"sat": "G03", "band": "L2", "epoch": "2026-01-04T00:01:45"}},
     "note": "版本号 v2 保留给参数修订，输入快照编号沿用 v1/v3...以便对照文件名"},
    {"id": "s4-receiver-clock-jump", "kind": "import_input", "file": "input-v4.txt",
     "title": "整机钟跳：e4 全星码相共同跳 1000us，仅产生钟跳事件，不产生单星周跳",
     "expect": {"clock_findings": 1, "slip_findings": 0,
                "clock_epoch": "2026-01-04T00:02:15", "clock_event_us": 1000}},
    {"id": "s5-lli-false-alarm", "kind": "import_input", "file": "input-v5.txt",
     "title": "LLI 误报：G03@e3 L1 置 LLI=1，但码/相连续；候选保留并被拒绝",
     "expect": {"lli_only_candidates": 1, "lli_only_status": "rejected_continuous",
                "candidate_retained": True}},
    {"id": "s6-single-sat-slip", "kind": "import_input", "file": "input-v6.txt",
     "title": "单星周跳：G02@e4 L1+3 / L2+2（与整机钟跳同历元，必须分开）",
     "expect": {"slip_findings": 1, "sat": "G02", "epoch": "2026-01-04T00:02:15",
                "legal_candidates": [[2, 1], [3, 2], [4, 3], [5, 4], [6, 5]],
                "td_best": [3, 2], "uniquely_identified": [3, 2],
                "clock_findings": 1, "clock_kept_separate": True,
                "decision": "accept", "repair_layer_only": True,
                "accepted_offsets": {"L1": 3, "L2": 2}}},
    {"id": "s7-ambiguous-equal-splits", "kind": "import_input", "file": "input-v7.txt",
     "title": "多个等分整周偏移：G01@e5 相邻候选相差 (1,1)；GF/MW 给 5 个等分候选，Doppler 时间差分因 45s 间隔与小加速度偏向另一处，不以任一检测器当真值",
     "expect": {"slip_findings_for": "G01", "epoch": "2026-01-04T00:03:00",
                "gf_candidates": [[2, 1], [3, 2], [4, 3], [5, 4], [6, 5]],
                "mw_candidates": [[2, 1], [3, 2], [4, 3], [5, 4], [6, 5]],
                "td_raw_cycles_approx": [4.53, 3.39],
                "td_best": [5, 3],
                "td_not_in_legal": True,
                "legal_candidates": [[2, 1], [3, 2], [4, 3], [5, 4], [6, 5]],
                "equal_split_step": [1, 1],
                "decision_a_branch": "main", "decision_a": [4, 3],
                "decision_b_branch": "alt", "decision_b": [3, 2]}},
    {"id": "s8-history-rewind", "kind": "rewind", "target_step": "s3-local-source-change",
     "title": "回到旧事件：恢复当时的输入版本/参数指纹与可见结论，不被最新结果覆盖",
     "expect": {"shows_context_of": "s3-local-source-change", "latest_unchanged": True}},
]


def main():
    out = os.path.join(os.path.dirname(__file__), "..", "cases", "carrier-break-station")
    out = os.path.abspath(out)
    os.makedirs(out, exist_ok=True)
    for v in (1, 3, 4, 5, 6, 7):
        with open(os.path.join(out, f"input-v{v}.txt"), "w") as fh:
            fh.write(render(v))
    pack = {
        "case_id": "carrier-break-station",
        "title": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
        "narrative": "测量团队导入 RINEX 观测历元和卫星状态，逐站星频率弧段审阅周跳候选与修复假设。",
        "constants": {"c": C, "f1_hz": F1, "f2_hz": F2, "lambda1_m": LAM1, "lambda2_m": LAM2,
                      "gamma": GAMMA, "wl_lambda_m": C / (F1 - F2)},
        "params_v1": PARAMS_V1,
        "params_v2": PARAMS_V2,
        "steps": STEPS,
    }
    with open(os.path.join(out, "steps.json"), "w") as fh:
        json.dump(pack, fh, ensure_ascii=False, indent=2)
    print("wrote case pack to", out)


if __name__ == "__main__":
    main()
