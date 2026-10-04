"""Independent document-fixture checks; optional product-output projection check."""
from __future__ import annotations
import argparse
import configparser
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent
MODEL_SCHEMAS = {
    "ethernet.frame": 1,
    "ethernet.reception": 1,
    "ethernet.transfer": 1,
}
MODEL_RECORD_KEYS = {
    "schema_name", "schema_version", "record_id", "subject", "request_id",
    "origin_request_id", "time_ps", "data",
}
RECORD_KEYS = {
    "seq", "event_seq", "effect_seq", "target", "metric", "unit",
    "value_kind", "value", "time_ps", "start_ps", "end_ps", "request_id",
    "receiver", "reason", "sample_count",
}
CSV_HEADER = (
    "schema_version,run_id,seq,event_seq,effect_seq,target,metric,unit,value_kind,"
    "value,time_ps,start_ps,end_ps,request_id,receiver,reason,sample_count"
)
METRICS = {
    "queue_length": ("count", "integer", "point", "identity"),
    "queue_max": ("count", "integer", "summary", "max"),
    "queue_mean": ("count", "number", "summary", "time_mean"),
    "ethernet.generated": ("count", "integer", "summary", "sum"),
    "ethernet.transfer_offered": ("count", "integer", "summary", "sum"),
    "ethernet.queued": ("count", "integer", "summary", "sum"),
    "ethernet.transmitting": ("count", "integer", "summary", "sum"),
    "ethernet.serialized": ("count", "integer", "summary", "sum"),
    "ethernet.dropped": ("count", "integer", "summary", "sum"),
    "ethernet.received": ("count", "integer", "summary", "sum"),
    "ethernet.filtered": ("count", "integer", "summary", "sum"),
    "ethernet.forwarded": ("count", "integer", "summary", "sum"),
    "ethernet.processing": ("count", "integer", "summary", "sum"),
    "ethernet.link_utilization": ("1", "number", "window_summary", "occupancy_ratio"),
    "ethernet.payload_bits": ("bit", "integer", "window_summary", "sum"),
    "ethernet.mac_bits": ("bit", "integer", "window_summary", "sum"),
    "ethernet.wire_bits": ("bit", "integer", "window_summary", "sum"),
    "ethernet.occupied_bits": ("bit", "integer", "window_summary", "sum"),
    "ethernet.payload_throughput_bps": ("bit/s", "number", "window_summary", "rate"),
    "ethernet.delivery_ps": ("ps", "integer", "point", "identity"),
    "ethernet.delivery_mean_ps": ("ps", "number", "summary", "sample_mean"),
}
FRAME_DATA_KEYS = {
    "source", "src_mac", "dst_mac", "ether_type", "data_hex", "pad_bytes",
    "mac_bytes", "fcs_hex", "mac_hex", "generated_ps", "ready_ps",
}
TRANSFER_DATA_KEYS = {
    "frame_id", "parent_transfer_id", "from_port", "to_port", "queued_ps",
    "sof_ps", "eof_ps", "release_ps", "arrival_ps", "planned_eof_ps",
    "planned_release_ps", "planned_arrival_ps", "status", "drop_reason",
}
RECEPTION_DATA_KEYS = {
    "frame_id", "transfer_id", "ingress", "observed_ps", "ready_ps",
    "planned_ready_ps", "status", "reason", "egress_transfer_ids",
}
D_RE = re.compile(r"0|[1-9][0-9]*\Z")


def require(condition, message):
    assert condition, message


def require_d(value, where):
    require(isinstance(value, str) and D_RE.fullmatch(value) is not None,
            f"{where} must be a canonical nonnegative decimal string")
    return int(value)

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
    require(len(vectors["vectors"]) == 4, "expected exactly four Ethernet frame vectors")
    require({v["id"] for v in vectors["vectors"]} == {
        "empty", "one-byte", "minimum-payload", "maximum-payload"
    }, "unexpected Ethernet frame vector set")
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
    require(len(scenarios) == 12, "expected exactly twelve Ethernet v1 input sets")
    require(len({scenario["name"] for scenario in scenarios}) == len(scenarios),
            "duplicate Ethernet scenario name")
    require({scenario["name"] for scenario in scenarios} == {
        "unicast", "duplex", "broadcast", "unknown-unicast", "source-full",
        "egress-full", "zero-capacity", "eof-boundary", "arrival-boundary",
        "switch-processing", "bad-fdb", "bad-payload",
    }, "Ethernet v1 scenario set changed")
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
            payload_hex = generator["frame"]["data"]
            assert isinstance(payload_hex, str) and re.fullmatch(r"[0-9a-fA-F]*", payload_hex)
            if scenario["name"] == "bad-payload":
                assert len(payload_hex) % 2 == 1
            else:
                assert len(payload_hex) % 2 == 0 and len(bytes.fromhex(payload_hex)) <= 1500
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

