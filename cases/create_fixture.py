#!/usr/bin/env python3
import json
from pathlib import Path

L1 = 0.1902936728
L2 = 0.2442102134
RATE = 1000.0
C0 = 2_000_000.0
CLOCK_M = 29.9792458

epochs = [
    ("E01", "2026-09-27T00:00:00Z", 0.0),
    ("E02", "2026-09-27T00:00:10Z", 10.0),
    ("E03", "2026-09-27T00:00:25Z", 25.0),
    ("E04", "2026-09-27T00:00:40Z", 40.0),
]
sats = ["G01", "G02", "G03"]

rows = []
def add(e, sat, code, freq, raw, unit, lli=None, snr=None):
    rows.append([e, sat, code, freq, f"{raw:.1f}" if isinstance(raw, float) else raw, unit, lli, snr])

for eid, _, t in epochs:
    clock = CLOCK_M if eid == "E04" else 0.0
    for sat in sats:
        code = C0 + RATE * t + clock
        l1 = -code / L1
        l2 = -code / L2
        if eid == "E04" and sat == "G02":
            l1 += 1.0
        if eid == "E04" and sat == "G03":
            l1 += 0.5
        lli_l1 = 1 if eid == "E04" and sat == "G01" else 0
        snr_g01_e03_s1 = "LOST" if eid == "E03" and sat == "G01" else f"{43.0 - ord(sat[-1]) % 3:.1f}"
        add(eid, sat, "C1W", 1, code, "m")
        add(eid, sat, "L1W", 1, l1, "cycles", lli=lli_l1)
        add(eid, sat, "D1W", 1, -RATE / L1, "Hz")
        add(eid, sat, "S1W", 1, snr_g01_e03_s1, "dB-Hz")
        if eid == "E03" and sat == "G01":
            continue
        add(eid, sat, "C2W", 2, code, "m")
        add(eid, sat, "L2W", 2, l2, "cycles", lli=0)
        add(eid, sat, "D2W", 2, -RATE / L2, "Hz")
        add(eid, sat, "S2W", 2, f"{40.0 - ord(sat[-1]) % 3:.1f}", "dB-Hz")

package = {
    "schema_version": "carrier-break-case/v1",
    "case_id": "carrier-break-tour-v1",
    "title": "载波断点台：GNSS 周跳候选、多频组合与弧段修订史",
    "narrative": "测量团队导入小型 RINEX 观测历元与卫星状态，按站-星-频率弧段审阅周跳候选、修复偏移和冲突决定。",
    "coordinate_reference": "ITRF2014",
    "time_scale": "UTC virtual laboratory clock",
    "seed": 20260927,
    "station": {
        "id": "STA01",
        "name": "Station Alpha",
        "receiver": "Virtual RINEX Receiver",
        "antenna": "Virtual Antenna",
        "position_m": [4_000_000.0, 300_000.0, 4_900_000.0],
        "coordinate_reference": "ITRF2014",
    },
    "frequencies": [
        {"constellation": "GPS", "frequency_number": 1, "code": "1W", "carrier_hz": 1575420000.0, "wavelength_m": L1},
        {"constellation": "GPS", "frequency_number": 2, "code": "2W", "carrier_hz": 1227600000.0, "wavelength_m": L2},
    ],
    "observation_columns": ["epoch_id", "satellite", "observation_code", "frequency_number", "raw_text", "unit", "lli", "snr_dBHz_text"],
    "observations": rows,
    "epochs": [{"id": eid, "timestamp": ts, "virtual_seconds": t} for eid, ts, t in epochs],
    "satellites": [{"id": sat, "constellation": "GPS", "prn": int(sat[1:])} for sat in sats],
    "satellite_states": [
        {
            "epoch_id": eid,
            "satellite": sat,
            "position_m": [1.0e7 + ord(sat[-1]) * 1000.0, 2.0e6 + t * 10.0, 1.5e7],
            "velocity_mps": [0.0, 10.0, 0.0],
            "coordinate_reference": "ECEF/ITRF2014",
            "clock_bias_s": 0.0,
            "health": "healthy",
        }
        for eid, _, t in epochs for sat in sats
    ],
    "clock_events": [
        {
            "id": "CLK-E04-receiver",
            "epoch_id": "E04",
            "scope": "receiver",
            "delta_seconds": 1.0e-7,
            "delta_m": CLOCK_M,
            "source_text": "RECEIVER_CLOCK_JUMP +1E-7 s",
            "applies_to": "all satellites and code/carrier channels",
        }
    ],
    "initial_parameters": {
        "algorithm_version": "carrier-break-engine-2026.09.1",
        "iono_cycles_threshold": 0.35,
        "mw_cycles_threshold": 0.45,
        "time_difference_cycles_threshold": 0.45,
        "lli_score_weight": 0.35,
        "tie_half_cycle": 0.5,
    },
}

Path(__file__).with_name("carrier-break-tour-v1.json").write_text(
    json.dumps(package, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)
