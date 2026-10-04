#!/usr/bin/env python3
"""Check design-only Ethernet VLAN wire vectors; does not execute a simulator."""

import hashlib
import json
from pathlib import Path
import zlib


ROOT = Path(__file__).resolve().parent
BITRATE_BPS = 1_000_000_000
PREAMBLE_SFD_BYTES = 8
INTERFRAME_GAP_BITS = 96
TPID = 0x8100


def read_vectors():
    document = json.loads((ROOT / "wire-vectors.json").read_text(encoding="utf-8"))
    assert document["status"] == "design_only"
    assert document["profile"] == "ethernet.l2.vlan.v1"
    assert document["source_base"] == "main@811360a"
    return document


def ethernet_crc32(data):
    """Reflected IEEE CRC-32 arithmetic, kept independent from zlib."""
    remainder = 0xFFFFFFFF
    for octet in data:
        remainder ^= octet
        for _ in range(8):
            if remainder & 1:
                remainder = (remainder >> 1) ^ 0xEDB88320
            else:
                remainder >>= 1
    return remainder ^ 0xFFFFFFFF


def fcs_bytes(mac_body):
    crc = ethernet_crc32(mac_body)
    assert crc == zlib.crc32(mac_body), "bitwise CRC and zlib CRC disagree"
    return crc.to_bytes(4, byteorder="little")


def vlan_header(vlan):
    if vlan is None:
        return b"", None
    vid = vlan["vid"]
    pcp = vlan["pcp"]
    dei = vlan["dei"]
    assert 1 <= vid <= 4094, f"VID out of range: {vid}"
    assert 0 <= pcp <= 7, f"PCP out of range: {pcp}"
    assert 0 <= dei <= 1, f"DEI out of range: {dei}"
    tci = (pcp << 13) | (dei << 12) | vid
    return TPID.to_bytes(2, "big") + tci.to_bytes(2, "big"), tci


def make_mac_frame(dst_mac, src_mac, inner_ethertype, payload, vlan):
    tag, tci = vlan_header(vlan)
    header = dst_mac + src_mac + tag + inner_ethertype.to_bytes(2, "big")
    padding = bytes(max(0, 46 - len(payload)))
    body = header + payload + padding
    frame = body + fcs_bytes(body)
    return {
        "header": header,
        "padding": padding,
        "body": body,
        "frame": frame,
        "tci": tci,
    }


def payload_from_input(spec):
    pattern = bytes.fromhex(spec["unit_hex"])
    repeat = spec["repeat"]
    payload = pattern * repeat
    assert len(payload) == spec["bytes"], "payload byte count mismatch"
    return payload


def expected_times(mac_bytes):
    wire_bits = (mac_bytes + PREAMBLE_SFD_BYTES) * 8
    occupied_bits = wire_bits + INTERFRAME_GAP_BITS
    serialization_ps = (wire_bits * 10**12 + BITRATE_BPS - 1) // BITRATE_BPS
    occupied_ps = (occupied_bits * 10**12 + BITRATE_BPS - 1) // BITRATE_BPS
    return wire_bits, occupied_bits, serialization_ps, occupied_ps


def check_vector(vector):
    source = vector["input"]
    expected = vector["expected"]
    dst_mac = bytes.fromhex(source["dst_mac"].replace(":", ""))
    src_mac = bytes.fromhex(source["src_mac"].replace(":", ""))
    assert len(dst_mac) == len(src_mac) == 6
    inner_ethertype = int(source["inner_ethertype_hex"], 16)
    payload = payload_from_input(source["payload"])
    built = make_mac_frame(
        dst_mac, src_mac, inner_ethertype, payload, source["vlan"]
    )
    frame = built["frame"]
    mac_bytes = len(frame)  # DA through FCS; preamble/SFD and IFG are timed separately.
    wire_bits, occupied_bits, serialization_ps, occupied_ps = expected_times(mac_bytes)
    actual = {
        "header_hex": built["header"].hex(),
        "mac_bytes": mac_bytes,
        "pad_bytes": len(built["padding"]),
        "tci_hex": None if built["tci"] is None else f"{built['tci']:04x}",
        "fcs_hex": frame[-4:].hex(),
        "wire_bits": wire_bits,
        "occupied_bits": occupied_bits,
        "serialization_ps": serialization_ps,
        "occupied_ps": occupied_ps,
        "sha256_mac_bytes": hashlib.sha256(frame).hexdigest(),
    }
    assert actual == expected, (
        f"{vector['name']} mismatch: "
        f"{ {key: (expected.get(key), value) for key, value in actual.items() if expected.get(key) != value} }"
    )
    return source, built


