#!/usr/bin/env python3
"""Deterministic generator for the carrier-break-v1 offline case pack.

No network, no randomness. Produces a RINEX 3.03 observation subset plus
satellite-state and clock-event sidecars. Units and coordinate references are
preserved explicitly. All knobs are fixed so replay is byte-identical.
"""
import json
import os

C = 299792458.0
F1 = 1.57542e9
F2 = 1.2276e9
LAM1 = C / F1
LAM2 = C / F2
GAMMA = (F1 / F2) ** 2  # f1^2/f2^2 ~ 1.64694

# Irregular sampling: gaps of 30, 45, 45, 60, 60, ... seconds
T = [0, 30, 75, 120, 180, 240, 300, 360, 420, 480, 540]

# (sat, pseudorange_bias_m, range_rate_mps, iono_bias_m, iono_rate_mps)
SATS = {
    "G01": (21_000_000.0, -2.0, 8.0, 0.010),
    "G02": (22_000_000.0, -0.5, 6.0, 0.003),
    "G03": (23_000_000.0, -1.0, 4.0, 0.000),
}
N1 = {"G01": 1000.0, "G02": 2000.0, "G03": 3000.0}
N2 = {"G01": 800.0, "G02": 1500.0}

# Engineered boundary conditions
CLOCK_JUMP_EPOCH = 4          # receiver (all-satellite) 1e-7 s clock jump
G01_SLIP_EPOCH = 5            # genuine +1 cycle slip on G01 L1
G01_MISSING_L2_EPOCH = 2      # missing frequency (no L2 observables at all)
G02_LLI_FALSE_EPOCH = 6       # LLI bit set, but phase is continuous
G03_HALF_EPOCH = 3            # +0.5 cycle: TD detector cannot choose 0 vs +1
CLOCK_OFFSET = 1.0e-7


def iono(sat, t):
    _, _, b, rate = SATS[sat]
    return b + rate * t


def f14(x):
    return f"{x:14.3f}"


def field(value, lli=" ", snr=" "):
    if value is None:
        return " " * 16
    return f14(value) + lli + snr


def obs_line(sat, t):
    p0, rate, _, _ = SATS[sat]
    i1 = iono(sat, t)
    p1 = p0 + rate * t + i1
    l1 = (p0 + rate * t - i1) / LAM1 + N1[sat]
    d1 = -(rate - (SATS[sat][3])) / LAM1  # phase-range derivative: rate - i_rate
    if sat == "G01" and t >= T[G01_SLIP_EPOCH]:
        l1 += 1.0
    if sat == "G03" and t >= T[G03_HALF_EPOCH]:
        l1 += 0.5
    c1c = field(p1, snr="5")
    l1c = field(l1, lli=("1" if (sat == "G02" and t == T[G02_LLI_FALSE_EPOCH]) else " "), snr="5")
    d1c = field(d1, snr="5")
    if sat == "G03":
        return sat + c1c + l1c + d1c
    i2 = GAMMA * i1
    p2 = p0 + rate * t + i2
    l2 = (p0 + rate * t - i2) / LAM2 + N2[sat]
    d2 = -(rate - GAMMA * SATS[sat][3]) / LAM2
    if sat == "G01" and t == T[G01_MISSING_L2_EPOCH]:
        c2w = l2w = d2w = " " * 16
    else:
        c2w = field(p2, snr="4")
        l2w = field(l2, snr="4")
        d2w = field(d2, snr="4")
    return sat + c1c + l1c + d1c + c2w + l2w + d2w


