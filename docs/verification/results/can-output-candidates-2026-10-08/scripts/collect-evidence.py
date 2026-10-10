#!/usr/bin/env python3
"""Collect and validate five-candidate CAN export comparison evidence.

This script is intentionally inert until called explicitly with a completed
measurement controller. It never builds binaries or starts measurements.
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import statistics
import sys
import tarfile
from datetime import datetime, timezone

ROOT = Path("/tmp/dir-opt-five-2026-10-08")
REPO = Path("/home/hideki/DIR-Simulator")
RESULTS = REPO / "docs/verification/results"
DOC_ID = "can-output-candidates-2026-10-08"
VARIANTS = ("baseline", "candidate1", "candidate2", "candidate3", "candidate4", "candidate5")
EXPECTED_CASES = ("n16000-rho030", "n3200-rho090", "n3200-rho120")
EXPECTED_FILES = {"diagnostics.jsonl", "events.csv", "results.json", "summary.csv"}
EXPECTED_BASELINE_SOURCE_FILES = 183
MAX_MEASUREMENT_FILE_BYTES = 10 * 1024 * 1024
SENSITIVE_NAMES = {".env", ".netrc", ".npmrc", ".pypirc", "credentials", "credentials.json",
                   "token", "token.json", "access_token", "auth.json", "id_rsa", "id_ed25519"}
SENSITIVE_PARTS = ("credential", "secret", "private-key", "private_key")
BINARY_SUFFIXES = {".bin", ".exe", ".so", ".dylib", ".dll", ".o", ".rlib", ".pyc",
                   ".png", ".jpg", ".jpeg", ".gif", ".pdf", ".wasm", ".zip", ".gz",
                   ".tar", ".data", ".sqlite", ".db", ".pem", ".key"}
SOURCE_TREE_EXCLUDED_DIRS = {".git", "target", "__pycache__", ".cache", "cache", ".pytest_cache", ".mypy_cache"}
BINARY_SHA_CACHE: dict[str, str] = {}
TEST_RESULT_RE = re.compile(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;")
DIAGNOSTIC_RE = re.compile(r"FAILED|error:|symlink input is unsupported|assertion .* failed|git_commit", re.IGNORECASE)


def fail(message: str) -> None:
    raise RuntimeError(message)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path):
    if path.is_symlink() or not path.is_file():
        fail(f"Required regular file is missing: {path}")
    return json.loads(path.read_text(encoding="utf-8"))


def require_regular(path: Path) -> Path:
    if path.is_symlink() or not path.is_file():
        fail(f"Required regular file is missing: {path}")
    return path


def safe_relative(raw: str) -> PurePosixPath:
    path = PurePosixPath(raw)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        fail(f"Unsafe relative path: {raw}")
    return path


def copy_file(source: Path, stage: Path, relative: str, inventory: list[dict], *,
              kind: str = "copied_file", generated_from: list[str] | None = None) -> dict:
    require_regular(source)
    rel = safe_relative(relative)
    destination = stage.joinpath(*rel.parts)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        fail(f"Support destination already exists: {destination}")
    shutil.copyfile(source, destination)
    source_sha = sha256_file(source)
    copied_sha = sha256_file(destination)
    if source_sha != copied_sha:
        fail(f"Copied file hash mismatch: {source}")
    record = {
        "kind": kind,
        "original_local_path": source.resolve().as_posix(),
        "support_relative_path": rel.as_posix(),
        "bytes": destination.stat().st_size,
        "sha256": copied_sha,
    }
    if generated_from:
        record["generated_from"] = generated_from
    inventory.append(record)
    return record


def write_generated(data: bytes, stage: Path, relative: str, inventory: list[dict],
                    generated_from: list[str], kind: str) -> dict:
    rel = safe_relative(relative)
    destination = stage.joinpath(*rel.parts)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        fail(f"Generated support destination already exists: {destination}")
    destination.write_bytes(data)
    record = {
        "kind": kind,
        "original_local_path": None,
        "generated_from": generated_from,
        "support_relative_path": rel.as_posix(),
        "bytes": len(data),
        "sha256": sha256_file(destination),
    }
    inventory.append(record)
    return record


def is_binary_file(path: Path) -> bool:
    if path.suffix.lower() in BINARY_SUFFIXES:
        return True
    try:
        with path.open("rb") as stream:
            head = stream.read(4)
    except OSError:
        return True
    return head.startswith(b"\x7fELF") or head in {
        b"\xfe\xed\xfa\xce", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf",
        b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe", b"MZ\x90\x00",
    }


def is_sensitive_path(path: Path) -> bool:
    components = [part.lower() for part in path.parts]
    return any(part in SENSITIVE_NAMES or part.startswith(".env") or part in {".ssh", ".aws"}
               or any(marker in part for marker in SENSITIVE_PARTS) for part in components)


def write_baseline_source_archive(source_root: Path, stage: Path, inventory: list[dict]) -> dict:
    archive_rel = "source-snapshots/baseline-source.tar.gz"
    destination = stage / archive_rel
    destination.parent.mkdir(parents=True, exist_ok=True)
    included = 0
    docs_specs_count = 0
    excluded = []
    with tarfile.open(destination, mode="w:gz", compresslevel=6) as archive:
        for path in sorted(source_root.rglob("*")):
            relative = path.relative_to(source_root)
            if any(part.lower() in SOURCE_TREE_EXCLUDED_DIRS for part in relative.parts):
                continue
            if path.is_symlink():
                excluded.append({"path": relative.as_posix(), "reason": "symlink_not_archived"})
                continue
            if path.is_dir():
                continue
            if not path.is_file():
                excluded.append({"path": relative.as_posix(), "reason": "non_regular_file_not_archived"})
                continue
            if is_sensitive_path(relative):
                excluded.append({"path": relative.as_posix(), "reason": "credential_or_secret_name"})
                continue
            if is_binary_file(path):
                excluded.append({"path": relative.as_posix(), "reason": "binary_file_not_archived"})
                continue
            archive.add(path, arcname=relative.as_posix(), recursive=False)
            included += 1
            if relative.parts[:2] == ("docs", "specs"):
                docs_specs_count += 1
    with tarfile.open(destination, mode="r:gz") as check:
        members = [member.name for member in check.getmembers()]
    if not any(member.startswith("docs/specs/") for member in members):
        fail("Baseline reproduction archive is missing docs/specs build inputs")
    if not any(member.startswith("docs/verification/fixtures/") for member in members):
        fail("Baseline reproduction archive is missing copied fixture inputs")
    if any(any(part in SOURCE_TREE_EXCLUDED_DIRS for part in PurePosixPath(member).parts) for member in members):
        fail("Baseline reproduction archive contains a forbidden repository/cache directory")
    ledger_relative = "docs/third-party/採用物台帳.md"
    ledger_present = (source_root / ledger_relative).is_file()
    if ledger_present != (ledger_relative in members):
        fail("Baseline reproduction archive does not preserve conditional adoption-ledger presence")
    record = {
        "kind": "compressed_baseline_source_reproduction_snapshot",
        "original_local_path": source_root.resolve().as_posix(),
        "support_relative_path": archive_rel,
        "bytes": destination.stat().st_size,
        "sha256": sha256_file(destination),
        "archived_regular_files": included,
        "archived_docs_specs_files": docs_specs_count,
        "adoption_ledger_path": ledger_relative,
        "adoption_ledger_present_in_frozen_tree": ledger_present,
        "adoption_ledger_build_value": "present" if ledger_present else "not-present",
        "excluded_entries": excluded,
        "exclusions": [".git", "target directories", "cache directories", "symlinks",
                       "credential-named files", "binary files"],
    }
    inventory.append(record)
    return {key: value for key, value in record.items() if key != "kind"}


def source_snapshot(source_root: Path) -> dict[str, str]:
    if not source_root.is_dir():
        fail(f"Source snapshot directory is missing: {source_root}")
    files = [*source_root.joinpath("crates").rglob("*"),
             *(source_root / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"))]
    files = [path for path in files if path.is_file() and not path.is_symlink()]
    return {path.relative_to(source_root).as_posix(): sha256_file(path) for path in sorted(files)}


def verified_binary(path: Path, expected_sha256: str) -> dict:
    require_regular(path)
    resolved = path.resolve()
    key = resolved.as_posix()
    actual = BINARY_SHA_CACHE.get(key)
    if actual is None:
        actual = sha256_file(resolved)
        BINARY_SHA_CACHE[key] = actual
    if actual != expected_sha256:
        fail(f"Local binary SHA-256 differs from recorded hash: {resolved}")
    return {"path": resolved.as_posix(), "bytes": resolved.stat().st_size,
            "sha256": actual, "local_hash_verified": True, "copied_into_support": False}


def validate_gate(name: str, source_root: Path, expected_source: dict[str, str] | None = None) -> tuple[dict, dict]:
    latest_path = ROOT / "gates" / name / "latest.json"
    gate = read_json(latest_path)
    if gate.get("candidate") != name:
        fail(f"Gate identity does not match {name}: {latest_path}")
    if Path(gate.get("source_directory", "")).resolve() != source_root.resolve():
        fail(f"Gate source directory differs for {name}: {latest_path}")
    commands = gate.get("commands", [])
    actions = [Path(row.get("argv", ["", "?"])[1]).name for row in commands]
    if len(commands) != 4 or actions != ["fmt", "clippy", "test", "build"]:
        fail(f"Expected four ordered Rust gates for {name}: {latest_path}")
    if any(row.get("exit_code") != 0 for row in commands):
        fail(f"A latest Rust gate failed for {name}: {latest_path}")
    if "--check" not in commands[0].get("argv", []):
        fail(f"Latest format command is not a check for {name}")
    if "--release" not in commands[3].get("argv", []):
        fail(f"Latest build command is not release mode for {name}")
    tests = gate.get("tests", {})
    if tests.get("failed") != 0 or not isinstance(tests.get("passed"), int) or tests["passed"] <= 0:
        fail(f"Latest test totals are not passing for {name}")

    actual_sources = source_snapshot(source_root)
    if gate.get("source_sha256") != actual_sources:
        fail(f"Latest gate source inventory differs from local source for {name}")
    if expected_source is not None and actual_sources != expected_source:
        fail(f"Source hash map differs from the baseline manifest for {name}")
    binary = gate.get("binary", {})
    binary_ref = verified_binary(Path(binary.get("path", "")), binary.get("sha256", ""))
    if binary.get("bytes") != binary_ref["bytes"]:
        fail(f"Latest gate binary size differs for {name}")
    if not isinstance(binary_ref["sha256"], str) or not binary_ref["sha256"]:
        fail(f"Latest gate binary hash is missing for {name}")

    copied_logs = []
    for index, row in enumerate(commands):
        log_path = require_regular(Path(row.get("log", "")))
        log_sha = sha256_file(log_path)
        if row.get("log_sha256") != log_sha:
            fail(f"Latest gate log hash mismatch for {name} command {index}")
        copied_logs.append({"index": index, "action": actions[index], "path": log_path,
                            "sha256": log_sha, "exit_code": row["exit_code"]})
    association = {
        "latest_report_path": latest_path.as_posix(),
        "latest_report_sha256": sha256_file(latest_path),
        "candidate": name,
        "source_directory": source_root.resolve().as_posix(),
        "source_file_count": len(actual_sources),
        "source_sha256": actual_sources,
        "commands": [{"action": actions[i], "exit_code": commands[i]["exit_code"],
                       "log_sha256": copied_logs[i]["sha256"]} for i in range(4)],
        "tests": tests,
        "binary": binary_ref,
    }
    return association, {"report_path": latest_path, "report": gate, "logs": copied_logs}


def historical_gates(name: str, latest_path: Path, stage: Path, inventory: list[dict]) -> list[dict]:
    root = ROOT / "gates" / name
    latest_sha = sha256_file(latest_path)
    histories = []
    if not root.is_dir():
        fail(f"Gate history directory is missing: {root}")
    for attempt_dir in sorted(path for path in root.iterdir() if path.is_dir()):
        report_path = attempt_dir / "report.json"
        if not report_path.is_file() or report_path.is_symlink():
            continue
        report_sha = sha256_file(report_path)
        if report_sha == latest_sha:
            continue
        old = read_json(report_path)
        commands = old.get("commands", [])
        failed_commands = [{"action": row.get("argv", ["", "?"])[1] if len(row.get("argv", [])) > 1 else None,
                            "exit_code": row.get("exit_code"), "log_path": row.get("log")}
                           for row in commands if row.get("exit_code") != 0]
        tests = old.get("tests", {})
        if failed_commands or tests.get("failed", 0):
            classification = "historical_failed_attempt_excluded_from_latest_gate_evidence"
        elif len(commands) < 4:
            classification = "historical_incomplete_attempt_excluded_from_latest_gate_evidence"
        else:
            classification = "superseded_attempt_excluded_from_latest_gate_evidence"
        copied_report = copy_file(report_path, stage,
                                  f"gates/history/{name}/{attempt_dir.name}/report.json", inventory,
                                  kind="historical_gate_report")
        copied_logs = []
        observed_tests = {"passed": 0, "failed": 0, "ignored": 0, "suites": 0}
        diagnostic_excerpt = []
        for index, row in enumerate(commands):
            raw_log = row.get("log")
            if not raw_log:
                continue
            log_path = Path(raw_log)
            if not log_path.is_file() or log_path.is_symlink():
                copied_logs.append({"index": index, "original_local_path": log_path.as_posix(),
                                    "missing": True, "recorded_sha256": row.get("log_sha256")})
                continue
            actual = sha256_file(log_path)
            if row.get("log_sha256") and actual != row["log_sha256"]:
                fail(f"Historical gate log hash mismatch: {log_path}")
            copied = copy_file(log_path, stage,
                               f"gates/history/{name}/{attempt_dir.name}/{index}.log", inventory,
                               kind="historical_gate_log")
            copied_logs.append({"index": index, "support_relative_path": copied["support_relative_path"],
                                "sha256": actual, "exit_code": row.get("exit_code")})
            if log_path.is_file():
                for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
                    match = TEST_RESULT_RE.search(line)
                    if match:
                        observed_tests["passed"] += int(match.group(1))
                        observed_tests["failed"] += int(match.group(2))
                        observed_tests["ignored"] += int(match.group(3))
                        observed_tests["suites"] += 1
                    if DIAGNOSTIC_RE.search(line):
                        diagnostic_excerpt.append(line.strip()[:500])
        histories.append({
            "candidate": name,
            "attempt_id": attempt_dir.name,
            "classification": classification,
            "original_report_path": report_path.resolve().as_posix(),
            "support_report_path": copied_report["support_relative_path"],
            "report_sha256": report_sha,
            "command_count": len(commands),
            "failed_commands": failed_commands,
            "test_totals": tests,
            "test_result_totals_from_logs": observed_tests,
            "diagnostic_excerpt_from_logs": diagnostic_excerpt[-12:],
            "logs": copied_logs,
        })
    return histories


def copy_source_patch(candidate: str, relative: str, baseline_root: Path, candidate_root: Path,
                      baseline_map: dict[str, str], candidate_map: dict[str, str],
                      stage: Path, inventory: list[dict]) -> dict:
    base_path = baseline_root / relative
    candidate_path = candidate_root / relative
    old_bytes = base_path.read_bytes() if relative in baseline_map else b""
    new_bytes = candidate_path.read_bytes() if relative in candidate_map else b""
    try:
        old_text = old_bytes.decode("utf-8")
        new_text = new_bytes.decode("utf-8")
    except UnicodeDecodeError:
        fail(f"Changed source is not UTF-8 text: {relative}")
    patch = "".join(difflib.unified_diff(
        old_text.splitlines(keepends=True), new_text.splitlines(keepends=True),
        fromfile=(f"a/{relative}" if relative in baseline_map else "/dev/null"),
        tofile=(f"b/{relative}" if relative in candidate_map else "/dev/null"),
    )).encode("utf-8")
    patch_ref = write_generated(patch, stage, f"patches/{candidate}/{relative}.patch", inventory,
                                [f"{baseline_root}/{relative}", f"{candidate_root}/{relative}"],
                                "unified_source_patch")
    candidate_ref = None
    if relative in candidate_map:
        candidate_ref = copy_file(candidate_path, stage,
                                  f"sources/{candidate}/{relative}", inventory,
                                  kind="changed_candidate_source")
        if candidate_ref["sha256"] != candidate_map[relative]:
            fail(f"Candidate source copy hash mismatch: {candidate}/{relative}")
    return {"path": relative, "baseline_present": relative in baseline_map,
            "candidate_present": relative in candidate_map,
            "baseline_sha256": baseline_map.get(relative),
            "candidate_sha256": candidate_map.get(relative),
            "candidate_support_path": candidate_ref["support_relative_path"] if candidate_ref else None,
            "patch_support_path": patch_ref["support_relative_path"],
            "patch_sha256": patch_ref["sha256"]}


def collect_inputs(cases: list[dict], stage: Path, inventory: list[dict]) -> tuple[dict, dict]:
    inputs_by_case = {}
    input_sets = {}
    for case in cases:
        config = require_regular(Path(case["config"]))
        generation_path = require_regular(config.parent / "generation.json")
        generation = read_json(generation_path)
        expected_inputs = generation.get("input_sha256", {})
        if not expected_inputs or config.name not in expected_inputs:
            fail(f"Case config is not pinned by generation.json: {config}")
        set_id = config.parent.name
        safe_relative(set_id)
        if set_id in input_sets and input_sets[set_id]["generation_sha256"] != sha256_file(generation_path):
            fail(f"Input set ID has conflicting generation files: {set_id}")
        if set_id not in input_sets:
            controller_pinned = case.get("pinned_inputs_sha256", {})
            if controller_pinned != expected_inputs:
                fail(f"Controller pinned-input map differs from generation.json for {case['name']}")
            generation_ref = copy_file(generation_path, stage, f"inputs/{set_id}/generation.json", inventory,
                                       kind="input_generation_manifest")
            pinned = {}
            for rel, expected in sorted(expected_inputs.items()):
                safe = safe_relative(rel).as_posix()
                source = require_regular(config.parent / safe)
                actual = sha256_file(source)
                if actual != expected:
                    fail(f"Pinned input hash mismatch: {source}")
                ref = copy_file(source, stage, f"inputs/{set_id}/{safe}", inventory,
                                kind="pinned_measurement_input")
                pinned[safe] = {"sha256": actual, "bytes": ref["bytes"],
                                "support_relative_path": ref["support_relative_path"]}
            input_sets[set_id] = {
                "original_directory": config.parent.resolve().as_posix(),
                "generation_sha256": generation_ref["sha256"],
                "generation_support_path": generation_ref["support_relative_path"],
                "pinned_inputs": pinned,
                "pinned_file_count": len(pinned),
                "request_count_per_configuration": generation.get("request_count_per_configuration"),
            }
        if case.get("generation_sha256") != input_sets[set_id]["generation_sha256"]:
            fail(f"Controller generation hash differs from generation.json for {case['name']}")
        actual_config_sha = sha256_file(config)
        pinned_record = case.get("pinned_inputs_sha256", {})
        if pinned_record.get(config.name) != actual_config_sha:
            fail(f"Controller config SHA differs from its pinned input set: {case['name']}")
        scenario = next((row for row in generation.get("scenarios", []) if row.get("ini") == config.name), None)
        if not scenario:
            fail(f"Generation manifest lacks scenario for {config.name}")
        rho = float(scenario.get("rho"))
        count = generation.get("request_count_per_configuration")
        if case.get("request_count_per_configuration") != count:
            fail(f"Controller request count differs from input generation for {case['name']}")
        controller_scenario = case.get("scenario")
        if not isinstance(controller_scenario, dict) or float(controller_scenario.get("rho")) != rho:
            fail(f"Controller rho differs from input generation for {case['name']}")
        inputs_by_case[case["name"]] = {"input_set": set_id, "config": config.name,
                                         "config_path": config.resolve().as_posix(),
                                         "config_sha256": actual_config_sha,
                                         "rho": rho, "request_count_per_configuration": count}
    return input_sets, inputs_by_case


def validate_comparison(path: Path, expected_binaries: dict[str, dict], input_case: dict) -> dict:
    report = read_json(path)
    if report.get("schema_version") != 1:
        fail(f"Unexpected comparison schema: {path}")
    if Path(report.get("config", "")).resolve() != Path(input_case["config_path"]).resolve():
        fail(f"Comparison config differs from pinned case input: {path}")
    binaries = report.get("binaries", {})
    if list(binaries) != list(VARIANTS):
        fail(f"Comparison binary names/order mismatch: {path}")
    for name in VARIANTS:
        record = binaries[name]
        expected = expected_binaries[name]
        if Path(record.get("path", "")).resolve() != Path(expected["path"]).resolve():
            fail(f"Comparison binary path mismatch for {name}: {path}")
        if record.get("sha256") != expected["sha256"]:
            fail(f"Comparison binary hash mismatch for {name}: {path}")
        verified_binary(Path(record["path"]), record["sha256"])

    attempts = report.get("attempts", [])
    if len(attempts) != 24:
        fail(f"Expected 24 attempts per case, found {len(attempts)}: {path}")
    by_round: dict[int, list[dict]] = {repeat: [] for repeat in range(4)}
    per_variant = {name: [] for name in VARIANTS}
    diagnostic_hashes = []
    output_manifests = 0
    for attempt in attempts:
        name = attempt.get("binary")
        repeat = attempt.get("repeat")
        if name not in per_variant or repeat not in by_round:
            fail(f"Unexpected attempt identity: {path}")
        by_round[repeat].append(attempt)
        per_variant[name].append(attempt)
        expected_kind = "warmup" if repeat == 0 else "measurement"
        if attempt.get("kind") != expected_kind or attempt.get("completed") is not True or attempt.get("returncode") != 0:
            fail(f"Failed or mislabeled attempt for {name}, repeat {repeat}: {path}")
        if attempt.get("manifest_verified") is not True or attempt.get("guard_reason") is not None:
            fail(f"Manifest/guard validation failed for {name}, repeat {repeat}: {path}")
        if attempt.get("large_outputs_removed_after_recording") is not True:
            fail(f"Timed output retention state missing for {name}, repeat {repeat}: {path}")
        output_dir = Path(attempt.get("evidence_directory", "")) / "output"
        if output_dir.exists():
            fail(f"Timed comparison output still exists: {output_dir}")
        files = attempt.get("files", [])
        if len(files) != 4 or {row.get("path") for row in files} != EXPECTED_FILES:
            fail(f"Attempt manifest is missing one of four output files: {path}")
        for row in files:
            declared = row.get("declared", {})
            declared_name = declared.get("name", declared.get("path"))
            declared_size = declared.get("bytes", declared.get("size_bytes"))
            if row.get("path") != declared_name or str(row.get("bytes")) != str(declared_size):
                fail(f"Attempt output path/size differs from manifest: {path}")
            digest = row.get("sha256")
            if not digest or digest != declared.get("sha256"):
                fail(f"Attempt output SHA differs from manifest: {path}")
            output_manifests += 1
            if row.get("path") == "diagnostics.jsonl":
                diagnostic_hashes.append(digest)
        wall = attempt.get("measurements", {}).get("wall_seconds")
        rss = attempt.get("max_rss_bytes")
        if isinstance(wall, bool) or not isinstance(wall, (int, float)) or not math.isfinite(wall) or wall < 0:
            fail(f"Invalid wall measurement: {path}")
        if isinstance(rss, bool) or not isinstance(rss, int) or rss < 0:
            fail(f"Invalid RSS measurement: {path}")
    for repeat in range(4):
        expected_order = list(VARIANTS) if repeat % 2 == 0 else list(reversed(VARIANTS))
        if [row.get("binary") for row in by_round[repeat]] != expected_order:
            fail(f"Forward/reverse serial order mismatch for repeat {repeat}: {path}")
    for name, rows in per_variant.items():
        if len(rows) != 4 or sum(row.get("kind") == "warmup" for row in rows) != 1:
            fail(f"Expected 1 warmup and 3 measurements for {name}: {path}")
    deterministic = [attempt.get("deterministic") for attempt in attempts]
    if not deterministic[0] or any(value != deterministic[0] for value in deterministic):
        fail(f"Normalized output hashes differ across attempts: {path}")
    if len(diagnostic_hashes) != 24 or len(set(diagnostic_hashes)) != 1:
        fail(f"diagnostics.jsonl hashes differ across attempts: {path}")
    if report.get("normalized_outputs_equal") is not True:
        fail(f"Comparison did not confirm normalized output equality: {path}")

    recomputed = {}
    summaries = report.get("summaries", {})
    for name, rows in per_variant.items():
        measured = [row for row in rows if row.get("kind") == "measurement"]
        walls = [row["measurements"]["wall_seconds"] for row in measured]
        rss_values = [row["max_rss_bytes"] for row in measured]
        median = statistics.median(walls)
        maximum_rss = max(rss_values)
        recorded = summaries.get(name, {})
        if recorded.get("completed") is not True or recorded.get("median_wall_seconds") != median:
            fail(f"Recorded median differs from raw measurements for {name}: {path}")
        if recorded.get("maximum_rss_bytes") != maximum_rss:
            fail(f"Recorded maximum RSS differs from raw measurements for {name}: {path}")
        recomputed[name] = {"median_wall_seconds": median,
                            "wall_seconds_min": min(walls), "wall_seconds_max": max(walls),
                            "wall_seconds_range": max(walls) - min(walls),
                            "measurement_wall_seconds": walls,
                            "maximum_measured_rss_bytes": maximum_rss,
                            "measurement_rss_bytes": rss_values}
    base = recomputed["baseline"]
    for name in VARIANTS[1:]:
        candidate = recomputed[name]
        candidate["median_delta_seconds_vs_baseline"] = candidate["median_wall_seconds"] - base["median_wall_seconds"]
        candidate["median_reduction_percent_vs_baseline"] = (
            (base["median_wall_seconds"] - candidate["median_wall_seconds"])
            / base["median_wall_seconds"] * 100 if base["median_wall_seconds"] else None)
        candidate["maximum_rss_delta_bytes_vs_baseline"] = (
            candidate["maximum_measured_rss_bytes"] - base["maximum_measured_rss_bytes"])
    if input_case["request_count_per_configuration"] not in (3200, 16000):
        fail(f"Unexpected workload size for {path}")
    return {"attempt_count": len(attempts), "attempts_complete": True,
            "output_manifest_files_verified": output_manifests,
            "normalized_hashes_equal": True, "normalized_hashes": deterministic[0],
            "diagnostics_sha256_equal": True, "diagnostics_sha256": diagnostic_hashes[0],
            "summaries_recomputed_from_raw_attempts": recomputed,
            "comparison_report_path": path.resolve().as_posix(),
            "comparison_report_sha256": sha256_file(path)}


def copy_measurement_tree(run_dir: Path, stage: Path, inventory: list[dict]) -> list[dict]:
    if not run_dir.is_dir():
        fail(f"Measurement run directory is missing: {run_dir}")
    exclusions = []
    for path in sorted(run_dir.rglob("*")):
        relative = path.relative_to(run_dir)
        parts = {part.lower() for part in relative.parts}
        if any(part in {".git", "output", "target", "bin", ".venv"} for part in parts):
            continue
        if path.is_dir():
            continue
        if path.is_symlink():
            exclusions.append({"original_local_path": path.as_posix(), "reason": "symlink_not_copied"})
            continue
        if is_sensitive_path(relative):
            exclusions.append({"original_local_path": path.as_posix(), "reason": "credential_or_secret_name"})
            continue
        if is_binary_file(path) or os.access(path, os.X_OK):
            exclusions.append({"original_local_path": path.as_posix(), "reason": "binary_or_executable_not_copied"})
            continue
        size = path.stat().st_size
        if size > MAX_MEASUREMENT_FILE_BYTES:
            exclusions.append({"original_local_path": path.as_posix(), "bytes": size,
                               "reason": "large_measurement_file_not_copied"})
            continue
        destination = f"measurements/{run_dir.name}/{relative.as_posix()}"
        copy_file(path, stage, destination, inventory, kind="measurement_run_file")
    return exclusions


def collect_worker_artifacts(stage: Path, inventory: list[dict]) -> None:
    required = ("measurement-plan.json", "baseline.json", "experimental-git-identity.json",
                "gate-candidates.py", "measure-candidates.py", "add-tests-notes.py",
                "implement3.py", "implement5.py")
    for name in required:
        source = ROOT / name
        destination = f"provenance/{name}" if name.endswith(".json") else f"scripts/{name}"
        copy_file(require_regular(source), stage, destination, inventory,
                  kind="experiment_provenance_or_script")
    live_source_check = ROOT / "live-production-source-check-before.json"
    if live_source_check.is_file() and not live_source_check.is_symlink():
        copy_file(live_source_check, stage, "provenance/live-production-source-check-before.json", inventory,
                  kind="production_source_precheck")
    after_source_check = ROOT / "live-production-source-check-after.json"
    copy_file(require_regular(after_source_check), stage,
              "provenance/live-production-source-check-after.json", inventory,
              kind="production_source_postcheck")
    if Path(__file__).is_file():
        copy_file(Path(__file__), stage, "scripts/collect-evidence.py", inventory,
                  kind="evidence_collector_source")
    for relative in ("scripts/performance/measure_can_export.py",
                     "scripts/performance/measure_can_million.py",
                     "scripts/performance/generate_can_million.py"):
        source = REPO / relative
        copy_file(require_regular(source), stage, f"scripts/{Path(relative).name}", inventory,
                  kind="measurement_or_generation_script")
    for number in range(1, 6):
        note = ROOT / f"candidate{number}-design.md"
        # Keep these outside docs/**/*.md so strict traceability does not treat
        # pre-gate worker notes as project documentation.
        copy_file(require_regular(note), stage, f"worker-notes/candidate{number}-design.txt", inventory,
                  kind="historical_pre_gate_worker_note")
    for filename in ("add-tests-notes.py", "implement3.py", "implement5.py"):
        source = ROOT / filename
        copy_file(require_regular(source), stage, f"worker-notes/{filename}.txt", inventory,
                  kind="historical_pre_gate_worker_artifact")
    marker = (
        "Historical pre-gate worker artifacts. These notes and scripts record design/edit work; "
        "they are not current gate or measurement evidence. Current acceptance evidence is in the "
        "successful latest gate reports and completed comparison reports.\n"
    ).encode("utf-8")
    write_generated(marker, stage, "worker-notes/README.txt", inventory,
                    ["collector-authored classification"], "historical_artifact_scope_note")


def collect() -> dict:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--controller", type=Path, required=True,
                        help="Path to completed measure-candidates.py controller.json")
    args = parser.parse_args()
    controller_path = require_regular(args.controller.resolve(strict=True))
    controller = read_json(controller_path)
    if controller.get("schema_version") != 1 or controller.get("status") != "complete":
        fail("The measurement controller must be schema 1 and complete")
    if controller.get("qualification") != "Scaled-input per-candidate comparison only; no full-million acceptance verdict.":
        fail("Controller qualification is missing the no-million-acceptance boundary")
    policy = controller.get("comparison_policy", {})
    if policy.get("full_million_acceptance_claim") is not False or policy.get("serial") is not True:
        fail("Controller comparison policy violates the accepted scope")
    if policy.get("warmups_per_binary") != 1 or policy.get("measurements_per_binary") != 3:
        fail("Controller must use one warmup and three measured runs per binary")
    if list(controller.get("binaries", {})) != list(VARIANTS):
        fail("Controller must contain baseline and candidate1 through candidate5 in order")
    if [case.get("name") for case in controller.get("cases", [])] != list(EXPECTED_CASES):
        fail("Controller case list differs from the three required cases")
    if [entry.get("id") for entry in controller.get("comparisons", [])] != list(EXPECTED_CASES):
        fail("Controller comparisons differ from the three required cases")
    if controller.get("completed_comparison_count") != 3 or controller.get("attempt_count") != 72:
        fail("Expected three completed comparisons and 72 completed attempts")
    if any(entry.get("status") != "complete" for entry in controller["comparisons"]):
        fail("Every controller comparison must be complete")

    manifest_path = ROOT / "baseline.json"
    baseline_manifest = read_json(manifest_path)
    baseline_root = Path(baseline_manifest["source_directory"]).resolve(strict=True)
    baseline_map = source_snapshot(baseline_root)
    if baseline_manifest.get("source_sha256") != baseline_map:
        fail("Baseline source tree differs from baseline.json")
    if len(baseline_map) != EXPECTED_BASELINE_SOURCE_FILES:
        fail(f"Expected the complete {EXPECTED_BASELINE_SOURCE_FILES}-file baseline source inventory")
    if baseline_manifest.get("binary", {}).get("sha256") != controller["binaries"]["baseline"]["sha256"]:
        fail("Controller baseline binary differs from baseline.json")
    baseline_binary = verified_binary(Path(baseline_manifest["binary"]["path"]),
                                      baseline_manifest["binary"]["sha256"])

    gate_data = {}
    gate_summaries = {}
    for name in VARIANTS:
        source_dir = baseline_root if name == "baseline" else ROOT / name
        expected = baseline_map if name == "baseline" else None
        association, details = validate_gate(name, source_dir, expected)
        pinned = controller.get("gate_associations", {}).get(name, {})
        if pinned.get("latest_sha256") != association["latest_report_sha256"]:
            fail(f"Controller/latest gate association changed for {name}")
        if pinned.get("source_sha256") != association["source_sha256"]:
            fail(f"Controller source snapshot differs from latest gate for {name}")
        if pinned.get("binary", {}).get("sha256") != association["binary"]["sha256"]:
            fail(f"Controller binary differs from latest gate for {name}")
        if controller["binaries"][name].get("sha256") != association["binary"]["sha256"]:
            fail(f"Controller binary identity differs from latest gate for {name}")
        if Path(controller["binaries"][name].get("path", "")).resolve() != Path(association["binary"]["path"]).resolve():
            fail(f"Controller binary path differs from latest gate for {name}")
        controller_snapshot = controller.get("source_snapshots", {}).get(name, {})
        if controller_snapshot.get("files") != association["source_sha256"]:
            fail(f"Controller source snapshot differs from latest gate source map for {name}")
        if Path(controller_snapshot.get("path", "")).resolve() != source_dir.resolve():
            fail(f"Controller source directory differs from latest gate for {name}")
        gate_data[name] = details
        gate_summaries[name] = association
    if gate_summaries["baseline"]["binary"]["sha256"] != baseline_binary["sha256"]:
        fail("Baseline gate binary differs from baseline manifest binary")

    source_deltas = {}
    source_changes = {}
    for name in VARIANTS[1:]:
        current_map = gate_summaries[name]["source_sha256"]
        added = sorted(set(current_map) - set(baseline_map))
        removed = sorted(set(baseline_map) - set(current_map))
        changed = sorted(path for path in set(current_map) & set(baseline_map)
                         if current_map[path] != baseline_map[path])
        recorded = controller.get("source_deltas_from_baseline", {}).get(name, {})
        if recorded.get("added") != added or recorded.get("removed") != removed or recorded.get("changed") != changed:
            fail(f"Controller source delta differs from gate source maps for {name}")
        source_deltas[name] = {"added": added, "removed": removed, "changed": changed}
        source_changes[name] = []
        for relative in sorted(set(added) | set(removed) | set(changed)):
            source_changes[name].append((relative, baseline_root if relative in baseline_map else None,
                                         ROOT / name if relative in current_map else None))

    cases = controller["cases"]
    run_dir = controller_path.parent
    stem = DOC_ID
    output_json = RESULTS / f"{stem}.json"
    output_support = RESULTS / stem
    if output_json.exists() or output_support.exists():
        fail(f"Evidence output already exists: {output_json} or {output_support}")
    RESULTS.mkdir(parents=True, exist_ok=True)
    stage = RESULTS / f".{stem}.collecting-{os.getpid()}"
    stage.mkdir(exist_ok=False)
    temporary_json = RESULTS / f".{stem}.json.tmp-{os.getpid()}"
    published_support = False
    try:
        inventory: list[dict] = []
        collect_worker_artifacts(stage, inventory)
        baseline_copy_entries = []
        for relative, expected_sha in sorted(baseline_map.items()):
            copied = copy_file(baseline_root / relative, stage,
                               f"sources/baseline/{relative}", inventory,
                               kind="baseline_source_snapshot")
            if copied["sha256"] != expected_sha:
                fail(f"Baseline source copy hash mismatch: {relative}")
            baseline_copy_entries.append(relative)
        baseline_archive = write_baseline_source_archive(baseline_root, stage, inventory)
        for name in VARIANTS[1:]:
            current_map = gate_summaries[name]["source_sha256"]
            for relative, _base, _candidate in source_changes[name]:
                copy_source_patch(name, relative, baseline_root, ROOT / name,
                                  baseline_map, current_map, stage, inventory)

        # Copy the current successful gate reports and the exact logs they reference.
        for name in VARIANTS:
            details = gate_data[name]
            copy_file(details["report_path"], stage, f"gates/{name}/latest.json", inventory,
                      kind="successful_latest_gate_report")
            for log in details["logs"]:
                copy_file(log["path"], stage,
                          f"gates/{name}/latest/{log['index']}-{log['action']}.log", inventory,
                          kind="successful_latest_gate_log")
        historical = []
        for name in VARIANTS:
            historical.extend(historical_gates(name, gate_data[name]["report_path"], stage, inventory))

        input_sets, inputs_by_case = collect_inputs(cases, stage, inventory)
        if {inputs_by_case[name]["request_count_per_configuration"] for name in EXPECTED_CASES} != {3200, 16000}:
            fail("Expected pinned 3,200- and 16,000-request input sets")
        expected_rho = {"n16000-rho030": 0.30, "n3200-rho090": 0.90, "n3200-rho120": 1.20}
        for name, rho in expected_rho.items():
            if inputs_by_case[name]["rho"] != rho:
                fail(f"Pinned input rho differs for {name}")

        comparison_results = []
        total_manifests = 0
        for entry in controller["comparisons"]:
            case_name = entry["id"]
            if entry.get("case") != case_name:
                fail(f"Controller comparison case mismatch: {case_name}")
            case_record = next(case for case in cases if case["name"] == case_name)
            if Path(entry.get("config", "")).resolve() != Path(inputs_by_case[case_name]["config_path"]).resolve():
                fail(f"Controller comparison config differs from pinned case: {case_name}")
            if entry.get("input_generation_sha256") != case_record.get("generation_sha256"):
                fail(f"Controller comparison generation hash differs from pinned case: {case_name}")
            if entry.get("source_snapshots") != {
                name: controller["source_snapshots"][name]["files"] for name in VARIANTS
            }:
                fail(f"Controller comparison source map differs from gate association: {case_name}")
            for name in VARIANTS:
                entry_binary = entry.get("binary_hashes", {}).get(name, {})
                if entry_binary.get("sha256") != controller["binaries"][name].get("sha256"):
                    fail(f"Controller comparison binary hash differs for {name} in {case_name}")
                if Path(entry_binary.get("path", "")).resolve() != Path(controller["binaries"][name]["path"]).resolve():
                    fail(f"Controller comparison binary path differs for {name} in {case_name}")
            comparison_path = Path(entry.get("helper_report_path", ""))
            if not comparison_path.is_absolute():
                comparison_path = run_dir / comparison_path
            expected_comparison_dir = Path(entry.get("comparison_directory", "")).resolve()
            if comparison_path.resolve().parent != expected_comparison_dir:
                fail(f"Controller comparison report path differs from its directory: {case_name}")
            comparison = validate_comparison(comparison_path, controller["binaries"], inputs_by_case[case_name])
            total_manifests += comparison["output_manifest_files_verified"]
            controller_validation = entry.get("validation", {})
            if controller_validation.get("normalized_hashes") != comparison["normalized_hashes"]:
                fail(f"Controller normalized hashes differ for {case_name}")
            if controller_validation.get("diagnostics_sha256") != comparison["diagnostics_sha256"]:
                fail(f"Controller diagnostics hash differs for {case_name}")
            if controller_validation.get("diagnostics_sha256_equal") is not True:
                fail(f"Controller did not confirm diagnostics hash equality for {case_name}")
            controller_summaries = controller_validation.get("summaries_recomputed_from_attempts", {})
            for name in VARIANTS:
                expected_summary = comparison["summaries_recomputed_from_raw_attempts"][name]
                recorded_summary = controller_summaries.get(name, {})
                for key in ("median_wall_seconds", "measurement_wall_seconds", "maximum_measured_rss_bytes"):
                    if recorded_summary.get(key) != expected_summary[key]:
                        fail(f"Controller raw summary differs for {name} in {case_name}: {key}")
            controller_deltas = controller_validation.get("candidate_deltas_vs_shared_baseline", {})
            for name in VARIANTS[1:]:
                expected_summary = comparison["summaries_recomputed_from_raw_attempts"][name]
                recorded_delta = controller_deltas.get(name, {})
                for recorded_key, expected_key in (
                    ("median_wall_delta_seconds_vs_shared_baseline", "median_delta_seconds_vs_baseline"),
                    ("median_wall_reduction_percent_vs_shared_baseline", "median_reduction_percent_vs_baseline"),
                    ("maximum_measured_rss_delta_bytes_vs_shared_baseline", "maximum_rss_delta_bytes_vs_baseline"),
                ):
                    if recorded_delta.get(recorded_key) != expected_summary[expected_key]:
                        fail(f"Controller delta differs for {name} in {case_name}: {recorded_key}")
            comparison_results.append({"case": case_name, **inputs_by_case[case_name], **comparison})

        if len(comparison_results) != 3 or total_manifests != 72 * 4:
            fail("Expected 3 comparisons and 288 manifest file verifications")
        measurement_exclusions = copy_measurement_tree(run_dir, stage, inventory)

        # Every copied source and every report/log has a single explicit inventory row.
        inv_by_path = {row["support_relative_path"]: row for row in inventory}
        if len(inv_by_path) != len(inventory):
            fail("Support inventory contains duplicate destination paths")
        for row in inventory:
            rel = stage / safe_relative(row["support_relative_path"])
            if not rel.is_file() or rel.stat().st_size != row["bytes"] or sha256_file(rel) != row["sha256"]:
                fail(f"Support inventory verification failed: {row['support_relative_path']}")

        report = {
            "schema_version": 1,
            "document_id": DOC_ID,
            "generated_at_utc": datetime.now(timezone.utc).isoformat(),
            "qualification": "Scaled-input five-candidate comparison only; no full-million acceptance verdict.",
            "controller": {"original_local_path": controller_path.resolve().as_posix(),
                           "sha256": sha256_file(controller_path),
                           "status": controller["status"],
                           "comparison_count": len(comparison_results),
                           "attempt_count": 72,
                           "warmup_attempts": 18,
                           "measured_attempts": 54,
                           "serial_order": "forward then reverse across four rounds per case",
                           "design": "one shared baseline with five candidates; candidate deltas are per-binary medians, not paired A/B results"},
            "inputs": {"sets": input_sets, "by_case": inputs_by_case},
            "results": comparison_results,
            "validation": {
                "expected_cases": list(EXPECTED_CASES), "validated_cases": len(comparison_results),
                "variants_including_baseline": len(VARIANTS), "validated_variants": list(VARIANTS),
                "expected_attempts": 72, "completed_attempts": 72,
                "attempts_per_case": 24, "attempts_per_variant": 12,
                "warmups_per_variant": 3, "measurements_per_variant": 9,
                "output_manifest_files_verified": total_manifests,
                "normalized_output_equivalence_cases": sum(row["normalized_hashes_equal"] for row in comparison_results),
                "diagnostics_hash_equality_cases": sum(row["diagnostics_sha256_equal"] for row in comparison_results),
                "timed_comparison_output_directories_absent": True,
                "comparison_outputs_deleted_after_manifest_and_hash_recording": True,
                "full_million_acceptance_claim": False,
            },
            "gates": {"successful_latest_reports": gate_summaries,
                      "all_latest_gates_passed": True,
                      "source_delta_from_baseline": source_deltas,
                      "historical_attempts": historical,
                      "historical_context": [
                          {"candidate": "candidate1", "context": "The first gate attempt encountered fixture symlink rejection; later source fixtures were copied as regular files.", "classification": "historical context; not evidence that every prior failure was environmental"},
                          {"candidate": "baseline", "context": "The first provenance check observed git_commit=unknown; a local non-secret Git identity was initialized for the later fresh gate.", "classification": "historical context; later gate is the accepted pass evidence"},
                          {"candidate": "candidate3", "context": "An earlier clippy gate failed on an unused old CSV archive wrapper and needless borrows; this was an implementation failure. The source was fixed and the subsequent gate report is recorded separately.", "classification": "historical genuine implementation failure; excluded from latest-pass evidence"},
                          {"candidate": "candidate4", "context": "An earlier fmt --check failed because of module declaration ordering (`csv` before `contribution_sort`); cargo fmt was then run on candidate4 and gates were restarted, with no semantic source change reported.", "classification": "historical formatting failure; excluded from latest-pass evidence"},
                      ]},
            "source_evidence": {"baseline_source_file_count": len(baseline_map),
                                "baseline_source_support_prefix": "sources/baseline/",
                                "baseline_source_files_copied": len(baseline_copy_entries),
                                "baseline_reproduction_archive": baseline_archive,
                                "candidate_changed_added_source_files_and_patches": source_deltas,
                                "source_scope": "gate inventory: all non-symlink files under crates/ plus Cargo.toml, Cargo.lock, rust-toolchain.toml"},
            "binaries": {name: gate_summaries[name]["binary"] for name in VARIANTS},
            "measurement_file_exclusions": measurement_exclusions,
            "support_directory": stem,
            "inventory_base_directory": f"docs/verification/results/{stem}/",
            "support_files": inventory,
        }
        report = rebase_paths(report, stage)
        if contains_collecting(report):
            fail("Final report contains a staging path")
        temporary_json.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        stage.rename(output_support)
        published_support = True
        temporary_json.replace(output_json)
        return report
    except BaseException:
        if temporary_json.exists():
            temporary_json.unlink()
        if stage.exists():
            shutil.rmtree(stage)
        if published_support and output_support.exists():
            shutil.rmtree(output_support)
        raise


def rebase_paths(value, stage: Path):
    prefix = stage.as_posix() + "/"
    if isinstance(value, dict):
        return {key: rebase_paths(item, stage) for key, item in value.items()}
    if isinstance(value, list):
        return [rebase_paths(item, stage) for item in value]
    if isinstance(value, str) and value.startswith(prefix):
        return value[len(prefix):]
    return value


def contains_collecting(value) -> bool:
    if isinstance(value, dict):
        return any(contains_collecting(item) for item in value.values())
    if isinstance(value, list):
        return any(contains_collecting(item) for item in value)
    return isinstance(value, str) and (".collecting-" in value or ".tmp-" in value)


def main() -> int:
    try:
        report = collect()
    except (RuntimeError, OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"evidence collector: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"status": "complete", "report": (RESULTS / f"{DOC_ID}.json").as_posix(),
                      "support_directory": (RESULTS / DOC_ID).as_posix(),
                      "cases": report["validation"]["validated_cases"],
                      "attempts": report["validation"]["completed_attempts"],
                      "support_files": len(report["support_files"])}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
