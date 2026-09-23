#!/usr/bin/env python3
"""Deterministic generator for the offline carrier-break case pack.

No RNG: every number is a closed-form constant. Regenerates the
RINEX-3-subset observation file, satellite state, a source amendment, a
detector-disagreement branch, explicit malformed samples and the
machine-readable event playbook. Run: python3 cases/generate_cases.py
"""
import json
import os

C = 299792458.0
F1 = 1575.42e6
F2 = 1227.60e6
LAM1 = C / F1
LAM2 = C / F2
WL_CYC_PER_M = 1.0 / LAM1 - 1.0 / LAM2

YEAR, MONTH, DAY = 2026, 9, 14
H0 = 2
OFFSETS = [0, 2, 4, 6, 16, 18, 20, 22, 24]  # irregular; 10 s gap at k=4
DP_MW_HALF = -5.5 / WL_CYC_PER_M  # code shift => MW jump of +5.5 cycles

SATS = {
    "G01": dict(P0=20_000_000.000, ion=6.0, n1=100000.0, doppler=12.0),
    "G02": dict(P0=21_000_000.000, ion=8.0, n1=120000.0, doppler=-7.0),
    "G03": dict(P0=22_000_000.000, ion=5.5, n1=140000.0, doppler=4.0),
    "G09": dict(P0=23_000_000.000, ion=10.0, n1=160000.0, doppler=0.0),
}
CODES = ["C1C", "L1C", "D1C", "C2W", "L2W", "D2W"]
OUT = "cases/carrier-break-station"


