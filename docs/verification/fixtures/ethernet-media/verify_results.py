#!/usr/bin/env python3
"""Compare Ethernet media CLI results with independent fixture projections."""

import argparse
import json
from pathlib import Path


HERE = Path(__file__).resolve().parent
SCHEMA_VERSIONS = {
    "ethernet.attempt": 1,
    "ethernet.frame": 1,
    "ethernet.phy_link": 1,
    "ethernet.reception": 1,
    "ethernet.transfer": 2,
}
ENVELOPE_KEYS = {
    "schema_name", "schema_version", "record_id", "subject", "request_id",
    "origin_request_id", "time_ps", "data",
}
DATA_KEYS = {
    "ethernet.attempt": {
        "transfer_id", "physical_link", "from_port", "to_port", "number",
        "sof_ps", "planned_eof_ps", "planned_release_ps", "planned_arrival_ps",
        "collision_ps", "planned_jam_start_ps", "planned_jam_end_ps", "jam_end_ps",
        "eof_ps", "release_ps", "arrival_ps", "backoff_slots", "backoff_until_ps",
        "status", "planned_mdi_sof_ps", "planned_mdi_eof_ps",
        "planned_peer_mdi_sof_ps", "planned_peer_mdi_eof_ps",
    },
    "ethernet.frame": {
        "source", "src_mac", "dst_mac", "ether_type", "data_hex", "pad_bytes",
        "mac_bytes", "fcs_hex", "mac_hex", "generated_ps", "ready_ps",
    },
    "ethernet.phy_link": {
        "a", "b", "phy_mode", "duplex", "bitrate_bps", "propagation_ps",
        "a_phy", "b_phy", "link_state", "seed", "deference_policy", "backoff_policy",
    },
    "ethernet.reception": {
        "frame_id", "transfer_id", "ingress", "observed_ps", "ready_ps",
        "planned_ready_ps", "status", "reason", "egress_transfer_ids",
    },
    "ethernet.transfer": {
        "frame_id", "parent_transfer_id", "from_port", "to_port", "queued_ps",
        "sof_ps", "eof_ps", "release_ps", "arrival_ps", "planned_eof_ps",
        "planned_release_ps", "planned_arrival_ps", "status", "drop_reason",
        "physical_link", "attempt_count", "collision_count", "last_attempt_id",
        "backoff_until_ps",
    },
}
TRANSFER_FIELDS = {
    "retry_sof_ps": "sof_ps",
    "eof_ps": "eof_ps",
    "retry_eof_ps": "eof_ps",
    "release_ps": "release_ps",
    "arrival_ps": "arrival_ps",
    "planned_arrival_ps": "planned_arrival_ps",
}
ATTEMPT_FIELDS = {
    "collision_ps": "collision_ps",
    "planned_jam_start_ps": "planned_jam_start_ps",
    "jam_end_ps": "jam_end_ps",
    "backoff_slots": "backoff_slots",
    "backoff_until_ps": "backoff_until_ps",
}
MEDIA_METRICS = json.loads((HERE / "metrics.json").read_text())
MEDIA_METRIC_IDS = {item["metric_id"] for item in MEDIA_METRICS}


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        assert key not in result, f"duplicate JSON key: {key}"
        result[key] = value
    return result


def read_json(path):
    return json.loads(Path(path).read_text(), object_pairs_hook=unique_pairs)


def result_file(root):
    root = Path(root)
    path = root / "results.json" if root.is_dir() else root
    assert path.is_file(), f"missing results.json: {path}"
    return path


def read_diagnostics(root):
    root = Path(root)
    path = root / "diagnostics.jsonl" if root.is_dir() else root.with_name("diagnostics.jsonl")
    if not path.is_file() or not path.read_text().strip():
        return None
    rows = []
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        if line.strip():
            try:
                rows.append(json.loads(line, object_pairs_hook=unique_pairs))
            except json.JSONDecodeError as error:
                raise AssertionError(f"invalid diagnostics.jsonl line {line_number}: {error}") from error
    return rows or None


