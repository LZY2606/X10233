#!/usr/bin/env python3
"""Deterministic offline case-pack generator.

Fixed seed, virtual clock. Produces cases/carrier-break-bench.json which is the
canonical "carrier-break bench" narrative material used by the workbench and by
the acceptance tests.

All raw observables are stored exactly as encoded (strings with units); the
Rust engine is the only place that normalises them.
"""
import json
import math
import os

ALGO = "carrier-break-v1"
SEED = 20260919
T0 = 0.0
# Unequally spaced epoch schedule (seconds since virtual origin).
SCHEDULE = [0.0, 30.0, 60.0, 95.0, 125.0, 155.0, 190.0, 220.0, 250.0,
            280.0, 310.0, 345.0, 375.0, 405.0]
CLOCK_JUMP_EPOCH = 6          # receiver clock jump entering this epoch
CLOCK_JUMP_M = 1000.0         # whole-receiver jump, metres of range
G01_SLIP = {"at_epoch": 5, "band": "L1", "cycles": 1.0}
G04_SLIP = {"at_epoch": 10, "band": "L1", "cycles": 1.0}
G04_MISSING = {8, 9}
G02_MISSING_L2 = {8, 9}
G03_LLI_FALSE = {8}

C = 299792458.0
F1 = 1.57542e9
F2 = 1.22760e9
WL1 = C / F1
WL2 = C / F2

# Station (Tokyo datum example), ECEF metres.
STATION = {
    "id": "CBS",
    "marker": "CARRIER-BREAK-BENCH",
    "datum": "ITRF2020 (ECEF, metres)",
    "xyz_m": [-3957430.983, 3310271.129, 3737378.412],
}

# Deterministic circular LEO-like satellite ground tracks around station.
SATS = {}
for i, name in enumerate(["G01", "G02", "G03", "G04"]):
    phase = i * math.pi / 2.0
    SATS[name] = {
        "id": name,
        "system": "G",
        "orbit_radius_m": 2.02e7,
        "omega_rps": 5.0e-5 + i * 3.7e-7,
        "phase0": phase,
        "incl_rad": 0.15 * (i - 1.5),
        "iono_a_m": 6.0 + 1.5 * i,          # I(t) = a + b t
        "iono_b_mps": 0.002 * (i - 1.5),
        "n1_0": 12345000 + 1000 * i,
        "n2_0": 9500000 + 700 * i,
        "snr_dbhz": {"L1": 46 - i, "L2": 42 - i},
    }

BANDS = [
    {"code": "L1", "freq_hz": F1, "wavelength_m": WL1,
     "pseudo_code": "C1C", "phase_code": "L1C", "doppler_code": "D1C"},
    {"code": "L2", "freq_hz": F2, "wavelength_m": WL2,
     "pseudo_code": "C2W", "phase_code": "L2W", "doppler_code": "D2W"},
]


def sat_xyz(sat, t):
    o = SATS[sat]
    r = o["orbit_radius_m"]
    ang = o["phase0"] + o["omega_rps"] * t
    incl = o["incl_rad"]
    return (r * math.cos(ang),
            r * math.sin(ang) * math.cos(incl),
            r * math.sin(ang) * math.sin(incl))


def geom(sat, t):
    sx, sy, sz = sat_xyz(sat, t)
    rx, ry, rz = STATION["xyz_m"]
    return math.sqrt((sx - rx) ** 2 + (sy - ry) ** 2 + (sz - rz) ** 2)


def iono(sat, t):
    o = SATS[sat]
    return o["iono_a_m"] + o["iono_b_mps"] * t


def clock_m(t_epoch_idx):
    return CLOCK_JUMP_M if t_epoch_idx >= CLOCK_JUMP_EPOCH else 0.0

def phase_cycles(sat, band, idx, t):
    o = SATS[sat]
    g = geom(sat, t) + iono(sat, t) + clock_m(idx)
    wl = WL1 if band == "L1" else WL2
    n0 = o["n1_0"] if band == "L1" else o["n2_0"]
    cyc = n0 - g / wl
    if sat == "G01" and idx >= G01_SLIP["at_epoch"] and band == "L1":
        cyc += G01_SLIP["cycles"]
    if sat == "G04" and idx >= G04_SLIP["at_epoch"] and band == "L1":
        cyc += G04_SLIP["cycles"]
    return cyc


def doppler_hz(sat, band, idx, t):
    # Instantaneous range-rate via central difference, Doppler sign follows
    # RINEX convention (positive when satellite approaches).
    h = 0.5
    dr = (geom(sat, t - h) - geom(sat, t + h)) / (2.0 * h)
    rate = -(dr + iono_deriv(sat))
    f = F1 if band == "L1" else F2
    d = rate / C * f
    if sat == "G01" and idx == G01_SLIP["at_epoch"]:
        # instantaneous impulse never visible in continuous Doppler
        pass
    return d