def ceil_div(numerator, denominator):
    return (numerator + denominator - 1) // denominator


def frame_bytes(data):
    payload = bytes.fromhex(data["data_hex"])
    pad = max(0, 46 - len(payload))
    header = (bytes.fromhex(data["dst_mac"].replace(":", ""))
              + bytes.fromhex(data["src_mac"].replace(":", ""))
              + int(data["ether_type"]).to_bytes(2, "big"))
    mac_without_fcs = header + payload + bytes(pad)
    fcs = crc_reflected(mac_without_fcs).to_bytes(4, "little")
    return payload, pad, mac_without_fcs + fcs, fcs


def check_frame_records(frames, transfers, scenario, simulation, metadata):
    model = read(ROOT / scenario["model_config"])
    endpoints = {endpoint["instance"]: endpoint["mac"] for endpoint in model["endpoints"]}
    workloads = read(ROOT / scenario["workload"])
    ini = configparser.ConfigParser(interpolation=None)
    ini.read(ROOT / scenario["config"])
    time_limit = int(ini["General"]["sim-time-limit"].removesuffix("ps"))
    expected_frames = {}
    for generator in workloads["generators"]:
        for ordinal, time in enumerate(map(int, generator["times_ps"])):
            if time < time_limit:
                expected_frames[f"{generator['id']}:{ordinal}"] = (
                    generator, time
                )
    require(set(frames) == set(expected_frames),
            f"frame IDs do not match workload: {scenario['name']}")
    transfer_by_id = {row["record_id"]: row for row in transfers}
    source_by_frame = {}
    for row in transfers:
        data = row["data"]
        if data["parent_transfer_id"] is None:
            require(data["frame_id"] not in source_by_frame,
                    f"multiple source transfers for {data['frame_id']}")
            source_by_frame[data["frame_id"]] = row
    for frame_id, (generator, generated_ps) in expected_frames.items():
        row = frames[frame_id]
        data = row["data"]
        expected_input = generator["frame"]
        expected_source = generator["node"]
        require(data["source"] == expected_source == row["subject"],
                f"frame source mismatch: {frame_id}")
        require(data["src_mac"] == endpoints[expected_source],
                f"source MAC mismatch: {frame_id}")
        require(data["dst_mac"] == expected_input["dst_mac"],
                f"destination MAC mismatch: {frame_id}")
        require(data["ether_type"] == str(expected_input["ether_type"]),
                f"EtherType mismatch: {frame_id}")
        require(data["data_hex"] == expected_input["data"].lower(),
                f"payload mismatch: {frame_id}")
        require(data["generated_ps"] == str(generated_ps),
                f"generation time mismatch: {frame_id}")
        payload, pad, expected_mac, fcs = frame_bytes(data)
        require(data["pad_bytes"] == str(pad), f"padding mismatch: {frame_id}")
        require(data["mac_bytes"] == str(len(expected_mac)), f"MAC length mismatch: {frame_id}")
        require(data["fcs_hex"] == fcs.hex(), f"FCS mismatch: {frame_id}")
        require(data["mac_hex"] == expected_mac.hex(), f"MAC bytes mismatch: {frame_id}")
        require(64 <= len(expected_mac) <= 1518, f"MAC length out of range: {frame_id}")
        require(len(payload) <= 1500, f"payload too long: {frame_id}")
        source_transfer = source_by_frame.get(frame_id)
        require(source_transfer is not None, f"missing source transfer: {frame_id}")
        require(data["ready_ps"] == source_transfer["data"]["queued_ps"],
                f"frame ready/source offer mismatch: {frame_id}")