def diagnostic_rules(value):
    rules = []
    if isinstance(value, dict):
        if isinstance(value.get("rule"), str):
            rules.append(value["rule"])
        for child in value.values():
            rules.extend(diagnostic_rules(child))
    elif isinstance(value, list):
        for child in value:
            rules.extend(diagnostic_rules(child))
    return rules


def records_by_schema(simulation):
    records = simulation["model_records"]
    grouped = {name: {} for name in SCHEMA_VERSIONS}
    seen = set()
    for row in records:
        assert set(row) == ENVELOPE_KEYS
        name = row["schema_name"]
        assert name in SCHEMA_VERSIONS, f"unexpected model record schema: {name}"
        assert row["schema_version"] == SCHEMA_VERSIONS[name]
        assert isinstance(row["record_id"], str)
        assert row["record_id"] not in seen, f"duplicate model record id: {row['record_id']}"
        seen.add(row["record_id"])
        assert set(row["data"]) == DATA_KEYS[name], f"{name} data field set changed"
        grouped[name][row["record_id"]] = row
    return grouped


def data_rows(grouped, schema):
    return list(grouped[schema].values())


def sort_rows(rows):
    return sorted(rows, key=lambda row: (row["request_id"] or "", row["record_id"]))


def transfer_for_key(transfers, key):
    exact = [row for row in transfers if row["record_id"] == key]
    if exact:
        assert len(exact) == 1
        return exact[0]
    by_frame = [row for row in transfers if row["data"]["frame_id"] == key]
    if by_frame:
        assert len(by_frame) == 1, f"ambiguous transfer key {key}"
        return by_frame[0]
    by_generator = [row for row in transfers
                    if row["data"]["frame_id"].split(":", 1)[0] == key]
    assert len(by_generator) == 1, f"cannot resolve transfer key {key}: {len(by_generator)} matches"
    return by_generator[0]


def transfer_source(row):
    return row["data"]["frame_id"].split(":", 1)[0]


def normalize_projection_value(value):
    if isinstance(value, str) and value.isdecimal():
        return int(value)
    return value


def assert_transfer_projection(transfers, field, expected, label):
    rows = sort_rows(transfers)
    if isinstance(expected, dict):
        projected = {key: normalize_projection_value(transfer_for_key(transfers, key)["data"][field])
                     for key in expected}
        assert projected == expected, f"{label}: {projected!r} != {expected!r}"
    else:
        actual = [normalize_projection_value(row["data"][field]) for row in rows]
        if isinstance(expected, list):
            if any(value is None for value in expected):
                assert actual == expected, f"{label}: {actual!r} != {expected!r}"
            else:
                actual = [value for value in actual if value is not None]
                assert actual == expected, f"{label}: {actual!r} != {expected!r}"
        else:
            assert actual and all(value == expected for value in actual), f"{label}: {actual!r} != {expected!r}"


def assert_attempt_projection(attempts, field, expected, label, number=None):
    rows = attempts
    if number is not None:
        rows = [row for row in rows if int(row["data"]["number"]) == number]
    rows = sorted(rows, key=lambda row: (row["data"]["transfer_id"], int(row["data"]["number"])))
    if isinstance(expected, dict):
        projected = {}
        for key, value in expected.items():
            candidates = [row for row in rows if row["data"]["transfer_id"] == key]
            if not candidates:
                candidates = [row for row in rows
                              if row["data"]["transfer_id"].split(":", 1)[0] == key
                              and (number is None or int(row["data"]["number"]) == number)]
            assert len(candidates) == 1, f"{label}: cannot uniquely resolve {key}"
            projected[key] = normalize_projection_value(candidates[0]["data"][field])
        assert len(projected) == len({row["record_id"] for row in rows}) or number is not None
        assert projected == expected, f"{label}: {projected!r} != {expected!r}"
    elif isinstance(expected, list):
        actual = [normalize_projection_value(row["data"][field]) for row in rows]
        if any(value is None for value in expected):
            assert actual == expected, f"{label}: {actual!r} != {expected!r}"
        else:
            actual = [value for value in actual if value is not None]
            assert actual == expected, f"{label}: {actual!r} != {expected!r}"
    else:
        actual = [normalize_projection_value(row["data"][field]) for row in rows]
        assert actual and all(value == expected for value in actual), f"{label}: {actual!r} != {expected!r}"


