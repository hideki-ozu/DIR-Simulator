"""Focused, filesystem-isolated tests for the DIR / FiCo comparison adapter."""

import csv
import importlib.util
import json
import tempfile
import unittest
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "tools/omnet_comparison/compare.py"
SPEC = importlib.util.spec_from_file_location("omnet_comparison", SCRIPT)
comparison = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(comparison)


def write_events(path, rows):
    fields = (
        "event", "time_ps", "node", "request_id", "source", "format", "can_id",
        "payload_hex", "native_bits", "queue_waiting",
    )
    with path.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


def native_row(event, time_ps, *, node="Main.a", request_id="a:0", source="Main.a",
               frame_format="standard", can_id=0, payload_hex=""):
    return {
        "event": event,
        "time_ps": str(time_ps),
        "node": node,
        "request_id": request_id,
        "source": source,
        "format": frame_format,
        "can_id": str(can_id),
        "payload_hex": payload_hex,
        "native_bits": "80",
        "queue_waiting": "",
    }


def valid_native_rows(*, ready_ps=0, enqueue_ps=0):
    complete = 106_000_000
    rows = [
        native_row("generated", 0),
        native_row("ready", ready_ps),
        native_row("enqueued", enqueue_ps),
        native_row("sof", 10_000_000),
        native_row("native_complete", complete),
    ]
    for node in ("Main.b", "Main.c"):
        rows.extend([
            native_row("native_rx_complete", complete, node=node),
            native_row("observed", complete, node=node),
            native_row("received", complete, node=node),
        ])
    return rows


def dir_result():
    return {
        "simulation": {
            "termination": "events_exhausted",
            "end_ps": 250_000_000_000,
            "requests": [{
                "request_id": "a:0",
                "source": "Main.a",
                "status": "success",
                "generated_ps": 0,
                "ready_ps": 0,
                "sof_ps": 10_000_000,
                "eof_ps": 100_000_000,
                "model_fields": {
                    "release_ps": 106_000_000,
                    "planned_eof_ps": 100_000_000,
                    "planned_release_ps": 106_000_000,
                },
                "serialized_bits": 80,
            }],
            "receivers": [{
                "request_id": "a:0",
                "receiver": "Main.b",
                "status": "received",
                "observed_ps": 106_000_000,
                "received_ps": 106_000_000,
            }, {
                "request_id": "a:0",
                "receiver": "Main.c",
                "status": "received",
                "observed_ps": 106_000_000,
                "received_ps": 106_000_000,
            }],
            "records": [],
        },
    }


