//! Real process-boundary persistence of the staged network owner and its FES.
use dir_simulator::registry::Registry;
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
const CONFIGS: [&str; 23] = [
    "examples/ethernet/dynamic/unicast.ini",
    "examples/frer-rtag/model.ini",
    "examples/ethernet/tsn/tas.ini",
    "examples/can-ethernet/r01.ini",
    "examples/network-extensions/composite.ini",
    "examples/network-extensions/redundancy.ini",
    "examples/network-extensions/preemption.ini",
    "examples/ethernet-wire/active.ini",
    "examples/ethernet/tsn/cbs.ini",
    "examples/ethernet/tsn/cbs-gated.ini",
    "examples/ethernet/tsn/psfp.ini",
    "examples/ethernet/tsn/tas-update.ini",
    "examples/ethernet/dynamic/unicast.ini",
    "examples/network-composite/local-clock.ini",
    "examples/network-composite/sampled-phy-short.ini",
    "examples/network-composite/active-multicast/model.ini",
    "examples/network-composite/qcc/model.ini",
    "examples/network-composite/shared-lan/model.ini",
    "examples/network-composite/provider-l2/model.ini",
    "examples/network-composite/mstp-region/model.ini",
    "examples/network-composite/psfp-clock/steps.ini",
    "examples/network-composite/provider-l2/protocol-short.ini",
    "examples/frer-rtag/distributed/model.ini",
];
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::var_os("DIR_NETWORK_CHECKPOINT_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "dir-network-checkpoint-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if std::env::var_os("DIR_KEEP_NETWORK_CHECKPOINT").is_none()
            && std::env::var_os("DIR_NETWORK_CHECKPOINT_EVIDENCE").is_none()
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn config(index: usize) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(CONFIGS[index])
}
fn pause(session: &mut RunSession, controller: &dir_simulator::Controller) {
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
}
fn run_child(root: &Path, mode: &str, index: usize, boundary: u64) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "network_checkpoint_process_worker",
            "--nocapture",
        ])
        .env("DIR_NETWORK_CHECKPOINT_MODE", mode)
        .env("DIR_NETWORK_CHECKPOINT_ROOT", root)
        .env("DIR_NETWORK_CHECKPOINT_PROFILE", index.to_string())
        .env("DIR_NETWORK_CHECKPOINT_BOUNDARY", boundary.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode} {} boundary {boundary}:\n{}\n{}",
        CONFIGS[index],
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn network_checkpoint_process_worker() {
    let Ok(mode) = std::env::var("DIR_NETWORK_CHECKPOINT_MODE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_NETWORK_CHECKPOINT_ROOT").unwrap());
    let index: usize = std::env::var("DIR_NETWORK_CHECKPOINT_PROFILE")
        .unwrap()
        .parse()
        .unwrap();
    let boundary: u64 = std::env::var("DIR_NETWORK_CHECKPOINT_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    if mode == "save" {
        let scenario = root.join("scenario.ini");
        if !scenario.exists() {
            let source = config(index);
            let mut text = fs::read_to_string(&source).unwrap();
            for key in ["ned-path", "model-config", "workload"] {
                let line = text
                    .lines()
                    .find(|l| l.starts_with(key))
                    .unwrap()
                    .to_owned();
                let relative = line.split_once('=').unwrap().1.trim().trim_matches('"');
                let absolute = source
                    .parent()
                    .unwrap()
                    .join(relative)
                    .canonicalize()
                    .unwrap();
                text = text.replace(&line, &format!("{key} = \"{}\"", absolute.display()));
            }
            if index == 6 {
                let limit = text
                    .lines()
                    .find(|l| l.starts_with("sim-time-limit"))
                    .unwrap()
                    .to_owned();
                text = text.replace(&limit, "sim-time-limit = 1000000000ps");
            }
            if index == 11 {
                text.push_str("\nmetrics-version = 2\n");
            }
            fs::write(&scenario, text).unwrap();
        }
        let (mut s, c) = RunSession::new(
            prepare(&scenario).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        if boundary == 1500 {
            // Stop immediately after the scheduled membership change at 1.5 s;
            // its same-time timer is still pending in the checkpoint FES.
            let change_ps = 1_500_000_000_000u64;
            loop {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
                if s.snapshot().common.last_event_time_ps.unwrap_or(0) >= change_ps {
                    break;
                }
            }
        } else if boundary == 2000 {
            // Stop when actual ISS delivery has queued a customer BPDU for a
            // physical provider port, after the service tree can forward.
            while !s.observe().unwrap().registered.as_ref().is_some_and(|r| {
                r.model_records.values().any(|row| {
                    row.schema.name == "ethernet.control.wire.event"
                        && row.data["event_kind"] == "tx_queued"
                        && row.data["port"]
                            .as_str()
                            .is_some_and(|p| p.ends_with(".tx_pnp"))
                        && row.data["bytes_hex"]
                            .as_str()
                            .is_some_and(|b| b.starts_with("0180c2000000"))
                })
            }) {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
            }
        } else if boundary == 1000 {
            let mut found = false;
            for _ in 0..512 {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
                if representative(&s, index) {
                    found = true;
                    break;
                }
            }
            assert!(found, "representative state missing for {}", CONFIGS[index]);
        } else if boundary > 0 {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            for _ in 1..boundary {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
            }
        }
        pause(&mut s, &c);
        let ack = s
            .checkpoint(&root.join("bundle"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let checkpoint: Value =
            serde_json::from_slice(&fs::read(root.join("bundle/state.json")).unwrap()).unwrap();
        fs::write(root.join("verification.json"),serde_json::to_vec_pretty(&serde_json::json!({"profile":s.prepared().common.profile,"boundary":boundary,"committed_events":s.snapshot().common.committed_events.to_string(),"horizon_ps":s.snapshot().common.last_event_time_ps.unwrap_or(0).to_string(),"fes_entries":checkpoint["fes"].as_array().unwrap().len(),"dirty_entries":checkpoint["dirty"].as_array().unwrap().len(),"live_timers":checkpoint["live_timers"].as_array().unwrap().len(),"initialized":!checkpoint["network_backend"].is_null(),"representative":boundary==1000})).unwrap()).unwrap();
        s.resume(s.pause_token().unwrap()).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&root.join("original")).unwrap().status,
            ControlStatus::Applied
        );
    } else {
        let (mut s, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        assert_eq!(s.state(), RunState::Paused);
        s.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&root.join("restored")).unwrap().status,
            ControlStatus::Applied
        );
    }
}
#[test]
fn all_network_profiles_resume_in_a_new_process_before_and_after_initialization() {
    for (index, config) in CONFIGS[..7].iter().enumerate() {
        for boundary in [0, 1, 4, 10] {
            eprintln!("checking {config} boundary {boundary}");
            let t = Temp::new();
            run_child(&t.0, "save", index, boundary);
            fs::write(
                t.0.join("scenario.ini"),
                "frozen input must be used instead of rereading this file",
            )
            .unwrap();
            run_child(&t.0, "restore", index, boundary);
            let read = |name| {
                serde_json::from_slice::<Value>(
                    &fs::read(t.0.join(name).join("results.json")).unwrap(),
                )
                .unwrap()
            };
            let a = read("original");
            let b = read("restored");
            assert_eq!(canonical(a), canonical(b), "{} boundary {boundary}", config);
        }
    }
}

