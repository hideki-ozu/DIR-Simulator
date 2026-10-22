//! Rehashed cross-owner mutations must match an actual source callback prefix.
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
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn scratch() -> PathBuf {
    let p = std::env::var_os("DIR_CAUSAL_PREFIX_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!(
            "causal-prefix-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn source(kind: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(match kind {
            "phy" => "examples/network-composite/t1s-multidrop/model.ini",
            "gptp" => "examples/gptp/two-node.ini",
            "phy-tsn" => "examples/network-composite/t1s-shaped/model.ini",
            "phy-wire" => "examples/network-composite/t1s-control-fifo/model.ini",
            "phy-preempt" => "examples/network-composite/preemption-phy/model.ini",
            _ => unreachable!(),
        })
}
fn child(root: &Path, mode: &str, kind: &str, cut: u64) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "causal_prefix_process_worker", "--nocapture"])
        .env("DIR_CAUSAL_WORKER", mode)
        .env("DIR_CAUSAL_ROOT", root)
        .env("DIR_CAUSAL_KIND", kind)
        .env("DIR_CAUSAL_CUT", cut.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode}/{kind}/{cut}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn causal_prefix_process_worker() {
    let Ok(mode) = std::env::var("DIR_CAUSAL_WORKER") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_CAUSAL_ROOT").unwrap());
    if mode.starts_with("save") {
        let kind = std::env::var("DIR_CAUSAL_KIND").unwrap();
        let cut: u64 = std::env::var("DIR_CAUSAL_CUT").unwrap().parse().unwrap();
        let ini = if kind == "phy-shift" {
            let original = source("phy");
            let mut text = fs::read_to_string(&original).unwrap();
            for key in ["ned-path", "model-config", "workload"] {
                let line = text
                    .lines()
                    .find(|l| l.starts_with(key))
                    .unwrap()
                    .to_owned();
                let path = original
                    .parent()
                    .unwrap()
                    .join(line.split_once('=').unwrap().1.trim().trim_matches('"'))
                    .canonicalize()
                    .unwrap();
                let selected = if key == "workload" {
                    let mut workload: Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    assert_eq!(workload["generators"][0]["times_ps"][0], "0");
                    workload["generators"][0]["times_ps"][0] = json!("1");
                    let local = root.join("shifted-workload.json");
                    fs::write(&local, serde_json::to_vec(&workload).unwrap()).unwrap();
                    local
                } else {
                    path
                };
                text = text.replace(&line, &format!("{key} = \"{}\"", selected.display()));
            }
            let local = root.join("shifted.ini");
            fs::write(&local, text).unwrap();
            local
        } else {
            source(&kind)
        };
        let (mut session, controller) = RunSession::new(
            prepare(&ini).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..cut {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
        }
        // Exercise owner-only operations: they change session control metadata,
        // but must leave the executable physical prefix exactly reproducible.
        session.observe().unwrap();
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let ack = session
            .checkpoint(&root.join("bundle"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        if mode.ends_with("continue") {
            session.resume(session.pause_token().unwrap()).unwrap();
            assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
            fs::write(
                root.join("expected.json"),
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
        }
    } else {
        let (mut session, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        if mode.ends_with("continue") {
            session.resume(token).unwrap();
            assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
            let expected: Value =
                serde_json::from_slice(&fs::read(root.join("expected.json")).unwrap()).unwrap();
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
        }
    }
}
fn rewrite(bundle: &Path, change: impl FnOnce(&mut Value)) {
    let mut state: Value =
        serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(bundle.join("state.json"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    manifest["commit_ordinal"] = state["commit_ordinal"].clone();
    manifest["horizon_ps"] = state["virtual_time_ps"].clone();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|v| v["path"] == "state.json")
        .unwrap();
    entry["bytes"] = json!(bytes.len().to_string());
    entry["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}
fn rejected(bundle: &Path, label: &str) {
    let error = match restore_checkpoint(bundle, Registry::default(), CheckpointLimits::default()) {
        Ok(_) => panic!("accepted {label}"),
        Err(e) => e,
    };
    assert!(
        error
            .message
            .contains("exact frozen-input committed prefix")
            || error.message.contains("causal prefix"),
        "{label}: {error:?}"
    );
    eprintln!("{label}: {}: {}", error.code, error.message);
}
#[test]
fn fresh_process_named_phy_and_gptp_prefixes_restore() {
    for (kind, cut) in [("phy", 0), ("phy", 5), ("phy", 35), ("gptp", 20)] {
        let root = scratch();
        child(&root, "save-continue", kind, cut);
        child(&root, "restore-continue", kind, cut);
    }
}

#[test]
fn shaped_physical_service_prefix_resumes_in_a_new_process() {
    let root = scratch();
    child(&root, "save-continue", "phy-tsn", 35);
    child(&root, "restore-continue", "phy-tsn", 35);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn actual_control_fifo_prefix_resumes_in_a_new_process() {
    for cut in [30, 170] {
        let root = scratch();
        child(&root, "save-continue", "phy-wire", cut);
        child(&root, "restore-continue", "phy-wire", cut);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn mixed_mac_owner_prefix_resumes_in_a_new_process() {
    for cut in [7, 35] {
        let root = scratch();
        child(&root, "save-continue", "phy-preempt", cut);
        child(&root, "restore-continue", "phy-preempt", cut);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn named_phy_generation_and_cap_are_source_bound_even_with_matching_fes() {
    let root = scratch();
    child(&root, "save", "phy", 1);
    child(&root, "restore", "phy", 1);
    let bundle = root.join("bundle");
    let original = fs::read(bundle.join("state.json")).unwrap();
    let manifest = fs::read(bundle.join("manifest.json")).unwrap();
    for mutation in ["generation", "short-silent-cap"] {
        fs::write(bundle.join("state.json"), &original).unwrap();
        fs::write(bundle.join("manifest.json"), &manifest).unwrap();
        rewrite(&bundle, |s| {
            let boundary = &mut s["network_backend"]["state"]["phy_boundaries"][0];
            let token = boundary["pending"]["token"]["id"]
                .as_u64()
                .unwrap()
                .to_string();
            let old_cut = boundary["pending"]["sample_cut"].as_u64().unwrap();
            assert_eq!(boundary["pending"]["effects_present"], false);
            if mutation == "generation" {
                boundary["generation"] = json!(u64::MAX);
            } else {
                assert!(old_cut > 1);
                boundary["pending"]["sample_cut"] = json!(old_cut - 1);
            }
            let timer = s["fes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|w| w["work"]["token"]["id"] == token)
                .unwrap();
            let bytes: Vec<u8> =
                serde_json::from_value(timer["work"]["envelope"]["payload"].clone()).unwrap();
            let mut payload: Value = serde_json::from_slice(&bytes).unwrap();
            if mutation == "generation" {
                payload["PhyBoundary"]["generation"] = json!(u64::MAX);
            } else {
                payload["PhyBoundary"]["sample_cut"] = json!(old_cut - 1);
                let time: u64 = timer["time_ps"].as_str().unwrap().parse().unwrap();
                // Fixture's exact sample grid is 25MHz * 16 = 2500ps/sample.
                timer["time_ps"] = json!((time - 2500).to_string());
                timer["work"]["envelope"]["time_ps"] = timer["time_ps"].clone();
            }
            timer["work"]["envelope"]["payload"] = json!(serde_json::to_vec(&payload).unwrap());
        });
        rejected(&bundle, mutation);
    }
}
#[test]
fn committed_count_and_same_time_gptp_cause_order_are_source_bound() {
    let root = scratch();
    child(&root, "save", "gptp", 20);
    child(&root, "restore", "gptp", 20);
    let bundle = root.join("bundle");
    let original = fs::read(bundle.join("state.json")).unwrap();
    let manifest = fs::read(bundle.join("manifest.json")).unwrap();
    rewrite(&bundle, |s| {
        let causes = s["network_backend"]["state"]["gptp"]["causes"]
            .as_array_mut()
            .unwrap();
        let pair = (0..causes.len() - 1)
            .find(|&i| {
                causes[i]["at_ps"] == causes[i + 1]["at_ps"]
                    && causes[i]["kind"].get("Timer").is_some()
                    && causes[i + 1]["kind"].get("Timer").is_some()
            })
            .expect("same-time independent initial timers");
        causes.swap(pair, pair + 1);
    });
    rejected(&bundle, "same-time gPTP cause permutation");
    fs::write(bundle.join("state.json"), &original).unwrap();
    fs::write(bundle.join("manifest.json"), &manifest).unwrap();
    rewrite(&bundle, |s| {
        let count: u64 = s["commit_ordinal"].as_str().unwrap().parse().unwrap();
        s["commit_ordinal"] = json!((count + 1).to_string());
    });
    rejected(&bundle, "forged committed callback count");
    fs::write(bundle.join("state.json"), &original).unwrap();
    fs::write(bundle.join("manifest.json"), &manifest).unwrap();
    rewrite(&bundle, |s| {
        s["commit_ordinal"] = json!(u64::MAX.to_string())
    });
    rejected(&bundle, "configured event bound checked before replay");
}

#[test]
fn electrically_valid_shifted_admission_is_bound_to_original_source() {
    let original = scratch();
    let shifted = scratch();
    child(&original, "save", "phy", 35);
    child(&shifted, "save", "phy-shift", 35);
    child(&original, "restore", "phy", 35);
    child(&shifted, "restore", "phy-shift", 35);
    // The second backend is an actually executed, electrically valid prefix.
    // Preserve the original captured inputs/fingerprints, but coforge all
    // physical scheduler/journal state from the one-picosecond-shifted source.
    let alternate: Value =
        serde_json::from_slice(&fs::read(shifted.join("bundle/state.json")).unwrap()).unwrap();
    rewrite(&original.join("bundle"), |s| {
        assert_ne!(s["network_backend"], alternate["network_backend"]);
        for key in [
            "engine_phase",
            "virtual_time_ps",
            "last_event_time_ps",
            "commit_ordinal",
            "next_event_id",
            "next_sequence",
            "last_committed_key",
            "fes",
            "dirty",
            "live_timers",
            "initialized_model_ids",
            "constructed_channel_ids",
            "journal",
            "metric_accumulators",
            "network_backend",
        ] {
            s[key] = alternate[key].clone();
        }
    });
    rejected(
        &original.join("bundle"),
        "electrically valid shifted source admission",
    );
}