def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    datadir = os.path.join(here, "data")
    os.makedirs(datadir, exist_ok=True)

    lines = [
        "     3.03           OBSERVATION DATA    M                   RINEX VERSION / TYPE",
        "CSDESK-GEN          PAIRWISE-GSB        20260923 000000 UTC PGM / RUN BY / DATE",
        "Carrier-break demo; crafted boundary conditions, deterministicCOMMENT",
        "CARRIER-BREAK-01                                             MARKER NAME",
        "PAIRWISE GSB SURVEY TEAM                                     OBSERVER / AGENCY",
        "    G03 C1C L1C D1C C2W L2W D2W                              SYS / # / OBS TYPES",
        "  2026     9    23     0     0    0.0000000     GPS         TIME OF FIRST OBS",
        "  2026     9    23     0     9    0.0000000     GPS         TIME OF LAST OBS",
        "                                                              END OF HEADER",
    ]
    for idx, t in enumerate(T):
        hh, rem = divmod(t, 3600)
        mm, ss = divmod(rem, 60)
        clk = f"{CLOCK_OFFSET:15.12e}" if idx == CLOCK_JUMP_EPOCH else ""
        lines.append(f"> 2026 09 23 {hh:02d} {mm:02d} {ss:11.7f}  0           3{clk}")
        for sat in ("G01", "G02", "G03"):
            lines.append(obs_line(sat, t))

    with open(os.path.join(datadir, "base.26o"), "w", newline="\n") as fh:
        fh.write("\n".join(lines) + "\n")

    states = []
    for t in T:
        for j, sat in enumerate(("G01", "G02", "G03")):
            states.append({
                "epoch_s": t,
                "sat": sat,
                "system": "GPS",
                "health": "OK",
                "elevation_deg": round(40.0 + j * 5.0 + 0.01 * t, 4),
                "azimuth_deg": round(120.0 + j * 30.0 + 0.02 * t, 4),
                "coordinate_ref": "ellipsoid/WGS84; az/el in degrees",
            })
    with open(os.path.join(datadir, "satellite-state.jsonl"), "w", newline="\n") as fh:
        for row in states:
            fh.write(json.dumps(row, sort_keys=True) + "\n")

    events = [{
        "t_epoch_s": T[CLOCK_JUMP_EPOCH],
        "kind": "receiver_clock_jump",
        "scope": "ALL_SATELLITES",
        "clock_offset_s_at_epoch": CLOCK_OFFSET,
        "source": "rinex_epoch_header",
        "unit": "second",
        "note": "Receiver clock jump shared by every satellite; not a single-satellite cycle slip.",
    }]
    with open(os.path.join(datadir, "clock-events.jsonl"), "w", newline="\n") as fh:
        for row in events:
            fh.write(json.dumps(row, sort_keys=True) + "\n")

    # Explicit format-error samples: parser must reject structurally, never crash.
    with open(os.path.join(datadir, "malformed-header.26o"), "w", newline="\n") as fh:
        fh.write("not a rinex file at all\n")
    with open(os.path.join(datadir, "malformed-field.26o"), "w", newline="\n") as fh:
        bad = list(lines)
        bad[10] = "> 2026 09 23 00 00 00.0000000  0           3"
        bad[11] = "G01 not_a_number_xx" + " " * 40
        fh.write("\n".join(bad) + "\n")

    meta = {
        "case_id": "carrier-break-v1",
        "title": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
        "time_scale": "GPS seconds from 2026-09-23T00:00:00Z",
        "epochs_s": T,
        "frequencies_hz": {"L1C": F1, "L2W": F2},
        "wavelengths_m": {"L1C": LAM1, "L2W": LAM2},
        "speed_of_light_mps": C,
        "engineered_conditions": {
            "irregular_epoch_gaps_s": [30, 45, 45, 60, 60, 60, 60, 60, 60, 60],
            "missing_frequency": {"sat": "G01", "epoch_s": T[2], "missing": ["C2W", "L2W", "D2W"]},
            "receiver_clock_jump": {"epoch_s": T[4], "offset_s": CLOCK_OFFSET, "scope": "ALL_SATELLITES"},
            "single_satellite_slip": {"sat": "G01", "epoch_s": T[5], "band": "L1C", "cycles": 1},
            "lli_false_positive": {"sat": "G02", "epoch_s": T[6], "band": "L1C"},
            "equipartition_half_cycle": {"sat": "G03", "epoch_s": T[3], "band": "L1C", "fraction": 0.5},
        },
        "detector_versions": {"GF": "gf@1", "MW": "mw@1", "TD": "td@1"},
    }
    with open(os.path.join(datadir, "manifest.json"), "w", newline="\n") as fh:
        json.dump(meta, fh, indent=2, sort_keys=True)
        fh.write("\n")


if __name__ == "__main__":
    main()
