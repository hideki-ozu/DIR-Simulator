"""Run eight exact acceptance regressions in a normal checkout; preserve logs.

From the repository root: python scripts/verification/run_extension_acceptance.py
    --output /absolute/new/output
No checkout, toolchain install, configuration change or remote mutation occurs.
"""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

TESTS = [
    ("ac0008_paths", "classical_can_explicit_run_config_preserves_plain_projection"),
    ("ac0008_paths", "classical_can_cli_validate_accepts_fixture_and_rejects_missing_config"),
    ("registry", "generic_prepare_defers_model_factories_until_runtime"),
    ("registry", "explicit_run_config_selects_custom_registry_instead_of_default"),
    ("registry", "acceptance_lifecycle_failures_release_models_and_keep_frozen_prefix"),
    ("registry", "initialization_error_discards_effects_and_finishes_successful_initializations"),
    ("registry", "custom_payload_channel_timer_cancel_and_arbitration_execute_deterministically"),
    ("registry", "callback_failure_discards_every_effect_and_preserves_pending_current"),
]


def exact_pass(stdout, name):
    return bool(re.search(r"(?m)^test " + re.escape(name) + r" \.\.\. ok\s*$", stdout)) and bool(
        re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", stdout)
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--plan", action="store_true", help="Save not_run plan without executing tools")
    args = parser.parse_args()
    root = args.root.resolve()
    output = args.output.resolve()
    if not (root / "Cargo.toml").is_file():
        parser.error("root must be a DIR Simulator checkout")
    output.mkdir(parents=True, exist_ok=False)
    report = {"schema_version": 1, "status": "not_run", "source_inputs": [], "commands": [],
              "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "scope": "Eight named acceptance regressions; no universal conformance or license approval"}
    for relative in ("Cargo.toml", "Cargo.lock", "crates/dir-simulator/tests/ac0008_paths.rs",
                     "crates/dir-simulator/tests/registry.rs"):
        data = (root / relative).read_bytes()
        report["source_inputs"].append({"path": relative, "bytes": len(data),
                                       "sha256": hashlib.sha256(data).hexdigest()})
    code = 2
    if args.plan:
        report["reason"] = "Requested plan only"
    elif not shutil.which("cargo"):
        report["reason"] = "cargo unavailable; no installation attempted"
    else:
        version = subprocess.run(["cargo", "--version"], cwd=root, capture_output=True, text=True)
        report["cargo"] = version.stdout.strip()
        if version.returncode or not version.stdout.startswith("cargo 1.85.0 "):
            report["reason"] = "Existing cargo must be 1.85.0; no toolchain modification attempted"
        else:
            for target, name in TESTS:
                argv = ["cargo", "test", "--locked", "-p", "dir-simulator", "--test", target,
                        name, "--", "--exact", "--nocapture"]
                completed = subprocess.run(argv, cwd=root, capture_output=True)
                item = {"target": target, "named_test": name, "argv": argv,
                        "exit_code": completed.returncode, "streams": {}}
                for stream, data in (("stdout", completed.stdout), ("stderr", completed.stderr)):
                    filename = name + "." + stream + ".log"
                    (output / filename).write_bytes(data)
                    item["streams"][stream] = {"path": filename, "bytes": len(data),
                                               "sha256": hashlib.sha256(data).hexdigest()}
                item["status"] = "passed_limited" if completed.returncode == 0 and exact_pass(
                    completed.stdout.decode("utf-8", errors="replace"), name) else "failed"
                report["commands"].append(item)
                if item["status"] == "failed":
                    break
            code = 0 if len(report["commands"]) == len(TESTS) and all(
                item["status"] == "passed_limited" for item in report["commands"]) else 1
            report["status"] = "passed_limited" if code == 0 else "failed"
    report["planned_tests"] = [{"target": t, "named_test": n} for t, n in TESTS]
    report["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    (output / "execution.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n",
                                          encoding="utf-8", newline="\n")
    print(json.dumps({"status": report["status"], "executed": len(report["commands"]),
                      "record": str(output / "execution.json")}))
    return code


if __name__ == "__main__":
    sys.exit(main())

