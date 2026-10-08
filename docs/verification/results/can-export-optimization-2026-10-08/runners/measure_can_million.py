#!/usr/bin/env python3
"""Measure the standard CLI sequentially, retaining evidence after every attempt.

GNU time measures the simulator child, excluding Python input generation and
post-run verification. A generous address-space guard protects the host; a run
that reaches the guard is incomplete and can never pass the benchmark.
"""
import argparse
import csv
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import re
import shutil
import statistics
import subprocess
import time


GIB = 1024**3


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def utc():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def save(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def available_memory():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise RuntimeError("MemAvailable unavailable")


def child_resources(parent):
    children = Path(f"/proc/{parent}/task/{parent}/children")
    try:
        pids = children.read_text().split()
    except FileNotFoundError:
        return None
    for pid in pids:
        try:
            fields = {}
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                key, value = line.split(":", 1)
                if key in ("VmRSS", "VmHWM", "VmSize"):
                    fields[key] = int(value.split()[0]) * 1024
                elif key == "Threads":
                    fields["threads"] = int(value.strip())
            return {"pid": int(pid), **fields}
        except (FileNotFoundError, ProcessLookupError):
            pass
    return None


def verify_output(output):
    manifest = json.loads((output / "manifest.json").read_text())
    entries = manifest["files"]
    if isinstance(entries, dict):
        entries = list(entries.values())
    if sorted(entry.get("name", entry.get("path")) for entry in entries) != ["diagnostics.jsonl", "events.csv", "results.json", "summary.csv"]:
        raise ValueError("Manifest must contain exactly the four standard result files")
    checks = []
    for entry in entries:
        name = entry.get("path", entry.get("name"))
        path = output / name
        checks.append({"path": name, "bytes": path.stat().st_size,
                       "sha256": digest(path), "declared": entry})
        if str(path.stat().st_size) != str(entry.get("size_bytes", entry.get("bytes"))):
            raise ValueError(f"Manifest size mismatch: {name}")
        if checks[-1]["sha256"] != entry["sha256"]:
            raise ValueError(f"Manifest hash mismatch: {name}")
    result = output / "results.json"
    h = hashlib.sha256()
    with result.open("rb") as stream:
        prefix = stream.read(1024 * 1024)
        marker = b',"schema_version":1,"simulation":'
        offset = prefix.index(marker) + len(marker)
        stream.seek(offset)
        remaining = result.stat().st_size - offset - 2  # final root '}' and LF
        while remaining:
            block = stream.read(min(1024 * 1024, remaining))
            if not block:
                raise RuntimeError("truncated results.json")
            h.update(block)
            remaining -= len(block)
    deterministic = {"simulation_sha256": h.hexdigest()}
    for name in ("events.csv", "summary.csv"):
        h = hashlib.sha256()
        with (output / name).open("rb") as stream:
            h.update(next(stream))
            for line in stream:
                schema, _, rest = line.split(b",", 2)
                h.update(schema + b",<run_id>," + rest)
        deterministic[name + "_normalized_sha256"] = h.hexdigest()
    summary = []
    with (output / "summary.csv").open(newline="") as stream:
        for row in csv.DictReader(stream):
            if row["target"] == "$all" or row["metric"] == "bus_utilization":
                summary.append(row)
    return {"manifest_verified": True, "files": checks,
            "output_bytes": sum(p.stat().st_size for p in output.iterdir() if p.is_file()),
            "deterministic": deterministic, "selected_summary": summary}


def attempt(binary, config, directory, address_limit, wall_limit_seconds=None):
    if wall_limit_seconds is not None and (not math.isfinite(wall_limit_seconds) or wall_limit_seconds <= 120):
        raise ValueError("Wall observation limit must be finite and exceed the 120-second target")
    directory.mkdir()
    output = directory / "output"
    time_path = directory / "gnu-time.txt"
    stdout = directory / "stdout.json"
    stderr = directory / "stderr.txt"
    command = ["/usr/bin/time", "-o", str(time_path), "-f",
               '{"wall_seconds":%e,"max_rss_kib":%M,"user_seconds":%U,"system_seconds":%S,"exit_status":%x}',
               str(binary), "run", "--config", str(config), "--output", str(output)]

    def guard():
        resource.setrlimit(resource.RLIMIT_AS, (address_limit, address_limit))
        # Linux pipe-based core handlers ignore a limit of zero. A limit of one
        # suppresses that handler, including WSL's lengthy crash-capture pipe.
        resource.setrlimit(resource.RLIMIT_CORE, (1, 1))

    started = utc()
    start = time.monotonic()
    reason = None
    observed_thread_counts = set()
    save(directory / "attempt-started.json", {"started_at_utc": started, "command": command,
         "address_space_limit_bytes": address_limit, "status": "started"})
    with stdout.open("w") as out, stderr.open("w") as err, (directory / "samples.jsonl").open("w") as samples:
        process = subprocess.Popen(command, stdout=out, stderr=err,
                                   env={**os.environ, "LC_ALL": "C"}, preexec_fn=guard)
        last_notice = -30.0
        try:
            while process.poll() is None:
                elapsed = time.monotonic() - start
                resources = child_resources(process.pid)
                if resources and "threads" in resources:
                    observed_thread_counts.add(resources["threads"])
                sample = {"elapsed_seconds": round(elapsed, 3), "available_memory_bytes": available_memory(),
                          "child": resources}
                samples.write(json.dumps(sample) + "\n")
                samples.flush()
                if reason is None and resources and (resources.get("VmRSS", 0) > 24 * GIB or sample["available_memory_bytes"] < 3 * GIB):
                    reason = "host_memory_guard"
                    try:
                        os.kill(resources["pid"], 15)
                    except ProcessLookupError:
                        pass
                if reason is None and resources and wall_limit_seconds is not None and elapsed >= wall_limit_seconds:
                    reason = "external_wall_observation_limit"
                    save(directory / "wall-limit-stop.json", {"reason": reason,
                         "limit_seconds": wall_limit_seconds, "timestamp_utc": utc(),
                         "sample": sample, "signal": 15})
                    try:
                        os.kill(resources["pid"], 15)
                    except ProcessLookupError:
                        pass
                if elapsed - last_notice >= 30:
                    print(json.dumps({"attempt": directory.name, **sample}), flush=True)
                    last_notice = elapsed
                time.sleep(0.25)
        except BaseException:
            resources = child_resources(process.pid)
            if resources:
                try:
                    os.kill(resources["pid"], 15)
                except ProcessLookupError:
                    pass
            process.wait()
            raise
        returncode = process.wait()
    raw_time = time_path.read_text()
    time_records = [json.loads(line) for line in raw_time.splitlines() if line.startswith("{")]
    if not time_records:
        failure = {"started_at_utc": started, "finished_at_utc": utc(), "returncode": returncode,
                   "completed": False, "measurement_error": "GNU time measurement record missing"}
        save(directory / "attempt.json", failure)
        raise RuntimeError(f"GNU time measurement record missing: {directory}")
    measured = time_records[-1]
    signal_match = re.search(r"Command terminated by signal (\d+)", raw_time)
    measured["termination_signal"] = int(signal_match.group(1)) if signal_match else None
    report = {"started_at_utc": started, "finished_at_utc": utc(), "command": command,
              "returncode": returncode, "measurements": measured,
              "max_rss_bytes": measured["max_rss_kib"] * 1024,
              "event_processing_wall_seconds": None,
              "observed_thread_counts": sorted(observed_thread_counts),
              "guard_reason": reason, "evidence_directory": str(directory)}
    if stdout.stat().st_size:
        try:
            report["run_report"] = json.loads(stdout.read_text())
        except json.JSONDecodeError:
            report["stdout_unparsed"] = stdout.read_text()[-4000:]
    report["stderr_tail"] = stderr.read_text()[-4000:]
    if "memory allocation of" in report["stderr_tail"] and "failed" in report["stderr_tail"]:
        report["guard_reason"] = "allocation_failure_with_address_space_guard"
    if report["guard_reason"] is None and measured["termination_signal"] is None:
        run_report = report.get("run_report")
        if isinstance(run_report, dict) and run_report.get("exit_code") == returncode:
            seconds = run_report.get("event_processing_wall_seconds")
            if isinstance(seconds, (int, float)) and not isinstance(seconds, bool) and math.isfinite(seconds) and seconds >= 0:
                report["event_processing_wall_seconds"] = seconds
    if report["event_processing_wall_seconds"] is None:
        report["event_processing_wall_note"] = "CLI timing unavailable or process aborted."
    report["completed"] = returncode == 0 and (output / "manifest.json").exists()
    save(directory / "attempt.json", report)
    if report["completed"]:
        try:
            report.update(verify_output(output))
        except (AssertionError, KeyError, ValueError, OSError, RuntimeError) as error:
            report["verification_error"] = repr(error)
            report["completed"] = False
    else:
        report["unpublished_files"] = [{"path": str(p.relative_to(output)), "bytes": p.stat().st_size}
                                        for p in output.rglob("*") if p.is_file()] if output.exists() else []
    save(directory / "attempt.json", report)
    return report


def evaluate(attempts):
    measured = [a for a in attempts if a["kind"] == "measurement"]
    complete = len(measured) == 3 and all(a["completed"] for a in measured)
    max_rss = max((a["max_rss_bytes"] for a in measured), default=None)
    deterministic = all(a["deterministic"] == measured[0]["deterministic"] for a in measured) if complete else None
    return {"complete_measurement_count": sum(a["completed"] for a in measured),
            "median_completion_wall_seconds": statistics.median(a["measurements"]["wall_seconds"] for a in measured) if complete else None,
            "maximum_observed_rss_bytes": max_rss,
            "median_attempt_wall_seconds": statistics.median(a["measurements"]["wall_seconds"] for a in measured) if measured else None,
            "performance_target_passed": statistics.median(a["measurements"]["wall_seconds"] for a in measured) <= 120 if complete else None,
            "memory_target_passed": False if max_rss is not None and max_rss > 2 * GIB else (True if complete else None),
            "deterministic_results_equal": deterministic,
            "verdict": "passed" if complete and deterministic and max_rss <= 2 * GIB and statistics.median(a["measurements"]["wall_seconds"] for a in measured) <= 120 else "failed"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--address-limit-gib", type=int, default=26)
    parser.add_argument("--retain-outputs", action="store_true")
    parser.add_argument("--wall-limit-seconds", type=float,
                        help="Optional observation limit above 120 seconds; interrupted runs are incomplete")
    args = parser.parse_args()
    if args.wall_limit_seconds is not None and (not math.isfinite(args.wall_limit_seconds) or args.wall_limit_seconds <= 120):
        parser.error("--wall-limit-seconds must be finite and exceed 120")
    binary, inputs, root = args.binary.resolve(), args.inputs.resolve(), args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    report_path = root / "measurement.json"
    if report_path.exists():
        raise SystemExit("Choose a fresh output directory; existing evidence is preserved.")
    report = {"schema_version": 1, "started_at_utc": utc(),
              "specification": "docs/品質・配布方針.md section 1",
              "series": "WSL2 separate environment", "binary_sha256": digest(binary),
              "cargo_lock_sha256": digest(Path("Cargo.lock")),
              "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "git_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], text=True)),
              "environment": {"uname": list(os.uname()), "cpuinfo": Path("/proc/cpuinfo").read_text(),
                              "os_release": Path("/etc/os-release").read_text(),
                              "meminfo": Path("/proc/meminfo").read_text(),
                              "rustc": subprocess.check_output(["/home/hideki/.cargo/bin/rustc", "-Vv"], text=True)},
              "resource_guard": {"address_space_limit_bytes": args.address_limit_gib * GIB,
                                 "rss_stop_bytes": 24 * GIB, "minimum_available_bytes": 3 * GIB,
                                 "core_file_limit_bytes": 1,
                                 "wall_observation_limit_seconds": args.wall_limit_seconds,
                                 "note": "Incomplete guarded/aborted runs do not establish unconstrained peak RSS or completion time."},
              "targets": {"wall_seconds": 120, "max_rss_bytes": 2 * GIB},
              "generation": json.loads((inputs / "generation.json").read_text()), "conditions": []}
    save(report_path, report)
    configs = sorted(inputs.glob("*.ini"))
    if len(configs) != 3:
        raise SystemExit("Exactly three generated INI conditions required")
    generation = report["generation"]
    if generation["request_count_per_configuration"] != 1_000_000:
        raise SystemExit("Million-request verdicts require exactly 1,000,000 requests")
    if {s["ini"] for s in generation["scenarios"]} != {p.name for p in configs}:
        raise SystemExit("Generated scenario/configuration mapping differs")

    def check_pinned_inputs():
        if digest(binary) != report["binary_sha256"]:
            raise RuntimeError("Pinned binary changed during benchmark")
        for name, expected in generation["input_sha256"].items():
            if digest(inputs / name) != expected:
                raise RuntimeError(f"Pinned input hash mismatch: {name}")
        for scenario in generation["scenarios"]:
            workload = json.loads((inputs / scenario["workload"]).read_text())
            if len(workload["generators"]) != 32 or sum(g["count"] for g in workload["generators"]) != 1_000_000:
                raise RuntimeError("Workload must contain 32 generators and 1,000,000 requests")

    check_pinned_inputs()
    for config in configs:
        condition = {"config": str(config), "input_sha256": digest(config), "attempts": []}
        report["conditions"].append(condition)
        for repeat in range(4):
            check_pinned_inputs()
            result = attempt(binary, config, root / f"{config.stem}-{repeat}", args.address_limit_gib * GIB,
                             wall_limit_seconds=args.wall_limit_seconds)
            if result["completed"]:
                generated = [r for r in result["selected_summary"] if r["target"] == "$all" and r["metric"] == "generated"]
                if len(generated) != 1 or generated[0]["value"] != "1000000":
                    result["completed"] = False
                    result["verification_error"] = "Published generated-request count differs from 1,000,000"
            result["kind"] = "warmup" if repeat == 0 else "measurement"
            result["repeat"] = repeat
            condition["attempts"].append(result)
            condition["evaluation"] = evaluate(condition["attempts"])
            save(report_path, report)
            print(json.dumps({"finished": config.stem, "repeat": repeat,
                              "completed": result["completed"], "measurements": result["measurements"]}), flush=True)
            if not args.retain_outputs:
                output = Path(result["evidence_directory"]) / "output"
                if output.exists():
                    shutil.rmtree(output)  # Only outputs created by this attempt.
                result["large_outputs_removed_after_recording"] = True
                save(report_path, report)
    report["finished_at_utc"] = utc()
    save(report_path, report)


if __name__ == "__main__":
    main()
