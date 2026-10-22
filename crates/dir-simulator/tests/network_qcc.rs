//! Public prepare-path coverage for the local fully centralized Qcc profile.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample() -> PathBuf {
    root().join("examples/network-composite/qcc")
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::var_os("DIR_QCC_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "dir-qcc-prepare-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if std::env::var_os("DIR_QCC_EVIDENCE").is_none() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[test]
fn two_distinct_priority_streams_prepare_on_one_managed_output() {
    prepare(&sample().join("model.ini")).unwrap();
    let workload: Value =
        serde_json::from_slice(&fs::read(sample().join("workload.json")).unwrap()).unwrap();
    let generators = workload["generators"].as_array().unwrap();
    assert_eq!(generators.len(), 2);
    assert_eq!(generators[0]["node"], "Net.a");
    assert_eq!(generators[1]["node"], "Net.a");
    assert_ne!(generators[0]["priority"], generators[1]["priority"]);
}

#[test]
fn local_prepare_accept_install_admits_both_priorities_and_delivers_frames() {
    let output = Temp::new();
    run(
        prepare(&sample().join("model.ini")).unwrap(),
        &output.0.join("run"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(output.0.join("run/results.json")).unwrap()).unwrap();
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let qcc = |kind: &str, at: &str| {
        rows.iter().any(|row| {
            row["schema_name"] == "dir.network.qcc"
                && row["data"]["kind"] == kind
                && row["time_ps"] == at
        })
    };
    assert!(qcc("prepared", "10000000"));
    assert_eq!(
        rows.iter()
            .filter(|row| row["schema_name"] == "dir.network.qcc"
                && row["data"]["kind"] == "participant_reply"
                && row["data"]["accepted"] == true
                && row["time_ps"] == "11000000")
            .count(),
        3
    );
    assert!(qcc("installed", "100000000"));
    for (stream, assigned) in [("high", "0"), ("low", "704000")] {
        assert!(
            rows.iter()
                .filter(|row| row["schema_name"] == "dir.network.qcc"
                    && row["data"]["kind"] == "source_sof"
                    && row["data"]["stream"] == stream
                    && row["data"]["assigned_offset_ps"] == assigned
                    && row["data"]["within_certificate"] == true)
                .count()
                == 2
        );
    }
    for request in ["high:0", "high:1", "low:0", "low:1"] {
        assert!(rows.iter().any(|row| {
            row["schema_name"] == "dir.network.qcc"
                && row["data"]["kind"] == "source_admitted"
                && row["request_id"] == request
        }));
        assert!(rows.iter().any(|row| {
            row["schema_name"] == "ethernet.dynamic.reception"
                && row["request_id"] == request
                && row["data"]["ingress"] == "Net.b.rx"
                && row["data"]["status"] == "received"
        }));
        assert!(
            rows.iter()
                .any(|row| row["schema_name"] == "dir.network.qcc"
                    && row["data"]["kind"] == "delivery"
                    && row["request_id"] == request
                    && row["data"]["within_certificate"] == true)
        );
    }
}

#[test]
fn distributed_architecture_is_rejected_at_prepare() {
    let t = Temp::new();
    let mut model: Value =
        serde_json::from_slice(&fs::read(sample().join("model.json")).unwrap()).unwrap();
    model["qcc"]["architecture"] = "distributed".into();
    fs::write(t.0.join("model.json"), model.to_string()).unwrap();
    fs::copy(sample().join("workload.json"), t.0.join("workload.json")).unwrap();
    fs::write(
        t.0.join("model.ini"),
        format!(
            "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 500000000ps\n",
            root().join("examples/ethernet/dynamic/models").display()
        ),
    )
    .unwrap();
    let diagnostic = prepare(&t.0.join("model.ini")).unwrap_err();
    assert_eq!(diagnostic.target.as_deref(), Some("/qcc"));
    assert!(diagnostic.message.contains("unsupported Qcc architecture"));
}

#[test]
fn link_loss_during_serialization_suspends_contract_without_stopping_run() {
    let t = Temp::new();
    let model: Value =
        serde_json::from_slice(&fs::read(sample().join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(sample().join("workload.json")).unwrap()).unwrap();
    workload["controls"] = serde_json::json!([{
        "id":"break-A", "at_ps":"200100000", "kind":"link_set", "link":"A", "up":false
    }]);
    fs::write(t.0.join("model.json"), model.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), workload.to_string()).unwrap();
    fs::write(t.0.join("model.ini"), format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 500000000ps\n",
        root().join("examples/ethernet/dynamic/models").display())).unwrap();
    run(
        prepare(&t.0.join("model.ini")).unwrap(),
        &t.0.join("result"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(t.0.join("result/results.json")).unwrap()).unwrap();
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["schema_name"] == "dir.network.qcc"
                && row["data"]["kind"] == "suspended"
                && row["time_ps"] == "200100000")
    );
    assert!(
        rows.iter()
            .any(|row| row["schema_name"] == "dir.network.qcc"
                && row["data"]["kind"] == "source_rejected"
                && row["data"]["reason"] == "inactive_stream")
    );
    assert_eq!(result["simulation"]["termination"], "events_exhausted");
}

