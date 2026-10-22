//! Packet-derived gPTP production and checkpoint evidence for the selected two-step tree profile.
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint, run, viewer,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-gptp-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gptp")
            .canonicalize()
            .unwrap()
    }
    fn ini(&self, horizon: u64, model: Option<&Value>) -> PathBuf {
        let root = Self::root();
        let model_path = self.0.join("model.json");
        if let Some(model) = model {
            fs::write(&model_path, serde_json::to_vec(model).unwrap()).unwrap()
        }
        let selected_model = if model.is_some() {
            model_path.clone()
        } else {
            root.join("two-node-model.json")
        };
        let original = fs::read_to_string(root.join("two-node.ini")).unwrap();
        let text = original
            .replace(
                "ned-path = \"models/two\"",
                &format!("ned-path = \"{}\"", root.join("models/two").display()),
            )
            .replace(
                "model-config = \"two-node-model.json\"",
                &format!("model-config = \"{}\"", selected_model.display()),
            )
            .replace(
                "workload = \"two-node-workload.json\"",
                &format!(
                    "workload = \"{}\"",
                    root.join("two-node-workload.json").display()
                ),
            )
            .replace(
                "sim-time-limit = 10000000000ps",
                &format!("sim-time-limit = {horizon}ps"),
            );
        let ini = self.0.join("case.ini");
        fs::write(&ini, text).unwrap();
        ini
    }
    fn named(&self, name: &str, horizon: u64) -> PathBuf {
        let root = Self::root();
        let source = root.join(format!("{name}.ini"));
        let mut text = fs::read_to_string(source).unwrap();
        for key in ["ned-path", "model-config", "workload"] {
            let line = text
                .lines()
                .find(|line| line.starts_with(key))
                .unwrap()
                .to_string();
            let relative = line.split_once('=').unwrap().1.trim().trim_matches('"');
            text = text.replace(
                &line,
                &format!("{key} = \"{}\"", root.join(relative).display()),
            );
        }
        let limit = text
            .lines()
            .find(|line| line.starts_with("sim-time-limit"))
            .unwrap()
            .to_string();
        text = text.replace(&limit, &format!("sim-time-limit = {horizon}ps"));
        let ini = self.0.join(format!("{name}.ini"));
        fs::write(&ini, text).unwrap();
        ini
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn two_node_packets_establish_as_capable_and_correct_clock() {
    let f = Fixture::new();
    let out = f.0.join("output");
    run(prepare(&f.ini(3_000_000_000, None)).unwrap(), &out).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
    let records = result["simulation"]["model_records"].as_array().unwrap();
    let kinds = records
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.gptp")
        .map(|r| r["data"]["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    for kind in [
        "tx_capture",
        "rx_capture",
        "pdelay_complete",
        "announce_qualified",
        "selection",
        "clock_corrected",
    ] {
        assert!(kinds.contains(&kind), "missing {kind}");
    }
    assert!(
        records
            .iter()
            .any(|r| r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "clock_corrected"
                && r["data"]["details"]["node"] == "Net.b"
                && r["data"]["details"]["rate_numerator"] == "1"
                && r["data"]["details"]["rate_denominator"] == "1")
    );
    let html = f.0.join("viewer.html");
    viewer::write(&out.join("results.json"), &html).unwrap();
    assert!(
        fs::read_to_string(html)
            .unwrap()
            .contains("gPTPパケットと時刻同期")
    );
}

#[test]
fn express_mac_merge_transports_real_gptp_packets_and_corrects_clock() {
    let f = Fixture::new();
    let out = f.0.join("merge");
    run(
        prepare(&f.named("two-node-merge", 3_000_000_000)).unwrap(),
        &out,
    )
    .unwrap();
    let rows = records(&out);
    assert!(rows.iter().any(|r| {
        r["schema_name"] == "dir.mac_merge.packet"
            && r["data"]["kind"] == "Express"
            && r["data"]["cause"] == "actual_receive"
            && r["data"]["transfer"]
                .as_str()
                .is_some_and(|v| v.starts_with("gptp:"))
    }));
    assert!(rows.iter().any(|r| {
        r["schema_name"] == "dir.network.gptp"
            && r["data"]["kind"] == "clock_corrected"
            && r["data"]["details"]["node"] == "Net.b"
    }));
}

#[test]
fn lost_sync_followup_and_bad_fcs_do_not_discipline_clock() {
    let f = Fixture::new();
    for (name, ordinal, outcome) in [
        ("two-node-drop-sync", "11", "dropped"),
        ("two-node-drop-followup", "13", "dropped"),
        ("two-node-bad-fcs-followup", "13", "corrupted"),
    ] {
        let out = f.0.join(name);
        run(prepare(&f.named(name, 1_000_000_000)).unwrap(), &out).unwrap();
        let rows = records(&out);
        assert!(
            rows.iter()
                .any(|r| r["schema_name"] == "dir.mac_merge.packet"
                    && r["data"]["port"] == "Net.a.tx"
                    && r["data"]["packet_ordinal"] == ordinal
                    && r["data"]["fault_outcome"] == outcome),
            "missing physical fault: {name}"
        );
        let corrections = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "dir.network.gptp" && r["data"]["kind"] == "clock_corrected"
            })
            .collect::<Vec<_>>();
        assert!(
            corrections
                .iter()
                .all(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap() > 700_000_000),
            "incomplete first Sync/FUP corrected clock: {name}"
        );
        assert!(
            !corrections.is_empty(),
            "later complete exchange should recover: {name}"
        );
    }
}

#[test]
fn real_queue_residence_expires_followup_without_clock_correction() {
    let f = Fixture::new();
    let out = f.0.join("delayed-followup");
    run(
        prepare(&f.named("two-node-delayed-followup", 1_600_000_000)).unwrap(),
        &out,
    )
    .unwrap();
    let rows = records(&out);
    let blocker = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer" && r["data"]["frame_id"] == "blocker:0"
        })
        .unwrap();
    assert_eq!(blocker["data"]["status"], "serialized");
    let waiting = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "sync_rx_waiting_followup"
        })
        .unwrap();
    let expiry = waiting["data"]["details"]["deadline_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let rejected = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "packet_rejected"
                && r["data"]["reason"] == "followup_after_pair_expiry"
                && r["data"]["sequence"] == waiting["data"]["sequence"]
        })
        .unwrap();
    assert!(
        rejected["time_ps"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > expiry
    );
    assert!(
        rows.iter()
            .all(|r| r["schema_name"] != "dir.network.gptp"
                || r["data"]["kind"] != "clock_corrected")
    );
    assert!(
        rows.iter().any(
            |r| r["schema_name"] == "dir.network.gptp" && r["data"]["kind"] == "pdelay_timeout"
        )
    );
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.gptp"
        && r["data"]["kind"] == "selection"
        && r["data"]["details"]["node"] == "Net.b"
        && r["data"]["details"]["parent"].is_null()));
}

#[test]
fn packet_discipline_changes_data_tas_departure_and_psfp_acceptance() {
    let f = Fixture::new();
    let mut outcomes = std::collections::BTreeMap::new();
    for name in ["two-node-servo-enabled", "two-node-servo-disabled"] {
        let out = f.0.join(name);
        run(prepare(&f.named(name, 3_000_000_000)).unwrap(), &out).unwrap();
        outcomes.insert(name, records(&out));
    }
    let enabled = &outcomes["two-node-servo-enabled"];
    let disabled = &outcomes["two-node-servo-disabled"];
    fn transfer<'a>(rows: &'a [Value], id: &str) -> &'a Value {
        rows.iter()
            .find(|r| {
                r["schema_name"] == "ethernet.dynamic.transfer"
                    && r["data"]["frame_id"] == id
                    && r["data"]["status"] == "serialized"
            })
            .unwrap()
    }
    let on_sof = transfer(enabled, "tas_probe:0")["data"]["sof_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let off_sof = transfer(disabled, "tas_probe:0")["data"]["sof_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert_eq!(on_sof, 2_000_000_000);
    assert!(off_sof > 2_008_000_000);
    fn reception(rows: &[Value]) -> &Value {
        rows.iter()
            .find(|r| {
                r["schema_name"] == "ethernet.dynamic.reception"
                    && r["data"]["frame_id"] == "psfp_probe:0"
            })
            .unwrap()
    }
    assert_eq!(reception(enabled)["data"]["status"], "received");
    assert_eq!(reception(disabled)["data"]["status"], "filtered");
    assert_eq!(reception(disabled)["data"]["reason"], "psfp_gate_closed");
    assert_eq!(
        transfer(enabled, "psfp_probe:0")["data"]["sof_ps"],
        transfer(disabled, "psfp_probe:0")["data"]["sof_ps"]
    );
    let corrections = enabled
        .iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "clock_corrected"
                && r["data"]["details"]["node"] == "Net.b"
        })
        .collect::<Vec<_>>();
    assert!(!corrections.is_empty());
    for correction in &corrections {
        let sync = correction["data"]["details"]["sync_transfer_id"]
            .as_str()
            .unwrap();
        let sync_frame = sync.split('@').next().unwrap();
        assert!(
            enabled
                .iter()
                .any(|r| r["schema_name"] == "ethernet.dynamic.transfer"
                    && r["data"]["frame_id"] == sync_frame
                    && r["data"]["status"] == "serialized")
        );
    }
    let clocks = enabled
        .iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.clock"
                && r["data"]["node"] == "Net.b"
                && r["data"]["cause"] == "gptp_sync"
        })
        .collect::<Vec<_>>();
    assert_eq!(clocks.len(), corrections.len());
    for row in clocks {
        let d = &row["data"];
        let den = d["local_denominator"]
            .as_str()
            .unwrap()
            .parse::<i128>()
            .unwrap();
        let num = d["local_numerator"]
            .as_str()
            .unwrap()
            .parse::<i128>()
            .unwrap();
        let global = d["global_ps"].as_str().unwrap().parse::<i128>().unwrap();
        assert!(
            (num - (global + 500_000) * den).abs() <= den,
            "GM phase error exceeded 1 ps: {d}"
        );
    }
}

#[test]
fn packet_correction_checkpoint_round_trip() {
    let f = Fixture::new();
    let p = prepare(&f.ini(3_000_000_000, None)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut corrected = false;
    for _ in 0..2000 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        corrected = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == "clock_corrected");
        if corrected {
            break;
        }
    }
    assert!(corrected, "no packet-derived correction before checkpoint");
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("bundle");
    assert_eq!(
        session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let (mut restored, token) =
        restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    session.resume(session.pause_token().unwrap()).unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(session.snapshot().network, restored.snapshot().network);
    assert_eq!(
        serde_json::to_value(
            &session
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
        )
        .unwrap(),
        serde_json::to_value(
            &restored
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
        )
        .unwrap()
    );
}

