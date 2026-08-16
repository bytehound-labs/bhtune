#!/usr/bin/env python3
"""Convert a captured legacy FalconTune `--log --decryptedLog` CSV pair into a bhtune
golden-trace fixture (see `tests/golden/fixtures/` and `crates/bhtune-core/tests/golden_replay.rs`).

Usage:

    scripts/convert_golden_trace.py \
        --static tests/golden/raw/<name>_1d.csv \
        --dynamic tests/golden/raw/<name>_2d.csv \
        --name <fixture_name> \
        --process-type flow --controller-type pi \
        --template "Yokogawa CentumVP" \
        --out tests/golden/fixtures/<fixture_name>.json

Notes for whoever runs this against the next capture (see the `capture-traces` todo --
5 more process types, PID/temperature, reverse action, cascade, and varied skip/count are
still uncaptured):

- The static log (`_1d.csv`) is written twice by the legacy app -- once at test start, once
  (overwritten) at test end. Only the final copy carries the `Calculated*` results, so make
  sure the captured file is the final one, not a start-of-test snapshot.
- `direction` cannot be read off the trace's filename or the DCS convention it was captured
  against -- it must be independently derived from the recorded data. Compute
  `mv_sign_next_step` for the first tick under both `ControllerDirection::Direct`
  (action_multiplier=-1) and `Reverse` (action_multiplier=+1) using
  `MrftEngine::switch_is_needed`'s formula, and see which one matches the CSV's own
  `MvSignNextStep` for that tick. Cross-check against which of `CounterPeaks`/
  `CounterTroughs` increments first in the dynamic log -- both must agree.
- The raw CSV's `TimeCurrent`/`MvSwitchTimesList_N` timestamps have whole-second precision
  only (no milliseconds), even though the real poll cadence is sub-second. This is a genuine,
  irrecoverable precision ceiling of the legacy logger, not a bug in this script. It can
  create exact-tie ambiguities at any threshold comparison that depends on true sub-second
  timing (confirmed once, for `flow_pi_direct`'s tick 3 noise-protection boundary) -- resolve
  a genuine tie using independent evidence from the dynamic log's own counter columns (see
  `--nudge-tick`), and otherwise expect period-derived results (`ti_minutes`, `integral`) to
  carry up to ~1 second of reconstruction uncertainty from this same precision ceiling; the
  replay harness's tolerance for those two fields already accounts for it.
- peaks/troughs array lengths follow `bhtune_core::tuning_math::measure_oscillation`'s own
  discriminant, reproduced below (`first_switch_is_peak`) -- do not guess these by inspection.
"""

import argparse
import csv
import json
from datetime import datetime

ACTION_MULTIPLIER = {"direct": -1, "reverse": 1}


def parse_dt(value: str) -> str:
    """Legacy format: "8/12/2026 2:27:15 PM" (US locale, no leading zeros, no timezone).
    Only relative deltas between these matter for the tuning math, so the naive local
    timestamp is treated as UTC verbatim rather than resolved against a real timezone."""
    dt = datetime.strptime(value, "%m/%d/%Y %I:%M:%S %p")
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def f32(row: dict, key: str) -> float:
    return float(row[key])


def as_int(row: dict, key: str) -> int:
    return int(row[key])