def assert_reception_projection(receptions, expected, label):
    rows = sorted(receptions, key=lambda row: (row["data"]["transfer_id"], row["record_id"]))
    actual = [normalize_projection_value(row["data"]["observed_ps"]) for row in rows]
    assert actual == expected, f"{label}: {actual!r} != {expected!r}"


def summary_rows(simulation):
    for row in simulation["records"] + simulation["summary"]:
        assert row["metric"] in MEDIA_METRIC_IDS, f"unexpected metric row: {row['metric']}"
    return simulation["summary"]


def one_metric_value(rows, target, metric):
    matches = [row for row in rows if row["target"] == target and row["metric"] == metric]
    assert len(matches) == 1, f"expected one {metric} row for {target}, got {len(matches)}"
    return matches[0]["value"]


def assert_record_conservation(grouped, summary, metadata):
    frames = grouped["ethernet.frame"]
    transfers = grouped["ethernet.transfer"]
    attempts = grouped["ethernet.attempt"]
    receptions = grouped["ethernet.reception"]

    status_counts = {name: 0 for name in
                     ("queued", "deferred", "transmitting", "jamming", "backoff", "serialized", "dropped")}
    attempts_by_transfer = {}
    for row in attempts.values():
        data = row["data"]
        attempts_by_transfer.setdefault(data["transfer_id"], []).append(row)
    arrivals = set()
    for record_id, row in transfers.items():
        data = row["data"]
        assert data["frame_id"] in frames, f"transfer references missing frame: {record_id}"
        assert data["status"] in status_counts, f"unknown transfer status: {data['status']}"
        status_counts[data["status"]] += 1
        related = sorted(attempts_by_transfer.get(record_id, []),
                         key=lambda attempt: int(attempt["data"]["number"]))
        numbers = [int(attempt["data"]["number"]) for attempt in related]
        assert numbers == list(range(1, len(related) + 1)), f"non-contiguous attempt numbers: {record_id}"
        assert int(data["attempt_count"]) == len(related), f"attempt count mismatch: {record_id}"
        collisions = sum(attempt["data"]["collision_ps"] is not None for attempt in related)
        assert int(data["collision_count"]) == collisions, f"collision count mismatch: {record_id}"
        if data["arrival_ps"] is not None:
            arrivals.add(record_id)
        if data["status"] == "dropped":
            assert data["arrival_ps"] is None and data["drop_reason"] in ("queue_full", "attempt_limit")
        else:
            assert data["drop_reason"] is None
    assert sum(status_counts.values()) == len(transfers), "offered transfer conservation failed"
    assert set(attempts_by_transfer) <= set(transfers)

    reception_transfer_ids = [row["data"]["transfer_id"] for row in receptions.values()]
    assert len(reception_transfer_ids) == len(set(reception_transfer_ids)), "duplicate reception per transfer"
    assert set(reception_transfer_ids) == arrivals, "reception count differs from arrived transfers"
    for row in receptions.values():
        data = row["data"]
        transfer = transfers[data["transfer_id"]]["data"]
        assert data["frame_id"] == transfer["frame_id"]
        assert data["observed_ps"] == transfer["arrival_ps"]

    outputs = sorted({row["data"]["from_port"] for row in transfers.values()})
    all_transfers = list(transfers.values())
    for output in outputs:
        selected = [row for row in all_transfers if row["data"]["from_port"] == output]
        expected_attempts = sum(int(row["data"]["attempt_count"]) for row in selected)
        expected_collisions = sum(int(row["data"]["collision_count"]) for row in selected)
        expected_exhausted = sum(row["data"]["drop_reason"] == "attempt_limit" for row in selected)
        assert int(one_metric_value(summary, output, "ethernet.media.attempts")) == expected_attempts
        assert int(one_metric_value(summary, output, "ethernet.media.collisions")) == expected_collisions
        assert int(one_metric_value(summary, output, "ethernet.media.retry_exhausted")) == expected_exhausted
    assert int(one_metric_value(summary, "$all", "ethernet.media.attempts")) == len(attempts)
    assert int(one_metric_value(summary, "$all", "ethernet.media.collisions")) == sum(
        row["data"]["collision_ps"] is not None for row in attempts.values())
    assert int(one_metric_value(summary, "$all", "ethernet.media.retry_exhausted")) == sum(
        row["data"]["drop_reason"] == "attempt_limit" for row in transfers.values())

    topology = metadata.get("ethernet_topology", metadata.get("topology", {}))
    endpoints = {device["id"] for device in topology.get("devices", [])
                 if device.get("kind") == "endpoint"}
    assert endpoints, "metadata is missing Ethernet endpoint identities"
    for target in sorted(endpoints | {"$all"}):
        count = sum(row["data"]["status"] == "received" and
                    (target == "$all" or row["subject"] == target) for row in receptions.values())
        assert int(one_metric_value(summary, target, "ethernet.media.delivered")) == count