def check_model_records(rows, scenario, simulation, metadata):
    require(isinstance(rows, list), "simulation.model_records must be an array")
    require(metadata.get("model_profile") == "ethernet.l2.store-forward.v1",
            "wrong Ethernet model profile")
    schemas = metadata.get("model_schemas")
    require(isinstance(schemas, list), "metadata.model_schemas must be an array")
    require(all(set(row) == {"schema_name", "schema_version"} for row in schemas),
            "model schema descriptor field set mismatch")
    actual_schemas = {(row.get("schema_name"), row.get("schema_version")) for row in schemas}
    require(actual_schemas == set(MODEL_SCHEMAS.items()) and len(schemas) == len(MODEL_SCHEMAS),
            "registered model schemas do not match Ethernet v1")
    require(all(isinstance(row["schema_name"], str)
                and type(row["schema_version"]) is int for row in schemas),
            "model schema descriptor types mismatch")
    metric_rows = metadata.get("metrics")
    require(isinstance(metric_rows, list), "metadata.metrics must be an array")
    require(all(set(metric) == {"metric_id", "version", "unit", "value_kind", "sampling", "aggregation"}
                for metric in metric_rows), "metric descriptor field set mismatch")
    for metric in metric_rows:
        require(metric.get("version") == "1", f"unexpected metric version: {metric}")
    require([row.get("metric_id") for row in metric_rows]
            == sorted(row.get("metric_id") for row in metric_rows),
            "metadata.metrics must be sorted by metric_id")
    actual_metrics = {metric.get("metric_id"): metric for metric in metric_rows}
    require(set(actual_metrics) == set(METRICS), "registered Ethernet metric set mismatch")
    for name, descriptor in METRICS.items():
        metric = actual_metrics[name]
        require((metric.get("unit"), metric.get("value_kind"), metric.get("sampling"),
                 metric.get("aggregation")) == descriptor,
                f"metric descriptor mismatch: {name}")

    identifiers = set()
    by_schema = {name: [] for name in MODEL_SCHEMAS}
    sim_end = require_d(simulation["end_ps"], "simulation.end_ps")
    is_time_limit = simulation["termination"] == "time_limit"
    for row in rows:
        require(set(row) == MODEL_RECORD_KEYS, "ModelRecord field set mismatch")
        schema = row["schema_name"]
        require(schema in MODEL_SCHEMAS, f"unknown model schema: {schema}")
        require(type(row["schema_version"]) is int and row["schema_version"] == MODEL_SCHEMAS[schema],
                f"ModelRecord schema version mismatch: {schema}")
        require(all(isinstance(row[key], str) for key in ("record_id", "subject", "request_id")),
                f"ModelRecord string field type mismatch: {schema}")
        require(row["origin_request_id"] is None, f"unexpected origin_request_id: {schema}")
        row_time = require_d(row["time_ps"], f"{schema}.time_ps")
        require(row_time <= sim_end, f"ModelRecord time exceeds simulation end: {schema}/{row['record_id']}")
        if is_time_limit:
            require(row_time < sim_end, f"ModelRecord event was committed at time limit: {schema}/{row['record_id']}")
        key = schema, row["record_id"]
        require(key not in identifiers, f"duplicate ModelRecord: {key}")
        identifiers.add(key)
        data = row["data"]
        require(isinstance(data, dict), f"ModelRecord data must be an object: {key}")
        expected_keys = {
            "ethernet.frame": FRAME_DATA_KEYS,
            "ethernet.transfer": TRANSFER_DATA_KEYS,
            "ethernet.reception": RECEPTION_DATA_KEYS,
        }[schema]
        require(set(data) == expected_keys, f"ModelRecord data field set mismatch: {key}")
        expected_request = data["frame_id"] if schema != "ethernet.frame" else row["record_id"]
        require(row["request_id"] == expected_request,
                f"ModelRecord request_id mismatch: {schema}/{row['record_id']}")
        if schema == "ethernet.frame":
            require(all(isinstance(data[field], str) for field in
                        ("source", "src_mac", "dst_mac", "data_hex", "fcs_hex", "mac_hex")),
                    f"frame string field type mismatch: {key}")
            for field in ("ether_type", "pad_bytes", "mac_bytes", "generated_ps"):
                require_d(data[field], f"{key}.{field}")
            if data["ready_ps"] is not None:
                require_d(data["ready_ps"], f"{key}.ready_ps")
        elif schema == "ethernet.transfer":
            require(all(isinstance(data[field], str) for field in
                        ("frame_id", "from_port", "to_port", "status")),
                    f"transfer string field type mismatch: {key}")
            require(data["parent_transfer_id"] is None or isinstance(data["parent_transfer_id"], str),
                    f"transfer parent type mismatch: {key}")
            require(data["drop_reason"] is None or isinstance(data["drop_reason"], str),
                    f"transfer drop_reason type mismatch: {key}")
        else:
            require(all(isinstance(data[field], str) for field in
                        ("frame_id", "transfer_id", "ingress", "status")),
                    f"reception string field type mismatch: {key}")
            require(data["reason"] is None or isinstance(data["reason"], str),
                    f"reception reason type mismatch: {key}")
            require(isinstance(data["egress_transfer_ids"], list)
                    and all(isinstance(child, str) for child in data["egress_transfer_ids"]),
                    f"reception egress list type mismatch: {key}")
        by_schema[schema].append(row)

    frames = {row["record_id"]: row for row in by_schema["ethernet.frame"]}
    transfers = {row["record_id"]: row for row in by_schema["ethernet.transfer"]}
    receptions = {row["record_id"]: row for row in by_schema["ethernet.reception"]}
    require(len(frames) == len(by_schema["ethernet.frame"]), "duplicate Ethernet frame IDs")
    require(len(transfers) == len(by_schema["ethernet.transfer"]), "duplicate Ethernet transfer IDs")
    require(len(receptions) == len(by_schema["ethernet.reception"]), "duplicate Ethernet reception IDs")
    check_frame_records(frames, by_schema["ethernet.transfer"], scenario, simulation, metadata)

    topology = metadata.get("ethernet_topology")
    require(isinstance(topology, dict), "missing metadata.ethernet_topology")
    devices = {device["id"]: device for device in topology["devices"]}
    directions = {(d["from_port"], d["to_port"]): d for d in topology["directions"]}
    require(len(directions) == len(topology["directions"]), "duplicate Ethernet link direction")
    for direction in directions.values():
        require_d(direction["bitrate_bps"], "topology bitrate_bps")
        require_d(direction["delay_ps"], "topology delay_ps")
        require(int(direction["bitrate_bps"]) > 0, "topology bitrate must be positive")
    receptions_by_transfer = {}
    children_by_parent = {}
    transfer_states = {"queued", "transmitting", "serialized", "dropped"}
    actual_times = ("sof_ps", "eof_ps", "release_ps", "arrival_ps")
    planned_times = ("planned_eof_ps", "planned_release_ps", "planned_arrival_ps")
    for transfer_id, row in transfers.items():
        data = row["data"]
        require(row["request_id"] == data["frame_id"] and data["frame_id"] in frames,
                f"transfer frame reference invalid: {transfer_id}")
        require(row["subject"] == data["from_port"], f"transfer subject mismatch: {transfer_id}")
        require((data["from_port"], data["to_port"]) in directions,
                f"transfer direction not in topology: {transfer_id}")
        require(data["status"] in transfer_states, f"invalid transfer status: {transfer_id}")
        queued = require_d(data["queued_ps"], f"{transfer_id}.queued_ps")
        require(queued <= sim_end, f"offer time exceeds simulation end: {transfer_id}")
        for key in actual_times + planned_times:
            value = data[key]
            if value is not None:
                number = require_d(value, f"{transfer_id}.{key}")
                if key in actual_times:
                    require(number <= sim_end, f"actual transfer time exceeds end: {transfer_id}.{key}")
                    if is_time_limit:
                        require(number < sim_end, f"actual transfer event at time limit: {transfer_id}.{key}")
        require(data["status"] != "dropped" or data["drop_reason"] == "queue_full",
                f"dropped transfer reason mismatch: {transfer_id}")
        require(data["status"] == "dropped" or data["drop_reason"] is None,
                f"unexpected transfer drop reason: {transfer_id}")
        sof = data["sof_ps"]
        if sof is None:
            require(data["status"] in {"queued", "dropped"},
                    f"transfer without SOF has active status: {transfer_id}")
            require(all(data[key] is None for key in planned_times),
                    f"planned transfer times exist before SOF: {transfer_id}")
        else:
            sof_ps = require_d(sof, f"{transfer_id}.sof_ps")
            require(queued <= sof_ps, f"SOF precedes offer: {transfer_id}")
            rate = int(directions[(data["from_port"], data["to_port"])]["bitrate_bps"])
            delay = int(directions[(data["from_port"], data["to_port"])]["delay_ps"])
            frame_data = frames[data["frame_id"]]["data"]
            mac_bytes = int(frame_data["mac_bytes"])
            expected_eof = sof_ps + ceil_div((mac_bytes + 8) * 8 * 10**12, rate)
            expected_release = sof_ps + ceil_div((mac_bytes + 20) * 8 * 10**12, rate)
            require(data["planned_eof_ps"] == str(expected_eof), f"planned EOF mismatch: {transfer_id}")
            require(data["planned_release_ps"] == str(expected_release), f"planned release mismatch: {transfer_id}")
            require(data["planned_arrival_ps"] == str(expected_eof + delay),
                    f"planned arrival mismatch: {transfer_id}")
            for actual, planned in (("eof_ps", "planned_eof_ps"),
                                    ("release_ps", "planned_release_ps"),
                                    ("arrival_ps", "planned_arrival_ps")):
                if data[actual] is not None:
                    require(data[actual] == data[planned],
                            f"actual/planned {actual} mismatch: {transfer_id}")
        if data["eof_ps"] is not None:
            require(data["sof_ps"] is not None
                    and require_d(data["sof_ps"], f"{transfer_id}.sof_ps")
                    <= require_d(data["eof_ps"], f"{transfer_id}.eof_ps"),
                    f"EOF ordering invalid: {transfer_id}")
            require(data["status"] == "serialized", f"EOF transfer is not serialized: {transfer_id}")
        if data["release_ps"] is not None:
            require(data["eof_ps"] is not None
                    and require_d(data["eof_ps"], f"{transfer_id}.eof_ps")
                    <= require_d(data["release_ps"], f"{transfer_id}.release_ps"),
                    f"release ordering invalid: {transfer_id}")
        if data["arrival_ps"] is not None:
            require(data["eof_ps"] is not None
                    and require_d(data["eof_ps"], f"{transfer_id}.eof_ps")
                    <= require_d(data["arrival_ps"], f"{transfer_id}.arrival_ps"),
                    f"arrival ordering invalid: {transfer_id}")
        parent_id = data["parent_transfer_id"]
        if parent_id is None:
            require(row["record_id"] == f"{data['frame_id']}@{data['from_port']}",
                    f"source transfer ID mismatch: {transfer_id}")
            require(data["from_port"].rsplit(".", 1)[0] == frames[data["frame_id"]]["data"]["source"],
                    f"source transfer owner mismatch: {transfer_id}")
        else:
            require(parent_id in transfers, f"missing parent transfer: {transfer_id}")
            parent = transfers[parent_id]["data"]
            require(parent["frame_id"] == data["frame_id"], f"parent frame mismatch: {transfer_id}")
            children_by_parent.setdefault(parent_id, []).append(transfer_id)
        require(row["record_id"] == f"{data['frame_id']}@{data['from_port']}",
                f"transfer ID mismatch: {transfer_id}")

    for reception_id, row in receptions.items():
        data = row["data"]
        transfer_id = data["transfer_id"]
        require(transfer_id in transfers, f"reception transfer reference invalid: {reception_id}")
        transfer = transfers[transfer_id]["data"]
        require(data["frame_id"] == transfer["frame_id"] == row["request_id"],
                f"reception frame reference invalid: {reception_id}")
        require(reception_id == f"{transfer_id}@rx", f"reception ID mismatch: {reception_id}")
        require(transfer["arrival_ps"] is not None and data["observed_ps"] == transfer["arrival_ps"],
                f"reception lacks matching arrival: {reception_id}")
        require(data["ingress"] == transfer["to_port"], f"reception ingress mismatch: {reception_id}")
        device_id = data["ingress"].rsplit(".", 1)[0]
        require(row["subject"] == device_id and device_id in devices,
                f"reception subject mismatch: {reception_id}")
        require(data["status"] in {"processing", "received", "filtered", "forwarded"},
                f"invalid reception status: {reception_id}")
        require(data["reason"] in {None, "destination_mismatch", "same_ingress"},
                f"invalid reception reason: {reception_id}")
        observed = require_d(data["observed_ps"], f"{reception_id}.observed_ps")
        require(observed <= sim_end, f"reception time exceeds simulation end: {reception_id}")
        for key in ("ready_ps", "planned_ready_ps"):
            if data[key] is not None:
                ready = require_d(data[key], f"{reception_id}.{key}")
                if key == "ready_ps":
                    require(ready <= sim_end, f"reception ready exceeds simulation end: {reception_id}")
                    require(not is_time_limit or ready < sim_end,
                            f"reception ready event at time limit: {reception_id}")
        if data["status"] == "processing":
            require(data["ready_ps"] is None and data["planned_ready_ps"] is not None,
                    f"processing reception readiness mismatch: {reception_id}")
        elif data["status"] in {"received", "forwarded"}:
            require(data["ready_ps"] is not None and data["planned_ready_ps"] == data["ready_ps"],
                    f"completed reception readiness mismatch: {reception_id}")
        elif data["status"] == "filtered":
            require(data["reason"] is not None and data["ready_ps"] == data["observed_ps"]
                    and data["planned_ready_ps"] is None,
                    f"immediate filter readiness mismatch: {reception_id}")
        if data["status"] == "forwarded":
            require(bool(data["egress_transfer_ids"]),
                    f"forwarded reception has no child transfers: {reception_id}")
        if data["status"] in {"received", "filtered"}:
            require(data["egress_transfer_ids"] == [],
                    f"terminal reception has child transfers: {reception_id}")
        child_ids = data["egress_transfer_ids"]
        require(isinstance(child_ids, list) and all(isinstance(child, str) for child in child_ids),
                f"invalid egress transfer list: {reception_id}")
        actual_children = sorted(children_by_parent.get(transfer_id, []))
        require(sorted(child_ids) == actual_children,
                f"reception/child transfer references differ: {reception_id}")
        for child_id in child_ids:
            require(transfers[child_id]["data"]["parent_transfer_id"] == transfer_id,
                    f"child parent reference mismatch: {child_id}")
        receptions_by_transfer[transfer_id] = row

    arrived_ids = {transfer_id for transfer_id, row in transfers.items()
                   if row["data"]["arrival_ps"] is not None}
    require(set(receptions_by_transfer) == arrived_ids,
            "arrival count/reference conservation failed")
    for transfer_id, child_ids in children_by_parent.items():
        parent_reception = receptions_by_transfer.get(transfer_id)
        require(parent_reception is not None
                and parent_reception["data"]["status"] == "forwarded",
                f"child transfer parent reception was not forwarded: {transfer_id}")
        ready_ps = parent_reception["data"]["ready_ps"]
        require(ready_ps is not None, f"forwarded parent has no ready time: {transfer_id}")
        require(all(transfers[child]["data"]["queued_ps"] == ready_ps for child in child_ids),
                f"child offer does not match parent ready time: {transfer_id}")
    expected = scenario["expected"]
    actual = dict(
        generated=len(frames), offered=len(transfers), receptions=len(receptions),
        queued=sum(row["data"]["status"] == "queued" for row in transfers.values()),
        transmitting=sum(row["data"]["status"] == "transmitting" for row in transfers.values()),
        serialized=sum(row["data"]["status"] == "serialized" for row in transfers.values()),
        dropped=sum(row["data"]["status"] == "dropped" for row in transfers.values()),
        received=sum(row["data"]["status"] == "received" for row in receptions.values()),
        filtered=sum(row["data"]["status"] == "filtered" for row in receptions.values()),
        forwarded=sum(row["data"]["status"] == "forwarded" for row in receptions.values()),
        processing=sum(row["data"]["status"] == "processing" for row in receptions.values()),
    )
    for key, value in actual.items():
        if key in expected:
            require(value == expected[key], f"{scenario['name']} {key}: {value} != {expected[key]}")
    require(actual["offered"] == sum(actual[state] for state in
            ("queued", "transmitting", "serialized", "dropped")),
            "transfer state conservation failed")
    if "drop_ids" in expected:
        require(sorted(row["record_id"] for row in transfers.values()
                       if row["data"]["status"] == "dropped") == sorted(expected["drop_ids"]),
                f"{scenario['name']} dropped IDs mismatch")
    if "transfer_times" in expected:
        for golden in expected["transfer_times"]:
            require(golden["id"] in transfers, f"missing expected transfer: {golden['id']}")
            actual_data = transfers[golden["id"]]["data"]
            for key, value in golden.items():
                if key != "id":
                    require(actual_data[key] == value,
                            f"{scenario['name']} {golden['id']} {key} mismatch")
    received = [row for row in receptions.values() if row["data"]["status"] == "received"]
    for delivery in expected.get("deliveries", []):
        matches = [row for row in received if row["request_id"] == delivery["frame_id"]
                   and row["subject"] == delivery["node"]]
        require(len(matches) == 1 and matches[0]["data"]["ready_ps"] == delivery["received_ps"],
                f"{scenario['name']} delivery mismatch: {delivery}")
    if "delivery_ps" in expected:
        require(all(row["data"]["ready_ps"] == expected["delivery_ps"] for row in received),
                f"{scenario['name']} delivery time mismatch")
    if "filter_reason" in expected:
        require(all(row["data"]["reason"] == expected["filter_reason"]
                    for row in receptions.values() if row["data"]["status"] == "filtered"),
                f"{scenario['name']} filter reason mismatch")