fn rewrite_state(path: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let mut state: Value =
        serde_json::from_slice(&fs::read(path.join("state.json")).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(path.join("state.json"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "state.json")
        .unwrap();
    entry["bytes"] = serde_json::json!(bytes.len().to_string());
    entry["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(
        path.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}
#[test]
fn corrupt_network_schema_cursor_queue_timer_and_bundle_limits_are_rejected() {
    let t = Temp::new();
    let (mut session, c) = RunSession::new(
        prepare(&config(0)).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..10 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
    }
    pause(&mut session, &c);
    for case in [
        "version",
        "cursor",
        "queue",
        "timer",
        "payload",
        "hash",
        "fes-limit",
        "blob-limit",
    ] {
        let path = t.0.join(case);
        assert_eq!(
            session
                .checkpoint(&path, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        match case {
            "version" => rewrite_state(&path, |s| {
                s["network_backend"]["version"] = serde_json::json!(999)
            }),
            "cursor" => rewrite_state(&path, |s| {
                s["network_backend"]["state"]["core"]["cursors"][0] = serde_json::json!(u64::MAX)
            }),
            "queue" => rewrite_state(&path, |s| {
                s["network_backend"]["state"]["core"]["queues"][0]["waiting"][0] =
                    serde_json::json!([999999])
            }),
            "timer" => rewrite_state(&path, |s| {
                s["live_timers"][0]["id"] = serde_json::json!(u64::MAX.to_string())
            }),
            "payload" => rewrite_state(&path, |s| {
                let delivery = s["fes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["work"]["tag"] == "Delivery")
                    .unwrap();
                delivery["work"]["envelope"]["payload"] = serde_json::json!(
                    serde_json::to_vec(&serde_json::json!({"Ready":999999})).unwrap()
                );
            }),
            "hash" => {
                fs::write(path.join("state.json"), b"{}").unwrap();
            }
            "fes-limit" => rewrite_state(&path, |s| {
                let entry = s["fes"][0].clone();
                s["fes"].as_array_mut().unwrap().push(entry);
            }),
            _ => {}
        }
        let mut limits = CheckpointLimits::default();
        if case == "fes-limit" {
            limits.max_fes_entries = 1;
        }
        if case == "blob-limit" {
            limits.max_blob_bytes = 1;
        }
        let result = restore_checkpoint(&path, Registry::default(), limits);
        assert!(result.is_err(), "corruption accepted: {case}");
    }
}

#[test]
fn tsn_credit_meter_and_updated_schedule_resume_in_a_new_process() {
    for fixture in [
        "examples/network-extensions/redundancy.ini",
        "examples/network-extensions/preemption.ini",
        "examples/ethernet/tsn/cbs.ini",
        "examples/ethernet/tsn/cbs-gated.ini",
        "examples/ethernet/tsn/psfp.ini",
        "examples/ethernet/tsn/tas-update.ini",
    ] {
        let index = CONFIGS.iter().position(|path| *path == fixture).unwrap();
        let t = Temp::new();
        run_child(&t.0, "save", index, 1000);
        run_child(&t.0, "restore", index, 1000);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "{}",
            CONFIGS[index]
        );
    }
}

#[test]
fn rtag_recovery_resumes_in_a_new_process() {
    let t = Temp::new();
    run_child(&t.0, "save", 12, 4);
    run_child(&t.0, "restore", 12, 4);
    let read = |name| {
        serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
            .unwrap()
    };
    assert_eq!(canonical(read("original")), canonical(read("restored")));
}

#[test]
fn composite_local_clocks_resume_in_a_new_process_across_adjustments() {
    for boundary in [0, 1, 4, 12, 24] {
        let t = Temp::new();
        run_child(&t.0, "save", 13, boundary);
        run_child(&t.0, "restore", 13, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "boundary {boundary}"
        );
    }
}

#[test]
fn composite_sampled_phy_midframe_resumes_in_a_new_process() {
    let t = Temp::new();
    run_child(&t.0, "save", 14, 5000);
    run_child(&t.0, "restore", 14, 5000);
    let read = |name| {
        serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
            .unwrap()
    };
    assert_eq!(canonical(read("original")), canonical(read("restored")));
}

#[test]
fn composite_sampled_phy_checkpoint_requires_a_matching_sample_wake() {
    let t = Temp::new();
    let (mut session, controller) = RunSession::new(
        prepare(&config(14)).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..4 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
    }
    pause(&mut session, &controller);
    let bundle = t.0.join("missing-phy-wake");
    assert_eq!(
        session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    rewrite_state(&bundle, |state| {
        let wakes = &mut state["network_backend"]["state"]["phy_wakes"];
        assert_eq!(wakes.as_array().unwrap().len(), 1);
        wakes[0] = Value::Null;
    });
    assert!(restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err());
}

#[test]
fn composite_active_multicast_resumes_in_a_new_process() {
    for boundary in [0, 1, 20] {
        let t = Temp::new();
        run_child(&t.0, "save", 15, boundary);
        run_child(&t.0, "restore", 15, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "active multicast boundary {boundary}"
        );
    }
}

#[test]
fn active_multicast_checkpoint_rejects_rehashed_packet_derived_membership() {
    let t = Temp::new();
    run_child(&t.0, "save", 15, 20);
    let bundle = t.0.join("bundle");
    rewrite_state(&bundle, |state| {
        let entries = state["network_backend"]["state"]["multicast_active"]["snooped"]
            .as_array_mut()
            .unwrap();
        let group = entries
            .iter_mut()
            .find(|entry| entry[1]["sources"].get("192.0.2.1").is_some())
            .unwrap();
        let expiry = group[1]["sources"]["192.0.2.1"].clone();
        group[1]["sources"]["192.0.2.99"] = expiry;
    });
    assert!(restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err());
}

#[test]
fn active_multicast_checkpoint_rejects_rehashed_blocked_set() {
    let t = Temp::new();
    run_child(&t.0, "save", 15, 20);
    let bundle = t.0.join("bundle");
    rewrite_state(&bundle, |state| {
        let entries = state["network_backend"]["state"]["multicast_active"]["snooped"]
            .as_array_mut()
            .unwrap();
        let group = entries
            .iter_mut()
            .find(|entry| entry[1]["sources"].get("192.0.2.1").is_some())
            .unwrap();
        group[1]["blocked"]
            .as_array_mut()
            .unwrap()
            .push("192.0.2.99".into());
    });
    assert!(restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err());
}

#[test]
fn active_multicast_scheduled_change_and_same_time_timer_resume() {
    let t = Temp::new();
    run_child(&t.0, "save", 15, 1500);
    run_child(&t.0, "restore", 15, 1500);
    let read = |name| {
        serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
            .unwrap()
    };
    assert_eq!(canonical(read("original")), canonical(read("restored")));
}

#[test]
fn composite_qcc_resumes_in_a_new_process() {
    for boundary in [0, 3, 12, 35, 70] {
        let t = Temp::new();
        run_child(&t.0, "save", 16, boundary);
        run_child(&t.0, "restore", 16, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "Qcc boundary {boundary}"
        );
    }
}

#[test]
fn composite_shared_lan_resumes_in_a_new_process() {
    for boundary in [0, 1, 8, 16, 30] {
        let t = Temp::new();
        run_child(&t.0, "save", 17, boundary);
        run_child(&t.0, "restore", 17, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "shared LAN boundary {boundary}"
        );
    }
}

#[test]
fn composite_provider_l2_resumes_with_component_scoped_fdb() {
    for boundary in [0, 1, 20, 50] {
        let t = Temp::new();
        run_child(&t.0, "save", 18, boundary);
        run_child(&t.0, "restore", 18, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "provider L2 boundary {boundary}"
        );
    }
}

#[test]
fn composite_mstp_region_resumes_with_per_vid_tree_projection() {
    for boundary in [0, 220, 420] {
        let t = Temp::new();
        run_child(&t.0, "save", 19, boundary);
        run_child(&t.0, "restore", 19, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "MSTP region boundary {boundary}"
        );
    }
}

#[test]
fn composite_psfp_local_clock_steps_resume_in_a_new_process() {
    for boundary in [0, 1, 5, 10] {
        let t = Temp::new();
        run_child(&t.0, "save", 20, boundary);
        run_child(&t.0, "restore", 20, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "PSFP local clock boundary {boundary}"
        );
    }
}

#[test]
fn shared_lan_checkpoint_rejects_rehashed_active_owner() {
    let t = Temp::new();
    let (mut session, controller) = RunSession::new(
        prepare(&config(17)).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut found = false;
    for step in 0..48 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
        pause(&mut session, &controller);
        let bundle = t.0.join(format!("active-{step}"));
        assert_eq!(
            session
                .checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let state: Value =
            serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
        if state["network_backend"]["state"]["segment_active"][0].is_object() {
            rewrite_state(&bundle, |state| {
                state["network_backend"]["state"]["segment_active"][0]["release_ps"] =
                    serde_json::json!(0);
            });
            assert!(
                restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
                    .is_err()
            );
            found = true;
            break;
        }
        session.resume(session.pause_token().unwrap()).unwrap();
    }
    assert!(found, "shared LAN active reservation was not observed");
}

#[test]
fn qcc_checkpoint_rejects_rehashed_source_and_schedule_forgery() {
    for corruption in ["source", "frame_origin", "schedule", "quiescence"] {
        let t = Temp::new();
        run_child(&t.0, "save", 16, 70);
        let bundle = t.0.join("bundle");
        rewrite_state(&bundle, |state| {
            let backend = &mut state["network_backend"]["state"];
            match corruption {
                "source" => {
                    let sources = backend["qcc_sources"].as_array_mut().unwrap();
                    assert!(!sources.is_empty());
                    sources[0][1]["payload_bytes"] = serde_json::json!(999);
                }
                "frame_origin" => {
                    let sources = backend["qcc_sources"].as_array().unwrap();
                    let frame = sources[0][0].as_u64().unwrap() as usize;
                    backend["core"]["frames"][frame]["row"]["source"] = serde_json::json!("Net.c");
                }
                "schedule" => {
                    let ports = backend["tsn"]["ports"].as_array_mut().unwrap();
                    let managed = ports
                        .iter_mut()
                        .find(|port| port["schedule_generation"].as_u64().unwrap_or(0) > 0)
                        .unwrap();
                    managed["schedule_generation"] = serde_json::json!(999);
                }
                "quiescence" => {
                    let audit = backend["qcc"]["audit"].as_array_mut().unwrap();
                    let advance = audit
                        .iter_mut()
                        .find(|step| step["action"] == "advance")
                        .unwrap();
                    let old = advance["quiescent"].as_bool().unwrap();
                    advance["quiescent"] = serde_json::json!(!old);
                }
                _ => unreachable!(),
            }
        });
        assert!(
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err(),
            "Qcc forged {corruption} was restored"
        );
    }
}

fn representative(s: &RunSession, index: usize) -> bool {
    let Some(n) = s.snapshot().network.as_ref() else {
        return false;
    };
    match CONFIGS[index] {
        "examples/network-extensions/redundancy.ini" => n["redundancy"]["streams"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["high"].is_string())),
        "examples/network-extensions/preemption.ini" => n["preemption"]["ports"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["suspended"].is_string())),
        "examples/ethernet/tsn/cbs.ini" | "examples/ethernet/tsn/cbs-gated.ini" => {
            n["tsn"]["ports"].as_array().is_some_and(|rows| {
                rows.iter().any(|r| {
                    r["credits"].as_array().is_some_and(|credits| {
                        credits
                            .iter()
                            .any(|c| c["credit"]["magnitude"].as_str().is_some_and(|v| v != "0"))
                    })
                })
            })
        }
        "examples/ethernet/tsn/psfp.ini" => n["tsn"]["meters"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|r| r["last_evaluated_ps"].as_str().is_some_and(|v| v != "0"))
        }),
        "examples/ethernet/tsn/tas-update.ini" => {
            n["tsn"]["update_cursor"].as_str().is_some_and(|v| v != "0")
        }
        _ => false,
    }
}

fn canonical(mut value: Value) -> Value {
    let root = value.as_object_mut().unwrap();
    root.remove("run_id");
    root.remove("control");
    let metadata = root.get_mut("metadata").unwrap().as_object_mut().unwrap();
    metadata.remove("started_at_utc");
    metadata.remove("finished_at_utc");
    value
}

#[test]
fn live_completion_ledgers_reject_missing_due_and_validly_reserved_invalid_reception() {
    let temp = Temp::new();
    let (mut session, controller) = RunSession::new(
        prepare(&config(0)).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..5 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
    }
    pause(&mut session, &controller);
    for case in [
        "missing-due",
        "invalid-complete",
        "control-generation",
        "missing-control",
        "cursor-ledger",
    ] {
        let bundle = temp.0.join(case);
        assert_eq!(
            session
                .checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        rewrite_state(&bundle, |state| {
            let core = &state["network_backend"]["state"]["core"];
            assert_eq!(core["transfers"].as_array().unwrap().len(), 1);
            assert!(core["receptions"].as_array().unwrap().is_empty());
            match case {
                "missing-due" => {
                    state["network_backend"]["state"]["core"]["due"] = serde_json::json!([])
                }
                "invalid-complete" => {
                    let id = state["next_event_id"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap();
                    let sequence = state["next_sequence"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap();
                    let time = state["virtual_time_ps"].as_str().unwrap().to_owned();
                    let token = serde_json::json!({"owner":"@network","id":id.to_string()});
                    let event = serde_json::json!({"time_ps":time,"delta":"1","phase":1,"sequence":sequence.to_string(),"work":{"tag":"Delivery","token":token,"envelope":{"source":"@network","destination":"@network","input_port":null,"request_id":null,"event_id":id.to_string(),"time_ps":time,"schema":{"name":"dir.network.event","version":1},"payload":serde_json::to_vec(&serde_json::json!({"Complete":0})).unwrap()}}});
                    state["fes"].as_array_mut().unwrap().push(event);
                    state["live_timers"].as_array_mut().unwrap().push(token);
                    state["next_event_id"] = serde_json::json!((id + 1).to_string());
                    state["next_sequence"] = serde_json::json!((sequence + 1).to_string());
                    let direct = state["network_backend"]["state"]["core"]["pending_direct"]
                        .as_u64()
                        .unwrap();
                    state["network_backend"]["state"]["core"]["pending_direct"] =
                        serde_json::json!(direct + 1);
                }
                "control-generation" => {
                    let generation = core["control_generation"].as_u64().unwrap();
                    state["network_backend"]["state"]["core"]["control_generation"] =
                        serde_json::json!(generation + 1);
                }
                "missing-control" => {
                    state["network_backend"]["state"]["core"]["control"] = Value::Null
                }
                "cursor-ledger" => {
                    state["network_backend"]["state"]["core"]["cursors"][0] = serde_json::json!(0)
                }
                _ => unreachable!(),
            }
        });
        let error = restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
            .err()
            .expect(case);
        assert_eq!(error.code, "CheckpointInvalid", "{case}: {error}");
        if case == "invalid-complete" {
            assert!(error.message.contains("completion reception"), "{error}");
        }
    }
}

#[test]
fn ini_metric_v2_catalog_is_rebuilt_from_captured_inputs_in_a_new_process() {
    let temp = Temp::new();
    run_child(&temp.0, "save", 11, 5);
    fs::write(
        temp.0.join("scenario.ini"),
        "captured metrics-version input must be used",
    )
    .unwrap();
    run_child(&temp.0, "restore", 11, 5);
    let read = |name| {
        serde_json::from_slice::<Value>(&fs::read(temp.0.join(name).join("results.json")).unwrap())
            .unwrap()
    };
    let original = read("original");
    assert_eq!(original["schema_version"], 3);
    assert_eq!(canonical(original), canonical(read("restored")));
}
#[test]
fn api_only_metric_v2_catalog_requires_the_exact_registry() {
    let temp = Temp::new();
    let mut prepared = prepare(&config(0)).unwrap();
    dir_simulator::runtime::network_metrics::enable(&mut prepared).unwrap();
    let registry = prepared
        .registered
        .as_ref()
        .unwrap()
        .registry
        .as_ref()
        .clone();
    let (mut session, controller) = RunSession::new(
        prepared,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    session.advance_one().unwrap();
    pause(&mut session, &controller);
    let bundle = temp.0.join("bundle");
    assert_eq!(
        session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    assert_eq!(
        restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default())
            .err()
            .unwrap()
            .code,
        "FingerprintMismatch"
    );
    let (mut restored, token) =
        restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
}

#[test]
fn authoritative_arena_journal_rejects_foreign_profile_future_time_and_nonobject_data() {
    let temp = Temp::new();
    let (mut session, controller) = RunSession::new(
        prepare(&config(0)).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..5 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
    }
    pause(&mut session, &controller);
    for case in ["valid", "foreign-profile", "future-time", "nonobject-data"] {
        let bundle = temp.0.join(case);
        assert_eq!(
            session
                .checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        if case != "valid" {
            rewrite_state(&bundle, |state| {
                let horizon = state["virtual_time_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                let records = state["network_backend"]["records"].as_array_mut().unwrap();
                assert!(!records.is_empty());
                match case {
                    "foreign-profile" => {
                        records[0][0] = serde_json::json!("ethernet.tsn.credit:foreign-checkpoint");
                        records[0][1]["id"] = serde_json::json!("foreign-checkpoint");
                        records[0][1]["schema"] =
                            serde_json::json!({"name":"ethernet.tsn.credit","version":1});
                        records.sort_by(|a, b| a[0].as_str().unwrap().cmp(b[0].as_str().unwrap()));
                    }
                    "future-time" => records[0][1]["time_ps"] = serde_json::json!(horizon + 1),
                    "nonobject-data" => records[0][1]["data"] = serde_json::json!(false),
                    _ => unreachable!(),
                }
            });
        }
        let result = restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default());
        if case == "valid" {
            let (mut restored, token) = result.unwrap();
            restored.resume(token).unwrap();
            assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        } else {
            let error = result.err().expect(case);
            assert_eq!(error.code, "CheckpointInvalid");
            assert!(
                error.message.contains("arena checkpoint journal"),
                "{case}: {error}"
            );
        }
    }
}

#[test]
fn provider_component_protocol_pending_iss_resumes_in_a_new_process() {
    for boundary in [0, 1, 4, 20] {
        let t = Temp::new();
        run_child(&t.0, "save", 21, boundary);
        run_child(&t.0, "restore", 21, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "provider protocol boundary {boundary}"
        );
    }
}

#[test]
fn distributed_frer_resume_preserves_physical_phase_boundaries() {
    for boundary in [0, 2, 4, 7, 10, 14, 18, 22, 26, 28, 30, 32, 33] {
        let t = Temp::new();
        run_child(&t.0, "save", 22, boundary);
        run_child(&t.0, "restore", 22, boundary);
        let read = |name| {
            serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
                .unwrap()
        };
        assert_eq!(
            canonical(read("original")),
            canonical(read("restored")),
            "boundary {boundary}"
        );
    }
}

#[test]
fn distributed_frer_rehashed_semantic_forgery_is_rejected() {
    for mutation in [
        "winner",
        "phase",
        "hop",
        "sequence",
        "bitmap",
        "timeout",
        "route",
        "suffix_parent",
        "delivery",
    ] {
        let t = Temp::new();
        run_child(&t.0, "save", 22, 30);
        rewrite_state(&t.0.join("bundle"), |s| {
            let backend = &mut s["network_backend"]["state"];
            let state = &mut backend["replication"];
            let flow_id = state["distributed"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            match mutation {
                "winner" => {
                    state["distributed"][&flow_id]["winner_reception"] = serde_json::json!(0)
                }
                "phase" => {
                    backend["core"]["transfers"][1]["circuit"]["distributed"]["leg"] =
                        serde_json::json!({"phase":"suffix"})
                }
                "hop" => backend["core"]["transfers"][1]["circuit"]["hop"] = serde_json::json!(1),
                "sequence" => state["distributed"][&flow_id]["sequence"] = serde_json::json!(7),
                "bitmap" => state["streams"][0]["history"]["words"][0] = serde_json::json!(3),
                "timeout" => state["streams"][0]["last_recovery_ps"] = serde_json::json!(0),
                "route" => {
                    state["distributed_identity"][0]["prefix_ports"][0] =
                        serde_json::json!("Net.e.tx_d")
                }
                "suffix_parent" => {
                    backend["core"]["transfers"][5]["row"]["parent_transfer_id"] =
                        serde_json::json!("invalid")
                }
                "delivery" => {
                    state["distributed"][&flow_id]["delivery_reception"] = serde_json::json!(0)
                }
                _ => unreachable!(),
            }
        });
        assert!(
            restore_checkpoint(
                &t.0.join("bundle"),
                Registry::default(),
                CheckpointLimits::default()
            )
            .is_err(),
            "accepted {mutation}"
        );
    }
}

#[test]
fn provider_relay_checkpoint_rejects_rehashed_transport_domain_forgery() {
    let t = Temp::new();
    let source = config(21);
    let mut text = fs::read_to_string(&source).unwrap();
    for key in ["ned-path", "model-config", "workload"] {
        let line = text
            .lines()
            .find(|l| l.starts_with(key))
            .unwrap()
            .to_owned();
        let relative = line.split_once('=').unwrap().1.trim().trim_matches('"');
        let path = source
            .parent()
            .unwrap()
            .join(relative)
            .canonicalize()
            .unwrap();
        text = text.replace(&line, &format!("{key} = \"{}\"", path.display()));
    }
    let line = text
        .lines()
        .find(|l| l.starts_with("sim-time-limit"))
        .unwrap()
        .to_owned();
    text = text.replace(&line, "sim-time-limit = 8500000000000ps");
    fs::write(t.0.join("scenario.ini"), text).unwrap();
    run_child(&t.0, "save", 21, 2000);
    run_child(&t.0, "restore", 21, 2000);
    let read = |name| {
        serde_json::from_slice::<Value>(&fs::read(t.0.join(name).join("results.json")).unwrap())
            .unwrap()
    };
    assert_eq!(canonical(read("original")), canonical(read("restored")));
    let bundle = t.0.join("bundle");
    let state = fs::read(bundle.join("state.json")).unwrap();
    let manifest = fs::read(bundle.join("manifest.json")).unwrap();
    for mutation in [
        "missing",
        "local_protocol",
        "component",
        "ingress",
        "vid",
        "tree",
    ] {
        fs::write(bundle.join("state.json"), &state).unwrap();
        fs::write(bundle.join("manifest.json"), &manifest).unwrap();
        rewrite_state(&bundle, |s| {
            let tx = s["network_backend"]["state"]["wire"]["host"]["tx"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .find(|tx| {
                    tx["transport"]["kind"] == "relayed_service"
                        && tx["eof"].is_null()
                        && tx["dropped"] == false
                })
                .expect("actual physical customer BPDU relay must be pending");
            match mutation {
                "missing" => {
                    tx.as_object_mut().unwrap().remove("transport");
                }
                "local_protocol" => {
                    tx["transport"] = serde_json::json!({
                    "kind":"local_protocol", "component":tx["transport"]["component"],
                    "port":tx["intent"]["port"]})
                }
                "component" => tx["transport"]["component"] = serde_json::json!("Net.east.ca"),
                "ingress" => tx["transport"]["ingress"] = tx["intent"]["port"].clone(),
                "vid" => tx["transport"]["vid"] = serde_json::json!(200),
                "tree" => tx["transport"]["tree"] = serde_json::json!(2),
                _ => unreachable!(),
            }
        });
        assert!(
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err(),
            "accepted forged component transport {mutation}"
        );
    }
}
