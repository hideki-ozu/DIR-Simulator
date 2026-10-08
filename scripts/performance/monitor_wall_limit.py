#!/usr/bin/env python3
"""Stop only this benchmark's child after an explicitly recorded wall limit.

Use alongside measure_can_million.py when an incomplete run must be bounded.
The limit must exceed the 120-second target. The resulting completion time is a
lower bound, not a measured completion duration.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import signal
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--measurement-root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--limit-seconds", type=float, default=180)
    args = parser.parse_args()
    if args.limit_seconds <= 120:
        parser.error("The observation limit must exceed the 120-second target")
    root = args.measurement_root.resolve()
    binary = args.binary.resolve()
    while True:
        report_path = root / "measurement.json"
        if report_path.exists():
            report = json.loads(report_path.read_text())
            if "finished_at_utc" in report:
                break
        for samples in root.glob("rho-*/samples.jsonl"):
            marker = samples.parent / "wall-limit-stop.json"
            if marker.exists() or (samples.parent / "attempt.json").exists():
                continue
            lines = samples.read_text().splitlines()
            sample = None
            for line in reversed(lines[-3:]):
                try:
                    sample = json.loads(line)
                    break
                except json.JSONDecodeError:
                    pass
            if not sample or sample["elapsed_seconds"] < args.limit_seconds or not sample.get("child"):
                continue
            pid = sample["child"]["pid"]
            try:
                command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
                expected_output = str(samples.parent / "output").encode()
                if command[0] != str(binary).encode() or expected_output not in command:
                    raise RuntimeError("Refusing to signal a process outside this benchmark")
                record = {"reason": "external_wall_observation_limit", "limit_seconds": args.limit_seconds,
                          "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                          "sample": sample, "signal": signal.SIGTERM.value}
                marker.write_text(json.dumps(record, indent=2) + "\n")
                os.kill(pid, signal.SIGTERM)
                print(json.dumps({"stopped_attempt": samples.parent.name, **record}), flush=True)
            except (FileNotFoundError, ProcessLookupError):
                pass
        time.sleep(1)


if __name__ == "__main__":
    main()
