"""Focused tests for the design-only proposal checker."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from check_extension_design import (
    ExtensionDesignChecker,
    check_wire_frame_integrity,
    oracle,
)


CHECKER = Path(__file__).with_name("check_extension_design.py")


def complete_nodes() -> list[dict[str, object]]:
    stages = (
        ("P-CORE-R01", "requirement", []),
        ("P-CORE-F01", "function", ["P-CORE-R01"]),
        ("P-CORE-S01", "spec", ["P-CORE-F01"]),
        ("P-CORE-A01", "architecture", ["P-CORE-S01"]),
        ("P-CORE-D01", "design", ["P-CORE-A01"]),
        ("P-CORE-T01", "verification", ["P-CORE-D01"]),
    )
    return [
        {"id": ident, "stage": stage, "upstream": upstream, "anchor": ident.lower()}
        for ident, stage, upstream in stages
    ]


def wire_time_vector() -> dict[str, object]:
    return {
        "id": "P-CORE-T01-V01",
        "test_id": "P-CORE-T01",
        "kind": "wire_time",
        "input": {
            "mac_bytes": 64,
            "preamble_sfd_bytes": 8,
            "ifg_bytes": 12,
            "rate_bps": "100000000",
            "sof_ps": "0",
        },
        "expected": {"eof_ps": "5760000", "release_ps": "6720000"},
    }


class ExtensionDesignCheckerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.proposals = self.root / "docs" / "proposals"
        self.proposals.mkdir(parents=True)
        self.fixture_dir = self.proposals / "fixtures"
        self.fixture_dir.mkdir()
        self.write_trace(complete_nodes())
        self.write_fixture(
            [
                {
                    "id": "P-CORE-T01-V01",
                    "test_id": "P-CORE-T01",
                    "kind": "readiness_gate",
                    "input": {},
                    "expected": {},
                }
            ]
        )

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write_trace(self, nodes: list[dict[str, object]], raw_json: str | None = None) -> Path:
        path = self.proposals / "trace.md"
        if raw_json is None:
            raw_json = json.dumps({"nodes": nodes})
        anchors = "\n".join(f'<a id="{node["anchor"]}"></a>' for node in nodes if "anchor" in node)
        path.write_text(f"{anchors}\n\n```proposal-trace\n{raw_json}\n```\n", encoding="utf-8")
        return path

    def write_fixture(self, vectors: list[dict[str, object]], raw_json: str | None = None) -> Path:
        path = self.fixture_dir / "sample-v1.json"
        if raw_json is None:
            raw_json = json.dumps(
                {"schema_version": 1, "status": "design_vectors_not_product_tests", "vectors": vectors}
            )
        path.write_text(raw_json, encoding="utf-8")
        return path

    def run_checker(self) -> dict[str, object]:
        return ExtensionDesignChecker(self.root).run()

    def test_valid_chain_and_structure_only_vector_are_reported_separately(self) -> None:
        report = self.run_checker()
        self.assertEqual(report["result"], "pass")
        vectors = report["vectors"]
        self.assertEqual(vectors["recomputed"], 0)
        self.assertEqual(vectors["structure_only"], 1)
        self.assertEqual(vectors["kind_coverage"]["readiness_gate"]["structure_only"], 1)

    def test_duplicate_json_key_is_rejected_in_trace_and_fixture(self) -> None:
        self.write_trace(complete_nodes(), '{"nodes": [], "nodes": []}')
        report = self.run_checker()
        self.assertTrue(any("duplicate JSON key 'nodes'" in error for error in report["errors"]))

        self.write_trace(complete_nodes())
        self.write_fixture([], '{"schema_version": 1, "schema_version": 1, "status": "design_vectors_not_product_tests", "vectors": []}')
        report = self.run_checker()
        self.assertTrue(any("duplicate JSON key 'schema_version'" in error for error in report["errors"]))

    def test_duplicate_trace_id_is_rejected(self) -> None:
        nodes = complete_nodes()
        nodes.append(dict(nodes[0]))
        self.write_trace(nodes)
        report = self.run_checker()
        self.assertTrue(any("duplicate trace ID P-CORE-R01" in error for error in report["errors"]))

    def test_missing_anchor_is_rejected(self) -> None:
        nodes = complete_nodes()
        nodes[2]["anchor"] = "absent-anchor"
        path = self.write_trace(nodes)
        path.write_text(path.read_text(encoding="utf-8").replace('<a id="absent-anchor"></a>\n', ""), encoding="utf-8")
        report = self.run_checker()
        self.assertTrue(any("missing anchor 'absent-anchor'" in error for error in report["errors"]))

    def test_missing_upstream_and_stage_jump_are_rejected(self) -> None:
        nodes = complete_nodes()
        nodes[1]["upstream"] = []
        nodes[2]["upstream"] = ["P-CORE-R01"]
        self.write_trace(nodes)
        report = self.run_checker()
        self.assertTrue(any("missing upstream for function stage" in error for error in report["errors"]))
        self.assertTrue(any("stage jump/invalid upstream P-CORE-R01" in error for error in report["errors"]))
        self.assertTrue(any("does not reach verification through all six stages" in error for error in report["errors"]))

    def test_unknown_verification_id_is_rejected(self) -> None:
        self.write_fixture(
            [
                {
                    "id": "P-CORE-UNKNOWN-V01",
                    "test_id": "P-MISSING-T99",
                    "kind": "readiness_gate",
                    "input": {},
                    "expected": {},
                }
            ]
        )
        report = self.run_checker()
        self.assertTrue(any("unknown verification ID 'P-MISSING-T99'" in error for error in report["errors"]))

    def test_unknown_kind_is_an_error_not_structure_only(self) -> None:
        self.write_fixture(
            [
                {
                    "id": "P-CORE-T01-V01",
                    "test_id": "P-CORE-T01",
                    "kind": "future_magic_kind",
                    "input": {},
                    "expected": {},
                }
            ]
        )
        report = self.run_checker()
        self.assertTrue(any("unknown vector kind 'future_magic_kind'" in error for error in report["errors"]))
        self.assertEqual(report["vectors"]["unknown_kind"], 1)
        self.assertEqual(report["vectors"]["structure_only"], 0)

    def test_invalid_hex_is_rejected(self) -> None:
        vector = {
            "id": "P-CORE-T01-V01",
            "test_id": "P-CORE-T01",
            "kind": "hex_decode",
            "input": {"bytes_hex": "0g"},
            "expected": {"bytes": [], "length_bytes": 0, "canonical_hex": ""},
        }
        self.write_fixture([vector])
        report = self.run_checker()
        self.assertTrue(any("bytes_hex contains non_hex characters" in error for error in report["errors"]))

    def test_hex_validation_vectors_can_describe_expected_rejection(self) -> None:
        vector = {
            "id": "P-CORE-T01-V01",
            "test_id": "P-CORE-T01",
            "kind": "hex_validation",
            "input": {"bytes_hex": "0"},
            "expected": {"prepare": "reject", "reason": "odd_hex", "checksum": "not_applicable"},
        }
        self.write_fixture([vector])
        report = self.run_checker()
        self.assertEqual(report["errors"], [])
        self.assertEqual(report["vectors"]["recomputed"], 1)

    def test_wrong_arithmetic_expectation_is_rejected(self) -> None:
        vector = wire_time_vector()
        vector["expected"]["eof_ps"] = "5760001"
        self.write_fixture([vector])
        report = self.run_checker()
        self.assertTrue(any("arithmetic oracle mismatch" in error for error in report["errors"]))
        self.assertEqual(report["vectors"]["recomputed"], 1)

    def test_report_option_writes_machine_readable_design_only_report(self) -> None:
        report_path = self.root / "reports" / "extension-design.json"
        result = subprocess.run(
            [sys.executable, str(CHECKER), "--root", str(self.root), "--report", str(report_path)],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertIn("design-only", report["evidence_scope"])


class AdditionalOracleTests(unittest.TestCase):
    def test_phase_order_uses_delta_before_phase(self):
        vector = {"input": {"events": [
            {"id": "later", "time_ps": "1", "delta": 1, "phase": 0, "seq": 0},
            {"id": "earlier", "time_ps": "1", "delta": 0, "phase": 2, "seq": 10}]},
            "expected": {"order": ["earlier", "later"]}}
        oracle("phase_order", vector)
        vector["expected"]["order"].reverse()
        with self.assertRaises(ValueError): oracle("phase_order", vector)

    def test_mld_small_code_uses_milliseconds(self):
        oracle("time_code", {"test_id": "P-MLD-T01", "input": {"response_code": 100, "qqic": 1},
                            "expected": {"response_ps": "100000000000", "interval_ps": "1000000000000"}})

    def test_recalculated_expected_fields_reject_corruption(self):
        root = Path(__file__).resolve().parents[1] / "docs/proposals/fixtures"
        fields = {"gauge_integral": "area", "recovery_window": "bitmaps_hex", "bytes": "folded_checksum_sum",
                  "wire_envelope": "prepare", "manifest_set": "valid", "checkpoint_publication": "ordinal_after",
                  "boundary_prefix": "cases", "timestamp_bucket": "window_min", "result_schema": "result_schema"}
        vectors = [v for p in root.glob("*-v1.json") for v in json.loads(p.read_text())["vectors"]]
        for kind, field in fields.items():
            with self.subTest(kind=kind):
                vector = next(v for v in vectors if v["kind"] == kind and field in v["expected"])
                oracle(kind, vector)
                mutated = json.loads(json.dumps(vector))
                mutated["expected"][field] = {"corrupted": True}
                with self.assertRaises(ValueError): oracle(kind, mutated)

    def test_packet_checksum_change_is_detected(self):
        root = Path(__file__).resolve().parents[1] / "docs/proposals/fixtures/membership-wire-v1.json"
        vector = next(v for v in json.loads(root.read_text())["vectors"] if v["kind"] == "bytes")
        raw = bytearray.fromhex(vector["input"]["bytes_hex"])
        raw[26] ^= 1  # IGMP checksum byte; IPv4 header remains intact.
        vector["input"]["bytes_hex"] = raw.hex()
        with self.assertRaises(ValueError): oracle("bytes", vector)

    def test_valid_rstp_and_mvrp_wire_envelopes_are_accepted(self):
        for protocol in ("rstp", "mvrp"):
            with self.subTest(protocol=protocol):
                oracle("wire_envelope", {
                    "input": {"protocol": protocol, "encapsulation": "mac_frame_fcs", "extra_fields": []},
                    "expected": {"prepare": "accept"},
                })

    def test_wire_frame_crc_is_recomputed_from_input(self):
        path = Path(__file__).resolve().parents[1] / "docs/proposals/fixtures/rstp-standard-v1.json"
        vector = next(v for v in json.loads(path.read_text())["vectors"] if v["kind"] == "wire_decode")
        check_wire_frame_integrity("wire_decode", vector)
        mutated = json.loads(json.dumps(vector))
        raw = bytearray.fromhex(mutated["input"]["bytes_hex"])
        raw[-1] ^= 1
        mutated["input"]["bytes_hex"] = raw.hex()
        with self.assertRaisesRegex(ValueError, "computed_fcs_hex|FCS"):
            check_wire_frame_integrity("wire_decode", mutated)

    def test_mvrp_base_six_event_packing_is_recomputed(self):
        oracle("packing_arithmetic", {
            "input": {"events": [3, 4, 5]},
            "expected": {"packed_hex": "89", "count": 3},
        })
        with self.assertRaisesRegex(ValueError, "packed_hex"):
            oracle("packing_arithmetic", {
                "input": {"events": [3, 4, 5]},
                "expected": {"packed_hex": "8a", "count": 3},
            })

    def test_mixed_dynamic_static_fdb_flush_expectation_is_rejected(self):
        path = Path(__file__).resolve().parents[1] / "docs/proposals/fixtures/bridge-integration-v1.json"
        vector = next(v for v in json.loads(path.read_text())["vectors"]
                      if v["kind"] == "bridge_fdb_flush" and v["input"]["kind"] == "rstp")
        oracle("bridge_fdb_flush", vector)
        mutated = json.loads(json.dumps(vector))
        mutated["expected"]["removed"].append("s")  # Static entry mixed into dynamic flush expectation.
        with self.assertRaisesRegex(ValueError, "expected.removed"):
            oracle("bridge_fdb_flush", mutated)

    def test_rstp_fsm_and_scenario_vectors_are_explicitly_structure_only(self):
        report = ExtensionDesignChecker(Path(__file__).resolve().parents[1]).run()
        coverage = report["vectors"]["kind_coverage"]
        self.assertGreater(coverage["fsm_unit"]["structure_only"], 0)
        self.assertGreater(coverage["scenario"]["structure_only"], 0)
        self.assertEqual(coverage["fsm_unit"]["recomputed"], 0)
        self.assertEqual(coverage["scenario"]["recomputed"], 0)


if __name__ == "__main__":
    unittest.main()
