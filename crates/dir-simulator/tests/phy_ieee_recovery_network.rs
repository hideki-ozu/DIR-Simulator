//! Production FES checkpoint/replay for new sampled PLCA recovery state.
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    registry::Registry, restore_checkpoint,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn root() -> PathBuf {
    PathBuf::from(std::env::var_os("DIR_PHY_RECOVERY_ROOT").unwrap())
}
fn value(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn canonical(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("run_id");
    v.as_object_mut().unwrap().remove("control");
    for f in ["started_at_utc", "finished_at_utc"] {
        v["metadata"].as_object_mut().unwrap().remove(f);
    }
    v
}
fn fixture(root: &Path) {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/t1s-multidrop");
    fs::create_dir_all(root).unwrap();
    let ini = fs::read_to_string(source.join("plca-pending.ini"))
        .unwrap()
        .replace(
            "ned-path = \"models\"",
            &format!("ned-path = \"{}\"", source.join("models").display()),
        );
    fs::write(root.join("model.ini"), ini).unwrap();
    for file in ["plca-pending-model.json", "plca-pending-workload.json"] {
        fs::copy(source.join(file), root.join(file)).unwrap();
    }
}
fn child(root: &Path, mode: &str) {
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "recovery_process_worker", "--nocapture"])
        .env("DIR_PHY_RECOVERY_ROOT", root)
        .env("DIR_PHY_RECOVERY_MODE", mode)
        .output()
        .unwrap();
    fs::write(
        root.join(format!("{mode}-process.log")),
        [result.stdout.clone(), result.stderr.clone()].concat(),
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{mode}: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn recovery_process_worker() {
    let Ok(mode) = std::env::var("DIR_PHY_RECOVERY_MODE") else {
        return;
    };
    let root = root();
    if mode.starts_with("save") {
        let (mut session, controller) = RunSession::new(
            prepare(&root.join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
            let snapshot = serde_json::to_value(
                &session
                    .observe()
                    .unwrap()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records,
            )
            .unwrap();
            let target = match mode.as_str() {
                "save-acquisition" => "\"command\":\"beacon\"",
                "save-jam" => "plca_mac_jam_start",
                "save-frame" => "plca_data:Transmit",
                _ => "plca_data:DelayPending",
            };
            let reached = if mode == "save-drain" {
                ["plca_control:Burst", "plca_control:Abort"]
                    .iter()
                    .all(|command| {
                        snapshot.as_object().unwrap().values().any(|row| {
                            row["data"]["endpoint"] == "p2" && row["data"]["command"] == *command
                        })
                    })
            } else {
                snapshot.to_string().contains(target)
            };
            if reached {
                break;
            }
            if session.snapshot().common.last_event_time_ps.unwrap_or(0) >= 1_000_000_000 {
                fs::write(
                    root.join("failed-prefix-records.json"),
                    snapshot.to_string(),
                )
                .unwrap();
                panic!("production fixture never reached pending recovery");
            }
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let ack = session
            .checkpoint(&root.join("bundle"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        if mode == "save-drain" {
            let state = value(root.join("bundle/state.json"));
            let attachment: Value = serde_json::from_slice(
                &serde_json::from_value::<Vec<u8>>(
                    state["network_backend"]["state"]["phy"][0].clone(),
                )
                .unwrap(),
            )
            .unwrap();
            let medium: Value = serde_json::from_slice(
                &serde_json::from_value::<Vec<u8>>(attachment["medium"].clone()).unwrap(),
            )
            .unwrap();
            let port = &medium["state"]["ports"][2];
            assert_eq!(port["plca_commit_active"], false);
            assert!(port["program"].is_null());
            assert_eq!(port["plca_control_chips"].as_array().unwrap().len(), 9);
        }
        session.resume(session.pause_token().unwrap()).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        let ack = session.export(&root.join("original")).unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    } else if mode == "restore" {
        let (mut session, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        session.resume(token).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        let ack = session.export(&root.join("restored")).unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    } else {
        assert!(
            restore_checkpoint(
                &root.join("bundle"),
                Registry::default(),
                CheckpointLimits::default()
            )
            .is_err(),
            "accepted {mode}"
        );
    }
}
fn rewrite(root: &Path, field: &str) {
    let bundle = root.join("bundle");
    let mut state = value(bundle.join("state.json"));
    let phy = &mut state["network_backend"]["state"]["phy"][0];
    let mut attachment: Value =
        serde_json::from_slice(&serde_json::from_value::<Vec<u8>>(phy.clone()).unwrap()).unwrap();
    let mut medium: Value = serde_json::from_slice(
        &serde_json::from_value::<Vec<u8>>(attachment["medium"].clone()).unwrap(),
    )
    .unwrap();
    if field == "commit_residual_polarity" {
        let chip = &mut medium["state"]["ports"][2]["plca_control_chips"][0];
        assert_eq!(chip.as_i64().unwrap().abs(), 1);
        *chip = json!(-chip.as_i64().unwrap());
    } else if field == "mii_start_endpoint" {
        let program = &mut medium["state"]["ports"][2]["program"];
        program["started"] = json!(program["started"].as_u64().unwrap() + 1);
    } else if field == "data_delay" {
        let node = &mut medium["state"]["ports"][2]["plca_node"];
        node["hold_samples"] = json!(node["hold_samples"].as_u64().unwrap() + 1);
    } else if field == "signal_status" {
        let node = &mut medium["state"]["ports"][2]["plca_node"];
        assert_eq!(node["signal_error"], true);
        node["signal_error"] = json!(false);
    } else if field == "mac_jam_flag" || field == "mac_jam_countdown" {
        let program = &mut medium["state"]["ports"][2]["program"];
        assert!(program["mac_jam_samples_left"].as_u64().unwrap() > 0);
        if field == "mac_jam_flag" {
            program
                .as_object_mut()
                .unwrap()
                .remove("mac_jam_samples_left");
        } else {
            program["mac_jam_samples_left"] =
                json!(program["mac_jam_samples_left"].as_u64().unwrap() - 1);
        }
    } else if field == "mac_input_endpoint" {
        let program = &mut medium["state"]["ports"][2]["program"];
        program["mac_input_end_sample"] =
            json!(program["mac_input_end_sample"].as_u64().unwrap() + 1);
    } else if field == "plca_enable" {
        medium["state"]["ports"][0]["plca_node"]["enabled"] = json!(false);
    } else if field == "pending_countdown" {
        let node = &mut medium["state"]["ports"][2]["plca_node"];
        assert_eq!(node["data"], "delay_pending");
        node["data_timer_samples"] = json!(node["data_timer_samples"].as_u64().unwrap() - 1);
    } else {
        let receiver = medium["state"]["ports"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .map(|p| &mut p["rx"])
            .find(|rx| rx["control_acquisition"]["reference"].is_object())
            .unwrap();
        match field {
            "reference_index" => {
                let r = &mut receiver["control_acquisition"]["reference"];
                r["sample_indices"][0] = json!(r["sample_indices"][0].as_u64().unwrap() + 1);
            }
            "phase" => receiver["control_acquisition"]["reference"]["phase"] = json!(31),
            "polarity" => {
                let r = &mut receiver["control_acquisition"]["reference"];
                r["initial_level"] = json!(-r["initial_level"].as_i64().unwrap());
            }
            "recent_adc" => {
                let sample = &mut receiver["control_acquisition"]["recent"][0][2];
                sample["rx"] = json!(sample["rx"].as_i64().unwrap() + 1000);
            }
            "count" => receiver["trained"] = json!(receiver["trained"].as_u64().unwrap() + 1),
            "epoch" => receiver["control_acquisition"]["epoch"] = json!(0),
            "lock" => receiver["acquisition"] = json!("Locked"),
            _ => unreachable!(),
        }
    }
    attachment["medium"] = json!(serde_json::to_vec(&medium).unwrap());
    *phy = json!(serde_json::to_vec(&attachment).unwrap());
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(bundle.join("state.json"), &bytes).unwrap();
    let mut manifest = value(bundle.join("manifest.json"));
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|v| v["path"] == "state.json")
        .unwrap();
    entry["bytes"] = json!(bytes.len().to_string());
    entry["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(bundle.join("manifest.json"), manifest.to_string()).unwrap();
}
#[test]
fn real_pending_recovery_fresh_process_continuation_and_rehashed_new_state_mutants() {
    let base = std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "phy-recovery-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for phase in ["acquisition", "jam", "pending", "frame", "drain"] {
        let root = root.join(phase);
        fixture(&root);
        if phase == "drain" {
            let path = root.join("plca-pending-model.json");
            let mut model = value(&path);
            model["phy_attachments"][0]["medium"]["plca"]["max_burst_count"] = json!(1);
            fs::write(path, model.to_string()).unwrap();
        }
        child(
            &root,
            match phase {
                "acquisition" => "save-acquisition",
                "jam" => "save-jam",
                "frame" => "save-frame",
                "drain" => "save-drain",
                _ => "save",
            },
        );
        child(&root, "restore");
        let original = value(root.join("original/results.json"));
        assert_eq!(
            canonical(original.clone()),
            canonical(value(root.join("restored/results.json")))
        );
        let rows = original["simulation"]["model_records"].as_array().unwrap();
        assert!(rows.iter().any(
            |r| r["schema_name"] == "dir.network.phy" && r["data"]["kind"] == "decoded_ingress"
        ));
        assert!(
            rows.iter()
                .any(|r| r["schema_name"] == "ethernet.dynamic.reception"
                    && r["data"]["status"] == "received")
        );
        let state = fs::read(root.join("bundle/state.json")).unwrap();
        let manifest = fs::read(root.join("bundle/manifest.json")).unwrap();
        let fields: &[&str] = match phase {
            "acquisition" => &[
                "reference_index",
                "recent_adc",
                "phase",
                "polarity",
                "count",
                "epoch",
                "lock",
            ],
            "jam" => &["mac_jam_flag", "mac_jam_countdown", "signal_status"],
            "frame" => &["mac_input_endpoint", "data_delay", "mii_start_endpoint"],
            "drain" => &["commit_residual_polarity"],
            _ => &["pending_countdown", "plca_enable"],
        };
        for field in fields {
            fs::write(root.join("bundle/state.json"), &state).unwrap();
            fs::write(root.join("bundle/manifest.json"), &manifest).unwrap();
            rewrite(&root, field);
            child(&root, field);
        }
    }
    if std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
#[ignore = "release gate: actual named Clause99 startup then active administrative link cut"]
fn active_administrative_cut_retires_actual_mac_without_eof_or_payload_reception() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = repo.join("examples/network-composite/gigabit-merge");
    let base = std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "active-admin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let ini = fs::read_to_string(source.join("model.ini"))
        .unwrap()
        .replace(
            "ned-path = \"models\"",
            &format!("ned-path = \"{}\"", source.join("models").display()),
        )
        .replace("6012000000ps", "6005000000ps");
    fs::write(root.join("model.ini"), ini).unwrap();
    let model = value(source.join("model.json"));
    fs::write(root.join("model.json"), model.to_string()).unwrap();
    let mut work = value(source.join("workload.json"));
    work["generators"].as_array_mut().unwrap().truncate(1);
    work["generators"][0]["frame"]["data"] = json!("5a".repeat(1400));
    work["controls"] = json!([{"id":"active-down","at_ps":"6000500000","kind":"link_set","link":"a-b","up":false}]);
    fs::write(root.join("workload.json"), work.to_string()).unwrap();
    dir_simulator::run(prepare(&root.join("model.ini")).unwrap(), &root.join("out")).unwrap();
    let v = value(root.join("out/results.json"));
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let transfers = rows
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
        .map(|r| &r["data"])
        .collect::<Vec<_>>();
    assert!(
        transfers.iter().any(|r| r["frame_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("slow:"))
            && !r["sof_ps"].is_null()
            && r["eof_ps"].is_null()
            && r["status"] == "dropped"),
        "{transfers:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "dir.mac_merge.packet"
                && r["data"]["aborted_ps"] == "6000500000"
                && r["data"]["eof_ps"].is_null()
                && r["data"]["transfer"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("slow:"))),
        "administrative cut must abort the actual active packet at its own cut"
    );
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.control"
                && r["data"]["kind"] == "link_set"
                && r["data"]["outcome"] == "changed")
    );
    assert!(
        !rows
            .iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["status"] == "received")
    );
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "dir.network.phy" && r["data"]["kind"] == "RxAbort")
    );
    if std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
#[ignore = "release gate: real Clause98 terminal conflict retires the common NetworkState PHY claim"]
fn terminal_an_failure_retires_network_claim_without_future_ready() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/gigabit-merge");
    let base = std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "terminal-an-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let ini = fs::read_to_string(source.join("model.ini"))
        .unwrap()
        .replace(
            "ned-path = \"models\"",
            &format!("ned-path = \"{}\"", source.join("models").display()),
        )
        .replace("6012000000ps", "2000000000ps");
    fs::write(root.join("model.ini"), ini).unwrap();
    let mut model = value(source.join("model.json"));
    model["mac_merge"] = Value::Null;
    let medium = &mut model["phy_attachments"][0]["medium"];
    medium["gigabit_raw_merge"] = json!(false);
    for (i, station) in medium["gigabit_t1"]["stations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        station["autoneg"] = json!(true);
        station["preferred_master"] = json!(true);
        station["an_random_seed"] = json!(if i == 0 { 11 } else { 22 });
    }
    fs::write(root.join("model.json"), model.to_string()).unwrap();
    let mut work = value(source.join("workload.json"));
    work["generators"].as_array_mut().unwrap().truncate(1);
    work["generators"][0]["times_ps"] = json!(["0"]);
    fs::write(root.join("workload.json"), work.to_string()).unwrap();
    dir_simulator::run(prepare(&root.join("model.ini")).unwrap(), &root.join("out")).unwrap();
    let v = value(root.join("out/results.json"));
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let retired = rows
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
        .map(|r| &r["data"])
        .find(|d| d["frame_id"] == "slow:0")
        .unwrap();
    assert_eq!(retired["status"], "dropped");
    assert!(retired["sof_ps"].is_null());
    assert!(retired["eof_ps"].is_null());
    assert!(!retired["release_ps"].is_null());
    assert!(!rows.iter().any(|r| r["schema_name"] == "dir.network.phy"
        && ((r["data"]["kind"] == "TxReady" && r["data"]["ready"] == true)
            || r["data"]["kind"] == "RxFrame"
            || r["data"]["kind"] == "TxFinished")));
    assert!(
        !rows
            .iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["status"] == "received")
    );
    if std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn network_mac_admission_reaches_burst_for_during_frame_and_postframe_arrivals() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/t1s-multidrop");
    let base = std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "network-burst-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let ini = fs::read_to_string(source.join("plca-noncoordinator.ini"))
        .unwrap()
        .replace(
            "ned-path = \"models\"",
            &format!("ned-path = \"{}\"", source.join("models").display()),
        )
        .replace("130000000ps", "300000000ps");
    fs::write(root.join("model.ini"), ini).unwrap();
    let mut model = value(source.join("plca-noncoordinator-model.json"));
    model["phy_attachments"][0]["medium"]["maximum_samples"] = json!(160_000);
    fs::write(
        root.join("plca-noncoordinator-model.json"),
        model.to_string(),
    )
    .unwrap();
    let original = value(source.join("plca-noncoordinator-workload.json"));
    fs::write(
        root.join("plca-noncoordinator-workload.json"),
        original.to_string(),
    )
    .unwrap();
    dir_simulator::run(
        prepare(&root.join("model.ini")).unwrap(),
        &root.join("baseline"),
    )
    .unwrap();
    let baseline = value(root.join("baseline/results.json"));
    let baseline_rows = baseline["simulation"]["model_records"].as_array().unwrap();
    let finish = baseline_rows
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.network.phy"
                && r["data"]["kind"] == "TxFinished"
                && r["data"]["endpoint"] == "p1"
        })
        .unwrap()["time_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let first_sof = baseline_rows
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.network.phy"
                && r["data"]["kind"] == "TxStarted"
                && r["data"]["endpoint"] == "p1"
        })
        .unwrap()["time_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let queued = baseline_rows
        .iter()
        .find(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["from_port"] == "Net.s2.tx_b"
        })
        .unwrap()["data"]["queued_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let application_to_mac = queued - 40_000_000;
    for (name, available) in [
        ("during", first_sof + 8_000_000),
        ("window", finish + 10_000_000),
    ] {
        let mut work = original.clone();
        work["generators"][0]["times_ps"] =
            json!(["40000000", (available - application_to_mac).to_string()]);
        fs::write(
            root.join("plca-noncoordinator-workload.json"),
            work.to_string(),
        )
        .unwrap();
        dir_simulator::run(prepare(&root.join("model.ini")).unwrap(), &root.join(name)).unwrap();
        let output = value(root.join(name).join("results.json"));
        let rows = output["simulation"]["model_records"].as_array().unwrap();
        let mut starts = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "dir.network.phy"
                    && r["data"]["kind"] == "TxStarted"
                    && r["data"]["endpoint"] == "p1"
            })
            .map(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .collect::<Vec<_>>();
        starts.sort();
        assert_eq!(starts.len(), 2, "{name}");
        assert!(
            starts[1] >= finish + 9_600_000 && starts[1] < finish + 12_800_000,
            "{name} {starts:?} first_finish={finish}"
        );
        assert!(!rows.iter().any(|r| {
            r["schema_name"] == "dir.network.phy"
                && r["data"]["command"] == "beacon_tx"
                && r["time_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .is_ok_and(|t| starts[0] < t && t < starts[1])
        }));
        assert_eq!(
            rows.iter()
                .filter(|r| r["schema_name"] == "dir.network.phy" && r["data"]["kind"] == "RxFrame")
                .count(),
            4
        );
    }
    if std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn pending_recovery_effect_on_last_sample_commits_and_next_budget_failure_is_atomic() {
    let base = std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "plca-atomic-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let execute = |path: &Path| {
        let (mut session, _) = RunSession::new(
            prepare(&path.join("model.ini")).unwrap(),
            RunOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            session.run_until_yield().unwrap(),
            RunState::Completed | RunState::Failed
        ));
        assert_eq!(
            session.export(&path.join("out")).unwrap().status,
            ControlStatus::Applied
        );
        value(path.join("out/results.json"))
    };
    let baseline = root.join("baseline");
    fixture(&baseline);
    let expected = execute(&baseline);
    let rows = expected["simulation"]["model_records"].as_array().unwrap();
    let cut = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.network.phy"
                && r["data"]["endpoint"] == "p2"
                && r["data"]["command"] == "plca_data:DelayPending"
        })
        .unwrap()["data"]["sample_index"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let mut results = vec![];
    for (name, extra) in [("at", 0), ("after", 1)] {
        let path = root.join(name);
        fixture(&path);
        let mut model = value(path.join("plca-pending-model.json"));
        model["phy_attachments"][0]["medium"]["maximum_samples"] = json!(cut);
        fs::write(path.join("plca-pending-model.json"), model.to_string()).unwrap();
        let ini = fs::read_to_string(path.join("model.ini"))
            .unwrap()
            .replace("1500000000ps", &format!("{}ps", (cut + 1) * 2500 + extra));
        fs::write(path.join("model.ini"), ini).unwrap();
        results.push(execute(&path));
    }
    assert_eq!(results[0]["simulation"]["termination"], "time_limit");
    assert_ne!(results[1]["simulation"]["termination"], "time_limit");
    assert_eq!(
        results[0]["simulation"]["model_records"],
        results[1]["simulation"]["model_records"]
    );
    assert!(
        results[1]["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["schema_name"] == "dir.network.phy"
                && r["data"]["command"] == "plca_data:DelayPending")
    );
    if std::env::var_os("DIR_PHY_RECOVERY_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}
