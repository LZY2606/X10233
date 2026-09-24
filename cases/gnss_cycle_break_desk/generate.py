#!/usr/bin/env python3
# 确定性生成器（固定 seed：本脚本不含任何随机源）。
# 产物：step1_initial.obs（初始版本）、step3_local_revision.obs（局部源数据修订）。
from datetime import datetime, timedelta, timezone

C = 299_792_458.0
F1, F2 = 1_575_420_000.0, 1_227_600_000.0
L1, L2 = C / F1, C / F2
DT = [0, 11, 23, 34, 45, 60, 80, 89]
T0 = datetime(2024, 1, 1, tzinfo=timezone.utc)
DOP = {"G01": (-3080.0, -2400.0), "G02": (-3157.0, -2460.0), "G03": (-3234.0, -2520.0)}
SNR = {"G01": (44.2, 41.7), "G02": (45.1, 42.0), "G03": (43.4, 40.9)}
POS = {  # ITRF2014, 米
 "G01": (13234567.123, 10098765.432, 20123456.789),
 "G02": (14111222.987, -9988776.654, 18888111.222),
 "G03": (-8877665.432, 15566778.899, 17799000.111),
}

def base_rows(rev=False):
    # missing[(ord,sat,code)] = True 表示该历元该观测缺失
    missing = set()
    # G02 在 ord4/ord5 缺失第二频率（配合 TD 等分候选边界组合）
    for o in (4, 5):
        for code in ("C2W", "L2W", "D2W", "S2W"):
            missing.add((o, "G02", code))
    # G03 在 ord6 缺失第二频率（单频/未知态、弧段中断）
    for code in ("C2W", "L2W", "D2W", "S2W"):
        missing.add((6, "G03", code))
    lli = {(5, "G03", "L2W"): "1"}  # LLI 误报：无数值跳变
    rows = {}
    for o, te in enumerate(DT):
        for sat, (d1, d2) in DOP.items():
            # 小的 Doppler 记录偏差：G03 ord2（影响 b1/b2 时间差分），无几何/物理跳变
            dd1 = -3233.941 if (sat == "G03" and o == 2) else d1
            if rev and sat == "G01" and o == 7:
                dd1 = -3081.000  # 局部修订：仅改动最后历元 Doppler
            clk = 1.0 if o >= 4 else 0.0  # 整机钟跳 +1ms，自 ord4 起
            p1 = 20_000_000.0 + d1 * L1 * te + clk * 299_792.458
            p2 = p1 + 3.0
            l1v = 105_000_000.0 + (-d1) * te + clk * F1 * 0.001
            l2v = 82_000_000.0 + (-d2) * te + clk * F2 * 0.001
            if sat == "G02" and o >= 5:
                l1v += 31.5  # 半格残差（永久偏移）-> TD 31/32 等分整周候选
            if sat == "G01" and o >= 6:
                l1v += 7.0   # 单星周跳 +7 周（L1，永久偏移）
            vals = {
                "C1C": (f"{p1:.3f}", "m"), "L1C": (f"{l1v:.3f}", "cy"),
                "D1C": (f"{dd1:.3f}", "Hz"), "S1C": (f"{SNR[sat][0]:.1f}", "dBHz"),
                "C2W": (f"{p2:.3f}", "m"), "L2W": (f"{l2v:.3f}", "cy"),
                "D2W": (f"{d2:.3f}", "Hz"), "S2W": (f"{SNR[sat][1]:.1f}", "dBHz"),
            }
            for code, (v, u) in vals.items():
                if (o, sat, code) in missing:
                    continue
                extra = ""
                if (o, sat, code) in lli:
                    extra = f" LLI {lli[(o,sat,code)]} SSI 4"
                rows[(o, sat, code)] = (v, u, extra)
    return rows

HEADER = """\
# 载波断点台：GNSS 周跳候选、多频组合与弧段修订史
# 离线案例包材料（小型 RINEX 风格观测摘录，原始表示按历元原样保留）
CASE       gnss_cycle_break_desk
STATION    ST01 MARKER "载波断点台" MARKER_TYPE GROUND TIMESCALE GPST COORD 35.6809 139.7673 40.0 DATUM ITRF2014
SIGNAL     G 1 C C1C 1575420000 Hz
SIGNAL     G 1 C L1C 1575420000 Hz
SIGNAL     G 1 C D1C 1575420000 Hz
SIGNAL     G 1 C S1C 1575420000 Hz
SIGNAL     G 2 W C2W 1227600000 Hz
SIGNAL     G 2 W L2W 1227600000 Hz
SIGNAL     G 2 W D2W 1227600000 Hz
SIGNAL     G 2 W S2W 1227600000 Hz
"""

def render(rev=False):
    rows = base_rows(rev)
    out = [HEADER.rstrip()]
    if rev:
        out.append('PARENT_NOTE "仅改动 ST01 G01 D1C @ 2024-01-01T00:01:29Z: -3080.000 -> -3081.000 Hz，影响域仅最后边界的 L1 时间差分"')
    out.append("")
    for o, te in enumerate(DT):
        t = (T0 + timedelta(seconds=te)).strftime("%Y-%m-%dT%H:%M:%SZ")
        clk_off = "0.001000" if o >= 4 else "0.000000"
        out.append(f"EPOCH {t} FLAG 0 CLOCK_OFFSET {clk_off} s")
        if o == 4:
            out.append("EVENT CLOCK_JUMP RECEIVER +0.001 s NOTE \"all-satellite code+phase common jump\"")
        for sat in ("G01", "G02", "G03"):
            x, y, z = POS[sat]
            out.append(f"SAT {sat} HEALTH OK POS {x:.3f} {y:.3f} {z:.3f} m FRAME ITRF2014")
            for code in ("C1C", "L1C", "D1C", "S1C", "C2W", "L2W", "D2W", "S2W"):
                k = (o, sat, code)
                if k in rows:
                    v, u, extra = rows[k]
                    out.append(f"OBS {sat} {code} {v} {u}{extra}")
        out.append("")
    return "\n".join(out) + "\n"

with open("step1_initial.obs", "w") as f:
    f.write(render(False))
with open("step3_local_revision.obs", "w") as f:
    f.write(render(True))

# ---- 数值自检（期望的检测器输入量）----
def cy(sat, o, code, rev=False):
    r = base_rows(rev)
    return float(r[(o, sat, code)][0])
print("f2/f1 =", F2 / F1, " 60/77 =", 60 / 77)
for b in range(1, 4):
    g3 = (cy("G03", b, "L1C") - cy("G03", b - 1, "L1C")) - \
        -((-3233.941 if b-1 == 2 else -3234.0) + (-3233.941 if b == 2 else -3234.0)) / 2 * (DT[b]-DT[b-1])
    print(f"G03 TD b{b-1} residual cy = {g3:+.4f}")
g02 = (cy("G02",5,"L1C")-cy("G02",4,"L1C")) - 3157.0*15
print("G02 TD b4 residual cy =", g02, " (tie 31/32:", abs(abs(g02-31)-abs(g02-32)) < 1e-9, ")")
g01 = (cy("G01",7,"L1C")-cy("G01",6,"L1C")) - -((-3081.0)+(-3080.0))/2*9
print("G01 TD b6 revised residual cy =", g01)
gf7 = 7*L1
print("G01 GF b5 jump m =", gf7, " MW jump cy = 7.0")
print("clock code jump m =", 299792.458, " L1 cy =", F1*0.001, " L2 cy =", F2*0.001)
