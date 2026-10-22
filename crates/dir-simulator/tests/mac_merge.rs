//! Actual common-network/FES execution for the explicitly selected Clause99 adapter.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scenario {
    root: PathBuf,
}
impl Scenario {
    fn pair(change: impl FnOnce(&mut Value, &mut Value)) -> Self {
        let root = std::env::var_os("DIR_MAC_MERGE_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "dir-mac-merge-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mac-merge");
        fs::create_dir_all(root.join("models/ethtsn")).unwrap();
        fs::copy(
            source.join("../ethernet/tsn/models/ethtsn/Main.ned"),
            root.join("models/ethtsn/Main.ned"),
        )
        .unwrap();
        let mut model: Value =
            serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
        let mut work: Value =
            serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
        change(&mut model, &mut work);
        fs::write(
            root.join("model.json"),
            serde_json::to_vec_pretty(&model).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("workload.json"),
            serde_json::to_vec_pretty(&work).unwrap(),
        )
        .unwrap();
        fs::write(root.join("model.ini"),format!("[General]\nnetwork = ethtsn.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",root.join("models").display())).unwrap();
        Self { root }
    }
    fn wire(&self, bitrate: Option<&str>, delay: Option<&str>) {
        let path = self.root.join("models/ethtsn/Main.ned");
        let mut text = fs::read_to_string(&path).unwrap();
        if let Some(rate) = bitrate {
            text = text.replace(
                "double bitrate @unit(bps) = default(1Gbps);",
                &format!("double bitrate @unit(bps) = default({rate});"),
            );
        }
        if let Some(delay) = delay {
            text = text.replace(
                "double delay @unit(s) = default(0ps);",
                &format!("double delay @unit(s) = default({delay});"),
            );
        }
        fs::write(path, text).unwrap();
    }
    fn execute(&self) -> Value {
        let p = prepare(&self.root.join("model.ini")).unwrap();
        run(p, &self.root.join("out")).unwrap();
        serde_json::from_slice(&fs::read(self.root.join("out/results.json")).unwrap()).unwrap()
    }
}
impl Drop for Scenario {
    fn drop(&mut self) {
        if std::env::var_os("DIR_MAC_MERGE_EVIDENCE").is_none() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
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
#[test]
fn actual_lldp_verify_respond_precedes_demand_driven_three_fragments() {
    let scenario = Scenario::pair(|_, _| {});
    let v = scenario.execute();
    let packets = records(&v, "dir.mac_merge.packet");
    assert!(packets.iter().any(|p| p["kind"] == "Verify"));
    assert!(packets.iter().any(|p| p["kind"] == "Respond"));
    let active = records(&v, "dir.mac_merge.verification");
    assert_eq!(
        active
            .iter()
            .filter(|p| p["p_active"] == true)
            .map(|p| p["port"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2
    );
    for p in active.iter().filter(|p| p["p_active"] == true) {
        assert!(
            p["peer_evidence"]
                .as_str()
                .unwrap()
                .starts_with("mac-merge-packet:")
        );
    }
    let mut fragments: Vec<_> = packets
        .iter()
        .filter(|p| {
            p["port"] == "Net.a.tx"
                && p["kind"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("Start") || s.starts_with("Continuation"))
        })
        .collect();
    fragments.sort_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap());
    assert_eq!(fragments.len(), 3);
    for (p, (sof, eof, release)) in fragments.iter().zip([
        (4_000_000, 4_576_000, 4_672_000),
        (5_344_000, 5_920_000, 6_016_000),
        (6_688_000, 7_264_000, 7_360_000),
    ]) {
        assert_eq!(p["sof_ps"], sof.to_string());
        assert_eq!(p["eof_ps"], eof.to_string());
        assert_eq!(p["release_ps"], release.to_string());
        assert_eq!(p["wire_hex"].as_str().unwrap().len(), 144);
    }
    let receptions = records(&v, "ethernet.dynamic.reception");
    assert_eq!(
        receptions
            .iter()
            .filter(|r| r["status"] == "received")
            .count(),
        3
    );
    let slow = records(&v, "ethernet.dynamic.frame")
        .into_iter()
        .find(|r| r["flow_id"] == "slow")
        .unwrap()["frame_id"]
        .clone();
    // Model-record IDs are outside data for frame records; match the full original MAC instead.
    let _ = slow;
    assert_eq!(
        receptions
            .iter()
            .filter(|r| r["observed_ps"] == "7264000")
            .count(),
        1
    );
}
#[test]
fn legacy_peer_fallback_uses_d5_without_inventing_capability() {
    let scenario = Scenario::pair(|m, _| {
        m["mac_merge"]["outputs"].as_array_mut().unwrap().pop();
    });
    let v = scenario.execute();
    let p = records(&v, "dir.mac_merge.packet");
    assert!(p.iter().all(|p| p["kind"] == "Express"));
    assert!(p.iter().all(|p| {
        p["wire_hex"]
            .as_str()
            .unwrap()
            .starts_with("55555555555555d5")
    }));
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received")
            .count(),
        3
    );
    assert!(
        !records(&v, "dir.mac_merge.verification")
            .iter()
            .any(|p| p["p_active"] == true)
    );
}
#[test]
fn actual_fragment_loss_or_corruption_never_delivers_cached_original() {
    for (ordinal, action) in [
        (3, json!({"kind":"Corrupt","octet":71,"xor":1})),
        (5, json!({"kind":"Drop"})),
        (7, json!({"kind":"Drop"})),
    ] {
        let scenario = Scenario::pair(|m, _| {
            m["mac_merge"]["packet_faults"] =
                json!([{"port":"Net.a.tx","packet_ordinal":ordinal,"action":action}])
        });
        let v = scenario.execute();
        assert_eq!(
            records(&v, "ethernet.dynamic.reception")
                .iter()
                .filter(|r| r["status"] == "received")
                .count(),
            2,
            "ordinal {ordinal}"
        );
        if ordinal == 7 {
            let dropped = records(&v, "dir.mac_merge.packet")
                .into_iter()
                .find(|p| p["fault_outcome"] == "dropped")
                .unwrap();
            assert!(dropped["arrival_ps"].is_null());
            assert!(dropped["eof_ps"].is_string());
        }

        if ordinal > 3 {
            assert!(
                records(&v, "dir.mac_merge.assembly")
                    .iter()
                    .any(|r| r["incomplete"] == true)
            );
        }
    }
}

fn pause(session: &mut dir_simulator::RunSession, controller: &dir_simulator::Controller) {
    controller
        .submit(dir_simulator::ControlCommand::Pause)
        .unwrap();
    assert_eq!(
        session.poll_control().unwrap(),
        dir_simulator::RunState::Paused
    );
}
fn rehash_state(path: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let mut v: Value = serde_json::from_slice(&fs::read(path.join("state.json")).unwrap()).unwrap();
    change(&mut v);
    let b = serde_json::to_vec(&v).unwrap();
    fs::write(path.join("state.json"), &b).unwrap();
    let mut m: Value =
        serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
    let e = m["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["path"] == "state.json")
        .unwrap();
    e["bytes"] = json!(b.len().to_string());
    e["sha256"] = json!(format!("{:x}", Sha256::digest(&b)));
    fs::write(path.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
}
#[test]
fn real_merge_checkpoints_resume_all_physical_and_verification_boundaries() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let scenario = Scenario::pair(|_, _| {});
    for boundary in [1, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44] {
        let (mut s, controller) = RunSession::new(
            prepare(&scenario.root.join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut completed = false;
        for _ in 0..boundary {
            if s.advance_one().unwrap() == RunState::Completed {
                completed = true;
                break;
            }
        }
        if completed {
            break;
        }
        pause(&mut s, &controller);
        let bundle = scenario.root.join(format!("cp-{boundary}"));
        let ack = s.checkpoint(&bundle, CheckpointLimits::default()).unwrap();
        assert_eq!(
            ack.status,
            ControlStatus::Applied,
            "boundary {boundary}: {:?}",
            ack.error
        );
        let (mut restored, token) =
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                .unwrap_or_else(|e| panic!("boundary {boundary}: {e:?}"));
        s.resume(s.pause_token().unwrap()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        let original = scenario.root.join(format!("original-{boundary}"));
        let other = scenario.root.join(format!("restored-{boundary}"));
        assert_eq!(s.export(&original).unwrap().status, ControlStatus::Applied);
        assert_eq!(
            restored.export(&other).unwrap().status,
            ControlStatus::Applied
        );
        let a: Value =
            serde_json::from_slice(&fs::read(original.join("results.json")).unwrap()).unwrap();
        let b: Value =
            serde_json::from_slice(&fs::read(other.join("results.json")).unwrap()).unwrap();
        for field in ["model_records", "records", "summary"] {
            assert_eq!(
                a["simulation"][field], b["simulation"][field],
                "boundary {boundary}, {field}"
            );
        }
    }
}
#[test]
fn rehashed_merge_checkpoint_mutations_cannot_forge_wire_or_activation() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let scenario = Scenario::pair(|_, _| {});
    let (mut s, c) = RunSession::new(
        prepare(&scenario.root.join("model.ini")).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
        if s.observe()
            .unwrap()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "dir.mac_merge.assembly" && r.data["incomplete"] == true)
        {
            break;
        }
    }
    pause(&mut s, &c);
    for case in [
        "counter",
        "crc",
        "owner",
        "event",
        "activation",
        "fault",
        "rx_frame",
        "rx_frag",
        "rx_bytes_crc",
    ] {
        let bundle = scenario.root.join(format!("mutant-{case}"));
        assert_eq!(
            s.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        rehash_state(&bundle, |v| {
            let n = &mut v["network_backend"]["state"];
            match case {
                "rx_frame" => {
                    n["mac_merge"]["ports"][1]["rx"]["assembly"]["frame_count"] = json!(1)
                }
                "rx_frag" => {
                    n["mac_merge"]["ports"][1]["rx"]["assembly"]["next_frag_count"] = json!(1)
                }
                "rx_bytes_crc" => {
                    let a = &mut n["mac_merge"]["ports"][1]["rx"]["assembly"];
                    a["bytes"][20] = json!(255);
                    let bytes = a["bytes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|b| b.as_u64().unwrap() as u8)
                        .collect::<Vec<_>>();
                    let mut crc = !0u32;
                    for b in bytes {
                        crc ^= u32::from(b);
                        for _ in 0..8 {
                            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
                        }
                    }
                    a["crc_state"] = json!(!crc);
                }
                "counter" => n["mac_merge"]["ports"][0]["tx"]["tx_frame"] = json!(4),
                "crc" => n["mac_merge"]["ports"][1]["rx"]["assembly"]["crc_state"] = json!(0),
                "owner" => n["mac_merge"]["ports"][0]["busy"] = Value::Null,
                "event" => n["core"]["due"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|d| d["kind"]["MacMerge"].get("Release").is_none()),
                "activation" => {
                    n["mac_merge"]["ports"][0]["verify"]["peer"] = Value::Null;
                }
                "fault" => {
                    let p = n["mac_merge"]["packets"]
                        .as_object_mut()
                        .unwrap()
                        .values_mut()
                        .next()
                        .unwrap();
                    p["received"][20] = json!(255);
                }
                _ => unreachable!(),
            }
        });
        assert!(
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err(),
            "accepted {case}"
        );
    }
}

#[test]
fn link_reset_aborts_retained_frames_and_invalidates_old_propagation() {
    let scenario = Scenario::pair(|_, w| {
        w["controls"] = json!([{"id":"down","at_ps":"4800000","kind":"link_set","link":"a-b","up":false},{"id":"up","at_ps":"6000000","kind":"link_set","link":"a-b","up":true}]);
        w["generators"][0]["times_ps"] = json!(["4000000", "12000000"]);
    });
    let ini = scenario.root.join("model.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "15000000ps");
    fs::write(&ini, text).unwrap();
    scenario.wire(None, Some("400000ps"));
    let v = scenario.execute();
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received")
            .count(),
        1
    );
    let states = records(&v, "dir.mac_merge.verification");
    assert!(
        states
            .iter()
            .any(|r| r["epoch"] == "2" && r["p_active"] == true)
    );
    assert!(
        states
            .iter()
            .any(|r| r["cause"] == "link_epoch_reset" && r["p_active"] == false)
    );
    assert!(
        records(&v, "ethernet.dynamic.transfer")
            .iter()
            .any(|r| r["drop_reason"] == "mac_merge_link_epoch_reset")
    );
}

#[test]
fn real_peer_capability_controls_all_add_frag_sizes_and_both_link_rates() {
    for add in 0u64..4 {
        let minimum = 64 * (1 + add) - 4;
        let scenario = Scenario::pair(|m, w| {
            m["mac_merge"]["outputs"][0]["advertised_add_frag_size"] = json!(0);
            m["mac_merge"]["outputs"][1]["advertised_add_frag_size"] = json!(add);
            w["generators"][0]["frame"]["data"] = json!("00".repeat((minimum + 64 - 18) as usize));
        });
        let v = scenario.execute();
        let packets = records(&v, "dir.mac_merge.packet");
        let first = packets
            .iter()
            .find(|p| {
                p["port"] == "Net.a.tx"
                    && p["kind"].as_str().is_some_and(|s| s.starts_with("Start"))
            })
            .unwrap();
        assert_eq!(first["source_end"], minimum.to_string());
        assert_eq!(
            first["eof_ps"],
            (4_000_000 + (8 + minimum + 4) * 8000).to_string()
        );
        assert_eq!(first["crc_kind"], "mcrc");
    }
    let scenario = Scenario::pair(|_, w| {
        w["generators"][0]["times_ps"] = json!(["40000000"]);
        w["generators"][1]["times_ps"] = json!(["43000000", "55000000"]);
    });
    let ini = scenario.root.join("model.ini");
    let s = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "100000000ps");
    fs::write(&ini, s).unwrap();
    scenario.wire(Some("100Mbps"), None);
    let v = scenario.execute();
    let p = records(&v, "dir.mac_merge.packet");
    let last = p
        .iter()
        .find(|p| p["port"] == "Net.a.tx" && p["frag_count"] == "1")
        .unwrap();
    assert_eq!(last["eof_ps"], "72640000");
    assert_eq!(last["release_ps"], "73600000");
}

#[test]
fn real_missing_responses_fail_after_three_completed_verify_transmissions() {
    let scenario = Scenario::pair(|m, _| {
        for p in m["mac_merge"]["outputs"].as_array_mut().unwrap() {
            p["verify_time_ms"] = json!(1);
        }
        m["mac_merge"]["packet_faults"]=json!((2..=4).map(|ordinal|json!({"port":"Net.b.tx","packet_ordinal":ordinal,"action":{"kind":"Drop"}})).collect::<Vec<_>>());
    });
    let ini = scenario.root.join("model.ini");
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace("10000000ps", "4000000000ps"),
    )
    .unwrap();
    let v = scenario.execute();
    let states = records(&v, "dir.mac_merge.verification");
    assert!(states.iter().any(|p| p["port"] == "Net.a.tx"
        && p["phase"] == "VerifyFail"
        && p["attempts"] == "3"
        && p["p_active"] == false));
    assert_eq!(
        records(&v, "dir.mac_merge.packet")
            .iter()
            .filter(|p| p["port"] == "Net.a.tx" && p["kind"] == "Verify" && p["eof_ps"].is_string())
            .count(),
        3
    );
}

#[test]
fn exact_deadline_real_response_wins_probe_and_commit_and_restores_between_them() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let scenario = Scenario::pair(|m, _| {
        for p in m["mac_merge"]["outputs"].as_array_mut().unwrap() {
            p["verify_time_ms"] = json!(1);
        }
    });
    let ini = scenario.root.join("model.ini");
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace("10000000ps", "2000000000ps"),
    )
    .unwrap();
    scenario.wire(None, Some("499712000ps"));
    let v = scenario.execute();
    let states = records(&v, "dir.mac_merge.verification");
    assert_eq!(
        states
            .iter()
            .filter(|p| p["p_active"] == true && p["attempts"] == "1")
            .map(|p| p["port"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2
    );
    let (mut s, c) = RunSession::new(
        prepare(&ini).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let due = 1_500_864_000;
    loop {
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
        if s.snapshot().common.last_event_time_ps.unwrap_or(0) >= due {
            break;
        }
    }
    pause(&mut s, &c);
    let bundle = scenario.root.join("timer-stage");
    assert_eq!(
        s.checkpoint(&bundle, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
    assert!(
        saved["network_backend"]["state"]["mac_merge"]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["verify_timer_commit"] == true)
    );
    let (mut restored, token) =
        restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    rehash_state(&bundle, |v| {
        for p in v["network_backend"]["state"]["mac_merge"]["ports"]
            .as_array_mut()
            .unwrap()
        {
            p["verify_timer_commit"] = json!(false);
        }
    });
    assert!(restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err());
}

#[test]
fn actual_rtag_branch_fragments_reassemble_before_distributed_recovery() {
    let scenario = Scenario::pair(|_, _| {});
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/frer-rtag/distributed");
    let mut model: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    let mut work: Value =
        serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
    let pair: Value =
        serde_json::from_slice(&fs::read(scenario.root.join("model.json")).unwrap()).unwrap();
    let outputs=model["base"]["config"]["tsn"]["outputs"].as_array().unwrap().iter().enumerate().map(|(i,o)|json!({"port":o["port"],"source_mac":format!("02:00:00:00:01:{:02x}",i+1),"express_classes":[7],"preemptable_classes":[0,1,2,3,4,5,6],"advertised_add_frag_size":0,"verify_time_ms":10,"admin_enabled":true})).collect::<Vec<_>>();
    model["schema_name"] = json!("dir.ethernet_network_composite.config");
    model["preemption"] = Value::Null;
    model["clocks"] = Value::Null;
    model["mac_merge"] = pair["mac_merge"].clone();
    model["mac_merge"]["outputs"] = json!(outputs);
    model["mac_merge"]["capability_adapter"]["interval_ms"] = json!(1);
    work["generators"][0]["times_ps"] = json!(["997400000"]);
    work["generators"][0]["frame"]["data"] = json!("a5".repeat(166));
    fs::write(
        scenario.root.join("model.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    fs::write(
        scenario.root.join("workload.json"),
        serde_json::to_vec_pretty(&work).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(scenario.root.join("models/ethdistributed")).unwrap();
    fs::copy(
        source.join("models/ethdistributed/Main.ned"),
        scenario.root.join("models/ethdistributed/Main.ned"),
    )
    .unwrap();
    let ini = scenario.root.join("model.ini");
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace("ethtsn.Net", "ethdistributed.Net")
            .replace("10000000ps", "1020000000ps"),
    )
    .unwrap();
    let v = scenario.execute();
    let packets = records(&v, "dir.mac_merge.packet");
    assert!(packets.iter().any(|p| p["port"] == "Net.r.tx_a"
        && p["crc_kind"] == "mcrc"
        && p["wire_hex"].as_str().unwrap().contains("f1c10000")));
    let recovery = records(&v, "dir.redundancy.recovery");
    assert_eq!(
        recovery
            .iter()
            .filter(|r| r["continuation_created"] == true)
            .count(),
        1
    );
    assert_eq!(recovery.len(), 2);
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received" && r["ingress"] == "Net.d.rx")
            .count(),
        1
    );
    // The same real-byte path also binds the explicitly selected VectorRecovery
    // owner, with a distinct combined checkpoint codec and a pending timeout.
    model["replication"]["streams"][0]["recovery_algorithm"] = json!("vector_8021cb_2017");
    fs::write(
        scenario.root.join("model.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let (mut session, controller) = RunSession::new(
        prepare(&ini).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
        if session
            .observe()
            .unwrap()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| {
                r.schema.name == "dir.redundancy.recovery" && r.data["continuation_created"] == true
            })
        {
            break;
        }
    }
    pause(&mut session, &controller);
    let bundle = scenario.root.join("combined-vector-mac-checkpoint");
    assert_eq!(
        session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved["network_backend"]["state"]["version"], 13);
    let (mut resumed, token) =
        restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).unwrap();
    resumed.resume(token).unwrap();
    assert_eq!(resumed.run_until_yield().unwrap(), RunState::Completed);
    let result = scenario.root.join("combined-vector-mac-restored");
    assert_eq!(
        resumed.export(&result).unwrap().status,
        ControlStatus::Applied
    );
    let result: Value =
        serde_json::from_slice(&fs::read(result.join("results.json")).unwrap()).unwrap();
    assert_eq!(
        records(&result, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received" && r["ingress"] == "Net.d.rx")
            .count(),
        1
    );
}

#[test]
fn lost_real_lldp_refresh_expires_peer_evidence_and_clears_active_state() {
    let scenario = Scenario::pair(|m, _| {
        m["mac_merge"]["capability_adapter"]["interval_ms"] = json!(999);
        m["mac_merge"]["capability_adapter"]["ttl_seconds"] = json!(1);
        m["mac_merge"]["packet_faults"] =
            json!([{"port":"Net.b.tx","packet_ordinal":3,"action":{"kind":"Drop"}}]);
    });
    let ini = scenario.root.join("model.ini");
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace("10000000ps", "1001000000000ps"),
    )
    .unwrap();
    let v = scenario.execute();
    assert!(
        records(&v, "dir.mac_merge.verification")
            .iter()
            .any(|p| p["port"] == "Net.a.tx"
                && p["cause"] == "capability_expired"
                && p["p_active"] == false
                && p["p_enable"] == false
                && p["peer_evidence"].is_null())
    );
}

fn independent_crc(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
#[test]
fn actual_wire_lldp_shutdown_and_missing_capability_refresh_withdraw_matching_peer() {
    let make = |faults: Value| {
        let s = Scenario::pair(|m, w| {
            m["mac_merge"]["capability_adapter"]["interval_ms"] = json!(1);
            m["mac_merge"]["packet_faults"] = faults;
            w["generators"] = json!([]);
        });
        let ini = s.root.join("model.ini");
        let text = fs::read_to_string(&ini)
            .unwrap()
            .replace("10000000ps", "1010000000ps");
        fs::write(ini, text).unwrap();
        s
    };
    let baseline = make(json!([]));
    let baseline = baseline.execute();
    let packet = records(&baseline, "dir.mac_merge.packet")
        .into_iter()
        .find(|p| p["port"] == "Net.a.tx" && p["packet_ordinal"] == "3")
        .unwrap();
    let hex = packet["wire_hex"].as_str().unwrap();
    let original: Vec<u8> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect();
    for (case, ttl, other, active) in [
        ("shutdown", 0, false, false),
        ("replacement", 3, false, false),
        ("other_msap", 0, true, true),
    ] {
        let mut changed = original.clone();
        let mut at = 8 + 14;
        for _ in 0..2 {
            let n = usize::from(u16::from_be_bytes([changed[at], changed[at + 1]]) & 511);
            if other && changed[at] >> 1 == 2 {
                changed[at + 2 + n - 1] ^= 1;
            }
            at += 2 + n;
        }
        changed[at + 2..at + 4].copy_from_slice(&(ttl as u16).to_be_bytes());
        at += 4;
        // End-of-LLDPDU replaces the optional capability; the rest is MAC padding.
        let end = changed.len() - 4;
        changed[at..end].fill(0);
        let crc = independent_crc(&changed[8..end]).to_le_bytes();
        changed[end..].copy_from_slice(&crc);
        let faults=original.iter().zip(&changed).enumerate().filter(|(_, (a,b))|a!=b).map(|(i,(a,b))|json!({"port":"Net.a.tx","packet_ordinal":3,"action":{"kind":"Corrupt","octet":i,"xor":a^b}})).collect::<Vec<_>>();
        let scenario = make(json!(faults));
        let v = scenario.execute();
        let peer = &v["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                r["schema_name"] == "dir.mac_merge.verification"
                    && r["data"]["port"] == "Net.b.tx"
                    && r["data"].get("p_active").is_some()
            })
            .max_by_key(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .unwrap()["data"];
        assert_eq!(peer["p_active"], active, "{case}");
        if !active {
            assert_eq!(peer["p_enable"], false);
            assert!(peer["peer_evidence"].is_null());
        }
        // Save after the physical replacement has arrived, then resume without stale capability.
        use dir_simulator::{
            CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
            restore_checkpoint,
        };
        let (mut s, c) = RunSession::new(
            prepare(&scenario.root.join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            if s.advance_one().unwrap() == RunState::Completed {
                break;
            }
            if s.observe().unwrap().common.last_event_time_ps.unwrap_or(0) >= 1_000_576_000 {
                break;
            }
        }
        pause(&mut s, &c);
        let bundle = scenario.root.join(format!("cp-{case}"));
        assert_eq!(
            s.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut restored, token) =
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    }
}

#[test]
fn actual_cbs_piecewise_credit_excludes_added_preemption_overhead_and_resumes_negative() {
    let scenario = Scenario::pair(|m, _| {
        m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"1","lo_credit_bits":"1"}]);
    });
    let v = scenario.execute();
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    for (at, bits) in [
        (4_544_000, -408),
        (4_576_000, -400),
        (4_672_000, -376),
        (5_344_000, -208),
        (5_408_000, -192),
        (5_888_000, -552),
        (5_920_000, -544),
        (6_016_000, -520),
        (6_688_000, -352),
        (6_752_000, -336),
        (7_264_000, -720),
        (7_360_000, -792),
    ] {
        let expected = (bits as i128) * 1_000_000_000_000i128;
        let at_text = at.to_string();
        assert!(
            rows.iter().any(|r| {
                if r["schema_name"] != "ethernet.tsn.credit"
                    || r["time_ps"].as_str() != Some(at_text.as_str())
                    || r["data"]["port"] != "Net.a.tx"
                    || r["data"]["priority"] != "0"
                {
                    return false;
                }
                let n = r["data"]["magnitude"]
                    .as_str()
                    .unwrap()
                    .parse::<i128>()
                    .unwrap();
                let n = if r["data"]["sign"] == "negative" {
                    -n
                } else {
                    n
                };
                n == expected
            }),
            "no analytic credit {bits} at {at}; {:?}",
            rows.iter()
                .filter(|r| r["schema_name"] == "ethernet.tsn.credit")
                .collect::<Vec<_>>()
        );
    }
    let starts = records(&v, "dir.mac_merge.packet");
    assert!(
        starts
            .iter()
            .any(|p| p["sof_ps"] == "5344000" && p["source_start"] == "60")
    );
    assert!(
        records(&v, "ethernet.tsn.credit")
            .iter()
            .any(|p| p["cause"] == "merge_configured_bound_exceeded")
    );
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|p| p["status"] == "received")
            .count(),
        3
    );
}
#[test]
fn actual_tas_advanced_hold_cuts_without_express_and_release_resumes_retained_pmac() {
    let scenario = Scenario::pair(|m, w| {
        w["generators"]
            .as_array_mut()
            .unwrap()
            .retain(|g| g["id"] == "slow");
        m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"hold","base_time_ps":"0","cycle_time_ps":"20000000","entries":[
            {"duration_ps":"7076000","open_priorities":[0,7]},
            {"duration_ps":"2924000","open_priorities":[7],"operation":"set_and_hold_mac"},
            {"duration_ps":"10000000","open_priorities":[0,7],"operation":"set_and_release_mac"}
        ]});
    });
    // HOLD targets7.076us, issued4.300us; conservative HRT2776bits. RELEASE targets10us.
    let ini = scenario.root.join("model.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "14000000ps");
    fs::write(ini, text).unwrap();
    let v = scenario.execute();
    let packets = records(&v, "dir.mac_merge.packet");
    assert!(
        packets
            .iter()
            .any(|p| p["kind"] == "Start { frame_count: 0 }" && p["planned_eof_ps"] == "4576000")
    );
    assert!(
        packets
            .iter()
            .any(|p| p["source_start"] == "60" && p["sof_ps"] == "9904000")
    );
    assert!(
        records(&v, "dir.mac_merge.verification")
            .iter()
            .any(|r| r["cause"] == "tas_mm_ctl"
                && r["request"] == "hold"
                && r["applied"] == true
                && r["advance_ps"] == "2776000")
    );
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|p| p["status"] == "received")
            .count(),
        1
    );
}

