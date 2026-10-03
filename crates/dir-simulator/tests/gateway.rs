use dir_simulator::{PreparedSimulation, input, runtime, snapshot::Snapshot};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dir-gateway-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy_tree(&path, &to.join(entry.file_name()));
        } else {
            fs::copy(path, to.join(entry.file_name())).unwrap();
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/gw")
        .join(name)
}
fn projection(p: &PreparedSimulation, s: &Snapshot, name: &str) -> Value {
    let native: Vec<_> = s
        .can
        .requests
        .iter()
        .filter(|r| {
            s.gateway.request_lineage[&r.request_id]
                .parent_request_id
                .is_none()
        })
        .collect();
    let copies: Vec<_> = s
        .can
        .requests
        .iter()
        .filter(|r| {
            s.gateway.request_lineage[&r.request_id]
                .parent_request_id
                .is_some()
        })
        .collect();
    let forwards = &s.gateway.forwards;
    let mut value = json!({"sof_ps":s.can.requests.iter().map(|r| (r.request_id.clone(),json!(r.sof_ps))).collect::<serde_json::Map<_,_>>(),
        "eof_ps":s.can.requests.iter().map(|r| (r.request_id.clone(),json!(r.eof_ps))).collect::<serde_json::Map<_,_>>(),
        "forward_count":forwards.len(), "child_request_count":copies.len(), "request_count":s.can.requests.len(), "prepare_valid":true});
    match name {
        "delay" => {
            let ingress = s
                .can
                .receivers
                .iter()
                .find(|r| r.request_id == "source:0" && r.receiver == "Main.gw.a")
                .unwrap();
            let sink = s
                .can
                .receivers
                .iter()
                .find(|r| r.receiver == "Main.sink" && r.request_id.starts_with("gw:"))
                .unwrap();
            value["ingress_observed_ps"] = json!(ingress.observed_ps);
            value["ingress_received_ps"] = json!(ingress.received_ps);
            value["copy_generated_ps"] = json!(copies[0].generated_ps);
            value["copy_ready_ps"] = json!(copies[0].ready_ps);
            value["sink_observed_ps"] = json!(sink.observed_ps);
            value["sink_received_ps"] = json!(sink.received_ps);
            value["path_delay_ps"] = json!(
                sink.received_ps.unwrap()
                    - native
                        .iter()
                        .find(|r| r.request_id == "source:0")
                        .unwrap()
                        .generated_ps
            );
        }
        "queue" => {
            value["source_eof_ps"] = json!(native.iter().map(|r| r.eof_ps).collect::<Vec<_>>());
            value["copy_sof_ps"] = json!(copies.iter().map(|r| r.sof_ps).collect::<Vec<_>>());
            value["copy_eof_ps"] = json!(copies.iter().map(|r| r.eof_ps).collect::<Vec<_>>());
            value["copy_status"] = json!(copies.iter().map(|r| &r.status).collect::<Vec<_>>());
            value["copy_drop_reason"] = json!(
                copies
                    .iter()
                    .map(|r| if r.status == "dropped" {
                        Some("queue_full")
                    } else {
                        None
                    })
                    .collect::<Vec<_>>()
            );
        }
        "capacity-zero" | "eof-boundary" => {
            value["copy_status"] = json!(copies[0].status);
            value["copy_drop_reason"] = json!(if copies[0].status == "dropped" {
                Some("queue_full")
            } else {
                None
            });
            value["attempts"] = json!(usize::from(copies[0].sof_ps.is_some()));
            value["copy_eof_ps"] = json!(copies[0].eof_ps);
            value["copy_planned_eof_ps"] = json!(copies[0].planned_eof_ps);
        }
        "forward-boundary" | "no-route" => {
            value["forward_status"] = json!(forwards[0].status);
            value["reason"] = json!(forwards[0].reason);
            value["planned_forward_ps"] = json!(forwards[0].planned_forward_ps);
        }
        "rx-filter" => {
            value["receiver_status"] = json!(
                s.can
                    .receivers
                    .iter()
                    .find(|r| r.receiver == "Main.gw.a")
                    .unwrap()
                    .status
            );
        }
        "multicast" | "multicast-drop" => {
            value["child_eof_ps"] = json!(
                copies
                    .iter()
                    .map(|r| (r.source.clone(), json!(r.eof_ps)))
                    .collect::<serde_json::Map<_, _>>()
            );
            value["copy_status"] = json!(
                copies
                    .iter()
                    .map(|r| (r.source.clone(), json!(r.status)))
                    .collect::<serde_json::Map<_, _>>()
            );
            value["origin"] =
                json!(s.gateway.request_lineage[&copies[0].request_id].origin_request_id);
            value["copy_count"] = json!(copies.len());
            value["reason_c"] = json!("queue_full");
        }
        "hop" => {
            value["hops"] = json!(forwards.iter().map(|f| f.gw_hops).collect::<Vec<_>>());
            value["forwarded_ps"] =
                json!(forwards.iter().map(|f| f.forwarded_ps).collect::<Vec<_>>());
            value["statuses"] = json!(forwards.iter().map(|f| &f.status).collect::<Vec<_>>());
            value["reason"] = json!(forwards.last().unwrap().reason);
        }
        _ => {}
    }
    for r in &s.can.receivers {
        let request = s
            .can
            .requests
            .iter()
            .find(|q| q.request_id == r.request_id)
            .unwrap();
        let receiver = p
            .can
            .controllers
            .iter()
            .position(|c| c.id == r.receiver)
            .unwrap();
        let source = p
            .can
            .controllers
            .iter()
            .position(|c| c.id == request.source)
            .unwrap();
        assert_eq!(
            p.can.controller_buses[receiver], p.can.controller_buses[source],
            "no cross-bus broadcast"
        );
    }
    value
}

