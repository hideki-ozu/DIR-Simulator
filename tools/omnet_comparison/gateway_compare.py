#!/usr/bin/env python3
"""Execute raw DIR Gateway inputs in OMNeT++/FiCo, without replaying DIR results.

This is an intentionally strict converter for the repository's documented
Classical CAN multibus fixtures. Native FiCo CAN timing is never compensated.
"""
from __future__ import annotations

import argparse
import copy
import csv
import json
import os
import re
import shutil
import subprocess
import sys
from collections import Counter, deque
from pathlib import Path

import compare as can

ROOT, HERE = can.ROOT, can.HERE
FIXTURES = ROOT / "docs/verification/fixtures/gw"
ROUTING_FIELDS = ["kind", "gateway", "ingress", "route_id", "egress", "format",
                  "id_min", "id_max", "processing_ps", "rx_capacity", "hop_limit"]


def sections(body: str) -> dict[str, str]:
    markers = list(re.finditer(r"\b(parameters|gates|submodules|connections)\s*:", body))
    prefix = body[:markers[0].start()] if markers else body
    if prefix.strip():
        raise ValueError("unsupported text before NED sections")
    result = {}
    for index, match in enumerate(markers):
        if match[1] in result:
            raise ValueError("duplicate NED section")
        result[match[1]] = body[match.end():markers[index + 1].start() if index + 1 < len(markers) else len(body)]
    return result


def ned_types(directory: Path) -> dict:
    definitions = {}
    for path in sorted(directory.rglob("*.ned")):
        text = re.sub(r"//[^\n]*|/\*.*?\*/", "", path.read_text(), flags=re.DOTALL)
        package = re.match(r"\s*package\s+([\w.]+)\s*;", text)
        if not package:
            raise ValueError(f"missing package: {path}")
        position = package.end()
        while text[position:].strip():
            match = re.match(r"\s*(simple|module|network|channel)\s+(\w+)\s*\{", text[position:])
            if not match:
                raise ValueError(f"unsupported NED definition: {path}")
            begin, cursor, depth = position + match.end(), position + match.end(), 1
            while cursor < len(text) and depth:
                if text[cursor] == "{":
                    depth += 1
                elif text[cursor] == "}":
                    depth -= 1
                cursor += 1
            if depth:
                raise ValueError("unbalanced NED definition")
            name = package[1] + "." + match[2]
            if name in definitions:
                raise ValueError(f"duplicate NED type: {name}")
            definitions[name] = {"kind": match[1], "sections": sections(text[begin:cursor - 1]), "path": path}
            position = cursor
    return definitions


def declarations(text: str, pattern: str) -> list[tuple]:
    matches = list(re.finditer(pattern, text))
    if re.sub(pattern, "", text).strip():
        raise ValueError(f"unmapped NED declaration: {text!r}")
    return [match.groups() for match in matches]


def gates(definition: dict) -> dict[str, str]:
    rows = declarations(definition["sections"].get("gates", ""), r"\b(input|output)\s+(\w+)\s*;")
    if len({row[1] for row in rows}) != len(rows):
        raise ValueError("duplicate NED gate")
    return {name: direction for direction, name in rows}


def uint(value, maximum=(1 << 32) - 1) -> int:
    if type(value) is not int or not 0 <= value <= maximum:
        raise ValueError(f"invalid nonnegative integer: {value!r}")
    return value


