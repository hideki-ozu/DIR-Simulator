#!/usr/bin/env python3
"""Run the DIR CAN fixtures against the installed, unmodified FiCo CAN engine.

The input adapter reads source fixtures, never DIR simulation results. Results
are read only after both runs, for configuration checks and comparison. Native
FiCo completion includes intermission and is deliberately not called CAN EOF.
"""
from __future__ import annotations

import argparse
import configparser
import copy
import csv
import hashlib
import json
import re
import shutil
import subprocess
import sys
from collections import Counter
from datetime import datetime, timezone
from fractions import Fraction
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
FIXTURES = ROOT / "docs/verification/fixtures/can"
EXAMPLES = ROOT / "examples/can"
TIME_UNITS = {"ps": 1, "ns": 1000, "us": 10**6, "ms": 10**9, "s": 10**12}
RATE_UNITS = {"bps": 1, "bit/s": 1, "kbps": 1000, "Mbps": 10**6}


def quantity(text: str, units: dict[str, int]) -> int:
    match = re.fullmatch(r"\s*(\d+(?:\.\d+)?)\s*([A-Za-z/]+)\s*", text)
    if not match or match[2] not in units:
        raise ValueError(f"unsupported quantity: {text!r}")
    value = Fraction(match[1]) * units[match[2]]
    if value.denominator != 1:
        raise ValueError(f"quantity is not an integer in base units: {text!r}")
    return value.numerator


def unquote(text: str) -> str:
    return json.loads(text) if text.startswith('"') else text


def read_ini(path: Path) -> configparser.ConfigParser:
    config = configparser.ConfigParser(interpolation=None, strict=True)
    config.optionxform = str
    with path.open(encoding="utf-8") as stream:
        config.read_file(stream)
    return config


def settings(path: Path) -> dict:
    """Read only the documented CAN fixture subset; reject unknown settings."""
    config = read_ini(path)
    general = dict(config["General"])
    if general.get("network") != "demo.Main":
        raise ValueError("comparison currently requires the documented demo.Main topology")
    ned = path.parent / unquote(general["ned-path"]) / "demo/Main.ned"
    ned_text = ned.read_text(encoding="utf-8")
    names = re.findall(r"(?m)^\s*(\w+)\s*:\s*demo\.Controller\s*;", ned_text)
    if names != ["a", "b", "c"]:
        raise ValueError("comparison requires the three-controller CAN fixture topology")
    controller = re.search(r"\bsimple\s+Controller\s*\{([^{}]*)\}", ned_text)
    controller_gates = re.search(r"\bgates\s*:(.*)", controller[1], re.DOTALL) if controller else None
    controller_declarations = re.findall(r"\b(input|output)\s+([A-Za-z_]\w*)\s*;", controller_gates[1]) if controller_gates else []
    if (sorted(controller_declarations) != [("input", "rx"), ("output", "tx")]
            or not controller_gates
            or re.sub(r"\b(input|output)\s+[A-Za-z_]\w*\s*;", "", controller_gates[1]).strip()):
        raise ValueError("comparison requires Controller tx output and rx input gates")
    bus = re.search(r"\bsimple\s+Bus\s*\{([^{}]*)\}", ned_text)
    gates = re.search(r"\bgates\s*:(.*)", bus[1], re.DOTALL) if bus else None
    declarations = re.findall(r"\b(input|output)\s+([A-Za-z_]\w*)\s*;", gates[1]) if gates else []
    inputs = {gate for direction, gate in declarations if direction == "input"}
    outputs = {gate for direction, gate in declarations if direction == "output"}
    if (len(declarations) != 6 or len(inputs) != 3 or len(outputs) != 3
            or inputs & outputs or not gates
            or re.sub(r"\b(input|output)\s+[A-Za-z_]\w*\s*;", "", gates[1]).strip()):
        raise ValueError("comparison requires three declared input/output CAN bus gate pairs")
    connections = re.findall(r"(\w+\.\w+)\s*-->\s*demo\.Wire\s*-->\s*(\w+\.\w+)\s*;", ned_text)
    tx_destinations = {}
    rx_sources = {}
    for source, target in connections:
        source_node, source_gate = source.split(".")
        target_node, target_gate = target.split(".")
        if (source_node in names and source_gate == "tx" and target_node == "bus"
                and target_gate in inputs and source_node not in tx_destinations):
            tx_destinations[source_node] = target
        elif (source_node == "bus" and source_gate in outputs and target_node in names
                and target_gate == "rx" and target_node not in rx_sources):
            rx_sources[target_node] = source
        else:
            raise ValueError("comparison requires the fixture's direct, bidirectional CAN bus wiring")
    if (len(connections) != 6 or ned_text.count("-->") != 12
            or set(tx_destinations) != set(names) or set(rx_sources) != set(names)
            or set(tx_destinations.values()) != {f"bus.{gate}" for gate in inputs}
            or set(rx_sources.values()) != {f"bus.{gate}" for gate in outputs}):
        raise ValueError("comparison requires the fixture's direct, bidirectional CAN bus wiring")
    defaults = {}
    for field in ("queueCapacity", "txProcessingDelay", "rxProcessingDelay", "rxFilter"):
        found = re.search(r"\b" + field + r"\s*(?:@unit\(s\)\s*)?=\s*default\((.*?)\)", ned_text)
        if not found:
            raise ValueError(f"missing NED default {field}")
        defaults[field] = found[1]
    wire_default = re.search(r"\bdelay\s*@unit\(s\)\s*=\s*default\((.*?)\)", ned_text)
    if not wire_default or quantity(wire_default[1], TIME_UNITS) != 0:
        raise ValueError("comparison requires zero-default fixture Wire channels")
    remaining = set(general) - {"network", "ned-path", "sim-time-limit", "metrics-window", "workload", "Main.bus.bitrate"}
    nodes = []
    channels_used = set()
    for name in names:
        label = f"Main.{name}"
        values = {}
        for field, default in defaults.items():
            key = f"{label}.{field}"
            values[field] = general.get(key, default)
            remaining.discard(key)
        delays = []
        for channel in (f"Channel Main::{name}.tx", f"Channel Main::{rx_sources[name]}"):
            delays.append(quantity(config[channel]["delay"], TIME_UNITS) if channel in config else 0)
            if channel in config:
                if set(config[channel]) != {"delay"}:
                    raise ValueError(f"unsupported channel settings: {channel}")
                channels_used.add(channel)
        rx_filter = unquote(values["rxFilter"])
        if rx_filter not in ("*", "none"):
            raise ValueError("comparison's fixture adapter currently requires '*' or 'none' rxFilter")
        nodes.append({"label": label, "queue_capacity": int(values["queueCapacity"]),
                      "tx_processing_ps": quantity(values["txProcessingDelay"], TIME_UNITS),
                      "rx_processing_ps": quantity(values["rxProcessingDelay"], TIME_UNITS),
                      "tx_channel_ps": delays[0], "rx_channel_ps": delays[1], "rx_filter": rx_filter})
    if remaining or set(config.sections()) - {"General"} - channels_used:
        raise ValueError(f"unmapped settings in {path}: {sorted(remaining)}")
    workload_path = path.parent / unquote(general["workload"])
    workload = json.loads(workload_path.read_text())
    return {"horizon_ps": quantity(general["sim-time-limit"], TIME_UNITS),
            "bitrate": quantity(general["Main.bus.bitrate"], RATE_UNITS), "nodes": nodes,
            "workload": workload, "ned_path": str(ned), "workload_path": str(workload_path)}


