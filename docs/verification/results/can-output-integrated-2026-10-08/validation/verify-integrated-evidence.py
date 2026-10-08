#!/usr/bin/env python3
"""Independently verify the archived baseline-versus-integrated evidence.

This verifier reads the completed evidence report and its support tree. It does
not invoke the simulator, build, test, or benchmark binaries. Patch replay uses a
fresh temporary copy of the archived baseline source tree.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import statistics
import subprocess
import sys
import tempfile


EXPECTED_CASES = ("n16000-rho030", "n3200-rho090", "n3200-rho120")
EXPECTED_BINARIES = ("baseline", "integrated")
EXPECTED_COMMANDS = ("fmt", "clippy", "test", "build")
EXPECTED_OUTPUTS = {"diagnostics.jsonl", "events.csv", "results.json", "summary.csv"}
EXPECTED_SOURCE_CHANGES = 7
EXPECTED_ATTEMPTS = 24
EXPECTED_MANIFEST_ENTRIES = 96
EXPECTED_SUPPORT_FILES = 389
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
TEST_RESULT_RE = re.compile(
    r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;"
)


class VerificationError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerificationError(message)


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
    if path.is_symlink():
        raise VerificationError(f"Output path is a symlink: {path}")
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n",
                         encoding="utf-8")
    temporary.replace(path)


def safe_relative(raw: str, label: str) -> PurePosixPath:
    require(isinstance(raw, str) and raw != "", f"{label} is empty or not a string")
    require("\\" not in raw and "\x00" not in raw, f"Unsafe separator in {label}: {raw!r}")
    path = PurePosixPath(raw)
    require(not path.is_absolute(), f"Absolute path is forbidden in {label}: {raw}")
    require(path.as_posix() == raw, f"Non-canonical path in {label}: {raw}")
    require(all(part not in ("", ".", "..") for part in path.parts),
            f"Traversal or empty component in {label}: {raw}")
    require(not re.match(r"^[A-Za-z]:", raw), f"Drive path is forbidden in {label}: {raw}")
    return path


def ensure_no_symlink_components(root: Path, rel: PurePosixPath) -> Path:
    require(not root.is_symlink(), f"Support root is a symlink: {root}")
    current = root
    for part in rel.parts:
        current = current / part
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError as error:
            raise VerificationError(f"Missing support path: {current}") from error
        require(not current.is_symlink(), f"Symlink is forbidden in support tree: {current}")
        if current != root / rel:
            require(current.is_dir(), f"Non-directory in support path: {current}")
        else:
            require(current.is_file(), f"Support member is not a regular file: {current}")
            require(os.path.isfile(current), f"Support member is not a regular file: {current}")
    resolved = current.resolve(strict=True)
    require(resolved == (root / rel).resolve(strict=True),
            f"Support member resolves outside its declared path: {rel}")
    require(root.resolve(strict=True) == resolved or root.resolve(strict=True) in resolved.parents,
            f"Support member resolves outside support root: {rel}")
    return resolved


def discover_support_files(root: Path) -> set[str]:
    require(root.is_dir() and not root.is_symlink(), f"Support directory is missing or unsafe: {root}")
    found = set()
    for current, directories, filenames in os.walk(root, topdown=True, followlinks=False):
        current_path = Path(current)
        for name in list(directories):
            path = current_path / name
            require(not path.is_symlink(), f"Symlink directory is forbidden: {path}")
            require(path.is_dir(), f"Non-directory encountered in support tree: {path}")
        for name in filenames:
            path = current_path / name
            require(not path.is_symlink(), f"Symlink file is forbidden: {path}")
            require(path.is_file(), f"Non-regular support member is forbidden: {path}")
            found.add(path.relative_to(root).as_posix())
    return found


def resolve_support_root(report_path: Path, report: dict) -> Path:
    support_directory = report.get("support_directory")
    inventory_base = report.get("inventory_base_directory")
    require(isinstance(inventory_base, str) and inventory_base,
            "Main report is missing inventory_base_directory")
    require(isinstance(support_directory, str) and support_directory,
            "Main report is missing support_directory")

    inventory_path = Path(inventory_base)
    if inventory_path.is_absolute():
        candidate = inventory_path
    elif len(inventory_path.parts) > 1:
        candidate = report_path.parent.parent.parent.parent / inventory_path
    else:
        candidate = report_path.parent / inventory_path
    if not candidate.exists():
        candidate = report_path.parent / support_directory
    require(not candidate.is_symlink(), f"Support directory is a symlink: {candidate}")
    candidate = candidate.resolve(strict=True)
    expected_name = Path(support_directory).name
    require(candidate.name == expected_name,
            "inventory_base_directory and support_directory identify different roots")
    require(not candidate.is_symlink(), f"Support directory is a symlink: {candidate}")
    return candidate


def verify_support_inventory(report: dict, support_root: Path) -> dict:
    rows = report.get("support_files")
    require(isinstance(rows, list), "Main report support_files must be an array")
    require(len(rows) == EXPECTED_SUPPORT_FILES,
            f"Main report support inventory has {len(rows)} entries; expected {EXPECTED_SUPPORT_FILES}")
    listed: dict[str, dict] = {}
    errors = []
    for row in rows:
        require(isinstance(row, dict), "support_files contains a non-object row")
        rel = safe_relative(row.get("support_relative_path"), "support_relative_path").as_posix()
        require(rel not in listed, f"Duplicate support inventory path: {rel}")
        path = ensure_no_symlink_components(support_root, PurePosixPath(rel))
        expected_bytes = row.get("bytes")
        require(isinstance(expected_bytes, int) and not isinstance(expected_bytes, bool),
                f"Invalid byte count in support inventory: {rel}")
        actual_bytes = path.stat().st_size
        actual_hash = sha256_file(path)
        if actual_bytes != expected_bytes:
            errors.append(f"size mismatch: {rel}: expected {expected_bytes}, got {actual_bytes}")
        expected_hash = row.get("sha256")
        if not isinstance(expected_hash, str) or not SHA256_RE.fullmatch(expected_hash):
            errors.append(f"invalid SHA-256 in support inventory: {rel}")
        elif actual_hash != expected_hash:
            errors.append(f"SHA-256 mismatch: {rel}")
        listed[rel] = row
    actual = discover_support_files(support_root)
    missing = sorted(actual - set(listed))
    absent = sorted(set(listed) - actual)
    require(not missing, f"Uninventoried support files: {missing[:10]}")
    require(not absent, f"Inventory entries missing from support tree: {absent[:10]}")
    require(not errors, "; ".join(errors[:20]))
    by_original: dict[str, dict] = {}
    for rel, row in listed.items():
        original = row.get("original_local_path")
        if isinstance(original, str) and original:
            by_original[os.path.normpath(original)] = row
    return {
        "expected_entries": len(rows), "unique_entries": len(listed),
        "verified_files": len(actual), "all_verified": True,
        "uninventoried_support_files": missing, "missing_inventory_files": absent,
        "support_by_relative_path": listed, "support_by_original_path": by_original,
    }


def source_inventory(source_root: Path) -> dict[str, str]:
    require(source_root.is_dir() and not source_root.is_symlink(),
            f"Source snapshot is missing or unsafe: {source_root}")
    found: dict[str, str] = {}
    for current, directories, filenames in os.walk(source_root, topdown=True, followlinks=False):
        current_path = Path(current)
        for name in list(directories):
            path = current_path / name
            require(not path.is_symlink(), f"Source snapshot contains a symlink: {path}")
            require(path.is_dir(), f"Source snapshot contains a non-directory: {path}")
        for name in filenames:
            path = current_path / name
            require(not path.is_symlink(), f"Source snapshot contains a symlink: {path}")
            require(path.is_file(), f"Source snapshot contains a non-regular file: {path}")
            rel = path.relative_to(source_root).as_posix()
            safe_relative(rel, "source path")
            found[rel] = sha256_file(path)
    return dict(sorted(found.items()))


def validate_source_map(value, label: str, expected_count: int) -> dict[str, str]:
    require(isinstance(value, dict), f"{label}.source_sha256 must be an object")
    source_map = {}
    for raw, digest in value.items():
        rel = safe_relative(raw, f"{label} source path").as_posix()
        require(isinstance(digest, str) and SHA256_RE.fullmatch(digest),
                f"Invalid source SHA-256 for {label}:{rel}")
        source_map[rel] = digest
    require(len(source_map) == expected_count,
            f"{label} source map has {len(source_map)} files; expected {expected_count}")
    return dict(sorted(source_map.items()))


def normalize_change_path(value) -> str:
    if isinstance(value, str):
        raw = value
    elif isinstance(value, dict):
        raw = next((value[key] for key in ("path", "relative_path", "source_path")
                    if isinstance(value.get(key), str)), None)
    else:
        raw = None
    return safe_relative(raw, "source_changes path").as_posix()


def verify_source_snapshots(report: dict, root: Path, inventory: dict) -> dict:
    baseline_map = validate_source_map(report.get("baseline", {}).get("source_sha256"),
                                       "baseline", 183)
    integrated_map = validate_source_map(report.get("integrated", {}).get("source_sha256"),
                                         "integrated", 184)
    baseline_dir = root / "source-before"
    copied_baseline = source_inventory(baseline_dir)
    require(copied_baseline == baseline_map,
            "Archived source-before snapshot differs from baseline.source_sha256")

    raw_changes = report.get("source_changes")
    require(isinstance(raw_changes, list), "Main report source_changes must be an array")
    change_paths = [normalize_change_path(value) for value in raw_changes]
    require(len(change_paths) == EXPECTED_SOURCE_CHANGES,
            f"source_changes has {len(change_paths)} paths; expected {EXPECTED_SOURCE_CHANGES}")
    require(len(set(change_paths)) == len(change_paths), "source_changes contains duplicate paths")
    changed = set(change_paths)
    added = set(integrated_map) - set(baseline_map)
    removed = set(baseline_map) - set(integrated_map)
    modified = {path for path in set(baseline_map) & set(integrated_map)
                if baseline_map[path] != integrated_map[path]}
    actual_delta = added | removed | modified
    require(actual_delta == changed,
            f"Source map delta differs from the 7 authorized paths: {sorted(actual_delta ^ changed)}")
    require(not removed, f"Integrated source unexpectedly removes files: {sorted(removed)}")

    changed_root = root / "source-changes" / "files"
    changed_files = source_inventory(changed_root)
    require(set(changed_files) == changed,
            "source-changes/files does not contain exactly the seven changed source files")
    for path in changed:
        require(changed_files[path] == integrated_map[path],
                f"Archived changed file differs from integrated source map: {path}")
    for path in set(baseline_map) - changed:
        require(integrated_map.get(path) == baseline_map[path],
                f"Source outside the authorized seven paths changed: {path}")
    patch_info = report.get("source_patch")
    require(isinstance(patch_info, dict), "Main report source_patch must be an object")
    patch_rel = safe_relative(patch_info.get("support_relative_path"),
                              "source_patch.support_relative_path").as_posix()
    require(patch_rel == "source-changes/integrated.patch",
            "source_patch.support_relative_path is not source-changes/integrated.patch")
    patch_file = ensure_no_symlink_components(root, PurePosixPath(patch_rel))
    patch_record = inventory["support_by_relative_path"][patch_rel]
    for key in ("bytes", "sha256"):
        if patch_info.get(key) is not None:
            require(patch_info[key] == patch_record[key],
                    f"source_patch.{key} differs from support inventory")
    return {
        "baseline_files": len(baseline_map), "integrated_files": len(integrated_map),
        "authorized_change_count": len(changed), "added": sorted(added),
        "modified": sorted(modified), "removed": sorted(removed),
        "source_map_delta_exact": True, "unchanged_outside_authorized_paths": True,
        "changed_source_files_verified": len(changed_files),
        "patch_support_path": patch_rel, "patch_path": patch_file,
        "baseline_source_map": baseline_map, "integrated_source_map": integrated_map,
        "authorized_paths": sorted(changed),
    }


def replay_patch(source_result: dict, support_root: Path) -> dict:
    baseline_dir = support_root / "source-before"
    patch_path = source_result["patch_path"]
    allowed = set(source_result["authorized_paths"])
    with tempfile.TemporaryDirectory(prefix="dir-opt-integrated-replay-") as temporary:
        replay_root = Path(temporary) / "source"
        shutil.copytree(baseline_dir, replay_root, symlinks=False, copy_function=shutil.copy2)
        require(source_inventory(replay_root) == source_result["baseline_source_map"],
                "Fresh patch replay copy differs from the archived baseline")
        parsed = subprocess.run(["git", "apply", "--numstat", patch_path.as_posix()],
                                cwd=replay_root, capture_output=True, text=True, check=False)
        require(parsed.returncode == 0,
                f"git apply could not parse integrated.patch: {parsed.stderr[-2000:]}")
        patch_paths = []
        for line in parsed.stdout.splitlines():
            fields = line.split("\t", 2)
            require(len(fields) == 3, f"Unexpected git apply --numstat row: {line!r}")
            raw_path = fields[2]
            path = safe_relative(raw_path, "integrated.patch path").as_posix()
            patch_paths.append(path)
        require(len(patch_paths) == len(set(patch_paths)), "Patch touches a path more than once")
        require(set(patch_paths) == allowed,
                f"Patch paths differ from authorized changes: {sorted(set(patch_paths) ^ allowed)}")
        check = subprocess.run(["git", "apply", "--check", patch_path.as_posix()],
                               cwd=replay_root, capture_output=True, text=True, check=False)
        require(check.returncode == 0,
                f"Integrated patch does not apply cleanly: {check.stderr[-2000:]}")
        applied = subprocess.run(["git", "apply", patch_path.as_posix()],
                                 cwd=replay_root, capture_output=True, text=True, check=False)
        require(applied.returncode == 0,
                f"Integrated patch replay failed: {applied.stderr[-2000:]}")
        replayed = source_inventory(replay_root)
        require(replayed == source_result["integrated_source_map"],
                "Replayed source tree differs from integrated.source_sha256")
        return {
            "patch_executable": "git apply", "method": "--numstat allowlist, --check, then apply to a fresh temporary copy",
            "patch_paths": sorted(patch_paths), "patches_applied": True,
            "source_map_exact_match": True,
            "replayed_source_file_count": len(replayed),
        }


def find_support_by_original(inventory: dict, original_path: str) -> dict | None:
    return inventory["support_by_original_path"].get(os.path.normpath(original_path))


def support_member_for_original(inventory: dict, support_root: Path,
                                original_path: str, fallback_rel: str | None = None) -> tuple[str, Path]:
    row = find_support_by_original(inventory, original_path)
    if row is not None:
        rel = safe_relative(row["support_relative_path"], "support path").as_posix()
    elif fallback_rel is not None:
        rel = safe_relative(fallback_rel, "fallback support path").as_posix()
        row = inventory["support_by_relative_path"].get(rel)
    else:
        row = None
        rel = ""
    require(row is not None, f"No archived support file for original evidence path: {original_path}")
    path = ensure_no_symlink_components(support_root, PurePosixPath(rel))
    return rel, path


def locate_comparison(case_name: str, inventory: dict, support_root: Path) -> tuple[str, Path]:
    matches = []
    for rel in inventory["support_by_relative_path"]:
        parts = PurePosixPath(rel).parts
        if (len(parts) >= 4 and parts[-4:] == ("cases", case_name, "helper-output", "comparison.json")):
            matches.append(rel)
        elif len(parts) >= 5 and parts[-5:] == ("measurements", case_name, "helper-output", "comparison.json", ""):
            matches.append(rel)
    if not matches:
        matches = [rel for rel in inventory["support_by_relative_path"]
                   if rel.endswith(f"/cases/{case_name}/helper-output/comparison.json")]
    require(len(matches) == 1,
            f"Expected one archived helper comparison.json for {case_name}, found {len(matches)}")
    rel = matches[0]
    return rel, ensure_no_symlink_components(support_root, PurePosixPath(rel))


def numbers_match(left, right, label: str) -> None:
    if isinstance(left, bool) or isinstance(right, bool):
        require(left == right, f"Boolean mismatch for {label}")
    elif isinstance(left, (int, float)) and isinstance(right, (int, float)):
        require(math.isclose(float(left), float(right), rel_tol=1e-12, abs_tol=1e-12),
                f"Numeric mismatch for {label}: {left!r} != {right!r}")
    else:
        require(left == right, f"Value mismatch for {label}: {left!r} != {right!r}")


def values_summary(values: list[int | float]) -> dict:
    return {"values": values, "median": statistics.median(values),
            "minimum": min(values), "maximum": max(values),
            "range": max(values) - min(values)}


def delta_summary(values: list[int | float]) -> dict:
    return {"by_measurement_repeat": values, "median": statistics.median(values),
            "minimum": min(values), "maximum": max(values),
            "range": max(values) - min(values)}


def compare_nested(expected, actual, label: str) -> None:
    if isinstance(expected, dict):
        require(isinstance(actual, dict), f"Expected object for {label}")
        require(set(expected) == set(actual), f"Object fields differ for {label}")
        for key in expected:
            compare_nested(expected[key], actual[key], f"{label}.{key}")
    elif isinstance(expected, list):
        require(isinstance(actual, list) and len(expected) == len(actual),
                f"Array size differs for {label}")
        for index, (left, right) in enumerate(zip(expected, actual)):
            compare_nested(left, right, f"{label}[{index}]")
    elif isinstance(expected, (int, float)) and not isinstance(expected, bool):
        numbers_match(expected, actual, label)
    else:
        require(expected == actual, f"Value mismatch for {label}: {expected!r} != {actual!r}")


def validate_output_manifest_rows(attempt: dict, label: str) -> dict:
    rows = attempt.get("files")
    require(isinstance(rows, list) and len(rows) == 4,
            f"{label} does not record exactly four output manifest rows")
    require({row.get("path") for row in rows} == EXPECTED_OUTPUTS,
            f"{label} output manifest paths differ from the standard four files")
    for row in rows:
        path = row.get("path")
        declared = row.get("declared")
        require(isinstance(declared, dict), f"{label} {path} has no manifest declaration")
        declared_path = declared.get("name", declared.get("path"))
        require(path == declared_path, f"{label} manifest name mismatch: {path}")
        declared_size = declared.get("bytes", declared.get("size_bytes"))
        require(str(row.get("bytes")) == str(declared_size),
                f"{label} manifest byte count mismatch: {path}")
        require(isinstance(row.get("bytes"), int) and row["bytes"] >= 0,
                f"{label} actual byte count is invalid: {path}")
        require(isinstance(row.get("sha256"), str) and SHA256_RE.fullmatch(row["sha256"]),
                f"{label} actual SHA-256 is invalid: {path}")
        require(row.get("sha256") == declared.get("sha256"),
                f"{label} output hash differs from manifest declaration: {path}")
    return {row["path"]: {"bytes": row["bytes"], "sha256": row["sha256"]} for row in rows}


def verify_attempt_evidence(case_name: str, comparison: dict, comparison_rel: str,
                            inventory: dict, support_root: Path,
                            binary_pins: dict) -> dict:
    require(comparison.get("schema_version") == 1,
            f"Unexpected helper comparison schema for {case_name}")
    binaries = comparison.get("binaries", {})
    require(list(binaries) == list(EXPECTED_BINARIES),
            f"Binary order differs in helper comparison for {case_name}")
    for name in EXPECTED_BINARIES:
        record = binaries.get(name, {})
        pin = binary_pins[name]
        require(Path(record.get("path", "")).resolve() == Path(pin["path"]).resolve(),
                f"Helper comparison binary path mismatch for {case_name}/{name}")
        require(record.get("sha256") == pin["sha256"],
                f"Helper comparison binary hash mismatch for {case_name}/{name}")

    attempts = comparison.get("attempts")
    require(isinstance(attempts, list) and len(attempts) == 8,
            f"Expected eight comparison attempts for {case_name}")
    by_repeat = {i: [] for i in range(4)}
    per_binary = {name: [] for name in EXPECTED_BINARIES}
    diagnostics = []
    deterministic = []
    manifest_entries = 0
    expected_hash_keys = {
        "simulation_sha256", "events.csv_normalized_sha256", "summary.csv_normalized_sha256"
    }
    for attempt in attempts:
        name, repeat = attempt.get("binary"), attempt.get("repeat")
        require(name in per_binary and repeat in by_repeat,
                f"Unexpected attempt identity in {case_name}")
        by_repeat[repeat].append(attempt)
        per_binary[name].append(attempt)
        require(attempt.get("kind") == ("warmup" if repeat == 0 else "measurement"),
                f"Warmup/measurement label mismatch in {case_name}/{name}/{repeat}")
        require(attempt.get("completed") is True and attempt.get("returncode") == 0,
                f"Attempt did not complete successfully: {case_name}/{name}/{repeat}")
        require(attempt.get("manifest_verified") is True and attempt.get("guard_reason") is None,
                f"Manifest check or guard failed: {case_name}/{name}/{repeat}")
        require(attempt.get("large_outputs_removed_after_recording") is True,
                f"Large output cleanup is not recorded: {case_name}/{name}/{repeat}")
        attempt_dir = attempt.get("evidence_directory")
        require(isinstance(attempt_dir, str) and Path(attempt_dir).name == f"{name}-{repeat}",
                f"Attempt evidence directory does not match identity: {case_name}/{name}/{repeat}")
        attempt_source = str(Path(attempt_dir) / "attempt.json")
        fallback_attempt = str(PurePosixPath(comparison_rel).parent / Path(attempt_dir).name / "attempt.json")
        attempt_rel, attempt_file = support_member_for_original(
            inventory, support_root, attempt_source, fallback_attempt)
        raw_attempt = read_json(attempt_file)
        for key, value in raw_attempt.items():
            require(attempt.get(key) == value,
                    f"comparison.json attempt differs from saved attempt.json at {case_name}/{name}/{repeat}/{key}")
        extras = set(attempt) - set(raw_attempt)
        require(extras == {"binary", "repeat", "kind", "large_outputs_removed_after_recording"},
                f"Unexpected comparison-only attempt fields for {case_name}/{name}/{repeat}: {sorted(extras)}")

        time_candidates = []
        for filename in ("gnu-time.txt", "time.txt"):
            original = str(Path(attempt_dir) / filename)
            fallback = str(PurePosixPath(comparison_rel).parent / Path(attempt_dir).name / filename)
            row = find_support_by_original(inventory, original)
            rel = row.get("support_relative_path") if row else fallback
            if rel in inventory["support_by_relative_path"]:
                time_candidates.append((rel, ensure_no_symlink_components(
                    support_root, PurePosixPath(rel))))
        require(len(time_candidates) == 1,
                f"Expected exactly one archived GNU-time record for {case_name}/{name}/{repeat}")
        time_rel, time_file = time_candidates[0]
        time_lines = [line for line in time_file.read_text(encoding="utf-8").splitlines()
                      if line.startswith("{")]
        require(time_lines, f"Raw GNU-time record is empty: {time_rel}")
        raw_time = json.loads(time_lines[-1])
        measurements = attempt.get("measurements", {})
        for key in ("wall_seconds", "max_rss_kib", "user_seconds", "system_seconds", "exit_status"):
            require(key in raw_time and key in measurements,
                    f"Raw GNU-time/attempt field missing: {case_name}/{name}/{repeat}/{key}")
            numbers_match(raw_time[key], measurements[key],
                          f"{case_name}/{name}/{repeat}/{key}")
        require(measurements.get("termination_signal") is None,
                f"Successful GNU-time attempt records a signal: {case_name}/{name}/{repeat}")
        require(attempt.get("max_rss_bytes") == raw_time["max_rss_kib"] * 1024,
                f"Attempt RSS bytes disagree with GNU time: {case_name}/{name}/{repeat}")
        require(attempt.get("returncode") == raw_time["exit_status"] == 0,
                f"Attempt return code disagrees with GNU time: {case_name}/{name}/{repeat}")
        validate_output_manifest_rows(attempt, f"{case_name}/{name}/{repeat}")
        manifest_entries += 4
        deterministic_row = attempt.get("deterministic")
        require(isinstance(deterministic_row, dict) and set(deterministic_row) == expected_hash_keys,
                f"Normalized output hashes are incomplete: {case_name}/{name}/{repeat}")
        deterministic.append(deterministic_row)
        diagnostics_row = next(row for row in attempt["files"]
                               if row["path"] == "diagnostics.jsonl")
        diagnostics.append(diagnostics_row["sha256"])

    for repeat in range(4):
        expected = list(EXPECTED_BINARIES) if repeat % 2 == 0 else list(reversed(EXPECTED_BINARIES))
        require([row["binary"] for row in by_repeat[repeat]] == expected,
                f"Forward/reverse attempt order mismatch for {case_name} repeat {repeat}")
    for name, rows in per_binary.items():
        require(len(rows) == 4 and sum(row["kind"] == "measurement" for row in rows) == 3,
                f"Expected one warmup plus three measurements for {case_name}/{name}")
    require(all(row == deterministic[0] for row in deterministic),
            f"Simulation or normalized CSV hashes differ in {case_name}")
    require(all(value == diagnostics[0] for value in diagnostics),
            f"diagnostics.jsonl hashes differ in {case_name}")
    require(comparison.get("normalized_outputs_equal") is True,
            f"Helper comparison does not confirm normalized equality for {case_name}")

    raw_stats = {}
    for name, rows in per_binary.items():
        measured = sorted((row for row in rows if row["kind"] == "measurement"),
                          key=lambda row: row["repeat"])
        wall_values = [row["measurements"]["wall_seconds"] for row in measured]
        rss_values = [row["max_rss_bytes"] for row in measured]
        raw_stats[name] = {
            "measurement_repeats": [row["repeat"] for row in measured],
            "wall_seconds": values_summary(wall_values),
            "max_rss_bytes": values_summary(rss_values),
        }
        summary = comparison.get("summaries", {}).get(name, {})
        require(summary.get("completed") is True,
                f"Helper comparison marks {case_name}/{name} incomplete")
        numbers_match(statistics.median(wall_values), summary.get("median_wall_seconds"),
                      f"{case_name}/{name} helper wall median")
        require(summary.get("maximum_rss_bytes") == max(rss_values),
                f"{case_name}/{name} helper RSS maximum differs from raw attempts")

    baseline = raw_stats["baseline"]
    integrated = raw_stats["integrated"]
    deltas_wall = []
    deltas_rss = []
    for repeat in (1, 2, 3):
        left = next(row for row in by_repeat[repeat] if row["binary"] == "baseline")
        right = next(row for row in by_repeat[repeat] if row["binary"] == "integrated")
        deltas_wall.append(right["measurements"]["wall_seconds"] - left["measurements"]["wall_seconds"])
        deltas_rss.append(right["max_rss_bytes"] - left["max_rss_bytes"])
    baseline_wall = baseline["wall_seconds"]["median"]
    recomputed = {
        "case": case_name, "attempt_count": 8,
        "normalized_hashes_equal": True, "normalized_hashes": deterministic[0],
        "diagnostics_sha256_equal": True, "diagnostics_sha256": diagnostics[0],
        "summaries_recomputed_from_gnu_time_attempts": raw_stats,
        "integrated_minus_baseline": {
            "median_wall_seconds": integrated["wall_seconds"]["median"] - baseline_wall,
            "wall_reduction_percent": ((baseline_wall - integrated["wall_seconds"]["median"])
                                        / baseline_wall * 100 if baseline_wall > 0 else None),
            "measurement_repeat_wall_delta_seconds": delta_summary(deltas_wall),
            "median_max_rss_bytes": integrated["max_rss_bytes"]["median"]
                                    - baseline["max_rss_bytes"]["median"],
            "maximum_max_rss_bytes": integrated["max_rss_bytes"]["maximum"]
                                     - baseline["max_rss_bytes"]["maximum"],
            "measurement_repeat_max_rss_delta_bytes": delta_summary(deltas_rss),
        },
    }
    return {
        "attempts": len(attempts), "manifest_entries": manifest_entries,
        "attempt_records_verified": len(attempts), "raw_gnu_time_records_verified": len(attempts),
        "comparison_wall_rss_reconciled": len(attempts),
        "normalized_outputs_equal": True, "diagnostics_sha256_equal": True,
        "comparison_support_path": comparison_rel, "recomputed": recomputed,
        "raw_stats": raw_stats,
    }


def find_numeric_fields(value, key_fragment: str, found: list[tuple[str, int | float]]) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key_fragment in str(key).lower() and isinstance(child, (int, float)) and not isinstance(child, bool):
                found.append((str(key), child))
            find_numeric_fields(child, key_fragment, found)
    elif isinstance(value, list):
        for child in value:
            find_numeric_fields(child, key_fragment, found)


def verify_main_validation_counts(report: dict) -> dict:
    validation = report.get("validation")
    require(isinstance(validation, dict), "Main report validation must be an object")
    expected = {
        "completed_attempts": EXPECTED_ATTEMPTS,
        "warmups": 6,
        "measurements": 18,
        "output_manifest_files_verified": EXPECTED_MANIFEST_ENTRIES,
        "normalized_outputs_equal_case_count": 3,
        "diagnostics_equal_case_count": 3,
        "source_inventory_before": 183,
        "source_inventory_integrated": 184,
    }
    for field, value in expected.items():
        require(validation.get(field) == value,
                f"Main report validation {field} is not {value}")
    for field in ("raw_gnu_time_records_match", "source_changes_exactly_authorized_paths",
                  "unrelated_source_files_unchanged", "large_outputs_removed_after_verification"):
        require(validation.get(field) is True,
                f"Main report validation {field} is not true")
    require(validation.get("full_million_request_verdict") is False,
            "Main report must not claim a full million-request verdict")
    return expected | {"raw_gnu_time_records_match": True,
                       "source_changes_exactly_authorized_paths": True,
                       "unrelated_source_files_unchanged": True,
                       "large_outputs_removed_after_verification": True,
                       "full_million_request_verdict": False}


def get_binary_record(report: dict, name: str) -> dict:
    candidates = [report.get(name, {}).get("binary"), report.get("binaries", {}).get(name)]
    record = next((value for value in candidates if isinstance(value, dict)), None)
    require(record is not None, f"Main report is missing the {name} binary record")
    return record


def verify_local_binary(record: dict, label: str) -> dict:
    raw_path = record.get("path")
    require(isinstance(raw_path, str) and Path(raw_path).is_absolute(),
            f"{label} binary path must be absolute")
    path = Path(raw_path)
    require(path.exists() and not path.is_symlink() and path.is_file(),
            f"{label} local binary is missing, symlinked, or not a regular file: {path}")
    actual_size, actual_hash = path.stat().st_size, sha256_file(path)
    require(record.get("bytes") == actual_size, f"{label} local binary size mismatch")
    require(record.get("sha256") == actual_hash, f"{label} local binary SHA-256 mismatch")
    return {"path": path.as_posix(), "bytes": actual_size, "sha256": actual_hash}


def gate_support_path(name: str, source_map: dict[str, str], inventory: dict,
                      support_root: Path) -> tuple[str, Path, dict]:
    expected_rel = ("gates/baseline/producer-latest.json" if name == "baseline"
                    else "gates/integrated/latest.json")
    possible = []
    row = inventory["support_by_relative_path"].get(expected_rel)
    if row is not None:
        path = ensure_no_symlink_components(support_root, PurePosixPath(expected_rel))
        gate = read_json(path)
        require(gate.get("source_sha256") == source_map,
                f"Archived {name} gate report source map differs from its expected path")
        possible.append((expected_rel, path, gate, row))
    require(len(possible) == 1,
            f"Expected archived {name} gate report at {expected_rel}")
    rel, path, gate, row = possible[0]
    return rel, path, gate


def parse_test_counts(log_path: Path) -> dict:
    totals = {"passed": 0, "failed": 0, "ignored": 0, "suites": 0}
    for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
        match = TEST_RESULT_RE.match(line.strip())
        if match:
            passed, failed, ignored = (int(match.group(i)) for i in (1, 2, 3))
            totals["passed"] += passed
            totals["failed"] += failed
            totals["ignored"] += ignored
            totals["suites"] += 1
    return totals


def resolve_gate_log_path(log_raw: str, gate_rel: str, inventory: dict,
                          support_root: Path) -> tuple[str, Path]:
    row = find_support_by_original(inventory, log_raw)
    if row is not None:
        rel = safe_relative(row["support_relative_path"], "gate log support path").as_posix()
        return rel, ensure_no_symlink_components(support_root, PurePosixPath(rel))
    source_path = Path(log_raw)
    timestamp = source_path.parent.name
    basename = source_path.name
    candidates = []
    for rel in inventory["support_by_relative_path"]:
        parts = PurePosixPath(rel).parts
        if basename == PurePosixPath(rel).name and timestamp in parts and "gates" in parts:
            side = "baseline" if "baseline" in parts else "integrated" if "integrated" in parts else None
            if side == ("baseline" if "baseline" in gate_rel.split("/") else "integrated"):
                candidates.append(rel)
    require(len(candidates) == 1,
            f"Could not uniquely map gate log into support tree: {log_raw}")
    rel = candidates[0]
    return rel, ensure_no_symlink_components(support_root, PurePosixPath(rel))


def verify_gate(name: str, report: dict, source_map: dict[str, str], binary: dict,
                inventory: dict, support_root: Path) -> dict:
    rel, gate_path, gate = gate_support_path(name, source_map, inventory, support_root)
    require(gate.get("source_sha256") == source_map,
            f"{name} latest gate source inventory differs from the main report")
    gate_digest = sha256_file(gate_path)
    if name == "integrated":
        require(report.get("integrated", {}).get("gate_sha256") == gate_digest,
                "Integrated gate hash differs from the main report")
    else:
        producer = report.get("baseline", {}).get("producer_gate")
        require(isinstance(producer, dict), "Main report has no baseline producer gate")
        require(producer.get("sha256") == gate_digest,
                "Baseline producer gate hash differs from archived gate")
        producer_binary = verify_local_binary(producer.get("binary", {}),
                                              "baseline producer")
        require(producer_binary["sha256"] == binary["sha256"]
                and producer_binary["bytes"] == binary["bytes"],
                "Baseline producer and pinned baseline binaries differ")
        require(Path(gate.get("binary", {}).get("path", "")).resolve()
                == Path(producer["binary"]["path"]).resolve(),
                "Baseline gate binary path differs from producer metadata")
    gate_binary = gate.get("binary", {})
    require(gate_binary.get("sha256") == binary["sha256"],
            f"{name} latest gate binary hash differs from main report")
    require(gate_binary.get("bytes") == binary["bytes"],
            f"{name} latest gate binary size differs from main report")
    if name == "integrated":
        require(Path(gate_binary.get("path", "")).resolve() == Path(binary["path"]).resolve(),
                "Integrated latest gate binary path differs from the integrated binary record")

    commands = gate.get("commands")
    require(isinstance(commands, list) and len(commands) == 4,
            f"{name} latest gate must contain four command records")
    actions_seen = []
    verified_logs = []
    test_log_counts = None
    for command in commands:
        argv = command.get("argv")
        require(isinstance(argv, list), f"{name} gate command lacks argv")
        action = next((token for token in argv if token in EXPECTED_COMMANDS), None)
        require(action in EXPECTED_COMMANDS and action not in actions_seen,
                f"{name} gate command identity is invalid or duplicated")
        actions_seen.append(action)
        require(command.get("exit_code") == 0, f"{name} gate command failed: {action}")
        log_raw = command.get("log")
        require(isinstance(log_raw, str) and log_raw,
                f"{name} gate command lacks a log path: {action}")
        log_rel, log_path = resolve_gate_log_path(log_raw, rel, inventory, support_root)
        actual_hash = sha256_file(log_path)
        require(command.get("log_sha256") == actual_hash,
                f"{name} gate log hash mismatch: {action}")
        verified_logs.append({"action": action, "support_path": log_rel,
                              "bytes": log_path.stat().st_size, "sha256": actual_hash})
        if action == "test":
            test_log_counts = parse_test_counts(log_path)
    require(set(actions_seen) == set(EXPECTED_COMMANDS),
            f"{name} latest gate does not contain fmt/clippy/test/build")
    test_summary = gate.get("tests")
    require(isinstance(test_summary, dict), f"{name} latest gate has no tests summary")
    require(test_summary.get("failed") == 0, f"{name} latest gate test summary reports failures")
    require(test_log_counts is not None, f"{name} latest gate test log was not verified")
    for field in ("passed", "failed", "ignored", "suites"):
        if field in test_summary:
            require(test_log_counts[field] == test_summary[field],
                    f"{name} gate test log count differs for {field}")
    main_counts = (report.get("gate_tests") if name == "integrated" else
                   report.get("baseline", {}).get("producer_gate", {}).get("tests"))
    require(isinstance(main_counts, dict), f"Main report has no {name} gate test counts")
    for field in ("passed", "failed", "ignored", "suites"):
        require(main_counts.get(field) == test_log_counts[field],
                f"{name} gate test log count differs from main report for {field}")
    require(test_log_counts["failed"] == 0 and test_log_counts["passed"] > 0,
            f"{name} gate test log does not show a successful test suite")
    return {"latest_gate_support_path": rel,
            "latest_gate_sha256": gate_digest,
            "source_file_count": len(source_map),
            "binary_sha256": binary["sha256"], "binary_bytes": binary["bytes"],
            "commands_verified": len(commands), "logs_verified": len(verified_logs),
            "test_counts_from_log": test_log_counts,
            "test_counts_match_gate": True, "verified_logs": verified_logs}


def verify_controller_copy(report: dict, inventory: dict, support_root: Path) -> dict:
    controller_ref = report.get("controller")
    require(isinstance(controller_ref, dict), "Main report has no controller metadata")
    original = controller_ref.get("original_local_path")
    rel = safe_relative(controller_ref.get("support_relative_path"),
                        "controller.support_relative_path").as_posix()
    require(rel == "measurements/controller.json",
            "Controller support path is not measurements/controller.json")
    row = inventory["support_by_relative_path"].get(rel)
    require(row is not None, "Controller report is absent from support inventory")
    require(row.get("original_local_path") == original,
            "Controller original path differs from its support inventory row")
    path = ensure_no_symlink_components(support_root, PurePosixPath(rel))
    digest = sha256_file(path)
    require(digest == controller_ref.get("sha256") == row.get("sha256"),
            "Controller report hash differs from main report or inventory")
    archived = read_json(path)
    require(archived.get("status") == "complete",
            "Archived controller report is not complete")
    require(archived.get("comparisons") == report.get("comparisons"),
            "Archived controller comparisons differ from main report")
    return {"support_path": rel, "sha256": digest,
            "comparison_count": len(archived.get("comparisons", [])),
            "matches_main_report": True}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True,
                        help="completed can-output-integrated-2026-10-08.json")
    parser.add_argument("--output", type=Path, required=True,
                        help="destination for the independent verification result")
    args = parser.parse_args()
    result = {"schema_version": 1, "status": "failed", "checks": {}}
    output_path = args.output.absolute()
    try:
        require(not args.report.is_symlink(), f"Main report is a symlink: {args.report}")
        report_path = args.report.resolve(strict=True)
        require(report_path.is_file(), f"Main report is missing or unsafe: {report_path}")
        report = read_json(report_path)
        result["input_report"] = report_path.as_posix()
        result["input_report_sha256"] = sha256_file(report_path)
        support_root = resolve_support_root(report_path, report)
        require(output_path.resolve().parent != support_root.resolve()
                and support_root.resolve() not in output_path.resolve().parents,
                "Verification output must be outside the immutable support tree")
        result["support_directory"] = support_root.as_posix()
        result["document_id"] = report.get("document_id")
        inventory = verify_support_inventory(report, support_root)
        result["checks"]["support_inventory"] = {
            key: value for key, value in inventory.items()
            if key not in ("support_by_relative_path", "support_by_original_path")
        }

        source_result = verify_source_snapshots(report, support_root, inventory)
        result["checks"]["source_snapshots_and_scope"] = {
            key: value for key, value in source_result.items()
            if key not in ("patch_path", "baseline_source_map", "integrated_source_map")
        }
        result["checks"]["patch_replay"] = replay_patch(source_result, support_root)

        binary_pins = {}
        binary_local_checks = {}
        for name in EXPECTED_BINARIES:
            record = get_binary_record(report, name)
            binary = verify_local_binary(record, name)
            binary_pins[name] = binary
            binary_local_checks[name] = binary
        result["checks"]["local_binaries"] = binary_local_checks

        source_maps = {
            "baseline": source_result["baseline_source_map"],
            "integrated": source_result["integrated_source_map"],
        }
        gate_checks = {}
        for name in EXPECTED_BINARIES:
            gate_checks[name] = verify_gate(name, report, source_maps[name],
                                            binary_pins[name], inventory, support_root)
        result["checks"]["gates"] = gate_checks
        result["checks"]["controller_copy"] = verify_controller_copy(
            report, inventory, support_root)

        report_validation = verify_main_validation_counts(report)
        result["checks"]["report_validation_counts"] = report_validation
        comparisons = report.get("comparisons")
        require(isinstance(comparisons, list) and len(comparisons) == len(EXPECTED_CASES),
                "Main report must contain exactly three case comparisons")
        case_results = []
        total_attempts = 0
        total_manifests = 0
        for index, case_name in enumerate(EXPECTED_CASES):
            entry = comparisons[index]
            actual_name = entry.get("case", entry.get("id", entry.get("name")))
            require(actual_name == case_name,
                    f"Main report comparison order/name mismatch at index {index}")
            require(entry.get("status") == "complete",
                    f"Main report comparison is not complete: {case_name}")
            comparison_rel, comparison_file = locate_comparison(case_name, inventory, support_root)
            helper_report = read_json(comparison_file)
            case_validation = verify_attempt_evidence(case_name, helper_report, comparison_rel,
                                                       inventory, support_root, binary_pins)
            require(entry.get("observed_attempt_count", case_validation["attempts"])
                    == case_validation["attempts"],
                    f"Controller attempt count differs for {case_name}")
            validation = entry.get("validation", {})
            require(validation.get("attempt_count") == 8,
                    f"Controller validation attempt count differs for {case_name}")
            require(validation.get("normalized_hashes_equal") is True
                    and validation.get("diagnostics_sha256_equal") is True,
                    f"Controller validation reports output inequality for {case_name}")
            compare_nested(case_validation["recomputed"]["normalized_hashes"],
                           validation.get("normalized_hashes"),
                           f"controller.{case_name}.normalized_hashes")
            require(case_validation["recomputed"]["diagnostics_sha256"]
                    == validation.get("diagnostics_sha256"),
                    f"Controller diagnostics hash differs from attempt manifests for {case_name}")
            compare_nested(case_validation["recomputed"]["summaries_recomputed_from_gnu_time_attempts"],
                           validation.get("summaries_recomputed_from_gnu_time_attempts"),
                           f"controller.{case_name}.summaries")
            compare_nested(case_validation["recomputed"]["integrated_minus_baseline"],
                           validation.get("integrated_minus_baseline"),
                           f"controller.{case_name}.deltas")
            total_attempts += case_validation["attempts"]
            total_manifests += case_validation["manifest_entries"]
            case_results.append({
                "case": case_name, "comparison_support_path": comparison_rel,
                "attempts": case_validation["attempts"],
                "manifest_entries": case_validation["manifest_entries"],
                "raw_gnu_time_attempt_wall_rss_reconciled": case_validation["comparison_wall_rss_reconciled"],
                "normalized_outputs_equal": True,
                "diagnostics_sha256_equal": True,
                "recomputed_wall_rss": case_validation["recomputed"],
            })
        require(total_attempts == EXPECTED_ATTEMPTS,
                f"Verified {total_attempts} attempts; expected 24")
        require(total_manifests == EXPECTED_MANIFEST_ENTRIES,
                f"Verified {total_manifests} output manifest rows; expected 96")
        result["checks"]["measurements"] = {
            "cases": case_results, "attempts_verified": total_attempts,
            "manifest_entries_verified": total_manifests,
            "wall_rss_reconciled_attempts": total_attempts,
            "normalized_output_cases": len(case_results),
            "diagnostics_equal_cases": len(case_results),
        }
        result["status"] = "passed"
        result["overall_checks_passed"] = len(result["checks"])
        result["overall_checks_expected"] = len(result["checks"])
        write_json(output_path, result)
        print(json.dumps({"status": result["status"], "output": output_path.as_posix(),
                          "attempts": total_attempts, "manifest_entries": total_manifests},
                         ensure_ascii=False))
        return 0
    except BaseException as error:
        result["failure"] = repr(error)
        try:
            write_json(output_path, result)
        except OSError:
            pass
        print(f"integrated evidence verifier: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