#[test]
fn actual_merge_cbs_tas_checkpoints_preserve_credit_segments_hold_and_pending_controls() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let scenario = Scenario::pair(|m, _| {
        m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
        m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"hold","base_time_ps":"0","cycle_time_ps":"20000000","entries":[
            {"duration_ps":"7076000","open_priorities":[0,7]},
            {"duration_ps":"2924000","open_priorities":[7],"operation":"set_and_hold_mac"},
            {"duration_ps":"10000000","open_priorities":[0,7],"operation":"set_and_release_mac"}
        ]});
    });
    let ini = scenario.root.join("model.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "16000000ps");
    fs::write(&ini, text).unwrap();
    for at in [
        4_300_000, 4_544_000, 4_576_000, 4_672_000, 5_248_000, 7_076_000, 9_904_000, 9_968_000,
        10_000_000,
    ] {
        let (mut s, c) = RunSession::new(
            prepare(&ini).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            if s.observe().unwrap().common.last_event_time_ps.unwrap_or(0) >= at {
                break;
            }
        }
        pause(&mut s, &c);
        let bundle = scenario.root.join(format!("shaper-cp-{at}"));
        assert_eq!(
            s.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut restored, token) =
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                .unwrap_or_else(|e| panic!("{at}: {e:?}"));
        s.resume(s.pause_token().unwrap()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        let a = scenario.root.join(format!("shaper-original-{at}"));
        let b = scenario.root.join(format!("shaper-restored-{at}"));
        s.export(&a).unwrap();
        restored.export(&b).unwrap();
        let a: Value = serde_json::from_slice(&fs::read(a.join("results.json")).unwrap()).unwrap();
        let b: Value = serde_json::from_slice(&fs::read(b.join("results.json")).unwrap()).unwrap();
        for field in ["model_records", "records", "summary"] {
            assert_eq!(
                a["simulation"][field], b["simulation"][field],
                "{at} {field}"
            );
        }
    }
}

#[test]
fn actual_merge_cbs_retains_positive_credit_and_only_new_frames_recheck_eligibility() {
    for (slope, positive) in [(900_000_000, true), (250_000_000, false)] {
        let scenario = Scenario::pair(|m, w| {
            m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":slope.to_string(),"hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
            if !positive {
                w["generators"][0]["times_ps"] = json!(["4000000", "7400000"]);
            }
        });
        let ini = scenario.root.join("model.ini");
        let text = fs::read_to_string(&ini)
            .unwrap()
            .replace("10000000ps", "15000000ps");
        fs::write(ini, text).unwrap();
        let v = scenario.execute();
        let rows = v["simulation"]["model_records"].as_array().unwrap();
        if positive {
            assert!(
                rows.iter()
                    .any(|r| r["schema_name"] == "ethernet.tsn.credit"
                        && r["time_ps"] == "5344000"
                        && r["data"]["priority"] == "0"
                        && r["data"]["sign"] == "positive"
                        && r["data"]["magnitude"] == "665600000000000")
            );
        } else {
            assert!(
                records(&v, "dir.mac_merge.packet")
                    .iter()
                    .any(|p| p["kind"] == "Start { frame_count: 1 }" && p["sof_ps"] == "10528000")
            );
        }
    }
}
#[test]
fn actual_merge_waiting_express_accumulates_credit_until_actual_wire_start() {
    let scenario = Scenario::pair(|m, _| {
        m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
    });
    let v = scenario.execute();
    assert!(
        v["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["schema_name"] == "ethernet.tsn.credit"
                && r["time_ps"] == "4672000"
                && r["data"]["priority"] == "7"
                && r["data"]["sign"] == "positive"
                && r["data"]["magnitude"] == "93000000000000"
                && r["data"]["slope_bps"] == "-750000000")
    );
}

#[test]
fn actual_tas_gate_close_freezes_idle_credit_but_does_not_replace_hold_or_stop_retained_transmit() {
    let scenario = Scenario::pair(|m, _| {
        m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
        m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"close","base_time_ps":"0","cycle_time_ps":"20000000","entries":[
            {"duration_ps":"6000000","open_priorities":[0,7]},
            {"duration_ps":"1500000","open_priorities":[7]},
            {"duration_ps":"12500000","open_priorities":[0,7]}
        ]});
    });
    let v = scenario.execute();
    assert!(
        records(&v, "dir.mac_merge.packet")
            .iter()
            .any(|p| p["source_start"] == "120" && p["sof_ps"] == "6688000")
    );
    assert!(
        records(&v, "dir.mac_merge.verification")
            .iter()
            .any(|r| r["cause"] == "transmission_overrun")
    );
    assert!(
        v["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["schema_name"] == "ethernet.tsn.credit"
                && r["time_ps"] == "7360000"
                && r["data"]["priority"] == "0"
                && r["data"]["sign"] == "negative"
                && r["data"]["magnitude"] == "980000000000000")
    );
}

