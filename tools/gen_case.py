#!/usr/bin/env python3
# Deterministic generator for the offline case pack
# "载波断点台 / Carrier Break Bench".
#
# It synthesises a self-consistent RINEX-obs-subset style import so that the
# geometry-free (ionosphere/GF), Melbourne-Wubbena (MW) and time-differenced
# (TD, Doppler aided) detectors can be hand-verified. No random numbers, no
# network, no wall clock. Outputs go to cases/.
#
# Units:
#   time      seconds since a fixed reference (virtual clock), also ISO tag
#   range/pseudorange  metre
#   carrier   cycle
#   doppler   hertz (cycle/second)
#   snr       raw RINEX signal-strength indicator 1..9
#   lli       raw RINEX Loss-of-Lock-Indication bit field
import os, json

C = 299792458.0
F1 = 1.57542e9          # GPS L1
F2 = 1.22760e9          # GPS L2
FR1 = 1.602e9 + (-7)*562500.0   # GLONASS slot R07, frequency number k=-7
FR2 = 1.246e9 + (-7)*437500.0
L1 = C/F1; L2 = C/F2
LR1 = C/FR1; LR2 = C/FR2

R0 = 20_000_000.0       # static geometric range (m)
VEL = -7.0              # constant range rate (m/s)
TAU = 1e-3              # receiver clock jump magnitude (s) between e4 and e5

T = [0, 30, 60, 120, 150, 210, 240, 270]
ISO = [f"2026-09-23T01:{t//60:02d}:{t%60:02d}Z" for t in T]

AMB = {
    "G01": (105_000_000, 82_000_000),
    "G02": (106_000_000, 83_000_000),
    "G03": (107_000_000, 84_000_000),
}
D1 = -VEL/L1
D2 = -VEL/L2

def clk_seconds(i):
    return TAU if i >= 4 else 0.0

def fmt(x, nd=4):
    return f"{x:.{nd}f}"

def gps_line(epoch_i, sat, overrides=None, l2_missing=False, lli=(0,0),
             snr=(8,7), dop_override=None):
    """overrides: dict band-> extra carrier cycles added at this epoch."""
    overrides = overrides or {}
    clk = clk_seconds(epoch_i)
    R = R0 + VEL*T[epoch_i]
    p1 = R + C*clk + 0.0000
    p2 = R + C*clk + 0.0000
    n1, n2 = AMB[sat]
    ph1 = (R + C*clk)/L1 + n1 + overrides.get(1, 0.0)
    ph2 = (R + C*clk)/L2 + n2 + overrides.get(2, 0.0)
    d1v = D1 if dop_override is None else dop_override.get(1, D1)
    d2v = D2 if dop_override is None else dop_override.get(2, D2)
    parts = [f"OBS {sat} L1 P1={fmt(p1,3)} L1={fmt(ph1,3)} D1={fmt(d1v,3)} S1={snr[0]} LLI1={lli[0]}"]
    if not l2_missing:
        parts.append(f"OBS {sat} L2 P2={fmt(p2,3)} L2={fmt(ph2,3)} D2={fmt(d2v,3)} S2={snr[1]} LLI2={lli[1]}")
    return parts

def glo_line(epoch_i):
    clk = clk_seconds(epoch_i)
    R = R0 + 1234.5 + VEL*T[epoch_i]
    p1 = R + C*clk; p2 = R + C*clk
    n1 = 108_000_000; n2 = 85_000_000
    ph1 = (R + C*clk)/LR1 + n1
    ph2 = (R + C*clk)/LR2 + n2
    dr1 = -VEL/LR1; dr2 = -VEL/LR2
    return [
      f"OBS R07 L1 P1={fmt(p1,3)} L1={fmt(ph1,3)} D1={fmt(dr1,3)} S1=7 LLI1=0",
      f"OBS R07 L2 P2={fmt(p2,3)} L2={fmt(ph2,3)} D2={fmt(dr2,3)} S2=7 LLI2=0",
    ]

def build_lines():
    L = []
    L.append("# Carrier Break Bench - offline case pack (RINEX-obs subset)")
    L.append("# Header metadata is preserved verbatim; FREQ declares the frequency")
    L.append("# basis (coordinate/time datum) and maps observation codes to bands.")
    L.append("STATION ST01 name=BREAKPOINT0 marker=ST01 receiver=RX-SIM antennatype=SIM")
    L.append("DATUM time_scale=GPST coordinate_frame=ITRF20 range_unit=metre carrier_unit=cycle doppler_unit=hertz snr=rinex_ssi lli=rinex_lli_bits")
    L.append("FREQ G L1 f=1575420000.0 codes=L1,P1,D1,S1,LLI1")
    L.append("FREQ G L2 f=1227600000.0 codes=L2,P2,D2,S2,LLI2")
    L.append("FREQ R L1 f=%.1f k=-7 codes=L1,P1,D1,S1,LLI1" % FR1)
    L.append("FREQ R L2 f=%.1f k=-7 codes=L2,P2,D2,S2,LLI2" % FR2)

    for i, iso in enumerate(ISO):
        rc = 2 if i < 4 else 1   # receiver clock event flag only on the boundary epoch
        L.append(f"EPOCH idx={i+1} t={T[i]} iso={iso} rcvr_clk_s={fmt(clk_seconds(i),4)} clock_event_flag={1 if i==4 else 0}")
        # G01 clean reference on both frequencies
        L += gps_line(i, "G01")
        # G02: e4 L2 missing (gap); e6 fractional 1.14 cycle break on L1 with
        # Doppler missing on BOTH bands -> GF yields two near-tied lattice
        # candidates while MW pins widelane = 1; e7 LLI false positive.
        ov = {1: 1.14} if i == 5 else None
        l2miss = (i == 3)
        lli = (4, 0) if i == 6 else (0, 0)   # bit2 set once on L1, no phase jump
        dop = {1: None, 2: None} if i == 5 else None
        # represent missing doppler by dropping the D token below
        parts = gps_line(i, "G02", overrides=ov, l2_missing=l2miss, lli=lli,
                         dop_override=None)
        if i == 5:
            # rewrite both G02 obs to omit Doppler (missing measurement, not 0)
            clk = clk_seconds(i); R = R0 + VEL*T[i]
            n1, n2 = AMB["G02"]
            ph1 = (R + C*clk)/L1 + n1 + 1.14
            ph2 = (R + C*clk)/L2 + n2
            parts = [
              f"OBS G02 L1 P1={fmt(R + C*clk,3)} L1={fmt(ph1,3)} S1=8 LLI1=0",
              f"OBS G02 L2 P2={fmt(R + C*clk,3)} L2={fmt(ph2,3)} S2=7 LLI2=0",
            ]
        L += parts
        # G03: clean except e6 integer pair slip (+2 L1, +1 L2)
        ov3 = {1: 2.0, 2: 1.0} if i == 5 else None
        L += gps_line(i, "G03", overrides=ov3)
        # GLONASS frequency-number carrier, clean, present all epochs
        L += glo_line(i)
        if i == 4:
            L.append("CLOCKJUMP t=%d iso=%s delta_s=%.4f scope=receiver cause=1ms_event" % (T[i], iso, TAU))
    return L