def read_settings(config_path: Path) -> dict:
    ini = can.read_ini(config_path)
    general = dict(ini["General"])
    if general.get("model-profile") != '"can.cc.multibus.v1"':
        raise ValueError("comparison requires the Classical CAN multibus profile")
    directory = config_path.parent / can.unquote(general["ned-path"])
    definitions = ned_types(directory)
    network_name = general["network"]
    network = definitions[network_name]
    if network["kind"] != "network" or set(network["sections"]) != {"submodules", "connections"}:
        raise ValueError("unsupported fixture network")
    label = network_name.rsplit(".", 1)[1]
    modules = declarations(network["sections"]["submodules"], r"\b(\w+)\s*:\s*([\w.]+)\s*;")
    endpoints, controller_defs, buses = {}, {}, {}
    for name, type_name in modules:
        definition = definitions[type_name]
        qualified = label + "." + name
        if definition["kind"] == "simple":
            params = definition["sections"].get("parameters", "")
            if '@class("dir.can.MultibusController")' in params:
                if gates(definition) != {"tx": "output", "rx": "input"}:
                    raise ValueError("unsupported Controller gate directions")
                controller_defs[qualified] = definition
                endpoints.update({qualified + ".tx": qualified + ".tx", qualified + ".rx": qualified + ".rx"})
            elif '@class("dir.can.MultibusBus")' in params:
                buses[qualified] = {"label": qualified, "gates": gates(definition), "definition": definition}
                for gate in buses[qualified]["gates"]:
                    endpoints[qualified + "." + gate] = qualified + "." + gate
            else:
                raise ValueError("unsupported simple module class")
        elif definition["kind"] == "module":
            sub = definition["sections"]
            if set(sub) != {"gates", "submodules", "connections"}:
                raise ValueError("unsupported Gateway container")
            children = declarations(sub["submodules"], r"\b(\w+)\s*:\s*([\w.]+)\s*;")
            for child, child_type in children:
                child_def = definitions[child_type]
                if '@class("dir.can.MultibusController")' not in child_def["sections"].get("parameters", ""):
                    raise ValueError("Gateway can contain only fixture Controllers")
                if gates(child_def) != {"tx": "output", "rx": "input"}:
                    raise ValueError("unsupported Gateway Controller gates")
                controller_defs[qualified + "." + child] = child_def
            used = set()
            for source, target in declarations(sub["connections"], r"\b([\w.]+)\s*-->\s*([\w.]+)\s*;"):
                external, inner = (source, target) if "." not in source else (target, source)
                if external in used or external not in gates(definition) or "." not in inner:
                    raise ValueError("unresolved Gateway wiring")
                child, child_gate = inner.split(".")
                if (qualified + "." + child not in controller_defs or
                        (gates(definition)[external], child_gate) != (("input", "rx") if external == source else ("output", "tx"))):
                    raise ValueError("wrong Gateway wiring direction")
                endpoints[qualified + "." + external] = qualified + "." + inner
                used.add(external)
            if used != set(gates(definition)) or len(used) != len(children) * 2:
                raise ValueError("incomplete Gateway wiring")
        else:
            raise ValueError("unsupported network child")
    connections = declarations(network["sections"]["connections"], r"\b([\w.]+)\s*-->\s*([\w.]+)\s*-->\s*([\w.]+)\s*;")
    tx, rx, used_bus_gates, used_channels = {}, {}, set(), set()
    for source, channel_type, target in connections:
        channel = definitions[channel_type]
        channel_params = channel["sections"].get("parameters", "")
        delay = re.search(r"\bdelay\s*@unit\(s\)\s*=\s*default\(([^)]+)\)", channel_params)
        if channel["kind"] != "channel" or not delay or can.quantity(delay[1], can.TIME_UNITS) != 0:
            raise ValueError("comparison requires zero-default fixed Wire channels")
        left, right = endpoints[label + "." + source], endpoints[label + "." + target]
        lnode, lgate = left.rsplit(".", 1)
        rnode, rgate = right.rsplit(".", 1)
        if lnode in controller_defs and lgate == "tx" and rnode in buses and buses[rnode]["gates"].get(rgate) == "input":
            controller, bus, mapping = lnode, rnode, tx
            bus_gate = right
        elif lnode in buses and buses[lnode]["gates"].get(lgate) == "output" and rnode in controller_defs and rgate == "rx":
            controller, bus, mapping = rnode, lnode, rx
            bus_gate = left
        else:
            raise ValueError("unmapped or wrong-direction multibus connection")
        if controller in mapping or bus_gate in used_bus_gates:
            raise ValueError("duplicate Controller or Bus connection")
        channel_section = f"Channel {label}::{source}"
        channel_delay = 0
        if channel_section in ini:
            if set(ini[channel_section]) != {"delay"}:
                raise ValueError("unsupported channel setting")
            channel_delay = can.quantity(ini[channel_section]["delay"], can.TIME_UNITS)
            used_channels.add(channel_section)
        mapping[controller] = (bus, channel_delay)
        used_bus_gates.add(bus_gate)
    if set(tx) != set(controller_defs) or set(rx) != set(controller_defs):
        raise ValueError("incomplete multibus Controller wiring")
    remaining = set(general) - {"network", "ned-path", "sim-time-limit", "metrics-window", "workload", "model-profile", "model-config"}
    nodes = []
    for name, definition in sorted(controller_defs.items()):
        if tx[name][0] != rx[name][0]:
            raise ValueError("Controller TX/RX attach to different buses")
        values = {}
        params = definition["sections"]["parameters"]
        for field in ("queueCapacity", "txProcessingDelay", "rxProcessingDelay", "rxFilter"):
            default = re.search(r"\b" + field + r"\s*(?:@unit\(s\)\s*)?=\s*default\(([^)]+)\)", params)
            if not default:
                raise ValueError(f"missing Controller default: {field}")
            key = name + "." + field
            values[field] = general.get(key, default[1])
            remaining.discard(key)
        rx_filter = can.unquote(values["rxFilter"])
        if rx_filter not in ("*", "none"):
            raise ValueError("unsupported multibus fixture filter")
        nodes.append({"label": name, "bus": tx[name][0], "queue_capacity": uint(int(values["queueCapacity"])),
                      "tx_processing_ps": can.quantity(values["txProcessingDelay"], can.TIME_UNITS),
                      "rx_processing_ps": can.quantity(values["rxProcessingDelay"], can.TIME_UNITS),
                      "tx_channel_ps": tx[name][1], "rx_channel_ps": rx[name][1], "rx_filter": rx_filter})
    bus_rows = []
    for name, bus in sorted(buses.items()):
        default = re.search(r"\bbitrate\s*@unit\(bps\)\s*=\s*default\(([^)]+)\)", bus["definition"]["sections"]["parameters"])
        key = name + ".bitrate"
        if not default:
            raise ValueError("missing Bus bitrate default")
        bitrate = can.quantity(general.get(key, default[1]), can.RATE_UNITS)
        if not bitrate:
            raise ValueError("zero bitrate")
        remaining.discard(key)
        members = [node["label"] for node in nodes if node["bus"] == name]
        if len(members) < 2 or {name + "." + gate for gate in bus["gates"]} != {gate for gate in used_bus_gates if gate.startswith(name + ".")}:
            raise ValueError("incomplete Bus gate wiring")
        bus_rows.append({"label": name, "bitrate": bitrate, "nodes": members})
    if remaining or set(ini.sections()) - {"General"} - used_channels:
        raise ValueError(f"unmapped configuration: {sorted(remaining)}")
    routing_path = config_path.parent / can.unquote(general["model-config"])
    routing = json.loads(routing_path.read_text(encoding="utf-8-sig"))
    if routing.get("schema_version") != 1 or set(routing) != {"schema_version", "gateways"}:
        raise ValueError("unsupported routing schema")
    gateways, owned = [], set()
    for item in sorted(routing["gateways"], key=lambda row: row["node"]):
        if set(item) - {"node", "ports", "routes", "rx_queue_capacity", "processing_delay", "hop_limit"}:
            raise ValueError("unknown Gateway field")
        ports = sorted(item["ports"])
        if len(set(ports)) != len(ports) or owned.intersection(ports) or any(port not in controller_defs for port in ports):
            raise ValueError("invalid Gateway ports")
        owned.update(ports)
        routes = []
        for route in sorted(item["routes"], key=lambda row: row["id"]):
            if set(route) != {"id", "ingress", "egress", "format", "id_min", "id_max"}:
                raise ValueError("unknown route fields")
            if (route["ingress"] not in ports or not route["egress"] or any(port not in ports or port == route["ingress"] for port in route["egress"])):
                raise ValueError("invalid route ports")
            maximum = 0x7ff if route["format"] == "standard" else 0x1fffffff
            if route["format"] not in ("standard", "extended"):
                raise ValueError("invalid route format")
            minimum, maximum_id = uint(route["id_min"], maximum), uint(route["id_max"], maximum)
            if minimum > maximum_id or len(set(route["egress"])) != len(route["egress"]):
                raise ValueError("invalid route range or duplicate egress")
            routes.append({**route, "egress": sorted(route["egress"])})
        gateways.append({"node": item["node"], "ports": ports, "routes": routes,
                         "rx_queue_capacity": uint(item.get("rx_queue_capacity", 64)),
                         "processing_ps": can.quantity(item.get("processing_delay", "0ps"), can.TIME_UNITS),
                         "hop_limit": uint(item.get("hop_limit", 16), 65535)})
        if gateways[-1]["hop_limit"] == 0:
            raise ValueError("zero Gateway hop limit")
    workload_path = config_path.parent / can.unquote(general["workload"])
    workload = json.loads(workload_path.read_text(encoding="utf-8-sig"))
    if workload.get("schema_version") != 2 or set(workload) != {"schema_version", "generators"}:
        raise ValueError("unsupported multibus workload")
    condition = {"horizon_ps": can.quantity(general["sim-time-limit"], can.TIME_UNITS), "nodes": nodes,
                 "buses": bus_rows, "gateways": gateways, "workload": {**workload, "schema_version": 1}}
    can.offered_frames(condition)
    return condition


def materialize(case: dict, destination: Path) -> Path:
    destination.mkdir()
    original = case["config"]
    ini = can.read_ini(original)
    general = ini["General"]
    shutil.copytree(original.parent / can.unquote(general["ned-path"]), destination / "models")
    routing = json.loads((original.parent / can.unquote(general["model-config"])).read_text(encoding="utf-8-sig"))
    workload = json.loads((original.parent / can.unquote(general["workload"])).read_text(encoding="utf-8-sig"))
    general.update({"ned-path": '"models"', "model-config": '"routing.json"', "workload": '"workload.json"'})
    mutation = case.get("mutation", "")
    gateway = routing["gateways"][0] if routing["gateways"] else None
    if mutation in {"rx-overflow", "gw-processing", "tx-processing", "fanout-held", "fanout-drained"}:
        gateway["rx_queue_capacity"] = 1
    if mutation == "rx-overflow" or mutation.startswith("fanout-"):
        workload["generators"][0]["times"] = [f"{n * 110000000}ps" for n in range(4)]
    if mutation == "queue-boundary":
        general["sim-time-limit"] = "524000000ps"
    if mutation in {"gw-processing", "tx-processing"}:
        general["sim-time-limit"] = "550000000ps"
        if mutation == "gw-processing":
            gateway["processing_delay"] = "500000000ps"
        else:
            general["Main.gw.b.txProcessingDelay"] = "500000000ps"
    if mutation.startswith("fanout-"):
        general["Multi.busB.bitrate"], general["Multi.gw.b.queueCapacity"] = "125kbps", "1"
        general["sim-time-limit"] = "500000000ps" if mutation == "fanout-held" else "1500000000ps"
    if mutation == "rx-zero":
        gateway["rx_queue_capacity"] = 0
    if mutation in {"default64", "max-rx"}:
        general["sim-time-limit"] = "8000000000ps"
        gateway["processing_delay"] = "10000000000ps"
        workload["generators"][0]["times"] = [f"{n * 110000000}ps" for n in range(65)]
        if mutation == "max-rx":
            gateway["rx_queue_capacity"] = (1 << 32) - 1
    if mutation in {"same-priority", "same-priority-permuted", "first-ready", "first-ready-permuted"}:
        general["Multi.busB.bitrate"], general["Multi.gw.b.queueCapacity"] = "125kbps", "1"
        general["sim-time-limit"] = "2000000000ps" if mutation.startswith("same-priority") else "3000000000ps"
        gateway["routes"][0]["egress"] = ["Multi.gw.b"]
        second = {"id": "cb", "ingress": "Multi.gw.c", "egress": ["Multi.gw.b"], "format": "standard", "id_min": 0, "id_max": 0}
        gateway["routes"].append(second)
        if mutation.startswith("same-priority"):
            # Rust constructs this INI with only busB overridden; busC is 500kbps.
            general.pop("Multi.busC.bitrate", None)
            workload["generators"] = [
                {"id": "z", "kind": "can.explicit.v1", "node": "Multi.src", "times": ["0ps"], "frame": {"format": "standard", "id": 0, "data": ""}},
                {"id": "a", "kind": "can.explicit.v1", "node": "Multi.sinkC", "times": ["0ps"], "frame": {"format": "standard", "id": 0, "data": ""}},
                {"id": "block", "kind": "can.explicit.v1", "node": "Multi.sinkB", "times": ["0ps"], "frame": {"format": "standard", "id": 1, "data": ""}}]
            # The Rust same-priority case retains default TX capacity64.
            general.pop("Multi.gw.b.queueCapacity", None)
        else:
            second.update(id_min=1, id_max=1)
            workload["generators"][0]["times"] = ["0ps", "110000000ps", "220000000ps"]
            other = copy.deepcopy(workload["generators"][0])
            other.update(id="cross", node="Multi.sinkC", times=["53000000ps", "163000000ps", "273000000ps"])
            other["frame"]["id"] = 1
            workload["generators"].append(other)
        if mutation.endswith("permuted"):
            gateway["ports"].reverse()
            gateway["routes"].reverse()
            workload["generators"].reverse()
    if mutation == "renamed-gates":
        for path in (destination / "models").rglob("*.ned"):
            text = path.read_text()
            for before, after in (("rx_a", "busInputOne"), ("rx_b", "busInputTwo"), ("tx_a", "busOutputOne"), ("tx_b", "busOutputTwo")):
                # Rename Bus declarations/endpoints, preserving Gateway external gates.
                text = re.sub(r"\b(bus[A-Z])\." + before + r"\b", r"\1." + after, text)
                text = re.sub(r"(simple\s+Bus\s*\{.*?)(\})", lambda match, old=before, new=after: match[1].replace(old, new) + match[2], text, flags=re.DOTALL)
            path.write_text(text)
        for section in list(ini.sections()):
            if section.startswith("Channel") and "busA.tx_b" in section:
                values = dict(ini[section]); ini.remove_section(section)
                ini[section.replace("busA.tx_b", "busA.busOutputTwo")] = values
            elif section.startswith("Channel") and "busB.tx_b" in section:
                values = dict(ini[section]); ini.remove_section(section)
                ini[section.replace("busB.tx_b", "busB.busOutputTwo")] = values
    if "horizon_ps" in case:
        general["sim-time-limit"] = str(case["horizon_ps"]) + "ps"
    (destination / "routing.json").write_text(json.dumps(routing, indent=2) + "\n")
    (destination / "workload.json").write_text(json.dumps(workload, indent=2) + "\n")
    path = destination / "scenario.ini"
    with path.open("w") as stream:
        ini.write(stream)
    return path