#[test]
fn unbound_ingress_reclassification_cannot_enter_a_reserved_qcc_class() {
    for default_priority in [0, 5] {
        let t = Temp::new();
        let mut model: Value =
            serde_json::from_slice(&fs::read(sample().join("model.json")).unwrap()).unwrap();
        let mut workload: Value =
            serde_json::from_slice(&fs::read(sample().join("workload.json")).unwrap()).unwrap();
        for port in model["base"]["config"]["base"]["config"]["ports"]
            .as_array_mut()
            .unwrap()
        {
            if port["port"] == "Net.c.tx" {
                port["vlans"][0]["tagged"] = false.into();
            }
            if port["port"] == "Net.s3.tx_b" {
                port["vlans"][0]["tagged"] = false.into();
            }
            if port["port"] == "Net.s1.tx_c" {
                port["default_priority"] = default_priority.into();
            }
        }
        let mut background = workload["generators"][0].clone();
        background["id"] = "background".into();
        background["flow_id"] = "background".into();
        background["node"] = "Net.c".into();
        background["priority"] = 0.into();
        background["frame"]["tag"] = Value::Null;
        background["times_ps"] = serde_json::json!(["201000000"]);
        workload["generators"]
            .as_array_mut()
            .unwrap()
            .push(background);
        fs::write(t.0.join("model.json"), model.to_string()).unwrap();
        fs::write(t.0.join("workload.json"), workload.to_string()).unwrap();
        fs::write(t.0.join("model.ini"), format!(
            "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 500000000ps\n",
            root().join("examples/ethernet/dynamic/models").display())).unwrap();
        run(
            prepare(&t.0.join("model.ini")).unwrap(),
            &t.0.join("result"),
        )
        .unwrap();
        let result: Value =
            serde_json::from_slice(&fs::read(t.0.join("result/results.json")).unwrap()).unwrap();
        let rows = result["simulation"]["model_records"].as_array().unwrap();
        let transfers: Vec<_> = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "ethernet.dynamic.transfer"
                    && r["data"]["frame_id"] == "background:0"
            })
            .collect();
        assert!(
            transfers
                .iter()
                .any(|r| r["data"]["from_port"] == "Net.c.tx"
                    && r["data"]["priority"] == "0"
                    && r["data"]["wire"]["tag"].is_null())
        );
        let managed: Vec<_> = transfers
            .iter()
            .filter(|r| r["data"]["from_port"] == "Net.s1.tx_b")
            .collect();
        assert_eq!(managed.len(), 1);
        assert_eq!(managed[0]["data"]["priority"], default_priority.to_string());
        if default_priority == 5 {
            assert_eq!(managed[0]["data"]["drop_reason"], "qcc_unbound_class");
            assert!(managed[0]["data"]["sof_ps"].is_null());
        } else {
            assert_eq!(managed[0]["data"]["status"], "serialized");
        }
        assert_eq!(
            rows.iter()
                .filter(|r| r["schema_name"] == "ethernet.dynamic.reception"
                    && r["data"]["frame_id"] == "background:0"
                    && r["data"]["status"] == "received")
                .count(),
            usize::from(default_priority == 0)
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r["schema_name"] == "dir.network.qcc"
                    && r["data"]["kind"] == "delivery"
                    && r["data"]["within_certificate"] == true)
                .count(),
            4
        );
    }
}

fn variant(model: &Value, workload: &Value) -> (Temp, Value) {
    let t = Temp::new();
    fs::write(t.0.join("model.json"), model.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), workload.to_string()).unwrap();
    fs::write(t.0.join("model.ini"), format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 500000000ps\n",
        root().join("examples/ethernet/dynamic/models").display())).unwrap();
    run(
        prepare(&t.0.join("model.ini")).unwrap(),
        &t.0.join("result"),
    )
    .unwrap();
    let result =
        serde_json::from_slice(&fs::read(t.0.join("result/results.json")).unwrap()).unwrap();
    (t, result)
}
fn sample_values() -> (Value, Value) {
    (
        serde_json::from_slice(&fs::read(sample().join("model.json")).unwrap()).unwrap(),
        serde_json::from_slice(&fs::read(sample().join("workload.json")).unwrap()).unwrap(),
    )
}
#[test]
fn qcc_static_affine_clock_installs_projected_gcl_and_certifies_real_callbacks() {
    let (mut model, workload) = sample_values();
    model["clocks"]["nodes"] = serde_json::json!([
        {"node":"Net.a","offset_ps":"1700","rate_num":"3","rate_den":"2"},
        {"node":"Net.s1","offset_ps":"2900","rate_num":"1","rate_den":"1"}
    ]);
    let (_t, result) = variant(&model, &workload);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "source_sof"
                && r["data"]["within_certificate"] == true)
            .count(),
        4
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        4
    );
}
#[test]
fn qcc_scripted_clock_dependency_suspends_then_actual_transaction_renews() {
    let (mut model, workload) = sample_values();
    model["clocks"]["adjustments"] = serde_json::json!([
        {"node":"Net.a","at_ps":"250000000","kind":"step","offset_ps":"1700"}
    ]);
    model["qcc"]["operations"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "operation":"prepare","id":"renew","at_ps":"260000000","expected_topology_epoch":"1",
            "add":[],"remove":[],"activate_at_ps":"300000000","timeout_at_ps":"280000000"
        }));
    let (_t, result) = variant(&model, &workload);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
        && r["data"]["kind"] == "suspended"
        && r["time_ps"] == "250000000"));
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
        && r["data"]["kind"] == "installed"
        && r["data"]["version"] == "2"));
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        4
    );
}
#[test]
fn qcc_external_tas_ownership_rejects_request_and_keeps_graph_running() {
    let (mut model, workload) = sample_values();
    model["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["port"] == "Net.s1.tx_b")
        .unwrap()["tas"] = serde_json::json!({
        "id":"external","base_time_ps":"0","cycle_time_ps":"100000000",
        "entries":[{"duration_ps":"100000000","open_priorities":[0,1,2,3,4,5,6,7]}]
    });
    let (_t, result) = variant(&model, &workload);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
        && r["data"]["kind"] == "rejected"
        && r["data"]["reason"]["no_proven_bound"] == "schedule_ownership"));
    assert!(
        !rows
            .iter()
            .any(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
    );
    assert_eq!(result["simulation"]["termination"], "events_exhausted");
}