def assert_media_envelope(raw, label):
    assert raw["schema_version"] == 2, f"{label}: not schema 2"
    metadata, simulation = raw["metadata"], raw["simulation"]
    assert metadata["model_profile"] == "ethernet.l2.store-forward.v2", f"{label}: wrong profile"
    schemas = {(row["schema_name"], row["schema_version"])
               for row in metadata["model_schemas"]}
    assert schemas == set(SCHEMA_VERSIONS.items()), f"{label}: model schema set {schemas!r}"
    assert metadata["metrics"] == MEDIA_METRICS, f"{label}: registered metric descriptors changed"
    return simulation


def assert_queue_retry(grouped, simulation, expected, label):
    transfers = list(grouped["ethernet.transfer"].values())
    attempts = list(grouped["ethernet.attempt"].values())
    current = transfer_for_key(transfers, expected["current"])
    dropped = transfer_for_key(transfers, expected["dropped"])
    at_ps = expected["at_ps"]
    current_attempts = [row for row in attempts if row["data"]["transfer_id"] == current["record_id"]]
    assert current_attempts, f"{label}: current transfer has no attempt history"
    assert any(int(row["data"]["sof_ps"]) <= at_ps
               and int(row["data"]["planned_jam_end_ps"]) > at_ps
               for row in current_attempts), f"{label}: current transfer is not active at {at_ps}ps"
    assert dropped["data"]["status"] == "dropped"
    assert dropped["data"]["queued_ps"] == str(at_ps), f"{label}: drop offer time"
    assert dropped["data"]["drop_reason"] == expected["reason"]
    for waiting_key in expected["waiting"]:
        waiting = transfer_for_key(transfers, waiting_key)
        assert int(waiting["data"]["queued_ps"]) < at_ps
        assert waiting["data"]["drop_reason"] is None
        later_attempts = [row for row in attempts if row["data"]["transfer_id"] == waiting["record_id"]]
        assert all(int(row["data"]["sof_ps"]) > at_ps for row in later_attempts)
    point_rows = [row for row in simulation["records"]
                  if row["metric"] == "ethernet.media.queue_length"
                  and row["target"] == current["data"]["from_port"] + ".queue"
                  and row["time_ps"] == str(at_ps)]
    assert len(point_rows) == 1, f"{label}: missing point queue length at {at_ps}ps"
    assert point_rows[0]["request_id"] == dropped["data"]["frame_id"]
    assert point_rows[0]["value"] == "1", f"{label}: waiting queue changed on rejected offer"