#[test]
fn all_gateway_fixtures_match_independent_expectations() {
    let scenarios: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("scenarios.json")).unwrap()).unwrap();
    for case in scenarios["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let prepared = input::prepare(&fixture(case["config"].as_str().unwrap()));
        if let Some(reason) = case["expected"]["prepare_failure"].as_str() {
            let error = prepared.unwrap_err();
            assert!(error.message.contains(reason), "{name}: {error}");
            continue;
        }
        let prepared = prepared.unwrap_or_else(|e| panic!("{name}: {e}"));
        let snapshot = runtime::simulate(&prepared).unwrap();
        assert!(
            !snapshot.common.partial,
            "{name}: {:?}",
            snapshot.common.diagnostics
        );
        let actual = projection(&prepared, &snapshot, name);
        for (key, expected) in case["expected"].as_object().unwrap() {
            assert_eq!(&actual[key], expected, "{name}: {key}");
        }
        assert_eq!(
            actual,
            projection(&prepared, &runtime::simulate(&prepared).unwrap(), name),
            "{name}: deterministic repetition"
        );
        for row in &snapshot.gateway.forwards {
            if let Some(id) = &row.child_request_id {
                let copy = snapshot
                    .can
                    .requests
                    .iter()
                    .find(|r| &r.request_id == id)
                    .unwrap();
                let parent = &snapshot.can.requests[row.parent_request];
                assert_eq!(
                    snapshot.gateway.request_lineage[&copy.request_id].origin_request_id,
                    snapshot.gateway.request_lineage[&parent.request_id].origin_request_id
                );
                assert_eq!(
                    snapshot.gateway.request_lineage[&copy.request_id]
                        .parent_request_id
                        .as_deref(),
                    Some(parent.request_id.as_str())
                );
                assert_eq!(
                    (
                        copy.crc15,
                        copy.stuff_bits,
                        copy.frame_bits,
                        copy.payload_bits
                    ),
                    (
                        parent.crc15,
                        parent.stuff_bits,
                        parent.frame_bits,
                        parent.payload_bits
                    )
                );
            }
        }
        assert_eq!(
            snapshot
                .gateway
                .forwards
                .iter()
                .filter(|f| f.egress.is_some())
                .count(),
            snapshot
                .gateway
                .forwards
                .iter()
                .filter(|f| ["processing", "submitted", "dropped"].contains(&f.status.as_str()))
                .count()
        );
    }
}