#[test]
fn actual_merge_tas_keeps_full_frame_guard_and_unsupported_operations_are_explicit() {
    let scenario = Scenario::pair(|m, w| {
        w["generators"]
            .as_array_mut()
            .unwrap()
            .retain(|g| g["id"] == "slow");
        w["generators"][0]["times_ps"] = json!(["6000000"]);
        m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"guard","base_time_ps":"0","cycle_time_ps":"20000000","entries":[
            {"duration_ps":"7000000","open_priorities":[0,7]},
            {"duration_ps":"3000000","open_priorities":[7]},
            {"duration_ps":"10000000","open_priorities":[0,7]}
        ]});
    });
    let ini = scenario.root.join("model.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "14000000ps");
    fs::write(&ini, text).unwrap();
    let v = scenario.execute();
    assert!(
        records(&v, "dir.mac_merge.packet")
            .iter()
            .any(|p| p["kind"] == "Start { frame_count: 0 }" && p["sof_ps"] == "10000000")
    );
    let path = scenario.root.join("model.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    m["mac_merge"] = Value::Null;
    m["base"]["config"]["tsn"]["outputs"][0]["tas"]["entries"][1]["operation"] =
        json!("set_and_hold_mac");
    fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(prepare(&ini).unwrap_err().message.contains("HOLD/RELEASE"));
}

#[test]
fn lost_final_rx_physical_fragment_proof_restores_without_delivering_cached_mac() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let scenario = Scenario::pair(|m, _| {
        m["mac_merge"]["packet_faults"] =
            json!([{"port":"Net.a.tx","packet_ordinal":7,"action":{"kind":"Drop"}}]);
    });
    let (mut s, c) = RunSession::new(
        prepare(&scenario.root.join("model.ini")).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
        if s.observe().unwrap().common.last_event_time_ps.unwrap_or(0) >= 7_360_000 {
            break;
        }
    }
    pause(&mut s, &c);
    let bundle = scenario.root.join("lost-final-proof");
    assert_eq!(
        s.checkpoint(&bundle, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let saved: Value =
        serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
    assert_eq!(
        saved["network_backend"]["state"]["mac_merge"]["ports"][1]["rx"]["assembly"]["fragments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (mut restored, token) =
        restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    let out = scenario.root.join("lost-restored");
    restored.export(&out).unwrap();
    let v: Value = serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received")
            .count(),
        2
    );
}

#[test]
fn cbs_historical_journal_rejects_rehashed_credit_residual_and_causal_mutants() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    for gates in [false, true] {
        let scenario = Scenario::pair(|m, w| {
            m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
            w["generators"][0]["times_ps"] = json!(["4000000", "7400000"]);
            if gates {
                m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"close","base_time_ps":"0","cycle_time_ps":"20000000","entries":[{"duration_ps":"6000000","open_priorities":[0,7]},{"duration_ps":"1500000","open_priorities":[7]},{"duration_ps":"12500000","open_priorities":[0,7]}]});
            }
        });
        let (mut s, c) = RunSession::new(
            prepare(&scenario.root.join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            if s.observe().unwrap().common.last_event_time_ps.unwrap_or(0) >= 7_360_000 {
                break;
            }
        }
        pause(&mut s, &c);
        for case in if gates {
            vec!["gate_freeze"]
        } else {
            vec![
                "credit",
                "residual",
                "omitted_overhead",
                "shifted_overhead",
                "continuation_as_original",
                "missing_backlog",
                "physical_owner",
                "same_time_order",
            ]
        } {
            let bundle = scenario.root.join(format!("history-{case}"));
            assert_eq!(
                s.checkpoint(&bundle, CheckpointLimits::default())
                    .unwrap()
                    .status,
                ControlStatus::Applied
            );
            // The active packet ledger is pruned; all completed packet proofs
            // needed to establish credit must still exist in causal history.
            let saved: Value =
                serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
            assert!(
                saved["network_backend"]["state"]["mac_merge"]["packets"]
                    .as_object()
                    .unwrap()
                    .is_empty()
            );
            let (mut valid, token) =
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .unwrap();
            valid.resume(token).unwrap();
            assert_eq!(valid.run_until_yield().unwrap(), RunState::Completed);
            rehash_state(&bundle, |v| {
                let n = &mut v["network_backend"]["state"];
                if case == "credit" || case == "residual" {
                    let credit = &mut n["tsn"]["ports"][0]["cbs"][0]["credit"];
                    let before = credit[1].as_str().unwrap().parse::<u128>().unwrap();
                    credit[1] = json!(
                        (before
                            - if case == "credit" {
                                1_000_000_000_000
                            } else {
                                1
                            })
                        .to_string()
                    );
                    return;
                }
                let journal = &mut n["mac_merge"]["ports"][0]["accounting_journal"];
                let entries = journal["entries"].as_array_mut().unwrap();
                match case {
                    "omitted_overhead" => {
                        let at = entries.iter().position(|e| e["at"] == 4_544_000).unwrap();
                        entries.remove(at);
                        for (j, e) in entries.iter_mut().enumerate() {
                            e["order"] = json!(j);
                        }
                        let count = entries
                            .iter()
                            .map(|e| 1 + e["causes"].as_array().unwrap().len())
                            .sum::<usize>();
                        journal["events"] = json!(count);
                    }
                    "shifted_overhead" => {
                        entries.iter_mut().find(|e| e["at"] == 4_544_000).unwrap()["at"] =
                            json!(4_544_001)
                    }
                    "continuation_as_original" => {
                        let packet = entries
                            .iter_mut()
                            .flat_map(|e| e["causes"].as_array_mut().unwrap())
                            .find_map(|c| {
                                c.get_mut("Start")
                                    .filter(|p| p["plan"]["start_offset"] == 60)
                            })
                            .unwrap();
                        packet["plan"]["start_offset"] = json!(0);
                    }
                    "missing_backlog" => {
                        let e = entries
                            .iter_mut()
                            .find(|e| {
                                e["causes"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|c| c["Start"]["origin"].get("Express").is_some())
                            })
                            .unwrap();
                        e["causes"]
                            .as_array_mut()
                            .unwrap()
                            .insert(0, json!({"Unselect":{"transfer":0,"express":false}}));
                        journal["events"] = json!(journal["events"].as_u64().unwrap() + 1);
                    }
                    "physical_owner" => {
                        let packet = entries
                            .iter_mut()
                            .flat_map(|e| e["causes"].as_array_mut().unwrap())
                            .find_map(|c| c.get_mut("Start"))
                            .unwrap();
                        packet["direction"] = json!(1);
                    }
                    "same_time_order" => {
                        let e = entries
                            .iter_mut()
                            .find(|e| {
                                e["causes"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|c| c.get("Select").is_some())
                                    && e["causes"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .any(|c| c.get("Start").is_some())
                            })
                            .unwrap();
                        e["causes"].as_array_mut().unwrap().reverse();
                    }
                    "gate_freeze" => {
                        entries
                            .iter_mut()
                            .find(|e| {
                                e["causes"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|c| c == "GateEdge")
                                    && e["at"] == 6_000_000
                            })
                            .unwrap()["at"] = json!(6_000_001)
                    }
                    _ => unreachable!(),
                }
            });
            assert!(
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .is_err(),
                "accepted historical {case}"
            );
        }
    }
}

fn held_capability_lifecycle(faults: Value) -> Scenario {
    let scenario = Scenario::pair(|m, w| {
        m["mac_merge"]["capability_adapter"]["interval_ms"] = json!(1);
        m["mac_merge"]["packet_faults"] = faults;
        m["base"]["config"]["tsn"]["outputs"][1]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"1000000","lo_credit_bits":"1000000"}]);
        m["base"]["config"]["tsn"]["outputs"][1]["tas"] = json!({"id":"lifecycle","base_time_ps":"0","cycle_time_ps":"2000000000","entries":[
            {"duration_ps":"900000000","open_priorities":[0,7]},
            {"duration_ps":"300000000","open_priorities":[7],"operation":"set_and_hold_mac"},
            {"duration_ps":"800000000","open_priorities":[0,7],"operation":"set_and_release_mac"}
        ]});
        for g in w["generators"].as_array_mut().unwrap() {
            g["node"] = json!("Net.b");
            g["frame"]["dst_mac"] = json!("02:00:00:00:00:01");
        }
        w["generators"][0]["times_ps"] = json!(["897000000"]);
        w["generators"][1]["times_ps"] = json!(["1100000000", "1300000000"]);
    });
    let ini = scenario.root.join("model.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "3300000000ps");
    fs::write(ini, text).unwrap();
    scenario
}
#[test]
fn actual_hold_latches_across_shutdown_inactive_release_and_reverification_until_real_release() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let baseline = held_capability_lifecycle(json!([]));
    let normal = baseline.execute();
    let packet = records(&normal, "dir.mac_merge.packet")
        .into_iter()
        .find(|p| p["port"] == "Net.a.tx" && p["packet_ordinal"] == "3")
        .unwrap();
    let raw = packet["wire_hex"].as_str().unwrap();
    let original: Vec<u8> = raw
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect();
    let mut shutdown = original.clone();
    let mut at = 22;
    for _ in 0..2 {
        at += 2 + usize::from(u16::from_be_bytes([shutdown[at], shutdown[at + 1]]) & 511);
    }
    shutdown[at + 2..at + 4].fill(0);
    at += 4;
    let end = shutdown.len() - 4;
    shutdown[at..end].fill(0);
    let crc = independent_crc(&shutdown[8..end]).to_le_bytes();
    shutdown[end..].copy_from_slice(&crc);
    let faults=original.iter().zip(shutdown).enumerate().filter(|(_, (a,b))|**a!=*b).map(|(i,(a,b))|json!({"port":"Net.a.tx","packet_ordinal":3,"action":{"kind":"Corrupt","octet":i,"xor":a^b}})).collect::<Vec<_>>();
    let scenario = held_capability_lifecycle(json!(faults));
    let v = scenario.execute();
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let state = |at: u64, cause: &str| {
        let at_text = at.to_string();
        rows.iter()
            .find(|r| {
                r["schema_name"] == "dir.mac_merge.verification"
                    && r["time_ps"].as_str() == Some(at_text.as_str())
                    && r["data"]["port"] == "Net.b.tx"
                    && r["data"]["cause"] == cause
            })
            .map(|r| &r["data"])
            .unwrap()
    };
    let held = state(897_224_000, "tas_mm_ctl");
    assert_eq!(held["hold"], true);
    assert_eq!(held["p_active"], true);
    let withdrawn = state(1_000_576_000, "received_lldp_capability");
    assert_eq!(withdrawn["hold"], true);
    assert_eq!(withdrawn["p_active"], false);
    assert_eq!(withdrawn["last_mm_ctl"], "hold");
    let ignored = state(1_199_904_000, "tas_mm_ctl");
    assert_eq!(ignored["request"], "release");
    assert_eq!(ignored["applied"], false);
    assert_eq!(ignored["hold"], true);
    assert_eq!(ignored["p_active"], false);
    let verified = state(2_001_824_000, "actual_control_receive");
    assert_eq!(verified["p_active"], true);
    assert_eq!(verified["hold"], true);
    let released = state(3_199_904_000, "tas_mm_ctl");
    assert_eq!(released["applied"], true);
    assert_eq!(released["hold"], false);
    assert_eq!(released["last_mm_ctl"], "release");
    let packets = records(&v, "dir.mac_merge.packet");
    assert!(packets.iter().any(|p| p["port"] == "Net.b.tx"
        && p["source_start"] == "0"
        && p["planned_eof_ps"] == "897576000"));
    assert!(packets.iter().any(|p| p["port"] == "Net.b.tx"
        && p["source_start"] == "60"
        && p["sof_ps"] == "3199904000"));
    assert!(
        packets.iter().any(|p| p["port"] == "Net.b.tx"
            && p["kind"] == "Express"
            && p["sof_ps"] == "1100000000")
    );
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received")
            .count(),
        3
    );
    for at in [
        897_224_000,
        897_576_000,
        1_000_576_000,
        1_199_904_000,
        2_000_576_000,
        2_001_824_000,
        3_199_904_000,
        3_200_960_000,
    ] {
        for extra in [0, 2] {
            let (mut s, c) = RunSession::new(
                prepare(&scenario.root.join("model.ini")).unwrap(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            loop {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
                if s.observe().unwrap().common.last_event_time_ps.unwrap_or(0) >= at {
                    break;
                }
            }
            for _ in 0..extra {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
            }
            pause(&mut s, &c);
            let bundle = scenario.root.join(format!("hold-{at}-{extra}"));
            assert_eq!(
                s.checkpoint(&bundle, CheckpointLimits::default())
                    .unwrap()
                    .status,
                ControlStatus::Applied
            );
            let (mut restored, token) =
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .unwrap_or_else(|e| panic!("HOLD {at}/{extra}: {e:?}"));
            restored.resume(token).unwrap();
            assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
            let out = scenario.root.join(format!("hold-restored-{at}-{extra}"));
            restored.export(&out).unwrap();
            let resumed: Value =
                serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
            for field in ["model_records", "records", "summary"] {
                assert_eq!(
                    v["simulation"][field], resumed["simulation"][field],
                    "HOLD {at}/{extra} {field}"
                );
            }
            if at == 1_199_904_000 {
                rehash_state(&bundle, |v| {
                    v["network_backend"]["state"]["mac_merge"]["ports"][1]["tx"]["hold"] =
                        json!(false)
                });
                assert!(
                    restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                        .is_err()
                );
            }
        }
    }
}

#[test]
fn accounting_history_limits_fail_explicitly_instead_of_pruning_causal_proofs() {
    for key in ["max_accounting_events", "max_accounting_octets"] {
        let scenario = Scenario::pair(|m, _| {
            m["mac_merge"]["limits"][key] = json!(1);
            m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
        });
        let v = scenario.execute();
        assert_eq!(v["simulation"]["termination"], "execution_failed");
        let diagnostics = fs::read_to_string(scenario.root.join("out/diagnostics.jsonl")).unwrap();
        assert!(
            diagnostics.contains("configured history limit exceeded"),
            "{key}: {diagnostics}"
        );
        assert_eq!(
            records(&v, "ethernet.dynamic.reception")
                .iter()
                .filter(|r| r["status"] == "received")
                .count(),
            0
        );
    }
}

#[test]
fn historical_credit_replay_retains_aborted_packet_proofs_across_actual_link_epochs() {
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    let scenario = Scenario::pair(|m, w| {
        m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
        w["controls"] = json!([{"id":"down","at_ps":"4800000","kind":"link_set","link":"a-b","up":false},{"id":"up","at_ps":"6000000","kind":"link_set","link":"a-b","up":true}]);
        w["generators"][0]["times_ps"] = json!(["4000000", "12000000"]);
    });
    scenario.wire(None, Some("400000ps"));
    let ini = scenario.root.join("model.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace("10000000ps", "15000000ps");
    fs::write(&ini, text).unwrap();
    let baseline = scenario.execute();
    for at in [4_800_000, 6_000_000, 12_000_000] {
        let (mut s, c) = RunSession::new(
            prepare(&ini).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            if s.observe().unwrap().common.last_event_time_ps.unwrap_or(0) >= at {
                break;
            }
        }
        pause(&mut s, &c);
        let bundle = scenario.root.join(format!("history-reset-{at}"));
        assert_eq!(
            s.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut restored, token) =
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                .unwrap_or_else(|e| panic!("reset {at}: {e:?}"));
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        let out = scenario.root.join(format!("reset-restored-{at}"));
        restored.export(&out).unwrap();
        let v: Value =
            serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
        for field in ["model_records", "records", "summary"] {
            assert_eq!(
                baseline["simulation"][field], v["simulation"][field],
                "reset {at} {field}"
            );
        }
    }
}

mod sampled_binding {
    use super::*;
    use dir_simulator::runtime::ethernet::phy::{
        Q30, SampleGrid,
        channel::{ChannelConfig, Path as ChannelPath, TransmitterConfig},
        ieee::t1_link,
        medium::*,
        receiver::*,
    };

    fn sampled_merge_medium() -> MediumConfig {
        let grid = SampleGrid {
            symbol_rate_hz: 750_000_000,
            samples_per_ui: 16,
        };
        let endpoints = (0..2)
            .map(|i| EndpointConfig {
                id: format!("p{i}"),
                transmitter: TransmitterConfig {
                    grid,
                    peak_uv: 1_000_000,
                    transition_ui_ppm: 0,
                    clock_ppm: 0,
                    filter_taps_q30: vec![Q30],
                },
                receiver: ReceiverConfig {
                    grid,
                    peak_uv: 1_000_000,
                    frontend_taps_q30: vec![Q30],
                    adc_bits: 16,
                    adc_full_scale_uv: 4_000_000,
                    ffe_taps: 1,
                    echo_taps: 0,
                    adaptation_q30: 0,
                    minimum_training_symbols: 32,
                    lock_error_ppm: 100_000,
                    clock_ppm: 0,
                    max_tracking_ppm: 0,
                    max_no_transition_symbols: 4096,
                    loss_error_ppm: 500_000,
                },
            })
            .collect::<Vec<_>>();
        MediumConfig {
            execution_contract: t1_link::CONTRACT.into(),
            standard_edition: "IEEE Std 802.3-2022".into(),
            physical_mode: PhysicalMode::Ieee1000BaseT1PointToPoint2022,
            plca: None,
            repeater: None,
            gigabit_t1: Some(t1_link::Config {
                stations: (0..2)
                    .map(|i| t1_link::StationConfig {
                        endpoint: format!("p{i}"),
                        autoneg: false,
                        preferred_master: i == 0,
                        force_role: true,
                        nonce_low4: i as u8 + 1,
                        an_random_seed: i as u64 + 1,
                        training_seed: if i == 0 { 0x1a2b_3c4d5 } else { 0x1234_56789 },
                        data_seed: if i == 0 { 0x2255 } else { 0x315a },
                    })
                    .collect(),
                ideal_reference_echo_cancellation: true,
            }),
            gigabit_raw_merge: true,
            channel: ChannelConfig {
                grid,
                endpoints: endpoints.iter().map(|e| e.id.clone()).collect(),
                paths: vec![
                    vec![
                        None,
                        Some(ChannelPath {
                            delay_samples: 0,
                            taps_q30: vec![Q30],
                        }),
                    ],
                    vec![
                        Some(ChannelPath {
                            delay_samples: 0,
                            taps_q30: vec![Q30],
                        }),
                        None,
                    ],
                ],
                noise_rms_uv: vec![0; 2],
                noise_seed: 1,
                clip_uv: 4_000_000,
            },
            endpoints,
            half_duplex: false,
            carrier_threshold_uv: 100_000,
            carrier_hold_samples: 32,
            collision_threshold_uv: 200_000,
            ifg_bits: 96,
            jam_bits: 32,
            slot_bits: 512,
            retry_limit: 16,
            seed: 11,
            maximum_mac_bytes: 1518,
            maximum_samples: 100_000_000,
            trace_samples: false,
        }
    }
    #[test]
    #[ignore = "runs actual continuous sampled Clause97 startup and Clause99 fragment/reassembly"]
    fn actual_sampled_merge_consumes_fragments_and_decodes_real_ingress() {
        let scenario = Scenario::pair(|model, workload| {
            model["phy_attachments"] = json!([{
                "origin_ps":0,
                "lookahead_samples":65536,
                "medium":sampled_merge_medium(),
                "bindings":[],
                "physical_ports":[
                    {"endpoint":"p0","tx_port":"Net.a.tx","rx_port":"Net.a.rx"},
                    {"endpoint":"p1","tx_port":"Net.b.tx","rx_port":"Net.b.rx"}
                ]
            }]);
            workload["generators"][0]["times_ps"] = json!(["6000000000"]);
            workload["generators"][1]["times_ps"] = json!(["6000300000", "6001500000"]);
        });
        let ini = fs::read_to_string(scenario.root.join("model.ini"))
            .unwrap()
            .replace("10000000ps", "6012000000ps");
        fs::write(scenario.root.join("model.ini"), ini).unwrap();
        let v = scenario.execute();
        assert_eq!(v["simulation"]["termination"], "time_limit");
        let packets = records(&v, "dir.mac_merge.packet");
        assert!(packets.iter().any(|p| p["kind"] == "Verify"));
        assert!(packets.iter().any(|p| p["kind"] == "Respond"));
        assert!(packets.iter().all(|p| p["timing_plane"] == "actual_xmii"));
        let mut slow: Vec<_> = packets
            .iter()
            .filter(|p| {
                p["port"] == "Net.a.tx"
                    && p["kind"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("Start") || s.starts_with("Continuation"))
            })
            .collect();
        slow.sort_by_key(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap());
        assert!(
            slow.len() >= 2,
            "actual demand must interrupt after startup verification: {slow:?}"
        );
        assert!(
            slow.iter()
                .all(|p| p["received_hex"] == p["wire_hex"] && p["complete"] == true)
        );
        let mut source_end = 0;
        for p in slow {
            assert_eq!(p["source_start"], source_end.to_string());
            source_end = p["source_end"].as_str().unwrap().parse::<usize>().unwrap();
            let eof = p["eof_ps"].as_str().unwrap().parse::<u64>().unwrap();
            let release = p["release_ps"].as_str().unwrap().parse::<u64>().unwrap();
            let arrival = p["arrival_ps"].as_str().unwrap().parse::<u64>().unwrap();
            assert_eq!(release - eof, 96_000);
            assert!(
                arrival > release,
                "actual FEC/decode arrival must not be ideal EOF"
            );
            assert_eq!(p["raw_phy"]["coded_end"], true);
            assert!(p["planned_arrival_ps"].is_null());
        }
        let physical = records(&v, "dir.network.phy");
        assert!(physical.iter().any(|p| p["kind"] == "RawMPacketIngress"));
        assert!(physical.iter().any(|p| p["kind"] == "MdiCodedEnd"));
    }
}

#[test]
fn managed_qcc_window_defers_retained_background_after_real_lldp_interruption() {
    let scenario = Scenario::pair(|m, w| {
        m["mac_merge"]["capability_adapter"]["interval_ms"] = json!(1);
        m["mac_merge"]["limits"]["max_rx_mac_octets"] = json!(16384);
        let base = &mut m["base"]["config"]["base"]["config"];
        for port in base["ports"].as_array_mut().unwrap() {
            port["vlans"][0]["tagged"] = json!(true);
        }
        m["qcc"] = json!({
            "architecture":"fully_centralized","transport":"dir.cuc-cnc.local.v1",
            "standard_edition":"IEEE802.1Q-2022","cycle_ps":"10000000000",
            "management_latency_ps":"1000000","clock_error_budget_ps":"0",
            "search_budget":10000,"max_streams":1,
            "requirements":[{"id":"managed","generator":"express","listeners":["Net.b"],
                "interval":{"numerator":1,"denominator":100},"max_frames_per_interval":1,
                "max_frame_size":46,"max_latency_ps":"9000000000",
                "earliest_transmit_offset_ps":"1024000000","latest_transmit_offset_ps":"9000000000",
                "jitter_ps":"0","rank":1,"num_seamless_trees":1,
                "paths":[{"listener":"Net.b","ports":["Net.a.tx"]}]}],
            "operations":[{"operation":"prepare","id":"install","at_ps":"100000000",
                "expected_topology_epoch":"2","add":["managed"],"remove":[],
                "activate_at_ps":"10000000000","timeout_at_ps":"1000000000"}]
        });
        w["generators"][0]["times_ps"] = json!(["10900000000"]);
        w["generators"][0]["frame"]["data"] = json!("a5".repeat(1500));
        w["generators"][0]["frame"]["tag"] = json!({"vid":10,"pcp":0,"dei":0});
        w["generators"][1]["times_ps"] = json!(["11024000000"]);
        w["generators"][1]["frame"]["tag"] = json!({"vid":10,"pcp":7,"dei":0});
    });
    scenario.wire(Some("100Mbps"), None);
    let ini = scenario.root.join("model.ini");
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace("10000000ps", "15000000000ps"),
    )
    .unwrap();
    let v = scenario.execute();
    let qcc = records(&v, "dir.network.qcc");
    assert!(qcc.iter().any(|r| r["kind"] == "installed"), "{qcc:?}");
    assert!(qcc.iter().any(|r| r["kind"] == "source_sof"
        && r["stream"] == "managed"
        && r["within_certificate"] == true));
    let packets = records(&v, "dir.mac_merge.packet");
    let slow: Vec<_> = packets
        .iter()
        .filter(|p| {
            p["port"] == "Net.a.tx"
                && p["transfer"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("slow:"))
        })
        .collect();
    assert!(slow.len() >= 2, "{slow:?}");
    let first = slow.iter().find(|p| p["source_start"] == "0").unwrap();
    assert_eq!(first["sof_ps"], "10900000000");
    let last = slow
        .iter()
        .filter(|p| p["source_start"] != "0")
        .map(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
        .min()
        .unwrap();
    assert!(
        last > 11_024_000_000,
        "retained background resumed across managed closure: {last}"
    );
    assert_eq!(
        records(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["status"] == "received")
            .count(),
        2
    );
}

mod sampled_checkpoint {
    use super::*;
    use dir_simulator::{
        CheckpointLimits, ControlStatus, RunOptions, RunSession, RunState, registry::Registry,
        restore_checkpoint,
    };
    use std::process::Command;

    fn child(root: &Path, mode: &str, bundle: &str) {
        let stderr_path = root.join(format!("{mode}-{bundle}.stderr.log"));
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sampled_checkpoint::process_worker",
                "--nocapture",
            ])
            .env("DIR_RAW_MERGE_CP_WORKER", mode)
            .env("DIR_RAW_MERGE_CP_ROOT", root)
            .env("DIR_RAW_MERGE_CP_BUNDLE", bundle)
            .stdout(std::process::Stdio::from(
                fs::File::create(root.join(format!("{mode}-{bundle}.stdout.log"))).unwrap(),
            ))
            .stderr(std::process::Stdio::from(
                fs::File::create(&stderr_path).unwrap(),
            ))
            .status()
            .unwrap();
        assert!(
            status.success(),
            "{mode}/{bundle}: {}",
            fs::read_to_string(stderr_path).unwrap()
        );
    }
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target)
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    #[test]
    fn process_worker() {
        let Ok(mode) = std::env::var("DIR_RAW_MERGE_CP_WORKER") else {
            return;
        };
        let root = PathBuf::from(std::env::var_os("DIR_RAW_MERGE_CP_ROOT").unwrap());
        let bundle = root.join(std::env::var_os("DIR_RAW_MERGE_CP_BUNDLE").unwrap());
        if mode == "save" {
            let ini = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/gigabit-merge/model.ini");
            let (mut session, controller) = RunSession::new(
                prepare(&ini).unwrap(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut last_progress = 0;
            loop {
                assert_eq!(session.advance_one().unwrap(), RunState::Running);
                let observed = session.observe().unwrap();
                if observed.common.last_event_time_ps.unwrap_or(0) / 1_000_000_000 > last_progress {
                    last_progress = observed.common.last_event_time_ps.unwrap_or(0) / 1_000_000_000;
                    eprintln!(
                        "actual raw checkpoint startup at {}ps",
                        observed.common.last_event_time_ps.unwrap_or(0)
                    );
                }
                if observed
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .values()
                    .any(|r| {
                        r.schema.name == "dir.mac_merge.packet"
                            && r.data["transfer"]
                                .as_str()
                                .is_some_and(|t| t.starts_with("slow:"))
                            && !r.data["raw_phy"]["tx_delimiter_sample"].is_null()
                            && r.data["eof_ps"].is_null()
                    })
                {
                    break;
                }
            }
            pause(&mut session, &controller);
            let ack = session
                .checkpoint(&bundle, CheckpointLimits::default())
                .unwrap();
            assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
            session.resume(session.pause_token().unwrap()).unwrap();
            assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
            fs::write(
                root.join("expected-records.json"),
                serde_json::to_vec(
                    &session
                        .observe()
                        .unwrap()
                        .registered
                        .as_ref()
                        .unwrap()
                        .model_records,
                )
                .unwrap(),
            )
            .unwrap();
        } else if mode == "restore" {
            let (mut session, token) =
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .unwrap();
            session.resume(token).unwrap();
            assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
            let expected: Value =
                serde_json::from_slice(&fs::read(root.join("expected-records.json")).unwrap())
                    .unwrap();
            assert_eq!(
                json!(
                    session
                        .observe()
                        .unwrap()
                        .registered
                        .as_ref()
                        .unwrap()
                        .model_records
                ),
                expected
            );
        } else {
            let failure =
                match restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                {
                    Ok(_) => panic!("accepted co-rehashed raw producer mutant"),
                    Err(e) => e,
                };
            assert!(
                failure
                    .message
                    .contains("exact frozen-input committed prefix")
                    || failure.message.contains("causal prefix"),
                "{failure:?}"
            );
            eprintln!("{failure:?}");
        }
    }
    #[test]
    #[ignore = "actual sampled startup and three fresh-process exact-prefix replays"]
    fn actual_raw_prefix_fresh_process_and_rehashed_generation_watermark_mutants() {
        let root = std::env::var_os("DIR_RAW_MERGE_CP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "raw-merge-cp-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        eprintln!("raw Merge causal evidence: {}", root.display());
        child(&root, "save", "bundle");
        child(&root, "restore", "bundle");
        for kind in ["generation", "watermark"] {
            let name = format!("mutant-{kind}");
            let target = root.join(&name);
            copy_tree(&root.join("bundle"), &target);
            rehash_state(&target, |state| {
                let network = &mut state["network_backend"]["state"];
                let bytes: Vec<u8> = serde_json::from_value(network["phy"][0].clone()).unwrap();
                let mut attachment: Value = serde_json::from_slice(&bytes).unwrap();
                let bytes: Vec<u8> = serde_json::from_value(attachment["medium"].clone()).unwrap();
                let mut medium: Value = serde_json::from_slice(&bytes).unwrap();
                let port = medium["state"]["ports"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|p| !p["raw_active"].is_null())
                    .unwrap();
                let packet = port["raw_active"]["request"]["packet_id"].as_u64().unwrap();
                if kind == "generation" {
                    let generation = port["raw_active"]["request"]["generation"]
                        .as_u64()
                        .unwrap()
                        + 1;
                    port["raw_active"]["request"]["generation"] = json!(generation);
                    for span in port["raw_spans"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .filter(|p| p["packet_id"] == packet)
                    {
                        span["generation"] = json!(generation);
                    }
                    network["mac_merge"]["packets"][packet.to_string()]["raw"]["generation"] =
                        json!(generation);
                } else {
                    let consumed = port["raw_active"]["consumed_octets"].as_u64().unwrap();
                    port["raw_active"]["consumed_octets"] = json!(consumed + 1);
                }
                attachment["medium"] = json!(serde_json::to_vec(&medium).unwrap());
                network["phy"][0] = json!(serde_json::to_vec(&attachment).unwrap());
            });
            child(&root, "reject", &name);
        }
    }
}