def assert_stop_projection(name, expected, grouped, summary, simulation):
    transfers = list(grouped["ethernet.transfer"].values())
    attempts = list(grouped["ethernet.attempt"].values())
    receptions = list(grouped["ethernet.reception"].values())
    if name == "stop-jam":
        assert all(row["data"]["status"] == expected["status"] for row in transfers)
        assert all(row["data"]["jam_end_ps"] is None for row in attempts)
        assert all(row["data"]["planned_jam_end_ps"] is not None for row in attempts)
        assert len(receptions) == expected["reception_count"]
        jam_values = [int(one_metric_value(summary, output, "ethernet.media.jam_ps"))
                      for output in sorted({row["data"]["from_port"] for row in transfers})]
        assert jam_values == [expected["jam_ps_per_output"]] * len(jam_values)
        utilizations = [one_metric_value(summary, output, "ethernet.media.tx_utilization")
                        for output in sorted({row["data"]["from_port"] for row in transfers})]
        assert utilizations == [1.0] * len(utilizations)
        assert simulation["end_ps"] == str(expected["tx_ps_per_output"])
    else:
        by_source = {transfer_source(row): row["data"]["status"] for row in transfers}
        assert by_source == {"a": expected["a_status"], "b": expected["b_status"]}
        assert len(receptions) == expected["reception_count"]
        jam_values = [int(one_metric_value(summary, output, "ethernet.media.jam_ps"))
                      for output in sorted({row["data"]["from_port"] for row in transfers})]
        assert jam_values == [expected["jam_ps_per_output"]] * len(jam_values)


def assert_scenario_projection(name, expected, grouped, summary, simulation):
    transfers = list(grouped["ethernet.transfer"].values())
    attempts = list(grouped["ethernet.attempt"].values())
    receptions = list(grouped["ethernet.reception"].values())
    assert expected.get("prepare_valid", True) is True
    for field, value in expected.items():
        if field == "sof_ps":
            assert_attempt_projection(attempts, "sof_ps", value, f"{name}.{field}", number=1)
        elif field in TRANSFER_FIELDS:
            assert_transfer_projection(transfers, TRANSFER_FIELDS[field], value, f"{name}.{field}")
        elif field in ATTEMPT_FIELDS:
            assert_attempt_projection(attempts, ATTEMPT_FIELDS[field], value, f"{name}.{field}")
        elif field == "received_ps":
            assert_reception_projection(receptions, value, f"{name}.{field}")
        elif field in ("collision_count", "collisions"):
            actual = sum(int(row["data"]["collision_count"]) for row in transfers)
            assert actual == value, f"{name}.{field}: {actual!r} != {value!r}"
        elif field in ("reception_count", "received_count"):
            assert len(receptions) == value, f"{name}.{field}: {len(receptions)} != {value}"
        elif field == "attempt1_slots":
            assert_attempt_projection(attempts, "backoff_slots", value, f"{name}.{field}", number=1)
        elif field == "attempt2_slots":
            assert_attempt_projection(attempts, "backoff_slots", value, f"{name}.{field}", number=2)
        elif field == "attempt2_sof_ps":
            assert_attempt_projection(attempts, "sof_ps", value, f"{name}.{field}", number=2)
        elif field == "attempt2_collision_ps":
            assert_attempt_projection(attempts, "collision_ps", value, f"{name}.{field}", number=2)
        elif field == "attempt2_jam_end_ps":
            assert_attempt_projection(attempts, "jam_end_ps", value, f"{name}.{field}", number=2)
        elif field == "attempt3_sof_ps":
            assert_attempt_projection(attempts, "sof_ps", value, f"{name}.{field}", number=3)
        elif field in ("at_ps", "current", "waiting", "dropped", "reason"):
            continue
        elif field in ("status", "jam_ps_per_output", "tx_ps_per_output", "a_status", "b_status"):
            continue
        elif field == "prepare_valid":
            continue
        elif field == "prepare_rule":
            continue
        else:
            raise AssertionError(f"unhandled independent projection {name}.{field}")
    if name == "queue-retry":
        assert_queue_retry(grouped, simulation, expected, name)
    if name in ("stop-jam", "stop-backoff"):
        assert_stop_projection(name, expected, grouped, summary, simulation)