class OmnetComparisonTests(unittest.TestCase):
    def setUp(self):
        self.condition = comparison.settings(comparison.FIXTURES / "competition.ini")

    def test_exact_quantities_reject_sub_picosecond_values(self):
        self.assertEqual(comparison.quantity("1.25us", comparison.TIME_UNITS), 1_250_000)
        self.assertEqual(comparison.quantity("333333bps", comparison.RATE_UNITS), 333_333)
        with self.assertRaises(ValueError):
            comparison.quantity("0.1ps", comparison.TIME_UNITS)

    def test_suite_includes_eight_canonical_scenarios_and_three_examples(self):
        cases = comparison.suite()
        canonical = [case for case in cases if case["group"] == "canonical"]
        examples = [case for case in cases if case["group"] == "example"]

        self.assertEqual(len(canonical), 8)
        self.assertEqual(len(examples), 3)
        self.assertEqual(
            [case["name"] for case in examples],
            ["baseline", "contention", "overload"],
        )
        self.assertTrue(all(case["config"].is_file() for case in canonical + examples))

    def test_materialized_fixtures_and_native_inputs_preserve_raw_workloads(self):
        cases = [case for case in comparison.suite() if case["group"] in ("canonical", "example")]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            poison = root / "dir-output/results.json"
            poison.parent.mkdir()
            poison.write_text('{"must_not_be_used_as_input": true}', encoding="utf-8")
            (root / "frozen").mkdir()
            (root / "native").mkdir()

            for case in cases:
                with self.subTest(case=case["name"]):
                    original = comparison.settings(case["config"])
                    frozen_ini = comparison.materialize(case, root / "frozen" / case["name"])
                    frozen = comparison.settings(frozen_ini)
                    for key in ("horizon_ps", "bitrate", "nodes", "workload"):
                        self.assertEqual(frozen[key], original[key])

                    destination = root / "native" / case["name"]
                    ini = comparison.omnet_inputs(frozen, destination)
                    offered = comparison.offered_frames(frozen)
                    self.assertEqual(
                        json.loads((destination / "offered.json").read_text(encoding="utf-8")),
                        offered,
                    )

                    tsv_rows = []
                    for index, node in enumerate(frozen["nodes"]):
                        with (destination / f"node-{index}.tsv").open(newline="", encoding="utf-8") as stream:
                            tsv_rows.extend(
                                (node["label"], row["generation_ps"], row["request_id"], row["format"],
                                 int(row["can_id"]), row["payload_hex"].replace("-", ""))
                                for row in csv.DictReader(stream, delimiter="\t")
                            )
                    expected_rows = [
                        (row["source"], str(row["generated_ps"]), row["request_id"], row["format"],
                         row["can_id"], row["payload_hex"])
                        for row in offered
                    ]
                    self.assertCountEqual(tsv_rows, expected_rows)
                    self.assertNotIn(str(poison), ini.read_text(encoding="utf-8"))
                    self.assertFalse((destination / "dir").exists())

    def test_periodic_phase_end_count_and_horizon_cutoffs(self):
        condition = dict(self.condition, horizon_ps=10)
        frame = {"format": "standard", "id": 1, "data": "aa"}
        condition["workload"] = {
            "schema_version": 1,
            "generators": [
                {"id": "end", "kind": "can.periodic.v1", "node": "Main.a",
                 "start": "0ps", "phase": "2ps", "period": "3ps", "end": "9ps", "count": 8,
                 "frame": frame},
                {"id": "count", "kind": "can.periodic.v1", "node": "Main.a",
                 "start": "1ps", "period": "2ps", "count": 3, "frame": frame},
                {"id": "horizon", "kind": "can.periodic.v1", "node": "Main.a",
                 "start": "1ps", "period": "4ps", "frame": frame},
            ],
        }

        frames = comparison.offered_frames(condition)
        by_generator = defaultdict(list)
        for frame_row in frames:
            by_generator[frame_row["generator_id"]].append(frame_row["generated_ps"])

        self.assertEqual(by_generator["end"], [2, 5, 8])  # phase applied; end is exclusive
        self.assertEqual(by_generator["count"], [1, 3, 5])
        self.assertEqual(by_generator["horizon"], [1, 5, 9])  # implicit periodic end is H

    def test_explicit_same_time_ordinals_sort_numerically(self):
        condition = dict(self.condition, horizon_ps=100)
        condition["workload"] = {
            "schema_version": 1,
            "generators": [{
                "id": "same-time", "kind": "can.explicit.v1", "node": "Main.a",
                "times": ["5ps"] * 12,
                "frame": {"format": "standard", "id": 1, "data": ""},
            }],
        }

        frames = comparison.offered_frames(condition)

        self.assertEqual([row["request_id"] for row in frames], [f"same-time:{i}" for i in range(12)])
        self.assertLess(
            [row["request_id"] for row in frames].index("same-time:2"),
            [row["request_id"] for row in frames].index("same-time:10"),
        )

    def test_materialized_mixed_format_mutations_and_zero_horizon(self):
        cases = {case.get("mutation"): case for case in comparison.suite() if case.get("mutation")}
        expected = {
            "mixed-format-extended-wins": {"a:0": ("standard", 0x123), "b:0": ("extended", 0x123)},
            "mixed-format-standard-wins": {"a:0": ("standard", 0x123), "b:0": ("extended", 0x048C0000)},
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for mutation, wanted in expected.items():
                with self.subTest(mutation=mutation):
                    ini = comparison.materialize(cases[mutation], root / mutation)
                    condition = comparison.settings(ini)
                    actual = {
                        row["request_id"]: (row["format"], row["can_id"])
                        for row in comparison.offered_frames(condition)
                    }
                    self.assertEqual({key: actual[key] for key in wanted}, wanted)

            zero_ini = comparison.materialize(cases["zero-horizon"], root / "zero-horizon")
            zero = comparison.settings(zero_ini)
            self.assertEqual(zero["horizon_ps"], 0)
            self.assertTrue(comparison.offered_frames(zero))  # raw offers remain in the frozen inputs
            native_csv = root / "zero-horizon.csv"
            write_events(native_csv, [])
            projection, _ = comparison.parse_omnet(native_csv, zero)
            self.assertEqual(projection["counts"]["generated"], 0)
            self.assertEqual(projection["counts"]["attempts"], 0)

    def test_settings_reject_unknown_general_and_channel_settings(self):
        case = next(case for case in comparison.suite() if case["name"] == "baseline")
        with tempfile.TemporaryDirectory() as temporary:
            for kind in ("general", "channel"):
                with self.subTest(kind=kind):
                    ini = comparison.materialize(case, Path(temporary) / kind)
                    config = comparison.read_ini(ini)
                    if kind == "general":
                        config["General"]["Main.bus.unmappedSetting"] = "1"
                    else:
                        config.add_section("Channel Main::a.tx")
                        config["Channel Main::a.tx"]["delay"] = "1ps"
                        config["Channel Main::a.tx"]["extra"] = "unsupported"
                    with ini.open("w", encoding="utf-8") as stream:
                        config.write(stream)
                    with self.assertRaises(ValueError):
                        comparison.settings(ini)

    def test_queue_area_integrates_through_horizon_and_handles_zero_and_ties(self):
        queues = comparison.queue_areas(
            [(5, "Main.a", 3), (5, "Main.a", 4), (10, "Main.a", 2), (15, "Main.a", 5)],
            ["Main.a", "Main.b"],
            15,
        )

        self.assertEqual(queues["Main.a"]["area_packet_ps"], "30")
        self.assertEqual(queues["Main.a"]["mean"]["numerator"], "2")
        self.assertEqual(queues["Main.a"]["mean"]["denominator"], "1")
        self.assertEqual(queues["Main.a"]["peak"], 5)
        self.assertEqual(queues["Main.a"]["final"], 5)
        self.assertEqual(queues["Main.b"]["area_packet_ps"], "0")
        self.assertEqual(queues["Main.b"]["mean"]["numerator"], "0")
        self.assertEqual(queues["Main.b"]["mean"]["denominator"], "1")

        zero = comparison.queue_areas([], ["Main.a"], 0)["Main.a"]
        self.assertEqual(zero["area_packet_ps"], "0")
        self.assertIsNone(zero["mean"])

    def test_valid_native_trace_is_accepted(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "events.csv"
            write_events(path, valid_native_rows())

            projection, events = comparison.parse_omnet(path, self.condition)

        self.assertEqual(projection["counts"]["generated"], 1)
        self.assertEqual(projection["counts"]["success"], 1)
        self.assertEqual(projection["counts"]["received"], 2)
        self.assertEqual(len(events), 11)

    def test_parse_omnet_rejects_out_of_order_and_duplicate_native_events(self):
        cases = {
            "time moves backwards": lambda rows: rows[5].update(time_ps="105999999"),
            "duplicate SOF": lambda rows: rows.insert(4, dict(rows[3])),
            "duplicate native receive": lambda rows: rows.insert(6, dict(rows[5])),
            "duplicate receive": lambda rows: rows.append(dict(rows[-1])),
        }
        with tempfile.TemporaryDirectory() as temporary:
            for label, mutate in cases.items():
                with self.subTest(label=label):
                    rows = valid_native_rows()
                    mutate(rows)
                    path = Path(temporary) / "events.csv"
                    write_events(path, rows)
                    with self.assertRaises(ValueError):
                        comparison.parse_omnet(path, self.condition)

    def test_parse_omnet_rejects_frame_mutation_after_generation(self):
        mutations = {
            "source": "Main.c",
            "format": "extended",
            "can_id": "2",
            "payload_hex": "ccdd",
        }
        with tempfile.TemporaryDirectory() as temporary:
            for field, value in mutations.items():
                with self.subTest(field=field):
                    rows = valid_native_rows()
                    rows[3][field] = value  # first mutation is at SOF, after generated
                    path = Path(temporary) / "events.csv"
                    write_events(path, rows)
                    with self.assertRaises(ValueError):
                        comparison.parse_omnet(path, self.condition)

    def test_dir_eof_and_native_completion_stay_distinct_and_wrong_trace_is_reported(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            dir_projection = comparison.parse_dir(dir_result(), self.condition)
            self.assertEqual(dir_projection["requests"]["a:0"]["eof_ps"], 100_000_000)
            self.assertEqual(dir_projection["requests"]["a:0"]["release_ps"], 106_000_000)

            events_path = directory / "events.csv"
            write_events(events_path, valid_native_rows())
            native_projection, _ = comparison.parse_omnet(events_path, self.condition)
            native_request = native_projection["requests"]["a:0"]
            self.assertEqual(native_request["native_complete_ps"], 106_000_000)
            self.assertNotIn("eof_ps", native_request)
            native_request["ready_ps"] = 5  # inject a projection-only timing mismatch

            summary = comparison.compare_projections(directory, dir_projection, native_projection)
            self.assertFalse(summary["common_projection_equal"])
            self.assertEqual(summary["common_trace_differences"], 1)
            report = json.loads((directory / "comparison.json").read_text(encoding="utf-8"))
            self.assertEqual(report["differences"][0]["field"], "ready_ps")
            with (directory / "requests.csv").open(newline="", encoding="utf-8") as stream:
                header = next(csv.reader(stream))
            self.assertIn("dir_eof_ps", header)
            self.assertIn("dir_release_ps", header)
            self.assertIn("omnet_native_complete_ps", header)
            self.assertNotIn("omnet_eof_ps", header)


if __name__ == "__main__":
    unittest.main()