fn custom_variant(
    model: &Value,
    workload: &Value,
    network: &str,
    ned: &Path,
    limit: u64,
) -> (Temp, Value) {
    let t = Temp::new();
    fs::write(t.0.join("model.json"), model.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), workload.to_string()).unwrap();
    fs::write(t.0.join("model.ini"), format!(
        "[General]\nnetwork = {network}\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {limit}ps\n", ned.display())).unwrap();
    run(
        prepare(&t.0.join("model.ini")).unwrap(),
        &t.0.join("result"),
    )
    .unwrap();
    let result =
        serde_json::from_slice(&fs::read(t.0.join("result/results.json")).unwrap()).unwrap();
    (t, result)
}
#[test]
fn qcc_shared_physical_junction_is_one_transmission_and_actual_listener_delivery() {
    let directory = root().join("examples/network-composite/shared-lan");
    let mut model: Value =
        serde_json::from_slice(&fs::read(directory.join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(directory.join("workload.json")).unwrap()).unwrap();
    let (qcc, _) = sample_values();
    model["qcc"] = qcc["qcc"].clone();
    model["qcc"]["requirements"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let requirement = &mut model["qcc"]["requirements"][0];
    requirement["id"] = "data".into();
    requirement["generator"] = "data".into();
    model["qcc"]["operations"][0]["add"] = serde_json::json!(["data"]);
    workload["generators"][0]["times_ps"] = serde_json::json!(["200000000"]);
    let (_t, result) = custom_variant(
        &model,
        &workload,
        "ethshared.Net",
        &directory.join("models"),
        250_000_000,
    );
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(
                |r| r["schema_name"] == "dir.network.segment" && r["data"]["kind"] == "tx_started"
            )
            .count(),
        1
    );
}
#[test]
fn qcc_deterministic_sampled_phy_admits_and_noise_rejects_only_request() {
    for noisy in [false, true] {
        let (mut model, mut workload) = sample_values();
        let sampled: Value = serde_json::from_slice(
            &fs::read(root().join("examples/network-composite/sampled-phy-model.json")).unwrap(),
        )
        .unwrap();
        model["phy_attachments"] = sampled["phy_attachments"].clone();
        model["phy_attachments"][0]["origin_ps"] = 7_999_000.into();
        model["phy_attachments"][0]["medium"]["maximum_samples"] = 32_008.into();
        for endpoint in model["phy_attachments"][0]["medium"]["endpoints"]
            .as_array_mut()
            .unwrap()
        {
            endpoint["receiver"]["adaptation_q30"] = 0.into();
            endpoint["receiver"]["max_tracking_ppm"] = 0.into();
        }
        if noisy {
            model["phy_attachments"][0]["medium"]["channel"]["noise_rms_uv"][1] = 1000.into();
        }
        model["qcc"]["cycle_ps"] = "4000000".into();
        model["qcc"]["management_latency_ps"] = "1000".into();
        model["qcc"]["requirements"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        model["qcc"]["requirements"][0]["interval"]["denominator"] = 250000.into();
        model["qcc"]["requirements"][0]["latest_transmit_offset_ps"] = "3000000".into();
        model["qcc"]["requirements"][0]["max_latency_ps"] = "3900000".into();
        model["qcc"]["operations"][0]["at_ps"] = "0".into();
        model["qcc"]["operations"][0]["activate_at_ps"] = "4000000".into();
        model["qcc"]["operations"][0]["timeout_at_ps"] = "1000000".into();
        model["qcc"]["operations"][0]["add"] = serde_json::json!(["high"]);
        workload["generators"].as_array_mut().unwrap().truncate(1);
        workload["generators"][0]["times_ps"] = serde_json::json!(["8000000"]);
        let (_t, result) = custom_variant(
            &model,
            &workload,
            "ethdynamic.Net",
            &root().join("examples/ethernet/dynamic/models"),
            12_000_000,
        );
        let rows = result["simulation"]["model_records"].as_array().unwrap();
        if noisy {
            assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "rejected"
                && r["data"]["reason"]["no_proven_bound"] == "physical_noise_or_retry"));
        } else {
            assert!(
                rows.iter()
                    .any(|r| r["schema_name"] == "dir.network.qcc"
                        && r["data"]["kind"] == "installed"),
                "{rows:?}"
            );
            assert!(
                rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
                    && r["data"]["kind"] == "delivery"
                    && r["data"]["within_certificate"] == true),
                "sampled receive must deliver inside its bound"
            );
        }
    }
}

