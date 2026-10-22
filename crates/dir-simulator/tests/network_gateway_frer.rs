//! Real CAN -> Gateway -> physical prefix -> FRER -> shaped Clause99 -> suffix.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite/gateway-frer")
}
struct Scenario(PathBuf);
impl Scenario {
    fn new(edit: impl FnOnce(&mut Value, &mut Value)) -> Self {
        let root = std::env::var_os("DIR_GATEWAY_FRER_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "gateway-frer-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.join("models/gatewayfrer")).unwrap();
        for f in ["model.ini", "models/gatewayfrer/Main.ned"] {
            fs::copy(example().join(f), root.join(f)).unwrap();
        }
        let mut m: Value =
            serde_json::from_slice(&fs::read(example().join("model.json")).unwrap()).unwrap();
        let mut w: Value =
            serde_json::from_slice(&fs::read(example().join("workload.json")).unwrap()).unwrap();
        edit(&mut m, &mut w);
        fs::write(
            root.join("model.json"),
            serde_json::to_vec_pretty(&m).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("workload.json"),
            serde_json::to_vec_pretty(&w).unwrap(),
        )
        .unwrap();
        Self(root)
    }
    fn execute(&self) -> Value {
        run(
            prepare(&self.0.join("model.ini")).unwrap(),
            &self.0.join("run"),
        )
        .unwrap();
        result(&self.0.join("run"))
    }
}
impl Drop for Scenario {
    fn drop(&mut self) {
        if std::env::var_os("DIR_GATEWAY_FRER_EVIDENCE").is_none() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn result(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p.join("results.json")).unwrap()).unwrap()
}
fn rows<'a>(v: &'a Value, s: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == s)
        .map(|r| &r["data"])
        .collect()
}
#[test]
fn actual_can_gateway_tagged_shaped_merge_recovery_and_destination() {
    let s = Scenario::new(|_, _| {});
    let v = s.execute();
    fs::write(
        s.0.join("acceptance.json"),
        serde_json::to_vec_pretty(&v).unwrap(),
    )
    .unwrap();
    assert_eq!(rows(&v, "dir.can_ethernet.conversion").len(), 1);
    assert_eq!(rows(&v, "dir.redundancy.origin").len(), 1);
    let indications = rows(&v, "dir.can_ethernet.terminal_indication");
    assert_eq!(indications.len(), 1);
    assert_eq!(indications[0]["terminal_id"], "Net.d");
    assert_eq!(indications[0]["origin_id"], "can:can_source:0");
    let frames = rows(&v, "ethernet.dynamic.frame");
    assert_eq!(frames.len(), 1);
    assert!(
        frames[0]["data_hex"]
            .as_str()
            .unwrap()
            .starts_with("44495243010000000123080123456789abcdef")
    );
    let transfers = rows(&v, "ethernet.dynamic.transfer");
    assert!(
        transfers
            .iter()
            .any(|t| t["from_port"] == "Net.gw.eth.tx" && t["status"] == "serialized")
    );
    assert!(
        rows(&v, "dir.mac_merge.packet")
            .iter()
            .any(|p| p["port"] == "Net.r.tx_a" && p["transfer"].is_string())
    );
    let can = rows(&v, "can.request")[0];
    assert_eq!(can["sof_ps"], "0");
    assert_eq!(can["eof_ps"], "220000000");
    assert_eq!(can["serialized_bits"], "110");
    assert_eq!(can["model_fields"]["crc15"], "11436");
    assert_eq!(can["model_fields"]["stuff_bits"], "2");
    assert_eq!(can["payload_bits"], "64");
    let prefix = transfers
        .iter()
        .find(|t| t["from_port"] == "Net.gw.eth.tx")
        .unwrap();
    let suffix = transfers
        .iter()
        .find(|t| t["from_port"] == "Net.e.tx_d")
        .unwrap();
    assert_eq!(prefix["wire"]["mac_hex"], suffix["wire"]["mac_hex"]);
    assert_eq!(prefix["sof_ps"], "226000000");
    assert_eq!(prefix["eof_ps"], "226608000");
    for port in ["Net.r.tx_a", "Net.r.tx_b"] {
        let branch = transfers.iter().find(|t| t["from_port"] == port).unwrap();
        assert_eq!(branch["wire"]["mac_bytes"], "74");
        assert_eq!(
            &branch["wire"]["mac_hex"].as_str().unwrap()[32..48],
            "f1c10000000088b5"
        );
        assert_eq!(branch["sof_ps"], "500000000");
        assert_eq!(branch["eof_ps"], "500656000");
        assert_eq!(branch["release_ps"], "500752000");
        let packets = rows(&v, "dir.mac_merge.packet");
        let packet = packets
            .iter()
            .find(|p| p["port"] == port && p["transfer"].is_string())
            .unwrap();
        assert_eq!(packet["smd"], "e6");
        assert_eq!(packet["final_packet"], true);
        assert_eq!(
            &packet["wire_hex"].as_str().unwrap()[16..],
            branch["wire"]["mac_hex"].as_str().unwrap()
        );
    }
    assert!(
        v["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["schema_name"] == "ethernet.tsn.credit"
                && r["time_ps"] == "500752000"
                && r["data"]["port"] == "Net.r.tx_a"
                && r["data"]["sign"] == "negative"
                && r["data"]["magnitude"] == "564000000000000")
    );
    assert_eq!(indications[0]["end_to_end_ps"], "501930000");
}
#[test]
fn invalid_gateway_selectors_and_prefix_fail_preparation() {
    for edit in 0..5 {
        let s = Scenario::new(|m, _| match edit {
            0 => m["replication"]["streams"][0]["source_generator"] = json!("invented"),
            1 => m["replication"]["streams"][0]["source"]["rule_id"] = json!("missing"),
            2 => m["replication"]["streams"][0]["source"]["egress_index"] = json!(1),
            3 => {
                m["replication"]["streams"][0]["placement"]["prefix_ports"] = json!(["Net.r.tx_a"])
            }
            _ => m["replication"]["streams"][0]["recovery_algorithm"] = json!("project_history_v1"),
        });
        assert!(prepare(&s.0.join("model.ini")).is_err(), "case {edit}");
    }
}

