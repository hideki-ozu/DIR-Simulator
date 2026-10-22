//! Real WireHost producers sharing existing Clause99 and project serializers.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn records<'a>(v: &'a Value, schema: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .map(|r| &r["data"])
        .collect()
}
fn execute(name: &str) -> Value {
    let root = std::env::var_os("DIR_WIRE_PRODUCER_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "wire-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    fs::create_dir_all(&root).unwrap();
    let p = prepare(&repo().join(format!(
        "examples/network-composite/wire-serializers/{name}.ini"
    )))
    .unwrap();
    run(p, &root).unwrap();
    serde_json::from_slice(&fs::read(root.join("results.json")).unwrap()).unwrap()
}
#[test]
fn ideal_wire_merge_actual_ingress_interrupts_pmac() {
    let v = execute("merge");
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let packets = records(&v, "dir.mac_merge.packet");
    let wire: Vec<_> = packets
        .iter()
        .filter(|p| p["port"] == "Net.s1.tx_b" && p["wire_control_id"].is_string())
        .collect();
    assert!(!wire.is_empty(), "{packets:?}");
    for p in wire {
        assert_eq!(p["kind"], "Express");
        assert_eq!(p["wire_hex"], p["received_hex"]);
        assert!(p["arrival_ps"].is_string());
        assert_eq!(p["wire_priority"], 7);
    }
    assert!(
        packets.iter().any(|p| p["port"] == "Net.s1.tx_b"
            && p["kind"]
                .as_str()
                .is_some_and(|s| s.starts_with("Continuation"))),
        "{packets:?}"
    );
    assert!(
        records(&v, "ethernet.control.wire.event")
            .iter()
            .any(|p| p["event_kind"] == "receive")
    );
}
#[test]
fn abstract_wire_producer_preserves_fragment_and_resume_gap() {
    let v = execute("abstract");
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let events = records(&v, "ethernet.control.wire.event");
    assert!(
        events
            .iter()
            .any(|p| p["port"] == "Net.s1.tx_b" && p["event_kind"] == "tx_started")
    );
    assert!(events.iter().any(|p| p["event_kind"] == "receive"));
    assert!(
        records(&v, "dir.preemption.fragment")
            .iter()
            .any(|p| p["port"] == "Net.s1.tx_b")
    );
    assert!(
        records(&v, "dir.preemption.gap")
            .iter()
            .any(|p| p["port"] == "Net.s1.tx_b")
    );
}
#[test]
#[ignore = "actual sampled 6ms training with fractional Wire producer and late PCS ingress"]
fn sampled_wire_merge_actual_training_fragments_and_late_pcs() {
    let v = execute("sampled");
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let packets = records(&v, "dir.mac_merge.packet");
    let wire: Vec<_> = packets
        .iter()
        .filter(|p| p["port"] == "Net.s1.tx_b" && p["wire_control_id"].is_string())
        .collect();
    assert!(!wire.is_empty());
    for p in wire {
        assert_eq!(p["timing_plane"], "actual_xmii");
        assert_eq!(p["wire_hex"], p["received_hex"]);
        let release = p["release_ps"].as_str().unwrap().parse::<u64>().unwrap();
        let arrival = p["arrival_ps"].as_str().unwrap().parse::<u64>().unwrap();
        assert!(arrival > release);
        assert!(p["raw_phy"]["start_sample"].is_number());
    }
    assert!(
        packets.iter().any(|p| p["port"] == "Net.s1.tx_b"
            && p["kind"]
                .as_str()
                .is_some_and(|s| s.starts_with("Continuation"))),
        "{packets:?}"
    );
    assert!(
        records(&v, "ethernet.control.wire.event")
            .iter()
            .any(|p| p["event_kind"] == "receive")
    );
}

mod checkpoint {
    use super::*;
    use dir_simulator::{
        CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
        registry::Registry, restore_checkpoint,
    };
    use serde_json::json;
    fn child(root: &Path, mode: &str, bundle: &str) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "checkpoint::process_worker", "--nocapture"])
            .env("DIR_WIRE_CP_ROOT", root)
            .env("DIR_WIRE_CP_MODE", mode)
            .env("DIR_WIRE_CP_BUNDLE", bundle)
            .output()
            .unwrap();
        let mut log = output.stdout.clone();
        log.extend_from_slice(&output.stderr);
        fs::write(root.join(format!("{mode}-{bundle}.log")), log).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for e in fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(e.file_name()))
            } else {
                fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    fn mutate(root: &Path, f: impl FnOnce(&mut Value)) {
        use sha2::{Digest, Sha256};
        let mut state: Value =
            serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        f(&mut state);
        let bytes = serde_json::to_vec(&state).unwrap();
        fs::write(root.join("state.json"), &bytes).unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        let entry = manifest["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["path"] == "state.json")
            .unwrap();
        entry["bytes"] = json!(bytes.len().to_string());
        entry["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn process_worker() {
        let Ok(mode) = std::env::var("DIR_WIRE_CP_MODE") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("DIR_WIRE_CP_ROOT").unwrap());
        let bundle = root.join(std::env::var_os("DIR_WIRE_CP_BUNDLE").unwrap());
        if mode == "save" {
            let src = repo().join("examples/network-composite/wire-serializers");
            let sampled = root.join("sampled.flag").exists();
            fs::copy(
                src.join(if sampled {
                    "sampled-model.json"
                } else {
                    "merge-model.json"
                }),
                root.join("merge-model.json"),
            )
            .unwrap();
            fs::copy(
                src.join(if sampled {
                    "sampled-workload.json"
                } else {
                    "workload.json"
                }),
                root.join("workload.json"),
            )
            .unwrap();
            let abstract_case = root.join("abstract-case.json");
            let abstract_input: Option<Value> = abstract_case
                .exists()
                .then(|| serde_json::from_slice(&fs::read(&abstract_case).unwrap()).unwrap());
            if let Some(input) = &abstract_input {
                fs::write(
                    root.join("merge-model.json"),
                    serde_json::to_vec(&input["model"]).unwrap(),
                )
                .unwrap();
                fs::write(
                    root.join("workload.json"),
                    serde_json::to_vec(&input["work"]).unwrap(),
                )
                .unwrap();
            }
            let mut model: Value =
                serde_json::from_slice(&fs::read(root.join("merge-model.json")).unwrap()).unwrap();
            for output in model["base"]["config"]["tsn"]["outputs"]
                .as_array_mut()
                .unwrap()
            {
                if abstract_input.is_none() && output["port"] == "Net.s1.tx_b" {
                    output["cbs"] = json!([{"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}]);
                }
            }
            if sampled {
                model["physical_mac_policy"] = json!("dir.experimental.sampled_wire_cbs.v1");
            }
            fs::write(
                root.join("merge-model.json"),
                serde_json::to_vec(&model).unwrap(),
            )
            .unwrap();
            fs::write(root.join("model.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"merge-model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",repo().join("examples/network-composite/wire-serializers/models").display())).unwrap();
            if sampled {
                let path = root.join("model.ini");
                fs::write(
                    &path,
                    fs::read_to_string(&path)
                        .unwrap()
                        .replace("10000000ps", "6012000000ps"),
                )
                .unwrap();
            }
            let (mut s, c) = RunSession::new(
                prepare(&root.join("model.ini")).unwrap(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            loop {
                let state = s.advance_one().unwrap();
                assert_eq!(
                    state,
                    RunState::Running,
                    "{:?}",
                    s.observe().unwrap().common.diagnostics
                );
                if s.observe()
                    .unwrap()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .values()
                    .any(|r| {
                        if let Some(input) = &abstract_input {
                            r.schema.name == "dir.preemption.transfer"
                                && r.data["reset_ps"] == input["cut"]
                        } else {
                            r.schema.name == "dir.mac_merge.packet"
                                && r.data["wire_control_id"].is_string()
                                && r.data["eof_ps"].is_null()
                        }
                    })
                {
                    break;
                }
            }
            c.submit(ControlCommand::Pause).unwrap();
            assert_eq!(s.poll_control().unwrap(), RunState::Paused);
            let ack = s.checkpoint(&bundle, CheckpointLimits::default()).unwrap();
            assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
            s.resume(s.pause_token().unwrap()).unwrap();
            assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
            fs::write(
                root.join("expected.json"),
                serde_json::to_vec(
                    &s.observe()
                        .unwrap()
                        .registered
                        .as_ref()
                        .unwrap()
                        .model_records,
                )
                .unwrap(),
            )
            .unwrap();
            for file in ["model.ini", "merge-model.json", "workload.json"] {
                fs::remove_file(root.join(file)).unwrap();
            }
        } else if mode == "restore" {
            let (mut s, t) =
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .unwrap();
            s.resume(t).unwrap();
            assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
            let expected: Value =
                serde_json::from_slice(&fs::read(root.join("expected.json")).unwrap()).unwrap();
            assert_eq!(
                json!(
                    s.observe()
                        .unwrap()
                        .registered
                        .as_ref()
                        .unwrap()
                        .model_records
                ),
                expected
            );
        } else {
            let error =
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .err()
                    .expect("accepted co-rehashed Wire producer mutant");
            eprintln!("co-rehashed producer rejection: {}", error.message);
            assert!(
                error.message.contains("frozen-input"),
                "mutant did not reach independent frozen source authority: {error:?}"
            );
        }
    }
    #[test]
    fn fresh_process_frozen_prefix_and_rehashed_wire_owner_mutants() {
        check_prefix(false)
    }
    #[test]
    #[ignore = "actual sampled Wire startup and fresh-process source-prefix/co-rehashed proof matrix"]
    fn sampled_frozen_prefix_and_rehashed_wire_sample_mutants() {
        check_prefix(true)
    }
    #[test]
    fn abstract_reset_fresh_process_prefix_continuation_and_rehashed_mutants() {
        let src = repo().join("examples/network-composite/wire-serializers");
        let mut model: Value =
            serde_json::from_slice(&fs::read(src.join("abstract-model.json")).unwrap()).unwrap();
        let work: Value =
            serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
        model["configured_media"] = json!({"seed":"7","physical_links":[{"id":"delayed_rx","a":"Net.b.tx","b":"Net.s1.tx_b","phy_mode":"1000base-t1","duplex":"full","a_phy":{"role":"master","tx_latency_ps":"0","rx_latency_ps":"500000"},"b_phy":{"role":"slave","tx_latency_ps":"0","rx_latency_ps":"0"}}]});
        let baseline = execute_variant_until(&model, &work, "abstract-cp-reference", 20_000_000);
        let transfers = records(&baseline, "dir.preemption.transfer");
        let row = transfers
            .iter()
            .find(|r| r["from_port"] == "Net.s1.tx_b")
            .unwrap();
        let events = records(&baseline, "ethernet.control.wire.event");
        let wire = events
            .iter()
            .find(|r| r["port"] == "Net.s1.tx_b" && r["event_kind"] == "tx_started")
            .unwrap();
        for cut in [
            row["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() + 100_000,
            wire["time_ps"].as_str().unwrap().parse::<u64>().unwrap() + 100_000,
            row["eof_ps"].as_str().unwrap().parse::<u64>().unwrap() + 1,
        ] {
            let root = std::env::var_os("DIR_WIRE_PRODUCER_EVIDENCE")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(format!(
                    "wire-abstract-cp-{}-{}",
                    cut,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&root).unwrap();
            let mut w = work.clone();
            w["controls"] = json!([{"id":"down","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":false},{"id":"up","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":true}]);
            fs::write(
                root.join("abstract-case.json"),
                serde_json::to_vec(&json!({"model":model,"work":w,"cut":cut.to_string()})).unwrap(),
            )
            .unwrap();
            child(&root, "save", "bundle");
            child(&root, "restore", "bundle");
            for field in ["partial_mac_bytes", "reset_ps"] {
                let name = format!("mutant-{field}");
                copy(&root.join("bundle"), &root.join(&name));
                mutate(&root.join(&name), |s| {
                    let transfers = s["network_backend"]["state"]["core"]["transfers"]
                        .as_array_mut()
                        .unwrap();
                    let t = transfers
                        .iter_mut()
                        .find(|t| !t["fragments"]["reset_ps"].is_null())
                        .unwrap();
                    let v = t["fragments"][field].as_u64().unwrap();
                    t["fragments"][field] = json!(v + 1);
                });
                child(&root, "reject", &name);
            }
        }
    }
    fn check_prefix(sampled: bool) {
        let root = std::env::var_os("DIR_WIRE_PRODUCER_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "wire-cp-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        if sampled {
            fs::write(root.join("sampled.flag"), b"sampled").unwrap();
        }
        child(&root, "save", "bundle");
        child(&root, "restore", "bundle");
        let mut kinds = vec!["id", "bytes", "source", "epoch", "owner", "credit"];
        if sampled {
            kinds.push("sample")
        }
        for kind in kinds {
            let name = format!("mutant-{kind}");
            let target = root.join(&name);
            copy(&root.join("bundle"), &target);
            mutate(&target, |s| {
                let n = &mut s["network_backend"]["state"];
                if kind == "credit" {
                    let ports = n["tsn"]["ports"].as_array_mut().unwrap();
                    let port = ports.iter_mut().find(|p| !p["cbs"][7].is_null()).unwrap();
                    port["cbs"][7]["credit"] = json!([false, "123456789"]);
                    return;
                }
                if kind == "sample" {
                    let packet = n["mac_merge"]["packets"]
                        .as_object_mut()
                        .unwrap()
                        .values_mut()
                        .find(|p| p["wire_control_id"].is_string())
                        .unwrap();
                    let index = packet["raw"]["attachment"].as_u64().unwrap() as usize;
                    let sample = packet["raw"]["start_sample"].as_u64().unwrap();
                    packet["raw"]["start_sample"] = json!(sample + 1);
                    let bytes: Vec<u8> = serde_json::from_value(n["phy"][index].clone()).unwrap();
                    let mut attachment: Value = serde_json::from_slice(&bytes).unwrap();
                    let bytes: Vec<u8> =
                        serde_json::from_value(attachment["medium"].clone()).unwrap();
                    let mut medium: Value = serde_json::from_slice(&bytes).unwrap();
                    let port = medium["state"]["ports"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|p| {
                            !p["raw_active"].is_null()
                                && p["raw_active"]["request"]["reference_sample"] == sample
                        })
                        .unwrap();
                    port["raw_active"]["request"]["reference_sample"] = json!(sample + 1);
                    attachment["medium"] = json!(serde_json::to_vec(&medium).unwrap());
                    n["phy"][index] = json!(serde_json::to_vec(&attachment).unwrap());
                    return;
                }
                let packet = n["mac_merge"]["packets"]
                    .as_object_mut()
                    .unwrap()
                    .values_mut()
                    .find(|p| p["wire_control_id"].is_string())
                    .unwrap();
                match kind {
                    "id" => packet["wire_control_id"] = json!("forged"),
                    "bytes" => packet["bytes"][10] = json!(9),
                    "source" => packet["origin"] = json!("Lldp"),
                    "epoch" => packet["epoch"] = json!(123),
                    "owner" => packet["direction"] = json!(0),
                    _ => unreachable!(),
                }
            });
            child(&root, "reject", &name);
        }
    }
}

fn execute_variant(model: &Value, work: &Value, label: &str) -> Value {
    execute_variant_until(
        model,
        work,
        label,
        if model.get("phy_attachments").is_some() {
            6_012_000_000
        } else {
            10_000_000
        },
    )
}
fn execute_variant_until(model: &Value, work: &Value, label: &str, limit: u64) -> Value {
    let root = std::env::var_os("DIR_WIRE_PRODUCER_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "wire-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("model.json"),
        serde_json::to_vec_pretty(model).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("workload.json"),
        serde_json::to_vec_pretty(work).unwrap(),
    )
    .unwrap();
    fs::write(root.join("model.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",repo().join("examples/network-composite/wire-serializers/models").display())).unwrap();
    if model.get("phy_attachments").is_some() {
        let path = root.join("model.ini");
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("10000000ps", "6012000000ps"),
        )
        .unwrap();
    }
    let ini = root.join("model.ini");
    let original = fs::read_to_string(&ini).unwrap();
    let old = if model.get("phy_attachments").is_some() {
        "6012000000ps"
    } else {
        "10000000ps"
    };
    fs::write(&ini, original.replace(old, &format!("{limit}ps"))).unwrap();
    run(prepare(&root.join("model.ini")).unwrap(), &root.join("out")).unwrap();
    serde_json::from_slice(&fs::read(root.join("out/results.json")).unwrap()).unwrap()
}
#[test]
fn same_class_wire_and_data_use_their_own_full_fit_in_both_size_orders() {
    use serde_json::json;
    let source = repo().join("examples/network-composite/wire-serializers");
    for large_wire in [false, true] {
        let mut m: Value =
            serde_json::from_slice(&fs::read(source.join("merge-model.json")).unwrap()).unwrap();
        let mut w: Value =
            serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
        m["mac_merge"]["capability_adapter"]["interval_ms"] = json!(1001);
        w["generators"][0]["times_ps"] = json!(["2000004000000"]);
        for p in m["base"]["config"]["base"]["config"]["ports"]
            .as_array_mut()
            .unwrap()
        {
            if p["port"] == "Net.s1.tx_b" {
                p["default_priority"] = json!(0)
            }
        }
        for output in m["base"]["config"]["tsn"]["outputs"]
            .as_array_mut()
            .unwrap()
        {
            if output["port"] == "Net.s1.tx_b" {
                output["tas"] = json!({"id":"sizes","base_time_ps":"0","cycle_time_ps":"2000012000000","entries":[{"duration_ps":"2000000000000","open_priorities":[0,1,2,3,4,5,6,7]},{"duration_ps":"6000000","open_priorities":[1,2,3,4,5,6,7]},{"duration_ps":"720000","open_priorities":[0,1,2,3,4,5,6,7]},{"duration_ps":"1280000","open_priorities":[1,2,3,4,5,6,7]},{"duration_ps":"4000000","open_priorities":[0,1,2,3,4,5,6,7]}]});
            }
        }
        if large_wire {
            let template: Value = serde_json::from_slice(
                &fs::read(repo().join("examples/network-composite/mstp-region/model.json"))
                    .unwrap(),
            )
            .unwrap();
            let mut a = template["wire_control"]["adapters"][0].clone();
            a["ports"] = json!(["Net.s1.tx_a", "Net.s1.tx_b"]);
            a["mstp"]["port_parameters"]
                .as_array_mut()
                .unwrap()
                .truncate(2);
            for p in a["mstp"]["port_parameters"].as_array_mut().unwrap() {
                p["admin_edge"] = json!(true)
            }
            m["wire_control"]["adapters"] = json!([a]);
            w["generators"][0]["frame"]["data"] = json!("");
        }
        let v = execute_variant_until(
            &m,
            &w,
            if large_wire {
                "large-wire"
            } else {
                "small-wire"
            },
            2_000_014_000_000,
        );
        assert_eq!(v["simulation"]["termination"], "time_limit");
        let packets = records(&v, "dir.mac_merge.packet");
        let wire = packets
            .iter()
            .filter(|p| {
                p["port"] == "Net.s1.tx_b"
                    && p["wire_control_id"].is_string()
                    && p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() >= 2_000_000_000_000
            })
            .min_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .unwrap();
        let data = packets
            .iter()
            .find(|p| p["port"] == "Net.s1.tx_b" && p["transfer"].is_string())
            .unwrap();
        if large_wire {
            assert!(
                wire["wire_hex"].as_str().unwrap().len() > data["wire_hex"].as_str().unwrap().len()
            );
            assert_eq!(data["sof_ps"], "2000006000000");
            assert!(wire["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() >= 2_000_008_000_000);
        } else {
            assert_eq!(wire["sof_ps"], "2000006000000");
            assert!(data["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() >= 2_000_008_000_000);
        }
    }
}

#[test]
fn project_pause_policy_uses_real_wire_producer_and_keeps_targeted_guards() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    let mut m: Value =
        serde_json::from_slice(&fs::read(src.join("abstract-model.json")).unwrap()).unwrap();
    let w: Value = serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
    m["mac_control"] = json!({"ports":[{"port":"Net.s1.tx_b","station_mac":"02:00:00:00:01:11","rx_enabled":true,"tx_enabled":true,"policy":"dir.experimental.project_preemption_pause.v1"},{"port":"Net.b.tx","station_mac":"02:00:00:00:00:02","rx_enabled":true,"tx_enabled":false,"policy":"dir.experimental.project_preemption_pause.v1"}],"controls":[{"at_ps":"0","port":"Net.s1.tx_b","kind":"pause_request","quanta":2}]});
    for output in m["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
    {
        if output["port"] == "Net.s1.tx_b" {
            output["tas"] = json!({"id":"closed","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[]}]});
        }
    }
    let v = execute_variant(&m, &w, "project-pause");
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let controls = records(&v, "dir.network.mac_control");
    let rx = controls
        .iter()
        .find(|p| p["kind"] == "receive" && p["port"] == "Net.b.tx")
        .unwrap();
    let bytes = rx["evidence"]["mac_hex"].as_str().unwrap();
    assert_eq!(bytes.len(), 128);
    assert!(bytes.starts_with("0180c2000001020000000111880800010002"));
    assert!(
        controls
            .iter()
            .any(|p| p["kind"] == "timer_load" && p["evidence"]["duration_ps"] == "1024000")
    );
    assert!(
        !records(&v, "ethernet.control.wire.event")
            .iter()
            .any(|p| p["port"] == "Net.s1.tx_b" && p["event_kind"] == "tx_started")
    );
    let root = std::env::temp_dir().join(format!(
        "wire-policy-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    for (case, policy, port) in [
        (
            "old-policy",
            "dir.experimental.merge_pause.v1",
            "Net.s1.tx_b",
        ),
        ("standard-policy", "ieee802.3.pause.v1", "Net.s1.tx_b"),
        (
            "wrong-owner",
            "dir.experimental.project_preemption_pause.v1",
            "Net.a.tx",
        ),
    ] {
        let mut bad = m.clone();
        bad["mac_control"]["ports"][0]["policy"] = json!(policy);
        bad["mac_control"]["ports"][0]["port"] = json!(port);
        bad["mac_control"]["controls"][0]["port"] = json!(port);
        fs::write(root.join("model.json"), serde_json::to_vec(&bad).unwrap()).unwrap();
        fs::write(root.join("workload.json"), serde_json::to_vec(&w).unwrap()).unwrap();
        fs::write(root.join("model.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",src.join("models").display())).unwrap();
        let e = prepare(&root.join("model.ini")).unwrap_err();
        assert_eq!(
            e.target.as_deref(),
            Some("/mac_control/ports"),
            "{case}: {e:?}"
        );
    }
}

#[test]
#[ignore = "actual sampled same-ceil demand/gate causality and unrelated continuous peer traffic"]
fn sampled_same_ceil_demand_gate_and_peer_traffic_remain_causal() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    let mut m: Value =
        serde_json::from_slice(&fs::read(src.join("sampled-model.json")).unwrap()).unwrap();
    let mut w: Value =
        serde_json::from_slice(&fs::read(src.join("sampled-workload.json")).unwrap()).unwrap();
    // Real source serialization + bridge processing produces the queued cut N.
    let queued = 6_001_573_679u64;
    w["generators"][0]["times_ps"] = json!(["6000003679"]);
    let opening = 6_002_005_679u64;
    for output in m["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
    {
        if output["port"] == "Net.s1.tx_b" {
            output["tas"]["entries"][0]["duration_ps"] = json!(opening.to_string());
            output["tas"]["entries"][1]["duration_ps"] =
                json!((7_000_000_000 - opening).to_string());
        }
    }
    let mut peer = w["generators"][0].clone();
    peer["id"] = json!("peer");
    peer["flow_id"] = json!("peer");
    peer["node"] = json!("Net.b");
    peer["priority"] = json!(7);
    peer["frame"]["dst_mac"] = json!("02:00:00:00:00:01");
    peer["frame"]["tag"]["pcp"] = json!(7);
    peer["frame"]["data"] = json!("");
    peer["times_ps"] = json!(
        (0..16)
            .map(|i| (6_000_000_000u64 + i * 600_000).to_string())
            .collect::<Vec<_>>()
    );
    w["generators"].as_array_mut().unwrap().push(peer);
    let v = execute_variant(&m, &w, "causal-peer");
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let packets = records(&v, "dir.mac_merge.packet");
    let data = packets
        .iter()
        .filter(|p| {
            p["port"] == "Net.s1.tx_b"
                && p["transfer"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("unicast:"))
        })
        .min_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
        .unwrap();
    let transfer = records(&v, "ethernet.dynamic.transfer")
        .into_iter()
        .find(|r| r["copy_id"] == data["transfer"])
        .unwrap();
    assert_eq!(transfer["queued_ps"], queued.to_string());
    let sample = data["raw_phy"]["start_sample"].as_u64().unwrap();
    let den = 12_000_000_000u128;
    let physical = u128::from(sample) * 1_000_000_000_000 + 12345 * den;
    assert!(
        physical >= u128::from(queued) * den,
        "new demand retrostarted"
    );
    assert!(physical - u128::from(queued) * den <= 8000 * den);
    assert_ne!(data["sof_ps"], queued.to_string());
    let wire = packets
        .iter()
        .filter(|p| p["port"] == "Net.s1.tx_b" && p["wire_control_id"].is_string())
        .min_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
        .unwrap();
    let sample = wire["raw_phy"]["start_sample"].as_u64().unwrap();
    assert!(
        u128::from(sample) * 1_000_000_000_000 + 12345 * den >= u128::from(opening) * den,
        "gate opening retrostarted"
    );
    assert!(
        wire["arrival_ps"].is_string(),
        "unrelated peer traffic stranded Wire"
    );
    assert!(
        packets
            .iter()
            .filter(|p| p["port"] == "Net.b.tx" && p["transfer"].is_string())
            .count()
            >= 8
    );
}

#[test]
fn wire_link_reset_terminalizes_old_claim_once_and_new_claim_survives() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    for owner in ["merge", "abstract"] {
        let original: Value =
            serde_json::from_slice(&fs::read(src.join(format!("{owner}-model.json"))).unwrap())
                .unwrap();
        let work: Value =
            serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
        let reference = execute_variant(&original, &work, "reset-reference");
        let source_events = records(&reference, "ethernet.control.wire.event");
        let sof = source_events
            .iter()
            .filter(|p| p["port"] == "Net.s1.tx_b" && p["event_kind"] == "tx_started")
            .map(|p| p["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .min()
            .unwrap();
        for late in [false, true] {
            let cut = sof + if late { 600_000 } else { 300_000 };
            let mut m = original.clone();
            let mut w = work.clone();
            if late {
                m["configured_media"] = json!({"seed":"7","physical_links":[{"id":"delayed_rx","a":"Net.b.tx","b":"Net.s1.tx_b","phy_mode":"1000base-t1","duplex":"full","a_phy":{"role":"master","tx_latency_ps":"0","rx_latency_ps":"500000"},"b_phy":{"role":"slave","tx_latency_ps":"0","rx_latency_ps":"0"}}]});
            }
            w["controls"] = json!([{"id":"down","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":false},{"id":"up","at_ps":(cut+100_000).to_string(),"kind":"link_set","link":"B","up":true}]);
            let v = execute_variant(&m, &w, "reset");
            assert_eq!(
                v["simulation"]["termination"], "time_limit",
                "{owner}/{late}"
            );
            let events = records(&v, "ethernet.control.wire.event");
            let started = events
                .iter()
                .filter(|p| p["port"] == "Net.s1.tx_b" && p["event_kind"] == "tx_started")
                .collect::<Vec<_>>();
            assert!(!started.is_empty());
            let id = &started[0]["packet_id"];
            assert!(id.is_string());
            assert_eq!(
                events
                    .iter()
                    .filter(|p| p["packet_id"] == *id && p["reason"] == "link_epoch_reset")
                    .count(),
                1,
                "{owner}/{late}"
            );
            assert!(!events.iter().any(|p| p["packet_id"] == *id
                && p["event_kind"] == "receive"
                && p["outcome"] == "changed"));
            assert!(
                started.iter().skip(1).any(|p| p["packet_id"] != *id),
                "new claim must survive {owner}/{late}"
            );
            if late {
                assert!(events.iter().any(|p| p["packet_id"] == *id
                    && p["event_kind"] == "tx_completed"
                    && p["outcome"] == "changed"));
                assert!(events.iter().any(|p| p["packet_id"] == *id
                    && p["event_kind"] == "rx_failed"
                    && p["outcome"] == "control_drop"));
            }
        }
    }
}

#[test]
#[ignore = "actual sampled clock/admin ABA/PAUSE-expiry admission fences"]
fn sampled_permission_changes_at_ceiling_defer_exactly_to_later_cut() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    for case in ["clock", "admin_aba", "pause_expiry"] {
        let mut m: Value =
            serde_json::from_slice(&fs::read(src.join("sampled-model.json")).unwrap()).unwrap();
        let mut w: Value =
            serde_json::from_slice(&fs::read(src.join("sampled-workload.json")).unwrap()).unwrap();
        w["generators"] = json!([]);
        let n = if case == "pause_expiry" {
            6_004_725_679u64
        } else {
            6_002_005_679
        };
        for o in m["base"]["config"]["tsn"]["outputs"]
            .as_array_mut()
            .unwrap()
        {
            if o["port"] == "Net.s1.tx_b" {
                o["tas"]["entries"][0]["duration_ps"] = json!((n - 1).to_string());
                o["tas"]["entries"][1]["duration_ps"] =
                    json!((7_000_000_000 - (n - 1)).to_string());
            }
        }
        if case == "clock" {
            m["clocks"] = json!({"nodes":[{"node":"Net.s1","offset_ps":"0","rate_num":"1","rate_den":"1"}],"adjustments":[{"node":"Net.s1","at_ps":n.to_string(),"kind":"step","offset_ps":"1"}]});
        } else {
            m["mac_control"] = json!({"ports":[{"port":"Net.s1.tx_b","station_mac":"02:00:00:00:01:11","rx_enabled":true,"tx_enabled":false,"policy":"dir.experimental.merge_pause.v1"}],"controls":[]});
            if case == "admin_aba" {
                m["mac_control"]["controls"] = json!([{"at_ps":n.to_string(),"port":"Net.s1.tx_b","kind":"transmit_enable","enabled":false},{"at_ps":n.to_string(),"port":"Net.s1.tx_b","kind":"transmit_enable","enabled":true}]);
            } else {
                let mut bytes = vec![1, 128, 194, 0, 0, 1, 2, 0, 0, 0, 0, 2, 0x88, 8, 0, 1, 0, 1];
                bytes.resize(60, 0);
                bytes.extend(dir_simulator::runtime::ethernet::crc32(&bytes).to_le_bytes());
                let hex = bytes.iter().fold(String::new(), |mut s, b| {
                    use std::fmt::Write;
                    write!(&mut s, "{b:02x}").unwrap();
                    s
                });
                m["wire_control"]["packets"] = json!([{"id":"external-pause","adapter_id":"rstp-s1","ingress_port":"Net.s1.rx_b","arrival_ps":(n-512_000).to_string(),"encapsulation":"mac_frame_fcs","bytes_hex":hex}]);
            }
        }
        let v = execute_variant(&m, &w, case);
        assert_eq!(v["simulation"]["termination"], "time_limit", "{case}");
        let packets = records(&v, "dir.mac_merge.packet");
        let wire = packets
            .iter()
            .filter(|p| p["port"] == "Net.s1.tx_b" && p["wire_control_id"].is_string())
            .min_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .unwrap();
        let sample = wire["raw_phy"]["start_sample"].as_u64().unwrap();
        let den = 12_000_000_000u128;
        let physical = u128::from(sample) * 1_000_000_000_000 + 12345 * den;
        assert!(
            physical >= u128::from(n) * den,
            "{case} used earlier physical cut"
        );
        assert!(
            physical - u128::from(n) * den <= 8000 * den,
            "{case} stranded stable permission"
        );
        assert_ne!(wire["sof_ps"], n.to_string());
        if case == "pause_expiry" {
            let controls = records(&v, "dir.network.mac_control");
            let timer = controls.iter().find(|p| p["kind"] == "timer_load").unwrap();
            assert_eq!(timer["state"]["timer"]["expires_ps"], n.to_string());
            assert_eq!(timer["evidence"]["duration_ps"], "512000");
        }
    }
}

#[test]
fn retained_large_emac_rechecks_own_fit_after_smaller_wire() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    let mut m: Value =
        serde_json::from_slice(&fs::read(src.join("merge-model.json")).unwrap()).unwrap();
    let mut w: Value =
        serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
    for p in m["base"]["config"]["base"]["config"]["ports"]
        .as_array_mut()
        .unwrap()
    {
        if p["port"] == "Net.s1.tx_b" {
            p["default_priority"] = json!(6)
        }
    }
    for o in m["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
    {
        if o["port"] == "Net.s1.tx_b" {
            o["tas"] = json!({"id":"retained","base_time_ps":"0","cycle_time_ps":"30000000","entries":[{"duration_ps":"17600000","open_priorities":[0,1,2,3,4,5,7]},{"duration_ps":"1900000","open_priorities":[0,1,2,3,4,5,6,7]},{"duration_ps":"2500000","open_priorities":[0,1,2,3,4,5,6]},{"duration_ps":"8000000","open_priorities":[0,1,2,3,4,5,6,7]}]});
        }
    }
    let mut express = w["generators"][0].clone();
    express["id"] = json!("retained");
    express["flow_id"] = json!("retained");
    express["priority"] = json!(7);
    express["frame"]["tag"]["pcp"] = json!(7);
    express["times_ps"] = json!(["4100000"]);
    w["generators"][0]["frame"]["data"] = json!("ab".repeat(1460));
    w["generators"].as_array_mut().unwrap().push(express);
    let v = execute_variant_until(&m, &w, "retained", 30_000_000);
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let packets = records(&v, "dir.mac_merge.packet");
    let wire = packets
        .iter()
        .find(|p| p["port"] == "Net.s1.tx_b" && p["wire_control_id"].is_string())
        .unwrap();
    assert!(wire["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() < 19_500_000);
    let express = packets
        .iter()
        .find(|p| {
            p["port"] == "Net.s1.tx_b"
                && p["transfer"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("retained:"))
        })
        .unwrap();
    assert!(
        express["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() >= 22_000_000,
        "large retained frame borrowed smaller Wire ready bit"
    );
    assert!(express["wire_hex"].as_str().unwrap().len() > wire["wire_hex"].as_str().unwrap().len());
    assert!(packets.iter().any(|p| {
        p["port"] == "Net.s1.tx_b"
            && p["kind"]
                .as_str()
                .is_some_and(|s| s.starts_with("Continuation"))
    }));
}

#[test]
fn capability_lldp_obeys_untagged_class_gate_own_fit_and_current_cbs() {
    capability_lldp_permission_cases(false);
}
#[test]
#[ignore = "actual sampled startup for ordinary LLDP gate/full-fit/CBS"]
fn sampled_capability_lldp_obeys_gate_own_fit_and_current_cbs() {
    capability_lldp_permission_cases(true);
}
fn capability_lldp_permission_cases(sampled: bool) {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    for cbs in [false, true] {
        let mut m: Value = serde_json::from_slice(
            &fs::read(src.join(if sampled {
                "sampled-model.json"
            } else {
                "merge-model.json"
            }))
            .unwrap(),
        )
        .unwrap();
        let mut w: Value =
            serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
        w["generators"] = json!([]);
        let open = if sampled { 6_002_000_000u64 } else { 6_000_000 };
        let cycle = if sampled {
            7_000_000_000u64
        } else {
            14_000_000
        };
        for o in m["base"]["config"]["tsn"]["outputs"]
            .as_array_mut()
            .unwrap()
        {
            if o["port"] == "Net.s1.tx_b" {
                o["tas"] = json!({"id":"lldp-own-fit","base_time_ps":"0","cycle_time_ps":cycle.to_string(),"entries":[{"duration_ps":open.to_string(),"open_priorities":[0,1,2,3,4,5,6]},{"duration_ps":"720000","open_priorities":[0,1,2,3,4,5,6,7]},{"duration_ps":"1280000","open_priorities":[0,1,2,3,4,5,6]},{"duration_ps":(cycle-open-2_000_000).to_string(),"open_priorities":[0,1,2,3,4,5,6,7]}]});
                if cbs {
                    o["cbs"] = json!([{"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}]);
                }
            }
        }
        if sampled && cbs {
            m["physical_mac_policy"] = json!("dir.experimental.sampled_wire_cbs.v1");
        }
        let v = execute_variant_until(
            &m,
            &w,
            if sampled {
                "sampled-lldp-permission"
            } else {
                "lldp-permission"
            },
            if sampled { 6_012_000_000 } else { 14_000_000 },
        );
        assert_eq!(
            v["simulation"]["termination"], "time_limit",
            "{:?}",
            v["simulation"]["diagnostics"]
        );
        let packets = records(&v, "dir.mac_merge.packet");
        let lldp = packets
            .iter()
            .filter(|p| {
                p["port"] == "Net.s1.tx_b"
                    && p["wire_control_id"].is_null()
                    && p["transfer"].is_null()
                    && p["kind"] == "Express"
            })
            .min_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .unwrap();
        let sof = lldp["sof_ps"].as_str().unwrap().parse::<u64>().unwrap();
        assert!(
            sof >= open + 2_000_000,
            "LLDP used another candidate's short window: {lldp}"
        );
        assert_eq!(lldp["priority"], 7);

        assert_eq!(lldp["wire_hex"], lldp["received_hex"]);
        if cbs {
            let wire = packets
                .iter()
                .filter(|p| p["port"] == "Net.s1.tx_b" && p["wire_control_id"].is_string())
                .min_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
                .unwrap();
            let first = wire["sof_ps"].as_str().unwrap().parse::<u64>().unwrap();
            let eof = wire["planned_eof_ps"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert!(
                sof >= eof + (eof - first) * 3,
                "LLDP started before class credit recovered: {lldp}"
            );
        }
    }
}

#[test]
fn abstract_link_set_retires_partial_suspended_and_eof_lifecycles() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    let original: Value =
        serde_json::from_slice(&fs::read(src.join("abstract-model.json")).unwrap()).unwrap();
    let work: Value =
        serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
    for express in [false, true] {
        let mut model = original.clone();
        let mut w = work.clone();
        if express {
            w["generators"][0]["priority"] = json!(7);
            w["generators"][0]["frame"]["tag"]["pcp"] = json!(7);
        }
        // Delay receive beyond release, exposing the separate receiver lifetime.
        model["configured_media"] = json!({"seed":"7","physical_links":[{"id":"delayed_rx","a":"Net.b.tx","b":"Net.s1.tx_b","phy_mode":"1000base-t1","duplex":"full","a_phy":{"role":"master","tx_latency_ps":"0","rx_latency_ps":"500000"},"b_phy":{"role":"slave","tx_latency_ps":"0","rx_latency_ps":"0"}}]});
        let baseline = execute_variant_until(&model, &w, "abstract-reset-baseline", 20_000_000);
        let transfers = records(&baseline, "dir.preemption.transfer");
        let row = transfers
            .iter()
            .find(|r| r["from_port"] == "Net.s1.tx_b")
            .unwrap();
        let sof = row["sof_ps"].as_str().unwrap().parse::<u64>().unwrap();
        let eof = row["eof_ps"].as_str().unwrap().parse::<u64>().unwrap();
        let mut cuts = vec![sof + 100_000, eof - 1, eof, eof + 1, eof + 100_000];
        for schema in ["dir.preemption.fragment", "dir.preemption.gap"] {
            for r in records(&baseline, schema)
                .into_iter()
                .filter(|r| r["port"] == "Net.s1.tx_b")
            {
                let start = r["start_ps"].as_str().unwrap().parse::<u64>().unwrap();
                let end = r["planned_end_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                cuts.extend([start, start + (end - start) / 2, end]);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for cut in cuts {
            let mut reset = w.clone();
            // Second frame survives queued/down and is reconsidered on up.
            reset["generators"][0]["times_ps"] = json!(["4000000", (cut + 50_000).to_string()]);
            reset["controls"] = json!([{"id":"down","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":false},{"id":"repeated-down","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":false},{"id":"up","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":true}]);
            let v = execute_variant_until(&model, &reset, "abstract-reset-data", 20_000_000);
            assert_eq!(
                v["simulation"]["termination"], "time_limit",
                "express={express} cut={cut}: {:?}",
                v["simulation"]["diagnostics"]
            );
            let rows = records(&v, "dir.preemption.transfer");
            let old = rows
                .iter()
                .find(|r| r["transfer_id"] == row["transfer_id"])
                .unwrap();
            if cut > sof && cut < eof + 500_000 {
                assert_eq!(
                    old["status"], "dropped",
                    "express={express} cut={cut}: {old}"
                );
                assert_eq!(old["reset_ps"], cut.to_string());
                assert!(old["arrival_ps"].is_null());
                if cut < eof {
                    assert!(old["eof_ps"].is_null());
                }
                if express && cut == sof + 100_000 {
                    assert_eq!(old["partial_mac_bytes"], "4");
                    assert_eq!(old["occupied_ps"], "100000");
                }
            }
            assert!(
                rows.iter().any(|r| r["frame_id"] == "unicast:1"
                    && r["from_port"] == "Net.s1.tx_b"
                    && r["arrival_ps"].is_string()),
                "queued recovery cut={cut}: {rows:?}"
            );
        }
    }
}

#[test]
fn abstract_down_after_actual_rx_preserves_delivery_and_retires_only_ipg() {
    use serde_json::json;
    let src = repo().join("examples/network-composite/wire-serializers");
    let model: Value =
        serde_json::from_slice(&fs::read(src.join("abstract-model.json")).unwrap()).unwrap();
    let mut work: Value =
        serde_json::from_slice(&fs::read(src.join("workload.json")).unwrap()).unwrap();
    work["generators"][0]["priority"] = json!(7);
    work["generators"][0]["frame"]["tag"]["pcp"] = json!(7);
    let baseline = execute_variant_until(&model, &work, "abstract-ipg-reference", 20_000_000);
    let rows = records(&baseline, "dir.preemption.transfer");
    let old = rows
        .iter()
        .find(|r| r["from_port"] == "Net.s1.tx_b")
        .unwrap();
    let arrival = old["arrival_ps"].as_str().unwrap().parse::<u64>().unwrap();
    let release = old["release_ps"].as_str().unwrap().parse::<u64>().unwrap();
    let cut = arrival + 1;
    assert!(cut < release);
    work["controls"] = json!([{"id":"down","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":false},{"id":"up","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":true}]);
    let v = execute_variant_until(&model, &work, "abstract-ipg-reset", 20_000_000);
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let rows = records(&v, "dir.preemption.transfer");
    let actual = rows
        .iter()
        .find(|r| r["transfer_id"] == old["transfer_id"])
        .unwrap();
    assert_eq!(actual["status"], "completed");
    assert_eq!(actual["arrival_ps"], old["arrival_ps"]);
    assert_eq!(actual["eof_ps"], old["eof_ps"]);
    assert_eq!(actual["completed_mac_bytes"], actual["mac_bytes"]);
    assert_eq!(actual["partial_mac_bytes"], "0");
    assert_eq!(actual["reset_ps"], cut.to_string());
    assert!(actual["release_ps"].is_null());
    assert!(
        records(&v, "dir.preemption.delivery")
            .iter()
            .any(|r| r["frame_id"] == actual["frame_id"] && r["completed_count"] == "1")
    );
}
