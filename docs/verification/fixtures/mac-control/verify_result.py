#!/usr/bin/env python3
"""Independent generated-PAUSE fixture oracle: bytes, CRC, receive and timer.

This checks declared physical rates and actual records, without importing the
simulator's parser, CRC, duration or timer implementation. It requires at least
one completed generated PAUSE exchange and one loaded positive timer.
"""
import argparse
import json
from pathlib import Path
import zlib


def verify(path: Path) -> dict:
    result = json.loads(path.read_text())
    rates = {
        d["from_port"]: int(d["bitrate_bps"])
        for d in result["metadata"]["ethernet_topology"]["directions"]
    }
    rows = sorted(
        (r for r in result["simulation"]["model_records"]
         if r["schema_name"] == "dir.network.mac_control"),
        key=lambda r: (int(r["time_ps"]), int(r["data"]["effect_seq"])),
    )
    sent, received, latest = {}, {}, {}
    positive = zero = 0
    for row in rows:
        at, data = int(row["time_ps"]), row["data"]
        kind, port, evidence = data["kind"], data["port"], data["evidence"]
        if kind in ("tx_started", "receive"):
            raw = bytes.fromhex(evidence["mac_hex"])
            assert len(raw) == 64
            assert raw[:6] == bytes.fromhex("0180c2000001")
            assert raw[12:16] == bytes.fromhex("88080001")
            assert raw[18:60] == bytes(42)
            assert raw[60:] == zlib.crc32(raw[:60]).to_bytes(4, "little")
            packet = evidence["packet_id"]
            quanta = int.from_bytes(raw[16:18], "big")
            if kind == "tx_started":
                assert packet not in sent
                sent[packet] = (at, raw, port)
            else:
                assert packet in sent and sent[packet][0] <= at
                assert sent[packet][1] == raw
                assert (packet, port) not in received
                received[packet, port] = at
                assert int(evidence["quanta"]) == quanta
                latest[port] = (packet, at, quanta)
        elif kind == "timer_load":
            packet, rx, quanta = latest[port]
            assert at >= rx
            rate = rates[port]
            expected = (quanta * 512 * 10**12 + rate - 1) // rate
            assert int(evidence["duration_ps"]) == expected
            assert int(evidence["expires_ps"]) == at + expected
            timer = data["state"]["timer"]
            if quanta:
                positive += 1
                assert int(timer["loaded_ps"]) == at
                assert int(timer["expires_ps"]) == at + expected
                assert int(timer["request"]["quanta"]) == quanta
                assert int(timer["request"]["received_ps"]) == rx
                assert timer["request"]["evidence"] == packet
            else:
                zero += 1
                assert timer is None
        elif kind == "expiry":
            assert data["state"]["timer"] is None
    assert received and positive, "fixture needs a real generated exchange and positive timer"
    return {"sent": len(sent), "received": len(received),
            "positive_timer_loads": positive, "zero_timer_loads": zero}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result", type=Path)
    args = parser.parse_args()
    print(json.dumps({"status": "passed", **verify(args.result)}, sort_keys=True))
