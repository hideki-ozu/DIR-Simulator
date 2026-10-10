"""Verify the public evidence projection; executes no product or audit workload."""
import hashlib
import json
import re
from pathlib import Path

PACKET = Path(__file__).resolve().parent
REPOSITORY = PACKET.parents[3]
TESTED = "ddfa7bd916212a0b9fedb6adffa9bdb5e83c8401"


def read(relative):
    return json.loads((PACKET / relative).read_text())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    manifest = read("publication-manifest.json")
    assert manifest["tested_source_commit"] == TESTED
    records = {r["path"]: r for r in manifest["files"]}
    assert len(records) == manifest["derived_file_count"] == len(manifest["files"])
    for path, record in records.items():
        candidate = PACKET / path
        assert candidate.resolve().is_relative_to(PACKET.resolve()) and not candidate.is_symlink()
        assert candidate.stat().st_size == record["published_bytes"], path
        assert sha(candidate) == record["published_sha256"], path
        if candidate.suffix == ".json":
            json.loads(candidate.read_text())
    checksums = {}
    for line in (PACKET / "SHA256SUMS").read_text().splitlines():
        digest, path = line.split("  ", 1)
        assert path not in checksums
        candidate = PACKET / path
        assert candidate.resolve().is_relative_to(PACKET.resolve()) and not candidate.is_symlink()
        assert sha(candidate) == digest, path
        checksums[path] = digest
    expected = {p.relative_to(PACKET).as_posix() for p in PACKET.rglob("*")
                if p.is_file() and p.name != "SHA256SUMS" and "__pycache__" not in p.parts}
    assert set(checksums) == expected
    source = read("results/source-inventory.json")
    assert source["source_commit"] == TESTED and len(source["files"]) == 6
    for record in source["files"]:
        candidate = REPOSITORY / record["path"]
        assert candidate.resolve().is_relative_to(REPOSITORY.resolve())
        assert candidate.stat().st_size == record["bytes"] and sha(candidate) == record["sha256"], record["path"]
    gates = read("results/integrated-checks.json")
    assert gates["source_commit"] == gates["test_commit"] == TESTED and gates["status"] == "passed"
    counts = {}
    for command in gates["commands"]:
        assert command["exit_code"] == 0
        for stream in ["stdout", "stderr"]:
            assert records[command[stream]]["original_sha256"] == command[stream + "_sha256"]
        outcomes = command["named_tests"]
        assert all(item["outcome"] == "ok" for item in outcomes)
        if outcomes:
            log = (PACKET / command["stdout"]).read_text()
            passed = sum(int(count) for count in re.findall(r"test result: ok\. (\d+) passed;", log))
            assert passed == len(outcomes), command["name"]
            counts[command["name"]] = passed
    assert counts == {"focused-tests": 15, "input-unit-tests": 45, "diagnostic-regressions": 15}
    bindings = read("results/runtime-case-bindings.json")
    assert bindings["source_commit"] == bindings["test_commit"] == TESTED
    assert len(bindings["atomic_partial_bindings"]) == 11
    for row in bindings["atomic_partial_bindings"]:
        assert row["status"] == "partial_predicate_evidence_only"
        for item in row["evidence"]:
            if item["direct_executed_test"]:
                assert records[item["execution_log"]]["original_sha256"] == item["execution_log_sha256"]
    rejection = bindings["rejection_bindings"]
    assert len(rejection) == 7 and not any(row["full_atomic_acceptance"] for row in rejection)
    gaps = [gap for row in rejection for gap in row["normative_diagnostic_mismatches"] if gap["field"] == "reason"]
    assert len(gaps) == 3
    assert all(gap["expected"] == "unsupported_syntax" and gap["actual"] == "syntax_error" for gap in gaps)
    print(json.dumps({"status": "passed", "tested_source_commit": TESTED,
                      "public_files_verified": len(checksums), "derived_files": len(records),
                      "source_files_verified": len(source["files"]), "recorded_tests": counts,
                      "atomic_acceptance": "partial; no complete pass claimed"}))


if __name__ == "__main__":
    main()
