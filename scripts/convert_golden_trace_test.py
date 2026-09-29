import csv
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import convert_golden_trace


FIRST_SWITCH_LEGACY_TIME = "9/28/2026 7:00:01 PM"
FIRST_SWITCH_UTC_TIME = "2026-09-28T19:00:01Z"


def write_csv(path: Path, rows: list[dict[str, str]]) -> None:
    with path.open("w", newline="", encoding="utf-8") as file:
        writer = csv.DictWriter(file, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


def static_row() -> dict[str, str]:
    return {
        "MvSignInit": "1",
        "NumCyclesCount": "1",
        "MvSwitchTimesList_0": "9/28/2026 7:00:00 PM",
        "MvSwitchTimesList_1": FIRST_SWITCH_LEGACY_TIME,
        "MvSwitchTimesList_2": "9/28/2026 7:00:02 PM",
        "MaxPVlist_0": "22.5",
        "MaxPVlist_1": "23.5",
        "MinPVlist_0": "18.5",
        "MinPVlist_1": "17.5",
        "RelayAmpPercent": "10.5",
        "NumCyclesSkip": "2",
        "NoiseProtDelay": "3",
        "MrftDelayTime": "4",
        "PvValueIni": "20.5",
        "MvValueIni": "50",
        "MvMSL": "0",
        "MvMSH": "100",
        "PvSH": "110",
        "PvSL": "10",
        "CalculatedMRFTperiodMinutes": "0.75",
        "CalculatedMRFTfrequency": "1.333333",
        "PvAmpRaw": "4",
        "PvAmpPercent": "4.5",
        "CalculatedKpAggressive": "1.1",
        "CalculatedTiMinutes": "2.2",
        "CalculatedTdMinutes": "0.3",
        "CalculatedPaggressive": "10.1",
        "CalculatedIaggressive": "10.2",
        "CalculatedDaggressive": "10.3",
        "CalculatedKpModerate": "2.1",
        "CalculatedPmoderate": "20.1",
        "CalculatedImoderate": "20.2",
        "CalculatedDmoderate": "20.3",
        "CalculatedKpSluggish": "3.1",
        "CalculatedPsluggish": "30.1",
        "CalculatedIsluggish": "30.2",
        "CalculatedDsluggish": "30.3",
    }


def dynamic_rows() -> list[dict[str, str]]:
    return [
        {
            "TimeCurrent": FIRST_SWITCH_LEGACY_TIME,
            "PvValueCurrent": "20.5",
            "Hysteresis": "0.1",
            "MvValueCurrent": "50",
            "MvSignNextStep": "1",
            "CounterAllSwitches": "0",
            "CyclesCompleted": "0",
            "CyclesRemaining": "1",
        },
        {
            "TimeCurrent": "9/28/2026 7:00:02 PM",
            "PvValueCurrent": "21.5",
            "Hysteresis": "0.2",
            "MvValueCurrent": "60",
            "MvSignNextStep": "-1",
            "CounterAllSwitches": "1",
            "CyclesCompleted": "0",
            "CyclesRemaining": "1",
        },
    ]


class ConvertGoldenTraceTests(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary_directory.name) / "repo"
        self.root.mkdir()
        self.capture_directory = self.root / "captures"
        self.capture_directory.mkdir()
        self.output_directory = self.root / "fixtures"
        self.output_directory.mkdir()
        self.static_path = self.capture_directory / "capture_20260928_191500_1d.csv"
        self.dynamic_path = self.capture_directory / "capture_20260928_191500_2d.csv"
        write_csv(self.static_path, [static_row()])
        write_csv(self.dynamic_path, dynamic_rows())

    def tearDown(self):
        self.temporary_directory.cleanup()

    def run_converter(self, direction: str, output_name: str):
        output_path = self.output_directory / output_name
        arguments = [
            "convert_golden_trace.py",
            "--static",
            self.static_path.relative_to(self.root).as_posix(),
            "--dynamic",
            self.dynamic_path.relative_to(self.root).as_posix(),
            "--name",
            "flow_pi_direct",
            "--process-type",
            "flow",
            "--controller-type",
            "pi",
            "--direction",
            direction,
            "--template",
            "Yokogawa CentumVP",
            "--out",
            output_path.relative_to(self.root).as_posix(),
            "--description",
            "boundary evidence",
            "--nudge-tick",
            "1=2026-09-28T19:00:02.500Z",
        ]
        output = io.StringIO()
        with (
            patch.object(convert_golden_trace, "REPOSITORY_ROOT", self.root),
            patch.object(sys, "argv", arguments),
            redirect_stdout(output),
        ):
            convert_golden_trace.main()
        fixture = json.loads(output_path.read_text(encoding="utf-8"))
        return fixture, output.getvalue()

    def test_parse_dt_converts_the_legacy_timestamp_to_utc_shape(self):
        self.assertEqual(
            convert_golden_trace.parse_dt(FIRST_SWITCH_LEGACY_TIME),
            FIRST_SWITCH_UTC_TIME,
        )

    def test_first_switch_peak_rule_uses_direction_and_initial_sign(self):
        self.assertTrue(convert_golden_trace.first_switch_is_peak(1, "reverse"))
        self.assertFalse(convert_golden_trace.first_switch_is_peak(1, "direct"))
        self.assertFalse(convert_golden_trace.first_switch_is_peak(-1, "reverse"))
        self.assertTrue(convert_golden_trace.first_switch_is_peak(-1, "direct"))

    def test_repository_path_rejects_escape_and_invalid_file_targets(self):
        outside_path = self.root.parent / "outside.csv"
        outside_path.write_text("outside", encoding="utf-8")
        with patch.object(convert_golden_trace, "REPOSITORY_ROOT", self.root):
            with self.assertRaisesRegex(ValueError, "must stay within the repository"):
                convert_golden_trace.repository_path(
                    str(outside_path), require_file=True
                )
            with self.assertRaisesRegex(ValueError, "Input path is not a file"):
                convert_golden_trace.repository_path("missing.csv", require_file=True)
            with self.assertRaisesRegex(ValueError, "Output directory does not exist"):
                convert_golden_trace.repository_path(
                    "missing/out.json", require_file=False
                )
            with self.assertRaisesRegex(ValueError, "Output path is not a file"):
                convert_golden_trace.repository_path("captures", require_file=False)

    def test_main_converts_logs_nudges_a_tick_and_writes_fixture_data(self):
        fixture, output = self.run_converter("reverse", "flow_pi_direct.json")

        self.assertEqual(fixture["name"], "flow_pi_direct")
        self.assertEqual(fixture["source"]["captured"], "2026-09-28")
        self.assertEqual(fixture["source"]["static_log"], self.static_path.name)
        self.assertEqual(fixture["direction"], "reverse")
        self.assertEqual(fixture["config"]["relay_amp_percent"], 10.5)
        self.assertEqual(fixture["initial"]["mv_range_high"], 100.0)
        self.assertEqual(fixture["pv_range"], {"high": 110.0, "low": 10.0})
        self.assertEqual(
            fixture["ticks"][0],
            {
                "time": FIRST_SWITCH_UTC_TIME,
                "pv": 20.5,
                "expected": {
                    "hysteresis": 0.1,
                    "mv_value_current": 50.0,
                    "mv_sign_next_step": 1,
                    "counter_all_switches": 0,
                    "cycles_completed": 0,
                    "cycles_remaining": 1,
                },
            },
        )
        self.assertEqual(fixture["ticks"][1]["time"], "2026-09-28T19:00:02.500Z")
        self.assertEqual(
            fixture["expected_final"]["switch_times"],
            [
                "2026-09-28T19:00:00Z",
                FIRST_SWITCH_UTC_TIME,
                "2026-09-28T19:00:02Z",
            ],
        )
        self.assertEqual(fixture["expected_final"]["peaks"], [22.5, 23.5])
        self.assertEqual(fixture["expected_final"]["troughs"], [18.5])
        self.assertEqual(
            fixture["expected_final"]["results"][0],
            {
                "response_level": "aggressive",
                "kp": 1.1,
                "ti_minutes": 2.2,
                "td_minutes": 0.3,
                "proportional": 10.1,
                "integral": 10.2,
                "derivative": 10.3,
            },
        )
        self.assertIn("boundary evidence", fixture["description"])
        self.assertIn("Ticks: 2", output)
        self.assertIn("first_switch_is_peak=True", output)

    def test_main_swaps_peak_and_trough_lengths_for_direct_action(self):
        fixture, output = self.run_converter("direct", "flow_pi_direct_raw.json")

        self.assertEqual(fixture["expected_final"]["peaks"], [22.5])
        self.assertEqual(fixture["expected_final"]["troughs"], [18.5, 17.5])
        self.assertIn("first_switch_is_peak=False", output)


if __name__ == "__main__":
    unittest.main()
