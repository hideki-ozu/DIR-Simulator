#!/usr/bin/env python3
"""Exercise the public CLI over every non-performance example and fixture INI.

The simulator/browser run is intentionally a separate, explicit operation. The
caller supplies a pinned absolute binary and a fresh output directory.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "docs" / "verification" / "fixtures"
NODE_DEFAULT = Path("/home/hideki/.nvm/versions/node/v22.22.0/bin/node")
NODE_PATH_DEFAULT = "/home/hideki/.npm/_npx/705bc6b22212b352/node_modules"
NETWORK_BROWSER = ROOT / "tests" / "network_viewer_browser.cjs"
NEW_NETWORK_PROFILES = {
    "can.ethernet.gateway.v1",
    "ethernet.l2.dynamic.v1",
    "ethernet.tsn.v1",
}
PREPARE_NEGATIVE_PREFIXES = ("invalid-", "bad-", "prepare-invalid-")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def strict_json_loads(text: str) -> Any:
    def reject_constant(value: str) -> None:
        raise ValueError(f"non-standard JSON constant {value}")

    return json.loads(text, parse_constant=reject_constant)


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def discover_inputs() -> tuple[list[Path], list[str]]:
    roots = (ROOT / "examples", FIXTURES)
    excluded: list[str] = []
    inputs: list[Path] = []
    performance = FIXTURES / "performance"
    for source_root in roots:
        for path in source_root.rglob("*.ini"):
            if not path.is_file():
                continue
            if path == performance or performance in path.parents:
                excluded.append(path.relative_to(ROOT).as_posix())
                continue
            inputs.append(path)
    return sorted(inputs), sorted(excluded)


def case_name(config: Path) -> str:
    # New profile directories use a uniform run.ini name; classify those by
    # their containing fixture directory while retaining direct fixture stems.
    return config.parent.name if config.name == "run.ini" else config.stem


def expectations(config: Path) -> dict[str, Any]:
    name = case_name(config)
    if name == "registration-capacity-error":
        return {"classification": "runtime-negative", "validate_exit": 0, "run_exit": 3}
    if name.startswith("codec-invalid-"):
        return {
            "classification": "valid-config-runtime-codec-rejection",
            "validate_exit": 0,
            "run_exit": 0,
        }
    if name.startswith(PREPARE_NEGATIVE_PREFIXES):
        return {"classification": "prepare-negative", "validate_exit": 2, "run_exit": 2}
    return {"classification": "ordinary", "validate_exit": 0, "run_exit": 0}


def _decode_output(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def invoke(
    argv: list[str],
    *,
    cwd: Path,
    expected_exit: int,
    env_overrides: dict[str, str] | None = None,
    timeout_s: int = 180,
) -> dict[str, Any]:
    env = os.environ.copy()
    env.update(env_overrides or {})
    started = time.monotonic()
    try:
        completed = subprocess.run(
            argv,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_s,
            check=False,
        )
        actual_exit = completed.returncode
        stdout, stderr = completed.stdout, completed.stderr
        timed_out = False
        launch_error = None
    except subprocess.TimeoutExpired as error:
        actual_exit = None
        stdout, stderr = _decode_output(error.stdout), _decode_output(error.stderr)
        timed_out = True
        launch_error = f"timed out after {timeout_s} seconds"
    except OSError as error:
        actual_exit = None
        stdout, stderr = "", str(error)
        timed_out = False
        launch_error = f"could not start command: {error}"
    duration = time.monotonic() - started
    return {
        "command": shlex.join(argv),
        "argv": argv,
        "cwd": str(cwd),
        "env_overrides": env_overrides or {},
        "expected_exit_code": expected_exit,
        "actual_exit_code": actual_exit,
        "timed_out": timed_out,
        "launch_error": launch_error,
        "duration_seconds": round(duration, 6),
        "stdout": stdout,
        "stderr": stderr,
        "status": "passed" if actual_exit == expected_exit else "failed",
    }


def source_inputs(result: dict[str, Any]) -> list[dict[str, Any]]:
    """Record provenance paths and hashes without copying source contents."""
    metadata = result.get("metadata")
    if not isinstance(metadata, dict):
        return []
    sources = metadata.get("sources", [])
    if not isinstance(sources, list):
        return []
    recorded = []
    for source in sources:
        if not isinstance(source, dict):
            continue
        raw_path = source.get("canonical_path")
        path = Path(raw_path) if isinstance(raw_path, str) else None
        row = {
            "logical_path": source.get("logical_path"),
            "canonical_path": raw_path,
            "reported_sha256": source.get("sha256"),
        }
        if path is not None and path.is_file():
            row["actual_sha256"] = sha256(path)
            row["bytes"] = path.stat().st_size
            row["hash_matches_report"] = row["actual_sha256"] == row["reported_sha256"]
        else:
            row["actual_sha256"] = None
            row["bytes"] = None
            row["hash_matches_report"] = None
        recorded.append(row)
    return recorded


def inspect_run_output(run_dir: Path) -> dict[str, Any]:
    files = sorted(path for path in run_dir.iterdir() if path.is_file()) if run_dir.is_dir() else []
    artifact_rows = [
        {"name": path.name, "bytes": path.stat().st_size, "sha256": sha256(path)}
        for path in files
    ]
    manifest_path = run_dir / "manifest.json"
    result_path = run_dir / "results.json"
    details: dict[str, Any] = {
        "run_directory": str(run_dir),
        "artifacts": artifact_rows,
        "manifest": None,
        "manifest_verification": "not-published",
        "results_json": None,
        "results_parse": "not-published",
        "result_model_profile": None,
        "simulation_partial": None,
        "source_inputs": [],
    }
    parsed_result = None
    if manifest_path.is_file():
        try:
            manifest = strict_json_loads(manifest_path.read_text(encoding="utf-8"))
            if not isinstance(manifest, dict):
                raise ValueError("top-level manifest JSON value is not an object")
            details["manifest"] = manifest
            failures = []
            entries = manifest.get("files", [])
            if not isinstance(entries, list):
                raise ValueError("manifest files field is not an array")
            if sorted(entry.get("name", "") for entry in entries if isinstance(entry, dict)) != ["diagnostics.jsonl", "events.csv", "results.json", "summary.csv"]:
                failures.append("manifest does not contain exactly four standard artifacts")
            if manifest.get("status") != "complete" or manifest.get("metadata_ref") != "results.json#/metadata":
                failures.append("manifest publication or metadata reference differs")
            for entry in entries:
                if not isinstance(entry, dict):
                    failures.append("manifest file entry is not an object")
                    continue
                name = entry.get("name")
                path = run_dir / name if isinstance(name, str) else None
                if path is None or not path.is_file():
                    failures.append(f"missing artifact {name!r}")
                    continue
                if sha256(path) != entry.get("sha256"):
                    failures.append(f"SHA-256 mismatch for {name}")
                try:
                    expected_bytes = int(entry.get("bytes"))
                except (TypeError, ValueError):
                    failures.append(f"invalid byte count for {name}")
                else:
                    if path.stat().st_size != expected_bytes:
                        failures.append(f"size mismatch for {name}")
            details["manifest_verification"] = "failed" if failures else "passed"
            details["manifest_errors"] = failures
        except (OSError, ValueError, TypeError) as error:
            details["manifest_verification"] = "failed"
            details["manifest_errors"] = [f"manifest parse/read failed: {error}"]
    if result_path.is_file():
        details["results_json"] = {
            "path": str(result_path),
            "bytes": result_path.stat().st_size,
            "sha256": sha256(result_path),
        }
        try:
            parsed_result = strict_json_loads(result_path.read_text(encoding="utf-8"))
            if not isinstance(parsed_result, dict):
                raise ValueError("top-level JSON value is not an object")
            details["results_parse"] = "passed"
            metadata = parsed_result.get("metadata", {})
            if isinstance(metadata, dict):
                details["result_model_profile"] = metadata.get("model_profile")
            simulation = parsed_result.get("simulation", {})
            if isinstance(simulation, dict):
                details["simulation_partial"] = simulation.get("partial")
            details["source_inputs"] = source_inputs(parsed_result)
            manifest = details.get("manifest")
            if isinstance(manifest, dict):
                checks = [(manifest.get("run_id"), parsed_result.get("run_id")), (manifest.get("schema_version"), parsed_result.get("schema_version"))]
                if isinstance(simulation, dict):
                    checks.extend((manifest.get(key), simulation.get(key)) for key in ("termination", "partial"))
                if any(expected != actual for expected, actual in checks):
                    details["manifest_verification"] = "failed"
                    details.setdefault("manifest_errors", []).append("manifest/result identity, version or termination differs")
        except (OSError, ValueError, TypeError) as error:
            details["results_parse"] = "failed"
            details["results_parse_error"] = str(error)
    return details


def safe_id(relative: Path) -> str:
    return relative.as_posix().replace("/", "__").removesuffix(".ini")


def case_is_successful(case: dict[str, Any]) -> bool:
    if case["validate_command"]["status"] != "passed" or case["run_command"]["status"] != "passed":
        return False
    run_expected = case["expectations"]["run_exit"]
    output = case["artifact_export"]
    if run_expected in (0, 2, 3) and output["manifest_verification"] != "passed":
        return False
    if output["manifest_verification"] == "failed" or output["results_parse"] == "failed":
        return False
    if any(source.get("hash_matches_report") is False for source in output.get("source_inputs", [])):
        return False
    if case.get("viewer_command") and case["viewer_command"]["status"] != "passed":
        return False
    if case.get("viewer_command") and not case.get("viewer_output", {}).get("exists"):
        return False
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True, help="pinned absolute dir-simulator executable")
    parser.add_argument("--output", type=Path, required=True, help="fresh output directory (must not exist)")
    parser.add_argument("--node", type=Path, default=NODE_DEFAULT, help="absolute Node.js executable")
    parser.add_argument("--node-path", default=NODE_PATH_DEFAULT, help="Playwright node_modules for NODE_PATH")
    parser.add_argument("--command-timeout", type=int, default=180)
    parser.add_argument("--browser-timeout", type=int, default=300)
    args = parser.parse_args()

    if not args.binary.is_absolute():
        parser.error("--binary must be an absolute path")
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"--binary is not an executable file: {binary}")
    if not args.node.is_absolute():
        parser.error("--node must be an absolute path")
    node = args.node.resolve()
    if not node.is_file() or not os.access(node, os.X_OK):
        parser.error(f"--node is not an executable file: {node}")
    if not NETWORK_BROWSER.is_file():
        parser.error(f"browser harness not found: {NETWORK_BROWSER}")
    if args.command_timeout < 1 or args.browser_timeout < 1:
        parser.error("timeouts must be positive integers")

    output = args.output.resolve()
    try:
        output.mkdir(parents=True, exist_ok=False)
    except FileExistsError:
        parser.error(f"--output must be fresh and must not already exist: {output}")

    inputs, excluded = discover_inputs()
    if not inputs:
        parser.error("no ordinary .ini inputs found under examples or docs/verification/fixtures")

    cases: list[dict[str, Any]] = []
    browser_coverage: list[dict[str, Any]] = []
    progress = {
        "schema_version": 1,
        "binary": str(binary),
        "binary_sha256": sha256(binary),
        "input_count": len(inputs),
        "excluded_performance_inputs": excluded,
        "completed_cases": 0,
        "completed_browser_checks": 0,
    }
    write_json(output / "progress.json", progress)

    for index, config in enumerate(inputs, start=1):
        relative = config.relative_to(ROOT)
        config_sha = sha256(config)
        expected = expectations(config)
        case_id = safe_id(relative)
        case_dir = output / "cases" / case_id
        case_dir.mkdir(parents=True, exist_ok=True)
        run_dir = case_dir / "run"
        case: dict[str, Any] = {
            "case_id": case_id,
            "input": str(config),
            "input_sha256": config_sha,
            "input_bytes": config.stat().st_size,
            "expectations": expected,
        }

        validation = invoke(
            [str(binary), "validate", "--config", str(config)],
            cwd=ROOT,
            expected_exit=expected["validate_exit"],
            timeout_s=args.command_timeout,
        )
        case["validate_command"] = validation
        run_command = invoke(
            [str(binary), "run", "--config", str(config), "--output", str(run_dir)],
            cwd=ROOT,
            expected_exit=expected["run_exit"],
            timeout_s=args.command_timeout,
        )
        case["run_command"] = run_command
        case["artifact_export"] = inspect_run_output(run_dir)

        results_path = run_dir / "results.json"
        if results_path.is_file():
            viewer_path = case_dir / "viewer.html"
            viewer_command = invoke(
                [str(binary), "view", "--input", str(results_path), "--output", str(viewer_path)],
                cwd=ROOT,
                expected_exit=0,
                timeout_s=args.command_timeout,
            )
            case["viewer_command"] = viewer_command
            case["viewer_output"] = {
                "path": str(viewer_path),
                "exists": viewer_path.is_file(),
                "bytes": viewer_path.stat().st_size if viewer_path.is_file() else None,
                "sha256": sha256(viewer_path) if viewer_path.is_file() else None,
            }
        else:
            case["viewer_command"] = None
            case["viewer_output"] = {"path": str(case_dir / "viewer.html"), "exists": False}

        case["status"] = "passed" if case_is_successful(case) else "failed"
        case_report_path = case_dir / "case.json"
        case["case_report"] = str(case_report_path)
        write_json(case_report_path, case)
        cases.append(case)

        profile = case["artifact_export"].get("result_model_profile")
        positive_result = (
            run_command["actual_exit_code"] == 0
            and case["artifact_export"].get("results_parse") == "passed"
            and profile in NEW_NETWORK_PROFILES
        )
        if positive_result:
            result_path = results_path
            node_env = {"NODE_PATH": args.node_path}
            parse_command = invoke(
                [
                    str(node),
                    "-e",
                    "const fs=require('node:fs');const p=process.argv[1];JSON.parse(fs.readFileSync(p,'utf8'));process.stdout.write('strict JSON.parse passed: '+p+'\\n');",
                    str(result_path),
                ],
                cwd=ROOT,
                expected_exit=0,
                env_overrides=node_env,
                timeout_s=args.command_timeout,
            )
            browser_command = invoke(
                [str(node), str(NETWORK_BROWSER), str(result_path)],
                cwd=ROOT,
                expected_exit=0,
                env_overrides=node_env,
                timeout_s=args.browser_timeout,
            )
            exported_html_path = Path(case["viewer_output"]["path"])
            exported_html_command = invoke(
                [
                    str(node),
                    str(NETWORK_BROWSER),
                    "--exported-html",
                    str(result_path),
                    str(exported_html_path),
                ],
                cwd=ROOT,
                expected_exit=0,
                env_overrides=node_env,
                timeout_s=args.browser_timeout,
            )
            browser_row = {
                "case_id": case_id,
                "input": str(config),
                "result_path": str(result_path),
                "model_profile": profile,
                "simulation_partial": case["artifact_export"].get("simulation_partial"),
                "strict_js_parse": parse_command,
                "browser_harness": str(NETWORK_BROWSER),
                "production_assets_browser_command": browser_command,
                "exported_html_path": str(exported_html_path),
                "exported_html_exists": exported_html_path.is_file(),
                "exported_html_browser_command": exported_html_command,
                "status": "passed"
                if parse_command["status"] == "passed"
                and browser_command["status"] == "passed"
                and exported_html_command["status"] == "passed"
                else "failed",
            }
            browser_coverage.append(browser_row)
            case["browser_coverage_status"] = browser_row["status"]
            if browser_row["status"] != "passed":
                case["status"] = "failed"
            write_json(case_report_path, case)
            write_json(output / "browser-coverage.json", browser_coverage)

        progress.update(
            {
                "completed_cases": len(cases),
                "completed_browser_checks": len(browser_coverage),
                "last_case_id": case_id,
                "last_case_status": case["status"],
            }
        )
        write_json(output / "progress.json", progress)
        print(
            f"[{index}/{len(inputs)}] {relative.as_posix()}: {case['status']}"
            + (f"; browser={case.get('browser_coverage_status')}" if positive_result else ""),
            flush=True,
        )

    overall = "passed" if all(case["status"] == "passed" for case in cases) else "failed"
    report = {
        "schema_version": 1,
        "status": overall,
        "binary": str(binary),
        "binary_sha256": sha256(binary),
        "repository_root": str(ROOT),
        "input_count": len(inputs),
        "excluded_performance_inputs": excluded,
        "expected_status_policy": {
            "prepare_negative_filename_prefixes": list(PREPARE_NEGATIVE_PREFIXES),
            "classification_name": "INI filename stem, or containing directory name for run.ini",
            "prepare_negative_exit": 2,
            "registration_capacity_error_exit": 3,
            "codec_invalid_validation_and_run_exit": 0,
            "ordinary_validation_and_run_exit": 0,
        },
        "new_network_profiles_browser_policy": sorted(NEW_NETWORK_PROFILES),
        "case_counts": {
            "passed": sum(case["status"] == "passed" for case in cases),
            "failed": sum(case["status"] == "failed" for case in cases),
        },
        "browser_coverage": {
            "production_assets_and_exported_html_reported_separately": True,
            "harness": str(NETWORK_BROWSER),
            "candidate_count": len(browser_coverage),
            "passed": sum(row["status"] == "passed" for row in browser_coverage),
            "failed": sum(row["status"] == "failed" for row in browser_coverage),
            "production_assets_passed": sum(row["production_assets_browser_command"]["status"] == "passed" for row in browser_coverage),
            "production_assets_failed": sum(row["production_assets_browser_command"]["status"] == "failed" for row in browser_coverage),
            "exported_html_passed": sum(row["exported_html_browser_command"]["status"] == "passed" for row in browser_coverage),
            "exported_html_failed": sum(row["exported_html_browser_command"]["status"] == "failed" for row in browser_coverage),
            "cases": browser_coverage,
        },
        "cases": cases,
    }
    write_json(output / "report.json", report)
    progress.update({"completed": True, "status": overall})
    write_json(output / "progress.json", progress)
    print(
        json.dumps(
            {
                "status": overall,
                "input_count": len(inputs),
                "browser_candidates": len(browser_coverage),
                "report": str(output / "report.json"),
            },
            ensure_ascii=False,
        )
    )
    return 0 if overall == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
