//! Product runs checked against independently authored Ethernet golden fixtures.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-ethernet-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs/verification/fixtures/ethernet")
}
fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn rows<'a>(result: &'a Value, schema: &str) -> Vec<&'a Value> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["schema_name"] == schema)
        .collect()
}

#[test]
fn frame_serializer_matches_independent_golden_bytes_and_crc() {
    let vectors = load(&fixtures().join("vectors.json"));
    assert_eq!(vectors["vectors"].as_array().unwrap().len(), 4);
    for vector in vectors["vectors"].as_array().unwrap() {
        let actual = dir_simulator::runtime::ethernet::serialize_frame(
            vector["src_mac"].as_str().unwrap(),
            vector["dst_mac"].as_str().unwrap(),
            vector["ether_type"].as_u64().unwrap() as u16,
            vector["data"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(actual.fcs_hex, vector["fcs_hex"].as_str().unwrap());
        assert_eq!(actual.mac_hex, vector["mac_hex"].as_str().unwrap());
        assert_eq!(actual.pad_bytes, vector["pad_bytes"].as_u64().unwrap());
        assert_eq!(actual.mac_bytes, vector["mac_bytes"].as_u64().unwrap());
    }
    assert_eq!(
        dir_simulator::runtime::ethernet::crc32(b"123456789"),
        0xcbf43926
    );
}

#[test]
fn unicast_metrics_keep_mac_occupancy_and_delivery_boundaries_separate() {
    let output = Temp::new();
    let prepared = prepare(&fixtures().join("unicast.ini")).unwrap();
    run(prepared, &output.0).unwrap();
    let result = load(&output.0.join("results.json"));
    let summary = result["simulation"]["summary"].as_array().unwrap();
    let metric = |target: &str, name: &str| {
        summary
            .iter()
            .find(|row| row["target"] == target && row["metric"] == name)
            .unwrap()
    };
    assert_eq!(metric("Main.a.tx", "ethernet.mac_bits")["value"], "512");
    assert_eq!(metric("Main.a.tx", "ethernet.wire_bits")["value"], "576");
    assert_eq!(
        metric("Main.a.tx", "ethernet.occupied_bits")["value"],
        "672"
    );
    assert_eq!(metric("Main.a.tx", "ethernet.payload_bits")["value"], "0");
    assert_eq!(
        metric("Main.a.tx", "ethernet.link_utilization")["value"].as_f64(),
        Some(0.224)
    );
    assert_eq!(
        metric("Main.b", "ethernet.delivery_mean_ps")["value"].as_f64(),
        Some(1_156_000.0)
    );
    assert_eq!(
        metric("Main.b", "ethernet.delivery_mean_ps")["sample_count"],
        "1"
    );
    assert!(metric("Main.c", "ethernet.delivery_mean_ps")["value"].is_null());
    assert!(
        result["metadata"]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| {
                row["metric_id"].as_str().unwrap().starts_with("ethernet.")
                    || ["queue_length", "queue_max", "queue_mean"]
                        .contains(&row["metric_id"].as_str().unwrap())
            })
    );
    assert!(
        !result["metadata"]["models"][0]["assumptions"]
            .to_string()
            .contains("ideal ACK")
    );
    assert_eq!(
        result["metadata"]["ethernet_topology"]["devices"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        result["metadata"]["ethernet_topology"]["directions"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
}

#[test]
fn eof_boundary_has_no_completed_bits_or_fabricated_reception() {
    let output = Temp::new();
    run(
        prepare(&fixtures().join("eof-boundary.ini")).unwrap(),
        &output.0,
    )
    .unwrap();
    let result = load(&output.0.join("results.json"));
    assert!(rows(&result, "ethernet.reception").is_empty());
    let transfer = rows(&result, "ethernet.transfer")[0];
    assert!(transfer["data"]["eof_ps"].is_null());
    assert_eq!(transfer["data"]["planned_eof_ps"], "576000");
    let summary = result["simulation"]["summary"].as_array().unwrap();
    assert_eq!(
        summary
            .iter()
            .find(|row| row["target"] == "Main.a.tx" && row["metric"] == "ethernet.wire_bits")
            .unwrap()["value"],
        "0"
    );
    assert_eq!(summary.iter().find(|row|row["target"] == "Main.a.tx" && row["metric"] == "ethernet.link_utilization").unwrap()["value"].as_f64(), Some(1.0));
}

#[test]
fn ethernet_product_matches_all_existing_golden_scenarios() {
    let catalog = load(&fixtures().join("scenarios.json"));
    for scenario in catalog["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let config = fixtures().join(scenario["config"].as_str().unwrap());
        let expected = &scenario["expected"];
        if expected["termination"] == "prep_failed" {
            let error = prepare(&config).unwrap_err();
            assert_eq!(error.code, expected["code"].as_str().unwrap(), "{name}");
            continue;
        }
        let output = Temp::new();
        let prepared = prepare(&config).unwrap_or_else(|error| panic!("{name}: {error}"));
        let report = run(prepared, &output.0).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(report.exit_code, 0, "{name}");
        let result = load(&output.0.join("results.json"));
        assert_eq!(result["schema_version"], 2, "{name}");
        assert_eq!(result["metadata"]["model_profile"], catalog["profile"]);
        let frames = rows(&result, "ethernet.frame");
        let transfers = rows(&result, "ethernet.transfer");
        let receptions = rows(&result, "ethernet.reception");
        for (key, count) in [
            ("generated", frames.len()),
            ("offered", transfers.len()),
            ("receptions", receptions.len()),
        ] {
            if let Some(expected_count) = expected[key].as_u64() {
                assert_eq!(count as u64, expected_count, "{name}: {key}");
            }
        }
        for (states, records) in [
            (
                ["queued", "transmitting", "serialized", "dropped"],
                &transfers,
            ),
            (
                ["processing", "received", "filtered", "forwarded"],
                &receptions,
            ),
        ] {
            for state in states {
                if let Some(count) = expected[state].as_u64() {
                    assert_eq!(
                        records
                            .iter()
                            .filter(|r| r["data"]["status"] == state)
                            .count() as u64,
                        count,
                        "{name}: {state}"
                    );
                }
            }
        }
        if let Some(times) = expected["transfer_times"].as_array() {
            for golden in times {
                let row = transfers
                    .iter()
                    .find(|r| r["record_id"] == golden["id"])
                    .unwrap();
                for (key, value) in golden.as_object().unwrap() {
                    if key != "id" {
                        assert_eq!(&row["data"][key], value, "{name}: {} {key}", golden["id"]);
                    }
                }
            }
        }
        if let Some(deliveries) = expected["deliveries"].as_array() {
            for golden in deliveries {
                let row = receptions
                    .iter()
                    .find(|r| {
                        r["request_id"] == golden["frame_id"]
                            && r["subject"] == golden["node"]
                            && r["data"]["status"] == "received"
                    })
                    .unwrap();
                assert_eq!(row["data"]["ready_ps"], golden["received_ps"], "{name}");
            }
        }
        if !expected["termination"].is_null() {
            assert_eq!(
                result["simulation"]["termination"], expected["termination"],
                "{name}"
            );
        }
        if let Some(drop_ids) = expected["drop_ids"].as_array() {
            let mut actual: Vec<_> = transfers
                .iter()
                .filter(|r| r["data"]["status"] == "dropped")
                .map(|r| r["record_id"].as_str().unwrap())
                .collect();
            actual.sort();
            let mut golden: Vec<_> = drop_ids.iter().map(|v| v.as_str().unwrap()).collect();
            golden.sort();
            assert_eq!(actual, golden, "{name}");
        }
        if let Some(reason) = expected["filter_reason"].as_str() {
            assert!(
                receptions
                    .iter()
                    .filter(|r| r["data"]["status"] == "filtered")
                    .all(|r| r["data"]["reason"] == reason),
                "{name}"
            );
        }
        if let Some(time) = expected["delivery_ps"].as_str() {
            assert!(
                receptions
                    .iter()
                    .filter(|r| r["data"]["status"] == "received")
                    .all(|r| r["data"]["ready_ps"] == time),
                "{name}"
            );
        }
        let keys: std::collections::BTreeSet<_> = result["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["schema_name"].as_str().unwrap(),
                    r["record_id"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            keys.len(),
            frames.len() + transfers.len() + receptions.len(),
            "{name}: unique IDs"
        );
        assert_eq!(
            transfers.len(),
            transfers
                .iter()
                .filter(|r| ["queued", "transmitting", "serialized", "dropped"]
                    .contains(&r["data"]["status"].as_str().unwrap()))
                .count(),
            "{name}: transfer conservation"
        );
        assert_eq!(
            transfers
                .iter()
                .filter(|r| !r["data"]["arrival_ps"].is_null())
                .count(),
            receptions.len(),
            "{name}: arrival conservation"
        );
        for row in &transfers {
            assert!(
                frames
                    .iter()
                    .any(|f| f["record_id"] == row["data"]["frame_id"]),
                "{name}: frame reference"
            );
        }
    }
}

#[test]
fn failure_boundary_counts_committed_eof_and_release_only_in_summary() {
    for (milestone, bit_metric, bits) in [
        ("eof_ps", "ethernet.wire_bits", "576"),
        ("release_ps", "ethernet.occupied_bits", "672"),
    ] {
        let mut prepared = prepare(&fixtures().join("duplex.ini")).unwrap();
        for generator in &mut prepared.ethernet.as_mut().unwrap().generators {
            generator.frame = dir_simulator::runtime::ethernet::serialize_frame(
                &generator.frame.src_mac,
                &generator.frame.dst_mac,
                generator.frame.ether_type,
                "",
            )
            .unwrap();
        }
        let snapshot = (1..40)
            .find_map(|limit| {
                prepared.common.max_events = limit;
                let snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
                let transfer = snapshot
                    .ethernet
                    .as_ref()
                    .unwrap()
                    .transfers
                    .iter()
                    .find(|t| t.from_port == "Main.a.tx")?;
                let time = if milestone == "eof_ps" {
                    transfer.eof_ps
                } else {
                    transfer.release_ps
                };
                (snapshot.common.partial && time == Some(snapshot.common.end_ps))
                    .then_some(snapshot)
            })
            .expect("a committed boundary milestone followed by a failed callback");
        let output = Temp::new();
        fs::create_dir(&output.0).unwrap();
        dir_simulator::output::export(&prepared, &snapshot, &output.0).unwrap();
        let result = load(&output.0.join("results.json"));
        let summary = result["simulation"]["summary"].as_array().unwrap();
        let row = summary
            .iter()
            .find(|row| row["target"] == "Main.a.tx" && row["metric"] == bit_metric)
            .unwrap();
        assert_eq!(row["value"], bits);
        let windows = result["simulation"]["records"].as_array().unwrap();
        assert!(
            windows
                .iter()
                .filter(|row| row["target"] == "Main.a.tx" && row["metric"] == bit_metric)
                .all(|row| row["value"] == "0")
        );
    }
}

#[test]
fn qos_metadata_preserves_future_candidate_without_invalid_u64_time() {
    use dir_simulator::types::ethernet::EthernetSchedule;
    let config =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ethernet/qos-priority.ini");
    let mut prepared = prepare(&config).unwrap();
    prepared.common.time_limit_ps = u64::MAX;
    prepared.common.metrics_window_ps = u64::MAX;
    let generator = &mut prepared.ethernet.as_mut().unwrap().generators[0];
    generator.schedule = Some(EthernetSchedule::Periodic {
        start_ps: u64::MAX,
        phase_ps: 1,
        period_ps: 2,
        end_ps: None,
        count: None,
    });
    let source = generator.source;
    let device_id = prepared.ethernet.as_ref().unwrap().devices[source]
        .id
        .clone();
    let output = Temp::new();
    run(prepared, &output.0).unwrap();
    let result = load(&output.0.join("results.json"));
    let initial = result["metadata"]["initial_state"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["instance"] == device_id)
        .unwrap();
    let state: Value = serde_json::from_str(initial["state"].as_str().unwrap()).unwrap();
    let cursor = &state["generators"][0];
    assert!(cursor["next_time_ps"].is_null());
    assert_eq!(cursor["next_candidate_ps"], "18446744073709551616");
}