def hms(sec):
    total = H0 * 3600 + sec
    h = int(total // 3600)
    m = int((total % 3600) // 60)
    whole = int(total)
    return h, m, whole - h * 3600 - m * 60, total - whole


def clock_offset(sec):
    return 0.002 if sec >= 16 else 0.0  # whole-receiver 1 ms jump at k=4


def nominal(sat, sec):
    st = SATS[sat]
    c1 = st["P0"] - st["ion"]
    c2 = st["P0"] - (F1 / F2) ** 2 * st["ion"]
    n2 = st["n1"] * (LAM1 / LAM2)
    base1 = c1 / LAM1 + st["n1"]
    base2 = c2 / LAM2 + n2
    return {
        "C1C": base1 + st["doppler"] * sec,
        "L1C": base1 + st["doppler"] * sec,
        "D1C": -st["doppler"],
        "C2W": base2 + st["doppler"] * sec,
        "L2W": base2 + st["doppler"] * sec,
        "D2W": -st["doppler"],
    }


def apply_injections(sat, sec, v):
    cjump_cycles1 = C * clock_offset(sec) / LAM1
    cjump_cycles2 = C * clock_offset(sec) / LAM2
    for c in ("C1C", "L1C"):
        v[c] += cjump_cycles1
    for c in ("C2W", "L2W"):
        v[c] += cjump_cycles2
    if sat == "G01" and sec >= 18:
        v["L1C"] += 7.0
        v["L2W"] += 5.0
        for c in ("C1C",):
            v[c] += DP_MW_HALF / LAM1
        for c in ("C2W",):
            v[c] += DP_MW_HALF / LAM2
    if sat == "G02" and sec >= 18:
        v["L1C"] += 3.0
        v["L2W"] += 2.0
    return v


def lli_map(sat, sec):
    # G03 carries LLI bit=1 at k=6 while phase is clean: false positive.
    return {"L1C": 1} if sat == "G03" and sec == 20 else {}


def missing_map(sat, sec):
    return ["C2W", "L2W", "D2W"] if sat == "G01" and sec == 20 else []


def fmt_obs(val, lli, present, ss):
    if not present:
        return " " * 16
    return "%14.3f%1d%1s" % (val, lli, ss)


HEADER_TAIL = (
    "CBST00JPN                                                            MARKER NAME",
    "  1234567890AB                                                       MARKER NUMBER",
    "    2  G    C1C L1C D1C C2W L2W D2W                                 SYS / # / OBS TYPES",
    "  2179606.4411  1355220.8224  3487573.3941  ITRF2014 EPOCH 2020.0   APPROX POSITION XYZ",
    "        0.0000        0.0000        0.0000                        ANTENNA: DELTA H/E/N",
    "G  1575.420  1227.600  MHz  frequency numbers 1/1                 FREQUENCY FACT",
    "     2    18    G    14    2    2     0.0000000     0.0000000     INTERVAL / TIME OF FIRST / LAST OBS",
    "END OF HEADER",
)


def header(amend_of=None):
    lines = [
        "     3.03           OBSERVATION DATA    M                   RINEX VERSION / TYPE",
        "CBCASE  Generator v1     20260914 020000 UTC                   PGM / RUN BY / DATE",
        "Carrier Break Station                                                COMMENT",
    ]
    if amend_of:
        lines.append("AMENDMENT OF %-54s" % amend_of[:54])
    lines.extend(HEADER_TAIL)
    return lines


def epoch_header(sec, n_sats, clk):
    h, m, s, frac = hms(sec)
    return "> %4d %02d %02d %02d %02d %10.7f  0%3d%21.10f" % (
        YEAR, MONTH, DAY, h, m, s + frac, n_sats, clk)


def sat_row(sat, sec, v, ss_map=None):
    ss_map = ss_map or {}
    lli = lli_map(sat, sec)
    miss = missing_map(sat, sec)
    row = "%-3s" % sat
    for code in CODES:
        present = code not in miss
        if not present:
            row += " " * 16
        else:
            row += "%14.3f%1d%1s" % (
                v[code], lli.get(code, 0), ss_map.get(code, "6"))
    return row


def write_rinex(path, sat_ids, overrides=None, amend_of=None, raw_events=None):
    overrides = overrides or {}
    raw_events = raw_events or {}
    lines = header(amend_of)
    for sec in OFFSETS:
        if sec in raw_events:
            lines.append(raw_events[sec])
        lines.append(epoch_header(sec, len(sat_ids), clock_offset(sec)))
        for sat in sat_ids:
            v = nominal(sat, sec)
            v = apply_injections(sat, sec, v)
            if (sat, sec) in overrides:
                v = overrides[(sat, sec)](v)
            lines.append(sat_row(sat, sec, v))
    with open(path, "w") as fh:
        fh.write("\n".join(lines) + "\n")


BASE_RNX = "CBST00JPN_R_20262570200_01H_MO.rnx"
AMEND_RNX = "CBST00JPN_R_20262570200_01H_MO_amend01.rnx"
BRANCH_RNX = "CBST00JPN_R_20262570200_01H_MO_branch02.rnx"


def write_state(path):
    lines = [
        "# Satellite state / signal inventory for the carrier-break station",
        "# epoch_gpst_base: 2026-09-14T02:00:00 (GPS time, seconds column)",
        "# datum: ITRF2014 (frame epoch 2020.0); units: freq MHz, wavelength m",
        "# system sat code frequency_MHz frequency_number wavelength_m",
        "G G01 1575.420 1 %.10f" % LAM1,
        "G G01 1227.600 1 %.10f" % LAM2,
        "G G02 1575.420 1 %.10f" % LAM1,
        "G G02 1227.600 1 %.10f" % LAM2,
        "G G03 1575.420 1 %.10f" % LAM1,
        "G G03 1227.600 1 %.10f" % LAM2,
        "# clock jump event (whole receiver): at gpst+16s, +2.000000000e-3 s",
        "CLOCK_EVENT kind=JUMP at_second=16 magnitude_s=0.002000000 scope=RECEIVER",
        "END OF STATE",
    ]
    with open(path, "w") as fh:
        fh.write("\n".join(lines) + "\n")


def write_files():
    os.makedirs(OUT, exist_ok=True)
    clock_event = ("> EVENT CLOCK JUMP at_second=16 magnitude_s=0.002000000 "
                   "scope=RECEIVER")
    write_rinex(os.path.join(OUT, BASE_RNX), ["G01", "G02", "G03"],
                raw_events={16: clock_event})
    write_state(os.path.join(OUT, "satellite_state.txt"))
    # Source amendment: only G01 L2W at k=6 changes by +0.003 cycles.
    # Only G01 pair/arcs that consume that raw observation go stale.
    def tweak(v):
        v["L2W"] += 0.003
        return v
    write_rinex(os.path.join(OUT, AMEND_RNX), ["G01", "G02", "G03"],
                overrides={("G01", 20): tweak}, amend_of=BASE_RNX,
                raw_events={16: clock_event})


def write_branch():
    # Branch 02: one satellite G09, 4 even epochs. A jump at k=2 is built so
    # the ionosphere/geometry-free residual lands exactly on the half-tie,
    # while the time-difference detector is unique and Melbourne-Wubbena is
    # uniquely zero. No single detector is the truth.
    b_offs = [0, 2, 4, 6]
    lines = header()
    # strip interval header assumption is fine; emit epochs manually
    lines = [ln for ln in lines if "INTERVAL" not in ln]
    for sec in b_offs:
        v = nominal("G09", sec)
        if sec >= 4:
            v["L1C"] += 1.0
            v["L2W"] += 0.0
            # half-tie geometry-free: residual of +0.5*l1 => tie (1,0)/(1,1)
            v["L1C"] += 0.5
        h, m, s, frac = hms(sec)
        lines.append("> %4d %02d %02d %02d %02d %10.7f  0  1%21.10f" % (
            YEAR, MONTH, DAY, h, m, s + frac, 0.0))
        lines.append(sat_row("G09", sec, v))
    with open(os.path.join(OUT, BRANCH_RNX), "w") as fh:
        fh.write("\n".join(lines) + "\n")


def write_malformed():
    os.makedirs("cases/malformed", exist_ok=True)
    bad_header = [
        "     2.11           OBSERVATION DATA    M                   RINEX VERSION / TYPE",
        "CBCASE  Generator v1                                                 PGM / RUN BY / DATE",
        "END OF HEADER",
    ]
    with open("cases/malformed/unsupported_version.rnx", "w") as fh:
        fh.write("\n".join(bad_header) + "\n")
    bad_epoch = [
        "     3.03           OBSERVATION DATA    M                   RINEX VERSION / TYPE",
        "    1  G    C1C                                                 SYS / # / OBS TYPES",
        "END OF HEADER",
        "> 2026 99 40 25 70 99.0000000  0  1                    0.0000000000",
        "G01  20000000.000 0 6",
    ]
    with open("cases/malformed/bad_epoch.rnx", "w") as fh:
        fh.write("\n".join(bad_epoch) + "\n")
    unknown_code = [
        "     3.03           OBSERVATION DATA    M                   RINEX VERSION / TYPE",
        "    1  G    Z9Z                                                 SYS / # / OBS TYPES",
        "END OF HEADER",
    ]
    with open("cases/malformed/unknown_code.rnx", "w") as fh:
        fh.write("\n".join(unknown_code) + "\n")


MANIFEST = {
    "case_pack": "carrier-break-station",
    "title": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
    "version": "2026.09.14-r1",
    "seed": "fixed:no-rng:constants-only",
    "time_base": {
        "scale": "GPS",
        "first_epoch": "2026-09-14T02:00:00",
        "epoch_offsets_s": OFFSETS,
        "datum": "ITRF2014",
        "frame_epoch": "2020.0",
    },
    "units": {
        "pseudorange": "cycle (code observation, RINEX C*)",
        "carrier": "cycle (RINEX L*)",
        "doppler": "Hz (RINEX D*)",
        "lli": "RINEX LLI flag 0..7 (loss-of-lock indicator)",
        "snr": "RINEX signal strength 1..9 (S map kept verbatim)",
        "frequency": "MHz",
        "wavelength": "m",
        "clock": "s",
    },
    "station": "CBST00JPN",
    "satellites": ["G01", "G02", "G03"],
    "observation_codes": CODES,
    "frequencies_hz": {"L1": F1, "L2": F2},
    "files": {
        "base_rinex": BASE_RNX,
        "satellite_state": "satellite_state.txt",
        "source_amendment": AMEND_RNX,
        "detector_branch": BRANCH_RNX,
        "dossier": "DOSSIER.md",
        "playbook": "playbook.json",
        "expected": "expected.json",
    },
    "counter_examples": [
        "uneven_epochs",
        "frequency_missing",
        "receiver_clock_jump",
        "single_sat_cycle_slip",
        "lli_false_positive",
        "multiple_equipartition_integer_offsets",
        "arc_local_expiry",
        "detector_disagreement",
    ],
}


def write_manifest():
    with open(os.path.join(OUT, "case.json"), "w") as fh:
        json.dump(MANIFEST, fh, ensure_ascii=False, indent=2, sort_keys=True)
        fh.write("\n")


PLAYBOOK = [
    {"step": 1, "action": "import", "file": BASE_RNX, "state": "satellite_state.txt",
     "check": "raw_verbatim"},
    {"step": 2, "action": "review", "scope": "G03@k6", "note": "LLI bit set but clean phase"},
    {"step": 3, "action": "reject", "candidate_ref": "lli:G03:L1C:20",
     "reason": "ll_false_positive_clean_combinations"},
    {"step": 4, "action": "revise_params", "params": {"gf_tol_cyc": 0.4},
     "note": "parameter revision; dependent results recomputed, history kept"},
    {"step": 5, "action": "accept", "candidate_ref": "gf_td:G02:L1C/L2W:18:(3,2)",
     "offset_layer": {"L1C": 3, "L2W": 2}},
    {"step": 6, "action": "accept", "candidate_ref": "mw:G01:WL:18:7",
     "note": "conflicting human decision branch point"},
    {"step": 7, "action": "accept", "candidate_ref": "gf_td:G01:L1C/L2W:18:(7,5)",
     "supersedes_step": 6, "note": "conflict resolution written to history"},
    {"step": 8, "action": "import", "file": AMEND_RNX, "amendment": True,
     "check": "only_G01_dependent_results_go_stale"},
    {"step": 9, "action": "export", "then": "delete_local_db", "then_import": True,
     "check": "identities_order_provenance_preserved"},
]


BRANCH_PLAYBOOK = [
    {"step": 1, "action": "import", "file": BRANCH_RNX, "state": "satellite_state.txt"},
    {"step": 2, "action": "review", "scope": "G09@k2",
     "note": "GF half-tie (1,0)/(1,1); TD unique; MW zero"},
    {"step": 3, "action": "accept", "candidate_ref": "gf:G09:L1C/L2W:4:(1,0)"},
    {"step": 4, "action": "fork"},
    {"step": 5, "action": "accept", "candidate_ref": "gf:G09:L1C/L2W:4:(1,1)",
     "note": "legal equipartition alternative kept; fork retained"},
]


EXPECTED = {
    "dp_mw_half_m": DP_MW_HALF,
    "wl_cycles_per_m": WL_CYC_PER_M,
    "lambda1_m": LAM1,
    "lambda2_m": LAM2,
    "clock_jump": {"at_second": 16, "magnitude_s": 0.002, "scope": "RECEIVER"},
    "epochs_s": OFFSETS,
    "expectations": [
        {"id": "uneven_epochs", "gap_s_between_k3_k4": 10,
         "assert": "dt used per pair; no assumed constant cadence"},
        {"id": "frequency_missing", "sat": "G01", "second": 20,
         "missing_codes": ["C2W", "L2W", "D2W"],
         "assert": "epoch retained; L1-only values present; pair status UNKNOWN"},
        {"id": "receiver_clock_jump", "at_second": 16,
         "assert": "all satellites flagged; separated from single-sat slips; GF unchanged"},
        {"id": "single_sat_cycle_slip_g02", "second": 18, "offset": {"L1C": 3, "L2W": 2},
         "assert": "GF and TD unique; no clock flag"},
        {"id": "lli_false_positive", "sat": "G03", "second": 20, "code": "L1C",
         "assert": "LLI candidate retained and rejected; clean combination evidence kept"},
        {"id": "mw_equipartition", "sat": "G01", "second": 18,
         "mw_wl_candidates": [7, 8],
         "assert": "two equal-score legal integer offsets both retained"},
        {"id": "gf_td_unique_g01", "sat": "G01", "second": 18,
         "offset": {"L1C": 7, "L2W": 5},
         "assert": "GF and TD agree; conflicts with MW tie -> human resolution"},
        {"id": "arc_local_expiry", "amendment_file": AMEND_RNX,
         "changed_raw": ["G01:L2W:20"],
         "assert": "only dependent G01 pair/arcs stale; others stay current"},
    ],
}


def write_expected_and_playbook():
    with open(os.path.join(OUT, "expected.json"), "w") as fh:
        json.dump(EXPECTED, fh, indent=2, sort_keys=True)
        fh.write("\n")
    with open(os.path.join(OUT, "playbook.json"), "w") as fh:
        json.dump({"main": PLAYBOOK, "branch_detector_disagreement": BRANCH_PLAYBOOK},
                  fh, ensure_ascii=False, indent=2)
        fh.write("\n")


DOSSIER = """# 反例档案：载波断点台（GNSS 周跳候选、多频组合与弧段修订史）

本档案只使用仓库内固定数值样例，无随机、无网络。所有领域量带单位/基准，
导入层按历元原样保存伪距（C*，周）、载波（L*，周）、Doppler（D*，Hz）、
LLI（0..7）、信噪比（1..9）、钟跳事件（s）、频率编号与观测码；派生层才规范化。

## 基线（baseline，不被覆盖）
`%s` + `satellite_state.txt` 导入后形成不可变输入版本：历元不等间隔
[0,2,4,6,16,18,20,22,24]（k3→k4 为 10 s），gpst+16s 有整机 1 ms 钟跳，
G01 在 k=5 起 (7,5) 周跳且码偏移使 Melbourne-Wubbena 观测跳变恰为 +7.5
（候选 7/8 等分整周），G02 在 k=5 起 (3,2) 周跳，G03 在 k=6 只有 LLI=1
（误报，相位干净），G01 在 k=6 缺失全部 L2 频率（保留该历元，组合置 UNKNOWN）。

## 只改一个直接来源的实验
1. 参数修订：`gf_tol_cyc` 0.25→0.40；仅依赖该参数的候选版本过期，历史可读。
2. 局部源数据改动：`%s` 只改 G01 在 20 s 的 L2W（+0.003 周）；
   仅消费该原始量的 G01 配对/弧段过期，G02/G03 仍为当前。
3. 冲突人工决定：先接受 MW 的 WL=7，再接受 GF/TD 的 (7,5)；
   两者都作为事件保留，后者标记 supersedes，撤销/冲突解决都写历史。

## 检测器判断依据（机器可读事件，互不当真值）
电离层/geometry-free、Melbourne-Wubbena、时间差分（Doppler）各自产出候选并带
拒绝原因、误差与未知状态；钟跳（RECEIVER 范围）与单星周跳分开记录；
修复只写入偏移层（offset layer），不改原始载波。

分支 `%s`：G09 的跳变使 geometry-free 残差落在半整数等分点，
(1,0) 与 (1,1) 得分相同；时间差分唯一；MW 唯一为 0。无法唯一决定时全部合法候选保留。

## 身份与回放
每条显示值有稳定身份；导出为 bundle、删除本地库再导入后，身份、顺序与来源关系保持。
回到任意旧事件时恢复当时的算法版本、参数指纹、直接来源与可见结论。
""" % (BASE_RNX, AMEND_RNX, BRANCH_RNX)


def write_dossier():
    with open(os.path.join(OUT, "DOSSIER.md"), "w") as fh:
        fh.write(DOSSIER)


def main():
    write_files()
    write_branch()
    write_malformed()
    write_manifest()
    write_expected_and_playbook()
    write_dossier()
    print("case pack generated in %s (dp_mw_half=%.9f m)" % (OUT, DP_MW_HALF))


if __name__ == "__main__":
    main()
