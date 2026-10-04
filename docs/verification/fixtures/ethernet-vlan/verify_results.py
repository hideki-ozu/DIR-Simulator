#!/usr/bin/env python3
"""Independent checks of VLAN product bytes, times, quantities and published files."""

import argparse
import csv
import hashlib
import json
from pathlib import Path

from verify_wire_vectors import ethernet_crc32


REASONS = (
    "ingress_frame_type", "ingress_vlan_membership", "destination_mismatch",
    "multicast_not_subscribed", "same_ingress", "no_vlan_egress",
    "multicast_no_egress", "unknown_multicast",
)


def read(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            assert key not in result, f"duplicate JSON key: {key}"
            result[key] = value
        return result
    return json.loads(path.read_text(), object_pairs_hook=unique)


def check_wire(wire):
    payload = bytes.fromhex(wire["data_hex"])
    padding = max(0, 46 - len(payload))
    tag = wire["tag"]
    prefix = b""
    if tag is not None:
        vid, pcp, dei = (int(tag[key]) for key in ("vid", "pcp", "dei"))
        assert 1 <= vid <= 4094 and 0 <= pcp <= 7 and 0 <= dei <= 1
        prefix = b"\x81\x00" + ((pcp << 13) | (dei << 12) | vid).to_bytes(2, "big")
    body = (
        bytes.fromhex(wire["dst_mac"].replace(":", ""))
        + bytes.fromhex(wire["src_mac"].replace(":", ""))
        + prefix + int(wire["ether_type"]).to_bytes(2, "big")
        + payload + bytes(padding)
    )
    fcs = ethernet_crc32(body).to_bytes(4, "little")
    assert wire["fcs_hex"] == fcs.hex()
    assert wire["mac_hex"] == (body + fcs).hex()
    assert int(wire["pad_bytes"]) == padding
    assert int(wire["mac_bytes"]) == len(body) + 4


def check_flows(metadata, frames, transfers, receptions, summary):
    """Count only actual receptions and decompose each completed delivery path."""
    deliveries = []
    for row in receptions.values():
        rx = row["data"]
        if rx["status"] != "received":
            continue
        frame = frames[rx["frame_id"]]["data"]
        delay = int(rx["ready_ps"]) - int(frame["generated_ps"])
        components = [0, 0, 0, int(frame["ready_ps"]) - int(frame["generated_ps"])]
        parent = rx["transfer_id"]
        seen = set()
        while parent is not None:
            assert parent not in seen
            seen.add(parent)
            copy = transfers[parent]["data"]
            hop_rx = receptions[parent + "@rx"]["data"]
            components[0] += int(copy["sof_ps"]) - int(copy["queued_ps"])
            components[1] += int(copy["eof_ps"]) - int(copy["sof_ps"])
            components[2] += int(copy["arrival_ps"]) - int(copy["eof_ps"])
            components[3] += int(hop_rx["ready_ps"]) - int(hop_rx["observed_ps"])
            parent = copy["parent_transfer_id"]
        assert sum(components) == delay and all(value >= 0 for value in components)
        deliveries.append((frame["flow_id"], row["subject"], delay, frame["deadline_ps"], components))

    endpoints = [device["id"] for device in metadata["ethernet_topology"]["devices"]
                 if device["kind"] == "endpoint"]
    for flow in metadata["flows"]:
        flow_id = flow["flow_id"]
        selected_frames = {key: row["data"] for key, row in frames.items()
                           if row["data"]["flow_id"] == flow_id}
        copies = [row["data"] for row in transfers.values()
                  if row["data"]["frame_id"] in selected_frames]
        for endpoint in [None] + endpoints:
            target = f"@flow:{flow_id}" + (f":{endpoint}" if endpoint else "")
            value = lambda metric: summary[target, "ethernet.flow." + metric]["value"]
            if endpoint is None:
                assert int(value("generated")) == len(selected_frames)
                assert int(value("source_processing")) == sum(f["ready_ps"] is None for f in selected_frames.values())
                assert int(value("copy_dropped")) == sum(t["status"] == "dropped" for t in copies)
                assert int(value("unfinished_copies")) == sum(t["status"] != "dropped" and t["arrival_ps"] is None for t in copies)
            for status in ("received", "filtered", "processing"):
                count = sum(row["data"]["frame_id"] in selected_frames
                            and (endpoint is None or row["subject"] == endpoint)
                            and row["data"]["status"] == status for row in receptions.values())
                assert int(value(status)) == count
            samples = [delivery for delivery in deliveries
                       if delivery[0] == flow_id and (endpoint is None or delivery[1] == endpoint)]
            deadlines = [sample for sample in samples if sample[3] is not None]
            assert int(value("deadline_sample_count")) == len(deadlines)
            assert int(value("deadline_missed")) == sum(sample[2] > int(sample[3]) for sample in deadlines)
            for index, metric in enumerate(("queue_wait", "serialization", "propagation", "processing")):
                expected = sum(sample[4][index] for sample in samples) / len(samples) if samples else None
                assert value(metric + "_mean_ps") == expected
            expected = sum(sample[2] for sample in samples) / len(samples) if samples else None
            assert value("delivery_mean_ps") == expected


def check(path):
    raw = read(path)
    sim, metadata = raw["simulation"], raw["metadata"]
    assert raw["schema_version"] == 2
    assert metadata["model_profile"] == "ethernet.l2.vlan.v1"
    assert {s["schema_name"]: s["schema_version"] for s in metadata["model_schemas"]} == {
        "ethernet.frame": 3, "ethernet.transfer": 3, "ethernet.reception": 2,
    }
    assert metadata["topology"] == metadata["ethernet_topology"]
    topology = metadata["ethernet_topology"]
    directions = {d["from_port"]: d for d in topology["directions"]}
    ports = {p["ingress"]: p for p in topology["ports"]}
    collections = {}
    for schema, version in (("ethernet.frame", 3), ("ethernet.transfer", 3), ("ethernet.reception", 2)):
        rows = [r for r in sim["model_records"] if r["schema_name"] == schema]
        assert all(r["schema_version"] == version for r in rows)
        collection = {r["record_id"]: r for r in rows}
        assert len(collection) == len(rows)
        collections[schema] = collection
    frames, transfers, receptions = (collections[s] for s in (
        "ethernet.frame", "ethernet.transfer", "ethernet.reception",
    ))
    assert len(sim["model_records"]) == sum(map(len, (frames, transfers, receptions)))
    for frame in frames.values():
        check_wire(frame["data"])
    for transfer_id, row in transfers.items():
        copy = row["data"]
        frame = frames[copy["frame_id"]]["data"]
        check_wire(copy["wire"])
        for key in ("src_mac", "dst_mac", "ether_type", "data_hex", "pad_bytes"):
            assert copy["wire"][key] == frame[key]
        assert copy["queue_id"] == f"{copy['from_port']}.queue.{copy['priority']}"
        if copy["sof_ps"] is not None:
            direction = directions[copy["from_port"]]
            rate = int(direction["bitrate_bps"])
            mac = int(copy["wire"]["mac_bytes"])
            sof = int(copy["sof_ps"])
            eof = sof + ((mac + 8) * 8 * 10**12 + rate - 1) // rate
            release = sof + ((mac + 20) * 8 * 10**12 + rate - 1) // rate
            assert int(copy["planned_eof_ps"]) == eof
            assert int(copy["planned_release_ps"]) == release
            assert int(copy["planned_arrival_ps"]) == eof + int(direction["delay_ps"])
            for field in ("eof_ps", "release_ps", "arrival_ps"):
                if copy[field] is not None:
                    assert copy[field] == copy[f"planned_{field}"]
        if copy["parent_transfer_id"] is not None:
            parent_rx = receptions[f"{copy['parent_transfer_id']}@rx"]["data"]
            assert parent_rx["status"] == "forwarded"
            assert transfer_id in parent_rx["egress_transfer_ids"]
            assert copy["queued_ps"] == parent_rx["ready_ps"]
            assert copy["vlan_id"] == parent_rx["vlan_id"]
            assert copy["priority"] == parent_rx["priority"]
    arrived = {key for key, row in transfers.items() if row["data"]["arrival_ps"] is not None}
    assert {r["data"]["transfer_id"] for r in receptions.values()} == arrived
    for row in receptions.values():
        rx = row["data"]
        copy = transfers[rx["transfer_id"]]["data"]
        assert rx["observed_ps"] == copy["arrival_ps"]
        policy = ports[rx["ingress"]]
        tag = copy["wire"]["tag"]
        assert rx["vlan_id"] == (tag["vid"] if tag else policy["pvid"])
        assert rx["priority"] == (tag["pcp"] if tag else policy["default_priority"])
        assert (rx["reason"] in REASONS) if rx["status"] == "filtered" else rx["reason"] is None

    summary = {(r["target"], r["metric"]): r for r in sim["summary"]}
    for target in [d["id"] for d in topology["devices"]] + ["$all"]:
        filtered = [r["data"] for r in receptions.values()
                    if r["data"]["status"] == "filtered" and (target == "$all" or r["subject"] == target)]
        assert int(summary[target, "ethernet.filtered"]["value"]) == len(filtered)
        for reason in REASONS:
            assert int(summary[target, f"ethernet.filtered.{reason}"]["value"]) == sum(r["reason"] == reason for r in filtered)
    for port in directions:
        copies = [r["data"] for r in transfers.values() if r["data"]["from_port"] == port]
        for metric, milestone, extra in (("mac_bits", "eof_ps", 0), ("wire_bits", "eof_ps", 8), ("occupied_bits", "release_ps", 20)):
            actual = sum((int(t["wire"]["mac_bytes"]) + extra) * 8 for t in copies if t[milestone] is not None)
            assert int(summary[port, f"ethernet.{metric}"]["value"]) == actual
    check_flows(metadata, frames, transfers, receptions, summary)

    manifest = read(path.parent / "manifest.json")
    assert manifest["schema_version"] == 2 and manifest["run_id"] == raw["run_id"]
    assert manifest["partial"] == sim["partial"] and manifest["termination"] == sim["termination"]
    for entry in manifest["files"]:
        data = (path.parent / entry["name"]).read_bytes()
        assert str(len(data)) == entry["bytes"]
        assert hashlib.sha256(data).hexdigest() == entry["sha256"]
    csv_count = 0
    for filename, key in (("events.csv", "records"), ("summary.csv", "summary")):
        with (path.parent / filename).open(newline="") as file:
            csv_rows = list(csv.DictReader(file))
        assert len(csv_rows) == len(sim[key])
        for csv_row, json_row in zip(csv_rows, sim[key]):
            assert csv_row["schema_version"] == "2" and csv_row["run_id"] == raw["run_id"]
            for field, value in json_row.items():
                if value is None:
                    assert csv_row[field] == ""
                elif isinstance(value, float):
                    assert float(csv_row[field]) == value
                else:
                    assert csv_row[field] == str(value)
        csv_count += len(csv_rows)
    return len(frames), len(transfers), len(receptions), csv_count


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path, nargs="+")
    args = parser.parse_args()
    for path in args.results:
        counts = check(path)
        print(f"PASS VLAN product projection: {path} ({counts[0]} frames/{counts[1]} copies/{counts[2]} receptions/{counts[3]} CSV rows)")
