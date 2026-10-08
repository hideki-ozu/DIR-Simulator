#!/usr/bin/env python3
"""Compare public output event prefixes when only the simulation horizon changes."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[2]
    cases = []
    for name, relative, horizons in [("bridge", "examples/can-ethernet/r01.ini", (150_000_000, 1_000_000_000)), ("axi", "examples/axi/read-write.ini", (50_000, 100_000))]:
        base = root / relative
        original = base.read_text()
        workload_name = re.search(r'^workload\s*=\s*"([^"]+)"', original, re.M)[1]
        workload = json.loads((base.parent / workload_name).read_text())
        if name == "bridge":
            generator = workload["can"]["generators"][0]
            generator.pop("times", None)
            generator.update(kind="can.periodic.v1", start="0ps", period="200us", end="201us")
        else:
            workload["generators"][0]["times"] = ["0ps", "75000ps"]
        workload_path = output / f"{name}-workload.json"
        workload_path.write_text(json.dumps(workload, indent=2) + "\n")
        for key in ("ned-path", "model-config"):
            match = re.search(rf'^{key}\s*=\s*"([^"]+)"', original, re.M)
            original = re.sub(rf'^{key}\s*=.*$', f'{key} = "{(base.parent / match[1]).resolve()}"', original, flags=re.M)
        original = re.sub(r'^workload\s*=.*$', f'workload = "{workload_path}"', original, flags=re.M)
        runs, points = [], []
        for horizon in horizons:
            config = output / f"{name}-{horizon}.ini"
            config.write_text(re.sub(r'^sim-time-limit\s*=.*$', f"sim-time-limit = {horizon}ps", original, flags=re.M))
            directory = output / f"{name}-{horizon}"
            argv = [str(binary), "run", "--config", str(config), "--output", str(directory)]
            executed = subprocess.run(argv, capture_output=True, text=True, timeout=90)
            assert executed.returncode == 0, executed.stderr
            result = json.loads((directory / "results.json").read_text())
            manifest = json.loads((directory / "manifest.json").read_text())
            for entry in manifest["files"]:
                path = directory / entry["name"]
                assert digest(path) == entry["sha256"] and path.stat().st_size == int(entry["bytes"])
            points.append([row for row in result["simulation"]["records"] if row["time_ps"] is not None and row["event_seq"] is not None and int(row["time_ps"]) < horizons[0]])
            runs.append({"argv": argv, "exit_code": executed.returncode, "stdout": executed.stdout, "stderr": executed.stderr, "config_sha256": digest(config), "results_sha256": digest(directory / "results.json"), "manifest_sha256": digest(directory / "manifest.json"), "pending_events": result["simulation"]["pending_events"]})
        differences = [{"index": i, "short": a, "full": b} for i, (a, b) in enumerate(zip(*points)) if a != b]
        cases.append({"name": name, "case_id": "DIR-TEST-0108" if name == "bridge" else "DIR-TEST-0022", "status": "passed" if points[0] == points[1] else "failed", "expected": "Ordered committed rows before the shorter horizon, including event_seq/effect_seq, must be exactly equal", "point_counts": list(map(len, points)), "different_row_count": len(differences), "differences": differences, "runs": runs, "workload_sha256": digest(workload_path)})
    report = {"product_execution": True, "binary": str(binary), "binary_sha256": digest(binary), "status": "passed" if all(case["status"] == "passed" for case in cases) else "failed", "cases": cases}
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "cases": [{"name": case["name"], "status": case["status"], "different_rows": case["different_row_count"]} for case in cases], "report": str(output / "report.json")}))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
