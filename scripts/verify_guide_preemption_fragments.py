"""Run and record the v1.1.6 fragment-payload comparison for guide issue #110.

ZIP reproduction mode requires --baseline-output to name the saved directory
that contains fragment-20/result/results.json, fragment-40/result/results.json,
and fragment-100/result/results.json.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import subprocess
import zipfile
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
INPUT = ROOT / "examples/guide/ethernet-preemption-fragments"
EXPECTED_COMMIT = "397900451015c9bca6a33d24feac0a88b9b7b8fe"
ARCHIVE = ROOT / "docs/guide/downloads/ethernet-preemption-fragments-inputs.zip"
ARCHIVE_PREFIX = "examples/guide/ethernet-preemption-fragments/"
CASES = (20, 40, 100)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def load(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def record_id(row: dict[str, Any]) -> str:
    return str(row.get("record_id", row.get("id", "")))


def verify_inputs(input_root: Path = INPUT) -> dict[str, Any]:
    source_model = load(ROOT / "examples/network-extensions/preemption-model.json")
    source_workload = load(ROOT / "examples/network-extensions/preemption-workload.json")
    source_ned = ROOT / "examples/ethernet/tsn/models/ethtsn/Main.ned"
    workload = load(input_root / "preemption-workload.json")
    assert workload == source_workload, "workload must be copied unchanged"
    assert (input_root / "models/ethtsn/Main.ned").read_bytes() == source_ned.read_bytes()
    normalized: list[dict[str, Any]] = []
    ini_normalized: list[str] = []
    input_hashes: dict[str, str] = {}
    for size in CASES:
        model_path = input_root / f"fragment-{size}-model.json"
        model = load(model_path)
        outputs = model["preemption"]["outputs"]
        assert [x["port"] for x in outputs] == ["Net.a.tx", "Net.b.tx"]
        assert outputs[0]["fragment_payload_bytes"] == size
        assert outputs[1]["fragment_payload_bytes"] == 40
        normalized_model = copy.deepcopy(model)
        normalized_model["preemption"]["outputs"][0]["fragment_payload_bytes"] = 0
        normalized.append(normalized_model)
        ini = (input_root / f"fragment-{size}.ini").read_text(encoding="utf-8")
        assert f'model-config = "fragment-{size}-model.json"' in ini
        assert "model-profile = \"ethernet.preemption.abstract.v1\"" in ini
        assert "sim-time-limit = 2000000ps" in ini
        ini_normalized.append(ini.replace(f"fragment-{size}-model.json", "MODEL.json"))
    assert normalized[0] == normalized[1] == normalized[2], "model JSONs differ outside Net.a fragment size"
    assert ini_normalized[0] == ini_normalized[1] == ini_normalized[2], "INI settings differ outside model file name"
    for path in sorted(p for p in input_root.rglob("*") if p.is_file()):
        relative = path.relative_to(input_root).as_posix()
        key = f"{ARCHIVE_PREFIX}{relative}" if input_root != INPUT else path.relative_to(ROOT).as_posix()
        input_hashes[key] = sha256(path)
    return {"input_sha256": input_hashes, "conditions": list(CASES)}


def extract_and_verify_archive(extraction_root: Path) -> dict[str, Any]:
    archive_hash = sha256(ARCHIVE)
    expected_files = {
        path.relative_to(INPUT).as_posix(): path
        for path in INPUT.rglob("*") if path.is_file()
    }
    member_hashes: dict[str, dict[str, Any]] = {}
    extraction_root.parent.mkdir(parents=True, exist_ok=True)
    extraction_root.mkdir(parents=False, exist_ok=False)
    with zipfile.ZipFile(ARCHIVE) as zipped:
        infos = zipped.infolist()
        members = [info for info in infos if not info.is_dir()]
        names = [info.filename for info in members]
        assert len(names) == len(set(names)), "ZIP contains duplicate file members"
        expected_names = {ARCHIVE_PREFIX + relative for relative in expected_files}
        assert set(names) == expected_names, {
            "missing": sorted(expected_names - set(names)),
            "unexpected": sorted(set(names) - expected_names),
        }
        for info in members:
            member = PurePosixPath(info.filename)
            assert not member.is_absolute() and ".." not in member.parts, info.filename
            assert info.filename.startswith(ARCHIVE_PREFIX), info.filename
            relative = info.filename.removeprefix(ARCHIVE_PREFIX)
            raw = zipped.read(info)
            source = expected_files[relative]
            assert raw == source.read_bytes(), f"ZIP member differs from input: {info.filename}"
            target = extraction_root / info.filename
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(raw)
            member_hashes[info.filename] = {"bytes": len(raw), "sha256": digest(raw)}
    extracted_input = extraction_root / ARCHIVE_PREFIX
    return {
        "archive": str(ARCHIVE.relative_to(ROOT)),
        "archive_bytes": ARCHIVE.stat().st_size,
        "archive_sha256": archive_hash,
        "member_count": len(member_hashes),
        "members": member_hashes,
        "member_bytes_equal_source_tree": True,
        "extraction_root": str(extraction_root),
        "input_root": str(extracted_input),
    }


def execute(binary: Path, argv: list[str], log_path: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    command = [str(binary), *argv]
    proc = subprocess.run(command, text=True, capture_output=True, check=False)
    log_path.write_text(
        "COMMAND: " + json.dumps(command, ensure_ascii=False) + "\n"
        + f"EXIT_CODE: {proc.returncode}\nSTDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}",
        encoding="utf-8",
    )
    if proc.returncode != 0:
        raise RuntimeError(f"command failed ({proc.returncode}): {command}; see {log_path}")
    try:
        value = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"expected JSON stdout from {command}; see {log_path}") from exc
    return value, {"command": command, "exit_code": proc.returncode, "stdout": value}


def verify_manifest(output: Path) -> dict[str, Any]:
    manifest_path = output / "manifest.json"
    manifest = load(manifest_path)
    assert manifest.get("status") == "complete", manifest
    assert not manifest.get("partial", False), manifest
    files: dict[str, dict[str, Any]] = {}
    for item in manifest.get("files", []):
        name = str(item["name"])
        raw = (output / name).read_bytes()
        assert str(len(raw)) == str(item["bytes"]), name
        assert digest(raw) == item["sha256"], name
        files[name] = {"bytes": len(raw), "sha256": item["sha256"]}
    diagnostics = output / "diagnostics.jsonl"
    diag_lines = diagnostics.read_text(encoding="utf-8").splitlines() if diagnostics.exists() else []
    assert not diag_lines, f"diagnostics are not empty: {diag_lines}"
    return {"status": manifest["status"], "partial": manifest.get("partial", False), "files": files, "diagnostic_lines": diag_lines}


def records_by_schema(result: dict[str, Any]) -> dict[str, list[dict[str, Any]]]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for row in result["simulation"].get("model_records", []):
        grouped.setdefault(str(row.get("schema_name", "")), []).append(row)
    return grouped


def summarize_condition(result: dict[str, Any]) -> dict[str, Any]:
    groups = records_by_schema(result)
    transfers = groups.get("dir.preemption.transfer", [])
    slow_rows = [r for r in transfers if r.get("data", {}).get("kind") == "preemptable"]
    express_rows = [r for r in transfers if r.get("data", {}).get("kind") == "express"]
    if len(slow_rows) != 1 or len(express_rows) != 1:
        raise AssertionError(f"expected one slow and one express transfer, saw {len(slow_rows)} and {len(express_rows)}")
    slow = slow_rows[0]
    express = express_rows[0]
    slow_data, express_data = slow["data"], express["data"]
    fragments = groups.get("dir.preemption.fragment", [])
    gaps = groups.get("dir.preemption.gap", [])
    fragment_map = {record_id(r): r for r in fragments}
    gap_map = {record_id(r): r for r in gaps}
    fragment_ids = slow_data.get("fragment_ids", [])
    gap_ids = slow_data.get("gap_ids", [])
    return {
        "express": {
            "record_id": record_id(express), "queued_ps": express_data.get("queued_ps"),
            "sof_ps": express_data.get("sof_ps"),
            "wait_ps": (int(express_data["sof_ps"]) - int(express_data["queued_ps"])) if express_data.get("sof_ps") is not None else None,
            "eof_ps": express_data.get("eof_ps"), "release_ps": express_data.get("release_ps"), "status": express_data.get("status"),
        },
        "slow": {
            "record_id": record_id(slow), "queued_ps": slow_data.get("queued_ps"), "sof_ps": slow_data.get("sof_ps"),
            "eof_ps": slow_data.get("eof_ps"), "occupied_ps": slow_data.get("occupied_ps"), "status": slow_data.get("status"),
            "fragment_count": len(fragment_ids), "fragments": [fragment_map.get(str(fid), {"missing_record_id": fid}) for fid in fragment_ids],
            "gap_count": len(gap_ids), "gaps": [gap_map.get(str(gid), {"missing_record_id": gid}) for gid in gap_ids],
        },
        "termination": result["simulation"].get("termination"),
        "partial": result["simulation"].get("partial"),
        "end_ps": result["simulation"].get("end_ps"),
        "last_event_time_ps": result["simulation"].get("last_event_time_ps"),
        "pending_events": result["simulation"].get("pending_events"),
        "schemas": {k: len(v) for k, v in sorted(groups.items()) if k.startswith("dir.preemption.")},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--zip-reproduction", action="store_true", help="extract and measure the published guide input archive")
    parser.add_argument("--baseline-output", type=Path, help="saved repository-input results root (required with --zip-reproduction)")
    parser.add_argument("--publish-report", action="store_true", help="also update docs/verification/results/preemption-fragments/measurements.json")
    args = parser.parse_args()
    binary = args.binary.resolve()
    output_root = args.output.resolve()
    assert binary.is_file(), binary
    archive_info: dict[str, Any] | None = None
    input_root = INPUT
    if args.zip_reproduction:
        if args.baseline_output is None:
            parser.error("--baseline-output is required with --zip-reproduction")
        baseline_root = args.baseline_output.resolve()
        if not baseline_root.is_dir():
            parser.error(f"--baseline-output is not a directory: {baseline_root}")
        extraction_root = output_root.parent / "inputs/preemption-fragments"
        archive_info = extract_and_verify_archive(extraction_root)
        input_root = Path(archive_info["input_root"])
    else:
        baseline_root = None
    verified_inputs = verify_inputs(input_root)
    binary_hash = sha256(binary)
    output_root.parent.mkdir(parents=True, exist_ok=True)
    output_root.mkdir(parents=False, exist_ok=False)
    report: dict[str, Any] = {
        "status": "running", "issue": 110, "source_tag": "v1.1.6", "source_commit": EXPECTED_COMMIT,
        "source_status": "", "rustc": None, "binary_sha256": binary_hash,
        **verified_inputs, "conditions": {},
    }
    for size in CASES:
        name = f"fragment-{size}"
        case_dir = output_root / name
        case_dir.mkdir()
        config = input_root / f"{name}.ini"
        validate, validate_meta = execute(binary, ["validate", "--config", str(config)], case_dir / "validate.log")
        assert validate.get("status") == "valid", validate
        result_dir = case_dir / "result"
        run, run_meta = execute(binary, ["run", "--config", str(config), "--output", str(result_dir)], case_dir / "run.log")
        assert run.get("exit_code", 0) == 0 and run.get("primary_diagnostic") is None, run
        result = load(result_dir / "results.json")
        metadata = result["metadata"]
        assert metadata["git_commit"] == EXPECTED_COMMIT, metadata
        assert metadata["git_dirty"] in (False, "false", "0", 0), metadata
        assert metadata["binary_sha256"] == report["binary_sha256"], metadata
        assert metadata.get("compiler"), metadata
        if report["rustc"] is None:
            report["rustc"] = metadata["compiler"]
        assert metadata["compiler"] == report["rustc"], metadata
        manifest = verify_manifest(result_dir)
        sim = result["simulation"]
        assert sim.get("termination") == "events_exhausted" and sim.get("partial") is False, sim
        assert str(sim.get("pending_events")) == "0", sim
        assert not manifest["diagnostic_lines"], manifest
        viewer = case_dir / "viewer.html"
        view, view_meta = execute(binary, ["view", "--input", str(result_dir / "results.json"), "--output", str(viewer)], case_dir / "view.log")
        assert view.get("status") == "complete" and viewer.is_file(), view
        report["conditions"][name] = {
            "commands": {"validate": validate_meta, "run": run_meta, "view": view_meta},
            "input_sha256": {k: v for k, v in verified_inputs["input_sha256"].items() if name in k or k.endswith("preemption-workload.json") or k.endswith("Main.ned")},
            "provenance": {key: metadata.get(key) for key in ("git_commit", "git_dirty", "compiler", "binary_sha256", "build_source_sha256", "cargo_lock_sha256")},
            "source_sha256": {s.get("logical_path"): s.get("sha256") for s in metadata.get("sources", [])},
            "manifest": manifest,
            "summary": summarize_condition(result),
            "viewer_sha256": sha256(viewer),
        }
        if args.zip_reproduction:
            assert baseline_root is not None
            baseline_result = baseline_root / name / "result/results.json"
            assert baseline_result.is_file(), baseline_result
            baseline = load(baseline_result)
            assert sim == baseline["simulation"], f"ZIP simulation differs from saved repository input: {name}"
            report["conditions"][name]["repository_baseline"] = {
                "results_sha256": sha256(baseline_result),
                "simulation_equal": True,
                "simulation_sha256": digest(json.dumps(sim, sort_keys=True, ensure_ascii=False).encode("utf-8")),
            }
    report["status"] = "passed"
    zip_section: dict[str, Any] | None = None
    if args.zip_reproduction:
        assert archive_info is not None
        assert baseline_root is not None
        zip_section = {
            **archive_info,
            "binary_sha256": binary_hash,
            "source_commit": EXPECTED_COMMIT,
            "source_status": "",
            "rustc": report["rustc"],
            "baseline_output": str(baseline_root),
            "cli_operations": sum(len(v["commands"]) for v in report["conditions"].values()),
            "conditions": report["conditions"],
        }
        report["zip_reproduction"] = zip_section
    serialized = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    (output_root / "measurements.json").write_text(serialized, encoding="utf-8")
    if args.publish_report:
        measurements_path = ROOT / "docs/verification/results/preemption-fragments/measurements.json"
        measurements_path.parent.mkdir(parents=True, exist_ok=True)
        if args.zip_reproduction:
            prior = load(measurements_path)
            assert prior.get("status") == "passed" and prior.get("issue") == 110, prior
            assert "zip_reproduction" not in prior, "refusing to replace existing ZIP reproduction evidence"
            prior["zip_reproduction"] = zip_section
            published = prior
        else:
            published = report
        temporary = measurements_path.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(published, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        temporary.replace(measurements_path)
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
