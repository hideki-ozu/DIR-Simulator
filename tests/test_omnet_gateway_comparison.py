"""Regression tests for raw OMNeT++ Gateway log validation."""

import copy
import csv
import json
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests/fixtures/omnet_gateway"
if not (ROOT / "tools/omnet_comparison/gateway_compare.py").is_file():
    raise unittest.SkipTest("OMNeT++ comparison tools are maintained outside this repository")
sys.path.insert(0, str(ROOT / "tools/omnet_comparison"))
import gateway_compare  # noqa: E402


def load_fixture(name):
    return json.loads((FIXTURES / f"{name}.json").read_text(encoding="utf-8"))


def fixture_events(fixture):
    return [dict(zip(fixture["fields"], row)) for row in fixture["rows"]]


def write_events(path, fixture, events):
    with path.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=fixture["fields"])
        writer.writeheader()
        for sequence, event in enumerate(events):
            event["sequence"] = str(sequence)
            writer.writerow(event)


class OmnetGatewayComparisonTests(unittest.TestCase):
    def parse_events(self, fixture, events):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "events.csv"
            write_events(path, fixture, events)
            return gateway_compare.parse_omnet(path, copy.deepcopy(fixture["condition"]))

    def assert_events_rejected(self, fixture, events):
        with self.assertRaises(ValueError):
            self.parse_events(fixture, events)

    def test_captured_fico_logs_parse_and_match_dir_configuration(self):
        for name in ("gw-queue", "gw-independent", "gw-multicast-first-ready"):
            with self.subTest(name=name):
                fixture = load_fixture(name)
                projection, events = self.parse_events(fixture, fixture_events(fixture))
                self.assertEqual(len(events), len(fixture["rows"]))
                self.assertGreater(projection["counts"]["generated"], 0)
                gateway_compare.check_dir_configuration(
                    {"metadata": fixture["dir_metadata"]},
                    copy.deepcopy(fixture["condition"]),
                )

    def test_default_gateway_hop_limit_matches_recorded_dir_metadata(self):
        fixture = load_fixture("gw-queue")
        case = next(case for case in gateway_compare.suite() if case["name"] == "gw-queue")
        with tempfile.TemporaryDirectory() as temporary:
            ini = gateway_compare.materialize(case, Path(temporary) / "queue")
            condition = gateway_compare.read_settings(ini)
        self.assertEqual(condition["gateways"][0]["hop_limit"], 16)
        gateway_compare.check_dir_configuration({"metadata": fixture["dir_metadata"]}, condition)

    def test_delayed_rx_release_is_rejected_even_when_same_time_rows_follow(self):
        fixture = load_fixture("gw-queue")
        events = fixture_events(fixture)
        release = next(event for event in events if event["event"] == "rx_released")
        release["time_ps"] = str(int(release["time_ps"]) + 1)
        events.sort(key=lambda event: int(event["time_ps"]))
        with self.assertRaisesRegex(ValueError, "RX release does not occur"):
            self.parse_events(fixture, events)

    def test_missing_due_forward_submission_and_child_is_rejected(self):
        fixture = load_fixture("gw-independent")
        events = fixture_events(fixture)
        child = "gw:source:0/Main.gw/ab/Main.gw.b"
        events = [
            event for event in events
            if not (event["event"] == "forward_submitted" and event["request_id"] == "source:0")
            and not (event["event"] == "rx_released" and event["request_id"] == "source:0")
            and event["request_id"] != child
        ]
        self.assertTrue(any(event["event"] == "forward_pending" for event in events))
        self.assertTrue(any(event["event"] == "rx_released" for event in events))
        with self.assertRaisesRegex(ValueError, "missing native scheduled forwarding outcome"):
            self.parse_events(fixture, events)

    def test_duplicate_child_generation_is_rejected(self):
        fixture = load_fixture("gw-independent")
        events = fixture_events(fixture)
        generated_index = next(i for i, event in enumerate(events)
                               if event["event"] == "generated" and event["parent_request_id"])
        events.insert(generated_index + 1, copy.deepcopy(events[generated_index]))
        self.assert_events_rejected(fixture, events)

    def test_payload_change_at_native_receive_is_rejected(self):
        fixture = load_fixture("gw-independent")
        events = fixture_events(fixture)
        receive = next(event for event in events if event["event"] == "native_rx_complete")
        receive["payload_hex"] = "01"
        self.assert_events_rejected(fixture, events)

    def test_changed_child_lineage_at_sof_is_rejected(self):
        fixture = load_fixture("gw-queue")
        events = fixture_events(fixture)
        sof = next(event for event in events
                   if event["event"] == "sof" and event["parent_request_id"])
        sof["hops"] = str(int(sof["hops"]) + 1)
        self.assert_events_rejected(fixture, events)

    def test_rx_admission_reason_mutation_is_rejected(self):
        fixture = load_fixture("gw-queue")
        events = fixture_events(fixture)
        admitted = next(event for event in events if event["event"] == "rx_admitted")
        admitted["reason"] = "rx_queue_full"
        self.assert_events_rejected(fixture, events)

    def test_missing_rx_release_is_rejected(self):
        fixture = load_fixture("gw-queue")
        events = fixture_events(fixture)
        events.remove(next(event for event in events if event["event"] == "rx_released"))
        self.assert_events_rejected(fixture, events)

    def test_unknown_ini_setting_is_rejected(self):
        case = next(case for case in gateway_compare.suite() if case["name"] == "gw-queue")
        with tempfile.TemporaryDirectory() as temporary:
            ini = gateway_compare.materialize(case, Path(temporary) / "queue")
            config = gateway_compare.can.read_ini(ini)
            config["General"]["ignoredFlag"] = "true"
            with ini.open("w", encoding="utf-8") as stream:
                config.write(stream)
            with self.assertRaises(ValueError):
                gateway_compare.read_settings(ini)

    def test_ned_connection_with_wrong_bus_rx_direction_is_rejected(self):
        case = next(case for case in gateway_compare.suite() if case["name"] == "gw-queue")
        with tempfile.TemporaryDirectory() as temporary:
            ini = gateway_compare.materialize(case, Path(temporary) / "queue")
            ned = ini.parent / "models/gw/Main.ned"
            text = ned.read_text(encoding="utf-8")
            self.assertIn("busA.tx_a --> gw.Wire --> src.rx;", text)
            ned.write_text(text.replace("busA.tx_a --> gw.Wire --> src.rx;",
                                        "busA.rx_a --> gw.Wire --> src.rx;", 1), encoding="utf-8")
            with self.assertRaises(ValueError):
                gateway_compare.read_settings(ini)

    def test_hop_limit_conversion_mismatch_is_rejected(self):
        fixture = load_fixture("gw-queue")
        condition = copy.deepcopy(fixture["condition"])
        condition["gateways"][0]["hop_limit"] = 64
        with self.assertRaises(ValueError):
            gateway_compare.check_dir_configuration({"metadata": fixture["dir_metadata"]}, condition)

    def test_same_priority_materialization_keeps_bus_c_default_and_tx_capacity(self):
        case = next(case for case in gateway_compare.suite()
                    if case.get("mutation") == "same-priority")
        with tempfile.TemporaryDirectory() as temporary:
            ini = gateway_compare.materialize(case, Path(temporary) / "same-priority")
            condition = gateway_compare.read_settings(ini)
        bus_c = next(bus for bus in condition["buses"] if bus["label"] == "Multi.busC")
        gateway_tx = next(node for node in condition["nodes"] if node["label"] == "Multi.gw.b")
        self.assertEqual(bus_c["bitrate"], 500_000)
        self.assertEqual(gateway_tx["queue_capacity"], 64)

    def test_overlapping_same_bus_transmissions_fail_even_below_horizon_occupancy(self):
        fixture = load_fixture("gw-independent")
        condition = copy.deepcopy(fixture["condition"])
        requests = {
            "first": {"id": "first", "status": "in_flight", "parent_request_id": None, "bus": "Main.busA",
                      "sof_ps": 0, "native_complete_ps": 10},
            "second": {"id": "second", "status": "in_flight", "parent_request_id": None, "bus": "Main.busA",
                       "sof_ps": 5, "native_complete_ps": 20},
        }
        self.assertLess(10 + 15, condition["horizon_ps"])
        with self.assertRaisesRegex(ValueError, "overlapping transmissions"):
            gateway_compare.summarize(requests, [], {}, {}, [], [], condition, "omnet")

    def test_first_ready_fifo_cannot_be_reordered_at_capacity_admission(self):
        fixture = load_fixture("gw-multicast-first-ready")
        events = fixture_events(fixture)
        admitted = next(event for event in events
                        if event["event"] == "enqueued" and event["node"] == "Multi.gw.b"
                        and event["time_ps"] == "480000000")
        self.assertEqual(admitted["request_id"], "gw:source:1/Multi.gw/ab/Multi.gw.b")
        younger = next(event for event in events if event["event"] == "waiting_tx"
                       and event["request_id"] == "gw:cross:1/Multi.gw/cb/Multi.gw.b")
        # Replace every immutable identity field, leaving a physically plausible
        # admission at the slot-freeing SOF; only the ready-order FIFO is wrong.
        identity = ("request_id", "source", "format", "can_id", "payload_hex", "native_bits",
                    "origin_request_id", "parent_request_id", "hops")
        admitted.update({field: younger[field] for field in identity})
        with self.assertRaisesRegex(ValueError, "FIFO was bypassed"):
            self.parse_events(fixture, events)

    def test_missing_sof_wake_is_rejected_with_self_consistent_queue_observations(self):
        fixture = load_fixture("gw-queue")
        events = fixture_events(fixture)
        child = "gw:source:2/Main.gw/ab/Main.gw.b"
        seen_wait = False
        kept = []
        for event in events:
            if event["request_id"] == child:
                if seen_wait:
                    continue
                seen_wait = event["event"] == "waiting_tx"
            if event["event"] == "rx_released" and event["request_id"] == "source:2":
                continue
            if (event["node"] == "Main.gw.b" and event["event"] == "native_complete"
                    and int(event["time_ps"]) > 480_000_000):
                event["queue_waiting"] = "0"
            kept.append(event)
        self.assertTrue(seen_wait)
        with self.assertRaisesRegex(ValueError, "missing native SOF wake admission"):
            self.parse_events(fixture, kept)


if __name__ == "__main__":
    unittest.main()
