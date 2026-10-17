#!/usr/bin/env python3
"""Check proposal trace structure and design vectors without running the product."""

from __future__ import annotations

import argparse
import json
import re
import sys
import zlib
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


STAGES = (
    "requirement",
    "function",
    "spec",
    "architecture",
    "design",
    "verification",
)
PREVIOUS_STAGE = {stage: STAGES[index - 1] for index, stage in enumerate(STAGES) if index}
TRACE_FENCE = re.compile(r"^```proposal-trace[ \t]*\r?\n(.*?)^```[ \t]*$", re.M | re.S)
ANY_FENCE = re.compile(r"^```[^\n]*\n.*?^```[ \t]*$", re.M | re.S)
ANCHOR = re.compile(r"<a\s+id=[\"']([^\"']+)[\"']\s*></a>", re.I)
HEX = re.compile(r"\A(?:[0-9a-fA-F]{2})*\Z")
U64_MAX = (1 << 64) - 1
PS_PER_SECOND = 1_000_000_000_000

# Kinds observed in the versioned design-vector files. Adding a new kind requires
# either an independent oracle below or an explicit structure-only entry here.
_STRUCTURE_ONLY_KINDS = frozenset(
    """
    atomic_capacity atomic_failure boundary_overlap boundary_prefix bytes
    checkpoint_gate checkpoint_mismatch checkpoint_phase checkpoint_publication
    checkpoint_tokens cleanup_order composite_admission composite_capacity
    control_boundary epoch_noop fallback filter_union frozen_export gauge_integral
    gauge_resume gap_rearrival horizon lease_timeline mailbox_capacity
    malformed_atomic malformed_boundary manifest_set metric_descriptor
    prepare_rejection query_budget ready_pause recovery_window replay_grouping
    replication_terminal request_retry same_time_order selection_order
    sequence_allocation simultaneous_expiry source_order stale_timer timer_refresh
    preemption_timeline preemption_prefix
    timestamp_bucket unknown_field wire_timing port_policy queue_revalidation
    readiness_gate
    bridge_binding bridge_map_order
    fsm_applicant fsm_registrar fsm_leaveall fsm_periodic fsm_unit
    fsm_applicant_admin fsm_registrar_admin fsm_tx_action fsm_tx_capacity
    fsm_local_leaveall_tx fsm_leaveall_sequence scenario
    wire_decode wire_reject wire_mac_frame_fcs codec_payload_exact
    integration_rule capacity_atomicity prepare_rule
    """.split()
)

ORACLE_KINDS = frozenset(
    """
    checked_sum crc_arithmetic fragment_formula gate_fit half_open hex_decode
    wire_envelope result_schema gauge_integral recovery_window wire_timing bytes
    manifest_set checkpoint_publication timestamp_bucket boundary_prefix
    hex_validation history_storage phase_order point_summary registration_union
    run_boundary time_code timer_boundary timer_overflow wire_time
    calculation timer_arithmetic packing_arithmetic
    bridge_catalog bridge_scope bridge_fdb_flush bridge_learning bridge_relearning
    """.split()
)


STRUCTURE_ONLY_KINDS = _STRUCTURE_ONLY_KINDS - ORACLE_KINDS
WIRE_FRAME_KINDS = frozenset({"wire_decode", "wire_reject", "wire_mac_frame_fcs"})

class DuplicateJsonKey(ValueError):
    """Raised when a JSON object repeats a key."""


def load_json(raw: str) -> Any:
    def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise DuplicateJsonKey(f"duplicate JSON key {key!r}")
            result[key] = value
        return result

    return json.loads(raw, object_pairs_hook=unique_object)


def ceil_div(numerator: int, denominator: int) -> int:
    if denominator <= 0:
        raise ValueError("denominator must be positive")
    return (numerator + denominator - 1) // denominator


def parse_int(value: Any, field: str) -> int:
    if isinstance(value, bool):
        raise ValueError(f"{field} must be an integer")
    if isinstance(value, int):
        return value
    if isinstance(value, str) and re.fullmatch(r"-?\d+", value):
        return int(value)
    raise ValueError(f"{field} must be an integer or decimal string")


def parse_hex(value: Any, field: str = "bytes_hex") -> bytes:
    if not isinstance(value, str):
        raise ValueError(f"{field} must be a hex string")
    if not value:
        raise ValueError(f"{field} is empty")
    if len(value) % 2:
        raise ValueError(f"{field} has odd_hex length")
    if not HEX.fullmatch(value):
        raise ValueError(f"{field} contains non_hex characters")
    return bytes.fromhex(value)


def compare_expected(expected: dict[str, Any], actual: dict[str, Any]) -> None:
    for key, value in actual.items():
        if expected.get(key) != value:
            raise ValueError(f"expected.{key} must equal {value!r}")


def check_wire_frame_integrity(kind: str, vector: dict[str, Any]) -> None:
    """Check frame bytes, FCS, and envelope bounds without asserting protocol behavior."""
    inputs = vector.get("input")
    expected = vector.get("expected")
    if not isinstance(inputs, dict) or not isinstance(expected, dict):
        raise ValueError("input and expected must be JSON objects")
    raw = parse_hex(inputs.get("bytes_hex"))
    if len(raw) < 64:
        raise ValueError("MAC frame is shorter than the 64-byte minimum including FCS")
    calculated_fcs = (zlib.crc32(raw[:-4]) & 0xFFFFFFFF).to_bytes(4, "little")
    calculated_hex = calculated_fcs.hex()
    fcs_valid = raw[-4:] == calculated_fcs

    supplied_calculation = inputs.get("computed_fcs_hex")
    if supplied_calculation is not None and supplied_calculation.lower() != calculated_hex:
        raise ValueError(f"input.computed_fcs_hex must equal independently computed {calculated_hex!r}")

    if kind == "wire_decode" and not fcs_valid:
        raise ValueError("wire_decode frame FCS is invalid")
    if kind == "wire_reject":
        if expected.get("reason") == "fcs" and fcs_valid:
            raise ValueError("wire_reject reason fcs requires a mismatched FCS")
        if expected.get("reason") != "fcs" and not fcs_valid:
            raise ValueError("wire_reject frame has an invalid FCS for its stated non-fcs reason")

    if kind in ("wire_decode", "wire_reject"):
        if len(raw) < 18:
            raise ValueError("802.3 frame is too short for LLC and FCS")
        declared_length = int.from_bytes(raw[12:14], "big")
        available = len(raw) - 18  # DA/SA/length and FCS are outside the 802.3 client data.
        if expected.get("reason") == "frame_bounds":
            if declared_length <= available:
                raise ValueError("frame_bounds vector length does not exceed available client data")
        elif declared_length > available:
            raise ValueError("802.3 declared length exceeds available client data")
        if "bpdu_length" in expected:
            compare_expected(expected, {"bpdu_length": declared_length - 3})
    elif kind == "wire_mac_frame_fcs":
        if raw[12:14] != b"\x88\xf5":
            raise ValueError("MVRP Ethernet II type must be 0x88f5")
        payload = parse_hex(inputs.get("payload_hex"), "payload_hex")
        if not raw[14:-4].startswith(payload):
            raise ValueError("input.payload_hex must prefix the MVRP client data")
        if not fcs_valid:
            raise ValueError("MVRP frame FCS is invalid")