def suite() -> list[dict]:
    cases = [{"name": "gw-" + item["name"], "config": FIXTURES / item["config"], "reference": "all_gateway_fixtures_match_independent_expectations"}
             for item in json.loads((FIXTURES / "scenarios.json").read_text())["cases"] if "prepare_failure" not in item["expected"]]
    for mutation, fixture, reference in [
        ("queue-boundary", "queue", "gateway_rx_holds_full_tx_copy_until_sof_frees_a_slot"),
        ("rx-overflow", "queue", "gateway_rx_overflow_drops_newest_without_changing_source_success"),
        ("gw-processing", "queue", "gateway_rx_capacity_counts_both_gateway_and_tx_processing"),
        ("tx-processing", "queue", "gateway_rx_capacity_counts_both_gateway_and_tx_processing"),
        ("fanout-held", "multicast", "gateway_rx_fanout_keeps_only_unadmitted_egress_and_does_not_duplicate_copies"),
        ("fanout-drained", "multicast", "gateway_rx_fanout_keeps_only_unadmitted_egress_and_does_not_duplicate_copies"),
        ("rx-zero", "queue", "gateway_zero_rx_and_unmatched_receptions_have_defined_capacity_semantics"),
        ("rx-zero", "no-route", "gateway_zero_rx_and_unmatched_receptions_have_defined_capacity_semantics"),
        ("default64", "queue", "gateway_default_rx_capacity_accepts_64_and_drops_the_65th"),
        ("max-rx", "queue", "gateway_default_rx_capacity_accepts_64_and_drops_the_65th"),
        ("same-priority", "multicast", "same_priority_copies_follow_ready_commit_order_and_configuration_permutations"),
        ("same-priority-permuted", "multicast", "same_priority_copies_follow_ready_commit_order_and_configuration_permutations"),
        ("first-ready", "multicast", "gateway_same_egress_waits_follow_first_ready_order_across_ingresses"),
        ("first-ready-permuted", "multicast", "gateway_same_egress_waits_follow_first_ready_order_across_ingresses"),
        ("renamed-gates", "delay", "arbitrary_bus_gate_names_preserve_multibus_results_and_timing")]:
        cases.append({"name": f"gw-{fixture}-{mutation}", "config": FIXTURES / (fixture + ".ini"), "mutation": mutation, "reference": reference})
    cases.append({"name": "gw-buffered-fanout", "config": ROOT / "examples/gateway/buffered-fanout.ini", "reference": "examples/gateway/buffered-fanout.ini"})
    return cases


