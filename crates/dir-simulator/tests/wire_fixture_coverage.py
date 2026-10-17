#!/usr/bin/env python3
"""Report assertion receipts emitted by successful Rust supplied-vector tests.
Never infer execution from a vector kind, test name, or fixture structure.
"""
import argparse
import collections
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FILES = ["membership-wire-v1", "bridge-wire-v1", "rstp-standard-v1", "mvrp-standard-v1", "bridge-integration-v1"]
OWN = {"membership-wire-v1", "rstp-standard-v1", "mvrp-standard-v1"}

def report(receipts):
    rows = []
    for name in FILES:
        path = ROOT / "docs/proposals/fixtures" / (name + ".json")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        for vector in json.loads(path.read_text())["vectors"]:
            file = receipts / f"{name}-{vector['id']}.json"
            row = {"fixture": path.name, "id": vector["id"], "kind": vector["kind"], "status": "not_run", "reason": "No current execution receipt; other-worker scope." if name not in OWN else "No current execution receipt."}
            if file.exists():
                receipt = json.loads(file.read_text())
                assert receipt["fixture"] == path.name and receipt["id"] == vector["id"], file
                assert receipt["fixture_sha256"] == digest, f"stale fixture receipt: {file}"
                fields = receipt["fields"]
                assert {f["field"] for f in fields} == set(vector["expected"]), f"incomplete fields: {file}"
                assert all(f["status"] in {"passed", "not_applicable"} for f in fields), file
                for field in fields:
                    if field["status"] == "passed":
                        assert field["actual"] == vector["expected"][field["field"]], file
                    else:
                        assert field.get("basis"), file
                row = receipt
            rows.append(row)
    own = [r for r in rows if r["fixture"][:-5] in OWN]
    return {"schema_version": 2, "scope": "Actual per-case assertion receipts; field-specific applicability is explicit. Not a full IEEE conformance claim.", "receipt_directory": str(receipts), "counts": dict(collections.Counter(r["status"] for r in rows)), "owned_counts": dict(collections.Counter(r["status"] for r in own)), "owned_total": len(own), "cases": rows}

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--receipts", type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    data = report(args.receipts)
    args.output.write_text(json.dumps(data, indent=2) + "\n")
    print(json.dumps({"counts": data["counts"], "owned_counts": data["owned_counts"]}))
