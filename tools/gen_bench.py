#!/usr/bin/env python3
"""Deterministic generator for the carrier-break-bench offline case pack.

Fixed seed (no randomness), virtual epoch grid. Produces:
  * cases/carrier-break-bench/bench.sheet.json  (explicit sheet + sat state)
  * cases/carrier-break-bench/bench.obs         (RINEX 3 subset, same epochs)

Sign convention (mirrors src/domain/math.rs):
  D [Hz] = -dL/dt ; L [cycles] ; C [m] ; lambda1=c/f1.

Embedded situations:
  e2: uneven epoch spacing (only)
  e5: receiver (whole-instrument) clock step 3.3356e-3 s (declared)
  e6: G02 single-satellite +1 cycle L1 slip
  e3: G03 LLI false alarm (bit set, data smooth)
  e7: G01 L2 missing at e7,e8 + 0.5-cycle L1 slip at e7 (equally divided)
  e8: G01 P1-only +2.5 m code bump (detectors disagree)
"""
import json, math, os

SEED = 20260924
C = 299_792_458.0
F1 = 1575.42e6
F2 = 1227.60e6
LAM1 = C / F1
LAM2 = C / F2
WL = C / (F1 - F2)

# uneven gaps (seconds): 30, 47, 61, 30, 31, 45, 33
START = (9, 0, 0.0)
GAPS = [30.0, 47.0, 61.0, 30.0, 31.0, 45.0, 33.0]