use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
    registry::Registry, restore_checkpoint,
};
fn canonical(mut v: Value) -> Value {
    for k in ["run_id", "control"] {
        v.as_object_mut().unwrap().remove(k);
    }
    for k in ["started_at_utc", "finished_at_utc"] {
        v["metadata"].as_object_mut().unwrap().remove(k);
    }
    v
}
fn child(root: &Path, mode: &str, cut: &str) {
    let o = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "gateway_checkpoint_worker", "--nocapture"])
        .env("DIR_GATEWAY_WORKER", mode)
        .env("DIR_GATEWAY_ROOT", root)
        .env("DIR_GATEWAY_CUT", cut)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{mode}/{cut}: {} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
#[test]
fn gateway_checkpoint_worker() {
    let Ok(mode) = std::env::var("DIR_GATEWAY_WORKER") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_GATEWAY_ROOT").unwrap());
    if mode == "save" {
        let cut = std::env::var("DIR_GATEWAY_CUT").unwrap();
        let (mut s, c) = RunSession::new(
            prepare(&root.join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..10000 {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            let view = s.observe().unwrap();
            let records = &view.registered.as_ref().unwrap().model_records;
            let has = |schema: &str, predicate: &dyn Fn(&Value) -> bool| {
                records
                    .values()
                    .any(|r| r.schema.name == schema && predicate(&r.data))
            };
            let ready = match cut.as_str() {
                "can" => has("can.request", &|r| r["status"] == "in_flight"),
                "queued" => has("ethernet.dynamic.transfer", &|r| {
                    r["from_port"] == "Net.gw.eth.tx" && r["status"] == "queued"
                }),
                "sof" => has("ethernet.dynamic.transfer", &|r| {
                    r["from_port"] == "Net.gw.eth.tx" && r["sof_ps"].is_string()
                }),
                "recovery" => has("dir.redundancy.origin", &|r| {
                    r["accepted_count"] == "1" && r["completed_count"] == "0"
                }),
                "suffix" => has("ethernet.dynamic.transfer", &|r| {
                    r["from_port"] == "Net.e.tx_d" && r["sof_ps"].is_string()
                }),
                "terminal" => has("dir.can_ethernet.terminal_indication", &|_| true),
                "suspended" => has("dir.mac_merge.packet", &|p| {
                    p["transfer"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("background:"))
                        && p["crc_kind"] == "mcrc"
                        && p["final_packet"] == false
                        && p["complete"] == true
                }),
                _ => panic!("unknown cut"),
            };
            if ready {
                c.submit(ControlCommand::Pause).unwrap();
                assert_eq!(s.poll_control().unwrap(), RunState::Paused);
                let ack = s
                    .checkpoint(&root.join("bundle"), CheckpointLimits::default())
                    .unwrap();
                assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
                s.resume(s.pause_token().unwrap()).unwrap();
                assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
                assert_eq!(
                    s.export(&root.join("original")).unwrap().status,
                    ControlStatus::Applied
                );
                return;
            }
        }
        panic!("cut {cut} not reached");
    } else {
        let (mut s, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        s.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&root.join("restored")).unwrap().status,
            ControlStatus::Applied
        );
    }
}
#[test]
fn real_process_checkpoints_match_at_can_queued_prefix_sof_recovery_suffix_and_terminal() {
    for cut in ["can", "queued", "sof", "recovery", "suffix", "terminal"] {
        let s = Scenario::new(|_, _| {});
        child(&s.0, "save", cut);
        child(&s.0, "resume", cut);
        assert_eq!(
            canonical(result(&s.0.join("original"))),
            canonical(result(&s.0.join("restored"))),
            "cut {cut}"
        );
    }
}
fn rewrite(root: &Path, edit: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let p = root.join("bundle");
    let mut v: Value = serde_json::from_slice(&fs::read(p.join("state.json")).unwrap()).unwrap();
    edit(&mut v);
    let bytes = serde_json::to_vec(&v).unwrap();
    fs::write(p.join("state.json"), &bytes).unwrap();
    let mut m: Value = serde_json::from_slice(&fs::read(p.join("manifest.json")).unwrap()).unwrap();
    let f = m["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "state.json")
        .unwrap();
    f["bytes"] = json!(bytes.len().to_string());
    f["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(p.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
}
#[test]
fn rehashed_gateway_provenance_bytes_sof_terminal_and_round_mutants_reject() {
    let s = Scenario::new(|_, _| {});
    child(&s.0, "save", "terminal");
    let saved = fs::read(s.0.join("bundle/state.json")).unwrap();
    let manifest = fs::read(s.0.join("bundle/manifest.json")).unwrap();
    for kind in [
        "source",
        "conversion",
        "branch",
        "child",
        "bytes",
        "vid",
        "priority",
        "sof",
        "terminal",
        "round",
        "duplicate-indication",
        "omit-indication",
        "forged-can",
    ] {
        fs::write(s.0.join("bundle/state.json"), &saved).unwrap();
        fs::write(s.0.join("bundle/manifest.json"), &manifest).unwrap();
        rewrite(&s.0, |v| {
            let n = &mut v["network_backend"]["state"];
            match kind {
                "source" => {
                    n["core"]["frames"][0]["gateway_injection"]["selector"]["rule_id"] =
                        json!("other")
                }
                "conversion" => {
                    n["core"]["frames"][0]["gateway_injection"]["conversion_index"] = json!(1)
                }
                "branch" => n["core"]["frames"][0]["gateway_injection"]["branch_index"] = json!(1),
                "child" => {
                    n["core"]["frames"][0]["gateway_injection"]["child_id"] = json!("forged")
                }
                "bytes" => n["core"]["frames"][0]["row"]["wire"]["data_hex"] = json!("00"),
                "vid" => n["core"]["frames"][0]["row"]["source_vlan_id"] = json!(11),
                "priority" => n["core"]["frames"][0]["row"]["priority"] = json!(2),
                "sof" => n["bridge"]["conversions"][0]["branches"][0]["row"]["sof_ps"] = json!(0),
                "terminal" => {
                    n["bridge"]["segments"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|s| {
                            s["source_record_id"]
                                .as_str()
                                .is_some_and(|id| id.ends_with("child"))
                        })
                        .unwrap()["targets"][0]["completed_ps"] = json!(0)
                }
                "round" => {
                    n["core"]["frames"][0]["terminal_indications"][0]["recovery_round"] = json!(1)
                }
                "duplicate-indication" => {
                    let i = n["core"]["frames"][0]["terminal_indications"][0].clone();
                    n["core"]["frames"][0]["terminal_indications"]
                        .as_array_mut()
                        .unwrap()
                        .push(i);
                }
                "omit-indication" => n["core"]["frames"][0]["terminal_indications"] = json!([]),
                "forged-can" => {
                    n["bridge"]["requests"][0]["frame"]["data"] = json!("0000000000000000")
                }
                _ => unreachable!(),
            }
        });
        let e = restore_checkpoint(
            &s.0.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        );
        assert!(e.is_err(), "accepted {kind}");
    }
}

#[test]
fn one_lost_branch_delivers_and_rejected_conversion_allocates_nothing() {
    let s = Scenario::new(|_, w| {
        w["ethernet"]["controls"] =
            json!([{"id":"lose-a","at_ps":"0","kind":"link_set","link":"RA","up":false}])
    });
    let v = s.execute();
    assert_eq!(rows(&v, "dir.can_ethernet.terminal_indication").len(), 1);
    assert!(
        rows(&v, "dir.redundancy.branch")
            .iter()
            .any(|b| b["status"] == "dropped")
    );
    let s = Scenario::new(|_, w| w["can"]["generators"][0]["frame"]["id"] = json!(292));
    let v = s.execute();
    assert!(rows(&v, "ethernet.dynamic.frame").is_empty());
    assert!(rows(&v, "dir.redundancy.origin").is_empty());
    assert!(
        rows(&v, "dir.can_ethernet.conversion")
            .iter()
            .all(|c| c["status"] == "rejected")
    );
}
#[test]
fn queued_prefix_backpressure_and_partial_horizon_do_not_allocate_sequences_or_complete_future_targets()
 {
    let s = Scenario::new(|m, w| {
        for o in m["tsn"]["outputs"].as_array_mut().unwrap() {
            if o["port"] == "Net.gw.eth.tx" {
                o["tas"] = json!({"id":"prefix","base_time_ps":"0","cycle_time_ps":"2000000000","entries":[{"duration_ps":"1000000000","open_priorities":[7]},{"duration_ps":"1000000000","open_priorities":[0,1,2,3,4,5,6,7]}]});
            }
        }
        m["ethernet"]["outputs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|o| o["port"] == "Net.gw.eth.tx")
            .unwrap()["queues"][3]["capacity_frames"] = json!("1");
        w["can"]["generators"][0]["period"] = json!("300us");
        w["can"]["generators"][0]["end"] = json!("900us");
    });
    let p = s.0.join("model.ini");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .unwrap()
            .replace("1100000000ps", "900000000ps"),
    )
    .unwrap();
    let v = s.execute();
    assert!(rows(&v, "dir.can_ethernet.terminal_indication").is_empty());
    assert!(
        rows(&v, "dir.redundancy.origin")
            .iter()
            .all(|o| o["sequence"].is_null())
    );
    assert!(
        rows(&v, "dir.can_ethernet.branch")
            .iter()
            .any(|b| b["status"] == "waiting")
    );
    assert_eq!(rows(&v, "ethernet.dynamic.frame").len(), 1);
    assert!(rows(&v, "dir.can_ethernet.segment").iter().all(|s| {
        s["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["completed_ps"].is_null())
    }));
}
#[test]
fn timeout_reacceptance_is_a_second_real_terminal_indication_with_one_unique_target() {
    let s = Scenario::new(|_, _| {});
    let p = s.0.join("models/gatewayfrer/Main.ned");
    let ned=fs::read_to_string(&p).unwrap()
 .replace("network Net {","channel Slow { parameters: @class(\"dir.ethernet.Link\"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(2000000000ps); }\nnetwork Net {")
 .replace("b.tx_e --> gatewayfrer.Wire --> e.rx_b","b.tx_e --> gatewayfrer.Slow --> e.rx_b")
 .replace("e.tx_b --> gatewayfrer.Wire --> b.rx_e","e.tx_b --> gatewayfrer.Slow --> b.rx_e");
    fs::write(&p, ned).unwrap();
    let p = s.0.join("model.ini");
    fs::write(
        &p,
        fs::read_to_string(&p)
            .unwrap()
            .replace("1100000000ps", "4000000000ps"),
    )
    .unwrap();
    let v = s.execute();
    let i = rows(&v, "dir.can_ethernet.terminal_indication");
    assert_eq!(i.len(), 2);
    assert_ne!(i[0]["reception_id"], i[1]["reception_id"]);
    assert!(i.iter().all(|i| i["origin_id"] == "can:can_source:0"));
    let ss = rows(&v, "dir.can_ethernet.segment");
    let segment = ss
        .iter()
        .find(|s| s["source_record_id"] == i[0]["source_frame_id"])
        .unwrap();
    assert_eq!(segment["target_count"], "1");
    assert_eq!(segment["targets"].as_array().unwrap().len(), 1);
    assert!(segment["targets"][0]["completed_ps"].is_string());
    assert_eq!(rows(&v, "dir.redundancy.origin")[0]["completed_count"], "2");
}

fn background(w: &mut Value) {
    w["ethernet"]["generators"] = json!([{"id":"background","node":"Net.d","kind":"ethernet.explicit.v1","times_ps":["995900000"],"frame":{"dst_mac":"02:00:00:00:00:01","ether_type":2048,"data":"a5".repeat(166),"tag":{"vid":10,"pcp":0,"dei":0},"ip_multicast":null},"flow_id":"background","priority":0,"deadline_ps":null}]);
}
#[test]
fn actual_competing_native_frame_uses_mcrc_suspension_and_lost_final_never_delivers_cached_original()
 {
    let good = Scenario::new(|_, w| background(w));
    let v = good.execute();
    let packets = rows(&v, "dir.mac_merge.packet");
    assert!(packets.iter().any(|p| {
        p["transfer"]
            .as_str()
            .is_some_and(|t| t.starts_with("background:"))
            && p["crc_kind"] == "mcrc"
            && p["final_packet"] == false
    }));
    let fragment = packets
        .iter()
        .find(|p| {
            p["transfer"]
                .as_str()
                .is_some_and(|t| t.starts_with("background:"))
                && p["crc_kind"] == "mcrc"
                && p["source_start"] == "0"
        })
        .unwrap();
    let bytes = unhex(fragment["wire_hex"].as_str().unwrap());
    let end = bytes.len() - 4;
    assert_eq!(
        &bytes[end..],
        &(reference_crc32(&bytes[8..end]) ^ 0x0000ffff).to_le_bytes()
    );

    let final_packet = packets
        .iter()
        .find(|p| {
            p["transfer"]
                .as_str()
                .is_some_and(|t| t.starts_with("background:"))
                && p["final_packet"] == true
                && p["source_start"] != "0"
        })
        .unwrap();
    let port = final_packet["port"].clone();
    let ordinal: u64 = final_packet["packet_ordinal"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let bad = Scenario::new(|m, w| {
        background(w);
        m["mac_merge"]["packet_faults"] =
            json!([{"port":port,"packet_ordinal":ordinal,"action":{"kind":"Drop"}}]);
    });
    let lost = bad.execute();
    assert_eq!(rows(&v, "dir.can_ethernet.terminal_indication").len(), 1);
    assert_eq!(rows(&lost, "dir.can_ethernet.terminal_indication").len(), 1);
    assert!(
        rows(&v, "ethernet.dynamic.reception")
            .iter()
            .any(|r| r["frame_id"] == "background:0"
                && r["ingress"] == "Net.gw.eth.rx"
                && r["status"] == "received")
    );
    assert!(
        !rows(&lost, "ethernet.dynamic.reception")
            .iter()
            .any(|r| r["frame_id"] == "background:0"
                && r["ingress"] == "Net.gw.eth.rx"
                && r["status"] == "received")
    );
}
#[test]
fn real_process_checkpoint_during_actual_native_fragment_suspension_in_gateway_composition() {
    let s = Scenario::new(|_, w| background(w));
    child(&s.0, "save", "suspended");
    child(&s.0, "resume", "suspended");
    assert_eq!(
        canonical(result(&s.0.join("original"))),
        canonical(result(&s.0.join("restored")))
    );
}
#[test]
fn actual_gateway_pmac_packet_loss_on_both_branches_has_no_false_terminal() {
    let s = Scenario::new(|m, _| {
        m["mac_merge"]["packet_faults"] = json!([
         {"port":"Net.r.tx_a","packet_ordinal":3,"action":{"kind":"Drop"}},
         {"port":"Net.r.tx_b","packet_ordinal":3,"action":{"kind":"Drop"}}
        ])
    });
    let v = s.execute();
    assert!(rows(&v, "dir.can_ethernet.terminal_indication").is_empty());
    assert!(rows(&v, "dir.redundancy.recovery").is_empty());
    assert!(
        rows(&v, "ethernet.dynamic.reception")
            .iter()
            .all(|r| r["ingress"] != "Net.d.rx")
    );
    assert!(
        rows(&v, "dir.mac_merge.packet")
            .iter()
            .filter(|p| p["transfer"].is_string())
            .all(|p| p["fault_outcome"] == "dropped")
    );
    assert!(rows(&v, "dir.can_ethernet.segment").iter().all(|s| {
        s["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["completed_ps"].is_null())
    }));
}
#[test]
fn gateway_prefix_merge_is_rejected_with_specific_guard() {
    for include_source in [false, true] {
        let s = Scenario::new(|m, _| {
            let mut a = m["mac_merge"]["outputs"][0].clone();
            a["port"] = json!("Net.gw.eth.tx");
            a["source_mac"] = json!("02:00:00:00:02:01");
            let mut b = a.clone();
            b["port"] = json!("Net.r.tx_s");
            b["source_mac"] = json!("02:00:00:00:02:02");
            let outputs = m["mac_merge"]["outputs"].as_array_mut().unwrap();
            if include_source {
                outputs.push(a);
            }
            outputs.push(b);
        });
        let e = prepare(&s.0.join("model.ini")).unwrap_err();
        assert!(
            e.message
                .contains("Gateway FRER prefix must use ordinary Ethernet"),
            "{e:?}"
        );
    }
}

#[test]
fn two_actual_can_conversions_demonstrate_analytic_cbs_delay_after_tas_opens() {
    let s = Scenario::new(|_, w| {
        // Second request waits for the real CAN intermission, then converts at452us.
        // Both Ethernet copies consequently wait behind the branch TAS gate.
        w["can"]["generators"][0]["period"] = json!("225us");
        w["can"]["generators"][0]["end"] = json!("450us");
    });
    let v = s.execute();
    let packets = rows(&v, "dir.mac_merge.packet");
    for port in ["Net.r.tx_a", "Net.r.tx_b"] {
        let mut sofs = packets
            .iter()
            .filter(|p| p["port"] == port && p["transfer"].is_string())
            .map(|p| p["sof_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .collect::<Vec<_>>();
        sofs.sort();
        // 74MAC +8preamble +12IFG =752wirebits. sendSlope=-750M
        // makes-564creditbits; 250M idleSlope needs2256ns after752ns occupancy.
        assert_eq!(sofs, [500_000_000, 503_008_000]);
    }
    assert_eq!(rows(&v, "dir.can_ethernet.terminal_indication").len(), 2);
    let mut sequences = rows(&v, "dir.redundancy.origin")
        .iter()
        .map(|o| o["sequence"].as_str().unwrap().parse::<u64>().unwrap())
        .collect::<Vec<_>>();
    sequences.sort();
    assert_eq!(sequences, [0, 1]);
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn reference_crc32(data: &[u8]) -> u32 {
    let mut r = 0xffff_ffffu32;
    for &octet in data {
        r ^= u32::from(octet);
        for _ in 0..8 {
            r = if r & 1 == 1 {
                (r >> 1) ^ 0xedb8_8320
            } else {
                r >> 1
            };
        }
    }
    !r
}