#[test]
fn qcc_provider_wire_growth_is_reserved_and_actual_service_path_delivers() {
    let directory = root().join("examples/network-composite/provider-l2");
    let mut model: Value =
        serde_json::from_slice(&fs::read(directory.join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(directory.join("workload.json")).unwrap()).unwrap();
    let (qcc, _) = sample_values();
    model["qcc"] = qcc["qcc"].clone();
    model["qcc"]["requirements"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let requirement = &mut model["qcc"]["requirements"][0];
    requirement["id"] = "a_data".into();
    requirement["generator"] = "a_data".into();
    requirement["max_frame_size"] = 60.into();
    requirement["listeners"] = serde_json::json!(["Net.a3"]);
    requirement["paths"] = serde_json::json!([{"listener":"Net.a3","ports":["Net.a0.tx","Net.east.tx_pnp","Net.west.tx_a3"]}]);
    model["qcc"]["operations"][0]["add"] = serde_json::json!(["a_data"]);
    let mut generator = workload["generators"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "a_data")
        .unwrap()
        .clone();
    generator["times_ps"] = serde_json::json!(["200000000"]);
    workload["generators"] = serde_json::json!([generator]);
    let (_t, result) = custom_variant(
        &model,
        &workload,
        "ethprovider.Net",
        &directory.join("models"),
        250_000_000,
    );
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let install = rows
        .iter()
        .find(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
        .expect("provider request must admit");
    let transfers: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
        .map(|r| &r["data"])
        .collect();
    assert_eq!(
        transfers.len(),
        3,
        "explicit admitted path must not emit learned CEP side copies"
    );
    for (port, priority) in [
        ("Net.a0.tx", "3"),
        ("Net.east.tx_pnp", "0"),
        ("Net.west.tx_a3", "3"),
    ] {
        assert!(
            transfers
                .iter()
                .any(|r| r["from_port"] == port && r["priority"] == priority)
        );
    }
    let source_bytes = transfers
        .iter()
        .find(|r| r["from_port"] == "Net.a0.tx")
        .unwrap()["wire"]["mac_bytes"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let provider_bytes = transfers
        .iter()
        .find(|r| r["from_port"] == "Net.east.tx_pnp")
        .unwrap()["wire"]["mac_bytes"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert_eq!(provider_bytes, source_bytes + 4, "actual S-tag growth");
    // MAC bound14+two VLAN tags8+maximum client60+FCS4=86B; one
    // full line reservation adds20B preamble/IPG at1Gb/s =848000ps.
    let slots = install["data"]["plan"]["ports"].as_array().unwrap();
    assert!(
        slots.iter().any(
            |p| p["slots"].as_array().unwrap().iter().any(|s| s["end_ps"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                - s["start_ps"].as_str().unwrap().parse::<u64>().unwrap()
                == 848_000)
        )
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        1
    );
}

#[test]
fn qcc_distributed_frer_reserves_prefix_once_and_each_branch_once_and_suffix_once() {
    let directory = root().join("examples/frer-rtag/distributed");
    let mut model: Value =
        serde_json::from_slice(&fs::read(directory.join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(directory.join("workload.json")).unwrap()).unwrap();
    let (qcc, _) = sample_values();
    model["schema_name"] = "dir.ethernet_network_composite.config".into();
    model["preemption"] = Value::Null;
    model["clocks"] = serde_json::json!({"nodes":[],"adjustments":[]});
    model["qcc"] = qcc["qcc"].clone();
    model["qcc"]["requirements"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let requirement = &mut model["qcc"]["requirements"][0];
    requirement["id"] = "unicast".into();
    requirement["generator"] = "unicast".into();
    requirement["max_frame_size"] = 60.into();
    requirement["num_seamless_trees"] = 2.into();
    requirement["listeners"] = serde_json::json!(["Net.d"]);
    requirement["paths"] = serde_json::json!([
        {"listener":"Net.d","ports":["Net.s.tx","Net.r.tx_a","Net.a.tx_e","Net.e.tx_d"]},
        {"listener":"Net.d","ports":["Net.s.tx","Net.r.tx_b","Net.b.tx_e","Net.e.tx_d"]}
    ]);
    model["qcc"]["operations"][0]["add"] = serde_json::json!(["unicast"]);
    workload["generators"][0]["times_ps"] = serde_json::json!(["200000000"]);
    let (_t, result) = custom_variant(
        &model,
        &workload,
        "ethdistributed.Net",
        &directory.join("models"),
        250_000_000,
    );
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let install = rows
        .iter()
        .find(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
        .expect("replicated request must admit");
    let reservations = install["data"]["plan"]["reservations"].as_array().unwrap();
    assert_eq!(reservations.len(), 6);
    assert!(reservations.iter().all(|r| r["frames"][0] == "1"));
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        1
    );
}

#[test]
fn qcc_existing_cbs_is_service_capacity_and_can_admit_without_tas_ownership_conflict() {
    let (mut model, workload) = sample_values();
    model["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["port"] == "Net.a.tx")
        .unwrap()["cbs"] = serde_json::json!([
        {"priority":5,"idle_slope_bps":"500000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}
    ]);
    let (_t, result) = variant(&model, &workload);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        4
    );
}

#[test]
fn qcc_joint_unequal_affine_shared_stations_use_one_nonintegral_calendar() {
    let directory = root().join("examples/network-composite/shared-lan");
    let mut model: Value =
        serde_json::from_slice(&fs::read(directory.join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(directory.join("workload.json")).unwrap()).unwrap();
    let (qcc, _) = sample_values();
    model["qcc"] = qcc["qcc"].clone();
    let cycle = 100_000_256u64;
    model["clocks"]["nodes"] = serde_json::json!([
        {"node":"Net.s1","offset_ps":"17","rate_num":"3","rate_den":"2"},
        {"node":"Net.s2","offset_ps":"-11","rate_num":"1","rate_den":"3"},
        {"node":"Net.s3","offset_ps":"23","rate_num":"2","rate_den":"5"}
    ]);
    model["qcc"]["cycle_ps"] = cycle.to_string().into();
    let mut first = model["qcc"]["requirements"][0].clone();
    first["id"] = "data".into();
    first["generator"] = "data".into();
    first["interval"] = serde_json::json!({"numerator":390626,"denominator":3906250000u64});
    let mut second = first.clone();
    second["id"] = "return".into();
    second["generator"] = "return".into();
    second["rank"] = 1.into();
    second["listeners"] = serde_json::json!(["Net.a"]);
    second["paths"] =
        serde_json::json!([{"listener":"Net.a","ports":["Net.c.tx","Net.s3.tx_b","Net.s1.tx_a"]}]);
    model["qcc"]["requirements"] = serde_json::json!([first, second]);
    model["qcc"]["operations"][0]["activate_at_ps"] = cycle.to_string().into();
    model["qcc"]["operations"][0]["add"] = serde_json::json!(["data", "return"]);
    workload["generators"][0]["times_ps"] = serde_json::json!([(2 * cycle).to_string()]);
    let mut reverse = workload["generators"][0].clone();
    reverse["id"] = "return".into();
    reverse["flow_id"] = "return".into();
    reverse["node"] = "Net.c".into();
    reverse["frame"]["dst_mac"] = "02:00:00:00:00:01".into();
    workload["generators"].as_array_mut().unwrap().push(reverse);
    let (_t, result) = custom_variant(
        &model,
        &workload,
        "ethshared.Net",
        &directory.join("models"),
        3 * cycle,
    );
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        2
    );
    let mut times: Vec<_> = rows
        .iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.segment"
                && (r["data"]["kind"] == "tx_started" || r["data"]["kind"] == "tx_released")
        })
        .map(|r| {
            (
                r["time_ps"].as_str().unwrap().parse::<u64>().unwrap(),
                r["data"]["kind"].as_str().unwrap(),
            )
        })
        .collect();
    times.sort();
    assert_eq!(times.len(), 4);
    assert_eq!(
        times.iter().map(|e| e.1).collect::<Vec<_>>(),
        ["tx_started", "tx_released", "tx_started", "tx_released"]
    );
    assert!(times[1].0 <= times[2].0);
}

fn merge_qcc_values() -> (Value, Value) {
    let directory = root().join("examples/mac-merge");
    let mut model: Value =
        serde_json::from_slice(&fs::read(directory.join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(directory.join("workload.json")).unwrap()).unwrap();
    // Let A's untagged LLDP bootstrap before Qcc opens managed class 0.
    // B is unmanaged; keep its ingress default 0 for the untagged slow stream.
    for port in model["base"]["config"]["base"]["config"]["ports"]
        .as_array_mut()
        .unwrap()
    {
        if port["port"] == "Net.a.tx" {
            port["default_priority"] = 7.into();
        }
    }
    let (qcc, _) = sample_values();
    model["qcc"] = qcc["qcc"].clone();
    model["qcc"]["cycle_ps"] = "10000000000".into(); //10ms, includes recurring LLDP
    model["qcc"]["requirements"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let requirement = &mut model["qcc"]["requirements"][0];
    requirement["id"] = "slow".into();
    requirement["generator"] = "slow".into();
    requirement["listeners"] = serde_json::json!(["Net.b"]);
    requirement["paths"] = serde_json::json!([{"listener":"Net.b","ports":["Net.a.tx"]}]);
    requirement["interval"] = serde_json::json!({"numerator":1,"denominator":100});
    requirement["max_frame_size"] = 1000.into();
    requirement["max_latency_ps"] = "9000000000".into();
    requirement["latest_transmit_offset_ps"] = "8000000000".into();
    model["qcc"]["operations"][0]["at_ps"] = "50000000".into();
    model["qcc"]["operations"][0]["expected_topology_epoch"] = "2".into();
    model["qcc"]["operations"][0]["activate_at_ps"] = "10000000000".into();
    model["qcc"]["operations"][0]["timeout_at_ps"] = "100000000".into();
    model["qcc"]["operations"][0]["add"] = serde_json::json!(["slow"]);
    model["mac_merge"]["capability_adapter"]["interval_ms"] = 1.into();
    workload["generators"].as_array_mut().unwrap().truncate(1);
    workload["generators"][0]["times_ps"] = serde_json::json!(["20000000000"]);
    workload["generators"][0]["frame"]["tag"] = Value::Null;
    // A 1KB pMAC exercises the reserved output. Fragment overhead remains
    // conservative when ordinary LLDP waits for its gate.
    workload["generators"][0]["frame"]["data"] = "a5".repeat(1000).into();
    (model, workload)
}
#[test]
fn qcc_bounded_merge_lldp_verify_respond_admits_real_loaded_output() {
    let (model, workload) = merge_qcc_values();
    let (_t, result) = custom_variant(
        &model,
        &workload,
        "ethtsn.Net",
        &root().join("examples/ethernet/tsn/models"),
        30_000_000_000,
    );
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
    );
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
        && r["data"]["kind"] == "source_sof"
        && r["data"]["within_certificate"] == true));
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == "delivery"
                && r["data"]["within_certificate"] == true)
            .count(),
        1
    );
    let install = rows
        .iter()
        .find(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
        .unwrap();
    let slot = &install["data"]["plan"]["ports"][0]["slots"][0];
    let start = slot["start_ps"].as_str().unwrap().parse::<u64>().unwrap();
    let end = slot["end_ps"].as_str().unwrap().parse::<u64>().unwrap();
    // Independent source arithmetic: D=11,952,000ps (1KB MAC plus worst
    // minimum-fragment overhead), residual B=16,736,000ps (2048B+framing),
    // L=4,V=15,R=17 packets at672,000ps each => x=52,880,000ps.
    assert_eq!(end - start, 52_880_000);
    let mut counted = std::collections::BTreeMap::new();
    for row in rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.mac_merge.packet" && r["data"]["port"] == "Net.a.tx")
    {
        let packet = &row["data"];
        let sof = packet["sof_ps"].as_str().unwrap().parse::<u64>().unwrap();
        if sof >= 20_000_000_000 + start
            && sof < 20_000_000_000 + end
            && packet["transfer"].is_null()
        {
            counted.insert(
                packet["packet_id"].as_str().unwrap().to_string(),
                packet["kind"].as_str().unwrap().to_string(),
            );
        }
    }
    for (kind, limit) in [("Express", 4), ("Verify", 15), ("Respond", 17)] {
        assert!(
            counted
                .values()
                .filter(|value| value.as_str() == kind)
                .count()
                <= limit
        );
    }
    let packets: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.mac_merge.packet")
        .collect();
    assert!(packets.iter().any(|r| r["data"]["kind"] == "Verify"));
    assert!(packets.iter().any(|r| r["data"]["kind"] == "Respond"));
    assert!(packets.iter().any(|r| r["data"]["kind"] == "Express"));
}

#[test]
fn qcc_merge_overloaded_burst_rejects_transaction_and_preserves_protocol_run() {
    let (mut model, workload) = merge_qcc_values();
    model["qcc"]["requirements"][0]["max_frames_per_interval"] = 4096.into();
    let (_t, result) = custom_variant(
        &model,
        &workload,
        "ethtsn.Net",
        &root().join("examples/ethernet/tsn/models"),
        30_000_000_000,
    );
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
        && r["data"]["kind"] == "rejected"
        && r["data"]["reason"] == "infeasible_frame_capacity"));
    assert!(
        !rows
            .iter()
            .any(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
    );
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "dir.mac_merge.packet" && r["data"]["kind"] == "Express")
    );
    assert_eq!(result["simulation"]["termination"], "time_limit");
}

#[test]
fn qcc_next_checkpoint_worker() {
    use dir_simulator::{
        CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
        restore_checkpoint,
    };
    let Ok(mode) = std::env::var("DIR_QCC_CHECKPOINT_MODE") else {
        return;
    };
    let directory = PathBuf::from(std::env::var_os("DIR_QCC_CHECKPOINT_ROOT").unwrap());
    if mode == "original" {
        let scenario = PathBuf::from(std::env::var_os("DIR_QCC_CHECKPOINT_SCENARIO").unwrap());
        let cut = std::env::var("DIR_QCC_CHECKPOINT_CUT")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let (mut session, controller) = RunSession::new(
            prepare(&scenario).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        while session.snapshot().common.last_event_time_ps.unwrap_or(0) < cut {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        assert_eq!(
            session
                .checkpoint(&directory.join("bundle"), CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        session.resume(session.pause_token().unwrap()).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            session.export(&directory.join("original")).unwrap().status,
            ControlStatus::Applied
        );
    } else if mode == "reject" {
        match restore_checkpoint(
            &directory.join("bundle"),
            dir_simulator::registry::Registry::default(),
            CheckpointLimits::default(),
        ) {
            Ok(_) => panic!("rehashed raw source mutant restored"),
            Err(error) => {
                let message = format!("{error:?}");
                println!("{message}");
                assert!(
                    message.contains("exact frozen-input committed prefix"),
                    "{message}"
                );
            }
        }
    } else {
        let (mut session, token) = restore_checkpoint(
            &directory.join("bundle"),
            dir_simulator::registry::Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        session.resume(token).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            session.export(&directory.join("restored")).unwrap().status,
            ControlStatus::Applied
        );
    }
}
fn qcc_checkpoint_roundtrip(scenario: &Path, directory: &Path, cut: u64) {
    for mode in ["original", "restored"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "qcc_next_checkpoint_worker", "--nocapture"])
            .env("DIR_QCC_CHECKPOINT_MODE", mode)
            .env("DIR_QCC_CHECKPOINT_ROOT", directory)
            .env("DIR_QCC_CHECKPOINT_SCENARIO", scenario)
            .env("DIR_QCC_CHECKPOINT_CUT", cut.to_string())
            .output()
            .unwrap();
        fs::write(directory.join(format!("{mode}.stdout.log")), &output.stdout).unwrap();
        fs::write(directory.join(format!("{mode}.stderr.log")), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{mode}:{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let canonical = |path: PathBuf| {
        let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("run_id");
        value.as_object_mut().unwrap().remove("control");
        let metadata = value["metadata"].as_object_mut().unwrap();
        metadata.remove("started_at_utc");
        metadata.remove("finished_at_utc");
        value
    };
    assert_eq!(
        canonical(directory.join("original/results.json")),
        canonical(directory.join("restored/results.json"))
    );
}
#[test]
fn qcc_exact_fractional_clock_certificates_resume_fresh_process_and_reject_pending_mutant() {
    let (mut model, workload) = sample_values();
    model["clocks"]["nodes"] =
        serde_json::json!([{"node":"Net.a","offset_ps":"0","rate_num":"3","rate_den":"2"}]);
    model["clocks"]["adjustments"] = serde_json::json!([
        {"node":"Net.a","at_ps":"50000001","kind":"slew","rate_num":"1","rate_den":"1"},
        {"node":"Net.a","at_ps":"250000001","kind":"step","offset_ps":"17"}]);
    model["qcc"]["operations"].as_array_mut().unwrap().extend([
        serde_json::json!({"operation":"prepare","id":"retry","at_ps":"60000000","expected_topology_epoch":"1","add":["high","low"],"remove":[],"activate_at_ps":"100000000","timeout_at_ps":"90000000"}),
        serde_json::json!({"operation":"prepare","id":"renew","at_ps":"260000000","expected_topology_epoch":"2","add":[],"remove":[],"activate_at_ps":"300000000","timeout_at_ps":"280000000"})]);
    let (t, _) = variant(&model, &workload);
    for cut in [61_000_000, 100_000_000, 250_000_001, 261_000_000] {
        let directory = t.0.join(format!("checkpoint-{cut}"));
        fs::create_dir_all(&directory).unwrap();
        qcc_checkpoint_roundtrip(&t.0.join("model.ini"), &directory, cut);
    }
    let bundle = t.0.join("checkpoint-61000000/bundle");
    let mut state: Value =
        serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
    let schedules = state["network_backend"]["state"]["qcc"]["pending"]["plan"]["schedules"]
        .as_array_mut()
        .unwrap();
    assert!(!schedules.is_empty());
    schedules.pop();
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(bundle.join("state.json"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    use sha2::{Digest, Sha256};
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "state.json")
        .unwrap();
    entry["bytes"] = bytes.len().to_string().into();
    entry["sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
    fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(
        dir_simulator::restore_checkpoint(
            &bundle,
            dir_simulator::registry::Registry::default(),
            dir_simulator::CheckpointLimits::default()
        )
        .is_err()
    );
}
#[test]
fn qcc_merge_pending_certificate_resumes_in_fresh_process() {
    let (model, workload) = merge_qcc_values();
    let (t, _) = custom_variant(
        &model,
        &workload,
        "ethtsn.Net",
        &root().join("examples/ethernet/tsn/models"),
        30_000_000_000,
    );
    let directory = t.0.join("merge-checkpoint");
    fs::create_dir_all(&directory).unwrap();
    qcc_checkpoint_roundtrip(&t.0.join("model.ini"), &directory, 80_000_000);
}

#[test]
#[ignore = "actual 6ms sampled startup; use release and copied executable"]
fn qcc_raw_certified_startup_installs_and_delivers_actual_sampled_stream() {
    let t = Temp::new();
    run(
        prepare(&root().join("examples/network-composite/qcc-raw/model.ini")).unwrap(),
        &t.0.join("raw"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(t.0.join("raw/results.json")).unwrap()).unwrap();
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let install = rows
        .iter()
        .find(|r| r["schema_name"] == "dir.network.qcc" && r["data"]["kind"] == "installed")
        .unwrap_or_else(|| {
            panic!(
                "Qcc effects: {:?}",
                rows.iter()
                    .filter(|r| r["schema_name"] == "dir.network.qcc")
                    .collect::<Vec<_>>()
            )
        });
    let certificate = &install["data"]["plan"]["raw_certificates"]["0"];
    assert_eq!(certificate["physical"]["bilateral_ready"], true);
    assert_eq!(certificate["physical"]["initial_idle_verified"], true);
    assert_eq!(certificate["merge"].as_array().unwrap().len(), 2);
    let grid = &certificate["physical"]["grid"];
    assert_eq!(grid["symbol_rate_hz"].as_str(), Some("750000000"));
    let ui = grid["samples_per_ui"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| grid["samples_per_ui"].as_u64())
        .unwrap();
    let integer = |v: &Value| {
        v.as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| v.as_u64())
            .unwrap()
    };
    for direction in certificate["physical"]["directions"].as_array().unwrap() {
        assert!(integer(&direction["pmax_samples"]) < ui);
        assert_eq!(
            integer(&direction["source_initial_idle_end_sample"])
                - integer(&direction["source_data_epoch_sample"]),
            2700 * ui
        );
        assert_eq!(
            integer(&direction["received_initial_idle_end_sample"])
                - integer(&direction["reconstructed_rx_epoch_sample"]),
            2700 * ui
        );
    }
    for kind in ["source_sof", "delivery"] {
        assert!(
            rows.iter().any(|r| r["schema_name"] == "dir.network.qcc"
                && r["data"]["kind"] == kind
                && r["data"]["within_certificate"] == true),
            "{kind}"
        );
    }
    let mut packets = std::collections::BTreeMap::new();
    for row in rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.mac_merge.packet")
    {
        let packet = &row["data"];
        if packet["arrival_ps"].is_string() {
            packets.insert(packet["packet_id"].as_str().unwrap().to_string(), packet);
        }
    }
    for port in ["Net.a.tx", "Net.b.tx"] {
        for kind in ["Express", "Verify", "Respond"] {
            assert!(
                packets.values().any(|p| p["port"] == port
                    && p["kind"] == kind
                    && p["timing_plane"] == "actual_xmii"
                    && !p["raw_phy"]["decoded_at_sample"].is_null()),
                "{port} {kind}"
            );
        }
    }
}

#[test]
#[ignore = "actual sampled startup and seven fresh-process runs; use copied release binary"]
fn qcc_raw_fresh_process_certificates_and_rehashed_source_mutants() {
    let t = Temp::new();
    let original = t.0.join("positive");
    fs::create_dir_all(&original).unwrap();
    qcc_checkpoint_roundtrip(
        &root().join("examples/network-composite/qcc-raw/model.ini"),
        &original,
        6_040_000_000,
    );
    let original_state: Value =
        serde_json::from_slice(&fs::read(original.join("bundle/state.json")).unwrap()).unwrap();
    assert!(!original_state["network_backend"]["state"]["qcc"]["pending"]["plan"]["raw_certificates"]["0"].is_null());
    fn mutate(value: &mut Value, kind: &str, count: &mut usize) {
        match value {
            Value::Object(map) => {
                if map.contains_key("medium_config_sha256")
                    && map.contains_key("physical_epoch_start_samples")
                {
                    *count += 1;
                    match kind {
                        "certificate" => {
                            map.insert("initial_idle_verified".into(), false.into());
                        }
                        "config" => {
                            map.insert("medium_config_sha256".into(), "0".repeat(64).into());
                        }
                        "epoch" => {
                            let v = &mut map.get_mut("physical_epoch_start_samples").unwrap()[0];
                            let n = v
                                .as_str()
                                .map(|s| s.parse::<u64>().unwrap())
                                .or_else(|| v.as_u64())
                                .unwrap();
                            *v = (n + 1).into();
                        }
                        "pmax" => {
                            let v = &mut map.get_mut("directions").unwrap()[0]["pmax_samples"];
                            let n = v
                                .as_str()
                                .map(|s| s.parse::<u64>().unwrap())
                                .or_else(|| v.as_u64())
                                .unwrap();
                            *v = (n + 1).into();
                        }
                        "identity" => {
                            map.get_mut("directions").unwrap()[0]["source_endpoint"] =
                                "forged-origin".into();
                        }
                        _ => unreachable!(),
                    }
                }
                for child in map.values_mut() {
                    mutate(child, kind, count);
                }
            }
            Value::Array(items) => {
                for child in items {
                    mutate(child, kind, count);
                }
            }
            _ => {}
        }
    }
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    for kind in ["certificate", "config", "epoch", "pmax", "identity"] {
        let directory = t.0.join(kind);
        copy_tree(&original.join("bundle"), &directory.join("bundle"));
        let mut state = original_state.clone();
        let mut count = 0;
        mutate(
            &mut state["network_backend"]["state"]["qcc"],
            kind,
            &mut count,
        );
        assert!(count >= 2, "mutate plan and source audit consistently");
        let bytes = serde_json::to_vec(&state).unwrap();
        fs::write(directory.join("bundle/state.json"), &bytes).unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(directory.join("bundle/manifest.json")).unwrap())
                .unwrap();
        use sha2::{Digest, Sha256};
        let file = manifest["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|f| f["path"] == "state.json")
            .unwrap();
        file["bytes"] = bytes.len().to_string().into();
        file["sha256"] = format!("{:x}", Sha256::digest(bytes)).into();
        fs::write(
            directory.join("bundle/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "qcc_next_checkpoint_worker", "--nocapture"])
            .env("DIR_QCC_CHECKPOINT_MODE", "reject")
            .env("DIR_QCC_CHECKPOINT_ROOT", &directory)
            .output()
            .unwrap();
        fs::write(directory.join("stdout.log"), &output.stdout).unwrap();
        fs::write(directory.join("stderr.log"), &output.stderr).unwrap();
        assert!(
            output.status.success(),
            "{kind}:{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn qcc_control_actual_starts_survive_multi_period_express_delay_and_response_carry_in() {
    let (mut model, mut workload) = merge_qcc_values();
    model["qcc"]["operations"] = serde_json::json!([]);
    for port in model["mac_merge"]["outputs"].as_array_mut().unwrap() {
        port["verify_time_ms"] = 1.into();
    }
    // Initial B LLDP is real and delivered; subsequent B transmissions keep
    // their actual serializer occupancy but are lost, forcing all three A V's.
    model["mac_merge"]["packet_faults"] = serde_json::json!((1..=120).map(|ordinal|
        serde_json::json!({"port":"Net.b.tx","packet_ordinal":ordinal,"action":{"kind":"Drop"}})).collect::<Vec<_>>());
    let mut flood = workload["generators"][0].clone();
    flood["id"] = "flooda".into();
    flood["flow_id"] = "flooda".into();
    flood["priority"] = 7.into();
    flood["frame"]["data"] = "a5".repeat(1500).into();
    flood["times_ps"] = serde_json::json!((0..16u64).map(|at| at.to_string()).collect::<Vec<_>>());
    let mut peer = flood.clone();
    peer["id"] = "floodb".into();
    peer["flow_id"] = "floodb".into();
    peer["node"] = "Net.b".into();
    peer["frame"]["dst_mac"] = "02:00:00:00:00:01".into();
    peer["times_ps"] = serde_json::json!(
        (0..16u64)
            .map(|at| (1_968_000_000 + at).to_string())
            .collect::<Vec<_>>()
    );
    workload["generators"] = serde_json::json!([workload["generators"][0].clone(), flood, peer]);
    let ned = Temp::new();
    fs::create_dir_all(ned.0.join("ethtsn")).unwrap();
    fs::write(
        ned.0.join("ethtsn/Main.ned"),
        fs::read_to_string(root().join("examples/ethernet/tsn/models/ethtsn/Main.ned"))
            .unwrap()
            .replace("1Gbps", "100Mbps"),
    )
    .unwrap();
    let (_t, result) = custom_variant(&model, &workload, "ethtsn.Net", &ned.0, 6_000_000_000);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let packets: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.mac_merge.packet")
        .map(|r| &r["data"])
        .collect();
    let time = |p: &&Value| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap();
    let mut verify: Vec<_> = packets
        .iter()
        .copied()
        .filter(|p| p["port"] == "Net.a.tx" && p["kind"] == "Verify")
        .collect();
    verify.sort_by_key(time);
    assert_eq!(verify.len(), 3);
    assert!(
        time(&verify[0]) > 1_000_000_000,
        "admission is delayed past one LLDP period"
    );
    let mut respond: Vec<_> = packets
        .iter()
        .copied()
        .filter(|p| p["port"] == "Net.b.tx" && p["kind"] == "Respond")
        .collect();
    respond.sort_by_key(time);
    assert!(respond.len() >= 2);
    assert_eq!(
        time(&respond[1]) - time(&respond[0]),
        6_720_000,
        "two coalesced response obligations drain consecutively"
    );
    assert!(time(&respond[0]) > time(&verify[1]));
    assert!(time(&respond[1]) < time(&verify[2]));
    // Independent sliding half-open interval oracle over actual starts. Timers
    // or nominal request timestamps are never used as packet arrivals.
    for port in ["Net.a.tx", "Net.b.tx"] {
        let controls: Vec<_> = packets
            .iter()
            .copied()
            .filter(|p| p["port"] == port && p["transfer"].is_null())
            .collect();
        let mut cuts: Vec<_> = controls.iter().map(time).collect();
        cuts.push(0);
        cuts.sort();
        cuts.dedup();
        for &a in &cuts {
            for &last in cuts.iter().filter(|last| **last >= a) {
                let b = last + 1;
                let t = b - a;
                let l = 3 + t.div_ceil(1_000_000_000);
                let v = 3 + 3 * l;
                let r = 2 + v;
                for (kind, upper) in [("Express", l), ("Verify", v), ("Respond", r)] {
                    let observed = controls
                        .iter()
                        .filter(|p| p["kind"] == kind && time(p) >= a && time(p) < b)
                        .count() as u64;
                    assert!(
                        observed <= upper,
                        "{port} {kind} [{a},{b}):{observed}>{upper}"
                    );
                }
            }
        }
    }
}
