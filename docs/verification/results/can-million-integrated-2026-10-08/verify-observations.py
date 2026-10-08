#!/usr/bin/env python3
"""Independently verify completed integrated million-request observations."""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import statistics
import sys
from pathlib import Path
from typing import Any


DEFAULT_ROOT = Path("/tmp/dir-million-integrated-2026-10-08")
DEFAULT_REPO = Path("/home/hideki/DIR-Simulator")
EXPECTED_RHOS = ("0.30", "0.90", "1.20")
EXPECTED_FILES = ("diagnostics.jsonl", "events.csv", "results.json", "summary.csv")
GIB = 1024**3


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def same_number(left: Any, right: Any) -> bool:
    if isinstance(left, bool) or isinstance(right, bool):
        return left == right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return math.isclose(float(left), float(right), rel_tol=0.0, abs_tol=1e-12)
    return left == right


class Audit:
    def __init__(self) -> None:
        self.errors: list[dict[str, str]] = []

    def check(self, condition: bool, where: str, message: str) -> bool:
        if not condition:
            self.errors.append({"where": where, "message": message})
        return condition


def compare_pins(root: Path, repo: Path, before: dict[str, Any], audit: Audit) -> dict[str, Any]:
    checked: dict[str, Any] = {}
    source = before.get("source_sha256", {})
    audit.check(len(source) == 184, "before.source_sha256", f"expected 184 pinned source files; found {len(source)}")
    for relative, expected in source.items():
        for label, path in (("repository", repo / relative), ("staged_copy", root / "source" / relative)):
            try:
                actual = sha256(path)
            except OSError as error:
                actual = None
                audit.check(False, f"source.{label}.{relative}", f"cannot hash pinned file: {error}")
            else:
                audit.check(actual == expected, f"source.{label}.{relative}", "SHA-256 differs from before.json")
            checked[f"{label}:{relative}"] = actual

    binary = before.get("binary", {})
    binary_path = Path(binary.get("path", ""))
    try:
        actual_binary_hash = sha256(binary_path)
        actual_binary_bytes = binary_path.stat().st_size
    except OSError as error:
        actual_binary_hash = None
        actual_binary_bytes = None
        audit.check(False, "binary", f"cannot read pinned binary: {error}")
    else:
        audit.check(actual_binary_hash == binary.get("sha256"), "binary.sha256", "binary SHA-256 differs from before.json")
        audit.check(actual_binary_bytes == binary.get("bytes"), "binary.bytes", "binary byte count differs from before.json")
    checked["binary"] = {"path": str(binary_path), "sha256": actual_binary_hash, "bytes": actual_binary_bytes}

    for label, pin_map, base in (
        ("helpers", before.get("helper_sha256", {}), root),
        ("inputs", before.get("input_sha256", {}), root / "inputs"),
        ("build_documents", before.get("build_documents_sha256", {}), repo),
    ):
        checked[label] = {}
        for relative, expected in pin_map.items():
            try:
                actual = sha256(base / relative)
            except OSError as error:
                actual = None
                audit.check(False, f"{label}.{relative}", f"cannot hash pinned file: {error}")
            else:
                audit.check(actual == expected, f"{label}.{relative}", "SHA-256 differs from before.json")
            checked[label][relative] = actual

    generation_path = root / "inputs" / "generation.json"
    try:
        generation_hash = sha256(generation_path)
    except OSError as error:
        generation_hash = None
        audit.check(False, "input_generation_sha256", f"cannot hash generation manifest: {error}")
    else:
        audit.check(generation_hash == before.get("input_generation_sha256"),
                    "input_generation_sha256", "generation manifest hash differs from before.json")
    checked["input_generation_sha256"] = generation_hash

    gate = root / "producer-gate.json"
    pinned_gate_path = Path(before.get("producer_gate", {}).get("path", ""))
    expected_gate_hash = before.get("producer_gate", {}).get("sha256")
    try:
        copied_gate_hash = sha256(gate)
        latest_gate_hash = sha256(pinned_gate_path)
        gate_data = read_json(gate)
    except (OSError, json.JSONDecodeError) as error:
        copied_gate_hash = latest_gate_hash = None
        gate_data = {}
        audit.check(False, "producer_gate", f"cannot read pinned producer gate: {error}")
    else:
        audit.check(copied_gate_hash == expected_gate_hash, "producer_gate.copy", "copied gate hash differs from before.json")
        audit.check(latest_gate_hash == expected_gate_hash, "producer_gate.latest", "latest gate hash differs from before.json")
        audit.check(gate_data.get("tests") == before.get("producer_gate", {}).get("tests"),
                    "producer_gate.tests", "copied gate test counts differ from the pinned counts")
        tests = gate_data.get("tests", {})
        audit.check(tests == {"passed": 543, "failed": 0, "ignored": 0, "suites": 28},
                    "producer_gate.tests", "expected 543 passed tests, 0 failed, 0 ignored, and 28 suites")
    checked["producer_gate"] = {"copied_sha256": copied_gate_hash, "latest_sha256": latest_gate_hash,
                                "tests": gate_data.get("tests")}
    return checked


