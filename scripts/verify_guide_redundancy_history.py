"""Run and record the v1.1.6 history-capacity comparison for guide issue #111.

ZIP reproduction mode requires --baseline-output to name the saved directory
that contains capacity-1/result/results.json and capacity-4/result/results.json.
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
INPUT = ROOT / "examples/guide/ethernet-redundancy-history"
EXPECTED_COMMIT = "397900451015c9bca6a33d24feac0a88b9b7b8fe"
ARCHIVE = ROOT / "docs/guide/downloads/ethernet-redundancy-history-inputs.zip"
ARCHIVE_PREFIX = "examples/guide/ethernet-redundancy-history/"
CASES = (1, 4)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def load(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def record_id(row: dict[str, Any]) -> str:
    return str(row.get("record_id", row.get("id", "")))


def verify_inputs(input_root: Path = INPUT) -> dict[str, Any]:
    source_model = load(ROOT / "examples/network-extensions/redundancy-model.json")
    source_workload = load(ROOT / "examples/network-extensions/redundancy-workload.json")
    source_ned = ROOT / "examples/ethernet/dynamic/models/ethdynamic/Main.ned"
    workload = load(input_root / "redundancy-workload.json")
    expected_workload = copy.deepcopy(source_workload)
    expected_workload["generators"][0]["times_ps"] = ["0", "2000000"]
    assert workload == expected_workload, "workload must contain only the issue's common two-origin preparation"
    assert (input_root / "models/ethdynamic/Main.ned").read_bytes() == source_ned.read_bytes()
    normalized: list[dict[str, Any]] = []
    ini_normalized: list[str] = []
    input_hashes: dict[str, str] = {}
    for capacity in CASES:
        model_path = input_root / f"capacity-{capacity}-model.json"
        model = load(model_path)
        dynamic = model["base"]["config"]["base"]["config"]
        switches = {s["instance"]: s for s in dynamic["switches"]}
        expected_delays = {"Net.s1": "2000", "Net.s2": "2000", "Net.s3": "5000000"}
        assert {key: switches[key]["forward_delay_ps"] for key in expected_delays} == expected_delays
        stream = model["replication"]["streams"][0]
        assert stream["history_capacity"] == capacity
        normalized_model = copy.deepcopy(model)
        normalized_model["replication"]["streams"][0]["history_capacity"] = 0
        normalized.append(normalized_model)
        ini = (input_root / f"capacity-{capacity}.ini").read_text(encoding="utf-8")
        assert f'model-config = "capacity-{capacity}-model.json"' in ini
        assert "model-profile = \"ethernet.redundancy.abstract.v1\"" in ini
        assert "sim-time-limit = 10000000ps" in ini
        ini_normalized.append(ini.replace(f"capacity-{capacity}-model.json", "MODEL.json"))
    assert normalized[0] == normalized[1], "capacity model JSONs differ outside history_capacity"
    assert ini_normalized[0] == ini_normalized[1], "INI settings differ outside model file name"
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
    manifest = load(output / "manifest.json")
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


def by_schema(result: dict[str, Any]) -> dict[str, list[dict[str, Any]]]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for row in result["simulation"].get("model_records", []):
        grouped.setdefault(str(row.get("schema_name", "")), []).append(row)
    return grouped


def summary(result: dict[str, Any]) -> dict[str, Any]:
    groups = by_schema(result)
    origins = groups.get("dir.redundancy.origin", [])
    branches = groups.get("dir.redundancy.branch", [])
    recoveries = groups.get("dir.redundancy.recovery", [])
    transfers = groups.get("ethernet.dynamic.transfer", [])
    receptions = groups.get("ethernet.dynamic.reception", [])
    reception_data = [r["data"] for r in receptions]
    rows = []
    for origin_row in sorted(origins, key=lambda r: int(r["data"]["sequence"])):
        origin = origin_row["data"]
        origin_branches = [r["data"] for r in branches if r["data"].get("origin_id") == origin["origin_id"]]
        origin_recoveries = [r["data"] for r in recoveries if r["data"].get("origin_id") == origin["origin_id"]]
        frame_transfer_rows = [
            r for r in transfers
            if r["data"].get("frame_id") == origin["source_frame_id"]
        ]
        frame_transfers = [
            {"transfer_id": record_id(r), **r["data"]}
            for r in frame_transfer_rows
        ]
        frame_transfer_ids = {record_id(r) for r in frame_transfer_rows}
        frame_receptions = [
            d for d in reception_data
            if d.get("transfer_id") in frame_transfer_ids
        ]
        rows.append({
            "sequence": origin.get("sequence"), "origin_id": origin.get("origin_id"),
            "source_frame_id": origin.get("source_frame_id"), "created_ps": origin.get("created_ps"),
            "first_sof_ps": origin.get("first_sof_ps"), "completed_ps": origin.get("completed_ps"),
            "completed_count": origin.get("completed_count"), "target_count": origin.get("target_count"),
            "delivered_branch_id": origin.get("delivered_branch_id"),
            "branches": sorted(origin_branches, key=lambda b: (int(b.get("path_index", "0")), b.get("path_id", ""))),
            "recoveries": sorted(origin_recoveries, key=lambda r: int(r.get("sequence", "0"))),
            "hop_transfers": sorted(frame_transfers, key=lambda t: (int(t.get("queued_ps", "0")), t.get("from_port", ""), t.get("transfer_id", ""))),
            "receptions": sorted(frame_receptions, key=lambda r: (int(r.get("observed_ps", "0")), r.get("transfer_id", ""))),
        })
    terminal_statuses = {"delivered", "duplicate", "stale", "dropped", "link_down", "queue_full", "stopped"}
    incomplete = [b for b in branches if b["data"].get("status") not in terminal_statuses or b["data"].get("completed_ps") is None]
    return {
        "sequence_rows": rows,
        "origin_count": len(origins), "branch_count": len(branches), "recovery_count": len(recoveries),
        "incomplete_branches": [b["data"] for b in incomplete],
        "termination": result["simulation"].get("termination"), "partial": result["simulation"].get("partial"),
        "end_ps": result["simulation"].get("end_ps"), "last_event_time_ps": result["simulation"].get("last_event_time_ps"),
        "pending_events": result["simulation"].get("pending_events"),
        "schemas": {k: len(v) for k, v in sorted(groups.items()) if k.startswith("dir.redundancy.")},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--zip-reproduction", action="store_true", help="extract and measure the published guide input archive")
    parser.add_argument("--baseline-output", type=Path, help="saved repository-input results root (required with --zip-reproduction)")
    parser.add_argument("--publish-report", action="store_true", help="also update docs/verification/results/redundancy-history/measurements.json")
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
        extraction_root = output_root.parent / "inputs/redundancy-history"
        archive_info = extract_and_verify_archive(extraction_root)
        input_root = Path(archive_info["input_root"])
    else:
        baseline_root = None
    verified_inputs = verify_inputs(input_root)
    binary_hash = sha256(binary)
    output_root.parent.mkdir(parents=True, exist_ok=True)
    output_root.mkdir(parents=False, exist_ok=False)
    report: dict[str, Any] = {
        "status": "running", "issue": 111, "source_tag": "v1.1.6", "source_commit": EXPECTED_COMMIT,
        "source_status": "", "rustc": None, "binary_sha256": binary_hash,
        **verified_inputs, "conditions": {},
    }
    for capacity in CASES:
        name = f"capacity-{capacity}"
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
            "input_sha256": {k: v for k, v in verified_inputs["input_sha256"].items() if name in k or k.endswith("redundancy-workload.json") or k.endswith("Main.ned")},
            "provenance": {key: metadata.get(key) for key in ("git_commit", "git_dirty", "compiler", "binary_sha256", "build_source_sha256", "cargo_lock_sha256")},
            "source_sha256": {s.get("logical_path"): s.get("sha256") for s in metadata.get("sources", [])},
            "manifest": manifest,
            "summary": summary(result),
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
        measurements_path = ROOT / "docs/verification/results/redundancy-history/measurements.json"
        measurements_path.parent.mkdir(parents=True, exist_ok=True)
        if args.zip_reproduction:
            prior = load(measurements_path)
            assert prior.get("status") == "passed" and prior.get("issue") == 111, prior
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
