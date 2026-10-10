//! End-to-end runs compared with the independent network design vectors.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "dir-network-product-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn product(example: &str, limit: Option<u64>) -> Value {
    product_config(&root().join(example), limit)
}
fn product_config(config: &Path, limit: Option<u64>) -> Value {
    let output = Temp::new();
    let mut prepared = prepare(config).unwrap();
    if let Some(limit) = limit {
        prepared.common.time_limit_ps = limit;
    }
    run(prepared, &output.0).unwrap();
    serde_json::from_slice(&fs::read(output.0.join("results.json")).unwrap()).unwrap()
}
fn dynamic_fixture(config: &Value, workload: &Value) -> Temp {
    let fixture = Temp::new();
    fs::create_dir_all(&fixture.0).unwrap();
    fs::write(
        fixture.0.join("model.json"),
        serde_json::to_string_pretty(config).unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.0.join("workload.json"),
        serde_json::to_string_pretty(workload).unwrap(),
    )
    .unwrap();
    fs::write(fixture.0.join("run.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.l2.dynamic.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",root().join("examples/ethernet/dynamic/models").display())).unwrap();
    fixture
}
fn bridge_fixture(workload: &Value) -> Temp {
    let fixture = Temp::new();
    fs::create_dir_all(&fixture.0).unwrap();
    let example = root().join("examples/can-ethernet");
    fs::write(
        fixture.0.join("workload.json"),
        serde_json::to_string_pretty(workload).unwrap(),
    )
    .unwrap();
    let ini = fs::read_to_string(example.join("bidirectional.ini"))
        .unwrap()
        .replace(
            "\"models\"",
            &format!("\"{}\"", example.join("models").display()),
        )
        .replace(
            "\"r02-model.json\"",
            &format!("\"{}\"", example.join("r02-model.json").display()),
        )
        .replace("\"bidirectional-workload.json\"", "\"workload.json\"");
    fs::write(fixture.0.join("run.ini"), ini).unwrap();
    fixture
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
fn dynamic_triangle_matches_independent_three_hop_timing() {
    let expected: Value = serde_json::from_slice(
        &fs::read(
            root().join("docs/verification/fixtures/network-extensions/dynamic/expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let result = product("examples/ethernet/dynamic/unicast.ini", None);
    assert_eq!(result["schema_version"], 2);
    assert_eq!(
        result["metadata"]["model_profile"],
        "ethernet.l2.dynamic.v1"
    );
    let transfers = rows(&result, "ethernet.dynamic.transfer");
    assert_eq!(transfers.len(), 3);
    for hop in expected["hops"].as_array().unwrap() {
        let transfer = transfers
            .iter()
            .find(|row| row["data"]["from_port"] == hop["from"])
            .unwrap();
        for boundary in ["sof_ps", "eof_ps", "release_ps", "arrival_ps"] {
            assert_eq!(
                transfer["data"][boundary], hop[boundary],
                "{}:{boundary}",
                hop["from"]
            );
        }
    }
    let received = rows(&result, "ethernet.dynamic.reception")
        .into_iter()
        .find(|row| row["data"]["ingress"] == "Net.b.rx")
        .unwrap();
    assert_eq!(received["data"]["ready_ps"], "1831000");
    assert_eq!(received["data"]["status"], "received");
    assert!(
        !result["simulation"]["summary"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn time_limited_dynamic_run_keeps_planned_and_observed_receptions_separate() {
    let result = product("examples/ethernet/dynamic/unicast.ini", Some(700_000));
    assert_eq!(result["simulation"]["partial"], false);
    assert_eq!(result["simulation"]["termination"], "time_limit");
    assert_eq!(result["simulation"]["end_ps"], "700000");
    assert!(
        rows(&result, "ethernet.dynamic.reception")
            .iter()
            .all(|row| row["data"]["ingress"] != "Net.b.rx")
    );
    let waiting = rows(&result, "ethernet.dynamic.transfer")
        .into_iter()
        .find(|row| row["data"]["from_port"] == "Net.s1.tx_b")
        .unwrap();
    assert_eq!(waiting["data"]["sof_ps"], "611000");
    assert_eq!(waiting["data"]["eof_ps"], Value::Null);
    assert_eq!(waiting["data"]["planned_eof_ps"], "1219000");
}

#[test]
fn empty_and_control_only_workloads_run_on_the_same_prepared_profile() {
    for case in ["empty", "topology", "membership", "registration"] {
        let result = product(&format!("examples/ethernet/dynamic/{case}.ini"), None);
        assert_eq!(
            result["simulation"]["termination"], "events_exhausted",
            "{case}"
        );
        assert!(
            rows(&result, "ethernet.dynamic.policy")
                .iter()
                .any(|row| row["data"]["initial"] == true)
        );
        assert!(result["metadata"]["network_runtime"]["dynamic"].is_object());
    }
}

#[test]
fn tsn_wire_scheduling_matches_independent_tas_and_cbs_boundaries() {
    let expected: Value = serde_json::from_slice(
        &fs::read(root().join("docs/verification/fixtures/network-extensions/tsn/expected.json"))
            .unwrap(),
    )
    .unwrap();
    for (case, key) in [
        ("cbs", "cbs_sof_ps"),
        ("cbs-gated", "cbs_gated_sof_ps"),
        ("tas", "tas_sof_ps"),
        ("tas-update", "tas_update_sof_ps"),
    ] {
        let result = product(&format!("examples/ethernet/tsn/{case}.ini"), None);
        assert_eq!(
            result["simulation"]["termination"], "events_exhausted",
            "{case}"
        );
        let mut actual = rows(&result, "ethernet.dynamic.transfer")
            .into_iter()
            .filter(|row| row["data"]["from_port"] == "Net.a.tx")
            .map(|row| row["data"]["sof_ps"].clone())
            .collect::<Vec<_>>();
        actual.sort_by_key(|time| time.as_str().unwrap().parse::<u64>().unwrap());
        assert_eq!(Value::Array(actual), expected[key], "{case}");
        assert_eq!(
            result["metadata"]["model_schemas"]
                .as_array()
                .unwrap()
                .len(),
            9
        );
        if case == "cbs" {
            assert!(
                rows(&result, "ethernet.tsn.credit")
                    .iter()
                    .any(|row| row["time_ps"] == "672000"
                        && row["data"]["sign"] == "negative"
                        && row["data"]["magnitude"] == "504000000000000")
            );
        }
    }
}

#[test]
fn psfp_product_arrivals_consume_once_and_stream_gate_drops_before_meter() {
    let result = product("examples/ethernet/tsn/psfp.ini", None);
    let mut policing = rows(&result, "ethernet.tsn.policing");
    policing.sort_by_key(|row| row["time_ps"].as_str().unwrap().parse::<u64>().unwrap());
    assert_eq!(policing.len(), 5);
    let colors = policing
        .iter()
        .map(|row| row["data"]["color"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(colors, ["green", "yellow", "red", "yellow", "green"]);
    let consumed = policing
        .iter()
        .map(|row| row["data"]["consumed_bits"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(consumed, ["1024", "512", "0", "512", "1024"]);
    let gate = product("examples/ethernet/tsn/psfp-gate.ini", None);
    let rows = rows(&gate, "ethernet.tsn.policing");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["data"]["verdict"], "psfp_gate_closed");
    assert_eq!(rows[0]["data"]["committed_before"], Value::Null);
    assert_eq!(rows[0]["data"]["consumed_bits"], "0");
}

#[test]
fn can_ethernet_product_preserves_r01_and_r02_processing_and_wire_times() {
    for (case, ethernet_sof, can_sof, terminal_time) in [
        ("r01", "106000000", "0", "115576000"),
        ("r02", "0", "10608000", "115608000"),
    ] {
        let result = product(&format!("examples/can-ethernet/{case}.ini"), None);
        assert_eq!(
            result["simulation"]["termination"], "events_exhausted",
            "{case}"
        );
        let transfer = rows(&result, "ethernet.transfer")
            .into_iter()
            .find(|row| row["data"]["sof_ps"].is_string())
            .unwrap();
        assert_eq!(transfer["data"]["sof_ps"], ethernet_sof, "{case}");
        let request = rows(&result, "can.request")
            .into_iter()
            .find(|row| row["data"]["sof_ps"].is_string())
            .unwrap();
        assert_eq!(request["data"]["sof_ps"], can_sof, "{case}");
        let segments = rows(&result, "dir.can_ethernet.segment");
        assert!(
            segments
                .iter()
                .flat_map(|row| row["data"]["targets"].as_array().unwrap())
                .any(|target| target["completed_ps"] == terminal_time),
            "{case}"
        );
        let accepted = rows(&result, "dir.can_ethernet.conversion")
            .into_iter()
            .filter(|row| row["data"]["rule_id"].is_string())
            .collect::<Vec<_>>();
        assert_eq!(accepted.len(), 1, "{case}");
        assert_eq!(accepted[0]["data"]["status"], "released", "{case}");
    }
}

#[test]
fn registrable_source_and_fdb_use_effective_membership_and_drop_expired_queue_copy() {
    let mut config: Value = serde_json::from_slice(
        &fs::read(root().join("examples/ethernet/dynamic/model.json")).unwrap(),
    )
    .unwrap();
    let mut workload: Value = serde_json::from_slice(
        &fs::read(root().join("examples/ethernet/dynamic/unicast.json")).unwrap(),
    )
    .unwrap();
    for switch in config["switches"].as_array_mut().unwrap() {
        switch["fdb"][0]["vid"] = 20.into();
    }
    workload["generators"][0]["frame"]["tag"]["vid"] = 20.into();
    let inactive = dynamic_fixture(&config, &workload);
    let result = product_config(&inactive.0.join("run.ini"), None);
    let copies = rows(&result, "ethernet.dynamic.transfer");
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0]["data"]["drop_reason"], "vlan_unregistered");
    assert_eq!(copies[0]["data"]["sof_ps"], Value::Null);
    workload["controls"]=Value::Array(config["dynamic"]["registrable"].as_array().unwrap().iter().enumerate().map(|(index,registration)|serde_json::json!({"id":format!("register_{index}"),"at_ps":"0","kind":"vlan_register","port":registration["port"],"vid":20,"lifetime_ps":"10000000"})).collect());
    let active = dynamic_fixture(&config, &workload);
    let result = product_config(&active.0.join("run.ini"), None);
    assert!(
        rows(&result, "ethernet.dynamic.reception")
            .iter()
            .any(|row| row["data"]["ingress"] == "Net.b.rx" && row["data"]["status"] == "received")
    );
    // Only the source registration expires while the first copy is on wire.
    workload["generators"][0]["times_ps"] = serde_json::json!(["0", "0"]);
    for control in workload["controls"].as_array_mut().unwrap() {
        if control["port"] == "Net.a.tx" {
            control["lifetime_ps"] = "500000".into();
        }
    }
    let expiring = dynamic_fixture(&config, &workload);
    let result = product_config(&expiring.0.join("run.ini"), None);
    let source = rows(&result, "ethernet.dynamic.transfer")
        .into_iter()
        .filter(|row| row["data"]["from_port"] == "Net.a.tx")
        .collect::<Vec<_>>();
    assert_eq!(source.len(), 2);
    assert_eq!(
        source
            .iter()
            .filter(|row| row["data"]["sof_ps"].is_string())
            .count(),
        1
    );
    assert_eq!(
        source
            .iter()
            .filter(|row| row["data"]["drop_reason"] == "vlan_unregistered"
                && row["data"]["sof_ps"].is_null())
            .count(),
        1
    );
    // Capability declaration cannot make the PVID dynamically dependent.
    config["ports"][0]["pvid"] = 20.into();
    let invalid = dynamic_fixture(&config, &workload);
    let diagnostic = prepare(&invalid.0.join("run.ini")).unwrap_err();
    assert_eq!(diagnostic.target.as_deref(), Some("/ports/0/pvid"));
    assert_eq!(
        diagnostic.source.as_deref(),
        Some(invalid.0.join("model.json").to_string_lossy().as_ref())
    );
    assert!(diagnostic.line.is_some());
}

#[test]
fn bridge_fanout_rx_limit_and_partial_denominators_preserve_native_contracts() {
    let expected: Value = serde_json::from_slice(
        &fs::read(
            root().join("docs/verification/fixtures/network-extensions/can-ethernet/expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let fanout = product("examples/can-ethernet/fanout.ini", None);
    let requests = rows(&fanout, "can.request");
    for (bus, key) in [
        ("Fanout.bus_a", "can_a_eof_ps"),
        ("Fanout.bus_b", "can_b_eof_ps"),
    ] {
        let request = requests
            .iter()
            .find(|row| row["data"]["bus"] == bus)
            .unwrap();
        assert_eq!(request["data"]["eof_ps"], expected["R03"][key]);
    }
    let limited = product("examples/can-ethernet/rx-limit.ini", None);
    let conversions = rows(&limited, "dir.can_ethernet.conversion");
    assert_eq!(
        conversions.len(),
        expected["rx_limit"]["attempted"].as_u64().unwrap() as usize
    );
    assert_eq!(
        conversions
            .iter()
            .filter(|row| row["data"]["reason"] == "rx_full")
            .count(),
        2
    );
    let partial = product("examples/can-ethernet/r01.ini", Some(101_000_000));
    assert_eq!(partial["simulation"]["partial"], false);
    assert_eq!(
        rows(&partial, "can.receiver")[0]["data"]["status"],
        "pending"
    );
    let summary = partial["simulation"]["summary"].as_array().unwrap();
    let metric = summary
        .iter()
        .find(|row| row["metric"] == "gateway.end_to_end.completion_ratio")
        .unwrap();
    assert_eq!(metric["value"], Value::Null);
    let descriptor = partial["metadata"]["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["metric_id"] == "gateway.end_to_end.completion_ratio")
        .unwrap();
    assert_eq!(descriptor["value_kind"], "number");
    assert!(rows(&partial, "ethernet.reception").is_empty());
}

#[test]
fn dynamic_large_cost_tree_and_at_prefixed_control_id_remain_valid() {
    let mut config: Value = serde_json::from_slice(
        &fs::read(root().join("examples/ethernet/dynamic/model.json")).unwrap(),
    )
    .unwrap();
    for link in config["dynamic"]["links"].as_array_mut().unwrap() {
        if link["id"] == "L12" {
            link["cost"] = "9223372036854775808".into();
        }
        if link["id"] == "L13" || link["id"] == "L23" {
            link["up"] = false.into();
        }
    }
    let mut workload = serde_json::json!({"schema_version":4,"generators":[],"controls":[{"id":"@maintenance","at_ps":"100","kind":"link_set","link":"L12","up":true}]});
    let fixture = dynamic_fixture(&config, &workload);
    let result = product_config(&fixture.0.join("run.ini"), None);
    let controls = rows(&result, "ethernet.dynamic.control");
    assert!(controls.iter().any(|row| row["record_id"] == "@maintenance"
        && row["data"]["control_id"] == "@maintenance"
        && row["data"]["outcome"] == "no_op"));
    for link in config["dynamic"]["links"].as_array_mut().unwrap() {
        if link["id"] == "L12" {
            link["up"] = false.into();
        }
    }
    workload["controls"][0]["id"] = "enable".into();
    let fixture = dynamic_fixture(&config, &workload);
    let result = product_config(&fixture.0.join("run.ini"), None);
    assert_eq!(result["simulation"]["termination"], "events_exhausted");
    assert_eq!(
        result["metadata"]["network_runtime"]["dynamic"]["policy"]["roles"]["Net.s2.tx_b"],
        "root"
    );
}

#[test]
fn identical_native_generator_ids_keep_media_qualified_origins_and_latencies() {
    for (can_time, ethernet_time) in [(200_000_000, 0), (0, 200_000_000)] {
        let fixture = Temp::new();
        fs::create_dir_all(&fixture.0).unwrap();
        let example = root().join("examples/can-ethernet");
        let mut workload: Value =
            serde_json::from_slice(&fs::read(example.join("bidirectional-workload.json")).unwrap())
                .unwrap();
        workload["can"]["generators"][0]["id"] = "shared".into();
        workload["ethernet"]["generators"][0]["id"] = "shared".into();
        workload["can"]["generators"][0]["start"] = format!("{can_time}ps").into();
        workload["ethernet"]["generators"][0]["times_ps"] =
            serde_json::json!([ethernet_time.to_string()]);
        fs::write(
            fixture.0.join("workload.json"),
            serde_json::to_string_pretty(&workload).unwrap(),
        )
        .unwrap();
        let ini = fs::read_to_string(example.join("bidirectional.ini"))
            .unwrap()
            .replace(
                "\"models\"",
                &format!("\"{}\"", example.join("models").display()),
            )
            .replace(
                "\"r02-model.json\"",
                &format!("\"{}\"", example.join("r02-model.json").display()),
            )
            .replace("\"bidirectional-workload.json\"", "\"workload.json\"");
        fs::write(fixture.0.join("run.ini"), ini).unwrap();
        let result = product_config(&fixture.0.join("run.ini"), None);
        assert!(
            rows(&result, "can.request")
                .iter()
                .any(|row| row["record_id"] == "shared:0")
        );
        assert!(
            rows(&result, "ethernet.frame")
                .iter()
                .any(|row| row["record_id"] == "shared:0")
        );
        let segments = rows(&result, "dir.can_ethernet.segment");
        for (origin, generated) in [
            ("can:shared:0", can_time),
            ("ethernet:shared:0", ethernet_time),
        ] {
            let owned = segments
                .iter()
                .filter(|row| row["data"]["origin_id"] == origin)
                .collect::<Vec<_>>();
            assert!(!owned.is_empty(), "{origin}");
            let mut completed = 0;
            for segment in owned {
                for target in segment["data"]["targets"].as_array().unwrap() {
                    if let Some(time) = target["completed_ps"].as_str() {
                        completed += 1;
                        let latency = time.parse::<u64>().unwrap() - generated;
                        let summary = result["simulation"]["summary"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|row| {
                                row["metric"] == "gateway.end_to_end.latency_ps"
                                    && row["reason"] == segment["record_id"]
                                    && row["receiver"] == target["terminal_id"]
                            })
                            .unwrap();
                        assert_eq!(summary["value"], latency.to_string());
                    }
                }
            }
            assert!(completed > 0, "{origin}");
        }
    }
}

#[test]
fn network_prepare_diagnostics_use_original_json_source_and_pointer() {
    let config: Value = serde_json::from_slice(
        &fs::read(root().join("examples/ethernet/dynamic/model.json")).unwrap(),
    )
    .unwrap();
    let mut workload: Value = serde_json::from_slice(
        &fs::read(root().join("examples/ethernet/dynamic/unicast.json")).unwrap(),
    )
    .unwrap();
    workload["generators"][0]["flow_id"] = true.into();
    let fixture = dynamic_fixture(&config, &workload);
    let diagnostic = prepare(&fixture.0.join("run.ini")).unwrap_err();
    assert_eq!(
        diagnostic.source.as_deref(),
        Some(fixture.0.join("workload.json").to_string_lossy().as_ref())
    );
    assert_eq!(diagnostic.target.as_deref(), Some("/generators/0/flow_id"));
    let text = fs::read_to_string(fixture.0.join("workload.json")).unwrap();
    assert!(
        text.lines()
            .nth(diagnostic.line.unwrap() - 1)
            .unwrap()
            .contains("flow_id")
    );
    let mut workload: Value = serde_json::from_slice(
        &fs::read(root().join("examples/can-ethernet/bidirectional-workload.json")).unwrap(),
    )
    .unwrap();
    workload["can"]["generators"][0]["frame"]["format"] = "unsupported".into();
    let fixture = bridge_fixture(&workload);
    let diagnostic = prepare(&fixture.0.join("run.ini")).unwrap_err();
    assert_eq!(
        diagnostic.source.as_deref(),
        Some(fixture.0.join("workload.json").to_string_lossy().as_ref())
    );
    assert_eq!(
        diagnostic.target.as_deref(),
        Some("/can/generators/0/frame/format")
    );
    let text = fs::read_to_string(fixture.0.join("workload.json")).unwrap();
    assert!(
        text.lines()
            .nth(diagnostic.line.unwrap() - 1)
            .unwrap()
            .contains("format")
    );
}
