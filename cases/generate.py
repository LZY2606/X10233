#!/usr/bin/env python3
"""载波断点台离线案例包生成器。固定参数、无随机、可复现。
重新生成: python3 cases/generate.py  (输出已提交进仓库, 无需在构建/测试时运行)
"""
import json, os

C = 299792458.0
F1, F2 = 1575.42e6, 1227.60e6
LAM1, LAM2 = C / F1, C / F2
LAM_WL = C / (F1 - F2)

WEEK = 2450
TOW0 = 345600.0  # 2026-12-24 00:00:00 GPS
DT = [0, 1, 16, 17, 18, 19, 20, 21]  # 不等间隔历元: 1s 后接 15s 跳变
CLK_JUMP_M = 157.5 * LAM1            # 整机钟跳: L1 上恰为 157.5 周
CLK_JUMP_S = CLK_JUMP_M / C

SATS = {
    "G01": dict(R0=1.20e6, v=-700.0, N1=400000.25, N2=310000.5),
    "G02": dict(R0=1.25e6, v=+650.0, N1=410000.5,  N2=320000.25),
    "G03": dict(R0=1.18e6, v=-720.0, N1=420000.75, N2=330000.0),
}
C2_OFFSET = 0.35  # C2W 与 C1C 的固定群延迟差 (m)

def fmt_field(value, lli=0):
    if value is None:
        return " " * 16
    s = f"{value:14.3f}"
    assert len(s) == 14, s
    return s + (str(lli) if lli else " ") + " "

def hdr():
    lines = [
        ("     3.03           OBSERVATION DATA    G (GPS)", "RINEX VERSION / TYPE"),
        ("CABS", "MARKER NAME"),
        ("载波断点台案例站".encode("utf-8").decode("utf-8"), "COMMENT"),
        ("G    8 C1C L1C D1C S1C C2W L2W D2W S2W", "SYS / # / OBS TYPES"),
        ("", "END OF HEADER"),
    ]
    return [f"{c:<60}{label}" for c, label in lines]

def epoch_line(t, clk_s=0.0):
    # 2026-12-24 00:00:00 + t  (GPS 周 2450, TOW 345600)
    sec = t
    return f"> 2026 12 24 00 00{sec:11.7f}  0  3{clk_s:15.12f}"

def build(variant):
    """variant: v1 | v2 | branch2"""
    out = hdr()
    for i, t in enumerate(DT):
        clk_s = 0.0
        if variant in ("v1", "v2") and t >= 16:
            clk_s = CLK_JUMP_S
        out.append(epoch_line(float(t), clk_s))
        for sat, p in SATS.items():
            R = p["R0"] + p["v"] * t
            c1, c2 = R, R + C2_OFFSET
            l1, l2 = R / LAM1 + p["N1"], R / LAM2 + p["N2"]
            d1, d2 = -p["v"] / LAM1, -p["v"] / LAM2
            s1, s2 = 45.0, 41.0
            lli1 = 0
            if variant in ("v1", "v2"):
                if t >= 16:  # 整机钟跳: 所有卫星码/载波整体平移
                    c1 += CLK_JUMP_M; c2 += CLK_JUMP_M
                    l1 += CLK_JUMP_M / LAM1; l2 += CLK_JUMP_M / LAM2
                if sat == "G01" and t >= 17:  # 单星周跳: G01 双频各 +2 周
                    l1 += 2.0; l2 += 2.0
                if sat == "G02" and t == 1:   # LLI 误报
                    lli1 = 1
                if sat == "G03":
                    if t >= 17:               # G03 L1 半周跳 (等分整周候选)
                        l1 += 0.5
                    if t in (17, 20):         # 频率缺失: L2 波段整体缺失
                        c2 = l2 = d2 = s2 = None
                if variant == "v1" and sat == "G01" and t == 1:
                    l1 += 1.0                 # v1 源数据笔误, v2 修订
            if variant == "branch2":
                if sat == "G03" and t >= 18:  # 码跳变 3.0 m (载波不变) -> MW 与 GF/TD 分歧
                    c1 += 3.0; c2 += 3.0
            fields = [fmt_field(c1), fmt_field(l1, lli1), fmt_field(d1), fmt_field(s1),
                      fmt_field(c2), fmt_field(l2), fmt_field(d2), fmt_field(s2)]
            out.append(sat + "".join(fields))
    return "\n".join(out) + "\n"

def bad_values():
    lines = build("v2").splitlines()
    # G02 第一个历元 L1C 字段不可解析: 保留原值 + 诊断
    for i, ln in enumerate(lines):
        if ln.startswith("G02"):
            lines[i] = ln[:3 + 16] + "**************" + ln[3 + 32:]
            break
    return "\n".join(lines) + "\n"

def malformed():
    txt = build("v2").splitlines()
    for i, ln in enumerate(txt):
        if ln.startswith(">"):
            txt[i] = "> 20X6 12 24 00 00  0.0000000  0  3"
            break
    return "\n".join(txt) + "\n"

def main():
    here = os.path.dirname(os.path.abspath(__file__))
    rinex = os.path.join(here, "rinex")
    os.makedirs(rinex, exist_ok=True)
    files = {
        "cabs_v1.obs": build("v1"),
        "cabs_v2.obs": build("v2"),
        "cabs_branch2.obs": build("branch2"),
        "bad_values.obs": bad_values(),
        "malformed.obs": malformed(),
    }
    for name, txt in files.items():
        with open(os.path.join(rinex, name), "w") as f:
            f.write(txt)
    # ---- 自检: 关键边残差 ----
    def series(variant, sat):
        rows = {}
        for ln in build(variant).splitlines():
            pass
        return rows
    print("clock jump m:", CLK_JUMP_M, "cycles L1:", CLK_JUMP_M / LAM1)
    print("lam_wl:", LAM_WL)
    print("files written:", sorted(files))

if __name__ == "__main__":
    main()