def first_switch_is_peak(mv_sign_init: int, direction: str) -> bool:
    return mv_sign_init * ACTION_MULTIPLIER[direction] == 1


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--static", required=True, help="path to the final _1d.csv (static log)")
    parser.add_argument("--dynamic", required=True, help="path to the _2d.csv (dynamic, per-tick log)")
    parser.add_argument("--name", required=True, help="fixture name, e.g. flow_pi_direct")
    parser.add_argument("--process-type", required=True, help="bhtune ProcessType, snake_case, e.g. flow")
    parser.add_argument("--controller-type", required=True, help="bhtune ControllerType, snake_case, e.g. pi")
    parser.add_argument("--direction", choices=["direct", "reverse"], required=True,
                        help="ControllerDirection, independently derived -- see module docstring")
    parser.add_argument("--template", required=True, help="built-in template name, e.g. 'Yokogawa CentumVP'")
    parser.add_argument("--out", required=True, help="output fixture JSON path")
    parser.add_argument("--description", default="", help="extra free-text appended to the fixture description")
    parser.add_argument(
        "--nudge-tick",
        action="append",
        default=[],
        metavar="INDEX=ISO_TIMESTAMP",
        help="override one tick's timestamp (0-based index) to break a whole-second-rounding "
        "tie; only use with independent corroborating evidence (e.g. a counter column) that "
        "identifies which tick and which direction to nudge. May be repeated.",
    )
    args = parser.parse_args()

    with open(args.static) as f:
        static_row = next(csv.DictReader(f))
    with open(args.dynamic) as f:
        dynamic_rows = list(csv.DictReader(f))

    nudges = {}
    for spec in args.nudge_tick:
        index_str, timestamp = spec.split("=", 1)
        nudges[int(index_str)] = timestamp

    ticks = []
    for i, row in enumerate(dynamic_rows):
        ticks.append(
            {
                "time": nudges.get(i, parse_dt(row["TimeCurrent"])),
                "pv": f32(row, "PvValueCurrent"),
                "expected": {
                    "hysteresis": f32(row, "Hysteresis"),
                    "mv_value_current": f32(row, "MvValueCurrent"),
                    "mv_sign_next_step": as_int(row, "MvSignNextStep"),
                    "counter_all_switches": as_int(row, "CounterAllSwitches"),
                    "cycles_completed": as_int(row, "CyclesCompleted"),
                    "cycles_remaining": as_int(row, "CyclesRemaining"),
                },
            }
        )

    mv_sign_init = as_int(static_row, "MvSignInit")
    num_cycles_count = as_int(static_row, "NumCyclesCount")
    switch_times = [parse_dt(static_row[f"MvSwitchTimesList_{n}"]) for n in range(2 * num_cycles_count + 1)]

    # The capture filename convention is `<name>_<YYYYMMDD>_<HHMMSS>_1d/2d.csv` -- pull the
    # date back out for the fixture's own provenance record rather than requiring it as a
    # separate flag.
    stem = args.static.split("/")[-1]
    date_part = stem.rsplit("_", 3)[-3] if stem.count("_") >= 3 else ""
    capture_date = f"{date_part[0:4]}-{date_part[4:6]}-{date_part[6:8]}" if len(date_part) == 8 else "unknown"


    # MaxPVlist/MinPVlist are fixed-size 3-slot legacy arrays; only the meaningful entries are
    # real peaks/troughs, the rest is unused zero-padding. Lengths follow
    # `measure_oscillation`'s own discriminant -- see the module docstring.
    if first_switch_is_peak(mv_sign_init, args.direction):
        peaks_len, troughs_len = num_cycles_count + 1, num_cycles_count
    else:
        peaks_len, troughs_len = num_cycles_count, num_cycles_count + 1
    peaks = [f32(static_row, f"MaxPVlist_{n}") for n in range(peaks_len)]
    troughs = [f32(static_row, f"MinPVlist_{n}") for n in range(troughs_len)]

    description = (
        f"Real MRFT capture from legacy FalconTune.exe against the Python FOPDT simulator "
        f"on the hp Windows VM (see capture-traces)."
    )
    if args.description:
        description += " " + args.description

    fixture = {
        "name": args.name,
        "description": description,
        "source": {
            "static_log": args.static.split("/")[-1],
            "dynamic_log": args.dynamic.split("/")[-1],
            "captured": capture_date,
        },
        "config": {
            "process_type": args.process_type,
            "controller_type": args.controller_type,
            "relay_amp_percent": f32(static_row, "RelayAmpPercent"),
            "num_cycles_skip": as_int(static_row, "NumCyclesSkip"),
            "num_cycles_count": num_cycles_count,
            "noise_protection_secs": as_int(static_row, "NoiseProtDelay"),
            "mrft_delay_secs": as_int(static_row, "MrftDelayTime"),
        },
        "direction": args.direction,
        "initial": {
            "pv_ini": f32(static_row, "PvValueIni"),
            "mv_ini": f32(static_row, "MvValueIni"),
            "mv_range_low": f32(static_row, "MvMSL"),
            "mv_range_high": f32(static_row, "MvMSH"),
        },
        "pv_range": {
            "high": f32(static_row, "PvSH"),
            "low": f32(static_row, "PvSL"),
        },
        "template_name": args.template,
        "ticks": ticks,
        "expected_final": {
            "mv_sign_init": mv_sign_init,
            "switch_times": switch_times,
            "peaks": peaks,
            "troughs": troughs,
            # Not consumed by the replay harness (which recomputes these from peaks/troughs/
            # switch_times itself, as the actual behavioral proof) -- carried through purely
            # as a human-readable cross-check against the raw CSV's own recorded values.
            "oscillation": {
                "period_minutes": f32(static_row, "CalculatedMRFTperiodMinutes"),
                "frequency": f32(static_row, "CalculatedMRFTfrequency"),
                "pv_amp_raw": f32(static_row, "PvAmpRaw"),
                "pv_amp_percent": f32(static_row, "PvAmpPercent"),
            },
            "results": [
                {
                    "response_level": level,
                    "kp": f32(static_row, f"CalculatedKp{level.capitalize()}"),
                    "ti_minutes": f32(static_row, "CalculatedTiMinutes"),
                    "td_minutes": f32(static_row, "CalculatedTdMinutes"),
                    "proportional": f32(static_row, f"CalculatedP{level}"),
                    "integral": f32(static_row, f"CalculatedI{level}"),
                    "derivative": f32(static_row, f"CalculatedD{level}"),
                }
                for level in ("aggressive", "moderate", "sluggish")
            ],
        },
    }

    with open(args.out, "w") as f:
        json.dump(fixture, f, indent=2)
        f.write("\n")

    print(f"Wrote {args.out}")
    print(f"Ticks: {len(ticks)}")
    print(f"Direction: {args.direction} (first_switch_is_peak={first_switch_is_peak(mv_sign_init, args.direction)})")
    print(f"Switch times: {switch_times}")
    print(f"Peaks ({len(peaks)}): {peaks}")
    print(f"Troughs ({len(troughs)}): {troughs}")


if __name__ == "__main__":
    main()