def insert_vlan_tag(untagged_frame, tci):
    body = untagged_frame[:-4]
    assert int.from_bytes(body[12:14], "big") != TPID
    tagged_body = body[:12] + TPID.to_bytes(2, "big") + tci.to_bytes(2, "big") + body[12:]
    return tagged_body + fcs_bytes(tagged_body)


def remove_vlan_tag(tagged_frame):
    body = tagged_frame[:-4]
    assert int.from_bytes(body[12:14], "big") == TPID
    untagged_body = body[:12] + body[16:]
    return untagged_body + fcs_bytes(untagged_body)


def check_boundaries_and_tag_transforms():
    lengths = (0, 42, 45, 46, 47, 1500)
    for payload_length in lengths:
        payload = bytes(index & 0xFF for index in range(payload_length))
        untagged = make_mac_frame(
            bytes.fromhex("01005e000001"),
            bytes.fromhex("020000000001"),
            0x0800,
            payload,
            None,
        )
        tagged = insert_vlan_tag(untagged["frame"], (7 << 13) | (1 << 12) | 10)
        restored = remove_vlan_tag(tagged)

        # The four-byte VLAN header changes the FCS while retaining data and source padding.
        assert tagged[-4:] != untagged["frame"][-4:], f"FCS did not change for {payload_length} bytes"
        assert restored == untagged["frame"], f"tag round trip changed wire frame at {payload_length} bytes"
        assert len(untagged["padding"]) == max(0, 46 - payload_length)
        assert tagged[18:-4] == untagged["body"][14:], f"tagging changed payload or padding at {payload_length} bytes"


def main():
    document = read_vectors()
    assert document["design_contract"]["tpid_hex"] == "8100"
    assert document["design_contract"]["vid_range"] == [1, 4094]
    assert document["design_contract"]["pcp_range"] == [0, 7]
    assert document["design_contract"]["dei_range"] == [0, 1]
    assert document["design_contract"]["tci_byte_order"] == "network"
    assert document["design_contract"]["source_padding"] == "max(0, 46 - payload_bytes), for tagged and untagged"
    assert ethernet_crc32(b"123456789") == 0xCBF43926

    vectors = document["vectors"]
    assert [vector["name"] for vector in vectors] == [
        "empty-untagged",
        "empty-tagged-vid10-pcp7-dei1",
        "payload1500-untagged",
        "payload1500-tagged-vid10-pcp7-dei1",
    ]
    for vector in vectors:
        check_vector(vector)

    contract_values = [
        (64, 576_000, 672_000),
        (68, 608_000, 704_000),
        (1518, 12_208_000, 12_304_000),
        (1522, 12_240_000, 12_336_000),
    ]
    observed = [
        (
            vector["expected"]["mac_bytes"],
            vector["expected"]["serialization_ps"],
            vector["expected"]["occupied_ps"],
        )
        for vector in vectors
    ]
    assert observed == contract_values, f"design length/timing sequence mismatch: {observed}"

    check_boundaries_and_tag_transforms()
    print(
        f"Ethernet VLAN design-only wire vectors: {len(vectors)} vectors; "
        "CRC, timing, payload boundaries and tag round trips PASS (no product execution)"
    )


if __name__ == "__main__":
    main()
