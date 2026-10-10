"""Check immutable Issue44 source and published regression evidence."""
import hashlib
import json
import re
import subprocess
from pathlib import Path

PACKET = Path(__file__).resolve().parent
REPOSITORY = PACKET.parents[3]


def sha(content):
    return hashlib.sha256(content).hexdigest()


def read(name):
    return json.loads((PACKET / name).read_text())


def main():
    manifest = read("evidence-manifest.json")
    files = {row["path"]: row for row in manifest["files"]}
    for name, row in files.items():
        content = (PACKET / name).read_bytes()
        assert sha(content) == row["published_sha256"] and len(content) == row["published_bytes"], name
    checksums = {}
    for line in (PACKET / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ", 1)
        candidate = PACKET / name
        assert name not in checksums and candidate.resolve().is_relative_to(PACKET.resolve())
        assert not candidate.is_symlink() and sha(candidate.read_bytes()) == digest, name
        checksums[name] = digest
    expected = {path.relative_to(PACKET).as_posix() for path in PACKET.rglob("*")
                if path.is_file() and path.name != "SHA256SUMS" and "__pycache__" not in path.parts}
    assert set(checksums) == expected
    run = read("run-result.json")
    head = run["source_commit"]
    assert run["status"] == "passed" and run["test_commit"] == manifest["tested_source_commit"] == head
    counts = {}
    for command in run["commands"]:
        assert command["exit_code"] == 0
        for stream in ["stdout", "stderr"]:
            content = (PACKET / command[stream]).read_bytes()
            assert sha(content) == command[stream + "_sha256"]
            assert files[command[stream]]["original_sha256"] == command["original_" + stream + "_sha256"]
        if command["named_tests"]:
            log = (PACKET / command["stdout"]).read_text()
            observed = sum(int(n) for n in re.findall(r"test result: ok\. (\d+) passed;", log))
            assert observed == len(command["named_tests"])
            assert all(row["outcome"] == "ok" for row in command["named_tests"])
            counts[command["name"]] = observed
    assert counts == {"focused-tests": 32, "input-unit-tests": 45, "diagnostic-regressions": 23}
    source = read("source-inventory.json")
    assert source["source_commit"] == head
    for row in source["files"]:
        content = subprocess.check_output(["git", "show", head + ":" + row["path"]], cwd=REPOSITORY)
        assert sha(content) == row["sha256"] and len(content) == row["bytes"]
    binding = read("runtime-bindings.json")
    assert binding["source_commit"] == binding["test_commit"] == head
    normative = [row for row in binding["bindings"] if row["issue44_normative_case"]]
    assert len(normative) == 3 and len(binding["bindings"]) == 24
    for row in normative:
        observation = row["observed"]
        assert observation["diagnostic"]["reason"] == "unsupported_syntax"
        assert observation["callback_count_asserted"] and observation["callback_count"] == 0
        assert observation["callback_positive_control_count"] == 1
    assert all(not row["full_atomic_acceptance"] for row in binding["bindings"])
    assert not binding["full_ac0008_acceptance"]
    preserved = read("historical-evidence-preserved.json")
    historical = PACKET.parent / "ac0008-vscode-execution-2026-10-10"
    assert preserved["derived_files_preserved"] == len(preserved["files"]) == 166
    for row in preserved["files"]:
        content = (historical / row["path"]).read_bytes()
        assert sha(content) == row["sha256"] and len(content) == row["bytes"]
    print(json.dumps({"status": "passed", "source_commit": head, "public_files": len(checksums),
                      "normative_cases": len(normative), "recorded_tests": counts,
                      "historical_files_preserved": 166, "ac0008": "not_satisfied"}))


if __name__ == "__main__":
    main()