def check_case(name, output_root, expected):
    if expected.get("prepare_rule"):
        diagnostic_rows = read_diagnostics(output_root)
        if diagnostic_rows is None:
            return "skipped: diagnostics.jsonl has no structured rows"
        found = [rule for row in diagnostic_rows for rule in diagnostic_rules(row)]
        assert expected["prepare_rule"] in found, (
            f"{name}: diagnostic rule {expected['prepare_rule']!r} not in {found!r}"
        )
        root = Path(output_root)
        candidate = root / "results.json" if root.is_dir() else root
        if candidate.is_file():
            raw = read_json(candidate)
            simulation = assert_media_envelope(raw, name)
            assert simulation["model_records"] == [], f"{name}: rejected prepare published model rows"
            return "rule checked; empty prepare snapshot checked"
        return "rule checked; prepare rejected before result publication"

    path = result_file(output_root)
    raw = read_json(path)
    simulation = assert_media_envelope(raw, name)
    grouped = records_by_schema(simulation)
    summary = summary_rows(simulation)

    assert_scenario_projection(name, expected, grouped, summary, simulation)
    assert_record_conservation(grouped, summary, raw["metadata"])
    if name == "t1-boundary":
        transfer = next(iter(grouped["ethernet.transfer"].values()))["data"]
        assert transfer["planned_arrival_ps"] == str(expected["planned_arrival_ps"])
        assert transfer["arrival_ps"] is None
        assert grouped["ethernet.reception"] == {}
    elif name == "t1-after-boundary":
        transfer = next(iter(grouped["ethernet.transfer"].values()))["data"]
        assert transfer["arrival_ps"] == str(expected["arrival_ps"])
        assert len(grouped["ethernet.reception"]) == expected["received_count"]
    return "checked"


def parse_mapping(values, allowed):
    mapping = {}
    for value in values:
        if "=" not in value:
            raise argparse.ArgumentTypeError("expected NAME=OUTPUT_ROOT")
        name, root = value.split("=", 1)
        if name not in allowed:
            raise argparse.ArgumentTypeError(f"unknown fixture name: {name}")
        if name in mapping:
            raise argparse.ArgumentTypeError(f"duplicate fixture mapping: {name}")
        mapping[name] = Path(root)
    missing = allowed - mapping.keys()
    if missing:
        raise argparse.ArgumentTypeError(f"missing fixture mappings: {', '.join(sorted(missing))}")
    return mapping


def main():
    cases = read_json(HERE / "scenarios.json")["cases"]
    expected = {case["name"]: case["expected"] for case in cases}
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--result", action="append", required=True, metavar="NAME=OUTPUT_ROOT",
        help="one generated output root per fixture case (repeat for all scenarios)",
    )
    args = parser.parse_args()
    roots = parse_mapping(args.result, set(expected))
    skipped = []
    prepare_only = []
    for name, fixture_expected in expected.items():
        result = check_case(name, roots[name], fixture_expected)
        if result.startswith("skipped:"):
            skipped.append(f"{name} ({result})")
        elif "prepare rejected before result publication" in result:
            prepare_only.append(name)
        print(f"PASS Ethernet media product projection: {name} ({result})")
    if skipped:
        print("SKIPPED structured diagnostic checks: " + "; ".join(skipped))
    if prepare_only:
        print("SKIPPED model-record and metric checks (prepare rejected without results.json): "
              + ", ".join(prepare_only))
    print(f"PASS: {len(expected)} schema-2 media result roots; analytic fixture literals used as expected values")


if __name__ == "__main__":
    main()