def offered_frames(condition: dict) -> list[dict]:
    """Expand raw explicit/periodic workload, preserving numeric ordinals."""
    workload = condition["workload"]
    if workload.get("schema_version") != 1:
        raise ValueError("unsupported workload schema")
    rows = []
    seen = set()
    for generator in sorted(workload["generators"], key=lambda item: item["id"]):
        ident = generator["id"]
        if ident in seen or any(char in ident for char in "\t\r\n"):
            raise ValueError("duplicate or unsupported generator identifier")
        seen.add(ident)
        if generator["node"] not in {node["label"] for node in condition["nodes"]}:
            raise ValueError("unknown source node")
        frame = generator["frame"]
        if frame["format"] not in ("standard", "extended"):
            raise ValueError("unsupported identifier format")
        payload = bytes.fromhex(frame["data"])
        if len(payload) > 8:
            raise ValueError("comparison supports Classical CAN data frames only")
        max_id = 0x7FF if frame["format"] == "standard" else 0x1FFFFFFF
        if not isinstance(frame["id"], int) or not 0 <= frame["id"] <= max_id:
            raise ValueError("invalid CAN identifier")
        if generator["kind"] == "can.explicit.v1":
            times = [quantity(value, TIME_UNITS) for value in generator["times"]]
        elif generator["kind"] == "can.periodic.v1":
            start = quantity(generator["start"], TIME_UNITS) + quantity(generator.get("phase", "0ps"), TIME_UNITS)
            period = quantity(generator["period"], TIME_UNITS)
            if period <= 0:
                raise ValueError("period must be positive")
            end = quantity(generator["end"], TIME_UNITS) if "end" in generator else condition["horizon_ps"]
            # Infinite periodic inputs need only the offered prefix inside H.
            count = generator.get("count", max(0, (end - start + period - 1) // period))
            if not isinstance(count, int) or not 0 <= count <= 1000000:
                raise ValueError("unsupported generator count")
            times = [start + ordinal * period for ordinal in range(count)
                     if "end" not in generator or start + ordinal * period < end]
        else:
            raise ValueError(f"unsupported generator kind: {generator['kind']}")
        for ordinal, at in enumerate(times):
            rows.append({"generated_ps": at, "request_id": f"{ident}:{ordinal}", "source": generator["node"],
                         "format": frame["format"], "can_id": frame["id"], "payload_hex": payload.hex(),
                         "generator_id": ident, "ordinal": ordinal})
    return sorted(rows, key=lambda row: (row["generated_ps"], row["generator_id"], row["ordinal"]))


def suite() -> list[dict]:
    canonical = json.loads((FIXTURES / "scenarios.json").read_text())["scenarios"]
    cases = [{"name": item["name"], "config": FIXTURES / item["config"], "group": "canonical",
              "reference": "docs/verification/fixtures/can/scenarios.json"} for item in canonical]
    cases += [{"name": name, "config": EXAMPLES / f"{name}.ini", "group": "example",
               "reference": f"examples/can/{name}.ini"} for name in ("baseline", "contention", "overload")]
    for name in ("mixed-format-extended-wins", "mixed-format-standard-wins", "zero-horizon", "empty-generators", "future-only"):
        cases.append({"name": name, "config": FIXTURES / "competition.ini", "group": "runtime-boundary",
                      "reference": "crates/dir-simulator/tests/can_scenarios.rs", "mutation": name})
    vectors = json.loads((FIXTURES / "vectors.json").read_text())["vectors"]
    for vector in vectors:
        for bitrate in (500000, 1000000, 333333):
            cases.append({"name": f"{vector['name']}-{bitrate}", "config": FIXTURES / "competition.ini",
                          "group": "frame-timing", "reference": "docs/verification/fixtures/can/vectors.json",
                          "vector": {key: vector[key] for key in ("name", "format", "id", "data")}, "bitrate": bitrate})
    return cases


def materialize(case: dict, destination: Path) -> Path:
    """Freeze raw inputs; variants reproduce Rust test mutations, never results."""
    destination.mkdir()
    config = read_ini(case["config"])
    source = settings(case["config"])
    workload = copy.deepcopy(source["workload"])
    mutation = case.get("mutation")
    if mutation and mutation.startswith("mixed-format"):
        workload["generators"][0]["frame"]["id"] = 0x123
        workload["generators"][1]["frame"].update(format="extended", id=0x123 if mutation.endswith("extended-wins") else 0x048C0000)
    elif mutation == "zero-horizon":
        config["General"]["sim-time-limit"] = "0ps"
    elif mutation == "empty-generators":
        workload["generators"] = []
    elif mutation == "future-only":
        for generator in workload["generators"]:
            generator["times"] = ["1ms"]
    if "vector" in case:
        frame = {key: case["vector"][key] for key in ("format", "id", "data")}
        workload = {"schema_version": 1, "generators": [{"id": "vector", "kind": "can.explicit.v1", "node": "Main.a", "times": ["0ps"], "frame": frame}]}
        config["General"]["sim-time-limit"] = "1ms"
        config["General"]["Main.bus.bitrate"] = f"{case['bitrate']}bps"
    config["General"]["workload"] = '"workload.json"'
    config["General"]["ned-path"] = '"models"'
    (destination / "models/demo").mkdir(parents=True)
    shutil.copyfile(source["ned_path"], destination / "models/demo/Main.ned")
    (destination / "workload.json").write_text(json.dumps(workload, indent=2) + "\n")
    path = destination / "scenario.ini"
    with path.open("w") as stream:
        config.write(stream)
    return path


def omnet_inputs(condition: dict, destination: Path) -> Path:
    destination.mkdir()
    offered = offered_frames(condition)
    lines = ["[General]", "network = dir.omnetcomparison.AdapterNetwork", "simtime-resolution = ps",
             "cmdenv-express-mode = true", "cmdenv-interactive = false", "record-eventlog = false",
             f"sim-time-limit = {condition['horizon_ps']}ps", f"*.horizonPs = {condition['horizon_ps']}",
             f"*.bitrate = {condition['bitrate']}", f"*.nodeCount = {len(condition['nodes'])}",
             f"*.outputFile = {json.dumps(str(destination / 'events.csv'))}",
             f"result-dir = {json.dumps(str(destination / 'results'))}"]
    keys = {"nodeLabel": "label", "queueCapacity": "queue_capacity", "txProcessingPs": "tx_processing_ps",
            "rxProcessingPs": "rx_processing_ps", "txChannelPs": "tx_channel_ps", "rxChannelPs": "rx_channel_ps", "rxFilter": "rx_filter"}
    for index, node in enumerate(condition["nodes"]):
        path = destination / f"node-{index}.tsv"
        with path.open("w", newline="") as stream:
            writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
            writer.writerow(["generation_ps", "request_id", "format", "can_id", "payload_hex"])
            for frame in offered:
                if frame["source"] == node["label"]:
                    writer.writerow([frame["generated_ps"], frame["request_id"], frame["format"], frame["can_id"], frame["payload_hex"] or "-"])
        lines.append(f"*.node[{index}].sourceFile = {json.dumps(str(path))}")
        for target, key in keys.items():
            lines.append(f"*.node[{index}].{target} = {json.dumps(node[key])}")
    path = destination / "omnetpp.ini"
    path.write_text("\n".join(lines) + "\n")
    (destination / "offered.json").write_text(json.dumps(offered, indent=2) + "\n")
    return path


def run(command: list[str], log: Path, cwd: Path = ROOT, env: dict | None = None) -> str:
    result = subprocess.run(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=180)
    log.write_text(result.stdout)
    if result.returncode:
        raise RuntimeError(f"command exited {result.returncode}; see {log}")
    return result.stdout


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ratio(numerator: int, denominator: int) -> dict | None:
    if denominator == 0:
        return None
    value = Fraction(numerator, denominator)
    return {"numerator": str(value.numerator), "denominator": str(value.denominator), "decimal": float(value)}


def queue_areas(samples: list[tuple[int, str, int]], nodes: list[str], horizon: int) -> dict:
    state = {node: [0, 0, 0, 0] for node in nodes}  # last time, length, area, peak
    for at, node, length in samples:
        last, previous, area, peak = state[node]
        if not last <= at <= horizon or length < 0:
            raise ValueError("invalid queue sample")
        state[node] = [at, length, area + (at - last) * previous, max(peak, length)]
    return {node: {"area_packet_ps": str(area + (horizon - last) * length), "mean": ratio(area + (horizon - last) * length, horizon),
                   "peak": peak, "final": length} for node, (last, length, area, peak) in state.items()}


def parse_dir(raw: dict, condition: dict) -> dict:
    simulation = raw["simulation"]
    requests = {}
    for row in simulation["requests"]:
        request = {"id": row["request_id"], "source": row["source"], "status": row["status"]}
        for key in ("generated_ps", "ready_ps", "sof_ps", "eof_ps"):
            request[key] = None if row[key] is None else int(row[key])
        for key in ("release_ps", "planned_eof_ps", "planned_release_ps"):
            value = row["model_fields"][key]
            request[key] = None if value is None else int(value)
        request["frame_bits"] = int(row["serialized_bits"])
        requests[request["id"]] = request
    receivers = [{"id": row["request_id"], "receiver": row["receiver"], "status": row["status"],
                  "observed_ps": None if row["observed_ps"] is None else int(row["observed_ps"]),
                  "received_ps": None if row["received_ps"] is None else int(row["received_ps"])} for row in simulation["receivers"]]
    samples = [(int(row["time_ps"]), row["target"][:-8], int(row["value"])) for row in simulation["records"] if row["metric"] == "queue_length"]
    return finish_projection(requests, receivers, samples, condition, "dir", simulation["termination"])


def parse_omnet(path: Path, condition: dict) -> tuple[dict, list[dict]]:
    with path.open(newline="") as stream:
        events = list(csv.DictReader(stream))
    validate_native_events(events, condition)
    requests, receivers, samples = {}, {}, []
    last_time = 0
    for row in events:
        at = int(row["time_ps"])
        if not last_time <= at < condition["horizon_ps"]:
            raise ValueError(f"nonchronological or out-of-horizon native observation: {row}")
        last_time = at
        kind, ident = row["event"], row["request_id"]
        if kind == "generated":
            if ident in requests:
                raise ValueError("duplicate generated request")
            requests[ident] = {"id": ident, "source": row["source"], "status": "processing", "generated_ps": at,
                               "ready_ps": None, "sof_ps": None, "native_complete_ps": None, "native_bits": int(row["native_bits"])}
        elif kind == "enqueued":
            requests[ident]["status"] = "pending"
        elif kind in ("ready", "dropped", "sof", "native_complete"):
            request = requests[ident]
            field = {"ready": "ready_ps", "dropped": "ready_ps", "sof": "sof_ps", "native_complete": "native_complete_ps"}[kind]
            request[field] = at
            request["status"] = {"ready": "pending", "dropped": "dropped", "sof": "in_flight", "native_complete": "success"}[kind]
        elif kind in ("native_rx_complete", "observed", "received", "filtered"):
            key = (ident, row["node"])
            receiver = receivers.setdefault(key, {"id": ident, "receiver": row["node"], "status": "pending", "native_rx_complete_ps": None,
                                                   "observed_ps": None, "received_ps": None})
            if kind == "native_rx_complete":
                receiver["native_rx_complete_ps"] = at
            elif kind == "observed":
                receiver["observed_ps"] = at
            elif kind == "received":
                receiver["received_ps"] = at
                receiver["status"] = "received"
            else:
                receiver["observed_ps"] = at
                receiver["status"] = "filtered"
        else:
            raise ValueError(f"unknown native event kind {kind!r}")
        if row["queue_waiting"]:
            samples.append((at, row["node"], int(row["queue_waiting"])))
    projection = finish_projection(requests, list(receivers.values()), samples, condition, "omnet", "see native run.log")
    return projection, events


def validate_native_events(events: list[dict], condition: dict) -> None:
    """Do not let dictionary aggregation hide corruption or duplicate delivery."""
    offered = {row["request_id"]: row for row in offered_frames(condition)}
    nodes = {node["label"]: node for node in condition["nodes"]}
    stages, rx_stages, times, rx_times, bits = {}, {}, {}, {}, {}
    waiting = {node: set() for node in nodes}
    previous_time = 0
    allowed = {"generated": None, "ready": "generated", "enqueued": "ready", "dropped": "ready", "sof": "enqueued", "native_complete": "sof"}
    rx_allowed = {"native_rx_complete": None, "observed": "native_rx_complete", "received": "observed", "filtered": "observed"}
    for row in events:
        kind, ident, node = row["event"], row["request_id"], row["node"]
        at = int(row["time_ps"])
        if not previous_time <= at < condition["horizon_ps"]:
            raise ValueError("native event time is nonchronological or outside [0,H)")
        previous_time = at
        if ident not in offered or node not in nodes:
            raise ValueError("native event has an unknown request or node")
        original = offered[ident]
        if (row["source"] != original["source"] or row["format"] != original["format"]
                or int(row["can_id"]) != original["can_id"] or row["payload_hex"] != original["payload_hex"]):
            raise ValueError(f"native immutable frame identity changed: {ident} / {kind}")
        native_bits = int(row["native_bits"])
        if native_bits <= 0 or (ident in bits and bits[ident] != native_bits):
            raise ValueError(f"native frame length changed: {ident}")
        bits[ident] = native_bits
        if kind in allowed:
            if node != original["source"] or stages.get(ident) != allowed[kind]:
                raise ValueError(f"duplicate or invalid native request transition: {ident} / {kind}")
            stages[ident] = kind
            times.setdefault(ident, {})[kind] = at
            if kind == "generated" and at != original["generated_ps"]:
                raise ValueError(f"native generation time differs from workload: {ident}")
            if kind == "ready" and at != original["generated_ps"] + nodes[node]["tx_processing_ps"]:
                raise ValueError(f"native TX processing delay differs from input: {ident}")
            if kind == "enqueued":
                waiting[node].add(ident)
            elif kind == "sof":
                waiting[node].remove(ident)
        elif kind in rx_allowed:
            key = (ident, node)
            if node == original["source"] or "sof" not in times.get(ident, {}) or rx_stages.get(key) != rx_allowed[kind]:
                raise ValueError(f"duplicate or invalid native receiver transition: {ident} / {node} / {kind}")
            rx_stages[key] = kind
            rx_times.setdefault(key, {})[kind] = at
        else:
            raise ValueError(f"unknown native event kind: {kind}")
        if row["queue_waiting"] and int(row["queue_waiting"]) != len(waiting[node]):
            raise ValueError(f"native pending queue observation disagrees with event history: {node}")
    # Completed native transfers must really reach every non-source port.
    # A missing row, even when all model counts happened to agree, is an error.
    for ident, history in times.items():
        original = offered[ident]
        ready = original["generated_ps"] + nodes[original["source"]]["tx_processing_ps"]
        if ready < condition["horizon_ps"] and stages[ident] in ("generated", "ready"):
            raise ValueError(f"missing native admission outcome: {ident}")
        complete = history.get("native_complete")
        if complete is None:
            if any(key[0] == ident for key in rx_times):
                raise ValueError(f"native receive without completed transfer: {ident}")
            continue
        for node, properties in nodes.items():
            if node == original["source"]:
                continue
            rx = rx_times.get((ident, node), {})
            if rx.get("native_rx_complete") != complete:
                raise ValueError(f"missing or mistimed native port completion: {ident} / {node}")
            observe = complete + nodes[original["source"]]["tx_channel_ps"] + properties["rx_channel_ps"]
            if observe < condition["horizon_ps"]:
                if rx.get("observed") != observe:
                    raise ValueError(f"native observation delay differs: {ident} / {node}")
                if properties["rx_filter"] == "none":
                    if rx.get("filtered") != observe:
                        raise ValueError(f"native filter outcome differs: {ident} / {node}")
                elif properties["rx_filter"] == "*":
                    receive = observe + properties["rx_processing_ps"]
                    if "filtered" in rx or (receive < condition["horizon_ps"] and rx.get("received") != receive):
                        raise ValueError(f"native RX processing outcome differs: {ident} / {node}")


def finish_projection(requests: dict, receivers: list[dict], samples: list, condition: dict, engine: str, termination: str) -> dict:
    ordered = sorted((row for row in requests.values() if row["sof_ps"] is not None), key=lambda row: row["sof_ps"])
    counts = Counter(row["status"] for row in requests.values())
    rx_counts = Counter(row["status"] for row in receivers)
    horizon = condition["horizon_ps"]
    end_key = "release_ps" if engine == "dir" else "native_complete_ps"
    occupied = sum((row[end_key] if row[end_key] is not None else horizon) - row["sof_ps"] for row in ordered)
    waits = [row["sof_ps"] - row["generated_ps"] for row in ordered]
    delivery = [row["received_ps"] - requests[row["id"]]["generated_ps"] for row in receivers if row["received_ps"] is not None]
    result = {"counts": {"generated": len(requests), "attempts": len(ordered), **{state: counts[state] for state in ("processing", "pending", "in_flight", "success", "dropped")},
                         "received": rx_counts["received"], "filtered": rx_counts["filtered"], "rx_pending": rx_counts["pending"]},
              "sof_order": [row["id"] for row in ordered], "occupied_ps": str(occupied), "occupancy": ratio(occupied, horizon),
              "tx_wait_mean_ps": ratio(sum(waits), len(waits)), "delivery_mean_ps": ratio(sum(delivery), len(delivery)),
              "queues": queue_areas(samples, [node["label"] for node in condition["nodes"]], horizon),
              "requests": requests, "receivers": sorted(receivers, key=lambda row: (row["id"], row["receiver"])), "termination": termination}
    return result


def check_inputs(condition: dict, raw_dir: dict, native_events: list[dict]) -> list[str]:
    """Validate the independent adapter against raw offered inputs after both runs."""
    expected = {row["request_id"]: row for row in offered_frames(condition) if row["generated_ps"] < condition["horizon_ps"]}
    errors = []
    dir_requests = {row["request_id"]: row for row in raw_dir["simulation"]["requests"]}
    native_generated = {row["request_id"]: row for row in native_events if row["event"] == "generated"}
    for label, actual in (("DIR", dir_requests), ("OMNeT", native_generated)):
        if set(actual) != set(expected):
            errors.append(f"{label}: generated request IDs differ from raw workload")
        for ident in set(actual) & set(expected):
            row, wanted = actual[ident], expected[ident]
            at = int(row["generated_ps"] if label == "DIR" else row["time_ps"])
            if at != wanted["generated_ps"] or row["source"] != wanted["source"]:
                errors.append(f"{label}: generation/source mismatch: {ident}")
            if label == "OMNeT" and (row["format"] != wanted["format"] or int(row["can_id"]) != wanted["can_id"] or row["payload_hex"].replace("-", "") != wanted["payload_hex"]):
                errors.append(f"OMNeT: raw frame mismatch: {ident}")
    actual_config = {row["key"]: row["value"] for row in raw_dir["metadata"]["config"]}
    if quantity(actual_config["Main.bus.bitrate"], RATE_UNITS) != condition["bitrate"]:
        errors.append("DIR resolved bitrate differs from raw configuration")
    if int(raw_dir["simulation"]["end_ps"]) != condition["horizon_ps"]:
        errors.append("DIR horizon differs from raw configuration")
    for node in condition["nodes"]:
        prefix = node["label"] + "."
        if int(actual_config[prefix + "queueCapacity"]) != node["queue_capacity"]:
            errors.append(f"DIR resolved queue capacity differs: {node['label']}")
        for field, key in (("txProcessingDelay", "tx_processing_ps"), ("rxProcessingDelay", "rx_processing_ps"),
                           ("txChannelDelay", "tx_channel_ps"), ("rxChannelDelay", "rx_channel_ps")):
            if quantity(actual_config[prefix + field], TIME_UNITS) != node[key]:
                errors.append(f"DIR resolved {field} differs: {node['label']}")
        if unquote(actual_config[prefix + "rxFilter"]) != node["rx_filter"]:
            errors.append(f"DIR resolved receive filter differs: {node['label']}")
    return errors


def compare_projections(directory: Path, left: dict, right: dict) -> dict:
    count_deltas = {key: right["counts"][key] - value for key, value in left["counts"].items()}
    differences = []
    trace_fields = ("generated_ps", "ready_ps", "sof_ps", "status")
    for ident in sorted(set(left["requests"]) | set(right["requests"])):
        a, b = left["requests"].get(ident, {}), right["requests"].get(ident, {})
        for field in trace_fields:
            if a.get(field) != b.get(field):
                differences.append({"request_id": ident, "field": field, "dir": a.get(field), "omnet": b.get(field)})
        if a.get("release_ps") != b.get("native_complete_ps"):
            differences.append({"request_id": ident, "field": "bus_release_ps (DIR release / FiCo native_complete)",
                                "dir": a.get("release_ps"), "omnet": b.get("native_complete_ps")})
    left_rx = {(row["id"], row["receiver"]): row for row in left["receivers"]}
    right_rx = {(row["id"], row["receiver"]): row for row in right["receivers"]}
    for key in sorted(set(left_rx) | set(right_rx)):
        a, b = left_rx.get(key, {}), right_rx.get(key, {})
        for field in ("observed_ps", "received_ps", "status"):
            if a.get(field) != b.get(field):
                differences.append({"request_id": key[0], "receiver": key[1], "field": field, "dir": a.get(field), "omnet": b.get(field)})
    # EOF is not given a false one-to-one mapping to native completion. Export
    # both timestamps plus release, which bounds the corresponding bus occupancy.
    with (directory / "requests.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["request_id", "source", "dir_status", "omnet_status", "generated_ps", "dir_sof_ps", "omnet_sof_ps",
                         "dir_eof_ps", "dir_release_ps", "omnet_native_complete_ps", "dir_frame_bits", "omnet_bits_including_ifs"])
        for ident in sorted(set(left["requests"]) | set(right["requests"])):
            a, b = left["requests"].get(ident, {}), right["requests"].get(ident, {})
            writer.writerow([ident, a.get("source", b.get("source")), a.get("status"), b.get("status"), a.get("generated_ps", b.get("generated_ps")),
                             a.get("sof_ps"), b.get("sof_ps"), a.get("eof_ps"), a.get("release_ps"), b.get("native_complete_ps"), a.get("frame_bits"), b.get("native_bits")])
    occupancy_equal = left["occupied_ps"] == right["occupied_ps"]
    queue_equal = left["queues"] == right["queues"]
    summary = {"count_deltas_omnet_minus_dir": count_deltas, "counts_equal": not any(count_deltas.values()),
               "sof_order_equal": left["sof_order"] == right["sof_order"], "common_trace_differences": len(differences),
               "occupancy_equal": occupancy_equal, "queues_equal": queue_equal,
               "common_projection_equal": not any(count_deltas.values()) and not differences and occupancy_equal and queue_equal,
               "differences": differences}
    (directory / "comparison.json").write_text(json.dumps({"dir": left, "omnet": right, **summary}, indent=2) + "\n")
    return {key: value for key, value in summary.items() if key != "differences"}


def provenance(workspace: Path, binary: Path) -> dict:
    files = [binary, ROOT / "Cargo.lock", workspace / "sources.lock.json",
             workspace / "upstream/FiCo4OMNeT/src/libFiCo4OMNeT.so"]
    files += sorted((ROOT / "crates/dir-simulator/src").rglob("*.rs"))
    files += sorted((HERE / "model").glob("*"))
    files += [Path(__file__)]
    fico = workspace / "upstream/FiCo4OMNeT/src/fico4omnet"
    files += [fico / "linklayer/can/CanFrameTiming.h", fico / "bus/can/CanBusLogic.cc",
              fico / "buffer/can/CanOutputBuffer.cc", fico / "linklayer/can/CanPortInput.cc"]
    library = workspace / "upstream/FiCo4OMNeT/src/libFiCo4OMNeT.so"
    return {"captured_utc": datetime.now(timezone.utc).isoformat(), "omnet_workspace": str(workspace), "dir_binary": str(binary),
            "files_sha256": {str(path): digest(path) for path in files if path.is_file()},
            "loaded_library_note": {"library_sha256": digest(library), "source_binary_correspondence_verified": False,
                                    "meaning": "The installed library was executed unchanged. Available source/header hashes identify files present during comparison; they do not certify the build provenance of the installed binary."},
            "dir_git_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
            "dir_git_status": subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True),
            "fico_git_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=workspace / "upstream/FiCo4OMNeT", text=True).strip()}


EXCLUSIONS = [
    {"scope": "DIR max-events / delta-cycle / committed-prefix failure tests", "reason": "内部イベントの粒度とトランザクション境界が異なるため、同じイベント数を同じ物理条件として扱えない。"},
    {"scope": "u64::MAX overflow injection", "reason": "DIRはu64 ps、OMNeT++は符号付き64bit SimTime。入力範囲と失敗契約が異なる。"},
    {"scope": "CRC / stuffed bitstring conformance vectors", "reason": "同じ9フレームを3速度で実行するが、FiCoはCRC・内容依存stuff列を生成しないためcodec一致試験は成立しない。"},
    {"scope": "INI/NED parser, invalid-input, export/atomic-write, viewer, synthetic Snapshot aggregation tests", "reason": "DIRのAPI・ファイル・UI契約の試験であり、通信シミュレーションへの同一設定対応はない。"},
    {"scope": "GW / multibus CAN runtime scenarios", "reason": "gateway_compare.pyで複数のFiCo CANバスと外部Gatewayアダプターを使って別途実行・比較する。"},
    {"scope": "CAN FD / Ethernet / AXI / SoC / memory / IPC future specification fixtures", "reason": "現在のDIR製品実装はClassical CANとGW。未実装fixtureを実行済みと扱わない。"},
]


def markdown(report: dict) -> str:
    completed = [case for case in report["cases"] if "error" not in case]
    equal = sum(case["comparison"]["common_projection_equal"] for case in completed)
    lines = ["# DIR / OMNeT++ CAN 実行結果比較", "", f"実行日時（UTC）: {report['provenance']['captured_utc']}", "",
             f"{len(completed)} / {len(report['cases'])}ケースを両エンジンで実行。共通観測項目の一致は{equal}ケース、差異ありは{len(completed)-equal}ケース。",
             "これは同じ入力に対するモデル間比較であり、OMNeT++をDIRの正解値とする適合認証ではない。", "",
             "## 比較条件", "",
             "OMNeT++ 6.4 / ローカルFiCo4OMNeTの既存CANバス・ポート・仲裁・時間計算を使用。原ソースは変更せず、入力注入と観測用の外部テストモジュールを追加した。",
             "送信時刻、ID形式・値、payload全バイト、500k/1M/333333bps、3ノード、観測期間をDIRと共通化。キュー容量（送信中を除くdrop-tail）、TX/RX処理遅延、受信フィルタ、配送遅延はテストアダプターで補った。MOB=false、誤り注入なし、stuff率0。",
             "OMNeT入力は元INI/NED/workloadから独立に作成し、DIRの送信時刻・フレーム長・CRC・結果を入力へ転用していない。各ケースのinputs/、omnet/offered.json、omnet/omnetpp.iniに入力を保存。",
             "DIRの通常観測期間に合わせ[0,H)のみ記録。集計期間はイベント枯渇時も設定H。終了理由と内部イベント数は同一視しない。", "",
             "## 結果（DIR / OMNeT の順）", "",
             "successはDIRのEOF到達数とFiCoのnative完了数の対照。FiCo完了はIFS込みで、同じEOF定義ではない。", "",
             "| ケース | 生成 | 完了 | 破棄 | 受信 | SOF順序 | 共通観測 |", "|---|---:|---:|---:|---:|---|---|"]
    for case in report["cases"]:
        if "error" in case:
            lines.append(f"| {case['name']} | 実行失敗 | — | — | — | — | {case['error']} |")
            continue
        left, right = case["dir_counts"], case["omnet_counts"]
        cells = [f"{left[key]} / {right[key]}" for key in ("generated", "success", "dropped", "received")]
        lines.append(f"| [{case['name']}](cases/{case['name']}/comparison.json) | " + " | ".join(cells) +
                     f" | {'一致' if case['comparison']['sof_order_equal'] else '差異'} | {'一致' if case['comparison']['common_projection_equal'] else '差異'} |")
    lines += ["", "## モデルの意味が異なる項目", "",
              "- DIRは内容依存CRC-15とbit stuffingを計算。FiCoは47/67bit＋payload＋固定率stuffの近似で、設定率0では内容に依存しない。DIRのEOF基底は44/64bitで、別途3bitのintermissionを持つ。",
              "- FiCoはアイドル時の最初の仲裁を1bit時間待つ。DIRはready時刻で仲裁する。後続フレームの送信順や有限キューへの入場結果にも影響し得る。",
              "- FiCoのnative_complete・native_rx_completeはIFS込み。DIRのeof_psと同じ名前にせず、requests.csvにDIR EOF・releaseと並べた。バス占有率はDIRのSOF→releaseとFiCoのSOF→native_completeを同じHで積分する。",
              "- FiCo標準バッファには有限容量がない。今回の容量制限は明示したアダプターの追加機能で、FiCo単体の既定動作との比較ではない。",
              "- 比率は分子・分母もJSONに保存。時刻・件数の比較はps整数で実施し、グラフや丸め表示を一致判定に使わない。", "",
              "## 対応しないテスト", "", "| 範囲 | 理由 |", "|---|---|"]
    lines += [f"| {item['scope']} | {item['reason']} |" for item in EXCLUSIONS]
    lines += ["", "## 証跡と再実行", "",
              "- report.json: 全ケースの件数、入力一致確認、差分概要、SHA-256・実行版。",
              "- cases/<case>/dir/results.json: DIRの実測結果。",
              "- cases/<case>/omnet/events.csv・run.log・results/: OMNeT++の実測イベントと標準結果。",
              "- cases/<case>/comparison.json・requests.csv: 全要求・全受信・キュー積分・時刻差分。", "",
              "```bash", "python3 tools/omnet_comparison/compare.py --omnet-workspace /home/hideki/Omnet++ --output tmp/omnet-comparison-new", "```", "",
              "既存出力先への上書きは行わない。差異自体は実行失敗と区別し、実行エラーまたは入力対応の不整合があれば非ゼロ終了する。", "",
              "参照: [OMNeT++ event ordering / SimTime](https://doc.omnetpp.org/omnetpp/manual/#sec:simple-modules:simulation-time)、FiCoのローカルソースとハッシュはreport.json参照。", ""]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--omnet-workspace", type=Path, default=Path("/home/hideki/Omnet++"))
    parser.add_argument("--dir-binary", type=Path, default=ROOT / "target/release/dir-simulator")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append", help="run selected named case(s), default all")
    args = parser.parse_args()
    output, workspace, binary = args.output.resolve(), args.omnet_workspace.resolve(), args.dir_binary.resolve()
    if output.exists():
        parser.error("output already exists; use a new directory")
    selected = [case for case in suite() if not args.case or case["name"] in args.case]
    if not selected or (args.case and set(args.case) - {case["name"] for case in selected}):
        parser.error("unknown case name")
    output.mkdir(parents=True)
    (output / "cases").mkdir()
    import os
    environment = dict(os.environ, OMNET_WORKSPACE=str(workspace), BUILD_DIR=str(output / "build"))
    run(["bash", str(HERE / "model/build.sh")], output / "build.log", env=environment)
    report = {"schema_version": 1, "provenance": provenance(workspace, binary), "cases": [], "exclusions": EXCLUSIONS}
    version = run(["bash", "-c", 'source "$1/scripts/env.sh" && opp_run -h', "omnet-version", str(workspace)], output / "omnet-version.log")
    report["provenance"]["omnet_version_output"] = version
    for case in selected:
        directory = output / "cases" / case["name"]
        directory.mkdir()
        item = {"name": case["name"], "group": case["group"], "reference": case["reference"]}
        try:
            config = materialize(case, directory / "inputs")
            condition = settings(config)
            ini = omnet_inputs(condition, directory / "omnet")
            # Both independent inputs are complete before either result is read.
            run([str(binary), "run", "--config", str(config), "--output", str(directory / "dir")], directory / "dir.log")
            run(["bash", "-c", 'set -e; source "$1/scripts/env.sh"; exec opp_run -u Cmdenv -n "$2:$1/upstream/FiCo4OMNeT/src" -l "$1/upstream/FiCo4OMNeT/src/FiCo4OMNeT" -l "$3/DirOmnetAdapter" -f "$4"',
                 "omnet-comparison", str(workspace), str(HERE / "model"), str(output / "build"), str(ini)], directory / "omnet/run.log", cwd=directory / "omnet")
            raw = json.loads((directory / "dir/results.json").read_text())
            left = parse_dir(raw, condition)
            right, events = parse_omnet(directory / "omnet/events.csv", condition)
            errors = check_inputs(condition, raw, events)
            if errors:
                raise ValueError("; ".join(errors[:10]))
            item.update(input_correspondence_verified=True, condition={key: value for key, value in condition.items() if key not in ("workload", "ned_path", "workload_path")},
                        dir_counts=left["counts"], omnet_counts=right["counts"], comparison=compare_projections(directory, left, right),
                        input_sha256={str(path.relative_to(directory)): digest(path) for path in (directory / "inputs").rglob("*") if path.is_file()})
            print(f"{'EQUAL' if item['comparison']['common_projection_equal'] else 'DIFF'} {case['name']}: DIR {left['counts']['success']} / OMNeT {right['counts']['success']} completed", flush=True)
        except (ValueError, RuntimeError, KeyError, OSError, subprocess.TimeoutExpired) as error:
            item["error"] = str(error)
            print(f"ERROR {case['name']}: {error}", flush=True)
        report["cases"].append(item)
        (output / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        (output / "report.md").write_text(markdown(report), encoding="utf-8")
    return int(any("error" in case for case in report["cases"]))


if __name__ == "__main__":
    sys.exit(main())
