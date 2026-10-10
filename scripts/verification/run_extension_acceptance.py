"""Run twelve exact regressions on existing WSL Ubuntu 24.04 x86_64; preserve logs.

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
TESTS = [("--test", target, name) for target, name in TESTS] + [
    ("--test", "registry", "generic_channel_factories_receive_frozen_inputs_in_stable_order"),
    ("--test", "registry", "generic_invalid_channel_parameter_never_reaches_factories"),
    ("--test", "registry", "generic_channel_factory_failure_releases_only_constructed_prefix"),
    ("--lib", None, "cli_runtime_boundary_tests::cli_validate_does_not_enter_runtime_and_run_positive_control_does"),
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
              "scope": "Twelve named regressions including four bounded additions; no full specification/AC0008, channel conformance or adoption approval"}
    for relative in ("Cargo.toml", "Cargo.lock", "crates/dir-simulator/tests/ac0008_paths.rs",
                     "crates/dir-simulator/tests/registry.rs", "crates/dir-simulator/src/lib.rs",
                     "crates/dir-simulator/src/runtime.rs", "crates/dir-simulator/src/runtime/engine.rs",
                     "crates/dir-simulator/src/main.rs", "crates/dir-simulator/src/cli_runtime_boundary_tests.rs"):
        data = (root / relative).read_bytes()
        report["source_inputs"].append({"path": relative, "bytes": len(data),
                                       "sha256": hashlib.sha256(data).hexdigest()})
    code = 2
    if args.plan:
        report["reason"] = "Requested plan only"
    elif not shutil.which("cargo"):
        report["reason"] = "cargo unavailable; no installation attempted"
    else:
        preflight = []
        for argv in (["uname", "-a"], ["cat", "/etc/os-release"], ["rustc", "-Vv"],
                     ["cargo", "--version"], ["git", "rev-parse", "HEAD"], ["git", "status", "--short"]):
            try:
                result = subprocess.run(argv, cwd=root, capture_output=True, text=True)
                preflight.append({"argv": argv, "exit_code": result.returncode,
                                  "stdout": result.stdout, "stderr": result.stderr})
            except OSError as error:
                preflight.append({"argv": argv, "exit_code": None, "error": str(error)})
        report["environment_preflight"] = preflight
        uname = preflight[0].get("stdout", "").lower()
        os_release = preflight[1].get("stdout", "")
        target_environment = (all(item["exit_code"] == 0 for item in preflight)
                              and "linux" in uname and "microsoft" in uname and "x86_64" in uname
                              and 'ID=ubuntu' in os_release and 'VERSION_ID="24.04"' in os_release
                              and "host: x86_64-unknown-linux-gnu" in preflight[2].get("stdout", ""))
        version = subprocess.run(["cargo", "--version"], cwd=root, capture_output=True, text=True)
        report["cargo"] = version.stdout.strip()
        if not target_environment:
            report["reason"] = "Required existing WSL Ubuntu 24.04 x86_64 environment not established; no setup changes attempted"
        elif version.returncode or not version.stdout.startswith("cargo 1.85.0 "):
            report["reason"] = "Existing cargo must be 1.85.0; no toolchain modification attempted"
        else:
            for kind, target, name in TESTS:
                argv = ["cargo", "test", "--locked", "-p", "dir-simulator", kind]
                if target:
                    argv.append(target)
                argv += [name, "--", "--exact", "--nocapture"]
                completed = subprocess.run(argv, cwd=root, capture_output=True)
                item = {"kind": kind, "target": target, "named_test": name, "argv": argv,
                        "exit_code": completed.returncode, "streams": {}}
                for stream, data in (("stdout", completed.stdout), ("stderr", completed.stderr)):
                    filename = name.replace("::", "__") + "." + stream + ".log"
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
            if code == 0:
                for index, argv in enumerate((
                    ["cargo", "fmt", "--all", "--", "--check"],
                    ["cargo", "test", "--locked", "--workspace"],
                    ["cargo", "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"],
                )):
                    result = subprocess.run(argv, cwd=root, capture_output=True)
                    item = {"argv": argv, "exit_code": result.returncode, "streams": {}}
                    for stream, data in (("stdout", result.stdout), ("stderr", result.stderr)):
                        filename = f"required-{index}.{stream}.log"
                        (output / filename).write_bytes(data)
                        item["streams"][stream] = {"path": filename, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                    report.setdefault("required_checks", []).append(item)
                    if result.returncode:
                        report["status"] = "failed"
                        code = 1
                        break
    report["planned_tests"] = [{"kind": k, "target": t, "named_test": n} for k, t, n in TESTS]
    report["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    (output / "execution.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n",
                                          encoding="utf-8", newline="\n")
    print(json.dumps({"status": report["status"], "executed": len(report["commands"]),
                      "record": str(output / "execution.json")}))
    return code


if __name__ == "__main__":
    sys.exit(main())
