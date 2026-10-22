#!/usr/bin/env python3
"""Independent bytes/CRC/receiver oracle for t1s-control-fifo/model.ini."""
import argparse
import json
import zlib
from collections import Counter
from pathlib import Path


def verify(path):
    result = json.loads(Path(path).read_text())
    assert result["simulation"]["termination"] == "time_limit"
    rows = result["simulation"]["model_records"]
    phy = [r for r in rows if r["schema_name"] == "dir.network.phy"]
    observations = [r for r in phy if r["data"].get("kind") == "control_decoded_ingress"]
    raw = [r for r in phy if r["data"].get("kind") == "RxFrame"]
    receives = [r for r in rows if r["schema_name"] == "ethernet.control.wire.event"
                and r["data"]["event_kind"] == "receive"]
    assert len(observations) >= 4, "actual shared decode is required"
    endpoints = {"Net.s1.rx_b": "p0", "Net.s2.rx_b": "p1", "Net.s3.rx_b": "p2"}
    counts = Counter()
    for row in observations:
        data = row["data"]
        packet, receiver = data["packet_id"], data["receiver"]
        mac = bytes.fromhex(data["mac_hex"])
        assert len(mac) == 64
        assert mac[:6] == bytes.fromhex("0180c2000000")
        assert mac[12:17] == bytes.fromhex("0027424203"), "802.3 length + LLC"
        assert int.from_bytes(mac[-4:], "little") == zlib.crc32(mac[:-4])
        physical = [p for p in raw if p["time_ps"] == row["time_ps"]
                    and p["data"]["receiver_endpoint"] == endpoints[receiver]
                    and bytes(map(int, p["data"]["mac_bytes"])) == mac
                    and len(p["data"]["origins"]) == 1
                    and p["data"]["origins"][0]["frame_id"] == f"wire-phy:0:{packet}"]
        assert len(physical) == 1, "each delivery needs its own actual PHY proof"
        proof = physical[0]["data"]
        assert int(proof["rx_sfd_sample"]) < int(proof["rx_fcs_sample"]) < int(proof["sample_index"])
        protocol = [r for r in receives if r["time_ps"] == row["time_ps"]
                    and r["data"]["packet_id"] == f"{packet}@{receiver}"
                    and r["data"]["bytes_hex"] == mac.hex()
                    and r["data"]["phase"] == 1
                    and not r["data"]["external_injection"]]
        assert len(protocol) == 1, "protocol must consume actual received bytes once"
        counts[packet] += 1
    assert len(counts) >= 2 and all(n == 2 for n in counts.values())
    assert any(r["data"].get("kind") == "Collision" for r in phy)
    assert any(r["data"].get("kind") == "Retry" for r in phy)
    return {"result": "PASS", "decoded_control_ingresses": len(observations),
            "shared_control_packets": len(counts), "crc": "zlib.crc32"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("results")
    print(json.dumps(verify(parser.parse_args().results), sort_keys=True))