# (h,m,s) epochs
def epoch_grid():
    h, m, s = START
    times = [s]
    for g in GAPS:
        times.append(times[-1] + g)
    out = []
    for t in times:
        hh = 9 + int(t // 3600)
        rem = t - (hh - 9) * 3600
        mm = int(rem // 60)
        ss = rem - mm * 60
        out.append((2026, 9, 24, hh, mm, ss))
    return out

EPOCHS = epoch_grid()
GPS0 = None  # filled below

def gps_seconds(e):
    from datetime import datetime
    y, mo, d, h, mi, s = e
    sec_int = int(s)
    frac = s - sec_int
    dt = datetime(y, mo, d, h, mi, sec_int)
    g0 = datetime(1980, 1, 6, 0, 0, 0)
    return (dt - g0).total_seconds() + frac

def iso(e):
    y, mo, d, h, mi, s = e
    return f"{y:04d}-{mo:02d}-{d:02d}T{h:02d}:{mi:02d}:{s:07.4f}"

# base geometric parameters per sv (constant velocity scenario)
SVS = {
    "G01": dict(rho0=21_000_000.0, vels=-600.0, az=42.0, el=55.0,
                ecef=(1.3475e7, 1.4532e7, 1.8674e7)),
    "G02": dict(rho0=22_500_000.0, vels=-420.0, az=120.0, el=38.0,
                ecef=(-1.7234e7, 1.3011e7, 1.5920e7)),
    "G03": dict(rho0=20_100_000.0, vels=+350.0, az=210.0, el=62.0,
                ecef=(9.1234e6, -2.0111e7, 1.6771e7)),
}
IONO0 = 5.0            # metres, slowly changing
IONO_RATE = 0.0002     # m/s (negligible over these gaps)

# injected faults keyed by (sv, epoch_index)
CLOCK_STEP_EPOCH = 5       # e6? index: 0-based -> e5? see below
CLOCK_STEP_S = 3.3356e-3

def build():
    rows = {}   # sv -> list over epochs of dict
    clock_events = []
    for sv, p in SVS.items():
        rows[sv] = []
        iono = IONO0
        prev = None
        for i, e in enumerate(EPOCHS):
            t = gps_seconds(e)
            # geometric range at this epoch (smooth)
            rho = p["rho0"] + p["vels"] * (t - gps_seconds(EPOCHS[0]))
            # receiver clock step persists from epoch index 4 onward (a reset
            # of the instrument time base moves both code & phase permanently)
            clk = CLOCK_STEP_S if i >= 4 else 0.0
            iono = IONO0 + IONO_RATE * (t - gps_seconds(EPOCHS[0]))

            P1 = rho + clk * C + iono
            P2 = rho + clk * C + iono * (F1 / F2) ** 2
            N1 = 100000 + (hash0(sv) % 900000)
            N2 = 200000 + (hash0(sv + "x") % 900000)
            # carrier phase advances where code delays: L_lam = geom - I_b (+clk)
            L1 = (rho + clk * C - iono) / LAM1 + N1
            L2 = (rho + clk * C - iono * (F1 / F2) ** 2) / LAM2 + N2
            # Doppler (Hz) = -dL/dt ; derive from velocity & rates
            # instantaneous frequency is unchanged by a clock reset
            D1 = -(p["vels"] / LAM1)
            D2 = -(p["vels"] / LAM2)
            SSI = 6

            rows[sv].append(dict(
                i=i, epoch=iso(e), gps=t,
                P1=P1, P2=P2, L1=L1, L2=L2, D1=D1, D2=D2, ssi=SSI,
                lli1=0, lli2=0, present2=True, clk=clk, iono=iono,
            ))
            prev = (L1, L2)

        # clock event declared once (epoch index 4)
    clock_events.append((4, CLOCK_STEP_S))

    # ---- inject satellite-specific faults ----
    # G02: single-sv +1 cycle L1 slip at epoch index 5 (e6 boundary)
    for j in range(5, len(rows["G02"])):
        rows["G02"][j]["L1"] += 1.0
        if j == 5:
            rows["G02"][j]["lli1"] = 1

    # G03: LLI false alarm at index 2 (e3), data untouched
    rows["G03"][2]["lli1"] = 1

    # G01: L2 missing at index 6,7 (e7,e8); +0.5 cycle L1 slip at index 6
    for j in (6, 7):
        rows["G01"][j]["present2"] = False
        rows["G01"][j]["L2"] = None
        rows["G01"][j]["P2"] = None
        rows["G01"][j]["D2"] = None
    for j in range(6, len(rows["G01"])):
        rows["G01"][j]["L1"] += 0.5
    rows["G01"][6]["lli1"] = 1

    # G01: P1-only +2.5 m code bump at index 7 (e8 boundary), persists after
    for j in range(7, len(rows["G01"])):
        rows["G01"][j]["P1"] += 2.5

    return rows, clock_events

def hash0(s):
    # stable tiny hash (avoid python hash() randomization)
    h = 0
    for ch in s:
        h = (h * 131 + ord(ch)) % (2**31)
    return h

def write_sheet(rows, clock_events, path):
    epochs_json = []
    for i, e in enumerate(EPOCHS):
        ce = None
        for (idx, jump) in clock_events:
            if idx == i:
                ce = {"declared": True, "jump_seconds": round(jump, 10)}
        sats = []
        for sv in ["G01", "G02", "G03"]:
            r = rows[sv][i]
            p = SVS[sv]
            obs = [
                {"code": "C1W", "value": round(r["P1"], 3), "lli": 0, "ssi": r["ssi"]},
                {"code": "C2W", "value": (round(r["P2"], 3) if r["P2"] is not None else None),
                 "diagnostic": ("missing-frequency: no L2 code at this epoch" if r["P2"] is None else None)},
                {"code": "L1W", "value": round(r["L1"], 4), "lli": r["lli1"], "ssi": r["ssi"]},
                {"code": "L2W", "value": (round(r["L2"], 4) if r["L2"] is not None else None),
                 "lli": r["lli2"], "ssi": r["ssi"],
                 "diagnostic": ("missing-frequency: no L2 phase at this epoch" if r["L2"] is None else None)},
                {"code": "D1W", "value": round(r["D1"], 4), "lli": 0, "ssi": r["ssi"]},
                {"code": "D2W", "value": (round(r["D2"], 4) if r["D2"] is not None else None),
                 "diagnostic": ("missing-frequency: no L2 doppler at this epoch" if r["D2"] is None else None)},
            ]
            for o in obs:
                if o.get("diagnostic") is None:
                    o.pop("diagnostic", None)
            sats.append({
                "sv": sv,
                "azimuth_deg": p["az"],
                "elevation_deg": p["el"],
                "ecef_m": list(p["ecef"]),
                "obs": obs,
            })
        epochs_json.append({
            "epoch": iso(e),
            "gps_seconds": round(gps_seconds(e), 4),
            "receiver_clock_seconds": round(CLOCK_STEP_S if i >= 4 else 0.0, 10),
            "clock_event": ce,
            "satellites": sats,
        })
    doc = {
        "format": "gsb-obs-sheet/1",
        "seed": SEED,
        "station": "BENCH",
        "epochs": epochs_json,
    }
    with open(path, "w") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")

def rinex_field(value, lli=0, ssi=6, missing=False):
    if missing or value is None:
        return " " * 16
    return f"{value:14.3f}{lli}{ssi}"

def write_rinex(rows, path):
    types = ["C1W", "C2W", "L1W", "L2W", "D1W", "D2W"]
    lines = []
    hdr = lambda body, label: f"{body:<60}{label:<20}"
    lines.append(hdr("3.04           OBSERVATION DATA    G: GPS", "RINEX VERSION / TYPE"))
    lines.append(hdr("gsb-bench", "PGM / RUN BY / DATE"))
    lines.append(hdr("BENCH", "MARKER NAME"))
    lines.append(hdr("  -2149098.9280  4373616.9280  4082010.6080", "APPROX POSITION XYZ"))
    lines.append(hdr(f"     {len(types)}    " + "".join(f"{t:>4}" for t in types), "# / TYPES OF OBSERV"))
    y, mo, d, h, mi, s = EPOCHS[0]
    lines.append(hdr(f"  {y:4d}    {mo:2d}    {d:2d}    {h:2d}    {mi:2d}   {s:11.7f}     GPS", "TIME OF FIRST OBS"))
    lines.append(hdr("", "END OF HEADER"))
    for i, e in enumerate(EPOCHS):
        y, mo, d, h, mi, s = e
        sstr = f"{s:11.7f}".strip()
        clk = CLOCK_STEP_S if i >= 4 else 0.0
        lines.append(
            f"> {y:4d} {mo:2d} {d:2d} {h:2d} {mi:2d} {sstr}  0  3    G01G02G03  {clk:13.10f}"
        )
        for sv in ["G01", "G02", "G03"]:
            r = rows[sv][i]
            fields = [
                rinex_field(r["P1"]),
                rinex_field(r["P2"], missing=not r["present2"]),
                rinex_field(r["L1"], lli=r["lli1"], ssi=r["ssi"]),
                rinex_field(r["L2"], lli=r["lli2"], ssi=r["ssi"], missing=not r["present2"]),
                rinex_field(r["D1"]),
                rinex_field(r["D2"], missing=not r["present2"]),
            ]
            lines.append("".join(fields))
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")

if __name__ == "__main__":
    rows, ce = build()
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = os.path.join(here, "cases", "carrier-break-bench")
    os.makedirs(out, exist_ok=True)
    write_sheet(rows, ce, os.path.join(out, "bench.sheet.json"))
    write_rinex(rows, os.path.join(out, "bench.obs"))
    print("wrote", out)
