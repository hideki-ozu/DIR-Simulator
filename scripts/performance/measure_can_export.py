#!/usr/bin/env python3
"""Compare pinned binaries on one generated CAN input without a million-request verdict."""

import argparse
import json
import os
from pathlib import Path
import shutil
import statistics

from measure_can_million import GIB, attempt, digest, save, utc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", action="append", required=True, metavar="NAME=PATH")
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--retain-outputs", action="store_true")
    args = parser.parse_args()
    binaries = {}
    for value in args.binary:
        name, separator, path = value.partition("=")
        if not separator or not name or not name.replace("-", "").isalnum() or name in binaries:
            parser.error("Each binary needs a unique alphanumeric/hyphen NAME=PATH")
        binaries[name] = Path(path).resolve(strict=True)
    config, directory = args.config.resolve(strict=True), args.output.resolve()
    generation = json.loads((config.parent / "generation.json").read_text())
    directory.mkdir(parents=True, exist_ok=False)
    report = {
        "schema_version": 1,
        "started_at_utc": utc(),
        "qualification": "Scaled-input optimization comparison; no million-request acceptance verdict.",
        "environment": {"uname": list(os.uname()), "cpuinfo": Path("/proc/cpuinfo").read_text()},
        "config": str(config),
        "generation": generation,
        "binaries": {name: {"path": str(path), "sha256": digest(path)} for name, path in binaries.items()},
        "attempts": [],
    }
    result_path = directory / "comparison.json"
    save(result_path, report)
    for repeat in range(4):
        names = list(binaries)
        if repeat % 2:
            names.reverse()
        for name in names:
            for filename, expected in generation["input_sha256"].items():
                if digest(config.parent / filename) != expected:
                    raise RuntimeError(f"Input changed: {filename}")
            if digest(binaries[name]) != report["binaries"][name]["sha256"]:
                raise RuntimeError(f"Binary changed: {name}")
            result = attempt(binaries[name], config, directory / f"{name}-{repeat}", 26 * GIB, 180)
            result.update(binary=name, repeat=repeat, kind="warmup" if repeat == 0 else "measurement")
            report["attempts"].append(result)
            if not args.retain_outputs:
                output = Path(result["evidence_directory"]) / "output"
                if output.exists():
                    shutil.rmtree(output)
                result["large_outputs_removed_after_recording"] = True
            save(result_path, report)
            print(json.dumps({"binary": name, "repeat": repeat, "completed": result["completed"],
                              "measurements": result.get("measurements")}), flush=True)
    report["summaries"] = {}
    for name in binaries:
        rows = [row for row in report["attempts"] if row["binary"] == name and row["kind"] == "measurement"]
        complete = len(rows) == 3 and all(row["completed"] for row in rows)
        report["summaries"][name] = {
            "completed": complete,
            "median_wall_seconds": statistics.median(row["measurements"]["wall_seconds"] for row in rows) if complete else None,
            "median_event_processing_wall_seconds": statistics.median(row["event_processing_wall_seconds"] for row in rows) if complete else None,
            "maximum_rss_bytes": max(row["max_rss_bytes"] for row in rows),
        }
    complete = all(row["completed"] for row in report["attempts"])
    report["normalized_outputs_equal"] = complete and all(
        row["deterministic"] == report["attempts"][0]["deterministic"] for row in report["attempts"]
    )
    report["finished_at_utc"] = utc()
    save(result_path, report)
    print(json.dumps({"summaries": report["summaries"], "normalized_outputs_equal": report["normalized_outputs_equal"]}))
    return 0 if report["normalized_outputs_equal"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