#[test]
fn servo_step_checkpoint_keeps_future_tas_and_psfp_data_outcomes() {
    let f = Fixture::new();
    let p = prepare(&f.named("two-node-servo-enabled", 3_000_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut live, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut corrected = false;
    for _ in 0..4000 {
        if live.advance_one().unwrap() != RunState::Running {
            break;
        }
        corrected = live
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "clock_corrected"
                    && r.data["details"]["node"] == "Net.b"
            });
        if corrected {
            break;
        }
    }
    assert!(corrected);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(live.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("servo-gates");
    live.checkpoint(&bundle, CheckpointLimits::default())
        .unwrap();
    let (mut restored, token) =
        restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    live.resume(live.pause_token().unwrap()).unwrap();
    assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(live.snapshot().network, restored.snapshot().network);
    assert_eq!(
        serde_json::to_value(&live.snapshot().registered.as_ref().unwrap().model_records).unwrap(),
        serde_json::to_value(
            &restored
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
        )
        .unwrap()
    );
    let final_snapshot = live.snapshot();
    let records = &final_snapshot.registered.as_ref().unwrap().model_records;
    assert!(records.values().any(|r| {
        r.schema.name == "ethernet.dynamic.transfer"
            && r.data["frame_id"] == "tas_probe:0"
            && r.data["sof_ps"] == "2000000000"
    }));
    assert!(records.values().any(|r| {
        r.schema.name == "ethernet.dynamic.reception"
            && r.data["frame_id"] == "psfp_probe:0"
            && r.data["status"] == "received"
    }));
}

#[test]
fn physical_and_protocol_pending_checkpoint_cuts_match_uninterrupted_trace() {
    let f = Fixture::new();
    for kind in [
        "packet_queued",
        "tx_capture",
        "rx_capture",
        "pdelay_response",
        "sync_rx_waiting_followup",
    ] {
        let p = prepare(&f.ini(1_000_000_000, None)).unwrap();
        let registry = (*p.registered.as_ref().unwrap().registry).clone();
        let (mut live, controller) = RunSession::new(
            p,
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut found = false;
        for _ in 0..4000 {
            if live.advance_one().unwrap() != RunState::Running {
                break;
            }
            found = live
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == kind);
            if found {
                break;
            }
        }
        assert!(found, "missing checkpoint cut {kind}");
        let snapshot = live.snapshot();
        let rows = &snapshot.registered.as_ref().unwrap().model_records;
        let milestone = rows
            .values()
            .find(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == kind)
            .unwrap();
        if ["packet_queued", "tx_capture", "rx_capture"].contains(&kind) {
            let transfer = &milestone.data["transfer_id"];
            let terminal = if kind == "packet_queued" {
                "tx_capture"
            } else {
                "physical_eof"
            };
            assert!(
                !rows.values().any(|r| r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == terminal
                    && r.data["transfer_id"] == *transfer),
                "{kind} cut already passed {terminal} on same physical packet"
            );
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(live.poll_control().unwrap(), RunState::Paused);
        let bundle = f.0.join(format!("cut-{kind}"));
        live.checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        if kind == "pdelay_response" {
            let mutant = f.0.join("cut-pdelay-response-mutant");
            live.checkpoint(&mutant, CheckpointLimits::default())
                .unwrap();
            rewrite_checkpoint(&mutant, |state| {
                let ports = state["network_backend"]["state"]["gptp"]["ports"]
                    .as_array_mut()
                    .unwrap();
                let pending = ports
                    .iter_mut()
                    .find_map(|p| {
                        p["pending"]
                            .as_object_mut()
                            .filter(|v| v["response_transfer"].is_number())
                    })
                    .unwrap();
                let t2 = &mut pending.get_mut("t2").unwrap()["num"];
                *t2 = (t2.as_str().unwrap().parse::<i128>().unwrap() + 1)
                    .to_string()
                    .into();
            });
            let error = restore_checkpoint(&mutant, registry.clone(), CheckpointLimits::default())
                .err()
                .unwrap();
            assert_eq!(error.code, "CheckpointInvalid");
        }
        let (mut restored, token) =
            restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
        restored.resume(token).unwrap();
        live.resume(live.pause_token().unwrap()).unwrap();
        assert_eq!(
            live.run_until_yield().unwrap(),
            RunState::Completed,
            "live {kind}"
        );
        assert_eq!(
            restored.run_until_yield().unwrap(),
            RunState::Completed,
            "restored {kind}"
        );
        assert_eq!(
            live.snapshot().network,
            restored.snapshot().network,
            "network {kind}"
        );
        assert_eq!(
            serde_json::to_value(
                live.snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .clone()
            )
            .unwrap(),
            serde_json::to_value(
                restored
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .clone()
            )
            .unwrap(),
            "records {kind}"
        );
    }
}

#[test]
fn relayed_rate_checkpoint_round_trip_after_downstream_correction() {
    let f = Fixture::new();
    let p = prepare(&f.named("three-node", 3_000_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut downstream = false;
    for _ in 0..2500 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        downstream = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "clock_corrected"
                    && r.data["details"]["node"] == "Net.c"
            });
        if downstream {
            break;
        }
    }
    assert!(downstream);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("relay-bundle");
    session
        .checkpoint(&bundle, CheckpointLimits::default())
        .unwrap();
    let (mut restored, token) =
        restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    session.resume(session.pause_token().unwrap()).unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(session.snapshot().network, restored.snapshot().network);
}

#[test]
fn queued_relay_sync_checkpoint_keeps_actual_residence_and_followup() {
    let f = Fixture::new();
    let p = prepare(&f.named("three-node-queue", 1_200_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut live, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut transfer = None;
    for _ in 0..5000 {
        if live.advance_one().unwrap() != RunState::Running {
            break;
        }
        transfer = live
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .find(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "packet_queued"
                    && r.data["message_type"] == 0
                    && r.data["transfer_id"]
                        .as_str()
                        .is_some_and(|s| s.contains("Net.b.tx_c"))
                    && r.time_ps > 900_000_000
            })
            .and_then(|r| r.data["transfer_id"].as_str().map(ToOwned::to_owned));
        if transfer.is_some() {
            break;
        }
    }
    let transfer = transfer.expect("relay Sync must queue while data occupies egress");
    assert!(
        !live
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "dir.network.gptp"
                && r.data["kind"] == "physical_sof"
                && r.data["transfer_id"] == transfer)
    );
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(live.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("queued-relay");
    live.checkpoint(&bundle, CheckpointLimits::default())
        .unwrap();
    let (mut restored, token) =
        restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    live.resume(live.pause_token().unwrap()).unwrap();
    assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(live.snapshot().network, restored.snapshot().network);
    assert_eq!(
        serde_json::to_value(
            live.snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .clone()
        )
        .unwrap(),
        serde_json::to_value(
            restored
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .clone()
        )
        .unwrap()
    );
}

#[test]
fn announce_timeout_boundary_checkpoint_matches_uninterrupted_prefix() {
    let f = Fixture::new();
    let p = prepare(&f.named("three-node-recovery", 4_000_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut live, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut at = None;
    for _ in 0..10000 {
        if live.advance_one().unwrap() != RunState::Running {
            break;
        }
        at = live
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .find(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "announce_timeout"
                    && r.data["details"]["port"] == "Net.a.tx"
            })
            .map(|r| r.time_ps);
        if at.is_some() {
            break;
        }
    }
    assert!(at.is_some(), "missing actual Announce receipt timeout");
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(live.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("timeout-boundary");
    live.checkpoint(&bundle, CheckpointLimits::default())
        .unwrap();
    let (mut restored, token) =
        restore_checkpoint(&bundle, registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    live.resume(live.pause_token().unwrap()).unwrap();
    assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(live.snapshot().network, restored.snapshot().network);
    assert_eq!(
        serde_json::to_value(
            live.snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .clone()
        )
        .unwrap(),
        serde_json::to_value(
            restored
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .clone()
        )
        .unwrap()
    );
}

#[test]
fn historical_correction_epoch_cannot_be_moved_to_another_valid_epoch() {
    let f = Fixture::new();
    let p = prepare(&f.named("three-node-recovery", 3_500_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut live, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut reselection = false;
    for _ in 0..10000 {
        if live.advance_one().unwrap() != RunState::Running {
            break;
        }
        reselection = live
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "selection"
                    && r.data["details"]["node"] == "Net.b"
                    && r.data["details"]["epoch"] == "3"
            });
        if reselection {
            break;
        }
    }
    assert!(reselection);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(live.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("historic-epoch");
    live.checkpoint(&bundle, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&bundle, |state| {
        let journal = state["network_backend"]["state"]["gptp"]["journal"]
            .as_array_mut()
            .unwrap();
        let first = journal.iter_mut().find(|r| r["node"] == 1).unwrap();
        assert_eq!(first["epoch"], 1);
        first["epoch"] = 2.into();
    });
    let error = restore_checkpoint(&bundle, registry, CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(
        error.message.contains("epoch")
            || error.message.contains("Announce")
            || error
                .message
                .contains("exact frozen-input committed prefix"),
        "{}",
        error.message
    );
}

fn rewrite_checkpoint(path: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let state_path = path.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(state_path, &bytes).unwrap();
    let manifest_path = path.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|v| v["path"] == "state.json")
        .unwrap();
    entry["bytes"] = bytes.len().to_string().into();
    entry["sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

#[test]
fn coordinated_pdelay_and_clock_claims_cannot_replace_physical_packet_evidence() {
    let f = Fixture::new();
    let p = prepare(&f.ini(3_000_000_000, None)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut corrected = false;
    for _ in 0..2000 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        corrected = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == "clock_corrected");
        if corrected {
            break;
        }
    }
    assert!(corrected);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let bundle = f.0.join("mutant");
    session
        .checkpoint(&bundle, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&bundle, |state| {
        let network = &mut state["network_backend"]["state"];
        let journal = &mut network["gptp"]["journal"];
        let sample_index = journal[0]["pdelay_sample"].as_u64().unwrap() as usize;
        let direction = journal[0]["parent"].as_u64().unwrap() as usize;
        let node = journal[0]["node"].as_u64().unwrap() as usize;
        let original = journal[0]["neighbor_rate"]["num"]
            .as_str()
            .unwrap()
            .parse::<i128>()
            .unwrap();
        let forged = original + 1;
        journal[0]["neighbor_rate"]["num"] = forged.to_string().into();
        journal[0]["target"]["num"] = (journal[0]["target"]["num"]
            .as_str()
            .unwrap()
            .parse::<i128>()
            .unwrap()
            + 1)
        .to_string()
        .into();
        network["gptp"]["samples"][sample_index]["rate"]["num"] = forged.to_string().into();
        network["gptp"]["ports"][direction]["rate"]["num"] = forged.to_string().into();
        network["clocks"][node]["anchor_local"]["numerator"] =
            (network["clocks"][node]["anchor_local"]["numerator"]
                .as_str()
                .unwrap()
                .parse::<i128>()
                .unwrap()
                + 1)
            .to_string()
            .into();
    });
    let error = restore_checkpoint(&bundle, registry.clone(), CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(
        error.message.contains("Pdelay")
            || error.message.contains("relay tuple")
            || error
                .message
                .contains("exact frozen-input committed prefix"),
        "{}",
        error.message
    );

    let forged_announce = f.0.join("forged-announce");
    session
        .checkpoint(&forged_announce, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&forged_announce, |state| {
        let ports = state["network_backend"]["state"]["gptp"]["ports"]
            .as_array_mut()
            .unwrap();
        let candidate = ports
            .iter_mut()
            .find_map(|port| port["candidate"].as_object_mut())
            .unwrap();
        let identity = candidate
            .get_mut("identity")
            .unwrap()
            .as_array_mut()
            .unwrap();
        identity[0] = (identity[0].as_u64().unwrap() + 1).into();
    });
    let error = restore_checkpoint(
        &forged_announce,
        registry.clone(),
        CheckpointLimits::default(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(
        error.message.contains("candidate")
            || error
                .message
                .contains("exact frozen-input committed prefix"),
        "{}",
        error.message
    );

    for kind in ["capture", "epoch", "target", "relay"] {
        let bundle = f.0.join(format!("forged-{kind}"));
        session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        rewrite_checkpoint(&bundle, |state| {
            let g = &mut state["network_backend"]["state"]["gptp"];
            match kind {
                "capture" => {
                    let packet = g["packets"]
                        .as_object_mut()
                        .unwrap()
                        .values_mut()
                        .find(|v| v["tx"].is_object())
                        .unwrap();
                    let at = packet["tx"]["at_ps"].as_u64().unwrap();
                    packet["tx"]["at_ps"] = (at + 1).into();
                }
                "epoch" => {
                    let at = g["journal"][0]["epoch"].as_u64().unwrap();
                    g["journal"][0]["epoch"] = (at + 1).into();
                }
                "target" => {
                    let original = g["journal"][0]["target"]["num"]
                        .as_str()
                        .unwrap()
                        .parse::<i128>()
                        .unwrap();
                    g["journal"][0]["target"]["num"] = (original + 1).to_string().into();
                }
                "relay" => {
                    let original = g["nodes"][1]["relay"]["upstream_raw"]["num"]
                        .as_str()
                        .unwrap()
                        .parse::<i128>()
                        .unwrap();
                    g["nodes"][1]["relay"]["upstream_raw"]["num"] =
                        (original + 1).to_string().into();
                }
                _ => unreachable!(),
            }
        });
        let error = restore_checkpoint(&bundle, registry.clone(), CheckpointLimits::default())
            .err()
            .unwrap_or_else(|| panic!("restore accepted forged {kind}"));
        assert_eq!(error.code, "CheckpointInvalid", "{kind}: {}", error.message);
    }
}

#[test]
fn selected_profile_rejects_broken_packet_ownership() {
    let f = Fixture::new();
    let root = Fixture::root();
    let baseline: Value =
        serde_json::from_slice(&fs::read(root.join("two-node-model.json")).unwrap()).unwrap();
    let mut cases = Vec::new();
    let mut m = baseline.clone();
    m["gptp"]["nodes"][1]["ports"][0]["source_mac"] = "01:00:00:00:00:02".into();
    cases.push(m);
    let mut m = baseline.clone();
    m["gptp"]["priority"] = 8.into();
    cases.push(m);
    let mut m = baseline.clone();
    m["gptp"]["nodes"][1]["clock_identity"] = "00:00:00:00:00:00:00:01".into();
    cases.push(m);
    let mut m = baseline.clone();
    m["gptp"]["domain"] = 1.into();
    cases.push(m);
    let mut m = baseline.clone();
    m["gptp"]["one_step"] = true.into();
    cases.push(m);
    for model in cases {
        assert!(prepare(&f.ini(1_000_000, Some(&model))).is_err());
    }
    let merge: Value =
        serde_json::from_slice(&fs::read(root.join("two-node-merge-model.json")).unwrap()).unwrap();
    assert!(
        prepare(&f.ini(1_000_000, Some(&merge))).is_ok(),
        "express MAC Merge and gPTP must compose"
    );
    let mut mismatch = merge.clone();
    mismatch["mac_merge"]["outputs"][0]["source_mac"] = "02:00:00:00:00:04".into();
    let error = prepare(&f.ini(1_000_000, Some(&mismatch))).unwrap_err();
    assert!(error.message.contains("source MAC disagree"), "{error}");
    let mut preemptable = merge;
    preemptable["gptp"]["priority"] = 0.into();
    let error = prepare(&f.ini(1_000_000, Some(&preemptable))).unwrap_err();
    assert!(error.message.contains("express MAC"), "{error}");
}

#[test]
fn pending_exchange_bound_is_enforced_during_physical_execution() {
    let f = Fixture::new();
    let mut model: Value =
        serde_json::from_slice(&fs::read(Fixture::root().join("two-node-model.json")).unwrap())
            .unwrap();
    model["gptp"]["max_pending_exchanges"] = 1.into();
    let report = run(
        prepare(&f.ini(1_000_000, Some(&model))).unwrap(),
        &f.0.join("bounded"),
    )
    .unwrap();
    assert_eq!(report.termination, "execution_failed");
    assert!(
        report
            .primary_diagnostic
            .as_ref()
            .unwrap()
            .message
            .contains("pending exchange limit")
    );
}

#[test]
fn incompatible_protocol_ports_are_rejected_after_other_adapters_are_prepared() {
    let f = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let gptp: Value =
        serde_json::from_slice(&fs::read(Fixture::root().join("two-node-model.json")).unwrap())
            .unwrap();
    for (index, name) in [
        "examples/network-composite/shared-lan/model.ini",
        "examples/network-composite/provider-l2/model.ini",
        "examples/network-composite/sampled-phy.ini",
    ]
    .iter()
    .enumerate()
    {
        let source = root.join(name);
        assert!(prepare(&source).is_ok(), "valid owner fixture {name}");
        let parent = source.parent().unwrap();
        let mut text = fs::read_to_string(&source).unwrap();
        let mut model_path = None;
        for key in ["ned-path", "model-config", "workload"] {
            let line = text
                .lines()
                .find(|line| line.starts_with(key))
                .unwrap()
                .to_string();
            let relative = line.split_once('=').unwrap().1.trim().trim_matches('"');
            let original = parent.join(relative).canonicalize().unwrap();
            let replacement = if key == "model-config" {
                let mut model: Value =
                    serde_json::from_slice(&fs::read(original).unwrap()).unwrap();
                model["gptp"] = gptp["gptp"].clone();
                let path = f.0.join(format!("owner-{index}.json"));
                fs::write(&path, serde_json::to_vec(&model).unwrap()).unwrap();
                model_path = Some(path.clone());
                path
            } else {
                original
            };
            text = text.replace(&line, &format!("{key} = \"{}\"", replacement.display()));
        }
        assert!(model_path.is_some());
        let ini = f.0.join(format!("owner-{index}.ini"));
        fs::write(&ini, text).unwrap();
        let error = prepare(&ini).unwrap_err();
        assert!(
            error.message.contains("gPTP") || error.message.contains("PTP Link"),
            "{name}: {error}"
        );
    }
}

fn records(path: &Path) -> Vec<Value> {
    let result: Value =
        serde_json::from_slice(&fs::read(path.join("results.json")).unwrap()).unwrap();
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .clone()
}
fn relay_sync_sof(rows: &[Value]) -> u64 {
    rows.iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "physical_sof"
                && r["data"]["message_type"] == 0
                && r["data"]["transfer_id"]
                    .as_str()
                    .is_some_and(|v| v.contains("Net.b.tx_c"))
        })
        .map(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
        .find(|&time| time > 900_000_000)
        .unwrap()
}
fn relay_followup_correction(rows: &[Value]) -> i64 {
    rows.iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "packet_queued"
                && r["data"]["message_type"] == 8
                && r["data"]["transfer_id"]
                    .as_str()
                    .is_some_and(|v| v.contains("Net.b.tx_c"))
        })
        .find(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap() > 900_000_000)
        .map(|r| {
            let hex = r["data"]["details"]["wire_hex"].as_str().unwrap();
            let bytes = (22..30)
                .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
                .collect::<Vec<_>>();
            i64::from_be_bytes(bytes.try_into().unwrap())
        })
        .unwrap()
}

#[test]
fn three_node_relay_residence_and_tsn_change_physical_outcome() {
    let f = Fixture::new();
    let mut outputs = std::collections::BTreeMap::new();
    for name in [
        "three-node",
        "three-node-queue",
        "three-node-psfp-open",
        "three-node-psfp-closed",
        "three-node-tas",
    ] {
        let out = f.0.join(name);
        run(prepare(&f.named(name, 3_000_000_000)).unwrap(), &out).unwrap();
        outputs.insert(name, records(&out));
    }
    let base = &outputs["three-node"];
    let b_corrected = base.iter().any(|r| {
        r["schema_name"] == "dir.network.gptp"
            && r["data"]["kind"] == "clock_corrected"
            && r["data"]["details"]["node"] == "Net.b"
    });
    let c_corrected = base.iter().any(|r| {
        r["schema_name"] == "dir.network.gptp"
            && r["data"]["kind"] == "clock_corrected"
            && r["data"]["details"]["node"] == "Net.c"
    });
    assert!(
        b_corrected && c_corrected,
        "both relay and downstream must synchronize"
    );
    assert!(relay_sync_sof(&outputs["three-node-queue"]) > relay_sync_sof(base) + 4_000_000);
    assert!(
        relay_followup_correction(&outputs["three-node-queue"]) > relay_followup_correction(base)
    );
    assert_eq!(
        relay_sync_sof(&outputs["three-node-psfp-open"]),
        relay_sync_sof(&outputs["three-node-queue"])
    );
    assert_eq!(
        relay_sync_sof(&outputs["three-node-psfp-closed"]),
        relay_sync_sof(base)
    );
    assert!(relay_sync_sof(&outputs["three-node-tas"]) > relay_sync_sof(base) + 3_000_000);
}

#[test]
fn link_failure_changes_grandmaster_and_recovery_reselects_original() {
    let f = Fixture::new();
    let out = f.0.join("recovery");
    run(
        prepare(&f.named("three-node-recovery", 4_000_000_000)).unwrap(),
        &out,
    )
    .unwrap();
    let rows = records(&out);
    let selection = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.gptp" && r["data"]["kind"] == "selection")
        .map(|r| {
            (
                r["time_ps"].as_str().unwrap().parse::<u64>().unwrap(),
                r["data"]["details"]["node"].as_str().unwrap().to_owned(),
                r["data"]["details"]["grandmaster"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        selection.iter().any(|(at, node, gm)| *at == 1_200_000_000
            && node == "Net.b"
            && gm == "0000000000000002")
    );
    assert!(
        selection.iter().any(|(at, node, gm)| *at > 2_200_000_000
            && node == "Net.b"
            && gm == "0000000000000001")
    );
    assert!(
        selection.iter().any(|(at, node, gm)| *at > 2_200_000_000
            && node == "Net.c"
            && gm == "0000000000000001")
    );
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.gptp"
        && r["data"]["kind"] == "clock_corrected"
        && r["data"]["details"]["node"] == "Net.c"
        && r["data"]["details"]["grandmaster"] == "0000000000000002"));
    let timeout = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.network.gptp"
                && r["data"]["kind"] == "announce_timeout"
                && r["data"]["details"]["port"] == "Net.a.tx"
        })
        .unwrap();
    let at = timeout["time_ps"].as_str().unwrap();
    assert!(rows.iter().any(|r| {
        r["schema_name"] == "dir.network.gptp"
            && r["data"]["kind"] == "announce_qualified"
            && r["data"]["transfer_id"]
                .as_str()
                .is_some_and(|v| v.contains("Net.b.tx_a"))
            && r["data"]["details"]["deadline_ps"] == at
    }));
    // Raw-local protocol periods may move the next Announce off this global
    // instant. The recorded candidate still expires at its exact deadline;
    // the independent boundary test checks same-time FES ordering.
}

#[test]
fn causal_replay_rejects_omitted_announce_and_substituted_older_sample() {
    let f = Fixture::new();
    let p = prepare(&f.ini(3_000_000_000, None)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut corrections = 0;
    for _ in 0..10000 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        corrections = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .filter(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == "clock_corrected")
            .count();
        if corrections >= 9 {
            break;
        }
    }
    assert!(corrections >= 9);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let baseline = f.0.join("causal-baseline");
    session
        .checkpoint(&baseline, CheckpointLimits::default())
        .unwrap();
    restore_checkpoint(&baseline, registry.clone(), CheckpointLimits::default()).unwrap();

    let omitted = f.0.join("causal-omitted-announce");
    session
        .checkpoint(&omitted, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&omitted, |state| {
        let g = &mut state["network_backend"]["state"]["gptp"];
        let parent = g["journal"].as_array().unwrap().last().unwrap()["parent"]
            .as_u64()
            .unwrap() as usize;
        let transfer = g["ports"][parent]["candidate"]["transfer"]
            .as_u64()
            .unwrap();
        let causes = g["causes"].as_array_mut().unwrap();
        let before = causes.len();
        causes.retain(|cause| cause["kind"]["Receive"]["transfer"].as_u64() != Some(transfer));
        assert_eq!(causes.len() + 1, before);
    });
    let error = restore_checkpoint(&omitted, registry.clone(), CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(error.message.contains("causal"), "{}", error.message);

    let older = f.0.join("causal-older-sample");
    session
        .checkpoint(&older, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&older, |state| {
        let g = &mut state["network_backend"]["state"]["gptp"];
        let last = g["journal"].as_array().unwrap().last().unwrap();
        let used = last["pdelay_sample"].as_u64().unwrap() as usize;
        let direction = last["parent"].clone();
        let replacement = g["samples"]
            .as_array()
            .unwrap()
            .iter()
            .take(used)
            .enumerate()
            .rev()
            .find(|(_, sample)| sample["direction"] == direction && sample["rate"].is_object())
            .map(|(i, _)| i)
            .expect("a prior qualified physical sample");
        g["journal"].as_array_mut().unwrap().last_mut().unwrap()["pdelay_sample"] =
            replacement.into();
    });
    let error = restore_checkpoint(&older, registry, CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(
        error.message.contains("causal") || error.message.contains("Pdelay"),
        "{}",
        error.message
    );
}

#[test]
fn causal_replay_rejects_coordinated_nonwinning_announce_selection() {
    let f = Fixture::new();
    let p = prepare(&f.named("three-node-recovery", 2_000_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut received = false;
    for _ in 0..5000 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        received = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "announce_qualified"
                    && r.data["transfer_id"]
                        .as_str()
                        .is_some_and(|id| id.contains("Net.b.tx_a"))
            });
        if received {
            break;
        }
    }
    assert!(
        received,
        "the inferior Announce must have physically arrived"
    );
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let baseline = f.0.join("nonwinning-baseline");
    session
        .checkpoint(&baseline, CheckpointLimits::default())
        .unwrap();
    restore_checkpoint(&baseline, registry.clone(), CheckpointLimits::default()).unwrap();
    let forged = f.0.join("nonwinning-forged");
    session
        .checkpoint(&forged, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&forged, |state| {
        let network = &state["network_backend"]["state"];
        let parent = network["gptp"]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .find_map(|(direction, port)| {
                (port["candidate"]["source"]["clock"][7] == 2).then_some(direction)
            })
            .unwrap();
        let node = 0usize; // Net.a is node 0 in this fixed three-node fixture.
        assert_eq!(network["gptp"]["nodes"][node]["grandmaster"][7], 1);
        let g = &mut state["network_backend"]["state"]["gptp"];
        let candidate = g["ports"][parent]["candidate"].clone();
        assert!(candidate.is_object());
        let transfer = candidate["transfer"].as_u64().unwrap();
        let epoch = g["nodes"][node]["epoch"].as_u64().unwrap() + 1;
        let link_epoch = g["packets"][transfer.to_string()]["link_epoch"].clone();
        g["nodes"][node]["parent"] = (parent as u64).into();
        g["nodes"][node]["grandmaster"] = candidate["grandmaster"].clone();
        g["nodes"][node]["epoch"] = epoch.into();
        g["nodes"][node]["synchronized"] = false.into();
        g["nodes"][node]["sync_timeout_generation"] = (g["nodes"][node]["sync_timeout_generation"]
            .as_u64()
            .unwrap()
            + 1)
        .into();
        g["ports"][parent]["role"] = "Slave".into();
        g["ports"][parent]["information_source"] = "Received".into();
        g["selections"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "node": node,
                "at_ps": candidate["receipt_ps"],
                "epoch": epoch,
                "parent": parent,
                "grandmaster": candidate["grandmaster"],
                "candidate_transfer": transfer,
                "candidate_link_epoch": link_epoch,
            }));
    });
    let error = restore_checkpoint(&forged, registry, CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(error.message.contains("causal"), "{}", error.message);
}

#[test]
fn received_body_proof_covers_announce_followup_and_pdelay_fields() {
    let f = Fixture::new();
    let p = prepare(&f.ini(2_000_000_000, None)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut corrected = false;
    for _ in 0..5000 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        corrected = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == "clock_corrected");
        if corrected {
            break;
        }
    }
    assert!(corrected);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let baseline = f.0.join("received-body-baseline");
    session
        .checkpoint(&baseline, CheckpointLimits::default())
        .unwrap();
    restore_checkpoint(&baseline, registry.clone(), CheckpointLimits::default()).unwrap();

    for (kind, field) in [
        ("PdelayResp", "request_receipt"),
        ("PdelayRespFollowUp", "response_origin"),
        ("Announce", "priority1"),
        ("FollowUp", "precise_origin"),
    ] {
        let bundle = f.0.join(format!("received-body-{kind}"));
        session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        rewrite_checkpoint(&bundle, |state| {
            let packets = state["network_backend"]["state"]["gptp"]["packets"]
                .as_object_mut()
                .unwrap();
            let body = packets
                .values_mut()
                .filter(|p| p["rx_validated"] == true)
                .find_map(|p| p["received_message"]["body"].get_mut(kind))
                .expect("validated packet for each selected message type");
            let value = if kind == "Announce" {
                &mut body[field]
            } else {
                &mut body[field]["nanoseconds"]
            };
            *value = (value.as_u64().unwrap() + 1).into();
        });
        let error = restore_checkpoint(&bundle, registry.clone(), CheckpointLimits::default())
            .err()
            .unwrap();
        assert_eq!(error.code, "CheckpointInvalid", "{kind}");
        assert!(
            error.message.contains("received gPTP body")
                || error
                    .message
                    .contains("exact frozen-input committed prefix"),
            "{kind}: {}",
            error.message
        );
    }
}

#[test]
fn causal_replay_rejects_omitted_physical_link_loss() {
    let f = Fixture::new();
    let p = prepare(&f.named("three-node-recovery", 2_000_000_000)).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut seen_loss = false;
    for _ in 0..12000 {
        if session.advance_one().unwrap() != RunState::Running {
            break;
        }
        seen_loss = session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "link_transition"
                    && r.data["details"]["up"] == false
                    && r.time_ps == 1_200_000_000
            });
        if seen_loss {
            break;
        }
    }
    assert!(seen_loss);
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let baseline = f.0.join("link-baseline");
    session
        .checkpoint(&baseline, CheckpointLimits::default())
        .unwrap();
    restore_checkpoint(&baseline, registry.clone(), CheckpointLimits::default()).unwrap();
    let omitted = f.0.join("link-omitted");
    session
        .checkpoint(&omitted, CheckpointLimits::default())
        .unwrap();
    rewrite_checkpoint(&omitted, |state| {
        let causes = state["network_backend"]["state"]["gptp"]["causes"]
            .as_array_mut()
            .unwrap();
        let index = causes
            .iter()
            .position(|cause| cause["kind"]["Link"]["up"] == false)
            .unwrap();
        causes.remove(index);
    });
    let error = restore_checkpoint(&omitted, registry, CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(error.message.contains("causal"), "{}", error.message);
}

#[test]
fn packet_servo_composes_with_cbs_and_gcl_updates() {
    use serde_json::json;
    let f = Fixture::new();
    let mut model: Value = serde_json::from_slice(
        &fs::read(Fixture::root().join("two-node-servo-enabled-model.json")).unwrap(),
    )
    .unwrap();
    let tsn = &mut model["base"]["config"]["tsn"];
    tsn["outputs"][1]["cbs"] = json!([{"priority":7,"idle_slope_bps":"500000000",
        "hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
    let mut schedule = tsn["outputs"][1]["tas"].clone();
    schedule["id"] = json!("servo_reinstalled");
    tsn["gcl_updates"] = json!([{"id":"servo-reinstall","submitted_at_ps":"1500000000",
        "effective_at_ps":"1500000000","port":"Net.b.tx","schedule":schedule}]);
    let ini = f.ini(3_000_000_000, Some(&model));
    let text = fs::read_to_string(&ini).unwrap().replace(
        &Fixture::root()
            .join("two-node-workload.json")
            .display()
            .to_string(),
        &Fixture::root()
            .join("two-node-servo-workload.json")
            .display()
            .to_string(),
    );
    fs::write(&ini, text).unwrap();
    let out = f.0.join("servo-cbs-gcl");
    let report = run(prepare(&ini).unwrap(), &out).unwrap();
    assert_eq!(report.termination, "time_limit");
    let rows = records(&out);
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.gptp"
        && r["data"]["kind"] == "clock_corrected"
        && r["data"]["details"]["node"] == "Net.b"));
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "ethernet.tsn.credit" && r["data"]["port"] == "Net.b.tx")
    );
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["frame_id"] == "tas_probe:0"
                && r["data"]["status"] == "serialized")
    );
}

mod sampled_binding {
    use super::*;
    use dir_simulator::runtime::ethernet::phy::{
        Q30, SampleGrid,
        channel::{ChannelConfig, TransmitterConfig},
        ieee::t1_link,
        medium::*,
        receiver::*,
    };
    use serde_json::json;
    fn forced_pair() -> MediumConfig {
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
            gigabit_raw_merge: false,
            channel: ChannelConfig {
                grid,
                endpoints: endpoints.iter().map(|e| e.id.clone()).collect(),
                paths: vec![
                    vec![
                        None,
                        Some(dir_simulator::runtime::ethernet::phy::channel::Path {
                            delay_samples: 0,
                            taps_q30: vec![Q30],
                        }),
                    ],
                    vec![
                        Some(dir_simulator::runtime::ethernet::phy::channel::Path {
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
    fn model() -> Value {
        model_with_delay(4)
    }
    fn model_with_delay(delay_samples: usize) -> Value {
        let mut model: Value =
            serde_json::from_slice(&fs::read(Fixture::root().join("two-node-model.json")).unwrap())
                .unwrap();
        let mut medium = forced_pair();
        for row in &mut medium.channel.paths {
            for path in row.iter_mut().flatten() {
                path.delay_samples = delay_samples;
            }
        }
        model["phy_attachments"] = json!([{
            "origin_ps":0, "lookahead_samples":65536, "medium":medium,
            "bindings":[], "physical_ports":[
                {"endpoint":"p0","tx_port":"Net.a.tx","rx_port":"Net.a.rx"},
                {"endpoint":"p1","tx_port":"Net.b.tx","rx_port":"Net.b.rx"}
            ]
        }]);
        model
    }
    fn raw_model(delay: usize) -> Value {
        let mut raw: Value = serde_json::from_slice(
            &fs::read(Fixture::root().join("two-node-merge-model.json")).unwrap(),
        )
        .unwrap();
        raw["phy_attachments"] = model_with_delay(delay)["phy_attachments"].clone();
        raw["phy_attachments"][0]["medium"]["gigabit_raw_merge"] = true.into();
        raw
    }
    fn trace(rows: &[Value], plane: &str) {
        let gp: Vec<_> = rows
            .iter()
            .filter(|r| r["schema_name"] == "dir.network.gptp")
            .map(|r| &r["data"])
            .collect();
        for kind in ["pdelay_complete", "announce_qualified", "clock_corrected"] {
            assert!(gp.iter().any(|r| r["kind"] == kind), "missing {kind}");
        }
        let captures: Vec<_> = gp
            .iter()
            .filter(|r| r["kind"] == "tx_capture" || r["kind"] == "rx_capture")
            .collect();
        assert!(!captures.is_empty());
        assert!(
            captures
                .iter()
                .all(|r| r["details"]["sampled"]["plane"] == plane)
        );
        let mut delayed_rx = false;
        let mut fractional = false;
        for row in captures {
            let sampled = &row["details"]["sampled"];
            let reference = &sampled["reference"];
            let num: i128 = reference["num"].as_str().unwrap().parse().unwrap();
            let den: i128 = reference["den"].as_str().unwrap().parse().unwrap();
            let observed = sampled["observed_at_ps"].as_u64().unwrap();
            assert!(num <= i128::from(observed) * den);
            assert_eq!(row["global_ps"], observed.to_string());
            fractional |= den != 1;
            delayed_rx |=
                row["kind"] == "rx_capture" && i128::from(observed) * den > num + 1_000_000 * den;
            // This independent equation uses the immutable initial oscillator,
            // including captures observed after packet-derived servo updates.
            let source_b = row["transfer_id"].as_str().unwrap().contains("Net.b.tx");
            let node_b = if row["kind"] == "tx_capture" {
                source_b
            } else {
                !source_b
            };
            let (offset, rn, rd) = if node_b {
                (1_500_000i128, 10001i128, 10000i128)
            } else {
                (500_000, 1, 1)
            };
            let raw_num: i128 = row["details"]["raw_numerator"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let raw_den: i128 = row["details"]["raw_denominator"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(raw_num * den * rd, (offset * den * rd + num * rn) * raw_den);
        }
        assert!(
            delayed_rx,
            "RX timestamp must precede full decode observation"
        );
        assert!(
            fractional,
            "750MBd/16UI exact sample reference must survive without rounding"
        );
        let clocks: Vec<_> = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "dir.network.clock" && r["data"]["cause"] == "gptp_sync"
            })
            .collect();
        assert!(!clocks.is_empty());
        for row in clocks {
            let d = &row["data"];
            let n = d["local_numerator"]
                .as_str()
                .unwrap()
                .parse::<i128>()
                .unwrap();
            let den = d["local_denominator"]
                .as_str()
                .unwrap()
                .parse::<i128>()
                .unwrap();
            let global = d["global_ps"].as_str().unwrap().parse::<i128>().unwrap();
            assert!(
                (n - (global + 500_000) * den).abs() <= den,
                "sampled source-derived GM phase error exceeds1ps: {d}"
            );
        }
        for kind in [0, 2, 3, 8, 10] {
            assert!(
                gp.iter()
                    .any(|r| r["kind"] == "packet_received" && r["message_type"] == kind),
                "missing actual decoded gPTP message {kind}"
            );
        }
    }
    #[test]
    fn sampled_profile_admits_exact_named_and_raw_peers_and_rejects_uncertified_planes() {
        let f = Fixture::new();
        let model = model();
        assert!(prepare(&f.ini(6_000_000_000, Some(&model))).is_ok());
        let mut unbound_raw = model.clone();
        unbound_raw["phy_attachments"][0]["medium"]["gigabit_raw_merge"] = true.into();
        assert!(prepare(&f.ini(6_000_000_000, Some(&unbound_raw))).is_err());
        let mut raw: Value = serde_json::from_slice(
            &fs::read(Fixture::root().join("two-node-merge-model.json")).unwrap(),
        )
        .unwrap();
        raw["phy_attachments"] = unbound_raw["phy_attachments"].clone();
        assert!(prepare(&f.ini(6_000_000_000, Some(&raw))).is_ok());
        for field in ["noise", "clock", "tracking", "filter"] {
            let mut unsupported = model.clone();
            let m = &mut unsupported["phy_attachments"][0]["medium"];
            match field {
                "noise" => m["channel"]["noise_rms_uv"][0] = 1.into(),
                "clock" => m["endpoints"][0]["transmitter"]["clock_ppm"] = 1.into(),
                "tracking" => m["endpoints"][0]["receiver"]["max_tracking_ppm"] = 1.into(),
                "filter" => m["channel"]["paths"][0][1]["taps_q30"] = json!([Q30 / 2, Q30 / 2]),
                _ => unreachable!(),
            }
            let error = prepare(&f.ini(6_000_000_000, Some(&unsupported))).unwrap_err();
            assert!(
                error.message.contains("exact MDI timestamps"),
                "{field}: {error}"
            );
        }
    }

    fn process(bundle: &Path, expected: &Path, reject: bool) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sampled_binding::sampled_checkpoint_process_worker",
                "--nocapture",
            ])
            .env("DIR_SAMPLED_GP_BUNDLE", bundle)
            .env("DIR_SAMPLED_GP_EXPECTED", expected)
            .env("DIR_SAMPLED_GP_REJECT", if reject { "1" } else { "0" })
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    }
    #[test]
    fn sampled_checkpoint_process_worker() {
        let Some(bundle) = std::env::var_os("DIR_SAMPLED_GP_BUNDLE") else {
            return;
        };
        let restored = restore_checkpoint(
            Path::new(&bundle),
            dir_simulator::registry::Registry::default(),
            CheckpointLimits::default(),
        );
        if std::env::var("DIR_SAMPLED_GP_REJECT").unwrap() == "1" {
            let error = restored.err().expect("coforged sampled capture accepted");
            assert_eq!(error.code, "CheckpointInvalid");
            assert!(
                error
                    .message
                    .contains("exact frozen-input committed prefix")
                    || error.message.contains("causal prefix"),
                "{error:?}"
            );
            println!("sampled mutation rejected: {}", error.message);
            return;
        }
        let (mut restored, token) = restored.unwrap();
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        let expected: Value = serde_json::from_slice(
            &fs::read(std::env::var_os("DIR_SAMPLED_GP_EXPECTED").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(
                &restored
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
            )
            .unwrap(),
            expected
        );
    }
    #[test]
    #[ignore = "fresh processes replay actual sampled Clause97 startup for each source-bound capture mutant"]
    fn sampled_capture_checkpoint_is_source_bound_across_processes() {
        let f = Fixture::new();
        let root = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!("checkpoint-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let (mut live, controller) = RunSession::new(
            prepare(&f.ini(4_780_000_000, Some(&model()))).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut captured = false;
        for _ in 0..100_000 {
            if live.advance_one().unwrap() != RunState::Running {
                break;
            }
            captured = live
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "dir.network.gptp" && r.data["kind"] == "tx_capture");
            if captured {
                break;
            }
        }
        assert!(captured, "actual TX capture not reached");
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(live.poll_control().unwrap(), RunState::Paused);
        let bundle = root.join("positive");
        assert_eq!(
            live.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        // The actual callback contains TX capture while RX is still waiting for
        // decoder availability. Continue the very same owner for the oracle.
        live.resume(live.pause_token().unwrap()).unwrap();
        assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
        let expected = root.join("expected.json");
        fs::write(
            &expected,
            serde_json::to_vec(&live.snapshot().registered.as_ref().unwrap().model_records)
                .unwrap(),
        )
        .unwrap();
        process(&bundle, &expected, false);
        for mutant in [
            "sample-reference-raw",
            "observed-cause",
            "attempt",
            "generation",
            "epoch",
            "channel-mapping",
            "plane",
            "cause-order",
        ] {
            let mutated = root.join(mutant);
            fn copy_dir(from: &Path, to: &Path) {
                fs::create_dir_all(to).unwrap();
                for entry in fs::read_dir(from).unwrap() {
                    let entry = entry.unwrap();
                    let dest = to.join(entry.file_name());
                    if entry.path().is_dir() {
                        copy_dir(&entry.path(), &dest)
                    } else {
                        fs::copy(entry.path(), dest).unwrap();
                    }
                }
            }
            copy_dir(&bundle, &mutated);
            rewrite_checkpoint(&mutated, |state| {
                if mutant == "channel-mapping" {
                    let encoded: Vec<u8> =
                        serde_json::from_value(state["network_backend"]["state"]["phy"][0].clone())
                            .unwrap();
                    let mut physical: Value = serde_json::from_slice(&encoded).unwrap();
                    let delay = &mut physical["config"]["medium"]["channel"]["paths"][0][1]["delay_samples"];
                    *delay = (delay.as_u64().unwrap() + 1).into();
                    state["network_backend"]["state"]["phy"][0] =
                        json!(serde_json::to_vec(&physical).unwrap());
                    return;
                }
                let g = &mut state["network_backend"]["state"]["gptp"];
                let (transfer, packet) = g["packets"]
                    .as_object_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|(_, p)| p["direction"] == 0 && !p["tx"].is_null())
                    .unwrap();
                let transfer = transfer.parse::<u64>().unwrap();
                if mutant == "epoch" {
                    packet["link_epoch"] = 99.into();
                }
                let cap = &mut packet["tx"];
                match mutant {
                    "sample-reference-raw" => {
                        cap["sampled"]["reference_sample"] =
                            (cap["sampled"]["reference_sample"].as_u64().unwrap() - 12).into();
                        for path in ["reference", "raw"] {
                            let target = if path == "raw" {
                                &mut cap["raw"]
                            } else {
                                &mut cap["sampled"]["reference"]
                            };
                            target["num"] =
                                (target["num"].as_str().unwrap().parse::<i128>().unwrap()
                                    - 1000
                                        * target["den"].as_str().unwrap().parse::<i128>().unwrap())
                                .to_string()
                                .into();
                        }
                        cap["at_ps"] = (cap["at_ps"].as_u64().unwrap() - 1000).into();
                    }
                    "observed-cause" => {
                        cap["sampled"]["observed_at_ps"] =
                            (cap["sampled"]["observed_at_ps"].as_u64().unwrap() + 1).into()
                    }
                    "attempt" => cap["sampled"]["physical_attempt"] = 99.into(),
                    "generation" => cap["sampled"]["packet_generation"] = 99.into(),
                    "epoch" => cap["link_epoch"] = 99.into(),
                    "plane" => cap["sampled"]["plane"] = "RawDecodeAvailability".into(),
                    "cause-order" => {}
                    _ => unreachable!(),
                }
                let provenance = cap["sampled"].clone();
                let causes = g["causes"].as_array_mut().unwrap();
                let index = causes
                    .iter()
                    .position(|cause| cause["kind"]["SampledCapture"]["transfer"] == transfer)
                    .unwrap();
                causes[index]["kind"]["SampledCapture"]["provenance"] = provenance.clone();
                if mutant == "observed-cause" {
                    causes[index]["at_ps"] = provenance["observed_at_ps"].clone();
                }
                if mutant == "cause-order" {
                    assert!(index > 0);
                    causes.swap(index, index - 1);
                }
            });
            process(&mutated, &expected, true);
        }
    }

    #[test]
    #[ignore = "actual packet-derived quantization provenance with fresh-process continuation and mutants"]
    fn sampled_pdelay_quantization_provenance_is_source_bound() {
        let f = Fixture::new();
        let raw = std::env::var("DIR_SAMPLED_GP_RAW").as_deref() == Ok("1");
        let m = if raw {
            raw_model(0)
        } else {
            model_with_delay(0)
        };
        let root = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!(
                "quantization-{}-{}",
                if raw { "raw" } else { "canonical" },
                std::process::id()
            ));
        fs::create_dir_all(&root).unwrap();
        let (mut live, controller) = RunSession::new(
            prepare(&f.ini(6_000_000_000, Some(&m))).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut derived = false;
        for _ in 0..100_000 {
            if live.advance_one().unwrap() != RunState::Running {
                break;
            }
            derived = live
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| {
                    r.schema.name == "dir.network.gptp"
                        && r.data["kind"] == "pdelay_complete"
                        && !r.data["details"]["wire_quantization"].is_null()
                });
            if derived {
                break;
            }
        }
        assert!(derived);
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(live.poll_control().unwrap(), RunState::Paused);
        let bundle = root.join("positive");
        assert_eq!(
            live.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        live.resume(live.pause_token().unwrap()).unwrap();
        assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            live.export(&root.join("result")).unwrap().status,
            ControlStatus::Applied
        );
        let rows = records(&root.join("result"));
        trace(
            &rows,
            if raw {
                "MdiMergeDelimiter"
            } else {
                "MdiMacSfd"
            },
        );
        assert_physical_delay(&rows, 0);
        assert!(
            rows.iter()
                .filter(|r| r["schema_name"] == "dir.network.gptp"
                    && r["data"]["kind"] == "pdelay_complete"
                    && !r["data"]["details"]["neighbor_rate"].is_null())
                .all(|r| !r["data"]["details"]["wire_quantization"].is_null())
        );
        if !raw {
            assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.gptp"
                && r["data"]["details"]["wire_quantization"]["quantization_limited_zero"] == true));
        }
        let expected = root.join("expected.json");
        fs::write(
            &expected,
            serde_json::to_vec(&live.snapshot().registered.as_ref().unwrap().model_records)
                .unwrap(),
        )
        .unwrap();
        process(&bundle, &expected, false);
        fn copy_dir(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for entry in fs::read_dir(from).unwrap() {
                let e = entry.unwrap();
                let d = to.join(e.file_name());
                if e.path().is_dir() {
                    copy_dir(&e.path(), &d)
                } else {
                    fs::copy(e.path(), d).unwrap();
                }
            }
        }
        for mutant in ["changed", "removed"] {
            let b = root.join(mutant);
            copy_dir(&bundle, &b);
            rewrite_checkpoint(&b, |state| {
                let sample = state["network_backend"]["state"]["gptp"]["samples"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|sample| !sample["quantization"].is_null())
                    .unwrap();
                if mutant == "removed" {
                    sample.as_object_mut().unwrap().remove("quantization");
                } else {
                    let n = &mut sample["quantization"]["point_delay"]["num"];
                    *n = (n.as_str().unwrap().parse::<i128>().unwrap() + 1)
                        .to_string()
                        .into();
                }
            });
            process(&b, &expected, true);
        }
    }
    #[test]
    #[ignore = "actual sampled readiness withdrawal and recovery, idle and active, with fresh-process epoch mutants"]
    fn sampled_physical_fault_withdraws_gptp_epoch_until_actual_retraining() {
        for active in [false, true] {
            let f = Fixture::new();
            let mut m = model();
            m["phy_attachments"][0]["medium"]["maximum_samples"] = 200_000_000.into();
            let root = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
                .map(PathBuf::from)
                .unwrap_or_else(|| f.0.clone())
                .join(format!(
                    "fault-{}-{}",
                    if active { "active" } else { "idle" },
                    std::process::id()
                ));
            fs::create_dir_all(&root).unwrap();
            let generators = if active {
                json!([{"id":"active_fault","node":"Net.a","kind":"ethernet.explicit.v1",
                "times_ps":["5795000000"],"frame":{"dst_mac":"02:00:00:00:00:02","ether_type":2048,
                "data":"aa".repeat(1400),"tag":null,"ip_multicast":null},"flow_id":"active_fault","priority":0,"deadline_ps":null}])
            } else {
                json!([])
            };
            let work = root.join("workload.json");
            fs::write(&work, serde_json::to_vec(&json!({"schema_version":4,"generators":generators,"controls":[
                {"id":"fault","at_ps":"5800000000","kind":"link_set","link":"a-b","up":false},
                {"id":"restore-policy","at_ps":"5820000000","kind":"link_set","link":"a-b","up":true}]})).unwrap()).unwrap();
            let ini = f.ini(12_000_000_000, Some(&m));
            fs::write(
                &ini,
                fs::read_to_string(&ini).unwrap().replace(
                    &Fixture::root()
                        .join("two-node-workload.json")
                        .display()
                        .to_string(),
                    &work.display().to_string(),
                ),
            )
            .unwrap();
            let (mut live, controller) = RunSession::new(
                prepare(&ini).unwrap(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut waiting_for_retraining = false;
            for _ in 0..200_000 {
                if live.advance_one().unwrap() != RunState::Running {
                    break;
                }
                waiting_for_retraining = live
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .values()
                    .any(|r| {
                        r.schema.name == "ethernet.dynamic.control"
                            && r.data["control_id"] == "restore-policy"
                    });
                // The public control row names the control in `id` on older adapters.
                waiting_for_retraining |= live
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .values()
                    .any(|r| {
                        r.schema.name == "ethernet.dynamic.control"
                            && r.data["id"] == "restore-policy"
                    });
                if waiting_for_retraining {
                    break;
                }
            }
            assert!(
                waiting_for_retraining,
                "policy restoration control was not reached"
            );
            controller.submit(ControlCommand::Pause).unwrap();
            assert_eq!(live.poll_control().unwrap(), RunState::Paused);
            let bundle = root.join("positive");
            live.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap();
            let paused: Value =
                serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
            let gp_state = &paused["network_backend"]["state"]["gptp"];
            assert_eq!(gp_state["physical_links"]["0"]["ready"], false);
            for port in gp_state["ports"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| !p.is_null())
            {
                assert_eq!(port["link_up"], false);
                assert_eq!(port["as_capable"], false);
                for field in ["pending", "candidate", "rate", "delay"] {
                    assert!(
                        port[field].is_null(),
                        "old epoch {field} survived readiness withdrawal"
                    );
                }
            }
            assert!(
                gp_state["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|n| n["parent"].is_null())
            );
            assert_eq!(gp_state["nodes"][1]["synchronized"], false);
            live.resume(live.pause_token().unwrap()).unwrap();
            assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
            live.export(&root.join("result")).unwrap();
            let rows = records(&root.join("result"));
            trace(&rows, "MdiMacSfd");
            let gp: Vec<_> = rows
                .iter()
                .filter(|r| r["schema_name"] == "dir.network.gptp")
                .map(|r| &r["data"])
                .collect();
            let mut physical: Vec<_> = gp
                .iter()
                .filter(|r| r["kind"] == "physical_readiness")
                .collect();
            physical.sort_by_key(|r| r["global_ps"].as_str().unwrap().parse::<u64>().unwrap());
            assert_eq!(
                physical.len(),
                3,
                "initial readiness, withdrawal, and recovered readiness"
            );
            for (i, p) in physical.iter().enumerate() {
                assert_eq!(p["details"]["physical_epoch"], i.to_string());
                assert_eq!(p["details"]["ready"], i != 1);
            }
            let recovered = physical[2]["global_ps"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert!(
                recovered > 5_820_000_000,
                "policy up must precede physical retraining"
            );
            for port in ["Net.a.tx", "Net.b.tx"] {
                let links: Vec<_> = gp
                    .iter()
                    .filter(|r| r["kind"] == "link_transition" && r["details"]["port"] == port)
                    .collect();
                assert!(
                    links
                        .iter()
                        .any(|r| r["details"]["up"] == false && r["global_ps"] == "5800000000")
                );
                assert!(
                    links
                        .iter()
                        .filter(|r| r["details"]["up"] == true)
                        .all(|r| r["global_ps"].as_str().unwrap().parse::<u64>().unwrap()
                            >= recovered)
                );
            }
            assert!(gp.iter().any(|r| r["kind"] == "clock_corrected"
                && r["global_ps"].as_str().unwrap().parse::<u64>().unwrap() > recovered));
            if active {
                assert!(
                    rows.iter()
                        .any(|r| r["schema_name"] == "ethernet.dynamic.transfer"
                            && r["data"]["frame_id"]
                                .as_str()
                                .is_some_and(|f| f.starts_with("active_fault:"))
                            && r["data"]["sof_ps"]
                                .as_str()
                                .is_some_and(|at| at.parse::<u64>().unwrap() < 5_800_000_000)
                            && r["data"]["status"] == "dropped"),
                    "fault must interrupt an actual active frame"
                );
            }
            let expected = root.join("expected.json");
            fs::write(
                &expected,
                serde_json::to_vec(&live.snapshot().registered.as_ref().unwrap().model_records)
                    .unwrap(),
            )
            .unwrap();
            process(&bundle, &expected, false);
            let mutant = root.join("epoch-mutant");
            fn copy_dir(from: &Path, to: &Path) {
                fs::create_dir_all(to).unwrap();
                for e in fs::read_dir(from).unwrap() {
                    let e = e.unwrap();
                    if e.path().is_dir() {
                        copy_dir(&e.path(), &to.join(e.file_name()));
                    } else {
                        fs::copy(e.path(), to.join(e.file_name())).unwrap();
                    }
                }
            }
            copy_dir(&bundle, &mutant);
            rewrite_checkpoint(&mutant, |state| {
                let gp = &mut state["network_backend"]["state"]["gptp"];
                gp["physical_links"]["0"]["epoch"] = 2.into();
                for cause in gp["causes"].as_array_mut().unwrap() {
                    if cause["kind"]["PhysicalReadiness"]["ready"] == false {
                        cause["kind"]["PhysicalReadiness"]["epoch"] = 2.into();
                    }
                }
            });
            process(&mutant, &expected, true);
        }
    }
    fn selected_clock(rows: &[Value], global: u64) -> dir_simulator::runtime::NodeClock {
        use dir_simulator::runtime::{LocalTime, NodeClock};
        let anchor = rows
            .iter()
            .filter(|r| r["schema_name"] == "dir.network.clock" && r["data"]["node"] == "Net.b")
            .map(|r| &r["data"])
            .filter(|d| d["global_ps"].as_str().unwrap().parse::<u64>().unwrap() <= global)
            .max_by_key(|d| d["generation"].as_str().unwrap().parse::<u64>().unwrap())
            .unwrap();
        let number = |key: &str| anchor[key].as_str().unwrap().parse::<u64>().unwrap();
        let mut clock = NodeClock::new(0, 0, 1, 1).unwrap();
        clock
            .reanchor_and_rate(
                number("global_ps"),
                LocalTime::fraction(
                    anchor["local_numerator"].as_str().unwrap().parse().unwrap(),
                    number("local_denominator"),
                )
                .unwrap(),
                number("rate_num"),
                number("rate_den"),
            )
            .unwrap();
        clock
    }
    #[test]
    #[ignore = "actual sampled gPTP servo with concurrent TAS/PSFP/CBS and a GCL replacement"]
    fn sampled_packet_servo_drives_local_tas_psfp_cbs_and_gcl_update() {
        let f = Fixture::new();
        let mut m = model();
        m["physical_mac_policy"] = "dir.experimental.sampled_wire_cbs.v1".into();
        let tsn = &mut m["base"]["config"]["tsn"];
        let schedule = |id: &str, open: u64| {
            json!({"id":id,"base_time_ps":"0","cycle_time_ps":"100000000",
            "entries":[{"duration_ps":open.to_string(),"open_priorities":[0,7]},
                {"duration_ps":(100000000-open).to_string(),"open_priorities":[7]}]})
        };
        tsn["outputs"][1]["tas"] = schedule("sampled_servo_initial", 20_000_000);
        tsn["outputs"][1]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
        tsn["streams"] = json!([{"id":"sampled_servo_psfp","ingress":"Net.b.rx","dst_mac":"02:00:00:00:00:02",
            "vid":10,"priority":7,"max_sdu_bytes":"64","gate":{"base_time_ps":"0","cycle_time_ps":"5000000",
                "entries":[{"duration_ps":"2500000","open":true},{"duration_ps":"2500000","open":false}]},"meter":null}]);
        tsn["gcl_updates"] = json!([{"id":"sampled-servo-update","submitted_at_ps":"5690000000","effective_at_ps":"5700000000",
            "port":"Net.b.tx","schedule":schedule("sampled_servo_replaced",30_000_000)}]);
        let generator = |id: &str, node: &str, dst: &str, count: u64, step: u64| {
            json!({"id":id,"node":node,"kind":"ethernet.explicit.v1",
            "times_ps":(0..count).map(|k|(5_550_000_000+k*step).to_string()).collect::<Vec<_>>(),
            "frame":{"dst_mac":dst,"ether_type":2048,"data":"","tag":null,"ip_multicast":null},"flow_id":id,"priority":0,"deadline_ps":null})
        };
        let workload = json!({"schema_version":4,"generators":[generator("physical_psfp","Net.a","02:00:00:00:00:02",36,3_600_000),
            generator("physical_tas","Net.b","02:00:00:00:00:01",12,1_000_000)],"controls":[]});
        let workload_path = f.0.join("physical-workload.json");
        fs::write(&workload_path, serde_json::to_vec(&workload).unwrap()).unwrap();
        let ini = f.ini(6_800_000_000, Some(&m));
        fs::write(
            &ini,
            fs::read_to_string(&ini).unwrap().replace(
                &Fixture::root()
                    .join("two-node-workload.json")
                    .display()
                    .to_string(),
                &workload_path.display().to_string(),
            ),
        )
        .unwrap();
        let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!("application-{}", std::process::id()));
        let report = run(prepare(&ini).unwrap(), &out).unwrap();
        assert_eq!(report.termination, "time_limit");
        let rows = records(&out);
        trace(&rows, "MdiMacSfd");
        let mut passed = 0;
        let mut filtered = 0;
        let mut changed_by_servo = 0;
        let first_servo = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "dir.network.clock"
                    && r["data"]["node"] == "Net.b"
                    && r["data"]["cause"] == "gptp_sync"
            })
            .map(|r| {
                r["data"]["global_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            })
            .min()
            .unwrap();
        for row in rows.iter().filter(|r| {
            r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["frame_id"]
                    .as_str()
                    .is_some_and(|f| f.starts_with("physical_psfp:"))
        }) {
            let d = &row["data"];
            let at = d["observed_ps"].as_str().unwrap().parse::<u64>().unwrap();
            if at < first_servo {
                continue;
            }
            let local = selected_clock(&rows, at).local_at(at).unwrap();
            let den = i128::from(local.denominator());
            let open = local.numerator().rem_euclid(5_000_000 * den) < 2_500_000 * den;
            assert_eq!(
                d["status"] == "received",
                open,
                "actual sampled PSFP at{at}: {d}"
            );
            let old_num = i128::from(at) * 10001 + 1_500_000 * 10000;
            let old_open = old_num.rem_euclid(5_000_000 * 10000) < 2_500_000 * 10000;
            changed_by_servo += usize::from(open != old_open);
            if open {
                passed += 1
            } else {
                filtered += 1;
                assert_eq!(d["reason"], "psfp_gate_closed");
            }
        }
        assert!(
            passed > 0 && filtered > 0 && changed_by_servo > 0,
            "PSFP outcomes:pass{passed},filter{filtered},servochanged{changed_by_servo}"
        );
        let mut started = 0;
        for row in rows.iter().filter(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["frame_id"]
                    .as_str()
                    .is_some_and(|f| f.starts_with("physical_tas:"))
        }) {
            let Some(at) = row["data"]["sof_ps"]
                .as_str()
                .map(|s| s.parse::<u64>().unwrap())
            else {
                continue;
            };
            let local = selected_clock(&rows, at).local_at(at).unwrap();
            let den = i128::from(local.denominator());
            let open = if at < 5_700_000_000 {
                20_000_000
            } else {
                30_000_000
            };
            assert!(
                local.numerator().rem_euclid(100_000_000 * den) < open * den,
                "TAS actual start outside corrected local gate: {row}"
            );
            started += 1;
        }
        assert!(started > 0, "sampled TAS class never starts");
        assert!(
            rows.iter()
                .any(|r| r["schema_name"] == "ethernet.tsn.credit"
                    && r["data"]["port"] == "Net.b.tx")
        );
        let reopened = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "ethernet.tsn.gate"
                    && r["data"]["port"] == "Net.b.tx"
                    && r["data"]["cause"] == "boundary"
                    && r["data"]["open_priorities"]
                        .as_array()
                        .is_some_and(|p| p.iter().any(|p| p == "0"))
            })
            .map(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .filter(|&at| at > first_servo)
            .min()
            .unwrap();
        let reopened_local = selected_clock(&rows, reopened).local_at(reopened).unwrap();
        let den = i128::from(reopened_local.denominator());
        assert!(
            reopened_local.numerator().rem_euclid(100_000_000 * den) <= den,
            "pending CBS/TAS work must wake at the corrected local cycle boundary"
        );
        let old_phase =
            (i128::from(reopened) * 10001 + 1_500_000 * 10000).rem_euclid(100_000_000 * 10000);
        assert!(
            old_phase > 1_000_000 * 10000,
            "this boundary must distinguish the corrected and initial calendars"
        );
        let mut credit_sends = 0;
        let mut credit_freezes = 0;
        for row in rows.iter().filter(|r| {
            r["schema_name"] == "ethernet.tsn.credit"
                && r["data"]["port"] == "Net.b.tx"
                && r["data"]["priority"] == "0"
        }) {
            let d = &row["data"];
            let at = row["time_ps"].as_str().unwrap().parse::<u64>().unwrap();
            if at <= first_servo {
                continue;
            }
            let local = selected_clock(&rows, at).local_at(at).unwrap();
            let den = i128::from(local.denominator());
            let width = if at < 5_700_000_000 {
                20_000_000
            } else {
                30_000_000
            };
            let open = local.numerator().rem_euclid(100_000_000 * den) < width * den;
            let slope = d["slope_bps"].as_str().unwrap().parse::<i128>().unwrap();
            if slope > 0 {
                assert!(
                    open,
                    "CBS accumulation must use the corrected local gate: {row}"
                );
            }
            if !open {
                assert!(
                    slope <= 0,
                    "closed corrected local gate cannot accumulate credit: {row}"
                );
            }
            credit_sends += usize::from(slope < 0);
            credit_freezes += usize::from(!open && slope == 0);
        }
        assert!(
            credit_sends > 0 && credit_freezes > 0,
            "CBS must send and freeze after packet correction"
        );
        assert!(rows.iter().any(|r| r["schema_name"] == "ethernet.tsn.gate"
            && r["data"]["schedule_id"] == "sampled_servo_replaced"));
    }
    fn assert_physical_delay(rows: &[Value], delay_samples: u64) {
        let captures: Vec<_> = rows
            .iter()
            .filter(|r| r["schema_name"] == "dir.network.gptp")
            .map(|r| &r["data"])
            .collect();
        let tx: std::collections::BTreeMap<_, _> = captures
            .iter()
            .filter(|r| r["kind"] == "tx_capture")
            .map(|r| (r["transfer_id"].as_str().unwrap(), *r))
            .collect();
        let mut checked = 0;
        for rx in captures.iter().filter(|r| r["kind"] == "rx_capture") {
            let tx = tx[rx["transfer_id"].as_str().unwrap()];
            let r = &rx["details"]["sampled"]["reference"];
            let t = &tx["details"]["sampled"]["reference"];
            let n = |v: &Value| v.as_str().unwrap().parse::<i128>().unwrap();
            assert_eq!(
                (n(&r["num"]) * n(&t["den"]) - n(&t["num"]) * n(&r["den"])) * 3,
                i128::from(delay_samples) * 250 * n(&r["den"]) * n(&t["den"]),
                "source-mapped actual physical propagation"
            );
            checked += 1;
        }
        assert!(checked > 10);
    }
    #[test]
    #[ignore = "actual Clause97 startup with known whole-UI physical path delay"]
    fn actual_named_phy_gptp_whole_ui_path_has_exact_propagation() {
        let f = Fixture::new();
        let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!("whole-ui-{}", std::process::id()));
        let report = run(
            prepare(&f.ini(6_000_000_000, Some(&model_with_delay(16)))).unwrap(),
            &out,
        )
        .unwrap();
        assert_eq!(report.termination, "time_limit");
        let rows = records(&out);
        trace(&rows, "MdiMacSfd");
        assert_physical_delay(&rows, 16);
    }

    #[test]
    #[ignore = "actual fixed-receiver electrical failure at the PAM2 phase edge; no protocol capture may be invented"]
    fn sampled_fixed_receiver_phase_edge_preserves_training_failure() {
        let f = Fixture::new();
        let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!("training-edge-15-{}", std::process::id()));
        let report = run(
            prepare(&f.ini(6_000_000_000, Some(&model_with_delay(15)))).unwrap(),
            &out,
        )
        .unwrap();
        assert_eq!(report.termination, "time_limit");
        let rows = records(&out);
        assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.phy"
            && r["data"]["kind"] == "PcsControl"
            && r["data"]["command"] == "link_fault:PAM2 training level is not binary"));
        assert!(rows.iter().all(|r| r["schema_name"] != "dir.network.gptp"
            || !matches!(
                r["data"]["kind"].as_str(),
                Some("tx_capture" | "rx_capture" | "clock_corrected")
            )));
    }
    #[test]
    #[ignore = "actual Clause97 startup at zero and one sample beyond a UI delay boundary"]
    fn sampled_canonical_known_delay_matrix() {
        let cases = std::env::var("DIR_SAMPLED_GP_DELAY_CASES")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|x| x.parse::<usize>().unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![0, 17]);
        for delay in cases {
            let f = Fixture::new();
            let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
                .map(PathBuf::from)
                .unwrap_or_else(|| f.0.clone())
                .join(format!("delay-{delay}-{}", std::process::id()));
            let report = run(
                prepare(&f.ini(6_000_000_000, Some(&model_with_delay(delay)))).unwrap(),
                &out,
            )
            .unwrap();
            assert_eq!(report.termination, "time_limit");
            let rows = records(&out);
            trace(&rows, "MdiMacSfd");
            assert_physical_delay(&rows, delay as u64);
        }
    }

    #[test]
    #[ignore = "actual raw Clause97 startup at zero and around a UI delay boundary"]
    fn sampled_raw_known_delay_matrix() {
        let cases = std::env::var("DIR_SAMPLED_GP_DELAY_CASES")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|x| x.parse::<usize>().unwrap())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![0, 16, 17]);
        for delay in cases {
            let f = Fixture::new();
            let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
                .map(PathBuf::from)
                .unwrap_or_else(|| f.0.clone())
                .join(format!("raw-delay-{delay}-{}", std::process::id()));
            let report = run(
                prepare(&f.ini(6_000_000_000, Some(&raw_model(delay)))).unwrap(),
                &out,
            )
            .unwrap();
            assert_eq!(report.termination, "time_limit");
            let rows = records(&out);
            trace(&rows, "MdiMergeDelimiter");
            assert_physical_delay(&rows, delay as u64);
        }
    }
    #[test]
    #[ignore = "requires actual mapped raw Merge TX/RX delimiter timestamps and full Clause97 startup"]
    fn actual_raw_merge_gptp_uses_mapped_mdi_delimiters() {
        let f = Fixture::new();
        let raw = raw_model(4);
        let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!("raw-{}", std::process::id()));
        let report = run(prepare(&f.ini(6_000_000_000, Some(&raw))).unwrap(), &out).unwrap();
        assert_eq!(report.termination, "time_limit");
        let rows = records(&out);
        trace(&rows, "MdiMergeDelimiter");
        assert_physical_delay(&rows, 4);
        let clocks: Vec<_> = rows
            .iter()
            .filter(|r| {
                r["schema_name"] == "dir.network.clock"
                    && r["data"]["node"] == "Net.b"
                    && r["data"]["cause"] == "gptp_sync"
            })
            .map(|r| &r["data"])
            .collect();
        assert!(
            rows.iter()
                .filter(|r| r["schema_name"] == "dir.network.gptp"
                    && r["data"]["kind"] == "rx_capture"
                    && r["data"]["transfer_id"]
                        .as_str()
                        .unwrap()
                        .contains("Net.a.tx"))
                .any(|r| {
                    let d = &r["data"];
                    let s = &d["details"]["sampled"];
                    let n = s["reference"]["num"]
                        .as_str()
                        .unwrap()
                        .parse::<i128>()
                        .unwrap();
                    let den = s["reference"]["den"]
                        .as_str()
                        .unwrap()
                        .parse::<i128>()
                        .unwrap();
                    clocks.iter().any(|c| {
                        let at = c["global_ps"].as_str().unwrap().parse::<u64>().unwrap();
                        n < i128::from(at) * den
                            && at <= s["observed_at_ps"].as_u64().unwrap()
                            && c["effect_seq"].as_str().unwrap().parse::<u64>().unwrap()
                                < d["effect_seq"].as_str().unwrap().parse::<u64>().unwrap()
                    })
                }),
            "rawRX references before an intervening servo must keep initial oscillator law"
        );
    }
    #[test]
    #[ignore = "runs actual continuous sampled Clause97 startup and packet-derived synchronization"]
    fn actual_named_phy_gptp_uses_exact_markers_and_delayed_raw_oscillator() {
        let f = Fixture::new();
        let out = std::env::var_os("DIR_SAMPLED_GPTP_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.0.clone())
            .join(format!("actual-{}", std::process::id()));
        let report = run(
            prepare(&f.ini(6_000_000_000, Some(&model()))).unwrap(),
            &out,
        )
        .unwrap();
        assert_eq!(report.termination, "time_limit");
        let rows = records(&out);
        trace(&rows, "MdiMacSfd");
        assert_physical_delay(&rows, 4);
    }
}