#[test]
fn schema2_publication_preserves_references_hashes_and_per_bus_metrics() {
    use sha2::{Digest, Sha256};
    let temp = Temp::new();
    let scenarios: Value =
        serde_json::from_str(&fs::read_to_string(fixture("scenarios.json")).unwrap()).unwrap();
    for case in scenarios["cases"].as_array().unwrap() {
        if case["expected"].get("prepare_failure").is_some() {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let p = input::prepare(&fixture(case["config"].as_str().unwrap())).unwrap();
        let directory = temp.0.join(name);
        dir_simulator::run(p, &directory).unwrap();
        let result: Value =
            serde_json::from_slice(&fs::read(directory.join("results.json")).unwrap()).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(result["schema_version"], 2);
        assert_eq!(manifest["schema_version"], 2);
        assert!(result["simulation"].get("requests").is_none());
        assert!(result["simulation"].get("receivers").is_none());
        assert_eq!(result["metadata"]["model_profile"], "can.cc.multibus.v1");
        assert_eq!(result["metadata"]["metrics"].as_array().unwrap().len(), 47);
        assert_eq!(
            result["metadata"]["model_schemas"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert!(
            result["metadata"]["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["logical_path"] == "model-config")
        );
        for row in manifest["files"].as_array().unwrap() {
            let bytes = fs::read(directory.join(row["name"].as_str().unwrap())).unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(&bytes)), row["sha256"]);
            assert_eq!(bytes.len().to_string(), row["bytes"]);
        }
        for file in ["events.csv", "summary.csv"] {
            let csv = fs::read_to_string(directory.join(file)).unwrap();
            assert!(csv.lines().skip(1).all(|row| row.starts_with("2,")));
        }
        let rows = result["simulation"]["model_records"].as_array().unwrap();
        let forwards: Vec<_> = rows
            .iter()
            .filter(|r| r["schema_name"] == "gw.forward")
            .map(|r| &r["data"])
            .collect();
        let records = result["simulation"]["records"].as_array().unwrap();
        for state in result["metadata"]["initial_state"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["instance"].as_str().unwrap().starts_with("@profile:"))
        {
            let value: Value = serde_json::from_str(state["state"].as_str().unwrap()).unwrap();
            assert_eq!(
                value,
                json!({"profile":"can.cc.multibus.v1","forward_records":[],"pending":[],"rx_buffers":[]})
            );
        }
        for forward in &forwards {
            let expected = if forward["status"] == "filtered" {
                vec![(
                    "gw_route_filtered",
                    &forward["received_ps"],
                    &forward["ingress"],
                    json!("no_route"),
                )]
            } else {
                let mut points = vec![(
                    "gw_copy_created",
                    &forward["received_ps"],
                    &forward["egress"],
                    Value::Null,
                )];
                if forward["status"] == "submitted" {
                    points.push((
                        "gw_copy_submitted",
                        &forward["forwarded_ps"],
                        &forward["egress"],
                        Value::Null,
                    ));
                }
                if forward["status"] == "dropped" {
                    points.push((
                        "gw_hop_dropped",
                        &forward["forwarded_ps"],
                        &forward["egress"],
                        json!("dropped_hop_limit"),
                    ));
                }
                points
            };
            for (metric, time, receiver, reason) in expected {
                let matching: Vec<_> = records
                    .iter()
                    .filter(|r| {
                        r["metric"] == metric
                            && r["target"] == forward["gateway"]
                            && r["request_id"] == forward["parent_request_id"]
                            && r["receiver"] == *receiver
                    })
                    .collect();
                assert_eq!(matching.len(), 1, "{name}: {metric}");
                assert_eq!(matching[0]["value"], "1");
                assert_eq!(matching[0]["time_ps"], *time);
                assert_eq!(matching[0]["reason"], reason);
            }
        }
        for gw in result["metadata"]["config"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["key"].as_str().unwrap().starts_with("@profile:"))
        {
            let config: Value = serde_json::from_str(gw["value"].as_str().unwrap()).unwrap();
            let count = |metric: &str| {
                records
                    .iter()
                    .filter(|r| r["target"] == config["node"] && r["metric"] == metric)
                    .count()
            };
            let pending = result["simulation"]["summary"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["target"] == config["node"] && r["metric"] == "gw_processing_pending")
                .unwrap();
            let pending_count: usize = pending["value"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                count("gw_copy_created"),
                count("gw_copy_submitted") + count("gw_hop_dropped") + pending_count
            );
            assert!(
                pending["request_id"].is_null()
                    && pending["receiver"].is_null()
                    && pending["reason"].is_null()
            );
        }
        if name == "multicast-drop" {
            let copy = rows
                .iter()
                .find(|r| r["schema_name"] == "can.request" && r["data"]["source"] == "Multi.gw.c")
                .unwrap();
            assert_eq!(copy["data"]["drop_reason"], "queue_full");
        }
        let keys: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r["schema_name"].as_str(),
                    r["subject"].as_str(),
                    r["record_id"].as_str(),
                )
            })
            .collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        for row in rows.iter().filter(|r| r["schema_name"] == "can.request") {
            let data = &row["data"];
            assert_eq!(row["request_id"], data["request_id"]);
            assert_eq!(
                row["origin_request_id"],
                data["model_fields"]["origin_request_id"]
            );
            assert!(
                rows.iter()
                    .any(|origin| origin["schema_name"] == "can.request"
                        && origin["record_id"] == row["origin_request_id"])
            );
        }
        if name == "independent-only" {
            let summary = result["simulation"]["summary"].as_array().unwrap();
            let metric = |target: &str, key: &str| {
                &summary
                    .iter()
                    .find(|r| r["target"] == target && r["metric"] == key)
                    .unwrap()["value"]
            };
            assert_eq!(metric("Main.busA", "bus_utilization"), 0.106);
            assert_eq!(metric("Main.busB", "bus_utilization"), 0.1);
            assert_eq!(metric("Main.busA", "received"), "1");
            assert_eq!(metric("Main.busB", "received"), "1");
            assert_eq!(metric("Main.busA", "transfer_mean_ps"), 100000000.0);
            assert_eq!(metric("Main.busB", "transfer_mean_ps"), 94000000.0);
        }
        let viewer = temp.0.join(format!("{name}.html"));
        dir_simulator::viewer::write(&directory.join("results.json"), &viewer).unwrap();
        assert!(fs::read_to_string(viewer).unwrap().contains("Gateway転送"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(["validate", "--config"])
        .arg(fixture("delay.ini"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let validation: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(validation["node_count"], "7"); // four Controllers, two buses, one structural Gateway
}

#[test]
fn arbitrary_bus_gate_names_preserve_multibus_results_and_timing() {
    let temp = Temp::new();
    let directory = temp.0.join("input");
    copy_tree(&fixture(""), &directory);
    // Start with the legacy Bus convention; structural Gateway boundary gates stay unchanged.
    let rename = |from: [(&str, &str); 4]| {
        let rename_bus_references = |mut text: String| {
            for bus in ["busA", "busB", "busC", "busD"] {
                for (old, new) in from {
                    text = text.replace(&format!("{bus}.{old}"), &format!("{bus}.{new}"));
                }
            }
            text
        };
        for entry in fs::read_dir(directory.join("models/gw")).unwrap() {
            let path = entry.unwrap().path();
            let mut text = rename_bus_references(fs::read_to_string(&path).unwrap());
            if let Some((before, rest)) = text.split_once("simple Bus {") {
                let (body, after) = rest.split_once('}').unwrap();
                let mut body = body.to_owned();
                for (old, new) in from {
                    body = body.replace(old, new);
                }
                text = format!("{before}simple Bus {{{body}}}{after}");
            }
            fs::write(path, text).unwrap();
        }
        let path = directory.join("delay.ini");
        let text = rename_bus_references(fs::read_to_string(&path).unwrap());
        fs::write(path, text).unwrap();
    };
    rename([
        ("rx_a", "tx_legacyA"),
        ("tx_a", "rx_legacyA"),
        ("rx_b", "tx_legacyB"),
        ("tx_b", "rx_legacyB"),
    ]);
    let run = |name: &str| {
        let prepared = input::prepare(&directory.join("delay.ini")).unwrap();
        let snapshot = runtime::simulate(&prepared).unwrap();
        let timing = projection(&prepared, &snapshot, "delay");
        assert!(!snapshot.common.partial);
        let controllers = serde_json::to_value(&prepared.can.controllers).unwrap();
        let buses = prepared.can.controller_buses.clone();
        let output = temp.0.join(name);
        dir_simulator::run(prepared, &output).unwrap();
        let result: Value =
            serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap();
        let csv = ["events.csv", "summary.csv"].map(|file| {
            fs::read_to_string(output.join(file))
                .unwrap()
                .replace(result["run_id"].as_str().unwrap(), "RUN")
        });
        (controllers, buses, timing, result, csv)
    };
    let baseline = run("legacy");
    // Inputs deliberately start with rx_, outputs with tx_; none share pair suffixes.
    rename([
        ("tx_legacyA", "rx_incoming"),
        ("rx_legacyA", "tx_deliver"),
        ("tx_legacyB", "_request"),
        ("rx_legacyB", "result"),
    ]);
    let renamed = run("arbitrary");
    assert_eq!(baseline.0, renamed.0);
    assert_eq!(baseline.1, renamed.1);
    assert_eq!(baseline.2, renamed.2);
    assert_eq!(baseline.3["simulation"], renamed.3["simulation"]);
    assert_eq!(baseline.4, renamed.4);
    assert_ne!(
        baseline.3["metadata"]["input_sha256"],
        renamed.3["metadata"]["input_sha256"]
    );
    assert_ne!(
        baseline.3["metadata"]["sources"],
        renamed.3["metadata"]["sources"]
    );
}

#[test]
fn wrong_bus_rx_and_bus_to_bus_paths_are_rejected() {
    let temp = Temp::new();
    let directory = temp.0.join("input");
    copy_tree(&fixture(""), &directory);
    let path = directory.join("models/gw/Main.ned");
    let original = fs::read_to_string(&path).unwrap();
    let wrong_bus = original
        .replace("--> src.rx;", "--> swap.rx;")
        .replace("--> sink.rx;", "--> src.rx;")
        .replace("--> swap.rx;", "--> sink.rx;");
    fs::write(&path, wrong_bus).unwrap();
    let error = input::prepare(&directory.join("delay.ini")).unwrap_err();
    assert!(
        error.message.contains("tx/rx must use the same Bus"),
        "{error}"
    );

    let bus_to_bus = original
        .replace(
            "busA.tx_a --> gw.Wire --> src.rx;",
            "busA.tx_a --> gw.Wire --> busB.rx_b;",
        )
        .replace(
            "sink.tx --> gw.Wire --> busB.rx_b;",
            "sink.tx --> gw.Wire --> src.rx;",
        );
    fs::write(&path, bus_to_bus).unwrap();
    let error = input::prepare(&directory.join("delay.ini")).unwrap_err();
    assert!(
        error.message.contains("incompatible CAN payload path"),
        "{error}"
    );
    assert!(
        error.message.contains("busA.tx_a --> Main.busB.rx_b"),
        "{error}"
    );
}

#[test]
fn input_roles_and_gateway_initial_state_survive_arbitrary_extensions_and_bom() {
    let temp = Temp::new();
    let inputs = temp.0.join("inputs");
    copy_tree(&fixture(""), &inputs);
    let ini = fs::read_to_string(inputs.join("delay.ini"))
        .unwrap()
        .replace("delay-routing.json", "routing.dat")
        .replace("delay-workload.json", "traffic.dat");
    fs::write(inputs.join("scenario.dat"), ini).unwrap();
    let content = format!(
        "\u{feff}{}",
        fs::read_to_string(inputs.join("delay-routing.json")).unwrap()
    );
    fs::write(inputs.join("routing.dat"), &content).unwrap();
    fs::copy(
        inputs.join("delay-workload.json"),
        inputs.join("traffic.dat"),
    )
    .unwrap();
    let prepared = input::prepare(&inputs.join("scenario.dat")).unwrap();
    let output = temp.0.join("output");
    dir_simulator::run(prepared, &output).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap();
    let sources = result["metadata"]["sources"].as_array().unwrap();
    for (role, file) in [
        ("config", "scenario.dat"),
        ("model-config", "routing.dat"),
        ("workload", "traffic.dat"),
    ] {
        let source = sources.iter().find(|s| s["logical_path"] == role).unwrap();
        assert_eq!(
            source["canonical_path"],
            inputs.join(file).to_str().unwrap()
        );
    }
    let source = sources
        .iter()
        .find(|s| s["logical_path"] == "model-config")
        .unwrap();
    assert_eq!(source["content_utf8"], content);
    let config = result["metadata"]["config"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "model-config")
        .unwrap();
    let path: Value = serde_json::from_str(config["value"].as_str().unwrap()).unwrap();
    assert_eq!(path, source["canonical_path"]);
}

#[test]
fn inactive_reservations_and_mutated_routing_are_validated_before_execution() {
    let temp = Temp::new();
    copy_tree(&fixture(""), &temp.0.join("input"));
    let directory = temp.0.join("input");
    let original: Value =
        serde_json::from_str(&fs::read_to_string(directory.join("delay-routing.json")).unwrap())
            .unwrap();
    for (field, value) in [
        ("hop_limit", json!(0)),
        ("hop_limit", json!(65536)),
        ("hop_limit", json!(true)),
        ("hop_limit", json!(1.0)),
        ("processing_delay", json!("-1ps")),
        ("unknown", json!(1)),
    ] {
        let mut routing = original.clone();
        routing["gateways"][0][field] = value;
        fs::write(directory.join("delay-routing.json"), routing.to_string()).unwrap();
        assert!(
            input::prepare(&directory.join("delay.ini")).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("id_min", json!(-1)),
        ("id_max", json!(2048)),
        ("id_max", json!(true)),
        ("id_min", json!(1.0)),
        ("format", json!("unknown")),
        ("egress", json!([])),
        ("egress", json!(["Main.gw.a"])),
        ("egress", json!(["Main.gw.b", "Main.gw.b"])),
        ("unknown", json!(1)),
    ] {
        let mut routing = original.clone();
        routing["gateways"][0]["routes"][0][field] = value;
        fs::write(directory.join("delay-routing.json"), routing.to_string()).unwrap();
        assert!(
            input::prepare(&directory.join("delay.ini")).is_err(),
            "route {field}"
        );
    }
    fs::write(
        directory.join("delay-routing.json"),
        original.to_string().replacen(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
            1,
        ),
    )
    .unwrap();
    assert!(
        input::prepare(&directory.join("delay.ini"))
            .unwrap_err()
            .message
            .contains("duplicate")
    );
    fs::write(directory.join("delay-routing.json"), original.to_string()).unwrap();
    let workload_path = directory.join("delay-workload.json");
    let original_workload: Value =
        serde_json::from_str(&fs::read_to_string(&workload_path).unwrap()).unwrap();
    let mut workload = original_workload.clone();
    workload["generators"][0]["node"] = json!("Main.gw.a");
    fs::write(&workload_path, workload.to_string()).unwrap();
    assert!(input::prepare(&directory.join("delay.ini")).is_err());
    let mut workload = original_workload;
    workload["generators"][1]["frame"]["id"] = json!(0);
    workload["generators"][1]["times"] = json!([]);
    fs::write(&workload_path, workload.to_string()).unwrap();
    assert!(
        input::prepare(&directory.join("delay.ini"))
            .unwrap_err()
            .message
            .contains("owner_overlap")
    );
}

#[test]
fn same_priority_copies_follow_ready_commit_order_and_configuration_permutations() {
    let temp = Temp::new();
    copy_tree(&fixture(""), &temp.0.join("input"));
    let directory = temp.0.join("input");
    let config = "[General]\nnetwork = gw.Multi\nned-path = \"models\"\nsim-time-limit = 2ms\nmodel-profile = \"can.cc.multibus.v1\"\nmodel-config = \"routing.json\"\nworkload = \"workload.json\"\nMulti.busB.bitrate = 125kbps\n";
    fs::write(directory.join("test.ini"), config).unwrap();
    let mut routing = json!({"schema_version":1,"gateways":[{"node":"Multi.gw","ports":["Multi.gw.a","Multi.gw.b","Multi.gw.c"],"routes":[
        {"id":"ab","ingress":"Multi.gw.a","egress":["Multi.gw.b"],"format":"standard","id_min":0,"id_max":0},
        {"id":"cb","ingress":"Multi.gw.c","egress":["Multi.gw.b"],"format":"standard","id_min":0,"id_max":0}]}]});
    let mut workload = json!({"schema_version":2,"generators":[
        {"id":"z","kind":"can.explicit.v1","node":"Multi.src","times":["0ps"],"frame":{"format":"standard","id":0,"data":""}},
        {"id":"a","kind":"can.explicit.v1","node":"Multi.sinkC","times":["0ps"],"frame":{"format":"standard","id":0,"data":""}},
        {"id":"block","kind":"can.explicit.v1","node":"Multi.sinkB","times":["0ps"],"frame":{"format":"standard","id":1,"data":""}}]});
    let run = |routing: &Value, workload: &Value| {
        fs::write(directory.join("routing.json"), routing.to_string()).unwrap();
        fs::write(directory.join("workload.json"), workload.to_string()).unwrap();
        let p = input::prepare(&directory.join("test.ini")).unwrap();
        let s = runtime::simulate(&p).unwrap();
        assert!(!s.common.partial);
        let mut copies: Vec<_> = s
            .can
            .requests
            .iter()
            .filter(|r| {
                s.gateway.request_lineage[&r.request_id]
                    .parent_request_id
                    .is_some()
            })
            .collect();
        copies.sort_by_key(|r| r.sof_ps);
        copies
            .iter()
            .map(|r| {
                (
                    s.gateway.request_lineage[&r.request_id]
                        .origin_request_id
                        .clone(),
                    r.sof_ps,
                    r.eof_ps,
                )
            })
            .collect::<Vec<_>>()
    };
    let original = run(&routing, &workload);
    assert_eq!(original[0].0, "z:0");
    assert_eq!(original[1].0, "a:0");
    assert_eq!(original[0].1, Some(400000000));
    assert_eq!(original[1].1, Some(824000000));
    routing["gateways"][0]["ports"]
        .as_array_mut()
        .unwrap()
        .reverse();
    routing["gateways"][0]["routes"]
        .as_array_mut()
        .unwrap()
        .reverse();
    workload["generators"].as_array_mut().unwrap().reverse();
    assert_eq!(run(&routing, &workload), original);
}

#[test]
fn gateway_failures_keep_the_last_committed_prefix_without_partial_fanout() {
    let mut p = input::prepare(&fixture("multicast.ini")).unwrap();
    p.gateway.gateways[0].processing_delay_ps = u64::MAX;
    let s = runtime::simulate(&p).unwrap();
    assert!(s.common.partial);
    assert!(s.gateway.forwards.is_empty());
    assert_eq!(s.can.requests.len(), 1);
    assert_eq!(s.can.requests[0].status, "success");
    assert_eq!(s.can.receivers[0].status, "received");
    assert!(
        s.common
            .points
            .iter()
            .all(|point| point.event_seq.is_none() || !point.metric.starts_with("gw_"))
    );
    assert!(s.gateway.rx_buffers.is_empty());
    let mut p = input::prepare(&fixture("delay.ini")).unwrap();
    let complete = runtime::simulate(&p).unwrap();
    for limit in 1..complete.common.committed_events {
        p.common.max_events = limit;
        let partial = runtime::simulate(&p).unwrap();
        assert!(partial.common.partial);
        assert_eq!(partial.common.committed_events, limit);
        for row in &partial.gateway.forwards {
            assert!(row.parent_request < partial.can.requests.len());
            if let Some(child) = &row.child_request_id {
                assert!(partial.can.requests.iter().any(|r| &r.request_id == child));
            }
        }
    }
}

#[test]
fn gateway_rx_holds_full_tx_copy_until_sof_frees_a_slot() {
    let p = input::prepare(&fixture("queue.ini")).unwrap();
    assert_eq!(p.gateway.gateways[0].rx_queue_capacity, 64);
    let s = runtime::simulate(&p).unwrap();
    let copies: Vec<_> = s
        .can
        .requests
        .iter()
        .filter(|r| r.request_id.starts_with("gw:"))
        .collect();
    assert_eq!(copies.len(), 3);
    assert_eq!(
        copies.iter().map(|r| r.ready_ps).collect::<Vec<_>>(),
        vec![Some(100_000_000), Some(210_000_000), Some(320_000_000)]
    );
    assert_eq!(
        copies.iter().map(|r| r.tx_enqueued_ps).collect::<Vec<_>>(),
        vec![Some(100_000_000), Some(210_000_000), Some(524_000_000)]
    );
    assert_eq!(
        copies.iter().map(|r| r.sof_ps).collect::<Vec<_>>(),
        vec![Some(100_000_000), Some(524_000_000), Some(948_000_000)]
    );
    assert_eq!(copies[2].status, "in_flight");
    assert_eq!(copies[2].eof_ps, None);
    let held = s
        .gateway
        .rx_buffers
        .iter()
        .find(|b| s.can.requests[b.parent_request].request_id == "source:2")
        .unwrap();
    assert_eq!(held.received_ps, 320_000_000);
    assert_eq!(held.released_ps, Some(524_000_000));
    assert!(held.remaining_egress.is_empty());
    assert!(
        s.common
            .points
            .iter()
            .any(|p| p.metric == "gw_tx_buffer_wait_ps" && p.value == 204_000_000)
    );
    // A stop at the exact SOF leaves C held; events at T must not drain RX.
    let mut at_boundary = p.clone();
    at_boundary.common.time_limit_ps = 524_000_000;
    let stopped = runtime::simulate(&at_boundary).unwrap();
    let c = stopped
        .can
        .requests
        .iter()
        .find(|r| r.request_id.starts_with("gw:source:2/"))
        .unwrap();
    assert_eq!(c.status, "waiting_tx");
    assert_eq!(c.tx_enqueued_ps, None);
    assert_eq!(
        stopped
            .gateway
            .rx_buffers
            .iter()
            .filter(|b| b.status == "holding")
            .count(),
        1
    );
}

#[test]
fn gateway_rx_overflow_drops_newest_without_changing_source_success() {
    let mut p = input::prepare(&fixture("queue.ini")).unwrap();
    p.gateway.gateways[0].rx_queue_capacity = 1;
    p.can.generators[0].schedule =
        dir_simulator::types::Schedule::Explicit(vec![0, 110_000_000, 220_000_000, 330_000_000]);
    let s = runtime::simulate(&p).unwrap();
    let dropped = s
        .gateway
        .rx_buffers
        .iter()
        .find(|b| b.status == "dropped")
        .unwrap();
    assert_eq!(
        s.can.requests[dropped.parent_request].request_id,
        "source:3"
    );
    assert_eq!(dropped.reason.as_deref(), Some("rx_queue_full"));
    assert_eq!(dropped.received_ps, 430_000_000);
    assert_eq!(dropped.released_ps, None);
    assert!(
        s.can
            .requests
            .iter()
            .filter(|r| r.request_id.starts_with("source:"))
            .all(|r| r.status == "success")
    );
    assert!(s.can.receivers.iter().any(|r| r.request_id == "source:3"
        && r.receiver == "Main.gw.a"
        && r.status == "received"));
    assert!(
        !s.gateway
            .forwards
            .iter()
            .any(|f| f.parent_request == dropped.parent_request)
    );
    assert_eq!(
        s.common
            .points
            .iter()
            .filter(|p| p.metric == "gw_rx_dropped")
            .count(),
        1
    );
    assert!(
        s.common
            .points
            .iter()
            .filter(|p| p.metric == "gw_rx_queue_length")
            .all(|p| p.value <= 1)
    );
}

#[test]
fn gateway_rx_capacity_counts_both_gateway_and_tx_processing() {
    for delay_in_gateway in [true, false] {
        let mut p = input::prepare(&fixture("queue.ini")).unwrap();
        p.gateway.gateways[0].rx_queue_capacity = 1;
        p.common.time_limit_ps = 550_000_000;
        if delay_in_gateway {
            p.gateway.gateways[0].processing_delay_ps = 500_000_000;
        } else {
            let output = p
                .can
                .controllers
                .iter()
                .position(|c| c.id == "Main.gw.b")
                .unwrap();
            p.can.controllers[output].tx_processing_ps = 500_000_000;
        }
        let s = runtime::simulate(&p).unwrap();
        assert_eq!(
            s.gateway
                .rx_buffers
                .iter()
                .filter(|b| b.status == "holding")
                .count(),
            1
        );
        assert_eq!(
            s.gateway
                .rx_buffers
                .iter()
                .filter(|b| b.status == "dropped")
                .count(),
            2
        );
        let copies: Vec<_> = s
            .can
            .requests
            .iter()
            .filter(|r| r.request_id.starts_with("gw:"))
            .collect();
        if delay_in_gateway {
            assert!(copies.is_empty());
        } else {
            assert_eq!(copies.len(), 1);
            assert_eq!(copies[0].status, "processing");
            assert_eq!(copies[0].ready_ps, None);
        }
    }
}

#[test]
fn gateway_rx_fanout_keeps_only_unadmitted_egress_and_does_not_duplicate_copies() {
    let mut p = input::prepare(&fixture("multicast.ini")).unwrap();
    p.gateway.gateways[0].rx_queue_capacity = 1;
    p.common.time_limit_ps = 500_000_000;
    p.can.generators[0].schedule =
        dir_simulator::types::Schedule::Explicit(vec![0, 110_000_000, 220_000_000, 330_000_000]);
    let slow = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Multi.gw.b")
        .unwrap();
    let fast = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Multi.gw.c")
        .unwrap();
    p.can.buses[p.can.controller_buses[slow]].bitrate = 125_000;
    p.can.controllers[slow].queue_capacity = 1;
    let s = runtime::simulate(&p).unwrap();
    let held = s
        .gateway
        .rx_buffers
        .iter()
        .find(|b| s.can.requests[b.parent_request].request_id == "source:2")
        .unwrap();
    assert_eq!(
        held.remaining_egress,
        std::collections::BTreeSet::from([slow])
    );
    assert_eq!(held.status, "holding");
    let fast_copies: Vec<_> = s
        .can
        .requests
        .iter()
        .filter(|r| r.source == p.can.controllers[fast].id)
        .collect();
    assert_eq!(fast_copies.len(), 3);
    assert!(fast_copies.iter().all(|r| r.status == "success"));
    assert_eq!(
        s.gateway
            .rx_buffers
            .iter()
            .filter(|b| b.status == "dropped")
            .count(),
        1
    );
    p.common.time_limit_ps = 1_500_000_000;
    let finished = runtime::simulate(&p).unwrap();
    assert!(
        finished
            .gateway
            .rx_buffers
            .iter()
            .all(|b| b.status != "holding")
    );
    assert_eq!(
        finished
            .can
            .requests
            .iter()
            .filter(|r| r.source == p.can.controllers[fast].id)
            .count(),
        3
    );
    assert_eq!(
        finished
            .can
            .requests
            .iter()
            .filter(|r| r.request_id.starts_with("gw:"))
            .count(),
        6
    );
}

#[test]
fn gateway_zero_rx_and_unmatched_receptions_have_defined_capacity_semantics() {
    let mut p = input::prepare(&fixture("queue.ini")).unwrap();
    p.gateway.gateways[0].rx_queue_capacity = 0;
    let s = runtime::simulate(&p).unwrap();
    assert!(s.gateway.forwards.is_empty());
    assert_eq!(s.gateway.rx_buffers.len(), 3);
    assert!(
        s.gateway
            .rx_buffers
            .iter()
            .all(|b| b.status == "dropped" && b.released_ps.is_none())
    );
    assert!(s.can.requests.iter().all(|r| r.status == "success"));
    let mut no_route = input::prepare(&fixture("no-route.ini")).unwrap();
    let accepted = runtime::simulate(&no_route).unwrap();
    assert_eq!(
        accepted.gateway.rx_buffers[0].released_ps,
        Some(accepted.gateway.rx_buffers[0].received_ps)
    );
    assert!(accepted.gateway.rx_buffers[0].egress.is_empty());
    no_route.gateway.gateways[0].rx_queue_capacity = 0;
    let rejected = runtime::simulate(&no_route).unwrap();
    assert!(rejected.gateway.forwards.is_empty());
    assert_eq!(
        rejected.gateway.rx_buffers[0].reason.as_deref(),
        Some("rx_queue_full")
    );
    let disabled =
        runtime::simulate(&input::prepare(&fixture("capacity-zero.ini")).unwrap()).unwrap();
    assert!(
        disabled
            .gateway
            .rx_buffers
            .iter()
            .all(|b| b.status == "released")
    );
}

#[test]
fn gateway_rx_capacity_json_is_strict_and_normalized() {
    let temp = Temp::new();
    copy_tree(&fixture(""), &temp.0.join("input"));
    let path = temp.0.join("input/queue-routing.json");
    let original: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    for value in [
        json!(-1),
        json!(4294967296u64),
        json!(true),
        json!(1.0),
        json!("1"),
        Value::Null,
    ] {
        let mut routing = original.clone();
        routing["gateways"][0]["rx_queue_capacity"] = value.clone();
        fs::write(&path, routing.to_string()).unwrap();
        assert!(
            input::prepare(&temp.0.join("input/queue.ini")).is_err(),
            "accepted {value}"
        );
    }
    for value in [0, 1, u32::MAX as u64] {
        let mut routing = original.clone();
        routing["gateways"][0]["rx_queue_capacity"] = json!(value);
        fs::write(&path, routing.to_string()).unwrap();
        let p = input::prepare(&temp.0.join("input/queue.ini")).unwrap();
        assert_eq!(p.gateway.gateways[0].rx_queue_capacity, value);
    }
}

#[test]
fn gateway_default_rx_capacity_accepts_64_and_drops_the_65th() {
    let mut p = input::prepare(&fixture("queue.ini")).unwrap();
    p.common.time_limit_ps = 8_000_000_000;
    p.gateway.gateways[0].processing_delay_ps = 10_000_000_000;
    p.can.generators[0].schedule =
        dir_simulator::types::Schedule::Explicit((0..65).map(|n| n * 110_000_000).collect());
    let default = runtime::simulate(&p).unwrap();
    assert_eq!(
        default
            .gateway
            .rx_buffers
            .iter()
            .filter(|b| b.status == "holding")
            .count(),
        64
    );
    assert_eq!(
        default
            .gateway
            .rx_buffers
            .iter()
            .filter(|b| b.status == "dropped")
            .count(),
        1
    );
    assert_eq!(
        default
            .can
            .requests
            .iter()
            .filter(|r| r.request_id.starts_with("gw:"))
            .count(),
        0
    );
    p.gateway.gateways[0].rx_queue_capacity = u32::MAX as u64;
    let maximum = runtime::simulate(&p).unwrap();
    assert_eq!(maximum.gateway.rx_buffers.len(), 65);
    assert!(
        maximum
            .gateway
            .rx_buffers
            .iter()
            .all(|b| b.status == "holding")
    );
}

#[test]
fn gateway_same_egress_waits_follow_first_ready_order_across_ingresses() {
    let mut p = input::prepare(&fixture("multicast.ini")).unwrap();
    let slow = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Multi.gw.b")
        .unwrap();
    let other_ingress = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Multi.gw.c")
        .unwrap();
    let other_source = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Multi.sinkC")
        .unwrap();
    p.common.time_limit_ps = 3_000_000_000;
    p.can.buses[p.can.controller_buses[slow]].bitrate = 125_000;
    p.can.controllers[slow].queue_capacity = 1;
    p.gateway.gateways[0].routes[0].egress = vec![slow];
    p.gateway.gateways[0]
        .routes
        .push(dir_simulator::types::Route {
            id: "cb".into(),
            ingress: other_ingress,
            egress: vec![slow],
            format: "standard".into(),
            id_min: 1,
            id_max: 1,
        });
    p.can.generators[0].schedule =
        dir_simulator::types::Schedule::Explicit(vec![0, 110_000_000, 220_000_000]);
    let mut other = p.can.generators[0].clone();
    other.id = "cross".into();
    other.source = other_source;
    other.frame.id = 1; // 47 bits at 1Mbps: EOF aligns with the 50-bit input at 500kbps.
    other.schedule =
        dir_simulator::types::Schedule::Explicit(vec![53_000_000, 163_000_000, 273_000_000]);
    p.can.generators.push(other);
    let order = |p: &PreparedSimulation| {
        let s = runtime::simulate(p).unwrap();
        assert!(!s.common.partial);
        s.common
            .points
            .iter()
            .filter(|point| {
                point.metric == "queue_length"
                    && point.target == "Multi.gw.b.txQueue"
                    && point.value == 1
            })
            .map(|point| point.request_id.clone().unwrap())
            .collect::<Vec<_>>()
    };
    let expected: Vec<_> = [
        "source:0", "cross:0", "source:1", "cross:1", "source:2", "cross:2",
    ]
    .iter()
    .map(|id| {
        format!(
            "gw:{id}/Multi.gw/{}/Multi.gw.b",
            if id.starts_with("cross") { "cb" } else { "ab" }
        )
    })
    .collect();
    assert_eq!(order(&p), expected);
    p.can.generators.reverse();
    p.gateway.gateways[0].routes.reverse();
    assert_eq!(order(&p), expected);
}
