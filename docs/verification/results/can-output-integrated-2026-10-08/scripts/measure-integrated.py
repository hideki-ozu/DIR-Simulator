#!/usr/bin/env python3
"""Run guarded, serial baseline-versus-integrated CAN measurements.

This controller only measures when explicitly invoked with --execute. It does
not build binaries. It requires the final fmt/clippy/test/release-build gate and
validates provenance and input pins before and after every case. Failed helper
outputs and logs remain in the run directory for diagnosis.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import sys


ROOT = Path("/tmp/dir-opt-integrated-2026-10-08")
REPO = Path("/home/hideki/DIR-Simulator")
CONTROLLER_PATH = ROOT / "measure-integrated.py"
PLAN_PATH = ROOT / "measurement-plan.json"
BEFORE_PATH = ROOT / "before.json"
INTEGRATED_GATE = ROOT / "gates/latest.json"
INTEGRATED_BINARY = ROOT / "bin/integrated"
HELPER = REPO / "scripts/performance/measure_can_export.py"
HELPER_IMPL = REPO / "scripts/performance/measure_can_million.py"
EXPECTED_OUTPUT_FILES = {
    "diagnostics.jsonl", "events.csv", "results.json", "summary.csv"
}
EXPECTED_COMMANDS = ("fmt", "clippy", "test", "build")
EXPECTED_BINARY_NAMES = ("baseline", "integrated")


class MeasurementError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise MeasurementError(message)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n",
                         encoding="utf-8")
    temporary.replace(path)


def source_snapshot(source_root: Path) -> dict[str, str]:
    if not source_root.is_dir():
        fail(f"Source snapshot directory is missing: {source_root}")
    roots = [source_root / name for name in
             ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    files = [path for path in (*((source_root / "crates").rglob("*")), *roots)
             if path.is_file() and not path.is_symlink()]
    if not files:
        fail(f"Source snapshot is empty: {source_root}")
    return {path.relative_to(source_root).as_posix(): sha256_file(path)
            for path in sorted(files)}


def extract_command_records(commands) -> dict[str, dict]:
    """Normalize supported gate representations into action -> command row."""
    if isinstance(commands, dict):
        records = {}
        for action, row in commands.items():
            if action in EXPECTED_COMMANDS and isinstance(row, dict):
                records[action] = row
        if set(records) == set(EXPECTED_COMMANDS):
            return records
        fail("Final gate commands must identify fmt, clippy, test, and build")
    if not isinstance(commands, list):
        fail("Final gate commands must be a list or action-keyed object")

    records = {}
    for row in commands:
        if not isinstance(row, dict):
            fail("Final gate command entry is not an object")
        action = next((row.get(key) for key in ("name", "stage", "action", "id")
                       if row.get(key) in EXPECTED_COMMANDS), None)
        argv = row.get("argv")
        if action is None and isinstance(argv, list):
            action = next((token for token in argv if token in EXPECTED_COMMANDS), None)
        if action not in EXPECTED_COMMANDS or action in records:
            fail("Final gate commands do not uniquely identify all four required actions")
        records[action] = row
    if set(records) != set(EXPECTED_COMMANDS):
        fail("Final gate is missing one or more required commands")
    return records


def command_passed(row: dict) -> bool:
    if row.get("exit_code") is not None:
        return row.get("exit_code") == 0
    if row.get("returncode") is not None:
        return row.get("returncode") == 0
    return row.get("passed") is True or row.get("status") in ("passed", "success", "succeeded")


def verify_gate_commands(gate: dict, label: str) -> dict:
    records = extract_command_records(gate.get("commands"))
    for action in EXPECTED_COMMANDS:
        row = records[action]
        if not command_passed(row):
            fail(f"{label} gate command did not pass: {action}")
        argv = row.get("argv")
        if isinstance(argv, list):
            if action == "fmt" and "--check" not in argv:
                fail(f"{label} format gate is not a check")
            if action == "build" and "--release" not in argv:
                fail(f"{label} build gate is not a release build")
    tests = gate.get("tests")
    if not isinstance(tests, dict) or "failed" not in tests or tests.get("failed") != 0:
        fail(f"{label} gate test summary is missing or reports failures")
    passed = tests.get("passed")
    if passed is not None and (isinstance(passed, bool) or not isinstance(passed, int) or passed <= 0):
        fail(f"{label} gate test summary has no passing tests")
    return {"commands": list(EXPECTED_COMMANDS), "tests": tests}


def verify_binary_record(record: dict, expected_path: Path, label: str) -> dict:
    path = expected_path.resolve(strict=True)
    if not path.is_file() or not os.access(path, os.X_OK):
        fail(f"{label} binary is missing or not executable: {path}")
    declared_path = Path(str(record.get("path", ""))).resolve()
    actual_hash = sha256_file(path)
    actual_bytes = path.stat().st_size
    if declared_path != path:
        fail(f"{label} binary path differs from its gate or frozen manifest")
    if record.get("sha256") != actual_hash:
        fail(f"{label} binary SHA-256 differs from its gate or frozen manifest")
    if record.get("bytes") is not None and record.get("bytes") != actual_bytes:
        fail(f"{label} binary size differs from its gate or frozen manifest")
    return {"path": path.as_posix(), "sha256": actual_hash, "bytes": actual_bytes}


def validate_baseline(plan: dict) -> dict:
    before_path = Path(plan["baseline"]["manifest_path"]).resolve(strict=True)
    if before_path != BEFORE_PATH.resolve(strict=True):
        fail("Baseline manifest path differs from this controller's frozen baseline")
    before_hash = sha256_file(before_path)
    if before_hash != plan["baseline"]["manifest_sha256"]:
        fail("Frozen baseline before.json changed since plan preparation")
    before = read_json(before_path)
    source_root = Path(before["source_directory"]).resolve(strict=True)
    source_hashes = before.get("source_sha256", {})
    if not source_hashes or source_snapshot(source_root) != source_hashes:
        fail("Frozen baseline source copy differs from before.json")
    if source_hashes != plan["baseline"]["source_sha256"]:
        fail("Frozen baseline source inventory differs from the measurement plan")

    binary_record = before.get("binary", {})
    binary_path = Path(binary_record.get("path", "")).resolve(strict=True)
    if binary_path != Path(plan["baseline"]["binary"]["path"]).resolve():
        fail("Frozen baseline binary path differs from the measurement plan")
    baseline_binary = verify_binary_record(binary_record, binary_path, "Frozen baseline")
    if baseline_binary != plan["baseline"]["binary"]:
        fail("Frozen baseline binary differs from the measurement plan")

    producer_path = Path(before["producer_gate"]).resolve(strict=True)
    if producer_path != Path(plan["baseline"]["producer_gate"]["path"]).resolve():
        fail("Frozen baseline producer gate path differs from the measurement plan")
    producer_hash = sha256_file(producer_path)
    if producer_hash != plan["baseline"]["producer_gate"]["sha256"]:
        fail("Frozen baseline producer gate changed since plan preparation")
    producer = read_json(producer_path)
    if producer.get("candidate") != "baseline":
        fail("Frozen baseline producer gate identity is not baseline")
    producer_source = Path(producer.get("source_directory", "")).resolve(strict=True)
    if source_snapshot(producer_source) != source_hashes:
        fail("Frozen baseline producer gate source differs from before.json")
    if producer.get("source_sha256") != source_hashes:
        fail("Frozen baseline producer gate source inventory differs from before.json")
    producer_binary = verify_binary_record(
        producer.get("binary", {}), Path(producer["binary"]["path"]),
        "Frozen baseline producer gate")
    if producer_binary["sha256"] != baseline_binary["sha256"]:
        fail("Frozen baseline binary is not the binary recorded by its producer gate")
    gate_summary = verify_gate_commands(producer, "Frozen baseline producer")
    return {
        "manifest_path": before_path.as_posix(), "manifest_sha256": before_hash,
        "source_directory": source_root.as_posix(), "source_sha256": source_hashes,
        "binary": baseline_binary,
        "producer_gate": {"path": producer_path.as_posix(), "sha256": producer_hash,
                          "binary": producer_binary, **gate_summary},
    }


def validate_integrated(plan: dict) -> dict:
    gate_path = INTEGRATED_GATE.resolve(strict=True)
    gate = read_json(gate_path)
    source_hashes = source_snapshot(REPO)
    if gate.get("source_sha256") != source_hashes:
        fail("Final integrated gate source inventory differs from the current repository")
    binary = verify_binary_record(gate.get("binary", {}), INTEGRATED_BINARY,
                                  "Final integrated")
    gate_summary = verify_gate_commands(gate, "Final integrated")
    return {
        "gate_path": gate_path.as_posix(), "gate_sha256": sha256_file(gate_path),
        "source_directory": REPO.resolve().as_posix(), "source_sha256": source_hashes,
        "binary": binary, **gate_summary,
    }


def pinned_case_record(case: dict) -> dict:
    config = Path(case["config"]).resolve(strict=True)
    generation_path = config.parent / "generation.json"
    generation = read_json(generation_path)
    generation_hash = sha256_file(generation_path)
    if generation_hash != case["generation_sha256"]:
        fail(f"Generated workload generation.json changed: {case['name']}")
    inputs = generation.get("input_sha256", {})
    if inputs != case["generation_input_sha256"]:
        fail(f"Generated workload input inventory changed: {case['name']}")
    for relative, expected in sorted(inputs.items()):
        input_path = config.parent / relative
        actual = sha256_file(input_path) if input_path.is_file() else None
        if actual != expected:
            fail(f"Generated workload input hash mismatch: {input_path}")
    actual_config_hash = sha256_file(config)
    if actual_config_hash != case["config_sha256"]:
        fail(f"Generated workload config changed: {config}")
    if inputs.get(config.name) != actual_config_hash:
        fail(f"Config hash does not match generation manifest: {config}")
    rows = generation.get("scenarios", [])
    scenario = next((row for row in rows if row.get("ini") == config.name), None)
    expected_scenario = case["scenario"]
    if not isinstance(scenario, dict):
        fail(f"Generated workload scenario is missing: {config.name}")
    actual_scenario = {key: scenario.get(key) for key in
                       ("rho", "request_count_total", "request_count_per_sender", "workload")}
    if actual_scenario != expected_scenario:
        fail(f"Generated workload scenario differs from the measurement plan: {case['name']}")
    return {
        "config": config.as_posix(), "config_sha256": actual_config_hash,
        "generation_path": generation_path.as_posix(),
        "generation_sha256": generation_hash,
        "generation_input_sha256": inputs,
        "scenario": actual_scenario,
    }


def validate_attempts(path: Path, case: dict, pins: dict) -> dict:
    report = read_json(path)
    if report.get("schema_version") != 1:
        fail(f"Unexpected helper report schema: {path}")
    binaries = report.get("binaries", {})
    if list(binaries) != list(EXPECTED_BINARY_NAMES):
        fail(f"Helper binary order differs from baseline/integrated policy: {path}")
    for name in EXPECTED_BINARY_NAMES:
        declared = binaries[name]
        expected = pins[name]
        if Path(declared.get("path", "")).resolve() != Path(expected["path"]).resolve():
            fail(f"Helper binary path differs for {name}: {path}")
        if declared.get("sha256") != expected["sha256"]:
            fail(f"Helper binary hash differs for {name}: {path}")
        if sha256_file(Path(expected["path"])) != expected["sha256"]:
            fail(f"Binary changed during comparison: {name}")

    attempts = report.get("attempts", [])
    if len(attempts) != 8:
        fail(f"Expected 8 attempts (1 warmup + 3 measurements for each binary): {path}")
    per_binary = {name: [] for name in EXPECTED_BINARY_NAMES}
    per_repeat = {repeat: [] for repeat in range(4)}
    for attempt in attempts:
        name, repeat = attempt.get("binary"), attempt.get("repeat")
        if name not in per_binary or repeat not in per_repeat:
            fail(f"Unexpected attempt identity in {path}")
        per_binary[name].append(attempt)
        per_repeat[repeat].append(attempt)
        if attempt.get("kind") != ("warmup" if repeat == 0 else "measurement"):
            fail(f"Warmup/measurement label mismatch in {path}")
        if attempt.get("completed") is not True or attempt.get("returncode") != 0:
            fail(f"Incomplete or failed attempt in {path}: {attempt.get('evidence_directory')}")
        if attempt.get("manifest_verified") is not True or attempt.get("guard_reason") is not None:
            fail(f"Attempt failed manifest verification or hit a guard in {path}")
        if attempt.get("large_outputs_removed_after_recording") is not True:
            fail(f"Attempt does not record successful output cleanup in {path}")
        output_dir = Path(attempt.get("evidence_directory", "")) / "output"
        if output_dir.exists():
            fail(f"Hashed large output directory remains after attempt: {output_dir}")
        files = attempt.get("files", [])
        if {row.get("path") for row in files} != EXPECTED_OUTPUT_FILES or len(files) != 4:
            fail(f"Attempt manifest does not contain exactly four output files in {path}")
        for row in files:
            declared = row.get("declared", {})
            if row.get("path") != declared.get("name", declared.get("path")):
                fail(f"Attempt output path differs from its manifest in {path}")
            if str(row.get("bytes")) != str(declared.get("bytes", declared.get("size_bytes"))):
                fail(f"Attempt output size differs from its manifest in {path}")
            if row.get("sha256") != declared.get("sha256"):
                fail(f"Attempt output SHA-256 differs from its manifest in {path}")
        measurements = attempt.get("measurements", {})
        wall = measurements.get("wall_seconds")
        rss = attempt.get("max_rss_bytes")
        if (isinstance(wall, bool) or not isinstance(wall, (int, float))
                or not math.isfinite(wall) or wall < 0):
            fail(f"Invalid GNU time wall measurement in {path}")
        if isinstance(rss, bool) or not isinstance(rss, int) or rss < 0:
            fail(f"Invalid GNU time RSS measurement in {path}")
        if not measurements.get("max_rss_kib") or rss != measurements["max_rss_kib"] * 1024:
            fail(f"GNU time RSS bytes do not match its recorded KiB value in {path}")

    for repeat in range(4):
        expected_order = (list(EXPECTED_BINARY_NAMES) if repeat % 2 == 0
                          else list(reversed(EXPECTED_BINARY_NAMES)))
        if [row.get("binary") for row in per_repeat[repeat]] != expected_order:
            fail(f"Forward/reverse serial order mismatch on repeat {repeat} in {path}")
    for name, rows in per_binary.items():
        if len(rows) != 4 or sum(row.get("kind") == "measurement" for row in rows) != 3:
            fail(f"Expected one warmup and three measured attempts for {name} in {path}")

    deterministic_hashes = [row.get("deterministic") for row in attempts]
    expected_hash_keys = {
        "simulation_sha256", "events.csv_normalized_sha256", "summary.csv_normalized_sha256"
    }
    if (not deterministic_hashes[0]
            or set(deterministic_hashes[0]) != expected_hash_keys
            or any(value != deterministic_hashes[0] for value in deterministic_hashes[1:])):
        fail(f"Simulation JSON or run_id-normalized CSV hashes differ in {path}")
    diagnostics_hashes = []
    for attempt in attempts:
        diagnostics = [row for row in attempt["files"]
                       if row.get("path") == "diagnostics.jsonl"]
        if len(diagnostics) != 1 or not diagnostics[0].get("sha256"):
            fail(f"Attempt lacks diagnostics.jsonl manifest hash in {path}")
        diagnostics_hashes.append(diagnostics[0]["sha256"])
    if any(value != diagnostics_hashes[0] for value in diagnostics_hashes[1:]):
        fail(f"diagnostics.jsonl hashes differ between attempts in {path}")
    if report.get("normalized_outputs_equal") is not True:
        fail(f"Helper did not confirm normalized output equality in {path}")

    raw_stats = {}
    for name, rows in per_binary.items():
        measured = sorted((row for row in rows if row.get("kind") == "measurement"),
                          key=lambda row: row["repeat"])
        wall_values = [row["measurements"]["wall_seconds"] for row in measured]
        rss_values = [row["max_rss_bytes"] for row in measured]
        raw_stats[name] = {
            "measurement_repeats": [row["repeat"] for row in measured],
            "wall_seconds": {
                "values": wall_values,
                "median": statistics.median(wall_values),
                "minimum": min(wall_values),
                "maximum": max(wall_values),
                "range": max(wall_values) - min(wall_values),
            },
            "max_rss_bytes": {
                "values": rss_values,
                "median": statistics.median(rss_values),
                "minimum": min(rss_values),
                "maximum": max(rss_values),
                "range": max(rss_values) - min(rss_values),
            },
        }
        helper_summary = report.get("summaries", {}).get(name, {})
        if helper_summary.get("completed") is not True:
            fail(f"Helper summary marks binary incomplete: {name} in {path}")
        if helper_summary.get("median_wall_seconds") != raw_stats[name]["wall_seconds"]["median"]:
            fail(f"Helper wall median differs from raw GNU time data: {name} in {path}")
        if helper_summary.get("maximum_rss_bytes") != raw_stats[name]["max_rss_bytes"]["maximum"]:
            fail(f"Helper RSS maximum differs from raw GNU time data: {name} in {path}")

    baseline, integrated = (raw_stats[name] for name in EXPECTED_BINARY_NAMES)
    wall_deltas = [
        per_repeat[repeat][1]["measurements"]["wall_seconds"]
        - per_repeat[repeat][0]["measurements"]["wall_seconds"]
        if [row["binary"] for row in per_repeat[repeat]] == list(EXPECTED_BINARY_NAMES)
        else next(row for row in per_repeat[repeat] if row["binary"] == "integrated")["measurements"]["wall_seconds"]
        - next(row for row in per_repeat[repeat] if row["binary"] == "baseline")["measurements"]["wall_seconds"]
        for repeat in (1, 2, 3)
    ]
    rss_deltas = [
        next(row for row in per_repeat[repeat] if row["binary"] == "integrated")["max_rss_bytes"]
        - next(row for row in per_repeat[repeat] if row["binary"] == "baseline")["max_rss_bytes"]
        for repeat in (1, 2, 3)
    ]

    def delta_summary(deltas: list[float | int]) -> dict:
        return {"by_measurement_repeat": deltas,
                "median": statistics.median(deltas),
                "minimum": min(deltas), "maximum": max(deltas),
                "range": max(deltas) - min(deltas)}

    baseline_wall = baseline["wall_seconds"]["median"]
    return {
        "case": case["name"], "attempt_count": 8,
        "normalized_hashes_equal": True,
        "normalized_hashes": deterministic_hashes[0],
        "diagnostics_sha256_equal": True,
        "diagnostics_sha256": diagnostics_hashes[0],
        "summaries_recomputed_from_gnu_time_attempts": raw_stats,
        "integrated_minus_baseline": {
            "median_wall_seconds": integrated["wall_seconds"]["median"] - baseline_wall,
            "wall_reduction_percent": ((baseline_wall - integrated["wall_seconds"]["median"])
                                        / baseline_wall * 100 if baseline_wall > 0 else None),
            "measurement_repeat_wall_delta_seconds": delta_summary(wall_deltas),
            "median_max_rss_bytes": (integrated["max_rss_bytes"]["median"]
                                      - baseline["max_rss_bytes"]["median"]),
            "maximum_max_rss_bytes": (integrated["max_rss_bytes"]["maximum"]
                                      - baseline["max_rss_bytes"]["maximum"]),
            "measurement_repeat_max_rss_delta_bytes": delta_summary(rss_deltas),
        },
        "comparison_design": (
            "One shared baseline and integrated serial round per repeat; run order reverses "
            "each round. Median differences and repeat-aligned deltas are descriptive, "
            "not isolated or statistically paired effects."
        ),
    }


def check_pins(baseline: dict, integrated: dict, plan_hash: str,
               controller_hash: str, helper_hashes: dict[str, str], case: dict) -> dict:
    checks = {"plan_sha256": sha256_file(PLAN_PATH),
              "controller_sha256": sha256_file(CONTROLLER_PATH),
              "before_sha256": sha256_file(BEFORE_PATH),
              "baseline_source_sha256": source_snapshot(Path(baseline["source_directory"])),
              "baseline_binary_sha256": sha256_file(Path(baseline["binary"]["path"])),
              "producer_gate_sha256": sha256_file(Path(baseline["producer_gate"]["path"])),
              "integrated_source_sha256": source_snapshot(REPO),
              "integrated_binary_sha256": sha256_file(Path(integrated["binary"]["path"])),
              "integrated_gate_sha256": sha256_file(Path(integrated["gate_path"])),
              "helper_sha256": {path: sha256_file(Path(path)) for path in helper_hashes},
              "case_inputs": pinned_case_record(case)}
    if checks["plan_sha256"] != plan_hash:
        fail("Measurement plan changed during the run")
    if checks["controller_sha256"] != controller_hash:
        fail("Measurement controller changed during the run")
    if checks["before_sha256"] != baseline["manifest_sha256"]:
        fail("Frozen baseline before.json changed during the run")
    if checks["baseline_source_sha256"] != baseline["source_sha256"]:
        fail("Frozen baseline source changed during the run")
    if checks["baseline_binary_sha256"] != baseline["binary"]["sha256"]:
        fail("Frozen baseline binary changed during the run")
    if checks["producer_gate_sha256"] != baseline["producer_gate"]["sha256"]:
        fail("Frozen baseline producer gate changed during the run")
    if checks["integrated_source_sha256"] != integrated["source_sha256"]:
        fail("Integrated source changed during the run")
    if checks["integrated_binary_sha256"] != integrated["binary"]["sha256"]:
        fail("Integrated binary changed during the run")
    if checks["integrated_gate_sha256"] != integrated["gate_sha256"]:
        fail("Final integrated gate changed during the run")
    for path, expected in helper_hashes.items():
        if checks["helper_sha256"][path] != expected:
            fail(f"Measurement helper changed during the run: {path}")
    return checks


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true",
                        help="run the 24 pinned measurements after all provenance checks pass")
    args = parser.parse_args()
    if not args.execute:
        parser.error("measurement is disabled unless --execute is supplied explicitly")

    plan = read_json(PLAN_PATH)
    if plan.get("schema_version") != 1:
        fail("Unsupported measurement plan schema")
    if plan.get("status") != "prepared_waiting_final_gate":
        fail("Measurement plan is not in its prepared state")
    plan_hash = sha256_file(PLAN_PATH)
    controller_script_path = Path(__file__).resolve(strict=True)
    if controller_script_path != Path(plan["measurement_controller"]).resolve():
        fail("Measurement controller path differs from the prepared plan")
    controller_hash = sha256_file(controller_script_path)
    if controller_hash != plan.get("measurement_controller_sha256"):
        fail("Measurement controller differs from the prepared plan")
    helper_hashes = plan["measurement_helpers"]["sha256"]
    for path, expected in helper_hashes.items():
        if sha256_file(Path(path)) != expected:
            fail(f"Measurement helper differs from the prepared plan: {path}")

    baseline = validate_baseline(plan)
    integrated = validate_integrated(plan)
    cases = plan.get("cases", [])
    if [row.get("name") for row in cases] != [
            "n16000-rho030", "n3200-rho090", "n3200-rho120"]:
        fail("Measurement cases or order differ from the approved three-case plan")
    for case in cases:
        pinned_case_record(case)

    helper = HELPER.resolve(strict=True)
    measurements_root = ROOT / "measurements"
    measurements_root.mkdir(parents=True, exist_ok=True)
    run_id = datetime.now(timezone.utc).strftime("run-%Y%m%dT%H%M%S.%fZ")
    run_dir = measurements_root / run_id
    run_dir.mkdir(exist_ok=False)
    controller_path = run_dir / "controller.json"
    baseline_binary = baseline["binary"]
    integrated_binary = integrated["binary"]
    binary_pins = {"baseline": baseline_binary, "integrated": integrated_binary}
    report = {
        "schema_version": 1,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "running",
        "qualification": (
            "Three scaled-input comparisons only; this is not a full-million-request "
            "acceptance verdict or claim."
        ),
        "measurement_plan": {"path": PLAN_PATH.as_posix(), "sha256": plan_hash},
        "measurement_controller": {"path": controller_script_path.as_posix(),
                                   "sha256": controller_hash},
        "measurement_helpers": helper_hashes,
        "baseline": baseline,
        "integrated": integrated,
        "cases": cases,
        "comparison_policy": plan["comparison_policy"],
        "comparisons": [],
    }
    write_json(controller_path, report)

    try:
        for case in cases:
            case_entry = {
                "id": case["name"], "status": "starting", "case": case["name"],
                "pre_case_pins": None, "post_case_pins": None,
            }
            report["comparisons"].append(case_entry)
            write_json(controller_path, report)
            case_entry["pre_case_pins"] = check_pins(
                baseline, integrated, plan_hash, controller_hash, helper_hashes, case)
            output_info = case_entry["pre_case_pins"]["case_inputs"]
            if output_info["config_sha256"] != case["config_sha256"]:
                fail(f"Pre-case input record changed: {case['name']}")

            case_dir = run_dir / "cases" / case["name"]
            case_dir.mkdir(parents=True, exist_ok=False)
            comparison_dir = case_dir / "helper-output"
            argv = [sys.executable, helper.as_posix(),
                    "--binary", f"baseline={baseline_binary['path']}",
                    "--binary", f"integrated={integrated_binary['path']}",
                    "--config", case["config"],
                    "--output", comparison_dir.as_posix()]
            case_entry.update({
                "status": "running", "helper": helper.as_posix(),
                "helper_report_path": (comparison_dir / "comparison.json").as_posix(),
                "comparison_directory": comparison_dir.as_posix(),
                "stdout_path": (case_dir / "helper.stdout.txt").as_posix(),
                "stderr_path": (case_dir / "helper.stderr.txt").as_posix(),
                "argv": argv,
            })
            write_json(controller_path, report)
            process = None
            launch_error = None
            try:
                with Path(case_entry["stdout_path"]).open("w", encoding="utf-8") as stdout, \
                        Path(case_entry["stderr_path"]).open("w", encoding="utf-8") as stderr:
                    process = subprocess.run(argv, cwd=REPO, stdout=stdout, stderr=stderr,
                                             check=False)
            except OSError as error:
                launch_error = repr(error)
            case_entry["helper_returncode"] = process.returncode if process else None
            case_entry["observed_attempt_count"] = (
                len(read_json(Path(case_entry["helper_report_path"])).get("attempts", []))
                if Path(case_entry["helper_report_path"]).is_file() else 0)
            write_json(controller_path, report)

            validation_error = None
            if process is None:
                validation_error = f"Measurement helper could not start: {launch_error}"
            elif process.returncode == 0:
                try:
                    case_entry["validation"] = validate_attempts(
                        Path(case_entry["helper_report_path"]), case, binary_pins)
                except (MeasurementError, OSError, KeyError, TypeError, ValueError) as error:
                    validation_error = str(error)
            else:
                validation_error = f"Measurement helper exited with status {process.returncode}"

            try:
                case_entry["post_case_pins"] = check_pins(
                    baseline, integrated, plan_hash, controller_hash, helper_hashes, case)
            except (MeasurementError, OSError, KeyError, TypeError, ValueError) as error:
                post_error = str(error)
                case_entry["post_case_pin_error"] = post_error
                validation_error = (f"{validation_error}; {post_error}" if validation_error
                                    else post_error)
            if validation_error:
                case_entry["status"] = "failed"
                case_entry["failure"] = validation_error
                write_json(controller_path, report)
                fail(f"Case {case['name']} failed; evidence is retained in {case_dir}")
            case_entry["status"] = "complete"
            write_json(controller_path, report)

        report["status"] = "complete"
        report["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        report["completed_case_count"] = len(report["comparisons"])
        report["attempt_count"] = sum(row.get("observed_attempt_count", 0)
                                       for row in report["comparisons"])
        if report["attempt_count"] != 24:
            fail("Completed measurement set does not contain exactly 24 attempts")
        write_json(controller_path, report)
        print(json.dumps({"status": report["status"],
                          "controller": controller_path.as_posix(),
                          "cases": report["completed_case_count"],
                          "attempts": report["attempt_count"]}, ensure_ascii=False))
        return 0
    except BaseException as error:
        report["status"] = "failed_or_interrupted"
        report["failure"] = repr(error)
        report["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        write_json(controller_path, report)
        raise


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (MeasurementError, OSError, KeyError, TypeError, ValueError) as error:
        print(f"integrated measurement controller: {error}", file=sys.stderr)
        raise SystemExit(1)
