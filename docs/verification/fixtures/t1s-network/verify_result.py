#!/usr/bin/env python3
"""Independent reference for the shipped three-station noiseless T1S case."""
import json
import sys
import zlib
from pathlib import Path


def verify(path):
    result = json.loads(Path(path).read_text())
    rows = result["simulation"]["model_records"]
    physical = [r for r in rows if r["schema_name"] == "dir.network.phy"]
    by_kind = lambda kind: [r for r in physical if r["data"].get("kind") == kind]
    assert len(by_kind("TxStarted")) == 1
    # Explicit empty Ethernet-II payload, VLAN10, selected legacy MAC padding.
    header = bytes.fromhex("0200000000020200000000018100000a0800")
    body = header + bytes(46)
    expected = body + zlib.crc32(body).to_bytes(4, "little")
    assert len(expected) == 68
    decoded = by_kind("RxFrame")
    assert {r["data"]["receiver_endpoint"] for r in decoded} == {"p1", "p2"}
    for row in decoded:
        actual = bytes(int(v) for v in row["data"]["mac_bytes"])
        assert actual == expected
        assert zlib.crc32(actual[:-4]) == int.from_bytes(actual[-4:], "little")
        assert len(row["data"]["origins"]) == 1
    markers = {kind: int(by_kind(kind)[0]["data"]["sample_index"])
               for kind in ["TxMacPreamble", "TxMacSfd", "TxMacFcsEnd"]}
    # At 400 Msamples/s each sample is exactly 2500ps.
    assert (markers["TxMacSfd"] - markers["TxMacPreamble"]) * 2500 == 64 * 100000
    assert (markers["TxMacFcsEnd"] - markers["TxMacSfd"]) * 2500 == len(expected) * 8 * 100000
    observations = by_kind("decoded_ingress")
    assert len(observations) == 2
    for row in observations:
        d = row["data"]
        delay = {"p1": 16, "p2": 32}[d["receiver_endpoint"]]
        assert int(d["rx_sfd_sample"]) - markers["TxMacSfd"] == delay
        assert int(d["rx_fcs_sample"]) - markers["TxMacFcsEnd"] == delay
        receptions = [r for r in rows if r["schema_name"] == "ethernet.dynamic.reception"
                      and r["data"]["ingress"] == d["receiver_port"]]
        assert len(receptions) == 1
        assert receptions[0]["data"]["observed_ps"] == row["time_ps"]
    receptions = [r for r in rows if r["schema_name"] == "ethernet.dynamic.reception"]
    assert not any(r["data"]["ingress"].startswith("Net.hub.") for r in receptions)
    assert sum(r["data"]["ingress"] == "Net.b.rx" and r["data"]["status"] == "received"
               for r in receptions) == 1
    return {"transmissions": 1, "physical_receptions": 2, "terminal_deliveries": 1,
            "independent_mac_bytes": len(expected), "propagation_ps": [40000, 80000]}


if __name__ == "__main__":
    print(json.dumps(verify(sys.argv[1]), indent=2))