def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    cdir = os.path.join(here, "cases")
    os.makedirs(cdir, exist_ok=True)
    lines = build_lines()
    with open(os.path.join(cdir, "01_baseline.obs"), "w") as f:
        f.write("\n".join(lines) + "\n")

    # Amendment 2: only G03 source fragment is replaced (local dependency).
    # We regenerate G03 so that e6 becomes a (3,2) slip instead of (2,1).
    # Implemented as an overlay submission containing only changed OBS tokens
    # for one satellite at one epoch (supersede=direct-source).
    clk = clk_seconds(5); R = R0 + VEL*T[5]
    n1, n2 = AMB["G03"]
    ph1 = (R + C*clk)/L1 + n1 + 3.0
    ph2 = (R + C*clk)/L2 + n2 + 2.0
    amend = [
        "# Local source amendment: replaces ONLY G03 e6 carrier phases.",
        "AMEND target=G03 epoch_idx=6 mode=supersede_direct_source reason=re-surveyed_phase_readout",
        f"OBS G03 L1 P1={fmt(R + C*clk,3)} L1={fmt(ph1,3)} D1={fmt(D1,3)} S1=8 LLI1=0",
        f"OBS G03 L2 P2={fmt(R + C*clk,3)} L2={fmt(ph2,3)} D2={fmt(D2,3)} S2=7 LLI2=0",
    ]
    with open(os.path.join(cdir, "03_amend_g03_e6.obs"), "w") as f:
        f.write("\n".join(amend) + "\n")

    # Replay script: deterministic narrative for tests and the demo loader.
    script = {
        "case_id": "carrier-break-bench-001",
        "title": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
        "seed": 20260923,
        "virtual_clock": {"origin_iso": ISO[0], "epoch_seconds": T},
        "steps": [
            {"seq": 1, "kind": "import", "label": "initial-import",
             "file": "01_baseline.obs", "kind_detail": "raw observations + header + clock event, stored verbatim"},
            {"seq": 2, "kind": "revise_params", "label": "tighten-gf-threshold",
             "params": {"gf_threshold_m": 0.020, "mw_threshold_cy": 0.25,
                        "td_threshold_cy": 0.30, "clock_threshold_m": 100000.0,
                        "max_lattice_n": 5}},
            {"seq": 3, "kind": "revert_params", "label": "restore-default-params"},
            {"seq": 4, "kind": "amend_source", "label": "amend-only-G03-e6",
             "file": "03_amend_g03_e6.obs"},
            {"seq": 5, "kind": "fork", "label": "decision-fork",
             "fork_name": "review-fork"},
            {"seq": 6, "kind": "decide", "label": "accept-1-0-on-fork",
             "on_branch": "review-fork", "sat": "G02", "epoch_idx": 6,
             "pair": [1, 0], "decision": "accept"},
            {"seq": 7, "kind": "decide", "label": "accept-0--1-on-main",
             "sat": "G02", "epoch_idx": 6, "pair": [0, -1], "decision": "accept"}
        ],
        "expectations": {
            "uneven_epochs": "gap seconds [30,30,60,30,60,30,30] are recorded, no equal-spacing assumption",
            "missing_frequency": "G02 L2 absent at epoch idx=4 -> band gap, split L2 arc",
            "receiver_clock_jump": "common step ~299792.458 m code / integer cycles phase at idx=5, scope=receiver, not a satellite slip",
            "single_sat_slip": "G03 idx=6 integer pair (2,1) baseline -> (3,2) after local amendment",
            "lli_false_positive": "G02 idx=7 LLI bit set with no phase step -> candidate rejected",
            "multiple_tied_offsets": "G02 idx=6 GF lattice contains both (1,0) and (0,-1), MW=1, TD unavailable -> ambiguous",
            "arc_local_expiry": "G03 amendment expires only G03 arcs/residuals/candidates; G01/G02/R07 stay current"
        }
    }
    with open(os.path.join(cdir, "replay.json"), "w") as f:
        json.dump(script, f, ensure_ascii=False, indent=2)
    print("wrote cases/01_baseline.obs (%d lines), 03_amend_g03_e6.obs, replay.json" % len(lines))

if __name__ == "__main__":
    main()