def omnet_inputs(condition: dict, destination: Path) -> Path:
    destination.mkdir()
    offered = can.offered_frames(condition)
    (destination / "offered.json").write_text(json.dumps(offered, indent=2) + "\n")
    nodes = condition["nodes"]
    with (destination / "routing.tsv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=ROUTING_FIELDS, delimiter="\t")
        writer.writeheader()
        for gateway in condition["gateways"]:
            base = {"gateway": gateway["node"], "processing_ps": gateway["processing_ps"], "rx_capacity": gateway["rx_queue_capacity"], "hop_limit": gateway["hop_limit"]}
            for port in gateway["ports"]:
                writer.writerow({**base, "kind": "port", "ingress": port})
            for route in gateway["routes"]:
                for egress in route["egress"]:
                    writer.writerow({**base, "kind": "route", "ingress": route["ingress"], "route_id": route["id"], "egress": egress,
                                     **{key: route[key] for key in ("format", "id_min", "id_max")}})
    lines = ["[General]", "network = dir.omnetcomparison.GatewayCase", "simtime-resolution = ps", "cmdenv-express-mode = true",
             "cmdenv-interactive = false", "record-eventlog = false", f"sim-time-limit = {condition['horizon_ps']}ps",
             f"*.horizonPs = {condition['horizon_ps']}", f"*.nodeCount = {len(nodes)}", '*.outputFile = "events.csv"', '*.routingFile = "routing.tsv"',
             'output-scalar-file = "results/native.sca"', 'output-vector-file = "results/native.vec"']
    for index, node in enumerate(nodes):
        with (destination / f"node-{index}.tsv").open("w", newline="") as stream:
            writer = csv.writer(stream, delimiter="\t")
            writer.writerow(["generation_ps", "request_id", "format", "can_id", "payload_hex"])
            for row in offered:
                if row["source"] == node["label"]:
                    writer.writerow([row["generated_ps"], row["request_id"], row["format"], row["can_id"], row["payload_hex"] or "-"])
        prefix = f"*.node[{index}]."
        lines += [prefix + 'nodeLabel = ' + json.dumps(node["label"]), prefix + f'sourceFile = "node-{index}.tsv"',
                  prefix + f"queueCapacity = {node['queue_capacity']}", prefix + f"txProcessingPs = {node['tx_processing_ps']}",
                  prefix + f"rxProcessingPs = {node['rx_processing_ps']}", prefix + f"txChannelPs = {node['tx_channel_ps']}",
                  prefix + f"rxChannelPs = {node['rx_channel_ps']}", prefix + "rxFilter = " + json.dumps(node["rx_filter"])]
    ned = ["package dir.omnetcomparison;", "import fico4omnet.bus.can.CanBus;", "network GatewayCase {", " parameters:",
           "  int nodeCount;", "  int horizonPs;", "  string outputFile;", "  string routingFile;", " submodules:",
           "  recorder: AdapterRecorder;", "  node[nodeCount]: AdapterNode;"]
    for index, bus in enumerate(condition["buses"]):
        ned += [f"  bus{index}: CanBus {{", "   parameters:", f"    bandwidth = {bus['bitrate']}bps;", "    bitStuffingPercentage = 0;",
                f"   gates: gate[{len(bus['nodes'])}];", "  }"]
    ned.append(" connections:")
    for index, bus in enumerate(condition["buses"]):
        for gate_index, label in enumerate(bus["nodes"]):
            node_index = next(i for i, node in enumerate(nodes) if node["label"] == label)
            ned.append(f"  node[{node_index}].gate <--> bus{index}.gate[{gate_index}];")
    ned.append("}")
    (destination / "models/dir/omnetcomparison").mkdir(parents=True)
    (destination / "models/dir/omnetcomparison/GatewayCase.ned").write_text("\n".join(ned) + "\n")
    ini = destination / "omnetpp.ini"
    ini.write_text("\n".join(lines) + "\n")
    return ini


def nullable(value):
    return None if value is None or value == "" else int(value)


def match_route(condition: dict, ingress: str, frame: dict):
    gateway = next((item for item in condition["gateways"] if ingress in item["ports"]), None)
    if gateway is None:
        return None, None
    matches = [route for route in gateway["routes"] if route["ingress"] == ingress and route["format"] == frame["format"]
               and route["id_min"] <= frame["can_id"] <= route["id_max"]]
    if len(matches) > 1:
        raise ValueError("ambiguous route in comparison input")
    return gateway, matches[0] if matches else None


def remaining_egress(buffer: dict, forwards: dict, requests: dict) -> list[str]:
    return sorted(row["egress"] for row in forwards.values() if row["parent_request_id"] == buffer["parent_request_id"]
                  and row["gateway"] == buffer["gateway"] and row["ingress"] == buffer["ingress"] and row["egress"] is not None
                  and row["status"] != "dropped" and (row["child_request_id"] is None or
                  (requests[row["child_request_id"]]["tx_enqueued_ps"] is None and requests[row["child_request_id"]]["status"] != "dropped")))


def expected_release(buffer: dict, forwards: dict, requests: dict):
    if buffer["status"] == "dropped":
        return None
    if remaining_egress(buffer, forwards, requests):
        return None
    times = []
    for forward in forwards.values():
        if (forward["parent_request_id"], forward["gateway"], forward["ingress"]) != (buffer["parent_request_id"], buffer["gateway"], buffer["ingress"]):
            continue
        if forward["egress"] is None:
            times.append(buffer["received_ps"])
        elif forward["status"] == "dropped":
            times.append(forward["forwarded_ps"])
        else:
            child = requests[forward["child_request_id"]]
            times.append(child["tx_enqueued_ps"] if child["tx_enqueued_ps"] is not None else child["ready_ps"])
    return max(times, default=buffer["received_ps"])


def summarize(requests: dict, receivers: list, forwards: dict, buffers: dict, tx_samples: list, rx_samples: list,
              condition: dict, engine: str) -> dict:
    nodes = {node["label"]: node for node in condition["nodes"]}
    states, rx_states = Counter(row["status"] for row in requests.values()), Counter(row["status"] for row in buffers.values())
    result = {"requests": requests, "receivers": sorted(receivers, key=lambda row: (row["id"], row["receiver"])),
              "forwards": forwards, "rx_buffers": buffers,
              "counts": {"generated": len(requests), "native": sum(row["parent_request_id"] is None for row in requests.values()),
                         "copies": sum(row["parent_request_id"] is not None for row in requests.values()),
                         **{state: states[state] for state in ("processing", "waiting_tx", "pending", "in_flight", "success", "dropped")},
                         "received": sum(row["status"] == "received" for row in receivers),
                         **{"rx_" + state: rx_states[state] for state in ("holding", "released", "dropped")}}, "buses": {}}
    end_key = "release_ps" if engine == "dir" else "native_complete_ps"
    horizon = condition["horizon_ps"]
    for bus in condition["buses"]:
        sent = sorted((row for row in requests.values() if row["bus"] == bus["label"] and row["sof_ps"] is not None),
                      key=lambda row: (row["sof_ps"], row["id"]))
        for previous, current in zip(sent, sent[1:]):
            if previous[end_key] is None or current["sof_ps"] < previous[end_key]:
                raise ValueError(f"overlapping transmissions on {bus['label']}")
        occupied = sum((row[end_key] if row[end_key] is not None else horizon) - row["sof_ps"] for row in sent)
        if not 0 <= occupied <= horizon:
            raise ValueError(f"invalid per-bus occupancy: {bus['label']}")
        result["buses"][bus["label"]] = {"sof_order": [row["id"] for row in sent], "occupied_ps": str(occupied),
                                          "occupancy": can.ratio(occupied, horizon)}
    result["tx_queues"] = can.queue_areas(tx_samples, list(nodes), horizon)
    ports = [port for gateway in condition["gateways"] for port in gateway["ports"]]
    result["rx_queues"] = can.queue_areas(rx_samples, ports, horizon)
    for buffer in buffers.values():
        buffer["remaining_egress"] = remaining_egress(buffer, forwards, requests)
        if buffer["status"] == "holding" and not buffer["remaining_egress"]:
            raise ValueError("RX holding without an outstanding branch")
        if buffer["status"] == "released" and buffer["remaining_egress"]:
            raise ValueError("released RX still owns a branch")
        if buffer["released_ps"] != expected_release(buffer, forwards, requests):
            raise ValueError("RX release does not occur at the last admission/terminal outcome")
    return result


def check_dir_configuration(raw: dict, condition: dict) -> None:
    """Validate the independent converter against the executed input metadata."""
    config = {row["key"]: row["value"] for row in raw["metadata"]["config"]}
    if can.quantity(config["time-limit"], can.TIME_UNITS) != condition["horizon_ps"]:
        raise ValueError("DIR horizon differs from independent conversion")
    topology = {row["id"]: row for row in raw["metadata"]["topology"]["controllers"]}
    if set(topology) != {node["label"] for node in condition["nodes"]}:
        raise ValueError("DIR controller set differs from raw NED conversion")
    for node in condition["nodes"]:
        label = node["label"]
        if topology[label]["bus"] != node["bus"]:
            raise ValueError("DIR Bus membership differs from raw wiring")
        for raw_key, key in (("txProcessingDelay", "tx_processing_ps"), ("rxProcessingDelay", "rx_processing_ps"),
                             ("txChannelDelay", "tx_channel_ps"), ("rxChannelDelay", "rx_channel_ps")):
            if can.quantity(config[label + "." + raw_key], can.TIME_UNITS) != node[key]:
                raise ValueError("DIR processing/channel settings differ from raw conversion")
        if int(config[label + ".queueCapacity"]) != node["queue_capacity"] or json.loads(config[label + ".rxFilter"]) != node["rx_filter"]:
            raise ValueError("DIR queue/filter settings differ from raw conversion")
    for bus in condition["buses"]:
        if can.quantity(config[bus["label"] + ".bitrate"], can.RATE_UNITS) != bus["bitrate"]:
            raise ValueError("DIR bitrate differs from raw conversion")
    gateway_keys = {key for key in config if key.startswith("@profile:can.cc.multibus.v1:")}
    if gateway_keys != {"@profile:can.cc.multibus.v1:" + gateway["node"] for gateway in condition["gateways"]}:
        raise ValueError("DIR Gateway set differs from raw conversion")
    for gateway in condition["gateways"]:
        actual = json.loads(config["@profile:can.cc.multibus.v1:" + gateway["node"]])
        routes = [{**route, "id_min": str(route["id_min"]), "id_max": str(route["id_max"])} for route in gateway["routes"]]
        expected = {"node": gateway["node"], "ports": gateway["ports"], "routes": routes,
                    "hop_limit": str(gateway["hop_limit"]), "rx_queue_capacity": str(gateway["rx_queue_capacity"]),
                    "processing_delay_ps": str(gateway["processing_ps"])}
        # Route declarations may differ in order; both engines canonicalize them.
        actual["ports"].sort()
        for route in actual["routes"]:
            route["egress"].sort()
        actual["routes"].sort(key=lambda row: row["id"])
        if actual != expected:
            raise ValueError("DIR Gateway settings differ from independent conversion")


def parse_dir(raw: dict, condition: dict) -> dict:
    if raw.get("schema_version") != 2 or raw["simulation"]["partial"]:
        raise ValueError("Gateway comparison requires a complete schema2 DIR execution")
    check_dir_configuration(raw, condition)
    records = raw["simulation"]["model_records"]
    requests, receivers, forwards, buffers = {}, [], {}, {}
    for envelope in records:
        data = envelope["data"]
        if envelope["schema_name"] == "can.request":
            fields, ident = data["model_fields"], data["request_id"]
            if ident in requests:
                raise ValueError("duplicate DIR request")
            request = {"id": ident, "source": data["source"], "bus": data["bus"], "status": data["status"],
                       "origin_request_id": fields["origin_request_id"], "parent_request_id": fields["parent_request_id"],
                       "hops": int(fields["gw_hops"]), "frame_bits": int(data["serialized_bits"]), "drop_reason": data["drop_reason"]}
            for key in ("generated_ps", "ready_ps", "sof_ps", "eof_ps"):
                request[key] = nullable(data[key])
            for key in ("release_ps", "tx_enqueued_ps"):
                request[key] = nullable(fields[key])
            requests[ident] = request
        elif envelope["schema_name"] == "can.receiver":
            receivers.append({"id": data["request_id"], "receiver": data["receiver"], "status": data["status"],
                              "observed_ps": nullable(data["observed_ps"]), "received_ps": nullable(data["received_ps"])})
        elif envelope["schema_name"] == "gw.forward":
            row = copy.deepcopy(data)
            for key in ("received_ps", "planned_forward_ps", "forwarded_ps", "gw_hops"):
                row[key] = nullable(row[key])
            forwards[row["forward_id"]] = row
        elif envelope["schema_name"] == "gw.rx_buffer":
            row = copy.deepcopy(data)
            for key in ("received_ps", "released_ps", "capacity"):
                row[key] = nullable(row[key])
            buffers[row["buffer_id"]] = row
    tx_samples, rx_samples = [], []
    for point in raw["simulation"]["records"]:
        if point["metric"] == "queue_length":
            tx_samples.append((int(point["time_ps"]), point["target"][:-8], int(point["value"])))
        elif point["metric"] == "gw_rx_queue_length":
            rx_samples.append((int(point["time_ps"]), point["target"][:-8], int(point["value"])))
    offered = {row["request_id"]: row for row in can.offered_frames(condition) if row["generated_ps"] < condition["horizon_ps"]}
    originals = {ident: row for ident, row in requests.items() if row["parent_request_id"] is None}
    if set(originals) != set(offered):
        raise ValueError("DIR original request set differs from frozen workload")
    for ident, row in originals.items():
        if (row["generated_ps"], row["source"], row["origin_request_id"], row["hops"]) != (offered[ident]["generated_ps"], offered[ident]["source"], ident, 0):
            raise ValueError("DIR original request fields differ from raw inputs")
    for ident, row in requests.items():
        parent_id = row["parent_request_id"]
        if parent_id is None:
            continue
        parent, forward = requests[parent_id], forwards[ident]
        if (row["origin_request_id"] != parent["origin_request_id"] or row["hops"] != parent["hops"] + 1
                or row["frame_bits"] != parent["frame_bits"] or row["source"] != forward["egress"]
                or row["generated_ps"] != forward["forwarded_ps"]):
            raise ValueError("invalid DIR child lineage or source")
    return summarize(requests, receivers, forwards, buffers, tx_samples, rx_samples, condition, "dir")


def parse_omnet(path: Path, condition: dict) -> tuple[dict, list]:
    with path.open(newline="") as stream:
        events = list(csv.DictReader(stream))
    offered = {row["request_id"]: row for row in can.offered_frames(condition) if row["generated_ps"] < condition["horizon_ps"]}
    nodes = {node["label"]: node for node in condition["nodes"]}
    requests, receivers, forwards, buffers = {}, {}, {}, {}
    waiting = {node: set() for node in nodes}
    external = {node: deque() for node in nodes}
    slot_freed = {}
    used = {port: 0 for gateway in condition["gateways"] for port in gateway["ports"]}
    tx_samples, rx_samples, state, rx_state = [], [], {}, {}
    previous, completion = -1, {}

    def require_wake_completion():
        # Same-time FES work may temporarily leave a free slot. Once the entire
        # timestamp is committed, every such slot must have drained a waiter.
        for label, pending in external.items():
            if pending and len(waiting[label]) < nodes[label]["queue_capacity"]:
                raise ValueError("missing native SOF wake admission")

    for sequence, event in enumerate(events):
        at, kind, ident, node = int(event["time_ps"]), event["event"], event["request_id"], event["node"]
        if int(event["sequence"]) != sequence or not previous <= at < condition["horizon_ps"] or node not in nodes:
            raise ValueError("invalid native chronology/sequence/node")
        if at > previous:
            require_wake_completion()
        previous = at
        origin = event["origin_request_id"] or ident
        if origin not in offered:
            raise ValueError("unknown native origin")
        frame = offered[origin]
        if (event["format"], int(event["can_id"]), event["payload_hex"]) != (frame["format"], frame["can_id"], frame["payload_hex"]):
            raise ValueError("native frame changed across generation/forwarding/receive")
        if kind == "generated":
            if ident in requests:
                raise ValueError("duplicate native generation")
            parent = event["parent_request_id"] or None
            hops = int(event["hops"] or 0)
            if parent is None:
                if ident not in offered or (at, node, event["source"], origin, hops) != (offered[ident]["generated_ps"], frame["source"], frame["source"], ident, 0):
                    raise ValueError("native raw generation mismatch")
            else:
                if parent not in requests or origin != requests[parent]["origin_request_id"] or hops != requests[parent]["hops"] + 1:
                    raise ValueError("native child has invalid lineage")
                matches = [forward for forward in forwards.values() if forward["forward_id"] == ident]
                if len(matches) != 1 or matches[0]["egress"] != node or at != matches[0]["planned_forward_ps"]:
                    raise ValueError("native child not derived from accepted RX route")
            requests[ident] = {"id": ident, "source": node, "bus": nodes[node]["bus"], "origin_request_id": origin,
                               "parent_request_id": parent, "hops": hops, "status": "processing", "generated_ps": at,
                               "ready_ps": None, "tx_enqueued_ps": None, "sof_ps": None, "native_complete_ps": None,
                               "native_bits": int(event["native_bits"]), "drop_reason": None}
            state[ident] = "generated"
        elif kind in {"ready", "waiting_tx", "enqueued", "dropped", "sof", "native_complete"}:
            request = requests[ident]
            if node != request["source"] or event["source"] != node:
                raise ValueError("native source changed")
            allowed = {"ready": {"generated"}, "waiting_tx": {"ready"}, "enqueued": {"ready", "waiting_tx"},
                       "dropped": {"ready", "waiting_tx"}, "sof": {"enqueued"}, "native_complete": {"sof"}}
            if state[ident] not in allowed[kind]:
                raise ValueError(f"duplicate/invalid native transition {ident}/{kind}")
            if kind == "ready":
                if at != request["generated_ps"] + nodes[node]["tx_processing_ps"]:
                    raise ValueError("native TX processing delay mismatch")
                request["ready_ps"], request["status"] = at, "pending"
            elif kind == "waiting_tx":
                if request["parent_request_id"] is None or nodes[node]["queue_capacity"] == 0:
                    raise ValueError("invalid native external TX wait")
                if len(waiting[node]) < nodes[node]["queue_capacity"] and not external[node]:
                    raise ValueError("native copy waits without a capacity/FIFO dependency")
                external[node].append(ident)
                request["status"] = "waiting_tx"
            elif kind == "enqueued":
                if len(waiting[node]) >= nodes[node]["queue_capacity"]:
                    raise ValueError("native finite TX capacity exceeded")
                if request["parent_request_id"] is not None and external[node]:
                    if external[node][0] != ident:
                        raise ValueError("native Gateway TX wait FIFO was bypassed")
                    if slot_freed.get(node) != at:
                        raise ValueError("native TX wait was not admitted at the freeing SOF")
                    external[node].popleft()
                waiting[node].add(ident)
                request["tx_enqueued_ps"], request["status"] = at, "pending"
                tx_samples.append((at, node, len(waiting[node])))
            elif kind == "dropped":
                if event["reason"] != "queue_full" or len(waiting[node]) < nodes[node]["queue_capacity"]:
                    raise ValueError("native TX drop reason/capacity mismatch")
                request["status"], request["drop_reason"] = "dropped", event["reason"] or "queue_full"
                if request["parent_request_id"] is not None and nodes[node]["queue_capacity"] > 0:
                    raise ValueError("positive-capacity Gateway child dropped at full TX")
            elif kind == "sof":
                waiting[node].remove(ident)
                slot_freed[node] = at
                request["sof_ps"], request["status"] = at, "in_flight"
                tx_samples.append((at, node, len(waiting[node])))
            else:
                request["native_complete_ps"], request["status"] = at, "success"
                completion[ident] = at
            state[ident] = kind
            if event["queue_waiting"] and int(event["queue_waiting"]) != len(waiting[node]):
                raise ValueError("native TX queue observation violates conservation")
        elif kind in {"native_rx_complete", "observed", "received", "filtered"}:
            request = requests[ident]
            key = ident, node
            expected = {"native_rx_complete": None, "observed": "native_rx_complete", "received": "observed", "filtered": "observed"}
            if rx_state.get(key) != expected[kind] or node == request["source"] or nodes[node]["bus"] != request["bus"]:
                raise ValueError("duplicate/cross-bus/invalid native reception")
            receiver = receivers.setdefault(key, {"id": ident, "receiver": node, "status": "pending", "observed_ps": None,
                                                  "received_ps": None, "native_rx_complete_ps": None})
            if kind == "native_rx_complete":
                receiver["native_rx_complete_ps"] = at
            elif kind == "observed":
                expected_at = receiver["native_rx_complete_ps"] + nodes[request["source"]]["tx_channel_ps"] + nodes[node]["rx_channel_ps"]
                if at != expected_at:
                    raise ValueError("native channel delay mismatch")
                receiver["observed_ps"] = at
            elif kind == "received":
                if nodes[node]["rx_filter"] == "none" or at != receiver["observed_ps"] + nodes[node]["rx_processing_ps"]:
                    raise ValueError("native RX processing/filter mismatch")
                receiver["received_ps"], receiver["status"] = at, "received"
            else:
                if nodes[node]["rx_filter"] != "none" or at != receiver["observed_ps"]:
                    raise ValueError("native filter mismatch")
                receiver["status"] = "filtered"
            rx_state[key] = kind
        elif kind in {"rx_admitted", "rx_dropped", "rx_released", "forward_pending", "forward_submitted", "forward_dropped", "route_filtered"}:
            ingress, gateway, route = event["ingress"], *match_route(condition, event["ingress"], frame)
            if gateway is None or event["gateway"] != gateway["node"]:
                raise ValueError("unknown native Gateway ingress")
            parent = requests[ident]
            expected_hops = parent["hops"] + int(kind.startswith("forward_"))
            expected_node = event["egress"] if kind.startswith("forward_") else ingress
            if (origin != parent["origin_request_id"] or event["parent_request_id"] != ident or
                    event["source"] != parent["source"] or int(event["hops"]) != expected_hops or node != expected_node):
                raise ValueError("native Gateway event lineage/node mismatch")
            receiver = receivers.get((ident, ingress))
            if receiver is None or receiver["status"] != "received":
                raise ValueError("native Gateway action before receiver completion")
            buffer_id = f"rx:{ident}/{gateway['node']}/{ingress}"
            if event["buffer_id"] != buffer_id:
                raise ValueError("native RX buffer identity mismatch")
            if kind in {"rx_admitted", "rx_dropped"}:
                if buffer_id in buffers or at != receiver["received_ps"]:
                    raise ValueError("duplicate/mistimed native RX admission")
                full = used[ingress] >= gateway["rx_queue_capacity"]
                if (kind == "rx_dropped") != full:
                    raise ValueError("native RX capacity outcome mismatch")
                if event["reason"] != ("rx_queue_full" if full else ""):
                    raise ValueError("native RX admission reason mismatch")
                if kind == "rx_admitted":
                    used[ingress] += 1
                    rx_samples.append((at, ingress, used[ingress]))
                buffers[buffer_id] = {"buffer_id": buffer_id, "parent_request_id": ident, "origin_request_id": origin,
                                      "gateway": gateway["node"], "ingress": ingress, "capacity": gateway["rx_queue_capacity"],
                                      "received_ps": at, "released_ps": None, "status": "dropped" if full else "holding",
                                      "reason": "rx_queue_full" if full else None, "egress": route["egress"] if route else []}
            elif kind == "rx_released":
                buffer = buffers[buffer_id]
                if buffer["status"] != "holding" or remaining_egress(buffer, forwards, requests) or event["reason"]:
                    raise ValueError("native RX released with outstanding branches")
                used[ingress] -= 1
                buffer.update(status="released", released_ps=at)
                rx_samples.append((at, ingress, used[ingress]))
            elif kind == "route_filtered":
                if route is not None or buffers[buffer_id]["status"] != "holding" or at != receiver["received_ps"] or event["reason"] != "no_route":
                    raise ValueError("invalid native no-route outcome")
                forward_id = f"filtered:{ident}/{gateway['node']}/{ingress}"
                if forward_id in forwards:
                    raise ValueError("duplicate native no-route record")
                forwards[forward_id] = {"forward_id": forward_id, "parent_request_id": ident, "origin_request_id": origin,
                                        "gateway": gateway["node"], "ingress": ingress, "egress": None, "route_id": None,
                                        "gw_hops": requests[ident]["hops"], "received_ps": at, "planned_forward_ps": None,
                                        "forwarded_ps": None, "child_request_id": None, "status": "filtered", "reason": "no_route"}
            else:
                egress = event["egress"]
                if route is None or egress not in route["egress"] or event["route_id"] != route["id"] or buffers[buffer_id]["status"] == "dropped":
                    raise ValueError("native forward is not owned by accepted RX")
                forward_id = f"gw:{ident}/{gateway['node']}/{route['id']}/{egress}"
                if kind == "forward_pending":
                    if forward_id in forwards or at != receiver["received_ps"]:
                        raise ValueError("duplicate native fanout branch")
                    forwards[forward_id] = {"forward_id": forward_id, "parent_request_id": ident, "origin_request_id": origin,
                                            "gateway": gateway["node"], "ingress": ingress, "egress": egress, "route_id": route["id"],
                                            "gw_hops": requests[ident]["hops"] + 1, "received_ps": at,
                                            "planned_forward_ps": at + gateway["processing_ps"], "forwarded_ps": None,
                                            "child_request_id": None, "status": "processing", "reason": None}
                else:
                    forward = forwards[forward_id]
                    if forward["status"] != "processing" or at != forward["planned_forward_ps"]:
                        raise ValueError("duplicate/mistimed native forwarding")
                    exceeded = forward["gw_hops"] > gateway["hop_limit"]
                    if (kind == "forward_dropped") != exceeded:
                        raise ValueError("native hop outcome mismatch")
                    if event["reason"] != ("dropped_hop_limit" if exceeded else ""):
                        raise ValueError("native forward reason mismatch")
                    forward.update(forwarded_ps=at, status="dropped" if exceeded else "submitted",
                                   reason="dropped_hop_limit" if exceeded else None, child_request_id=None if exceeded else forward_id)
                    if not exceeded and event["child_request_id"] != forward_id:
                        raise ValueError("native submitted child identity mismatch")
            if event["rx_used"] and int(event["rx_used"]) != used[ingress]:
                raise ValueError("native RX observation violates conservation")
        else:
            raise ValueError(f"unknown native event: {kind}")
        if ident in requests and kind not in {"forward_pending", "forward_submitted", "forward_dropped", "rx_admitted", "rx_released", "rx_dropped", "route_filtered"}:
            request = requests[ident]
            if (int(event["native_bits"]) != request["native_bits"] or event["source"] != request["source"] or
                    origin != request["origin_request_id"] or (event["parent_request_id"] or None) != request["parent_request_id"] or
                    int(event["hops"] or 0) != request["hops"]):
                raise ValueError("native immutable lineage/source/frame length changed")
    require_wake_completion()
    originals = {ident for ident, request in requests.items() if request["parent_request_id"] is None}
    if originals != set(offered):
        raise ValueError("native offered original request set mismatch")
    for ident, request in requests.items():
        if request["generated_ps"] + nodes[request["source"]]["tx_processing_ps"] < condition["horizon_ps"] and state[ident] in {"generated", "ready"}:
            raise ValueError("missing native TX ready/admission outcome")
        complete = completion.get(ident)
        if complete is None:
            if any(key[0] == ident for key in receivers):
                raise ValueError("native receive for an incomplete transmission")
            continue
        for node in nodes:
            if node == request["source"] or nodes[node]["bus"] != request["bus"]:
                continue
            receiver = receivers.get((ident, node))
            if receiver is None or receiver["native_rx_complete_ps"] != complete:
                raise ValueError("missing native same-bus delivery")
            observed = complete + nodes[request["source"]]["tx_channel_ps"] + nodes[node]["rx_channel_ps"]
            if observed < condition["horizon_ps"] and receiver["observed_ps"] != observed:
                raise ValueError("missing native observation")
            received = observed + nodes[node]["rx_processing_ps"]
            if nodes[node]["rx_filter"] != "none" and received < condition["horizon_ps"]:
                if receiver["received_ps"] != received:
                    raise ValueError("missing native receiver completion")
                gateway, _ = match_route(condition, node, offered[request["origin_request_id"]])
                if gateway is not None and f"rx:{ident}/{gateway['node']}/{node}" not in buffers:
                    raise ValueError("missing native RX buffer admission/rejection")
    for buffer in buffers.values():
        branches = [row for row in forwards.values() if row["parent_request_id"] == buffer["parent_request_id"] and row["ingress"] == buffer["ingress"]]
        expected = 0 if buffer["status"] == "dropped" else max(1, len(buffer["egress"]))
        if len(branches) != expected:
            raise ValueError("missing native fanout/no-route branches")
    for forward in forwards.values():
        if forward["child_request_id"] is not None and forward["child_request_id"] not in requests:
            raise ValueError("native submitted branch has no child")
        if forward["status"] == "processing" and forward["planned_forward_ps"] < condition["horizon_ps"]:
            raise ValueError("missing native scheduled forwarding outcome")
    return summarize(requests, list(receivers.values()), forwards, buffers, tx_samples, rx_samples, condition, "omnet"), events


def compare_projections(directory: Path, left: dict, right: dict) -> dict:
    differences = []
    for kind, keys in (("requests", ("source", "bus", "origin_request_id", "parent_request_id", "hops", "status", "generated_ps", "ready_ps", "tx_enqueued_ps", "sof_ps", "drop_reason")),
                       ("forwards", ("parent_request_id", "origin_request_id", "gateway", "ingress", "egress", "route_id", "gw_hops", "received_ps", "planned_forward_ps", "forwarded_ps", "child_request_id", "status", "reason")),
                       ("rx_buffers", ("parent_request_id", "origin_request_id", "gateway", "ingress", "capacity", "received_ps", "released_ps", "status", "reason", "egress", "remaining_egress"))):
        for ident in sorted(set(left[kind]) | set(right[kind])):
            a, b = left[kind].get(ident, {}), right[kind].get(ident, {})
            if not a or not b:
                differences.append({"kind": kind, "id": ident, "field": "presence", "dir": bool(a), "omnet": bool(b)})
            for key in keys:
                if a.get(key) != b.get(key):
                    differences.append({"kind": kind, "id": ident, "field": key, "dir": a.get(key), "omnet": b.get(key)})
            if kind == "requests" and a.get("release_ps") != b.get("native_complete_ps"):
                differences.append({"kind": kind, "id": ident, "field": "DIR release / native complete", "dir": a.get("release_ps"), "omnet": b.get("native_complete_ps")})
    a_rx = {(row["id"], row["receiver"]): row for row in left["receivers"]}
    b_rx = {(row["id"], row["receiver"]): row for row in right["receivers"]}
    for key in sorted(set(a_rx) | set(b_rx)):
        a, b = a_rx.get(key, {}), b_rx.get(key, {})
        for field in ("status", "observed_ps", "received_ps"):
            if a.get(field) != b.get(field):
                differences.append({"kind": "receivers", "id": key[0], "receiver": key[1], "field": field, "dir": a.get(field), "omnet": b.get(field)})
    with (directory / "requests.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["request_id", "source", "bus", "origin", "dir_status", "omnet_status", "dir_generated_ps", "omnet_generated_ps", "dir_ready_ps", "omnet_ready_ps", "dir_enqueued_ps", "omnet_enqueued_ps", "dir_sof_ps", "omnet_sof_ps", "dir_eof_ps", "dir_release_ps", "omnet_native_complete_ps", "dir_frame_bits", "omnet_bits_including_ifs"])
        for ident in sorted(set(left["requests"]) | set(right["requests"])):
            a, b = left["requests"].get(ident, {}), right["requests"].get(ident, {})
            writer.writerow([ident, a.get("source", b.get("source")), a.get("bus", b.get("bus")), a.get("origin_request_id", b.get("origin_request_id")),
                             a.get("status"), b.get("status"), a.get("generated_ps"), b.get("generated_ps"), a.get("ready_ps"), b.get("ready_ps"),
                             a.get("tx_enqueued_ps"), b.get("tx_enqueued_ps"), a.get("sof_ps"), b.get("sof_ps"), a.get("eof_ps"), a.get("release_ps"),
                             b.get("native_complete_ps"), a.get("frame_bits"), b.get("native_bits")])
    summary = {"counts_equal": left["counts"] == right["counts"], "count_deltas_omnet_minus_dir": {key: right["counts"][key] - left["counts"][key] for key in left["counts"]},
               "sof_order_equal": all(left["buses"][bus]["sof_order"] == right["buses"][bus]["sof_order"] for bus in left["buses"]),
               "buses_equal": left["buses"] == right["buses"], "tx_queues_equal": left["tx_queues"] == right["tx_queues"],
               "rx_queues_equal": left["rx_queues"] == right["rx_queues"], "common_trace_differences": len(differences)}
    summary["common_projection_equal"] = not differences and all(summary[key] for key in ("counts_equal", "buses_equal", "tx_queues_equal", "rx_queues_equal"))
    (directory / "comparison.json").write_text(json.dumps({"dir": left, "omnet": right, **summary, "differences": differences}, indent=2) + "\n")
    return summary


def markdown(report: dict) -> str:
    completed = [case for case in report["cases"] if "error" not in case]
    lines = ["# DIR / OMNeT++ Gateway comparison", "", f"Executed: {len(completed)}/{len(report['cases'])}", "",
             "Native FiCo CAN buses, arbitration and approximate frame times are unchanged. Gateway RX/TX/fanout/hop behavior is a test adapter running in the OMNeT++ FES, not the native SignalsAndGateways implementation.", "",
             "DIR EOF and FiCo completion including IFS are recorded separately; no timestamp or bit-length compensation is used. Inputs are converted from frozen raw files before either output is read.", "",
             "| Case | Native requests DIR/OMNeT | Copies | Success | RX hold/release/drop DIR | RX hold/release/drop OMNeT | Per-bus SOF order | Common projection |", "|---|---:|---:|---:|---|---|---|---|"]
    for case in report["cases"]:
        if "error" in case:
            lines.append(f"| {case['name']} | ERROR | | | | | | {case['error']} |")
            continue
        left, right = case["dir_counts"], case["omnet_counts"]
        rx = lambda counts: "/".join(str(counts["rx_" + state]) for state in ("holding", "released", "dropped"))
        lines.append(f"| [{case['name']}](cases/{case['name']}/comparison.json) | {left['native']}/{right['native']} | {left['copies']}/{right['copies']} | {left['success']}/{right['success']} | {rx(left)} | {rx(right)} | {'equal' if case['comparison']['sof_order_equal'] else 'different'} | {'equal' if case['comparison']['common_projection_equal'] else 'different'} |")
    lines += ["", "## Validation and scope", "", "Every real native event is checked for raw payload/ID/format, lineage, sequence, same-bus delivery, configured processing/channel delays, TX admission, RX capacity and outstanding-egress conservation before comparison.", "",
              "Invalid-input tests, DIR internal event-limit/transaction/overflow contracts, codec bitstream assertions, output publication, viewer and unimplemented model fixtures have no equivalent physical experiment. They are not reported as simulated in OMNeT++.", ""]
    return "\n".join(lines)


def execute_case(case: dict, output: Path, workspace: Path, binary: Path) -> dict:
    directory = output / "cases" / case["name"]
    directory.mkdir()
    item = {"name": case["name"], "reference": case["reference"], "mutation": case.get("mutation")}
    try:
        config = materialize(case, directory / "inputs")
        condition = read_settings(config)
        ini = omnet_inputs(condition, directory / "omnet")
        can.run([str(binary), "run", "--config", str(config), "--output", str(directory / "dir")], directory / "dir.log")
        can.run(["bash", "-c", 'set -e; source "$1/scripts/env.sh"; exec opp_run -u Cmdenv -n "$2:$3:$1/upstream/FiCo4OMNeT/src" -l "$1/upstream/FiCo4OMNeT/src/FiCo4OMNeT" -l "$4/DirOmnetAdapter" -f "$5"',
                 "gw-comparison", str(workspace), str(directory / "omnet/models"), str(HERE / "model"), str(output / "build"), str(ini)],
                directory / "omnet/run.log", cwd=directory / "omnet")
        raw = json.loads((directory / "dir/results.json").read_text())
        left = parse_dir(raw, condition)
        right, events = parse_omnet(directory / "omnet/events.csv", condition)
        item.update(input_correspondence_verified=True, native_invariants_verified=True,
                    condition={key: value for key, value in condition.items() if key != "workload"}, dir_counts=left["counts"], omnet_counts=right["counts"],
                    comparison=compare_projections(directory, left, right),
                    input_sha256={str(path.relative_to(directory)): can.digest(path) for path in (directory / "inputs").rglob("*") if path.is_file()},
                    native_input_sha256={str(path.relative_to(directory)): can.digest(path) for path in (directory / "omnet").rglob("*")
                                         if path.is_file() and path.suffix in {".ini", ".tsv", ".ned", ".json"}})
        print(f"{'EQUAL' if item['comparison']['common_projection_equal'] else 'DIFF'} {case['name']}: copies {left['counts']['copies']}/{right['counts']['copies']}; RX drop {left['counts']['rx_dropped']}/{right['counts']['rx_dropped']}", flush=True)
    except (ValueError, RuntimeError, KeyError, OSError, subprocess.TimeoutExpired) as error:
        item["error"] = str(error)
        print(f"ERROR {case['name']}: {error}", flush=True)
    return item


def native_boundaries(output: Path) -> list[dict]:
    """Choose only H from earlier OMNeT observations, never DIR planned times."""
    cases = []
    for name, source_case, event_kind, wanted_id, fixture in [
        ("forward", "gw-forward-boundary", "forward_submitted", "source:0", "forward-boundary"),
        ("sof", "gw-queue", "enqueued", "gw:source:2/Main.gw/ab/Main.gw.b", "queue"),
        ("complete", "gw-queue", "native_complete", "source:0", "queue")]:
        source = output / "cases" / source_case / "omnet/events.csv"
        with source.open(newline="") as stream:
            row = next(row for row in csv.DictReader(stream) if row["event"] == event_kind and row["request_id"] == wanted_id)
        at = int(row["time_ps"])
        for offset, suffix in ((0, "at"), (1, "after")):
            cases.append({"name": f"gw-native-{name}-{suffix}", "config": FIXTURES / (fixture + ".ini"),
                          "horizon_ps": at + offset, "reference": "additional exclusive horizon comparison from native observed event",
                          "boundary": {"kind": name, "at_ps": at, "offset_ps": offset, "source_case": source_case,
                                       "source_event": event_kind, "source_request_id": wanted_id, "source_csv_sha256": can.digest(source)}})
    return cases


def verify_native_boundary(case: dict, directory: Path) -> dict:
    data = json.loads((directory / "comparison.json").read_text())["omnet"]
    boundary, offset = case["boundary"]["kind"], case["boundary"]["offset_ps"]
    child = "gw:source:0/Main.gw/ab/Main.gw.b"
    if boundary == "forward":
        assert (child in data["requests"]) == bool(offset), "exclusive native forward boundary violated"
    elif boundary == "sof":
        child = "gw:source:2/Main.gw/ab/Main.gw.b"
        expected = case["boundary"]["at_ps"] if offset else None
        assert data["requests"][child]["tx_enqueued_ps"] == expected, "exclusive native TX admission boundary violated"
        buffer = data["rx_buffers"]["rx:source:2/Main.gw/Main.gw.a"]
        assert buffer["released_ps"] == expected, "exclusive native RX release boundary violated"
    else:
        child = case["boundary"]["source_request_id"]
        expected = case["boundary"]["at_ps"] if offset else None
        assert data["requests"][child]["native_complete_ps"] == expected, "exclusive native completion boundary violated"
    return {**case["boundary"], "verified": True}


def verify_configuration_invariance(output: Path, completed: set) -> list[dict]:
    results = []
    for original, permutation in (("gw-multicast-same-priority", "gw-multicast-same-priority-permuted"),
                                  ("gw-multicast-first-ready", "gw-multicast-first-ready-permuted"),
                                  ("gw-delay", "gw-delay-renamed-gates")):
        if {original, permutation} <= completed:
            left = json.loads((output / "cases" / original / "comparison.json").read_text())
            right = json.loads((output / "cases" / permutation / "comparison.json").read_text())
            for engine in ("dir", "omnet"):
                if left[engine] != right[engine]:
                    raise ValueError(f"configuration permutation changed {engine} observations: {original}")
            results.append({"original": original, "permutation": permutation, "dir_equal": True, "omnet_equal": True})
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--omnet-workspace", type=Path, default=Path("/home/hideki/Omnet++"))
    parser.add_argument("--dir-binary", type=Path, default=ROOT / "target/release/dir-simulator")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append")
    parser.add_argument("--native-boundaries", action="store_true", help="add six H/H+1 runs derived from native observations; requires the full suite")
    args = parser.parse_args()
    output, workspace, binary = args.output.resolve(), args.omnet_workspace.resolve(), args.dir_binary.resolve()
    if output.exists():
        parser.error("output exists; choose a new directory")
    cases = [case for case in suite() if not args.case or case["name"] in args.case]
    if not cases or args.case and set(args.case) - {case["name"] for case in cases}:
        parser.error("unknown case")
    if args.native_boundaries and args.case:
        parser.error("native boundaries require the full suite")
    output.mkdir(parents=True)
    (output / "cases").mkdir()
    can.run(["bash", str(HERE / "model/build.sh")], output / "build.log", env=dict(os.environ, OMNET_WORKSPACE=str(workspace), BUILD_DIR=str(output / "build")))
    report = {"schema_version": 1, "provenance": can.provenance(workspace, binary), "cases": [],
              "model_scope": "native FiCo CAN plus external Gateway adapter; no native SignalsAndGateways validation"}
    report["provenance"]["files_sha256"][str(Path(__file__))] = can.digest(Path(__file__))
    report["provenance"]["omnet_version_output"] = can.run(["bash", "-c", 'source "$1/scripts/env.sh" && opp_run -h', "version", str(workspace)], output / "omnet-version.log")
    for case in cases:
        report["cases"].append(execute_case(case, output, workspace, binary))
        (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        (output / "report.md").write_text(markdown(report))
    if args.native_boundaries and not any("error" in case for case in report["cases"]):
        for case in native_boundaries(output):
            item = execute_case(case, output, workspace, binary)
            if "error" not in item:
                try:
                    item["native_boundary"] = verify_native_boundary(case, output / "cases" / case["name"])
                except (AssertionError, KeyError, ValueError) as error:
                    item["error"] = str(error)
            report["cases"].append(item)
    report["configuration_invariance"] = verify_configuration_invariance(output, {case["name"] for case in report["cases"] if "error" not in case})
    (output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    (output / "report.md").write_text(markdown(report))
    return int(any("error" in case for case in report["cases"]))


if __name__ == "__main__":
    sys.exit(main())