def number_wire(value):
    if value == 0.0:
        return "0.0000000000000000e+0"
    mantissa, exponent = format(value, ".16e").split("e")
    return f"{mantissa}e{int(exponent):+d}"


def check_csv(path, rows, run_id, version, name):
    raw = path.read_bytes()
    require(not raw.startswith(b"\xef\xbb\xbf"), f"{name} has a UTF-8 BOM")
    require(raw.endswith(b"\n") and b"\r\n" not in raw, f"{name} must use LF and end in LF")
    text = raw.decode("utf-8")
    parsed = list(csv.reader(io.StringIO(text, newline="")))
    require(parsed and ",".join(parsed[0]) == CSV_HEADER, f"{name} header mismatch")
    expected_rows = [CSV_HEADER.split(",")]
    for row in rows:
        require(set(row) == RECORD_KEYS, f"{name} record field set mismatch")
        value = row["value"]
        if row["value_kind"] == "integer":
            require(value is None or D_RE.fullmatch(value) is not None,
                    f"{name} integer value is not D")
            value_cell = "" if value is None else value
        else:
            require(row["value_kind"] == "number", f"{name} has unknown value_kind")
            require(value is None or (isinstance(value, (int, float)) and not isinstance(value, bool)
                                      and math.isfinite(float(value))),
                    f"{name} number value is invalid")
            value_cell = "" if value is None else number_wire(float(value))
        projection = [
            str(version), run_id, row["seq"], row["event_seq"] or "",
            row["effect_seq"] or "", row["target"], row["metric"], row["unit"],
            row["value_kind"], value_cell, row["time_ps"] or "", row["start_ps"] or "",
            row["end_ps"] or "", row["request_id"] or "", row["receiver"] or "",
            row["reason"] or "", row["sample_count"] or "",
        ]
        expected_rows.append(projection)
    require(parsed == expected_rows, f"{name} does not project JSON rows exactly")


