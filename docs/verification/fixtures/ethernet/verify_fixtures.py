"""Independent document-fixture checks; optional product-output projection check."""
from __future__ import annotations
import argparse
import configparser
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent

def read(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            assert key not in result, f"duplicate JSON key: {key}"
            result[key] = value
        return result
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique)

def crc_reflected(data):
    register = 0xFFFFFFFF
    for octet in data:
        for bit_index in range(8):
            feedback = (register & 1) ^ ((octet >> bit_index) & 1)
            register >>= 1
            if feedback:
                register ^= 0xEDB88320
    return register ^ 0xFFFFFFFF

def verify_static():
    vectors = read(ROOT / "vectors.json")
    rate = int(vectors["bitrate_bps"])
    for v in vectors["vectors"]:
        data = bytes.fromhex(v["data"])
        pad = max(0, 46 - len(data))
        raw = (bytes.fromhex(v["dst_mac"].replace(":", ""))
               + bytes.fromhex(v["src_mac"].replace(":", ""))
               + v["ether_type"].to_bytes(2, "big") + data + bytes(pad))
        fcs = crc_reflected(raw).to_bytes(4, "little")
        assert v["pad_bytes"] == pad
        assert v["fcs_hex"] == fcs.hex()
        assert v["mac_hex"] == (raw + fcs).hex()
        assert v["mac_bytes"] == len(raw) + 4
        assert 64 <= v["mac_bytes"] <= 1518
        assert v["wire_bits"] == 8 * (v["mac_bytes"] + 8)
        assert v["occupied_bits"] - v["wire_bits"] == 96
        for bits, field in [(v["wire_bits"], "eof_ps"),
                            (v["occupied_bits"], "release_ps")]:
            assert int(v[field]) == (bits * 10**12 + rate - 1) // rate
    assert crc_reflected(b"123456789") == 0xCBF43926
    scenarios = read(ROOT / "scenarios.json")["scenarios"]
    for scenario in scenarios:
        ini = configparser.ConfigParser(interpolation=None)
        ini.read(ROOT / scenario["config"])
        general = ini["General"]
        assert general["model-profile"] == '"ethernet.l2.store-forward.v1"'
        assert general["model-config"].strip('"') == scenario["model_config"]
        assert general["workload"].strip('"') == scenario["workload"]
        assert (ROOT / general["ned-path"].strip('"') / "ethdemo/Main.ned").is_file()
        workload = read(ROOT / scenario["workload"])
        model = read(ROOT / scenario["model_config"])
        assert set(workload) == {"schema_version", "generators"}
        assert workload["schema_version"] == 2 and model["schema_version"] == 1
        assert len({g["id"] for g in workload["generators"]}) == len(workload["generators"])
        for generator in workload["generators"]:
            assert generator["kind"] == "ethernet.explicit.v1"
            assert generator["node"] in {e["instance"] for e in model["endpoints"]}
            assert all(re.fullmatch(r"0|[1-9][0-9]*", t) for t in generator["times_ps"])
            assert list(map(int, generator["times_ps"])) == sorted(map(int, generator["times_ps"]))
            if scenario["name"] == "bad-payload":
                assert len(generator["frame"]["data"]) % 2 == 1
            else:
                assert len(bytes.fromhex(generator["frame"]["data"])) <= 1500
        if scenario["name"] == "bad-fdb":
            assert model["switches"][0]["fdb"][0]["egress"] == "Main.sw.tx_missing"
        else:
            assert all(e["egress"] in {"Main.sw.tx_a", "Main.sw.tx_b", "Main.sw.tx_c"}
                       for e in model["switches"][0]["fdb"])
    # Independent closed-form path computation, not a simulator implementation.
    eof, release, propagation, switching = 576000, 672000, 1000, 2000
    second_sof = eof + propagation + switching
    assert second_sof == 579000
    expected = next(s for s in scenarios if s["name"] == "unicast")["expected"]
    first, second = expected["transfer_times"]
    assert first["arrival_ps"] == str(eof + propagation)
    assert second["sof_ps"] == str(second_sof)
    assert second["eof_ps"] == str(second_sof + eof)
    assert second["release_ps"] == str(second_sof + release)
    assert expected["deliveries"][0]["received_ps"] == str(second_sof + eof + propagation)
    duplex = next(s for s in scenarios if s["name"] == "duplex")["expected"]
    long_wire = (1518 + 8) * 8 * 1000
    assert 579000 < 1155000 < long_wire
    assert duplex["deliveries"][0]["received_ps"] == str(2 * (long_wire + propagation) + switching)
    for scenario in scenarios:
        e = scenario["expected"]
        if "offered" in e:
            assert e["offered"] == sum(e.get(k, 0) for k in ("queued", "transmitting", "serialized", "dropped"))
    return scenarios, len(vectors["vectors"])

