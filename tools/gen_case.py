#!/usr/bin/env python3
"""Provenance generator for the offline case 'carrier-break-station'.

Synthesizes RINEX-style epoch observations (ROBS/1 text) with units kept on
every token. The scenario replays:
  t5 - 95 s gap, receiver clock jump +1e-4 s, G01 LLI false positive
  t6 - G01 +3 cycles on L1 only, L2 frequency missing (t6,t7)
  t4 - G03 (1,1) cycle slip plus a P1 code glitch (-1.070 m):
       Melbourne-Wubbena disagrees with phase TD / geometry-free
  t7 - G02 L1 revised in batch b2 (+0.1 cycle): local-only invalidation
All quantities are zero-noise and deterministic (fixed seed is irrelevant,
kept for the project rule: seed=20260924).
"""
import math, os

SEED = 20260924
C = 299792458.0
F1, F2 = 1575.42e6, 1227.60e6
W1, W2 = C / F1, C / F2
WW = C / (F1 - F2)
ALPHA = (F1 / F2) ** 2

T0 = "2026-09-24T01:00:00Z"
T = [0, 10, 20, 30, 45, 140, 150, 160]

# sat: (r0, v, accel, I0, dI, N1, N2, snr1, snr2, el0, az0)
SATS = {
    "G01": (20100000.0, -760.0, 0.12, 6.2, -0.0009, 123456.0, 98765.0, 45.2, 42.1, 42.0, 181.0),
    "G02": (21400000.0,  540.0,-0.09, 8.1,  0.0006, 222111.0,177000.0, 47.0, 43.8, 55.0,  96.0),
    "G03": (19800000.0, -120.0, 0.05, 4.7, -0.0004,  70013.0, 54002.0, 39.5, 36.9, 24.0, 274.0),
}
CLOCK_JUMP_AT = 5          # epoch index, +1e-4 s at t5..t8
CLOCK_US = 1e-4
SLIP = {("G01", 6): (3.0, 0.0)}                 # (a,b) cycles
CODE_GLITCH = {("G03", 4): ("P1", -1.070)}      # meters, one epoch
LLI = {("G01", 5, 1): "1"}
MISSING = {("G01", 6): (2,), ("G01", 7): (2,)}
REVISION_B2 = ("G02", 7, "L1C", 0.1)            # cycles added in batch b2


def clock(i):
    return CLOCK_US if i >= CLOCK_JUMP_AT else 0.0


def base(sat, i):
    r0, v, ac, I0, dI, N1, N2, *_ = SATS[sat]
    t = T[i]
    r = r0 + v * t + 0.5 * ac * t * t
    I = I0 + dI * t
    cd = C * clock(i)
    P1 = r + I + cd
    P2 = r + ALPHA * I + cd
    L1 = (r - I + cd) / W1 + N1
    L2 = (r - ALPHA * I + cd) / W2 + N2
    D1 = (v + ac * t - dI) / W1
    D2 = (v + ac * t - ALPHA * dI) / W2
    for (s, j), (a, b) in SLIP.items():
        if s == sat and i >= j:
            L1 += a
            L2 += b
    for (s, j), (code, val) in CODE_GLITCH.items():
        if s == sat and i == j:
            if code == "P1":
                P1 += val
    return P1, P2, L1, L2, D1, D2


def elaz(sat, i):
    *_, el0, az0 = SATS[sat]
    return el0 + 0.13 * i, az0 - 0.6 * i


def header(batch, parent):
    lines = [
        "# ROBS/1 carrier-break-station offline case (zero-noise synthetic, seed=%d)" % SEED,
        "@FORMAT ROBS/1.0",
        "@STATION CBSM  35.681 139.767 40.0  datum=WGS84",
        "@COORDSYS datum=WGS84",
        "@FREQ G L1 1575.42[MHz] code=L1C",
        "@FREQ G L2 1227.60[MHz] code=L2W",
        "@BATCH %s" % batch,
    ]
    if parent:
        lines.append("@PARENT %s" % parent)
    return lines


def epoch_lines(i, b2=False):
    out = ["EPOCH %s +%06.1f[s]" % (T0, float(T[i]))]
    if i == CLOCK_JUMP_AT:
        out.append("EVENT CLOCK_JUMP +%.7f[s] source=rinex-header-note" % CLOCK_US)
    for sat in SATS:
        P1, P2, L1, L2, D1, D2 = base(sat, i)
        if b2:
            s2, j2, code2, add = REVISION_B2
            if sat == s2 and i == j2 and code2 == "L1C":
                L1 += add
        _, _, _, _, _, _, _, s1, s2n, *_ = SATS[sat]
        el, az = elaz(sat, i)
        miss_bands = MISSING.get((sat, i), ())
        toks = ["SAT %s" % sat, "AZ[deg]=%.2f" % az, "EL[deg]=%.2f" % el,
                "CLOCK_BIAS[s]=%.7f" % clock(i)]
        if 1 not in miss_bands:
            toks += ["L1C[cyc]=%.4f" % L1, "P1[m]=%.4f" % P1,
                     "D1[Hz]=%.3f" % D1, "S1[dBHz]=%.1f" % s1]
            lli = LLI.get((sat, i, 1))
            if lli:
                toks.append("LLI1=%s" % lli)
        if 2 not in miss_bands:
            toks += ["L2W[cyc]=%.4f" % L2, "P2[m]=%.4f" % P2,
                     "D2[Hz]=%.3f" % D2, "S2[dBHz]=%.1f" % s2n]
        out.append("  " + "  ".join(toks))
    return out


def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    d = os.path.join(here, "cases", "carrier-break-station")
    with open(os.path.join(d, "observations-b1.robs"), "w") as f:
        f.write("\n".join(header("b1", None)) + "\n")
        for i in range(len(T)):
            f.write("\n".join(epoch_lines(i, False)) + "\n")
    with open(os.path.join(d, "observations-b2.robs"), "w") as f:
        f.write("\n".join(header("b2", "b1")) + "\n")
        _, i2, _, _ = REVISION_B2
        f.write("\n".join(epoch_lines(i2, True)) + "\n")
    # sanity prints
    P1, P2, L1, L2, D1, D2 = base("G03", 4)
    P1p, P2p, L1p, L2p, *_ = base("G03", 3)
    dw = ((L1 - L1p) - (L2 - L2p)) * WW - (F1 * (P1 - P1p) + F2 * (P2 - P2p)) / (F1 + F2)
    print("G03 t4 MW residual m=%.4f (w cycles=%.3f)" % (dw, dw / WW))
    P1, P2, L1, L2, D1, D2 = base("G01", 6)
    P1p, P2p, L1p, L2p, _, D1p = base("G01", 5)
    dt6 = T[6] - T[5]
    print("G01 t6 TD band1 residual cycles=%.4f (boundary dt=%.0fs)" %
          ((L1 - L1p) - 0.5 * (D1 + D1p) * dt6, dt6))
    P1g, P2g, L1g, L2g, _, _ = base("G02", 5)
    P1q, P2q, L1q, L2q, _, _ = base("G02", 4)
    print("G02 t5 TD band1 residual cycles=%.4f (clock jump, should be large)" %
          (L1g - L1q - 0.0))



if __name__ == "__main__":
    main()
