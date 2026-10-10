#!/usr/bin/env python3
"""Run independent public-operation acceptance assertions (DIR-TEST-0080..0083)."""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(path):
    return json.loads(path.read_text())


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    base = args.output.resolve()
    base.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[2]
    source = root / "docs/verification/fixtures/can"
    inputs = base / "inputs"
    inputs.mkdir()
    shutil.copytree(source / "models", inputs / "models")
    workload = load(source / "delay-filter.json")
    write(inputs / "workload.json", workload)
    two = json.loads(json.dumps(workload))
    two["generators"][0]["times"] = ["0ps", "150us"]
    write(inputs / "two.json", two)
    original = (source / "delay-filter.ini").read_text().replace('"delay-filter.json"', '"workload.json"')
    variants = {
        "A": (original, [3_000_000], [103_000_000], [109_000_000], [115_000_000]),
        "B": (original.replace("Main.a.txProcessingDelay = 3us", "Main.a.txProcessingDelay = 5us"), [5_000_000], [105_000_000], [111_000_000], [117_000_000]),
        "C": (original.replace("500kbps", "250kbps"), [3_000_000], [203_000_000], [215_000_000], [215_000_000]),
        "D": (original + "\n", [], [], [], []),
        "E": (original.replace('"workload.json"', '"two.json"'), [3_000_000, 153_000_000], [103_000_000, 253_000_000], [109_000_000, 259_000_000], [115_000_000, 265_000_000]),
    }
    variants["D"] = (original.replace('workload = "workload.json"', 'workload = "workload.json"\nMain.a.queueCapacity = 0'), [], [], [], [])
    commands = []
    cases = []

    def command(argv, expected=0):
        completed = subprocess.run([str(binary), *map(str, argv)], capture_output=True, text=True, timeout=90)
        row = {"argv": [str(binary), *map(str, argv)], "exit_code": completed.returncode, "stdout": completed.stdout, "stderr": completed.stderr}
        commands.append(row)
        assert completed.returncode == expected, row
        return json.loads(completed.stdout) if completed.stdout.strip() else None

    def published(directory):
        manifest = load(directory / "manifest.json")
        assert {f["name"] for f in manifest["files"]} == {"results.json", "events.csv", "summary.csv", "diagnostics.jsonl"}
        for entry in manifest["files"]:
            path = directory / entry["name"]
            assert sha(path) == entry["sha256"]
            assert path.stat().st_size == int(entry["bytes"])
        result = load(directory / "results.json")
        assert manifest["run_id"] == result["run_id"]
        assert manifest["termination"] == result["simulation"]["termination"]
        assert manifest["partial"] == result["simulation"]["partial"]
        return result

    def csv_rows(directory, name):
        with (directory / name).open(newline="") as stream:
            return [{k: v for k, v in row.items() if k != "run_id"} for row in csv.DictReader(stream)]

    for name, (ini, sofs, eofs, releases, received) in variants.items():
        config = inputs / f"{name}.ini"
        config.write_text(ini)
        command(["validate", "--config", config])
        destination = base / name
        operation = command(["run", "--config", config, "--output", destination])
        result = published(destination)
        sim = result["simulation"]
        assert operation["exit_code"] == 0 and operation["partial"] is False
        assert sim["termination"] == "events_exhausted" and sim["end_ps"] == "300000000"
        requests = sim["requests"]
        sent = [r for r in requests if r["sof_ps"] is not None]
        assert [int(r["sof_ps"]) for r in sent] == sofs
        assert [int(r["eof_ps"]) for r in sent] == eofs
        assert [int(r["model_fields"]["release_ps"]) for r in sent] == releases
        got = [r for r in sim["receivers"] if r["status"] == "received"]
        assert [int(r["received_ps"]) for r in got] == received
        assert all(r["receiver"] == "Main.b" for r in got)
        filtered = [r for r in sim["receivers"] if r["status"] == "filtered"]
        assert len(filtered) == len(sofs) and all(r["receiver"] == "Main.c" for r in filtered)
        if name == "D":
            assert len(requests) == 1 and requests[0]["status"] == "dropped" and requests[0]["drop_reason"] == "queue_full"
            assert not sim["receivers"]
        assert not (destination / "diagnostics.jsonl").read_text().strip()
        command(["view", "--input", destination / "results.json", "--output", destination / "viewer.html"])
        cases.append({"case_id": "DIR-TEST-0080", "subcase": name, "status": "passed", "config": str(config), "config_sha256": sha(config), "output": str(destination), "expected": {"sof_ps": sofs, "eof_ps": eofs, "release_ps": releases, "received_ps": received}, "actual": {"requests": requests, "receivers": sim["receivers"]}})

    # Verify overwrite refusal preserves every published artifact.
    preserved = {p.name: sha(p) for p in (base / "A").iterdir() if p.is_file()}
    command(["run", "--config", inputs / "A.ini", "--output", base / "A"], expected=4)
    assert preserved == {p.name: sha(p) for p in (base / "A").iterdir() if p.is_file()}
    cases.append({"case_id": "DIR-TEST-0081", "subcase": "existing-output-preserved", "status": "passed"})

    for name, replacement in [("missing-workload", 'workload = "absent.json"'), ("negative-time", "sim-time-limit = -1ps")]:
        config = inputs / f"{name}.ini"
        key = "workload" if name == "missing-workload" else "sim-time-limit"
        lines = [replacement if line.startswith(key + " =") else line for line in original.splitlines()]
        config.write_text("\n".join(lines) + "\n")
        command(["validate", "--config", config], expected=2)
        destination = base / name
        operation = command(["run", "--config", config, "--output", destination], expected=2)
        result = published(destination)
        assert operation["termination"] == "prep_failed"
        assert result["simulation"]["committed_events"] == "0"
        assert not result["simulation"]["requests"] and not result["simulation"]["receivers"]
        cases.append({"case_id": "DIR-TEST-0081", "subcase": name, "status": "passed", "output": str(destination)})
    command(["run", "--unknown-acceptance-option"], expected=2)
    cases.append({"case_id": "DIR-TEST-0081", "subcase": "unknown-option", "status": "passed"})

    for time in [0, 3_000_000, 103_000_000, 105_000_000, 108_000_000, 109_000_000, 115_000_000]:
        for delta in [-1, 0, 1]:
            limit = time + delta
            if limit < 0:
                continue
            config = inputs / f"boundary-{limit}.ini"
            config.write_text(original.replace("300us", f"{limit}ps"))
            destination = base / f"boundary-{limit}"
            operation = command(["run", "--config", config, "--output", destination])
            result = published(destination)
            sim = result["simulation"]
            assert operation["partial"] is False and sim["partial"] is False
            if limit == 0:
                assert not sim["requests"] and not sim["receivers"]
            else:
                request = sim["requests"][0]
                assert (request["sof_ps"] is not None) == (limit > 3_000_000)
                assert (request["eof_ps"] is not None) == (limit > 103_000_000)
                assert (request["model_fields"]["release_ps"] is not None) == (limit > 109_000_000)
                completed = [r for r in sim["receivers"] if r["received_ps"] is not None]
                assert len(completed) == (1 if limit > 115_000_000 else 0)
            cases.append({"case_id": "DIR-TEST-0081", "subcase": f"half-open-boundary-{limit}", "status": "passed", "output": str(destination)})

    for name in ["competition", "release-arrival"]:
        previous = None
        for repeat in range(3):
            destination = base / f"repeat-{name}-{repeat}"
            command(["run", "--config", source / f"{name}.ini", "--output", destination])
            result = published(destination)
            signature = [result["simulation"], csv_rows(destination, "events.csv"), csv_rows(destination, "summary.csv")]
            if previous is not None:
                assert signature == previous
            previous = signature
        sent = sorted((r for r in previous[0]["requests"] if r["sof_ps"] is not None), key=lambda r: int(r["sof_ps"]))
        expected_ids = ["a:0", "b:0"] if name == "competition" else ["b:0", "a:0", "b:1"]
        expected_times = [0, 106_000_000] if name == "competition" else [0, 100_000_000, 206_000_000]
        assert [r["request_id"] for r in sent] == expected_ids
        assert [int(r["sof_ps"]) for r in sent] == expected_times
        cases.append({"case_id": "DIR-TEST-0083", "subcase": name + "-three-repetitions", "status": "passed"})
        reversed_workload = load(source / f"{name}.json")
        reversed_workload["generators"].reverse()
        workload_path = inputs / f"reversed-{name}.json"
        write(workload_path, reversed_workload)
        config = inputs / f"reversed-{name}.ini"
        config.write_text((source / f"{name}.ini").read_text().replace(f'"{name}.json"', f'"reversed-{name}.json"'))
        destination = base / f"reversed-{name}"
        command(["run", "--config", config, "--output", destination])
        result = published(destination)
        assert [result["simulation"], csv_rows(destination, "events.csv"), csv_rows(destination, "summary.csv")] == previous
        cases.append({"case_id": "DIR-TEST-0083", "subcase": name + "-generator-permutation", "status": "passed", "output": str(destination)})

    write(base / "report.json", {"status": "passed", "binary": str(binary), "binary_sha256": sha(binary), "cases": cases, "commands": commands, "limits": ["Lifecycle allocation/finish fault injection is exercised separately by registered-engine tests", "Current verification scope is Ubuntu 24.04 LTS x86_64 on WSL; native Ubuntu comparison and Windows native verification are outside current requirements", "Cases 0082 and remaining 0083 scheduler/permutation assertions are supplied by Rust registered-engine tests"]})
    print(json.dumps({"status": "passed", "cases": len(cases), "commands": len(commands), "report": str(base / "report.json")}))


if __name__ == "__main__":
    main()