def iono_deriv(sat):
    return SATS[sat]["iono_b_mps"]


def pseudo_m(sat, idx, t):
    return geom(sat, t) + iono(sat, t) + clock_m(idx)


def lli_flag(sat, idx):
    if sat == "G03" and idx in G03_LLI_FALSE:
        return "1"
    return "0"


def band_present(sat, idx, band):
    if sat == "G04" and idx in G04_MISSING:
        return False
    if sat == "G02" and band == "L2" and idx in G02_MISSING_L2:
        return False
    return True


def make_epochs():
    epochs = []
    for idx, t in enumerate(SCHEDULE):
        clock_events = []
        if idx == CLOCK_JUMP_EPOCH:
            clock_events.append({
                "event_id": f"clk-CBS-{idx:02d}",
                "kind": "receiver_clock_jump",
                "scope": "receiver",
                "raw_magnitude": "+1000.000 m (pseudo-range + phase, all sats)",
                "declared_at_epoch": f"e{idx:02d}",
            })
        rows = []
        for sat in SATS:
            for band in ("L1", "L2"):
                if not band_present(sat, idx, band):
                    continue
                wl = WL1 if band == "L1" else WL2
                fq = F1 if band == "L1" else F2
                bc = BANDS[0] if band == "L1" else BANDS[1]
                cyc = phase_cycles(sat, band, idx, t)
                dop = doppler_hz(sat, band, idx, t)
                rows.append({
                    "obs_id": f"{sat}-{band}-e{idx:02d}",
                    "sat": sat,
                    "band": band,
                    "freq_code": band,
                    "pseudo_code": bc["pseudo_code"],
                    "phase_code": bc["phase_code"],
                    "doppler_code": bc["doppler_code"],
                    "pseudo_raw": f"{pseudo_m(sat, idx, t):.3f} m",
                    "phase_raw": f"{cyc:.4f} cycle",
                    "doppler_raw": f"{dop:.4f} Hz",
                    "lli": lli_flag(sat, idx) if band == "L1" else "0",
                    "snr_raw": f"{SATS[sat]['snr_dbhz'][band]} dB-Hz",
                    "wavelength_raw": f"{wl:.10f} m",
                    "freq_raw": f"{fq:.1f} Hz",
                })
        epochs.append({
            "epoch_id": f"e{idx:02d}",
            "virtual_time_s": t,
            "virtual_clock": "2026-01-01T00:%02d:%06.3fZ" % (int(t // 60), t %% 60),
            "clock_offset_raw": (f"+{(clock_m(idx)/C):.10e} s"
                                 if clock_m(idx) else "0 s"),
            "clock_events": clock_events,
            "observations": rows,
        })
    return epochs

def omissions(epochs):
    out = []
    for ep in epochs:
        idx = int(ep["epoch_id"][1:])
        if idx in G04_MISSING:
            out.append({"epoch_id": ep["epoch_id"], "sat": "G04",
                        "bands": ["L1", "L2"], "reason": "satellite below mask / no tracking"})
        if idx in G02_MISSING_L2:
            out.append({"epoch_id": ep["epoch_id"], "sat": "G02",
                        "bands": ["L2"], "reason": "L2 tracking loss, L1 kept"})
    return out


def build_revisions(base_epochs):
    # b01: parameter-only revision is expressed as a preset, not an input edit.
    # b02: local source edit - G02 L1 phase at e08 gains +1 cycle, other rows
    # byte-identical to baseline (patch layer records the one changed source).
    changed = "G02-L1-e08"
    new_epochs = json.loads(json.dumps(base_epochs))
    for ep in new_epochs:
        for row in ep["observations"]:
            if row["obs_id"] == changed:
                v = float(row["phase_raw"].split()[0]) + 1.0
                row["phase_raw"] = f"{v:.4f} cycle"
    patches = [{
        "patch_id": "patch-b02-G02-L1-e08",
        "obs_id": changed,
        "field": "phase_raw",
        "from": next(r["phase_raw"] for e in base_epochs for r in e["observations"]
                     if r["obs_id"] == changed),
        "to": next(r["phase_raw"] for e in new_epochs for r in e["observations"]
                   if r["obs_id"] == changed),
        "reason": "re-survey log: L1 phase at e08 corrected by +1 cycle (local edit)",
    }]
    return new_epochs, patches


PARAMS_V1 = {
    "max_gap_s": 120.0,
    "clock_step_m": 800.0,
    "td_residual_cycles": 0.75,
    "gf_residual_m": 0.10,
    "mw_residual_wl": 0.75,
}
PARAMS_V2 = dict(PARAMS_V1)
PARAMS_V2.update({"mw_residual_wl": 1.10, "td_residual_cycles": 0.90})

# Machine-readable narrative / acceptance expectations. Tolerances are loose
# enough for f64 storage of raw strings but the physics is deterministic.
SCRIPT = {
    "case_id": "carrier-break-bench",
    "title": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
    "fixed_seed": SEED,
    "virtual_time": {"origin": "2026-01-01T00:00:00Z", "epoch_seconds": SCHEDULE},
    "principles": [
        "ionosphere / Melbourne-Wubbena / time-difference are independent detectors; none is truth",
        "whole-receiver clock jump is separated from per-satellite cycle slips",
        "repair lives only in an offset layer; raw inputs are immutable",
        "local source edits expire only dependent arcs; stale results stay viewable",
    ],
    "expected": {
        "clock": {"at_epoch": "e06", "scope": "receiver", "step_m_about": 1000.0,
                  "must_not_create_per_sat_slip": True},
        "slips": [
            {"sat": "G01", "boundary_epoch": "e05", "bands": {"L1": 1},
             "detectors": ["TD", "GF", "MW"]},
            {"sat": "G02", "boundary_epoch": "e08", "bands": {"L1": 1},
             "note": "post-gap L1, L2 missing -> single-band candidate",
             "detectors": ["TD", "GF", "MW"], "after_batch": "b02"},
            {"sat": "G04", "boundary_epoch": "e10", "bands": {"L1": 1},
             "detectors_v1": ["GF", "MW"], "unique_v1": True,
             "tied_v2": True},
        ],
        "lli_false_positive": {"sat": "G03", "epoch": "e08"},
        "missing_frequency": {"sat": "G02", "epochs": ["e08", "e09"], "band": "L2"},
        "unequal_gap_sat": {"sat": "G04", "missing_epochs": ["e08", "e09"]},
        "local_edit": {"obs_id": "G02-L1-e08", "field": "phase_raw",
                       "newly_expired_sat": "G02", "unaffected_sats": ["G01", "G03", "G04"]},
    },
    "stages": [
        {"id": "s0-baseline", "kind": "import", "input_version": "b00",
         "params_version": "p01", "narrative": "import baseline; verify all raw fields epoch-faithful"},
        {"id": "s1-params", "kind": "params", "params_version": "p02",
         "narrative": "parameter revision; G04 return-boundary becomes multi-candidate tie"},
        {"id": "s2-local-edit", "kind": "input", "input_version": "b02",
         "narrative": "local source edit on one G02 row; only G02 arcs expire"},
        {"id": "s3-conflict", "kind": "conflict",
         "narrative": "main accepts G04 offset; review fork rejects it; both histories replay"},
    ],
}

def main():
    epochs = make_epochs()
    rev_epochs, patches = build_revisions(epochs)
    pack = {
        "format": "carrier-break-case/1",
        "case": SCRIPT,
        "station": STATION,
        "bands": BANDS,
        "satellites": [SATS[s] for s in SATS],
        "units_conventions": {
            "pseudo": "metre", "phase": "cycle", "doppler": "hertz",
            "snr": "dB-Hz", "time": "seconds relative to virtual origin (UTC)",
            "coordinates": "ECEF metre, ITRF2020",
            "doppler_sign": "RINEX: positive when satellite approaches receiver",
        },
        "baseline": {
            "batch_id": "b00",
            "parent_batch_id": None,
            "kind": "baseline",
            "algorithm_lineage": ALGO,
            "epochs": epochs,
            "omissions": omissions(epochs),
        },
        "revisions": [
            {
                "batch_id": "b02",
                "parent_batch_id": "b00",
                "kind": "local_source_edit",
                "epochs": rev_epochs,
                "patches": patches,
            }
        ],
        "param_presets": [
            {"params_id": "p01", "parent_params_id": None, "params": PARAMS_V1},
            {"params_id": "p02", "parent_params_id": "p01", "params": PARAMS_V2},
        ],
        "malformed_examples": [
            {"id": "bad-epoch-time", "reason": "epoch virtual_time_s is not a number"},
            {"id": "bad-phase-unit", "reason": "phase_raw missing '<float> cycle' unit suffix"},
            {"id": "bad-band-code", "reason": "band not declared in bands[] (e.g. L5)"},
        ],
    }
    here = os.path.dirname(os.path.abspath(__file__))
    out = os.path.join(here, "carrier-break-bench.json")
    with open(out, "w", encoding="utf-8") as fh:
        json.dump(pack, fh, ensure_ascii=False, indent=2, sort_keys=False)
        fh.write("\n")
    print("wrote", out, "epochs", len(epochs), "rows",
          sum(len(e["observations"]) for e in epochs))


if __name__ == "__main__":
    main()
