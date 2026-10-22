#!/usr/bin/env python3
"""Independent literal timer/MAC-byte oracle for plca-pending.ini results."""
import json
import sys
import zlib

result = json.load(open(sys.argv[1], encoding="utf-8"))
rows = result["simulation"]["model_records"]
phy = [r["data"] for r in rows if r["schema_name"] == "dir.network.phy"]
phy.sort(key=lambda d: (int(d.get("sample_index", d.get("rx_fcs_sample", 0))),
                        int(d.get("effect_seq", 0))))
node = [d for d in phy if d.get("endpoint") == "p2"]

def state(name):
    return next(d for d in node if d.get("command") == "plca_data:" + name)

hold = state("Hold")
collide = next(d for d in node if d["kind"] == "Collision")
assert int(collide["sample_index"]) - int(hold["sample_index"]) == 396 * 40
assert collide["late"] is False
pending_delay, pending, wait = (state(n) for n in ("DelayPending", "Pending", "WaitMac"))
assert int(pending_delay["sample_index"]) - int(collide["sample_index"]) == 32 * 40
mac_jam_start = next(d for d in node if d.get("command") == "plca_mac_jam_start")
mac_jam_end = next(d for d in node if d.get("command") == "plca_mac_jam_end")
assert int(mac_jam_end["sample_index"]) - int(mac_jam_start["sample_index"]) == 32 * 40
for d in node:
    if d["kind"] in {"TxStarted", "TxMacPreamble", "TxMacSfd", "TxMacFcsEnd", "TxCodedEnd"}:
        assert not int(mac_jam_start["sample_index"]) <= int(d["sample_index"]) < int(mac_jam_end["sample_index"])

# COLLIDE ELSE restarts pending_timer during JAM;148.4.5.4 specifies512 bits in DELAY_PENDING.
assert int(pending["sample_index"]) - int(collide["sample_index"]) == (32 + 512) * 40
assert int(pending["sample_index"]) - int(pending_delay["sample_index"]) == 512 * 40
assert int(wait["sample_index"]) >= int(pending["sample_index"])
assert any(d["kind"] == "Retry" for d in node)
mac_off = state("WaitIdle")
finish = next(d for d in node if d["kind"] == "TxFinished")
assert int(mac_off["sample_index"]) < int(finish["sample_index"])

# Literal application contract: Net.c MAC03 -> Net.b MAC02, VLAN10, IPv4
# EtherType, empty tagged payload padded to46 bytes. Do not read TX bytes.
body = bytes.fromhex("0200000000020200000000038100000a0800") + bytes(46)
expected = body + zlib.crc32(body).to_bytes(4, "little")
frames = [d for d in phy if d["kind"] == "RxFrame"]
assert {d["receiver_endpoint"] for d in frames} == {"p0", "p1"}
assert len(frames) == 2
for frame in frames:
    assert bytes(map(int, frame["mac_bytes"])) == expected
    assert len(frame["origins"]) == 1
    assert int(frame["origins"][0]["attempt"]) > 0
    assert frame["origins"][0]["source_endpoint"] == "p2"
    assert int(frame["rx_fcs_sample"]) - int(frame["rx_sfd_sample"]) == 68 * 8 * 40
assert any(r["schema_name"] == "ethernet.dynamic.reception"
           and r["data"]["ingress"] == "Net.b.rx"
           and r["data"]["status"] == "received" for r in rows)
print("PASS: HOLD396, MAC-only-JAM32 (suppressed MDI), pending512, committed MAC retry, zero-delay MAC-off before coded end, two literal68B/FCS decodes, actual destination")
