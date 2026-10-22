#!/usr/bin/env python3
"""Independent byte/FCS/time oracle for the full provider-l2 example result."""
import argparse
import hashlib
import json
from pathlib import Path
import zlib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result", type=Path)
    args = parser.parse_args()
    raw = args.result.read_bytes()
    result = json.loads(raw)
    example = Path(__file__).resolve().parents[4] / "examples/network-composite/provider-l2"
    config = json.loads((example / "model.json").read_bytes())
    workload = json.loads((example / "workload.json").read_bytes())
    endpoints = config["base"]["config"]["base"]["config"]["endpoints"]
    macs = {e["instance"]: bytes.fromhex(e["mac"].replace(":", "")) for e in endpoints}
    expected = {}
    for generator in workload["generators"]:
        frame = generator["frame"]
        tag = frame["tag"]
        tci = (tag["pcp"] << 13) | (tag["dei"] << 12) | tag["vid"]
        header = bytes.fromhex(frame["dst_mac"].replace(":", "")) + macs[generator["node"]]
        payload = bytes.fromhex(frame["data"])
        assert len(payload) == 46, "oracle fixture expects exact 46-byte payloads"
        expected[generator["id"] + ":0"] = (
            header + b"\x81\x00" + tci.to_bytes(2, "big")
            + frame["ether_type"].to_bytes(2, "big") + payload
        )
    count = 0
    for record in result["simulation"]["model_records"]:
        if record["schema_name"] != "ethernet.dynamic.transfer":
            continue
        transfer = record["data"]
        if not transfer["from_port"].endswith(".tx_pnp"):
            continue
        frame_id = transfer["frame_id"]
        old = expected[frame_id]
        sv = 200 if frame_id.startswith("b_") or frame_id == "learn_b:0" else 100
        outer_tci = (int.from_bytes(old[14:16], "big") & 0xF000) | sv
        body = old[:12] + b"\x88\xa8" + outer_tci.to_bytes(2, "big") + old[12:]
        fcs = zlib.crc32(body).to_bytes(4, "little")
        actual = bytes.fromhex(transfer["wire"]["mac_hex"])
        assert actual == body + fcs, (frame_id, "literal bytes/FCS differ from immutable workload")
        assert transfer["wire"]["fcs_hex"] == fcs.hex()
        assert int(transfer["wire"]["mac_bytes"]) == len(actual) == 72
        sof, eof, release = (int(transfer[k]) for k in ["sof_ps", "eof_ps", "release_ps"])
        assert eof - sof == (len(actual) + 8) * 8 * 1000, "1Gbps preamble + actual MAC"
        assert release - eof == 12 * 8 * 1000, "IFG"
        count += 1
    assert count == len(expected) == 6, "full fixture must serialize each service packet once"
    print(json.dumps({"status": "pass", "pnp_transmissions": count,
                      "oracle": "immutable workload bytes, Python zlib CRC32, integer wire duration",
                      "result_sha256": hashlib.sha256(raw).hexdigest()}))


if __name__ == "__main__":
    main()