def check_results(scenario, path):
    result = read(path)
    assert result["schema_version"] == 2
    simulation = result["simulation"]
    e = scenario["expected"]
    if "termination" in e:
        assert simulation["termination"] == e["termination"]
    if e.get("termination") == "prep_failed":
        assert simulation["committed_events"] == "0"
        diagnostic = read_lines(path.parent / "diagnostics.jsonl")[0]
        assert diagnostic["code"] == e["code"] and diagnostic["reason"] == e["reason"]
        assert simulation["model_records"] == []
        return
    rows = simulation["model_records"]
    keys = [(r["schema_name"], r["record_id"]) for r in rows]
    assert len(keys) == len(set(keys))
    frames = [r for r in rows if r["schema_name"] == "ethernet.frame"]
    transfers = [r for r in rows if r["schema_name"] == "ethernet.transfer"]
    receptions = [r for r in rows if r["schema_name"] == "ethernet.reception"]
    actual = dict(generated=len(frames), offered=len(transfers), receptions=len(receptions))
    for state in ("queued", "transmitting", "serialized", "dropped"):
        actual[state] = sum(r["data"]["status"] == state for r in transfers)
    for state in ("received", "filtered", "forwarded", "processing"):
        actual[state] = sum(r["data"]["status"] == state for r in receptions)
    for key in actual.keys() & e.keys():
        assert actual[key] == e[key], (key, actual[key], e[key])
    if "drop_ids" in e:
        assert sorted(r["record_id"] for r in transfers if r["data"]["status"] == "dropped") == sorted(e["drop_ids"])
    by_id = {r["record_id"]: r["data"] for r in transfers}
    for expected in e.get("transfer_times", []):
        for key, value in expected.items():
            if key != "id":
                assert by_id[expected["id"]][key] == value
    received = [r for r in receptions if r["data"]["status"] == "received"]
    for d in e.get("deliveries", []):
        matches = [r for r in received if r["request_id"] == d["frame_id"] and r["subject"] == d["node"]]
        assert len(matches) == 1 and matches[0]["data"]["ready_ps"] == d["received_ps"]
    if "delivery_ps" in e:
        assert all(r["data"]["ready_ps"] == e["delivery_ps"] for r in received)
    if "filter_reason" in e:
        assert all(r["data"]["reason"] == e["filter_reason"] for r in receptions if r["data"]["status"] == "filtered")

def read_lines(path):
    return [json.loads(line) for line in path.read_text().splitlines()]

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario")
    parser.add_argument("--results", type=Path)
    args = parser.parse_args()
    scenarios, vector_count = verify_static()
    assert bool(args.scenario) == bool(args.results), "use both --scenario and --results"
    if args.results:
        scenario = next(s for s in scenarios if s["name"] == args.scenario)
        check_results(scenario, args.results)
        print(f"PASS product output projection: {args.scenario}")
    print(f"PASS static fixtures: {vector_count} CRC/frame vectors, {len(scenarios)} input sets, analytical path times")