def check_manifest(path, result):
    manifest = read(path / "manifest.json")
    require(manifest.get("schema_version") == 2, "manifest schema_version mismatch")
    require(manifest.get("run_id") == result["run_id"], "manifest run_id mismatch")
    require(manifest.get("status") == "complete", "manifest is not complete")
    simulation = result["simulation"]
    require(manifest.get("termination") == simulation["termination"], "manifest termination mismatch")
    require(manifest.get("partial") == simulation["partial"], "manifest partial flag mismatch")
    files = manifest.get("files")
    require(isinstance(files, list), "manifest files must be an array")
    by_name = {entry.get("name"): entry for entry in files}
    require(len(by_name) == len(files) and set(by_name) == {
        "results.json", "events.csv", "summary.csv", "diagnostics.jsonl"
    }, "manifest file set mismatch")
    require({entry.name for entry in path.iterdir()} == set(by_name) | {"manifest.json"},
            "published output file set mismatch")
    for name, entry in by_name.items():
        raw = (path / name).read_bytes()
        require(set(entry) == {"name", "sha256", "bytes"}, f"manifest entry fields mismatch: {name}")
        require(entry["bytes"] == str(len(raw)), f"manifest byte count mismatch: {name}")
        require(entry["sha256"] == hashlib.sha256(raw).hexdigest(), f"manifest SHA256 mismatch: {name}")