def verify_generation(root: Path, before: dict[str, Any], audit: Audit) -> dict[str, Any]:
    generation = read_json(root / "inputs" / "generation.json")
    audit.check(generation.get("request_count_per_configuration") == 1_000_000,
                "generation.request_count_per_configuration", "must equal 1,000,000")
    audit.check(generation.get("sender_count") == 32, "generation.sender_count", "must equal 32")
    scenarios = generation.get("scenarios", [])
    audit.check(len(scenarios) == 3, "generation.scenarios", f"expected 3 scenarios; found {len(scenarios)}")
    audit.check(generation.get("input_sha256") == before.get("input_sha256"),
                "generation.input_sha256", "generation input hash map differs from before.json")
    report: dict[str, Any] = {
        "request_count_per_configuration": generation.get("request_count_per_configuration"),
        "sender_count": generation.get("sender_count"),
        "conditions": [],
    }
    for rho in EXPECTED_RHOS:
        matches = [scenario for scenario in scenarios if scenario.get("rho") == rho]
        audit.check(len(matches) == 1, f"generation.rho-{rho}", f"expected exactly one scenario; found {len(matches)}")
        if not matches:
            continue
        scenario = matches[0]
        ini_name = f"rho-{rho}.ini"
        workload_name = scenario.get("workload")
        audit.check(scenario.get("ini") == ini_name, f"generation.rho-{rho}.ini", "scenario INI mapping differs")
        audit.check(scenario.get("request_count_total") == 1_000_000,
                    f"generation.rho-{rho}.request_count_total", "must equal 1,000,000")
        audit.check(scenario.get("request_count_per_sender") == 31_250,
                    f"generation.rho-{rho}.request_count_per_sender", "must equal 31,250")
        workload = read_json(root / "inputs" / str(workload_name))
        generators = workload.get("generators", [])
        total = sum(item.get("count", 0) for item in generators if isinstance(item.get("count"), int))
        audit.check(len(generators) == 32, f"workload.rho-{rho}.generators", f"expected 32; found {len(generators)}")
        audit.check(all(isinstance(item.get("count"), int) and item["count"] == 31_250 for item in generators),
                    f"workload.rho-{rho}.counts", "each generator must schedule exactly 31,250 requests")
        audit.check(total == 1_000_000, f"workload.rho-{rho}.total", f"scheduled count is {total}, expected 1,000,000")
        report["conditions"].append({"rho": rho, "ini": ini_name, "workload": workload_name,
                                     "generator_count": len(generators), "scheduled_requests": total})
    return report


