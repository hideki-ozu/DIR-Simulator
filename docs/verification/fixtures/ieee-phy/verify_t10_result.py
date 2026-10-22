#!/usr/bin/env python3
"""Independent byte/FCS and 10 Mb/s marker oracle for the shipped T10 cases."""
import argparse
import json
import zlib
from pathlib import Path


def verify(path, repeater=False):
    rows = json.loads(Path(path).read_text())["simulation"]["model_records"]
    physical = [r for r in rows if r["schema_name"] == "dir.network.phy"]

    def kind(name):
        return [r for r in physical if r["data"].get("kind") == name]

    sender = "m0" if repeater else "p0"
    receivers = {"m1", "m2"} if repeater else {"p1"}
    assert len(kind("TxStarted")) == 1, "one actual MAC attempt"
    header = bytes.fromhex("0200000000020200000000018100000a0800")
    body = header + bytes(46)
    expected = body + zlib.crc32(body).to_bytes(4, "little")
    decoded = kind("RxFrame")
    assert {r["data"]["receiver_endpoint"] for r in decoded} == receivers
    assert len(decoded) == len(receivers)
    for row in decoded:
        data = row["data"]
        actual = bytes(int(b) for b in data["mac_bytes"])
        assert actual == expected
        assert zlib.crc32(actual[:-4]) == int.from_bytes(actual[-4:], "little")
        assert len(data["origins"]) == 1
        assert data["origins"][0]["source_endpoint"] == sender
    markers = {}
    for name in ["TxMacPreamble", "TxMacSfd", "TxMacFcsEnd"]:
        found = kind(name)
        assert len(found) == 1, (name, "exactly one milestone per physical attempt", len(found))
        markers[name] = int(found[0]["data"]["sample_index"])
    # 20 Mchip/s *16 samples/chip. Marker values are boundaries, not
    # callback completion times; this catches an early/duplicate last-bit marker.
    assert (markers["TxMacSfd"] - markers["TxMacPreamble"]) * 3125 == 64 * 100000
    assert (markers["TxMacFcsEnd"] - markers["TxMacSfd"]) * 3125 == len(expected) * 8 * 100000
    ingress = kind("decoded_ingress")
    assert len(ingress) == len(receivers)
    for row in ingress:
        data = row["data"]
        assert data["receiver_endpoint"] in receivers
        assert int(data["rx_sfd_sample"]) > markers["TxMacSfd"]
        assert int(data["rx_fcs_sample"]) > markers["TxMacFcsEnd"]
        assert int(data["rx_fcs_sample"]) - int(data["rx_sfd_sample"]) == len(expected) * 8 * 32
        matched = [r for r in rows if r["schema_name"] == "ethernet.dynamic.reception"
                   and r["data"]["ingress"] == data["receiver_port"]]
        assert len(matched) == 1
        assert matched[0]["data"]["observed_ps"] == row["time_ps"]
    receptions = [r for r in rows if r["schema_name"] == "ethernet.dynamic.reception"]
    assert not any(r["data"]["ingress"].startswith("Net.hub.") for r in receptions)
    assert sum(r["data"]["ingress"] == "Net.b.rx" and r["data"]["status"] == "received"
               for r in receptions) == 1
    return {"transmissions": 1, "decoded_receivers": len(receivers), "terminal_deliveries": 1,
            "independent_mac_bytes": len(expected), "repeater": repeater}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result", type=Path)
    parser.add_argument("--repeater", action="store_true")
    args = parser.parse_args()
    print(json.dumps(verify(args.result, args.repeater), indent=2))