def oracle(kind: str, vector: dict[str, Any]) -> None:
    """Raise ValueError when an independently recomputed expectation differs."""
    inputs = vector.get("input")
    expected = vector.get("expected")
    if not isinstance(inputs, dict) or not isinstance(expected, dict):
        raise ValueError("input and expected must be JSON objects")

    def compare(actual: dict[str, Any]) -> None:
        for key, value in actual.items():
            if expected.get(key) != value:
                raise ValueError(f"expected.{key} must equal {value!r}")

    if kind == "calculation":
        operation = inputs.get("operation")
        if operation == "updtRcvdInfoWhile":
            age = parse_int(inputs["message_age_ticks"], "input.message_age_ticks")
            max_age = parse_int(inputs["max_age_ticks"], "input.max_age_ticks")
            hello = parse_int(inputs["hello_time_s"], "input.hello_time_s")
            if min(age, max_age, hello) < 0:
                raise ValueError("RSTP time inputs must be nonnegative")
            # IEEE MessageAge is in 1/256-second units; round (age + 1 s)
            # to the nearest second, with half values rounded upward.
            rounded = (age + 256 + 128) // 256
            info_while = 3 * hello if rounded * 256 <= max_age else 0
            compare({"rounded_message_age_plus_one_s": rounded, "rcvdInfoWhile": info_while})
            return
        if operation == "recordTimes":
            # The raw wire field is preserved; its projection into portTimes
            # is left to the normative design review.
            compare({"received_wire_value_preserved_in_message": inputs["received_hello_ticks"]})
            return
        if operation == "lexicographic_tie_break":
            left, right = inputs["left"], inputs["right"]
            compare({"winner": "left" if left <= right else "right"})
            return
        if operation == "root_path_vector":
            if "received" not in inputs:
                external_cost = parse_int(inputs["external_cost"], "input.external_cost")
                path_cost = parse_int(inputs["external_path_cost"], "input.external_path_cost")
                total = external_cost + path_cost
                if total > 0xFFFFFFFF:
                    compare({"outcome": "run_failure", "reason": "arithmetic_overflow", "committed_effects": 0})
                else:
                    raise ValueError("root_path_vector success case must include received vector")
                return
            received = inputs["received"]
            cost = parse_int(received[1], "input.received[1]")
            external_path = parse_int(inputs["external_path_cost"], "input.external_path_cost")
            total = cost + external_path
            if total > 0xFFFFFFFF:
                compare({"outcome": "run_failure", "reason": "arithmetic_overflow", "committed_effects": 0})
            else:
                actual = list(received)
                actual[1] = total
                actual[2] = inputs["local_bridge"]
                compare({"root_path_vector": actual})
            return
        if operation == "tick_expiry":
            assigned = parse_int(inputs["assigned_at_ps"], "input.assigned_at_ps")
            counter = parse_int(inputs["counter"], "input.counter")
            origin = parse_int(inputs["tick_origin_ps"], "input.tick_origin_ps")
            period = parse_int(inputs["tick_period_ps"], "input.tick_period_ps")
            if counter < 0 or period <= 0 or assigned < origin:
                raise ValueError("tick inputs require a nonnegative counter and positive period")
            first_tick = origin + ((assigned - origin) // period + 1) * period
            expires = first_tick + max(0, counter - 1) * period
            compare({"expires_at_ps": str(expires), "not_at_ps": str(expires + period // 2)})
            return
        if operation == "fdb_flush":
            if inputs.get("operEdge"):
                removed = []
            else:
                fixed = set(inputs.get("fixed_vids_on_nonedge_ports", []))
                removed = [index for index, entry in enumerate(inputs.get("entries", []))
                           if entry.get("port") == inputs.get("port")
                           and entry.get("vid") in fixed and entry.get("kind") == "dynamic"]
            actual = {"removed_entry_indices": removed, "fdbFlush_after_completion": False}
            if "removed_count" in expected:
                actual = {"removed_count": len(removed), "fdbFlush_after_completion": False}
            compare(actual)
            return
        if operation == "counter_assignment":
            ticks = parse_int(inputs["wire_ticks"], "input.wire_ticks")
            if ticks < 0:
                raise ValueError("wire_ticks must be nonnegative")
            compare({"counter_seconds": ceil_div(ticks, 256), "wire_ticks_preserved": ticks})
            return
        raise ValueError(f"unknown calculation operation {operation!r}")

    if kind == "timer_arithmetic":
        join_cs = parse_int(inputs["join_time_cs"], "input.join_time_cs")
        leave_cs = parse_int(inputs["leave_time_cs"], "input.leave_time_cs")
        leaveall_cs = parse_int(inputs["leaveall_time_cs"], "input.leaveall_time_cs")
        if min(join_cs, leave_cs, leaveall_cs) < 0:
            raise ValueError("timer values must be nonnegative")
        leaveall_ps = leaveall_cs * 10_000_000_000
        compare({"join_ps": str(join_cs * 10_000_000_000),
                 "leave_ps": str(leave_cs * 10_000_000_000),
                 "leaveall_open_interval_ps": [str(leaveall_ps), str(leaveall_ps * 3 // 2)],
                 "periodic_ps": str(PS_PER_SECOND)})
        return

    if kind == "packing_arithmetic":
        events = inputs.get("events")
        if not isinstance(events, list):
            raise ValueError("input.events must be an array")
        values = [parse_int(event, "event") for event in events]
        if any(event < 0 or event > 5 for event in values):
            raise ValueError("MVRP event values must be in base-6 range 0..5")
        packed = []
        for offset in range(0, len(values), 3):
            group = values[offset:offset + 3]
            group += [0] * (3 - len(group))
            packed.append(36 * group[0] + 6 * group[1] + group[2])
        compare({"packed_hex": bytes(packed).hex(), "count": len(values)})
        return

    if kind == "bridge_catalog":
        ports = inputs.get("ports")
        static = inputs.get("static")
        allowed = inputs.get("allowed")
        overrides = inputs.get("overrides", [])
        if not isinstance(ports, list) or not isinstance(static, dict) or not isinstance(allowed, dict):
            raise ValueError("ports, static, and allowed must have the bridge catalog shape")
        static_by_port = {port: set(static.get(port, [])) for port in ports}
        allowed_by_port = {port: set(allowed.get(port, [])) for port in ports}
        svre_vids = set().union(*static_by_port.values(), *allowed_by_port.values()) if ports else set()
        override_modes: dict[tuple[str, int], str] = {}
        fixed_modes = {"fixed_ignore_new", "fixed_propagate_new"}
        for row in overrides:
            port, vid, mode = row["port"], parse_int(row["vid"], "override.vid"), row["mode"]
            if mode in fixed_modes and vid not in static_by_port.get(port, set()):
                compare({"accept": False, "reason": "fixed_outside_static"})
                return
            override_modes[(port, vid)] = mode
            svre_vids.add(vid)
        modes = {}
        for port in ports:
            for vid in sorted(svre_vids):
                if (port, vid) in override_modes:
                    mode = override_modes[(port, vid)]
                elif vid in static_by_port.get(port, set()):
                    mode = "fixed_ignore_new"
                else:
                    mode = "normal"
                modes[f"{port}/{vid}"] = mode
        actual: dict[str, Any] = {"accept": True, "svre_vids": sorted(svre_vids), "modes": modes}
        restricted_key = next((key for key in expected if key.startswith("restricted_")), None)
        if restricted_key is not None:
            match = re.fullmatch(r"restricted_(.+)_vid(\d+)", restricted_key)
            if not match:
                raise ValueError(f"unsupported bridge catalog output {restricted_key!r}")
            port, vid = match.group(1), int(match.group(2))
            actual[restricted_key] = vid in svre_vids and modes.get(f"{port}/{vid}") == "normal"
        compare(actual)
        return

    if kind == "bridge_fdb_flush":
        flush_kind = inputs["kind"]
        target_port = inputs["target_port"]
        entries = inputs.get("entries", [])
        if flush_kind == "rstp":
            oper_edge, fixed = inputs["oper_edge"], inputs["fixed"]
            mask = sorted({vid for port, vids in fixed.items() if not oper_edge.get(port, False) for vid in vids})
            removed = [] if oper_edge.get(target_port, False) else [
                entry["id"] for entry in entries
                if not entry.get("static") and entry.get("port") == target_port and entry.get("vid") in mask
            ]
            fixed_mask: list[int] | None = mask
        elif flush_kind == "mvrp_new":
            target_vid = parse_int(inputs["target_vid"], "input.target_vid")
            removed = [entry["id"] for entry in entries
                       if not entry.get("static") and entry.get("port") == target_port
                       and entry.get("vid") == target_vid]
            fixed_mask = None
        else:
            raise ValueError(f"unknown bridge FDB flush kind {flush_kind!r}")
        removed_set = set(removed)
        actual = {"removed": removed,
                  "kept": [entry["id"] for entry in entries if entry["id"] not in removed_set],
                  "fixed_vid_mask": fixed_mask,
                  "flush_completed": True}
        compare(actual)
        return

    if kind == "bridge_learning":
        eligible = bool(inputs["link_up"] and inputs["admit"] and inputs["vid_eligible"])
        learn = eligible and bool(inputs["learning"]) and bool(inputs["individual_source"])
        relay = eligible and bool(inputs["forwarding"])
        if not inputs["link_up"]:
            reason = "link_down"
        elif not inputs["admit"]:
            reason = "frame_not_admitted"
        elif not inputs["vid_eligible"]:
            reason = "vlan_unregistered"
        elif not inputs["forwarding"]:
            reason = "stp_learning" if inputs["learning"] else "stp_discarding"
        else:
            reason = None
        compare({"learn": learn, "relay": relay, "reason": reason})
        return

    if kind == "bridge_relearning":
        before = parse_int(inputs["generation_before"], "input.generation_before")
        stale = parse_int(inputs["stale_expiry_generation"], "input.stale_expiry_generation")
        flush, relearn = bool(inputs["flush"]), bool(inputs["relearn"])
        after = before + int(relearn)
        live_entry = not flush or relearn
        compare({"generation_after": str(after),
                 "stale_removes_new_entry": bool(live_entry and stale == after)})
        return

    if kind == "bridge_scope":
        reason = None
        if inputs.get("wire_rstp") and parse_int(inputs["convergence_ps"], "input.convergence_ps") != 0:
            reason = "wire_convergence_nonzero"
        bridges, mvrp_bridges = set(inputs["bridges"]), set(inputs["mvrp_bridges"])
        if reason is None and mvrp_bridges and (
            mvrp_bridges != bridges or not inputs.get("all_mvrp_ports_owned")
        ):
            reason = "mvrp_domain_incomplete"
        actual = {"accept": reason is None}
        if reason is not None:
            actual["reason"] = reason
        compare(actual)
        return

    if kind == "wire_envelope":
        protocol, enc = inputs["protocol"], inputs["encapsulation"]
        extras = inputs["extra_fields"]
        allowed = {"vid", "source_mac", "destination_mac"} if enc == "ip_packet" else set()
        valid = len(extras) == len(set(extras)) and set(extras) == allowed
        valid &= (protocol in ("igmp", "mld") and enc == "ip_packet") or (protocol in ("rstp", "mvrp") and enc == "mac_frame_fcs")
        result = "reject" if not valid else "accept"
        compare({"prepare": result})
        return

    if kind == "result_schema":
        compare({"result_schema": 3 if inputs["controlled_export"] or inputs["metric_v2"] else inputs["base_schema"]})
        return

    if kind == "recovery_window":
        # Set of accepted sequence values is independent of the specified bitmap update.
        capacity = int(inputs["history_capacity"])
        high = None if inputs["initial_high"] is None else int(inputs["initial_high"])
        bitmap = int(inputs["initial_bitmap_hex"], 16)
        seen = set() if high is None else {high-i for i in range(capacity) if bitmap & (1 << i)}
        decisions, highs, bitmaps = [], [], []
        for raw in inputs["sequences"]:
            seq = int(raw)
            if high is None:
                decision, high = "accept_first", seq
                seen.add(seq)
            elif seq > high:
                decision, high = "accept_advance", seq
                seen = {v for v in seen if v > high-capacity} | {seq}
            elif seq <= high-capacity:
                decision = "stale"
            elif seq in seen:
                decision = "duplicate"
            else:
                decision = "accept_late"
                seen.add(seq)
            decisions.append(decision)
            highs.append(str(high))
            bitmaps.append(format(sum(1 << (high-v) for v in seen), "x"))
        compare({"decisions": decisions, "watermarks": highs, "bitmaps_hex": bitmaps,
                 "delivered_count": sum(d.startswith("accept") for d in decisions),
                 "duplicate_count": decisions.count("duplicate"), "stale_count": decisions.count("stale")})
        return

    if kind == "gauge_integral":
        horizon, width = int(inputs["horizon_ps"]), int(inputs["window_ps"])
        changes = [(int(v["t"]), int(v["v"])) for v in inputs["changes"]]
        def area(lo: int, hi: int) -> int:
            last, value, result = 0, int(inputs["initial"]), 0
            for t, new_value in changes + [(horizon, 0)]:
                if t > horizon:
                    break
                result += max(0, min(hi, t)-max(lo, last)) * value
                last, value = t, new_value
            return result
        windows = [{"start": str(t), "end": str(min(t+width,horizon)), "area": str(area(t,min(t+width,horizon)))} for t in range(0,horizon,width)]
        compare({"area": str(area(0,horizon)), "duration": str(horizon), "windows": windows})
        return

    if kind == "wire_timing":
        mac = (18 if inputs["tagged"] else 14) + max(46, inputs["ip_packet_length"]) + 4
        rate, arrival = int(inputs["bitrate_bps"]), int(inputs["arrival_ps"])
        duration = ceil_div((mac+8)*8*PS_PER_SECOND,rate)
        start = arrival-duration
        compare({"mac_bytes": mac, "wire_duration_ps": str(duration), "source_start_ps": str(start),
                 "release_ps": str(start+ceil_div((mac+20)*8*PS_PER_SECOND,rate))})
        return

    if kind == "manifest_set":
        listed, actual = inputs["listed"], inputs["actual"]
        valid = len(set(listed)) == len(listed) and "manifest.json" not in listed and set(actual) == set(listed) | {"manifest.json"}
        compare({"valid": valid})
        if "manifest_self_hash" in expected:
            compare({"manifest_self_hash": "manifest.json" in listed})
        if "reason" in expected:
            compare({"reason": "unlisted_file" if set(actual)-set(listed)-{"manifest.json"} else "missing_file"})
        return

    if kind == "checkpoint_publication":
        failure = inputs["failure_at"]
        published = failure != "before_rename"
        compare({"published": published, "durability": None if not published else "unknown" if failure == "parent_sync" else "durable",
                 "ordinal_after": str(int(inputs["checkpoint_ordinal"])+int(published)), "delete_destination": False})
        return

    if kind == "timestamp_bucket":
        horizon = int(inputs["horizon_ps"])
        all_values = [int(v["v"]) for v in inputs["points"] if int(v["t"]) <= horizon]
        before = [int(v["v"]) for v in inputs["points"] if int(v["t"]) < horizon]
        at = [int(v["v"]) for v in inputs["points"] if int(v["t"]) == horizon]
        compare({"summary_min": str(min(all_values)), "window_min": str(min(before)), "held_bucket_time": str(horizon), "held_bucket_min": str(min(at))})
        return

    if kind == "boundary_prefix":
        start, arrival, release = (int(inputs[f"{v}_ps"]) for v in ("start", "arrival", "release"))
        cases = []
        for raw in inputs["horizons_ps"]:
            h = int(raw)
            records = sum(t < h for t in (start,arrival,release))
            cases.append({"horizon_ps": raw, "status": ["planned","receiving","received","released"][records], "boundary_records": records,
                "actual_start_ps": str(start) if start < h else None, "actual_arrival_ps": str(arrival) if arrival < h else None,
                "actual_release_ps": str(release) if release < h else None, "rx_wire_ps": str(max(0,min(h,arrival)-start)),
                "ifg_ps": str(max(0,min(h,release)-arrival)), "completed_rx_frames": int(arrival < h), "membership_callbacks": int(arrival < h)})
        compare({"cases": cases})
        return

    if kind == "bytes":
        # Byte/length/checksum oracle only; this is not a second protocol FSM.
        raw = parse_hex(inputs["bytes_hex"])
        ipv4 = raw[0] >> 4 == 4
        offset = (raw[0] & 15)*4 if ipv4 else 48  # fixtures use one 8-byte HBH header
        length = int.from_bytes(raw[2:4],"big") if ipv4 else 40+int.from_bytes(raw[4:6],"big")
        if length != len(raw):
            raise ValueError("IP total length differs from actual bytes")
        msg = raw[offset:]
        def folded(data: bytes) -> int:
            data += b"\0" if len(data) % 2 else b""
            total = sum(int.from_bytes(data[i:i+2],"big") for i in range(0,len(data),2))
            while total >> 16:
                total = (total & 65535)+(total >> 16)
            return total
        pseudo = b"" if ipv4 else raw[8:40]+len(msg).to_bytes(4,"big")+b"\0\0\0\x3a"
        actual = {"ip_packet_length": length, "message_offset": offset, "message_length": len(msg)}
        if "message_checksum_hex" in expected: actual["message_checksum_hex"] = msg[2:4].hex()
        if "folded_checksum_sum" in expected: actual["folded_checksum_sum"] = folded(pseudo+msg)
        if "ipv4_header_checksum_hex" in expected: actual["ipv4_header_checksum_hex"] = raw[10:12].hex()
        if ipv4 and folded(raw[:offset]) != 65535: raise ValueError("IPv4 header checksum is invalid")
        if expected.get("reason") == "checksum" and folded(pseudo+msg) == 65535: raise ValueError("expected malformed checksum is valid")
        compare(actual)
        return

    if kind == "hex_validation":
        raw = inputs.get("bytes_hex")
        if not isinstance(raw, str):
            raise ValueError("input.bytes_hex must be a string")
        if not raw:
            reason = "empty"
        elif len(raw) % 2:
            reason = "odd_hex"
        elif not HEX.fullmatch(raw):
            reason = "non_hex"
        else:
            raise ValueError("hex_validation vector does not contain invalid hex")
        if expected.get("prepare") != "reject" or expected.get("reason") != reason:
            raise ValueError(f"expected rejection reason must be {reason}")
        if expected.get("checksum") != "not_applicable":
            raise ValueError("invalid hex must have checksum=not_applicable")
        return

    if kind == "hex_decode":
        decoded = parse_hex(inputs.get("bytes_hex"))
        actual = {
            "bytes": list(decoded),
            "length_bytes": len(decoded),
            "canonical_hex": decoded.hex(),
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "crc_arithmetic":
        decoded = parse_hex(inputs.get("bytes_hex"))
        crc = zlib.crc32(decoded) & 0xFFFFFFFF
        actual = {
            "length_bytes": len(decoded),
            "crc32_hex": f"{crc:08x}",
            "fcs_little_endian_hex": crc.to_bytes(4, "little").hex(),
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "wire_time":
        mac = parse_int(inputs.get("mac_bytes"), "input.mac_bytes")
        preamble = parse_int(inputs.get("preamble_sfd_bytes"), "input.preamble_sfd_bytes")
        ifg = parse_int(inputs.get("ifg_bytes"), "input.ifg_bytes")
        rate = parse_int(inputs.get("rate_bps"), "input.rate_bps")
        sof = parse_int(inputs.get("sof_ps"), "input.sof_ps")
        if min(mac, preamble, ifg, sof) < 0 or rate <= 0:
            raise ValueError("wire-time inputs must be nonnegative and rate_bps positive")
        eof = sof + ceil_div((mac + preamble) * 8 * PS_PER_SECOND, rate)
        release = sof + ceil_div((mac + preamble + ifg) * 8 * PS_PER_SECOND, rate)
        actual = {"eof_ps": str(eof), "release_ps": str(release)}
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "timer_boundary":
        deadline = parse_int(inputs.get("deadline_ps"), "input.deadline_ps")
        run_end = parse_int(inputs.get("run_end_ps"), "input.run_end_ps")
        observations = inputs.get("observations_ps")
        if not isinstance(observations, list):
            raise ValueError("input.observations_ps must be an array")
        expired = [parse_int(t, "observation") >= deadline for t in observations]
        callback_times = [str(deadline)] if 0 <= deadline < run_end else []
        if expected.get("expired") != expired:
            raise ValueError(f"expected.expired must equal {expired!r}")
        if expected.get("callback_times_ps") != callback_times:
            raise ValueError(f"expected.callback_times_ps must equal {callback_times!r}")
        return

    if kind == "run_boundary":
        arrival = parse_int(inputs.get("arrival_ps"), "input.arrival_ps")
        deadline = parse_int(inputs.get("deadline_ps"), "input.deadline_ps")
        run_end = parse_int(inputs.get("run_end_ps"), "input.run_end_ps")
        actual = {
            "receive_callbacks": int(0 <= arrival < run_end),
            "timer_callbacks": int(0 <= deadline < run_end),
            "pending": int(arrival >= run_end) + int(deadline >= run_end),
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "timer_overflow":
        now = parse_int(inputs.get("now_ps"), "input.now_ps")
        duration = parse_int(inputs.get("duration_ps"), "input.duration_ps")
        if min(now, duration) < 0:
            raise ValueError("timer inputs must be nonnegative")
        overflow = now + duration > U64_MAX
        actual = {"result": "run_failure" if overflow else "scheduled", "committed_effects": 0 if overflow else 1}
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "registration_union":
        static = inputs.get("static_vids")
        wire = inputs.get("wire_registered_vids")
        authorized = inputs.get("authorized_vids")
        withdrawn = inputs.get("withdraw_vid")
        if not all(isinstance(items, list) for items in (static, wire, authorized)):
            raise ValueError("static_vids, wire_registered_vids, and authorized_vids must be arrays")
        before = sorted(set(static) | (set(wire) & set(authorized)))
        after = sorted(set(static) | ((set(wire) - {withdrawn}) & set(authorized)))
        actual = {"effective_before": before, "effective_after": after, "formula": "S union (W intersection A)"}
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "phase_order" and "events" in inputs:
        events = inputs["events"]
        ordered = sorted(events, key=lambda e: (int(e["time_ps"]), e["delta"], e["phase"], e["seq"]))
        if expected.get("order") != [e["id"] for e in ordered]:
            raise ValueError("event order disagrees with (time,delta,phase,seq)")
        return

    if kind == "phase_order":
        actual = {
            "ordered_events": ["timer_phase0", "receive_phase1", "select_phase2"],
            "formula": "phase0 < phase1 < phase2",
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "checked_sum":
        accumulator = parse_int(inputs.get("accumulator"), "input.accumulator")
        new_value = parse_int(inputs.get("new_value"), "input.new_value")
        bits = parse_int(inputs.get("bits"), "input.bits")
        if accumulator < 0 or new_value < 0 or bits <= 0:
            raise ValueError("checked_sum requires nonnegative values and positive bits")
        overflow = accumulator + new_value >= (1 << bits)
        actual = {
            "commit": not overflow,
            "accumulator_after": str(accumulator if overflow else accumulator + new_value),
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "point_summary":
        samples = inputs.get("samples")
        if not isinstance(samples, list):
            raise ValueError("input.samples must be an array")
        values = [parse_int(sample.get("v"), "sample.v") for sample in samples if isinstance(sample, dict)]
        if len(values) != len(samples):
            raise ValueError("each sample must be an object with v")
        total = sum(values)
        actual = {
            "count": str(len(values)),
            "sum": str(total),
            "min": str(min(values)) if values else None,
            "max": str(max(values)) if values else None,
            "mean_numerator": str(total) if values else None,
            "mean_denominator": str(len(values)),
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "time_code":
        response_code = parse_int(inputs.get("response_code"), "input.response_code")
        qqic = parse_int(inputs.get("qqic"), "input.qqic")
        if not 0 <= response_code <= 0xFFFF or not 0 <= qqic <= 0xFF:
            raise ValueError("time codes are outside their encoded widths")

        def decode_8(code: int, unit_ps: int) -> int:
            if code < 128:
                return code * unit_ps
            mantissa = 16 | (code & 0x0F)
            exponent = ((code >> 4) & 0x07) + 3
            return (mantissa << exponent) * unit_ps

        if vector["test_id"].startswith("P-IGMP-"):
            if response_code > 255:
                raise ValueError("IGMP response_code exceeds u8")
            response_ps = decode_8(response_code, 100_000_000_000)
        elif response_code < 0x8000:
            response_ps = response_code * 1_000_000_000
        else:
            mantissa = 0x1000 | (response_code & 0x0FFF)
            exponent = ((response_code >> 12) & 0x07) + 3
            response_ps = (mantissa << exponent) * 1_000_000_000
        interval_ps = decode_8(qqic, PS_PER_SECOND)
        actual = {"response_ps": str(response_ps), "interval_ps": str(interval_ps)}
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "fragment_formula":
        frame_bytes = parse_int(inputs.get("mac_bytes"), "input.mac_bytes")
        payload_limit = parse_int(inputs.get("fragment_payload_bytes"), "input.fragment_payload_bytes")
        overhead = parse_int(inputs.get("fragment_overhead_bytes"), "input.fragment_overhead_bytes")
        rate = parse_int(inputs.get("bitrate_bps"), "input.bitrate_bps")
        if frame_bytes <= 0 or payload_limit <= 0 or overhead < 0 or rate <= 0:
            raise ValueError("fragment inputs require positive frame, payload limit, and bitrate")
        payloads = [min(payload_limit, remaining) for remaining in range(frame_bytes, 0, -payload_limit)]
        accounted = [payload + overhead for payload in payloads]
        durations = [ceil_div(size * 8 * PS_PER_SECOND, rate) for size in accounted]
        actual = {
            "payloads": payloads,
            "accounted_bytes": accounted,
            "durations_ps": [str(duration) for duration in durations],
            "total_occupied_ps": str(sum(durations)),
            "formula": "ceil(8*(L+O)*1000000000000/R)",
        }
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    if kind == "gate_fit":
        start = parse_int(inputs.get("start_ps"), "input.start_ps")
        duration = parse_int(inputs.get("duration_ps"), "input.duration_ps")
        windows = inputs.get("window_end_ps")
        if not isinstance(windows, list):
            raise ValueError("input.window_end_ps must be an array")
        earliest_end = start + duration
        fits = [earliest_end <= parse_int(end, "window_end_ps") for end in windows]
        if expected.get("fits") != fits:
            raise ValueError(f"expected.fits must equal {fits!r}")
        if expected.get("earliest_end_ps") != str(earliest_end):
            raise ValueError(f"expected.earliest_end_ps must equal {str(earliest_end)!r}")
        return

    if kind == "half_open":
        event = parse_int(inputs.get("event_ps"), "input.event_ps")
        horizons = inputs.get("T_ps")
        if not isinstance(horizons, list):
            raise ValueError("input.T_ps must be an array")
        executed = [event < parse_int(horizon, "T_ps") for horizon in horizons]
        if expected.get("event_executed") != executed:
            raise ValueError(f"expected.event_executed must equal {executed!r}")
        count_key = "target_count" if "target_count" in expected else "completed_count"
        if count_key not in expected:
            raise ValueError("expected must include target_count or completed_count")
        count = [int(yes) for yes in executed]
        if expected.get(count_key) != count:
            raise ValueError(f"expected.{count_key} must equal {count!r}")
        if "sof_ps" in expected:
            sof = [str(event) if yes else None for yes in executed]
            if expected.get("sof_ps") != sof:
                raise ValueError(f"expected.sof_ps must equal {sof!r}")
        return

    if kind == "history_storage":
        capacities = inputs.get("capacities")
        bits = parse_int(inputs.get("bits_per_word"), "input.bits_per_word")
        if not isinstance(capacities, list) or bits <= 0 or bits % 8:
            raise ValueError("capacities must be an array and bits_per_word a positive multiple of 8")
        words = [ceil_div(parse_int(capacity, "capacity"), bits) for capacity in capacities]
        actual = {"words": words, "bytes": [word * bits // 8 for word in words]}
        for field, value in actual.items():
            if expected.get(field) != value:
                raise ValueError(f"expected.{field} must equal {value!r}")
        return

    raise AssertionError(f"no oracle registered for {kind}")


class ExtensionDesignChecker:
    def __init__(self, root: Path):
        self.root = root.resolve()
        self.proposals = self.root / "docs" / "proposals"
        self.errors: list[str] = []
        self.nodes: dict[str, dict[str, Any]] = {}
        self.node_occurrences: Counter[str] = Counter()
        self.requirements_without_full_chain: list[str] = []
        self.trace_document_count = 0
        self.trace_node_count = 0
        self.fixture_file_count = 0
        self.vector_count = 0
        self.recomputed_vectors = 0
        self.structure_only_vectors = 0
        self.unknown_kind_vectors = 0
        self.kind_coverage: dict[str, dict[str, int]] = defaultdict(
            lambda: {"total": 0, "recomputed": 0, "structure_only": 0, "unknown": 0}
        )

    def label(self, path: Path) -> str:
        try:
            return path.relative_to(self.root).as_posix()
        except ValueError:
            return str(path)

    def error(self, message: str) -> None:
        self.errors.append(message)

    def read(self, path: Path) -> str:
        try:
            return path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            self.error(f"{self.label(path)}: cannot read: {exc}")
            return ""

    def scan_traces(self) -> None:
        if not self.proposals.is_dir():
            self.error("docs/proposals: directory is missing")
            return
        documents = sorted(self.proposals.rglob("*.md"))
        for path in documents:
            text = self.read(path)
            anchors = set(ANCHOR.findall(ANY_FENCE.sub("", text)))
            matches = list(TRACE_FENCE.finditer(text))
            if not matches:
                continue
            self.trace_document_count += 1
            for block_index, match in enumerate(matches, start=1):
                try:
                    payload = load_json(match.group(1))
                except (ValueError, json.JSONDecodeError) as exc:
                    self.error(f"{self.label(path)}: proposal-trace block {block_index}: {exc}")
                    continue
                if not isinstance(payload, dict) or set(payload) != {"nodes"}:
                    self.error(f"{self.label(path)}: proposal-trace block {block_index}: expected only a nodes field")
                    continue
                nodes = payload["nodes"]
                if not isinstance(nodes, list):
                    self.error(f"{self.label(path)}: proposal-trace nodes must be an array")
                    continue
                for node_index, node in enumerate(nodes, start=1):
                    self.trace_node_count += 1
                    location = f"{self.label(path)}: proposal-trace node {node_index}"
                    if not isinstance(node, dict):
                        self.error(f"{location}: node must be an object")
                        continue
                    required = {"id", "stage", "upstream", "anchor"}
                    if set(node) != required:
                        self.error(f"{location}: expected fields {sorted(required)}")
                    ident = node.get("id")
                    stage = node.get("stage")
                    upstream = node.get("upstream")
                    anchor = node.get("anchor")
                    valid = True
                    if not isinstance(ident, str) or not ident:
                        self.error(f"{location}: id must be a nonempty string")
                        valid = False
                    else:
                        self.node_occurrences[ident] += 1
                        if ident in self.nodes:
                            self.error(f"duplicate trace ID {ident}: {self.label(path)}")
                        else:
                            self.nodes[ident] = {**node, "path": path}
                    if stage not in STAGES:
                        self.error(f"{location}: unknown stage {stage!r}")
                        valid = False
                    if not isinstance(upstream, list) or any(not isinstance(item, str) for item in upstream):
                        self.error(f"{location}: upstream must be an array of string IDs")
                        valid = False
                    elif len(set(upstream)) != len(upstream):
                        self.error(f"{location}: duplicate upstream ID")
                    if not isinstance(anchor, str) or not anchor:
                        self.error(f"{location}: anchor must be a nonempty string")
                    elif anchor not in anchors:
                        self.error(f"{location}: missing anchor {anchor!r}")
                    if valid and stage in STAGES and isinstance(upstream, list):
                        if stage == "requirement" and upstream:
                            self.error(f"{location}: requirement stage must have empty upstream")
                        elif stage != "requirement" and not upstream:
                            self.error(f"{location}: missing upstream for {stage} stage")

        if self.trace_node_count == 0:
            self.error("docs/proposals: no proposal-trace nodes found")

        for ident, node in self.nodes.items():
            stage = node.get("stage")
            upstream = node.get("upstream")
            if stage not in STAGES or not isinstance(upstream, list):
                continue
            required_parent_stage = PREVIOUS_STAGE.get(stage)
            for parent_id in upstream:
                parent = self.nodes.get(parent_id)
                if parent is None:
                    self.error(f"{ident}: missing upstream ID {parent_id}")
                elif required_parent_stage is None or parent.get("stage") != required_parent_stage:
                    got = parent.get("stage")
                    self.error(
                        f"{ident}: stage jump/invalid upstream {parent_id} ({got}) for {stage}; "
                        f"expected {required_parent_stage or 'no upstream'}"
                    )

        children: dict[str, list[str]] = defaultdict(list)
        for ident, node in self.nodes.items():
            upstream = node.get("upstream")
            if isinstance(upstream, list):
                for parent_id in upstream:
                    if parent_id in self.nodes:
                        children[parent_id].append(ident)

        def reaches_verification(ident: str, expected_index: int, visiting: set[str]) -> bool:
            node = self.nodes.get(ident)
            if node is None or node.get("stage") != STAGES[expected_index] or ident in visiting:
                return False
            if expected_index == len(STAGES) - 1:
                return True
            next_stage = STAGES[expected_index + 1]
            return any(
                self.nodes.get(child, {}).get("stage") == next_stage
                and reaches_verification(child, expected_index + 1, visiting | {ident})
                for child in children.get(ident, [])
            )

        requirements = sorted(ident for ident, node in self.nodes.items() if node.get("stage") == "requirement")
        for ident in requirements:
            if not reaches_verification(ident, 0, set()):
                self.requirements_without_full_chain.append(ident)
                self.error(f"{ident}: requirement does not reach verification through all six stages")

    def check_hex_fields(self, value: Any, location: str) -> None:
        if isinstance(value, dict):
            for key, child in value.items():
                if key == "bytes_hex":
                    try:
                        parse_hex(child, f"{location}.bytes_hex")
                    except ValueError as exc:
                        self.error(f"{location}: {exc}")
                else:
                    self.check_hex_fields(child, f"{location}.{key}")
        elif isinstance(value, list):
            for index, child in enumerate(value):
                self.check_hex_fields(child, f"{location}[{index}]")

    def scan_fixtures(self) -> None:
        fixture_dir = self.proposals / "fixtures"
        fixtures = sorted(fixture_dir.glob("*-v1.json")) if fixture_dir.is_dir() else []
        if not fixtures:
            self.error("docs/proposals/fixtures: no *-v1.json files found")
            return
        self.fixture_file_count = len(fixtures)
        vector_ids: set[str] = set()
        for path in fixtures:
            try:
                payload = load_json(self.read(path))
            except (ValueError, json.JSONDecodeError) as exc:
                self.error(f"{self.label(path)}: {exc}")
                continue
            if not isinstance(payload, dict):
                self.error(f"{self.label(path)}: fixture root must be an object")
                continue
            if payload.get("schema_version") != 1:
                self.error(f"{self.label(path)}: schema_version must be 1")
            if payload.get("status") != "design_vectors_not_product_tests":
                self.error(f"{self.label(path)}: status must be design_vectors_not_product_tests")
            vectors = payload.get("vectors")
            if not isinstance(vectors, list):
                self.error(f"{self.label(path)}: vectors must be an array")
                continue
            for vector_index, vector in enumerate(vectors, start=1):
                self.vector_count += 1
                location = f"{self.label(path)} vector {vector_index}"
                if not isinstance(vector, dict):
                    self.error(f"{location}: vector must be an object")
                    continue
                required = {"id", "test_id", "kind", "input", "expected"}
                missing = required - set(vector)
                if missing:
                    self.error(f"{location}: missing fields {sorted(missing)}")
                vector_id = vector.get("id")
                if not isinstance(vector_id, str) or not vector_id:
                    self.error(f"{location}: id must be a nonempty string")
                elif vector_id in vector_ids:
                    self.error(f"duplicate vector ID {vector_id}")
                else:
                    vector_ids.add(vector_id)
                test_id = vector.get("test_id")
                verification = self.nodes.get(test_id) if isinstance(test_id, str) else None
                if verification is None or verification.get("stage") != "verification":
                    self.error(f"{location}: unknown verification ID {test_id!r}")
                kind = vector.get("kind")
                if not isinstance(kind, str) or not kind:
                    self.error(f"{location}: kind must be a nonempty string")
                    continue
                # Invalid strings are the subject of hex_validation vectors;
                # their rejection reason is checked by that vector's oracle.
                if kind != "hex_validation":
                    self.check_hex_fields(vector, location)
                coverage = self.kind_coverage[kind]
                coverage["total"] += 1
                if kind in WIRE_FRAME_KINDS:
                    try:
                        check_wire_frame_integrity(kind, vector)
                    except (ValueError, TypeError, KeyError, IndexError) as exc:
                        self.error(f"{location} ({kind}): frame-integrity check failed: {exc}")
                if kind in ORACLE_KINDS:
                    self.recomputed_vectors += 1
                    coverage["recomputed"] += 1
                    try:
                        oracle(kind, vector)
                    except (ValueError, TypeError, KeyError, IndexError) as exc:
                        self.error(f"{location} ({kind}): arithmetic oracle mismatch: {exc}")
                elif kind in STRUCTURE_ONLY_KINDS:
                    self.structure_only_vectors += 1
                    coverage["structure_only"] += 1
                else:
                    self.unknown_kind_vectors += 1
                    coverage["unknown"] += 1
                    self.error(f"{location}: unknown vector kind {kind!r}; add an oracle or explicit structure-only classification")

    def run(self) -> dict[str, Any]:
        self.scan_traces()
        self.scan_fixtures()
        return {
            "schema_version": 1,
            "evidence_scope": "design-only; fixture and trace checks do not execute or validate product behavior",
            "recomputation_scope": {"bytes": "IP lengths/header and message checksums only; acceptance/action/state assertions remain design-review conditions", "wire_envelope": "tagged packet envelope only; assumes valid base configuration and ports", "wire_frames": "802.3/EtherType bounds and FCS input integrity only; protocol acceptance/action/state assertions remain structure-only", "other_oracles": "only the fields explicitly compared by oracle(kind, vector)"},
            "result": "fail" if self.errors else "pass",
            "inputs": {
                "proposal_trace_documents": self.trace_document_count,
                "trace_nodes": self.trace_node_count,
                "fixture_files": self.fixture_file_count,
                "vectors": self.vector_count,
            },
            "trace": {
                "requirements_checked": sum(node.get("stage") == "requirement" for node in self.nodes.values()),
                "requirements_reaching_verification": sum(
                    ident not in self.requirements_without_full_chain
                    for ident, node in self.nodes.items()
                    if node.get("stage") == "requirement"
                ),
                "requirements_without_full_chain": self.requirements_without_full_chain,
            },
            "vectors": {
                "recomputed": self.recomputed_vectors,
                "structure_only": self.structure_only_vectors,
                "unknown_kind": self.unknown_kind_vectors,
                "kind_coverage": {kind: counts for kind, counts in sorted(self.kind_coverage.items())},
            },
            "errors": self.errors,
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--report", type=Path, help="write a JSON report to this path")
    args = parser.parse_args(argv)
    report = ExtensionDesignChecker(args.root).run()
    if args.report:
        try:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        except OSError as exc:
            print(f"cannot write report {args.report}: {exc}", file=sys.stderr)
            return 2
    print(
        f"{report['result'].upper()}: design-only; "
        f"{report['inputs']['trace_nodes']} trace nodes, "
        f"{report['inputs']['vectors']} vectors "
        f"({report['vectors']['recomputed']} recomputed, "
        f"{report['vectors']['structure_only']} structure-only), "
        f"{len(report['errors'])} errors"
    )
    for error in report["errors"]:
        print(f"ERROR: {error}")
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