def manifest_entries(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    entries = manifest.get("files", [])
    if isinstance(entries, dict):
        entries = list(entries.values())
    return entries if isinstance(entries, list) else []


def get_file_details(attempt: dict[str, Any], name: str) -> dict[str, Any] | None:
    for item in attempt.get("files", []):
        if item.get("path") == name:
            return item
    return None


def verify_attempt(root: Path, condition: dict[str, Any], attempt: dict[str, Any], index: int,
                   target_wall: float, target_rss: int, allow_probe_relabel: bool,
                   audit: Audit) -> dict[str, Any]:
    rho = str(condition.get("rho"))
    label = f"condition-{rho}.attempt-{index}"
    evidence_name = attempt.get("evidence_directory")
    if not evidence_name:
        audit.check(False, label, "evidence_directory is missing")
        return {"index": index, "verified": False}
    folder = Path(evidence_name).resolve()
    measurements_root = (root / "measurements").resolve()
    within_root = folder == measurements_root or measurements_root in folder.parents
    audit.check(within_root, f"{label}.evidence_directory", "attempt evidence must remain under measurements/")
    if not within_root:
        return {"index": index, "verified": False, "evidence_directory": str(folder)}

    required = ("attempt-record.json", "attempt.json", "gnu-time.txt", "stdout.json", "stderr.txt",
                "samples.jsonl", "attempt-started.json")
    files_ok = True
    for name in required:
        present = (folder / name).is_file()
        files_ok = audit.check(present, f"{label}.{name}", "required evidence file is missing") and files_ok
    if not files_ok:
        return {"index": index, "verified": False, "evidence_directory": str(folder)}

    disk_record = read_json(folder / "attempt-record.json")
    helper = read_json(folder / "attempt.json")
    for key, value in disk_record.items():
        if key == "kind" and attempt.get("kind") == "warmup" and value == "completion_probe":
            audit.check(allow_probe_relabel, f"{label}.attempt-record.kind",
                        "probe-to-warmup relabel is allowed only for a completed probe within 120 seconds")
            continue  # run-probes.py relabels the first attempt after writing its per-attempt record.
        audit.check(attempt.get(key) == value, f"{label}.attempt-record.{key}",
                    "aggregate attempt differs from its per-attempt record")
    for key, value in helper.items():
        audit.check(disk_record.get(key) == value, f"{label}.helper.{key}",
                    "per-attempt helper record differs from attempt-record.json")

    expected_condition = attempt.get("condition")
    audit.check(expected_condition == rho, f"{label}.condition", "attempt condition does not match its parent")
    audit.check(disk_record.get("condition") == rho,
                f"{label}.disk-condition", "per-attempt condition does not match its parent")

    post_pins = attempt.get("post_attempt_pins", {})
    source_count = len(read_json(root / "before.json").get("source_sha256", {}))
    audit.check(post_pins.get("all_pins_match") is True, f"{label}.post_attempt_pins", "pins were not all verified")
    audit.check(post_pins.get("source_file_count") == source_count,
                f"{label}.post_attempt_pins.source_file_count", "source pin count differs from before.json")
    audit.check(attempt.get("large_outputs_removed_after_recording") is True,
                f"{label}.large_outputs_removed_after_recording", "large output removal was not recorded")
    audit.check(not (folder / "output").exists(), f"{label}.output", "attempt output directory remains after recording")

    raw_time_text = (folder / "gnu-time.txt").read_text(encoding="utf-8")
    raw_records = []
    for line in raw_time_text.splitlines():
        if line.startswith("{"):
            try:
                raw_records.append(json.loads(line))
            except json.JSONDecodeError:
                audit.check(False, f"{label}.gnu-time", "GNU time JSON line is malformed")
    audit.check(bool(raw_records), f"{label}.gnu-time", "GNU time record is missing")
    raw = raw_records[-1] if raw_records else {}
    measured = attempt.get("measurements", {})
    for key in ("wall_seconds", "max_rss_kib", "user_seconds", "system_seconds", "exit_status"):
        audit.check(same_number(measured.get(key), raw.get(key)), f"{label}.measurements.{key}",
                    "measurement does not match raw GNU time")
    audit.check(attempt.get("max_rss_bytes") == measured.get("max_rss_kib", -1) * 1024,
                f"{label}.max_rss_bytes", "byte RSS does not equal GNU time KiB converted to bytes")
    audit.check(helper.get("measurements") == measured,
                f"{label}.helper_measurements", "measurement differs from original helper output")
    audit.check(helper.get("max_rss_bytes") == attempt.get("max_rss_bytes"),
                f"{label}.helper_rss", "full-process RSS differs from original helper output")

    completed = attempt.get("completed") is True
    wall = measured.get("wall_seconds")
    rss = attempt.get("max_rss_bytes")
    expected_wall_flag = (wall <= target_wall) if completed and isinstance(wall, (int, float)) else (
        False if isinstance(wall, (int, float)) and wall > target_wall else None)
    expected_rss_flag = (rss <= target_rss) if completed and isinstance(rss, int) else (
        False if isinstance(rss, int) and rss > target_rss else None)
    audit.check(attempt.get("completion_wall_target_met") is expected_wall_flag,
                f"{label}.completion_wall_target_met", "flag does not match the GNU time wall observation")
    audit.check(attempt.get("full_process_rss_target_met") is expected_rss_flag,
                f"{label}.full_process_rss_target_met", "flag does not match the GNU time full-process RSS observation")

    result: dict[str, Any] = {
        "index": index, "kind": attempt.get("kind"), "repeat": attempt.get("repeat"),
        "completed": completed, "wall_seconds": wall, "max_rss_bytes": rss,
        "completion_wall_target_met": attempt.get("completion_wall_target_met"),
        "full_process_rss_target_met": attempt.get("full_process_rss_target_met"),
        "evidence_directory": str(folder),
    }
    if not completed:
        audit.check(attempt.get("manifest_verified") is not True, f"{label}.manifest_verified",
                    "incomplete attempt must not claim verified complete output")
        return result

    audit.check(attempt.get("returncode") == 0, f"{label}.returncode", "completed attempt must exit 0")
    audit.check(measured.get("exit_status") == 0, f"{label}.gnu-time-exit", "GNU time exit status must be 0")
    audit.check(attempt.get("manifest_verified") is True, f"{label}.manifest_verified", "helper did not verify output manifest")
    manifest_path = folder / "manifest.json"
    metadata_path = folder / "metadata.json"
    audit.check(manifest_path.is_file(), f"{label}.manifest", "copied manifest.json is missing")
    audit.check(metadata_path.is_file(), f"{label}.metadata", "copied metadata.json is missing")
    if not manifest_path.is_file() or not metadata_path.is_file():
        return result
    manifest = read_json(manifest_path)
    metadata = read_json(metadata_path)
    entries = manifest_entries(manifest)
    names = [entry.get("name", entry.get("path")) for entry in entries]
    audit.check(sorted(names) == sorted(EXPECTED_FILES), f"{label}.manifest.files",
                f"expected exactly {EXPECTED_FILES}; found {names}")
    audit.check(manifest.get("status") == "complete", f"{label}.manifest.status", "manifest status must be complete")
    audit.check(isinstance(manifest.get("termination"), str) and bool(manifest["termination"]),
                f"{label}.manifest.termination", "complete manifest must record its termination reason")
    audit.check(manifest.get("partial") is False, f"{label}.manifest.partial", "manifest must be non-partial")

    file_evidence: dict[str, Any] = {}
    for entry in entries:
        name = entry.get("path", entry.get("name"))
        detail = get_file_details(attempt, name)
        if detail is None:
            audit.check(False, f"{label}.manifest.{name}", "helper file verification record is missing")
            continue
        declared = detail.get("declared", {})
        declared_size = entry.get("size_bytes", entry.get("bytes"))
        audit.check(str(declared_size) == str(detail.get("bytes")), f"{label}.manifest.{name}.size",
                    "copied manifest size does not match helper's measured size")
        audit.check(entry.get("sha256") == detail.get("sha256"), f"{label}.manifest.{name}.sha256",
                    "copied manifest hash does not match helper's measured hash")
        audit.check(declared == entry, f"{label}.manifest.{name}.declared", "helper's declaration differs from copied manifest")
        file_evidence[name] = {"bytes": detail.get("bytes"), "sha256": detail.get("sha256")}

    selected = attempt.get("selected_summary", [])
    generated = [row for row in selected if row.get("target") == "$all" and row.get("metric") == "generated"]
    audit.check(len(generated) == 1 and str(generated[0].get("value")) == "1000000",
                f"{label}.generated", "published summary must report exactly 1,000,000 generated requests for $all")

    stdout = (folder / "stdout.json").read_text(encoding="utf-8")
    try:
        stdout_report = json.loads(stdout)
    except json.JSONDecodeError as error:
        stdout_report = {}
        audit.check(False, f"{label}.stdout.json", f"completed CLI stdout is not one JSON report: {error}")
    run_report = attempt.get("run_report", {})
    audit.check(stdout_report == run_report, f"{label}.run_report.stdout", "run_report differs from captured CLI stdout")
    audit.check(helper.get("run_report") == run_report, f"{label}.run_report.helper", "run_report differs from helper record")
    audit.check(run_report.get("exit_code") == 0, f"{label}.run_report.exit_code", "run report exit_code must be 0")
    audit.check(run_report.get("termination") == manifest.get("termination"),
                f"{label}.run_report.termination", "run report and manifest termination reasons differ")
    audit.check(run_report.get("partial") is False, f"{label}.run_report.partial", "run report must be non-partial")
    audit.check(run_report.get("primary_diagnostic") is None, f"{label}.run_report.primary_diagnostic", "completed run has a primary diagnostic")
    audit.check(attempt.get("event_processing_wall_seconds") == run_report.get("event_processing_wall_seconds"),
                f"{label}.run_report.event_wall", "runtime wall observation differs from run report")
    audit.check(Path(run_report.get("output_path", "")).resolve() == (folder / "output").resolve(),
                f"{label}.run_report.output_path", "run report output path does not identify this attempt")
    audit.check(Path(run_report.get("manifest_path", "")).resolve() == (folder / "output" / "manifest.json").resolve(),
                f"{label}.run_report.manifest_path", "run report manifest path does not identify this attempt")
    audit.check(bool(manifest.get("run_id")), f"{label}.manifest.run_id", "manifest run_id is missing")

    model_types = [model.get("type") for model in metadata.get("models", []) if isinstance(model, dict)]
    config_items = metadata.get("config", [])
    configured_profiles = [item.get("value") for item in config_items
                           if isinstance(item, dict) and item.get("key") in ("model-profile", "Main.bus.profile")]
    normalized_profiles = {value.strip('"') for value in configured_profiles if isinstance(value, str)}
    profile = model_types[0] if model_types else None
    audit.check(bool(profile), f"{label}.metadata.models", "metadata has no model type identity")
    audit.check(len(set(model_types)) == 1, f"{label}.metadata.models", "metadata model types are missing or inconsistent")
    audit.check(bool(normalized_profiles) and normalized_profiles == {profile},
                f"{label}.metadata.profile", "metadata model type and resolved profile configuration differ")
    audit.check(bool(metadata.get("config_sha256")), f"{label}.metadata.config_sha256", "metadata config hash is missing")
    audit.check(bool(metadata.get("input_sha256")), f"{label}.metadata.input_sha256", "metadata input hash is missing")
    result["manifest"] = {"status": manifest.get("status"), "run_id": manifest.get("run_id"),
                          "files": file_evidence}
    result["metadata_run_report_identity_verified"] = bool(
        stdout_report == run_report and run_report.get("exit_code") == 0 and
        run_report.get("termination") == manifest.get("termination") and manifest.get("status") == "complete" and
        Path(run_report.get("output_path", "")).resolve() == (folder / "output").resolve() and
        manifest.get("run_id") and profile and normalized_profiles == {profile} and
        metadata.get("config_sha256") and metadata.get("input_sha256")
    )

    diag = get_file_details(attempt, "diagnostics.jsonl")
    stderr = (folder / "stderr.txt").read_text(encoding="utf-8")
    diagnostics_clear = bool(diag and diag.get("bytes") == 0 and not stderr and run_report.get("primary_diagnostic") is None)
    result["diagnostics"] = {"clear": diagnostics_clear, "stderr_bytes": len(stderr.encode("utf-8")),
                             "diagnostics_bytes": diag.get("bytes") if diag else None,
                             "diagnostics_sha256": diag.get("sha256") if diag else None}
    return result


def expected_evaluation(attempts: list[dict[str, Any]], target_wall: float, target_rss: int) -> dict[str, Any]:
    measured = [item for item in attempts if item.get("kind") == "measurement"]
    complete = len(measured) == 3 and all(item.get("completed") is True for item in measured)
    rss_values = [item["max_rss_bytes"] for item in measured if isinstance(item.get("max_rss_bytes"), int)]
    maximum_rss = max(rss_values, default=None)
    deterministic_equal = None
    median_wall = None
    if complete:
        deterministic_equal = all(item.get("deterministic") == measured[0].get("deterministic") for item in measured)
        median_wall = statistics.median(item["measurements"]["wall_seconds"] for item in measured)
    return {
        "complete_measurement_count": sum(item.get("completed") is True for item in measured),
        "median_completion_wall_seconds": median_wall,
        "maximum_observed_rss_bytes": maximum_rss,
        "median_attempt_wall_seconds": statistics.median(item["measurements"]["wall_seconds"] for item in measured) if measured else None,
        "performance_target_passed": (median_wall <= target_wall) if complete else None,
        "memory_target_passed": False if maximum_rss is not None and maximum_rss > target_rss else (True if complete else None),
        "deterministic_results_equal": deterministic_equal,
        "verdict": "passed" if complete and deterministic_equal and maximum_rss <= target_rss and median_wall <= target_wall else "failed",
    }


def verify_condition(root: Path, condition: dict[str, Any], target_wall: float, target_rss: int,
                     audit: Audit) -> dict[str, Any]:
    rho = str(condition.get("rho"))
    attempts = condition.get("attempts", [])
    audit.check(condition.get("status") == "complete", f"condition-{rho}.status", "condition is not complete")
    audit.check(condition.get("ini") == f"rho-{rho}.ini", f"condition-{rho}.ini", "condition INI mapping differs")
    probes = [item for item in attempts if item.get("kind") in ("completion_probe", "warmup") and item.get("repeat") == 0]
    audit.check(len(probes) == 1, f"condition-{rho}.probe", f"expected one initial probe/warmup; found {len(probes)}")
    probe = probes[0] if len(probes) == 1 else {}
    relabeled = probe.get("kind") == "warmup"
    allow_probe_relabel = (
        relabeled and probe.get("completed") is True and
        isinstance(probe.get("measurements", {}).get("wall_seconds"), (int, float)) and
        probe["measurements"]["wall_seconds"] <= target_wall
    )
    if relabeled:
        audit.check(allow_probe_relabel, f"condition-{rho}.probe-relabel",
                    "warmup relabel requires a completed probe within the wall target")
    audits = [verify_attempt(root, condition, item, index, target_wall, target_rss,
                             allow_probe_relabel and item is probe, audit)
              for index, item in enumerate(attempts)]
    measurements = [item for item in attempts if item.get("kind") == "measurement"]
    audit.check(len(measurements) in (0, 3), f"condition-{rho}.measurements",
                f"expected either no formal repeats or 3; found {len(measurements)}")
    if measurements:
        audit.check([item.get("repeat") for item in measurements] == [1, 2, 3],
                    f"condition-{rho}.repeat_numbers", "formal repeats must be numbered 1, 2, 3")
    if len(probes) == 1 and probe.get("completed") is True:
        probe_wall = probe.get("measurements", {}).get("wall_seconds")
        if isinstance(probe_wall, (int, float)) and probe_wall <= target_wall:
            audit.check(len(measurements) == 3, f"condition-{rho}.repeat-policy",
                        "a probe within the time target requires three formal measurements")
        else:
            audit.check(not measurements, f"condition-{rho}.repeat-policy",
                        "formal repeats must not run when the initial probe misses the time target")

    stored = condition.get("formal_evaluation")
    recomputed = expected_evaluation(attempts, target_wall, target_rss)
    if not measurements:
        audit.check(stored is None, f"condition-{rho}.formal_evaluation", "formal evaluation must be absent without repeats")
        audit.check(condition.get("formal_repeats_not_run_reason") is not None,
                    f"condition-{rho}.formal_repeats_not_run_reason", "reason for omitting repeats is missing")
    else:
        audit.check(isinstance(stored, dict), f"condition-{rho}.formal_evaluation", "formal evaluation is missing")
        if isinstance(stored, dict):
            for key, expected in recomputed.items():
                audit.check(same_number(stored.get(key), expected), f"condition-{rho}.formal_evaluation.{key}",
                            "stored formal evaluation differs from independent recomputation")
            if recomputed["median_completion_wall_seconds"] is None:
                audit.check(stored.get("median_completion_wall_seconds") is None,
                            f"condition-{rho}.formal_evaluation.median_completion_wall_seconds",
                            "formal completion median must be absent without 3 completed measurements")

    complete_measurements = [item for item in measurements if item.get("completed") is True]
    deterministic_equal = None
    diagnostics_clear = None
    diagnostics_equal = None
    independent_verdict = "not_evaluated"
    formal_observations = [audits[index] for index, item in enumerate(attempts) if item.get("kind") == "measurement"]
    if len(complete_measurements) == 3:
        hashes = [item.get("deterministic") for item in complete_measurements]
        audit.check(all(isinstance(value, dict) and value for value in hashes),
                    f"condition-{rho}.deterministic_hashes", "each repeat must contain deterministic hashes")
        deterministic_equal = all(value == hashes[0] for value in hashes)
        audit.check(deterministic_equal, f"condition-{rho}.deterministic_equal", "normalized output hashes differ across repeats")
        diag_records = [entry.get("diagnostics", {}) for entry in formal_observations]
        diagnostics_clear = all(item.get("clear") is True for item in diag_records)
        diag_hashes = [item.get("diagnostics_sha256") for item in diag_records]
        diagnostics_equal = len(set(diag_hashes)) == 1 and bool(diag_hashes[0])
        audit.check(diagnostics_clear, f"condition-{rho}.diagnostics", "at least one formal repeat has diagnostics or stderr")
        audit.check(diagnostics_equal, f"condition-{rho}.diagnostics_equal", "diagnostic outputs differ across repeats")
        independent_verdict = "passed" if recomputed["verdict"] == "passed" and diagnostics_clear and diagnostics_equal else "failed"
    elif measurements:
        independent_verdict = "incomplete_formal_set"

    return {
        "rho": rho,
        "scheduled_requests_per_run": 1_000_000,
        "attempts": audits,
        "formal_measurement_count": len(measurements),
        "formal_evaluation_recomputed": recomputed,
        "stored_formal_evaluation": stored,
        "formal_deterministic_hashes_equal": deterministic_equal,
        "formal_diagnostics_clear": diagnostics_clear,
        "formal_diagnostics_equal": diagnostics_equal,
        "initial_probe_relabel_note": (
            "aggregate record says warmup; per-attempt record retains completion_probe"
            if allow_probe_relabel else None
        ),
        "independent_formal_verdict": independent_verdict,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    parser.add_argument("--repo", type=Path, default=DEFAULT_REPO)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    repo = args.repo.resolve()
    report = read_json(root / "measurement.json")
    if report.get("status") != "complete":
        raise SystemExit("measurement.json is not complete; verifier left no output")
    if not report.get("finished_at_utc"):
        raise SystemExit("measurement.json has no finished_at_utc; verifier left no output")

    audit = Audit()
    before = read_json(root / "before.json")
    audit.check(report.get("before_sha256") == sha256(root / "before.json"),
                "measurement.before_sha256", "measurement report does not pin before.json")
    audit.check(report.get("binary") == before.get("binary"), "measurement.binary", "binary pin differs from before.json")
    audit.check(report.get("source_sha256") == before.get("source_sha256"),
                "measurement.source_sha256", "source pin map differs from before.json")
    audit.check(report.get("targets") == before.get("targets"), "measurement.targets", "targets differ from before.json")
    audit.check(before.get("probe_policy", {}).get("formal_native_baseline_verdict") is False,
                "before.probe_policy.formal_native_baseline_verdict", "native baseline must remain unqualified")
    audit.check("WSL2" in report.get("qualification", ""),
                "measurement.qualification", "observation must retain its WSL2 scope")
    audit.check(report.get("wall_observation_limit_seconds") == before.get("probe_policy", {}).get("wall_observation_limit_seconds"),
                "measurement.wall_observation_limit_seconds", "wall observation limit differs from the pinned policy")

    pins = compare_pins(root, repo, before, audit)
    generation = verify_generation(root, before, audit)
    conditions = report.get("conditions", [])
    audit.check(len(conditions) == 3, "measurement.conditions", f"expected 3 conditions; found {len(conditions)}")
    audit.check(tuple(str(item.get("rho")) for item in conditions) == EXPECTED_RHOS,
                "measurement.condition_order", "conditions must be exactly 0.30, 0.90, 1.20 in order")
    target_wall = before.get("targets", {}).get("wall_seconds", 120)
    target_rss = before.get("targets", {}).get("max_rss_bytes", 2 * GIB)
    condition_results = [verify_condition(root, item, target_wall, target_rss, audit) for item in conditions]

    output = {
        "schema_version": 1,
        "verified_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "status": "passed" if not audit.errors else "failed",
        "scope": "WSL2 integrated completion observations; this does not establish a native baseline formal pass",
        "native_baseline_formal_pass": False,
        "pins": pins,
        "generation": generation,
        "conditions": condition_results,
        "errors": audit.errors,
    }
    output_path = args.output.resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = output_path.with_suffix(output_path.suffix + ".tmp")
    temporary.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(output_path)
    print(json.dumps({"status": output["status"], "errors": len(audit.errors), "output": str(output_path)}, ensure_ascii=False))
    return 0 if not audit.errors else 1


if __name__ == "__main__":
    sys.exit(main())
