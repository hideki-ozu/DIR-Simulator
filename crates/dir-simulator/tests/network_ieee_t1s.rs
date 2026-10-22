//! The named electrical domain drives real per-station network ingress.
use dir_simulator::registry::Registry;
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite/t1s-multidrop")
}
fn scratch() -> PathBuf {
    let root = std::env::var_os("DIR_T1S_NETWORK_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "dir-t1s-network-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&root).unwrap();
    root
}
fn result(root: &std::path::Path, folder: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(folder).join("results.json")).unwrap()).unwrap()
}
fn canonical(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("run_id");
    value.as_object_mut().unwrap().remove("control");
    for field in ["started_at_utc", "finished_at_utc"] {
        value["metadata"].as_object_mut().unwrap().remove(field);
    }
    value
}

#[test]
fn one_actual_transmission_reaches_two_decoders_and_one_destination() {
    let root = scratch();
    let (mut session, _) = RunSession::new(
        prepare(&fixture().join("model.ini")).unwrap(),
        RunOptions::default(),
    )
    .unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
    if std::env::var_os("DIR_T1S_NETWORK_EVIDENCE").is_some() {
        let observed = session.observe().unwrap();
        fs::write(
            root.join("committed-network-records.json"),
            serde_json::to_vec(&observed.registered.as_ref().unwrap().model_records).unwrap(),
        )
        .unwrap();
    }
    let ack = session.export(&root.join("result")).unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    let value = result(&root, "result");
    assert_eq!(value["simulation"]["termination"], "time_limit");
    let rows = value["simulation"]["model_records"].as_array().unwrap();
    let physical: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.phy")
        .collect();
    let starts: Vec<_> = physical
        .iter()
        .filter(|r| r["data"]["kind"] == "TxStarted")
        .collect();
    assert_eq!(starts.len(), 1, "one electrical MAC attempt");
    let mut ingress: Vec<_> = physical
        .iter()
        .filter(|r| r["data"]["kind"] == "decoded_ingress")
        .collect();
    ingress.sort_by_key(|r| r["data"]["receiver_port"].as_str().unwrap());
    assert_eq!(ingress.len(), 2);
    for (row, expected) in ingress.iter().zip(["Net.s2.rx_b", "Net.s3.rx_b"]) {
        assert_eq!(row["data"]["receiver_port"], expected);
        let sfd: u64 = row["data"]["rx_sfd_sample"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let fcs: u64 = row["data"]["rx_fcs_sample"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(sfd < fcs);
        assert_eq!(row["data"]["physical_attempt"]["attempt"], 0);
        assert_eq!(row["data"]["physical_attempt"]["source_endpoint"], "p0");
    }
    let received: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.reception")
        .collect();
    for physical in &ingress {
        let observations: Vec<_> = received
            .iter()
            .filter(|r| r["data"]["ingress"] == physical["data"]["receiver_port"])
            .collect();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0]["data"]["observed_ps"], physical["time_ps"]);
        assert!(
            observations[0]["record_id"]
                .as_str()
                .unwrap()
                .contains("@phy")
        );
    }
    assert!(!received.iter().any(|r| {
        r["data"]["ingress"]
            .as_str()
            .is_some_and(|port| port.starts_with("Net.hub."))
    }));
    assert_eq!(
        received
            .iter()
            .filter(|r| r["data"]["ingress"] == "Net.b.rx" && r["data"]["status"] == "received")
            .count(),
        1
    );
    if std::env::var_os("DIR_T1S_NETWORK_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn named_phy_binding_rejects_ambiguous_domain_and_wrong_carrier() {
    let root = scratch();
    let source = fixture();
    let ini = fs::read_to_string(source.join("model.ini"))
        .unwrap()
        .replace(
            "ned-path = \"models\"",
            &format!("ned-path = \"{}\"", source.join("models").display()),
        );
    fs::write(root.join("model.ini"), ini).unwrap();
    fs::copy(source.join("workload.json"), root.join("workload.json")).unwrap();
    let original: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    for case in ["duplicate", "rx", "carrier", "missing-port"] {
        let mut value = original.clone();
        let cfg = &mut value["phy_attachments"][0];
        match case {
            "duplicate" => cfg["physical_ports"][2]["tx_port"] = "Net.s1.tx_b".into(),
            "rx" => cfg["physical_ports"][1]["rx_port"] = "Net.s3.rx_b".into(),
            "carrier" => cfg["physical_ports"][1]["tx_port"] = "Net.b.tx".into(),
            "missing-port" => {
                cfg["physical_ports"].as_array_mut().unwrap().pop();
            }
            _ => unreachable!(),
        }
        fs::write(root.join("model.json"), value.to_string()).unwrap();
        assert!(prepare(&root.join("model.ini")).is_err(), "accepted {case}");
    }
    if std::env::var_os("DIR_T1S_NETWORK_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

fn child(root: &Path, mode: &str, cut: u64) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "physical_checkpoint_process_worker",
            "--nocapture",
        ])
        .env("DIR_T1S_WORKER", mode)
        .env("DIR_T1S_ROOT", root)
        .env("DIR_T1S_CUT", cut.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode} at {cut}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn physical_checkpoint_process_worker() {
    let Ok(mode) = std::env::var("DIR_T1S_WORKER") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_T1S_ROOT").unwrap());
    if mode == "save" {
        let cut: u64 = std::env::var("DIR_T1S_CUT").unwrap().parse().unwrap();
        let (mut session, controller) = RunSession::new(
            prepare(&fixture().join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        while cut > 0 && session.snapshot().common.last_event_time_ps.unwrap_or(0) < cut {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let ack = session
            .checkpoint(&root.join("bundle"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        session.resume(session.pause_token().unwrap()).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            session.export(&root.join("original")).unwrap().status,
            ControlStatus::Applied
        );
    } else {
        let (mut session, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        session.resume(token).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            session.export(&root.join("restored")).unwrap().status,
            ControlStatus::Applied
        );
    }
}
#[test]
fn electrical_prefix_resumes_in_a_new_process_across_actual_receive() {
    // Uninitialized, before SFD, mid-payload and after actual FCS/ESD reception.
    for cut in [0, 1, 5_000_000, 25_000_000, 65_000_000] {
        let root = scratch();
        child(&root, "save", cut);
        child(&root, "restore", cut);
        assert_eq!(
            canonical(result(&root, "original")),
            canonical(result(&root, "restored")),
            "cut {cut}"
        );
        if std::env::var_os("DIR_T1S_NETWORK_EVIDENCE").is_none() {
            fs::remove_dir_all(root).unwrap();
        }
    }
}
fn rewrite(bundle: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let mut state: Value =
        serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(bundle.join("state.json"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["path"] == "state.json")
        .unwrap();
    entry["bytes"] = json!(bytes.len().to_string());
    entry["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(bundle.join("manifest.json"), manifest.to_string()).unwrap();
}
#[test]
fn physical_and_network_ingress_proofs_cannot_be_rebound() {
    let root = scratch();
    child(&root, "save", 65_000_000);
    let bundle = root.join("bundle");
    let original = fs::read(bundle.join("state.json")).unwrap();
    let manifest = fs::read(bundle.join("manifest.json")).unwrap();
    for mutation in [
        "receiver",
        "time",
        "missing-reception",
        "physical-receiver",
        "physical-time",
        "missing-proof",
    ] {
        fs::write(bundle.join("state.json"), &original).unwrap();
        fs::write(bundle.join("manifest.json"), &manifest).unwrap();
        rewrite(&bundle, |value| {
            let network = &mut value["network_backend"]["state"];
            if mutation.starts_with("physical-") || mutation == "missing-proof" {
                let bytes: Vec<u8> = serde_json::from_value(network["phy"][0].clone()).unwrap();
                let mut medium: Value = serde_json::from_slice(&bytes).unwrap();
                let history = &mut medium["physical_ingress_history"];
                assert_eq!(history.as_array().unwrap().len(), 2);
                match mutation {
                    "physical-receiver" => history[0]["receiver_endpoint"] = json!("p0"),
                    "physical-time" => {
                        let sample = history[0]["completed_sample"].as_u64().unwrap();
                        history[0]["completed_sample"] = json!(sample + 1);
                    }
                    "missing-proof" => {
                        history.as_array_mut().unwrap().pop();
                    }
                    _ => unreachable!(),
                }
                network["phy"][0] = json!(serde_json::to_vec(&medium).unwrap());
            } else {
                let receptions = network["core"]["receptions"].as_array_mut().unwrap();
                let i = receptions
                    .iter()
                    .position(|r| {
                        r["row"]["reception_id"]
                            .as_str()
                            .is_some_and(|id| id.contains("@phy"))
                    })
                    .unwrap();
                match mutation {
                    "receiver" => receptions[i]["row"]["ingress"] = json!("Net.s1.rx_b"),
                    "time" => {
                        let t = receptions[i]["row"]["observed_ps"].as_u64().unwrap();
                        receptions[i]["row"]["observed_ps"] = json!(t + 1);
                    }
                    "missing-reception" => {
                        receptions.remove(i);
                    }
                    _ => unreachable!(),
                }
            }
        });
        assert!(
            restore_checkpoint(&bundle, Registry::default(), CheckpointLimits::default()).is_err(),
            "accepted {mutation}"
        );
    }
    if std::env::var_os("DIR_T1S_NETWORK_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}
