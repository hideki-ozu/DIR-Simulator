#!/usr/bin/env python3
"""Check schema-2 CAN FD and 100BASE-T1 product outputs against fixture literals."""

import argparse
import json
from pathlib import Path


HERE = Path(__file__).resolve().parent
MEDIA_METRICS_PATH = HERE.parent / "ethernet-media" / "metrics.json"
FD_METRICS = [
    {"metric_id": "canfd.dropped", "version": "1", "unit": "count",
     "value_kind": "integer", "sampling": "summary", "aggregation": "sum"},
    {"metric_id": "canfd.generated", "version": "1", "unit": "count",
     "value_kind": "integer", "sampling": "summary", "aggregation": "sum"},
    {"metric_id": "canfd.received", "version": "1", "unit": "count",
     "value_kind": "integer", "sampling": "summary", "aggregation": "sum"},
    {"metric_id": "canfd.serialized", "version": "1", "unit": "count",
     "value_kind": "integer", "sampling": "summary", "aggregation": "sum"},
]
FD_SCHEMAS = {
    "dir.canfd.frame": 1,
    "dir.canfd.reception": 1,
    "dir.canfd.request": 1,
}
MEDIA_SCHEMAS = {
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
FD_DATA_KEYS = {
    "dir.canfd.frame": {
        "format", "id", "data", "dlc", "brs", "nominal_bits", "data_bits",
        "evidence", "binding_sha256", "nominal_rate", "data_rate", "fidelity",
        "wire_validation",
    },
    "dir.canfd.request": {
        "frame_id", "source", "bus", "generated_ps", "ready_ps", "sof_ps", "eof_ps",
        "release_ps", "planned_ready_ps", "planned_eof_ps", "planned_release_ps",
        "state", "drop_reason",
    },
    "dir.canfd.reception": {
        "frame_id", "receiver", "planned_arrival_ps", "planned_completed_ps",
        "arrival_ps", "completed_ps", "state",
    },
}
MEDIA_DATA_KEYS = {
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


def load_result(root):
    raw = read_json(result_file(root))
    assert raw["schema_version"] == 2
    return raw


def group_records(raw, schemas, data_keys, label):
    rows = raw["simulation"]["model_records"]
    grouped = {name: {} for name in schemas}
    seen = set()
    for row in rows:
        assert set(row) == ENVELOPE_KEYS, f"{label}: ModelRecord envelope changed"
        name = row["schema_name"]
        assert name in schemas, f"{label}: unexpected record schema {name}"
        assert row["schema_version"] == schemas[name]
        assert set(row["data"]) == data_keys[name], f"{label}: {name} field set changed"
        assert row["record_id"] not in seen, f"{label}: duplicate record id {row['record_id']}"
        seen.add(row["record_id"])
        grouped[name][row["record_id"]] = row
    return grouped


def rows(grouped, name):
    return list(grouped[name].values())


def sorted_by_request(records):
    return sorted(records, key=lambda row: (row["request_id"] or "", row["record_id"]))


def assert_profile(raw, profile, schemas, metrics, label):
    metadata = raw["metadata"]
    assert metadata["model_profile"] == profile, f"{label}: wrong selected profile"
    actual_schemas = {row["schema_name"]: row["schema_version"]
                      for row in metadata["model_schemas"]}
    assert actual_schemas == schemas, f"{label}: model schema set {actual_schemas!r}"
    assert metadata["metrics"] == metrics, f"{label}: registered metric descriptors changed"


def summary_values(raw):
    return raw["simulation"]["summary"]


def check_fd(root, vectors):
    raw = load_result(root)
    assert_profile(raw, "can.fd.precomputed.v1", FD_SCHEMAS, FD_METRICS, "fd")
    simulation = raw["simulation"]
    grouped = group_records(raw, FD_SCHEMAS, FD_DATA_KEYS, "fd")
    assert len(grouped["dir.canfd.frame"]) == 1
    assert len(grouped["dir.canfd.request"]) == 1
    assert len(grouped["dir.canfd.reception"]) == 2

    workload = read_json(HERE / "fd.workload.json")
    generator = workload["generators"][0]
    frame_input = generator["frame"]
    vector = next(case for case in vectors["fd"] if case["name"] == "brs64-realistic-count-range")
    vector_input = vector["input"]
    duration, release, dlc = vector["expected"]
    assert frame_input["format"] == vector_input["format"]
    assert frame_input["id"] == vector_input["id"]
    assert frame_input["data"].lower() == vector_input["data"].lower()
    assert frame_input["brs"] == vector_input["brs"]
    assert frame_input["wire"] == {
        "nominal_bits": vector_input["nominal_bits"],
        "data_bits": vector_input["data_bits"],
        "evidence": vector_input["evidence"],
        "binding_sha256": vector_input["binding_sha256"],
    }

    frame = next(iter(grouped["dir.canfd.frame"].values()))
    frame_data = frame["data"]
    assert frame["record_id"] == generator["id"]
    assert frame["subject"] == generator["node"]
    assert frame["request_id"] is None and frame["origin_request_id"] is None
    assert frame["time_ps"] == "0"
    assert frame_data == {
        "format": vector_input["format"], "id": vector_input["id"],
        "data": vector_input["data"].lower(), "dlc": dlc, "brs": vector_input["brs"],
        "nominal_bits": str(vector_input["nominal_bits"]),
        "data_bits": str(vector_input["data_bits"]), "evidence": vector_input["evidence"],
        "binding_sha256": vector_input["binding_sha256"],
        "nominal_rate": str(vector_input["nominal_rate"]),
        "data_rate": str(vector_input["data_rate"]),
        "fidelity": "externally-precomputed-phase-bits",
        "wire_validation": "structural-only",
    }

    request_id = generator["id"] + ":0"
    request = next(iter(grouped["dir.canfd.request"].values()))
    assert request["record_id"] == request_id and request["request_id"] == request_id
    assert request["subject"] == generator["node"] and request["origin_request_id"] is None
    assert request["data"] == {
        "frame_id": generator["id"], "source": generator["node"], "bus": "Main.bus",
        "generated_ps": "0", "ready_ps": "0", "sof_ps": "0",
        "eof_ps": str(duration), "release_ps": str(release),
        "planned_ready_ps": "0", "planned_eof_ps": str(duration),
        "planned_release_ps": str(release), "state": "serialized", "drop_reason": None,
    }
    assert request["time_ps"] == str(release)

    receptions = sorted(rows(grouped, "dir.canfd.reception"), key=lambda row: row["subject"])
    assert [row["subject"] for row in receptions] == ["Main.b", "Main.c"]
    for reception in receptions:
        receiver = reception["subject"]
        assert reception["record_id"] == request_id + ":" + receiver
        assert reception["request_id"] == request_id and reception["origin_request_id"] is None
        assert reception["data"] == {
            "frame_id": generator["id"], "receiver": receiver,
            "planned_arrival_ps": str(duration), "planned_completed_ps": str(duration),
            "arrival_ps": str(duration), "completed_ps": str(duration), "state": "completed",
        }
        assert reception["time_ps"] == str(duration)

    expected_metrics = {
        "canfd.generated": {"Main.a": 1, "Main.b": 0, "Main.c": 0, "$all": 1},
        "canfd.serialized": {"Main.a": 1, "Main.b": 0, "Main.c": 0, "$all": 1},
        "canfd.dropped": {"Main.a": 0, "Main.b": 0, "Main.c": 0, "$all": 0},
        "canfd.received": {"Main.a": 0, "Main.b": 1, "Main.c": 1, "$all": 2},
    }
    summary = summary_values(raw)
    actual_metric_ids = {row["metric"] for row in summary}
    assert actual_metric_ids == {item["metric_id"] for item in FD_METRICS}
    expected_metric_rows = {(metric, target): value for metric, targets in expected_metrics.items()
                            for target, value in targets.items()}
    actual_metric_rows = {}
    for row in summary:
        key = (row["metric"], row["target"])
        assert key not in actual_metric_rows, f"duplicate summary metric row {key}"
        actual_metric_rows[key] = row["value"]
    assert set(actual_metric_rows) == set(expected_metric_rows)
    for key, value in expected_metric_rows.items():
        assert actual_metric_rows[key] == str(value), f"{key}: {actual_metric_rows[key]!r} != {value}"

    return duration, release, dlc


def check_t1(root, vectors):
    raw = load_result(root)
    metrics = read_json(MEDIA_METRICS_PATH)
    assert_profile(raw, "ethernet.l2.100base-t1.v1", MEDIA_SCHEMAS, metrics, "t1")
    grouped = group_records(raw, MEDIA_SCHEMAS, MEDIA_DATA_KEYS, "t1")
    transfers = sorted_by_request(rows(grouped, "ethernet.transfer"))
    attempts = rows(grouped, "ethernet.attempt")
    receptions = sorted(rows(grouped, "ethernet.reception"),
                        key=lambda row: row["data"]["transfer_id"])
    frames = rows(grouped, "ethernet.frame")
    phy_links = rows(grouped, "ethernet.phy_link")
    assert (len(frames), len(transfers), len(attempts), len(receptions), len(phy_links)) == (2, 2, 2, 2, 1)

    fixture = read_json(HERE / "t1.workload.json")
    generators = fixture["generators"]
    expected_cases = [case for case in vectors["t1"] if case["name"] in ("a-to-b", "b-to-a")]
    assert [generator["id"] for generator in generators] == ["a", "b"]
    assert [case["name"] for case in expected_cases] == ["a-to-b", "b-to-a"]
    expected = [case["expected"] for case in expected_cases]
    for index, (transfer, vector) in enumerate(zip(transfers, expected)):
        data = transfer["data"]
        sof, eof, release, arrival = 0, vector[0], vector[1], vector[2]
        generator = generators[index]
        assert data["frame_id"] == generator["id"] + ":0"
        assert data["from_port"] == generator["node"] + ".tx"
        assert data["sof_ps"] == str(sof)
        assert data["planned_eof_ps"] == data["eof_ps"] == str(eof)
        assert data["planned_release_ps"] == data["release_ps"] == str(release)
        assert data["planned_arrival_ps"] == data["arrival_ps"] == str(arrival)
        assert data["status"] == "serialized" and data["drop_reason"] is None
        assert int(data["attempt_count"]) == 1 and int(data["collision_count"]) == 0
        assert len([row for row in attempts if row["data"]["transfer_id"] == transfer["record_id"]]) == 1

    attempt_rows = sorted(attempts, key=lambda row: row["data"]["transfer_id"])
    for index, (attempt, vector) in enumerate(zip(attempt_rows, expected)):
        data = attempt["data"]
        assert data["number"] == "1" and data["status"] == "serialized"
        assert data["collision_ps"] is None and data["jam_end_ps"] is None
        assert data["backoff_slots"] is None and data["backoff_until_ps"] is None
        assert data["sof_ps"] == "0" and data["eof_ps"] == str(vector[0])

    expected_receptions = {transfer["record_id"]: vector[2]
                           for transfer, vector in zip(transfers, expected)}
    for reception in receptions:
        data = reception["data"]
        assert data["transfer_id"] in expected_receptions
        assert data["observed_ps"] == str(expected_receptions[data["transfer_id"]])
        assert data["status"] == "received" and data["reason"] is None
        assert data["ready_ps"] is not None

    topology = read_json(HERE / "t1.model.json")
    link = topology["physical_links"][0]
    phy = phy_links[0]
    assert phy["record_id"] == link["id"]
    assert phy["subject"] == "@media:" + link["id"]
    pdata = phy["data"]
    assert pdata["a"] == link["a"] and pdata["b"] == link["b"]
    assert pdata["phy_mode"] == "100base-t1" and pdata["duplex"] == "full"
    assert pdata["bitrate_bps"] == "100000000" and pdata["propagation_ps"] == "1000"
    assert pdata["a_phy"] == link["a_phy"] and pdata["b_phy"] == link["b_phy"]
    assert pdata["link_state"] == "up"

    summary = summary_values(raw)
    assert {row["metric"] for row in summary} <= {item["metric_id"] for item in metrics}
    delivered = {(row["target"]): row["value"] for row in summary
                 if row["metric"] == "ethernet.media.delivered"}
    assert delivered == {"Main.a": "1", "Main.b": "1", "$all": "2"}
    return len(transfers), len(receptions)


def parse_mapping(values):
    allowed = {"fd", "t1"}
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--result", action="append", required=True, metavar="NAME=OUTPUT_ROOT",
        help="generated output root for fd and t1 (repeat twice)",
    )
    args = parser.parse_args()
    roots = parse_mapping(args.result)
    vectors = read_json(HERE / "vectors.json")
    fd_duration, fd_release, dlc = check_fd(roots["fd"], vectors)
    t1_transfers, t1_receptions = check_t1(roots["t1"], vectors)
    print(f"PASS CAN FD product projection: binding, DLC {dlc}, EOF {fd_duration}ps, release {fd_release}ps, four metrics")
    print(f"PASS 100BASE-T1 product projection: {t1_transfers} transfers/{t1_receptions} receptions, vector times and PHY metadata")
    print("PASS: schema-2 output checked against independent vectors; CAN FD bitstream conformance is not asserted")


if __name__ == "__main__":
    main()
