#!/usr/bin/env python3
"""Serially compare each pinned candidate binary with the fixed baseline.

This controller invokes the repository's measurement helper once per case and
candidate. It is deliberately not a build or benchmark command until run by the
owner after scheduling; importing or compiling this file performs no measurement.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import statistics
import subprocess
import sys
from datetime import datetime, timezone


ROOT = Path("/tmp/dir-opt-five-2026-10-08")
REPO = Path("/home/hideki/DIR-Simulator")
DEFAULT_BASELINE_MANIFEST = ROOT / "baseline.json"
MEASUREMENT_HELPER = REPO / "scripts/performance/measure_can_export.py"
VARIANTS = ("candidate1", "candidate2", "candidate3", "candidate4", "candidate5")
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
EXPECTED_OUTPUT_FILES = {
    "diagnostics.jsonl", "events.csv", "results.json", "summary.csv"
}


def fail(message: str) -> None:
    raise RuntimeError(message)


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
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def source_snapshot(source_root: Path) -> dict[str, str]:
    if not source_root.is_dir():
        fail(f"Source snapshot directory is missing: {source_root}")
    cargo_files = [source_root / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    crate_files = (source_root / "crates").rglob("*")
    files = [path for path in (*crate_files, *cargo_files) if path.is_file() and not path.is_symlink()]
    if not files:
        fail(f"Source snapshot is empty: {source_root}")
    return {
        path.relative_to(source_root).as_posix(): sha256_file(path)
        for path in sorted(files)
    }


def pinned_input_record(config: Path) -> dict:
    config = config.resolve(strict=True)
    generation_path = config.parent / "generation.json"
    generation = read_json(generation_path)
    pinned = generation.get("input_sha256", {})
    if not pinned or config.name not in pinned:
        fail(f"Config is not pinned by {generation_path}: {config.name}")
    verified = {}
    for relative, expected in sorted(pinned.items()):
        input_path = config.parent / relative
        actual = sha256_file(input_path) if input_path.is_file() else None
        if actual != expected:
            fail(f"Generated workload input hash mismatch: {input_path}")
        verified[relative] = actual
    return {
        "config": config.as_posix(),
        "config_sha256": sha256_file(config),
        "generation_path": generation_path.as_posix(),
        "generation_sha256": sha256_file(generation_path),
        "request_count_per_configuration": generation.get("request_count_per_configuration"),
        "scenario": next((row for row in generation.get("scenarios", []) if row.get("ini") == config.name), None),
        "pinned_inputs_sha256": verified,
    }


def validate_gate_association(name: str, source_dir: Path, binary_path: Path,
                              source_root: Path) -> dict:
    latest = source_root / "gates" / name / "latest.json"
    gate = read_json(latest)
    if gate.get("candidate") != name:
        fail(f"Gate report candidate identity mismatch: {latest}")
    if Path(gate.get("source_directory", "")).resolve() != source_dir.resolve():
        fail(f"Gate report source directory mismatch: {latest}")
    commands = gate.get("commands", [])
    actions = [
        argv[1] if isinstance(argv, list) and len(argv) > 1 else None
        for argv in (row.get("argv") for row in commands)
    ]
    if len(commands) != 4 or actions != ["fmt", "clippy", "test", "build"]:
        fail(f"Gate report does not contain fmt/clippy/test/release build in order: {latest}")
    if any(row.get("exit_code") != 0 for row in commands):
        fail(f"Gate report contains a failed command: {latest}")
    if "--check" not in commands[0].get("argv", []):
        fail(f"Gate format command is not a check: {latest}")
    if "--release" not in commands[3].get("argv", []):
        fail(f"Gate build command is not a release build: {latest}")
    tests = gate.get("tests", {})
    if tests.get("failed") != 0:
        fail(f"Gate test summary reports failures: {latest}")

    source_hashes = source_snapshot(source_dir)
    if gate.get("source_sha256") != source_hashes:
        fail(f"Gate source hash inventory differs from the current source copy: {latest}")
    binary = gate.get("binary", {})
    binary_path = binary_path.resolve(strict=True)
    if Path(binary.get("path", "")).resolve() != binary_path:
        fail(f"Gate binary path does not match the selected binary: {latest}")
    actual_binary_sha = sha256_file(binary_path)
    if binary.get("sha256") != actual_binary_sha:
        fail(f"Gate binary hash does not match the selected binary: {latest}")
    if binary.get("bytes") is not None and binary.get("bytes") != binary_path.stat().st_size:
        fail(f"Gate binary size does not match the selected binary: {latest}")
    return {
        "latest_path": latest.as_posix(),
        "latest_sha256": sha256_file(latest),
        "candidate": name,
        "successful_commands": actions,
        "source_directory": source_dir.resolve().as_posix(),
        "source_sha256": source_hashes,
        "binary": {
            "path": binary_path.as_posix(),
            "bytes": binary_path.stat().st_size,
            "sha256": actual_binary_sha,
        },
    }


def validate_comparison(path: Path, variant_names: list[str], binary_records: dict[str, dict]) -> dict:
    report = read_json(path)
    if report.get("schema_version") != 1:
        fail(f"Unexpected helper report schema: {path}")
    if list(report.get("binaries", {})) != variant_names:
        fail(f"Helper report variants/order differ from the requested round: {path}")
    for name, pinned in binary_records.items():
        record = report["binaries"].get(name, {})
        if Path(record.get("path", "")).resolve() != Path(pinned["path"]).resolve():
            fail(f"Helper report binary path differs for {name}: {path}")
        if record.get("sha256") != pinned["sha256"]:
            fail(f"Helper report binary hash differs for {name}: {path}")
        if sha256_file(Path(pinned["path"])) != pinned["sha256"]:
            fail(f"Binary changed during comparison: {name}")

    attempts = report.get("attempts", [])
    expected_attempt_count = 4 * len(variant_names)
    if len(attempts) != expected_attempt_count:
        fail(f"Expected {expected_attempt_count} attempts (one warmup and three measurements per variant): {path}")
    per_variant = {name: [] for name in variant_names}
    by_repeat = {repeat: [] for repeat in range(4)}
    for attempt in attempts:
        name = attempt.get("binary")
        repeat = attempt.get("repeat")
        if name not in per_variant or repeat not in by_repeat:
            fail(f"Unexpected attempt identity in {path}")
        per_variant[name].append(attempt)
        by_repeat[repeat].append(attempt)
        if attempt.get("kind") != ("warmup" if repeat == 0 else "measurement"):
            fail(f"Warmup/measurement label mismatch in {path}")
        if attempt.get("completed") is not True or attempt.get("returncode") != 0:
            fail(f"Incomplete or failed attempt in {path}: {attempt.get('evidence_directory')}")
        if attempt.get("manifest_verified") is not True or attempt.get("guard_reason") is not None:
            fail(f"Attempt failed manifest or guard checks in {path}")
        if attempt.get("large_outputs_removed_after_recording") is not True:
            fail(f"Attempt output retention status is missing in {path}")
        output_dir = Path(attempt.get("evidence_directory", "")) / "output"
        if output_dir.exists():
            fail(f"Comparison output directory should have been removed: {output_dir}")
        files = attempt.get("files", [])
        if {row.get("path") for row in files} != EXPECTED_OUTPUT_FILES or len(files) != 4:
            fail(f"Attempt manifest does not contain the four standard outputs in {path}")
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
        if isinstance(wall, bool) or not isinstance(wall, (int, float)) or not math.isfinite(wall) or wall < 0:
            fail(f"Attempt wall measurement is invalid in {path}")
        if isinstance(rss, bool) or not isinstance(rss, int) or rss < 0:
            fail(f"Attempt RSS measurement is invalid in {path}")
    for repeat in range(4):
        expected = variant_names if repeat % 2 == 0 else list(reversed(variant_names))
        group = by_repeat[repeat]
        if [row.get("binary") for row in group] != expected:
            fail(f"Forward/reverse serial round order mismatch on repeat {repeat} in {path}")
    for name, rows in per_variant.items():
        if len(rows) != 4 or sum(row.get("kind") == "measurement" for row in rows) != 3:
            fail(f"Expected 1 warmup and 3 measurements for {name} in {path}")
    hashes = [row.get("deterministic") for row in attempts]
    if not hashes[0] or any(row != hashes[0] for row in hashes[1:]):
        fail(f"Normalized output hashes differ between attempts in {path}")
    diagnostics_hashes = []
    for attempt in attempts:
        diagnostics = [row for row in attempt["files"] if row.get("path") == "diagnostics.jsonl"]
        if len(diagnostics) != 1 or not diagnostics[0].get("sha256"):
            fail(f"Attempt lacks diagnostics.jsonl manifest hash in {path}")
        diagnostics_hashes.append(diagnostics[0]["sha256"])
    if any(digest != diagnostics_hashes[0] for digest in diagnostics_hashes[1:]):
        fail(f"diagnostics.jsonl hashes differ between variants or rounds in {path}")
    if report.get("normalized_outputs_equal") is not True:
        fail(f"Helper did not confirm deterministic output equality: {path}")

    summaries = report.get("summaries", {})
    calculated = {}
    for name, rows in per_variant.items():
        measured = [row for row in rows if row.get("kind") == "measurement"]
        median_wall = statistics.median(row["measurements"]["wall_seconds"] for row in measured)
        maximum_rss = max(row["max_rss_bytes"] for row in measured)
        summary = summaries.get(name, {})
        if summary.get("completed") is not True or summary.get("median_wall_seconds") != median_wall:
            fail(f"Helper summary differs from raw attempts for {name} in {path}")
        if summary.get("maximum_rss_bytes") != maximum_rss:
            fail(f"Helper RSS summary differs from raw attempts for {name} in {path}")
        calculated[name] = {
            "median_wall_seconds": median_wall,
            "measurement_wall_seconds": [row["measurements"]["wall_seconds"] for row in measured],
            "maximum_measured_rss_bytes": maximum_rss,
        }
    baseline_median = calculated["baseline"]["median_wall_seconds"]
    candidate_deltas = {}
    for name in variant_names:
        if name == "baseline":
            continue
        candidate_median = calculated[name]["median_wall_seconds"]
        candidate_deltas[name] = {
            "median_wall_delta_seconds_vs_shared_baseline": candidate_median - baseline_median,
            "median_wall_reduction_percent_vs_shared_baseline": (
                (baseline_median - candidate_median) / baseline_median * 100
                if baseline_median > 0 else None
            ),
            "maximum_measured_rss_delta_bytes_vs_shared_baseline": (
                calculated[name]["maximum_measured_rss_bytes"]
                - calculated["baseline"]["maximum_measured_rss_bytes"]
            ),
        }
    return {
        "report_path": path.as_posix(),
        "attempt_count": len(attempts),
        "normalized_hashes_equal": True,
        "normalized_hashes": hashes[0],
        "diagnostics_sha256_equal": True,
        "diagnostics_sha256": diagnostics_hashes[0],
        "summaries_recomputed_from_attempts": calculated,
        "candidate_deltas_vs_shared_baseline": candidate_deltas,
        "comparison_design": "shared baseline in forward/reverse rounds; candidate deltas use per-binary medians and are not paired A/B results",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, required=True,
                        help="Directory containing baseline, candidate1..candidate5, and optionally combined")
    parser.add_argument("--case", action="append", required=True, metavar="NAME=INI",
                        help="Pinned input case; repeat in execution order")
    parser.add_argument("--source-root", type=Path, default=ROOT,
                        help=f"Directory containing baseline.json, baseline-source, and candidate source copies (default: {ROOT})")
    args = parser.parse_args()

    binary_dir = args.binary_dir.resolve(strict=True)
    source_root = args.source_root.resolve(strict=True)
    manifest_path = source_root / "baseline.json"
    baseline_manifest = read_json(manifest_path)
    baseline_source_root = Path(baseline_manifest["source_directory"]).resolve(strict=True)
    manifest_source_hashes = baseline_manifest.get("source_sha256", {})
    if not manifest_source_hashes or source_snapshot(baseline_source_root) != manifest_source_hashes:
        fail("Baseline source tree differs from its prepared source hash manifest")

    binary_names = ["baseline", *VARIANTS]
    if (binary_dir / "combined").is_file():
        binary_names.append("combined")
    source_dirs = {"baseline": baseline_source_root}
    source_dirs.update({name: source_root / name for name in VARIANTS})
    if "combined" in binary_names:
        if not (source_root / "combined").is_dir():
            fail("A combined binary requires its matching combined source directory")
        source_dirs["combined"] = source_root / "combined"
    source_hashes = {name: source_snapshot(path) for name, path in source_dirs.items()}
    source_deltas = {}
    for name, snapshot in source_hashes.items():
        if name == "baseline":
            continue
        baseline_snapshot = source_hashes["baseline"]
        source_deltas[name] = {
            "added": sorted(set(snapshot) - set(baseline_snapshot)),
            "removed": sorted(set(baseline_snapshot) - set(snapshot)),
            "changed": sorted(path for path in set(snapshot) & set(baseline_snapshot)
                               if snapshot[path] != baseline_snapshot[path]),
        }

    binary_paths = {name: (binary_dir / name).resolve(strict=True) for name in binary_names}
    for name, path in binary_paths.items():
        if not path.is_file() or not os.access(path, os.X_OK):
            fail(f"Binary is missing or not executable: {name}={path}")
    binary_records = {
        name: {"path": path.as_posix(), "bytes": path.stat().st_size, "sha256": sha256_file(path)}
        for name, path in binary_paths.items()
    }
    expected_baseline = baseline_manifest.get("binary", {}).get("sha256")
    if binary_records["baseline"]["sha256"] != expected_baseline:
        fail("Selected baseline binary differs from baseline.json")
    expected_baseline_path = Path(baseline_manifest.get("binary", {}).get("path", "")).resolve()
    if binary_paths["baseline"] != expected_baseline_path:
        fail("Selected baseline binary path differs from baseline.json")

    gate_associations = {
        name: validate_gate_association(name, source_dirs[name], binary_paths[name], source_root)
        for name in binary_names
    }
    for name, association in gate_associations.items():
        if association["binary"]["sha256"] != binary_records[name]["sha256"]:
            fail(f"Gate/binary inventory mismatch for {name}")
    if baseline_manifest.get("source_sha256") != gate_associations["baseline"]["source_sha256"]:
        fail("Refreshed baseline.json source inventory differs from the successful baseline gate")

    cases = []
    seen = set()
    for value in args.case:
        name, separator, raw_path = value.partition("=")
        if not separator or not SAFE_NAME.fullmatch(name) or name in seen or not raw_path:
            parser.error("Each --case must be a unique NAME=INI using letters, digits, dot, underscore, or hyphen")
        seen.add(name)
        cases.append({"name": name, **pinned_input_record(Path(raw_path))})
    if not cases:
        parser.error("At least one --case is required")

    helper = MEASUREMENT_HELPER.resolve(strict=True)
    measurements_root = source_root / "measurements"
    measurements_root.mkdir(parents=True, exist_ok=True)
    run_id = datetime.now(timezone.utc).strftime("run-%Y%m%dT%H%M%S.%fZ")
    run_dir = measurements_root / run_id
    run_dir.mkdir(exist_ok=False)
    controller_path = run_dir / "controller.json"
    report = {
        "schema_version": 1,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "running",
        "qualification": "Scaled-input per-candidate comparison only; no full-million acceptance verdict.",
        "measurement_helper": helper.as_posix(),
        "source_root": source_root.as_posix(),
        "baseline_manifest": {
            "path": manifest_path.as_posix(),
            "sha256": sha256_file(manifest_path),
            "source_sha256": manifest_source_hashes,
        },
        "gate_associations": gate_associations,
        "source_snapshots": {
            name: {"path": source_dirs[name].as_posix(), "files": hashes}
            for name, hashes in source_hashes.items()
        },
        "source_deltas_from_baseline": source_deltas,
        "binaries": binary_records,
        "cases": cases,
        "comparison_policy": {
            "serial": True,
            "all_variants_share_one_baseline_round": True,
            "variant_order_forward": binary_names,
            "variant_order_reverse": list(reversed(binary_names)),
            "warmups_per_binary": 1,
            "measurements_per_binary": 3,
            "paired_ab_design": False,
            "delta_method": "per-candidate median versus the shared baseline median in the same case",
            "helper_wall_guard_seconds": 180,
            "address_space_guard_bytes": 26 * 1024**3,
            "large_outputs_retained": False,
            "full_million_acceptance_claim": False,
        },
        "comparisons": [],
    }
    write_json(controller_path, report)

    try:
        for case in cases:
            # Recheck all pinned inputs, source snapshots, and binaries immediately
            # before each helper run so every shared-baseline case has provenance.
            current_sources = {name: source_snapshot(path) for name, path in source_dirs.items()}
            if current_sources != source_hashes:
                fail("A source copy changed after the controller inventory was recorded")
            current_case = pinned_input_record(Path(case["config"]))
            if current_case != {key: value for key, value in case.items() if key != "name"}:
                fail(f"Pinned input changed before comparison: {case['name']}")
            for name, record in binary_records.items():
                if sha256_file(Path(record["path"])) != record["sha256"]:
                    fail(f"Binary changed before comparison: {name}")
                gate_path = Path(gate_associations[name]["latest_path"])
                if sha256_file(gate_path) != gate_associations[name]["latest_sha256"]:
                    fail(f"Gate association changed before comparison: {name}")

            comparison_id = case["name"]
            comparison_dir = run_dir / "comparisons" / comparison_id
            argv = [sys.executable, helper.as_posix()]
            for name in binary_names:
                argv.extend(("--binary", f"{name}={binary_records[name]['path']}"))
            argv.extend(("--config", case["config"], "--output", comparison_dir.as_posix()))
            entry = {
                "id": comparison_id,
                "status": "running",
                "case": case["name"],
                "config": case["config"],
                "input_generation_sha256": case["generation_sha256"],
                "source_snapshots": source_hashes,
                "source_deltas_from_baseline": source_deltas,
                "binary_hashes": binary_records,
                "argv": argv,
                "comparison_directory": comparison_dir.as_posix(),
            }
            report["comparisons"].append(entry)
            write_json(controller_path, report)
            process = subprocess.run(argv, capture_output=True, text=True, check=False)
            stdout_path = comparison_dir.parent / f"{comparison_id}.stdout.txt"
            stderr_path = comparison_dir.parent / f"{comparison_id}.stderr.txt"
            stdout_path.parent.mkdir(parents=True, exist_ok=True)
            stdout_path.write_text(process.stdout, encoding="utf-8")
            stderr_path.write_text(process.stderr, encoding="utf-8")
            entry["returncode"] = process.returncode
            entry["stdout_path"] = stdout_path.as_posix()
            entry["stderr_path"] = stderr_path.as_posix()
            comparison_report_path = comparison_dir / "comparison.json"
            entry["helper_report_path"] = comparison_report_path.as_posix()
            if process.returncode != 0:
                entry["status"] = "failed"
                write_json(controller_path, report)
                fail(f"Measurement helper failed for {comparison_id}, exit {process.returncode}")
            entry["validation"] = validate_comparison(comparison_report_path, binary_names, binary_records)
            current_sources = {name: source_snapshot(path) for name, path in source_dirs.items()}
            if current_sources != source_hashes:
                fail("A source copy changed during a comparison")
            for name, record in binary_records.items():
                if sha256_file(Path(record["path"])) != record["sha256"]:
                    fail(f"Binary changed during comparison: {name}")
                gate_path = Path(gate_associations[name]["latest_path"])
                if sha256_file(gate_path) != gate_associations[name]["latest_sha256"]:
                    fail(f"Gate association changed during comparison: {name}")
            current_case = pinned_input_record(Path(case["config"]))
            if current_case != {key: value for key, value in case.items() if key != "name"}:
                fail(f"Pinned input changed during comparison: {case['name']}")
            entry["status"] = "complete"
            write_json(controller_path, report)
        report["status"] = "complete"
        report["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
        report["completed_comparison_count"] = len(report["comparisons"])
        report["attempt_count"] = 4 * len(binary_names) * len(report["comparisons"])
        write_json(controller_path, report)
        print(json.dumps({"status": report["status"], "controller": controller_path.as_posix(),
                          "comparisons": report["completed_comparison_count"], "attempts": report["attempt_count"]},
                         ensure_ascii=False))
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
    except (RuntimeError, OSError, KeyError, TypeError, ValueError) as error:
        print(f"candidate controller: {error}", file=sys.stderr)
        raise SystemExit(1)