def check_results(scenario, path):
    result = read(path)
    require(result.get("schema_version") == 2, "results schema_version must be 2")
    simulation = result.get("simulation")
    require(isinstance(simulation, dict), "simulation must be an object")
    require(set(simulation) == {
        "termination", "partial", "start_ps", "end_ps", "last_event_time_ps",
        "committed_events", "pending_events", "records", "summary", "model_records",
    }, "schema 2 Simulation field set mismatch")
    require(simulation["termination"] == scenario["expected"].get("termination", "events_exhausted"),
            f"{scenario['name']} termination mismatch")
    require(simulation["partial"] is False, f"{scenario['name']} should be a complete run")
    require_d(simulation["start_ps"], "simulation.start_ps")
    require(simulation["start_ps"] == "0", "simulation.start_ps must be zero")
    require_d(simulation["end_ps"], "simulation.end_ps")
    require_d(simulation["committed_events"], "simulation.committed_events")
    require_d(simulation["pending_events"], "simulation.pending_events")
    metadata = result.get("metadata")
    require(isinstance(metadata, dict), "metadata must be an object")
    rows = simulation["model_records"]
    check_model_records(rows, scenario, simulation, metadata)
    records, summary = simulation["records"], simulation["summary"]
    require(isinstance(records, list) and isinstance(summary, list), "records/summary must be arrays")
    for name, collection in (("records", records), ("summary", summary)):
        for index, row in enumerate(collection):
            require(row.get("seq") == str(index), f"{name} sequence is not contiguous")
            require(set(row) == RECORD_KEYS, f"{name} field set mismatch")
            descriptor = METRICS.get(row["metric"])
            require(descriptor is not None, f"unregistered metric in {name}: {row['metric']}")
            require((row["unit"], row["value_kind"]) == descriptor[:2],
                    f"metric unit/value_kind mismatch: {row['metric']}")
            sampling, aggregation = descriptor[2:]
            require((sampling != "point" or (name == "records" and row["time_ps"] is not None)),
                    f"point metric is not a record point: {row['metric']}")
            require((sampling != "summary" or (name == "summary" and row["time_ps"] is None)),
                    f"summary metric is not in summary: {row['metric']}")
            require((sampling != "window_summary" or row["time_ps"] is None),
                    f"window summary has a point time: {row['metric']}")
            require(row["metric"] == "ethernet.delivery_mean_ps"
                    or row["sample_count"] is None,
                    f"unexpected sample_count: {row['metric']}")
            if row["metric"] == "ethernet.delivery_mean_ps":
                require(row["sample_count"] is not None,
                        "delivery_mean_ps is missing its sample_count")
            if row["metric"] == "ethernet.delivery_ps":
                require(row["request_id"] is not None and row["receiver"] is not None,
                        "delivery point is missing request/receiver")
            require(aggregation in {"identity", "sum", "max", "time_mean", "occupancy_ratio", "rate", "sample_mean"},
                    f"unknown metric aggregation: {row['metric']}")
            require((row["time_ps"] is not None and row["start_ps"] is None and row["end_ps"] is None)
                    or (row["time_ps"] is None and row["start_ps"] is not None and row["end_ps"] is not None),
                    f"{name} time/window fields invalid at seq {index}")
            for key in ("time_ps", "start_ps", "end_ps", "sample_count"):
                if row[key] is not None:
                    require_d(row[key], f"{name}[{index}].{key}")
            if row["time_ps"] is not None:
                require_d(row["time_ps"], f"{name}[{index}].time_ps")
            if row["start_ps"] is not None:
                require_d(row["start_ps"], f"{name}[{index}].start_ps")
                require_d(row["end_ps"], f"{name}[{index}].end_ps")
                require(int(row["start_ps"]) <= int(row["end_ps"]),
                        f"{name} window is reversed at seq {index}")
            if row["value_kind"] == "integer":
                require(row["value"] is not None, f"{name}[{index}] integer value is null")
                require_d(row["value"], f"{name}[{index}].value")
            if row["value_kind"] == "number" and row["value"] is not None:
                require(isinstance(row["value"], (int, float)) and not isinstance(row["value"], bool)
                        and math.isfinite(float(row["value"])), f"{name}[{index}].value is invalid")
    require(path.parent.is_dir(), "result directory is missing")
    check_csv(path.parent / "events.csv", records, result["run_id"], 2, "events.csv")
    check_csv(path.parent / "summary.csv", summary, result["run_id"], 2, "summary.csv")
    check_manifest(path.parent, result)

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
