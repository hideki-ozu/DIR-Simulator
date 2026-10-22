//! Production shared-LAN root-loss regressions for MSTP and RSTP.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const SOURCE_A_MAC: &str = "02:00:00:00:00:01";
const SHARED_PORTS: [&str; 4] = [
    "Backup.s1.tx_b",
    "Backup.s2.tx_b",
    "Backup.s3.tx_b",
    "Backup.s1.tx_c",
];

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite/shared-lan")
}

fn scratch(protocol: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "dir-shared-tree-{protocol}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn decimal(value: &Value) -> u64 {
    value
        .as_str()
        .and_then(|value| value.parse().ok())
        .or_else(|| value.as_u64())
        .unwrap_or_else(|| panic!("expected an unsigned decimal value, got {value}"))
}

fn ordered_records(result: &Value) -> Vec<&Value> {
    let mut records: Vec<_> = result["simulation"]["model_records"]
        .as_array()
        .expect("production model_records")
        .iter()
        .collect();
    records.sort_by_key(|record| {
        (
            decimal(&record["time_ps"]),
            record["data"]["effect_seq"]
                .as_str()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
        )
    });
    records
}

fn assert_frame_path(
    records: &[&Value],
    frame_id: &str,
    source_port: &str,
    receiver_port: &str,
    expected_path: &[&str],
    shared_attachments: usize,
) {
    let frames: Vec<_> = records
        .iter()
        .filter(|record| {
            record["schema_name"] == "ethernet.dynamic.frame" && record["record_id"] == frame_id
        })
        .collect();
    assert_eq!(frames.len(), 1, "one generated frame {frame_id}");
    let frame = &frames[0]["data"];
    assert_eq!(frame["source"], source_port.trim_end_matches(".tx"));

    let deliveries: Vec<_> = records
        .iter()
        .filter(|record| {
            record["schema_name"] == "ethernet.dynamic.reception"
                && record["data"]["frame_id"] == frame_id
                && record["data"]["status"] == "received"
        })
        .collect();
    assert_eq!(deliveries.len(), 1, "one endpoint delivery for {frame_id}");
    assert_eq!(deliveries[0]["data"]["ingress"], receiver_port);

    let transfers: HashMap<_, _> = records
        .iter()
        .filter(|record| {
            record["schema_name"] == "ethernet.dynamic.transfer"
                && record["data"]["frame_id"] == frame_id
        })
        .map(|record| {
            (
                record["record_id"].as_str().expect("transfer record id"),
                *record,
            )
        })
        .collect();
    let mut current = deliveries[0]["data"]["transfer_id"]
        .as_str()
        .expect("received transfer id")
        .to_owned();
    let mut chain = Vec::new();
    while let Some(transfer) = transfers.get(current.as_str()) {
        chain.push(*transfer);
        assert!(
            chain.len() <= transfers.len(),
            "transfer parents are acyclic"
        );
        match transfer["data"]["parent_transfer_id"].as_str() {
            Some(parent) => current = parent.to_owned(),
            None => break,
        }
    }
    assert!(
        !chain.is_empty(),
        "actual transfer chain exists for {frame_id}"
    );
    chain.reverse();

    let actual_path: Vec<_> = chain
        .iter()
        .map(|transfer| transfer["data"]["from_port"].as_str().unwrap())
        .collect();
    assert_eq!(actual_path, expected_path, "actual path for {frame_id}");
    assert_eq!(chain.last().unwrap()["data"]["to_port"], receiver_port);
    for transfer in &chain {
        assert_eq!(
            transfer["data"]["wire"]["mac_hex"], frame["mac_hex"],
            "actual wire bytes are preserved for {frame_id}"
        );
    }

    let mut shared_sends = 0;
    for transfer in chain
        .iter()
        .filter(|transfer| SHARED_PORTS.contains(&transfer["data"]["from_port"].as_str().unwrap()))
    {
        shared_sends += 1;
        let source = transfer["data"]["from_port"].as_str().unwrap();
        let sof_ps = &transfer["data"]["sof_ps"];
        let reservations: Vec<_> = records
            .iter()
            .filter(|record| {
                record["schema_name"] == "dir.network.segment"
                    && record["data"]["kind"] == "tx_started"
                    && record["data"]["segment_id"] == "shared"
                    && record["data"]["source_port"] == source
                    && record["time_ps"] == *sof_ps
            })
            .collect();
        assert_eq!(
            reservations.len(),
            1,
            "one shared-medium reservation for {frame_id} from {source}"
        );
        let reservation = &reservations[0]["data"];
        assert_eq!(
            reservation["receivers"].as_u64(),
            Some((shared_attachments - 1) as u64)
        );
        let owner = &reservation["owner"];
        assert_eq!(
            records
                .iter()
                .filter(|record| {
                    record["schema_name"] == "dir.network.segment"
                        && record["data"]["kind"] == "tx_released"
                        && record["data"]["segment_id"] == "shared"
                        && record["data"]["source_port"] == source
                        && record["data"]["owner"] == *owner
                })
                .count(),
            1,
            "one release for the shared-medium reservation"
        );
    }
    assert!(shared_sends > 0, "{frame_id} traverses the shared LAN");
}

fn assert_root_loss_run(
    ini_name: &str,
    protocol: &str,
    loss_ps: u64,
    second_frame_ps: u64,
    horizon_ps: u64,
    adapter_prefix: &str,
) {
    let root = fixture();
    let model: Value = serde_json::from_slice(
        &fs::read(root.join(format!("cycle-loss-{protocol}-model.json"))).unwrap(),
    )
    .unwrap();
    let model_config = &model["base"]["config"]["base"]["config"]["dynamic"];
    assert_eq!(decimal(&model_config["mac_age_ps"]), 300_000_000_000_000);
    let shared_attachments = model["shared_lans"][0]["ports"]
        .as_array()
        .expect("shared LAN attachment list")
        .len();
    assert!(
        shared_attachments >= 3,
        "shared LAN has at least three attachments"
    );

    let output = scratch(protocol).join("result");
    run(prepare(&root.join(ini_name)).unwrap(), &output).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap();
    let records = ordered_records(&result);

    assert_frame_path(
        &records,
        "before:0",
        "Backup.a.tx",
        "Backup.b.rx",
        &["Backup.a.tx", "Backup.s1.tx_b", "Backup.s2.tx_a"],
        shared_attachments,
    );
    assert_frame_path(
        &records,
        "after:0",
        "Backup.b.tx",
        "Backup.c.rx",
        &["Backup.b.tx", "Backup.s2.tx_b", "Backup.s3.tx_a"],
        shared_attachments,
    );

    if protocol == "rstp" {
        assert_frame_path(
            &records,
            "flush_probe:0",
            "Backup.b.tx",
            "Backup.a.rx",
            &["Backup.b.tx", "Backup.s2.tx_b", "Backup.s1.tx_a"],
            shared_attachments,
        );
    }

    if protocol == "mstp" {
        let tree_records: Vec<_> = records
            .iter()
            .filter(|record| record["schema_name"] == "ethernet.control.wire.tree_policy")
            .collect();
        assert!(
            tree_records.iter().any(|record| {
                record["data"]["bridge"] == "Backup.s1"
                    && record["data"]["port"] == "Backup.s1.tx_c"
                    && record["data"]["mstid"].as_u64() == Some(0)
                    && record["data"]["policy"]["role"] == "Backup"
                    && decimal(&record["time_ps"]) < loss_ps
            }),
            "s1's duplicate attachment is observed as Backup before root loss"
        );
        assert!(
            tree_records.iter().any(|record| {
                record["data"]["bridge"] == "Backup.s2"
                    && record["data"]["mstid"].as_u64() == Some(0)
                    && record["data"]["root_port"].is_null()
                    && record["data"]["policy"]["forwarding"] == true
                    && decimal(&record["time_ps"]) > loss_ps
                    && decimal(&record["time_ps"]) < second_frame_ps
            }),
            "s2 becomes root and forwards after H1/H4 fail"
        );
    } else {
        let policies: Vec<_> = records
            .iter()
            .filter(|record| record["schema_name"] == "ethernet.control.wire.policy")
            .collect();
        assert!(
            policies.iter().any(|record| {
                record["data"]["ports"]["Backup.s1.tx_c"]["role"] == "Backup"
                    && decimal(&record["time_ps"]) < loss_ps
            }),
            "s1's duplicate attachment is observed as Backup before root loss"
        );
        assert!(
            policies.iter().any(|record| {
                let ports = &record["data"]["ports"];
                ports["Backup.s2.tx_a"]["role"] == "Designated"
                    && ports["Backup.s2.tx_a"]["forwarding"] == true
                    && ports["Backup.s2.tx_b"]["role"] == "Designated"
                    && ports["Backup.s2.tx_b"]["forwarding"] == true
                    && ports["Backup.s3.tx_b"]["role"] == "Root"
                    && decimal(&record["time_ps"]) > loss_ps
                    && decimal(&record["time_ps"]) < second_frame_ps
            }),
            "s2 becomes root and forwards after H1/H4 fail"
        );
    }

    let control_events: Vec<_> = records
        .iter()
        .filter(|record| record["schema_name"] == "ethernet.control.wire.event")
        .collect();
    for port in ["Backup.s1.tx_b", "Backup.s1.tx_c"] {
        assert!(
            control_events.iter().any(|record| {
                record["data"]["adapter_id"] == format!("{adapter_prefix}-s1")
                    && record["data"]["event_kind"] == "receive"
                    && record["data"]["port"] == port
                    && record["data"]["reason"] == "link_down"
                    && record["data"]["outcome"] == "control_drop"
                    && decimal(&record["time_ps"]) >= loss_ps
            }),
            "actual control BPDU drop at failed attachment {port}"
        );
    }

    let learned_before_loss = records.iter().any(|record| {
        if record["schema_name"] != "ethernet.dynamic.policy"
            || decimal(&record["time_ps"]) >= loss_ps
        {
            return false;
        }
        record["data"]["changes"].as_array().is_some_and(|changes| {
            changes.iter().any(|change| {
                change["table"] == "mac"
                    && change["key"]["switch"] == "Backup.s2"
                    && change["key"]["vid"] == "10"
                    && change["key"]["mac"] == SOURCE_A_MAC
                    && change["after"]["value"] == "Backup.s2.tx_b"
                    && decimal(&change["after"]["expires_at"]) > horizon_ps
            })
        })
    });
    assert!(
        learned_before_loss,
        "source A's FDB entry remains unexpired beyond the run horizon"
    );

    let nonempty_topology_flush = control_events.iter().any(|record| {
        let within_run =
            decimal(&record["time_ps"]) >= loss_ps && decimal(&record["time_ps"]) < second_frame_ps;
        within_run
            && record["data"]["effects"].as_array().is_some_and(|effects| {
                effects.iter().any(|effect| {
                    effect["kind"] == "mac_flush"
                        && effect["reason"]
                            .as_str()
                            .is_some_and(|reason| reason.contains("topology_change"))
                        && effect["keys"]
                            .as_array()
                            .is_some_and(|keys| !keys.is_empty())
                })
            })
    });
    assert!(
        nonempty_topology_flush,
        "topology change flushes at least one still-live MAC entry"
    );

    if protocol == "rstp" {
        // 13.27.28 is a port-scoped flush. Source A remains learned on an
        // unaffected s2 port; the reverse probe creates a real live victim
        // on the s1 port that transitions to INACTIVE when H1 fails.
        let source_b = "02:00:00:00:00:02";
        assert!(records.iter().any(|r| {
            r["schema_name"] == "ethernet.dynamic.policy"
                && decimal(&r["time_ps"]) < loss_ps
                && r["data"]["changes"].as_array().is_some_and(|changes| {
                    changes.iter().any(|x| {
                        x["table"] == "mac"
                            && x["key"]["switch"] == "Backup.s1"
                            && x["key"]["mac"] == source_b
                            && x["after"]["value"] == "Backup.s1.tx_b"
                            && decimal(&x["after"]["expires_at"]) > horizon_ps
                    })
                })
        }));
        assert!(control_events.iter().any(|r| {
            decimal(&r["time_ps"]) == loss_ps
                && r["data"]["effects"].as_array().is_some_and(|effects| {
                    effects.iter().any(|x| {
                        x["kind"] == "mac_flush"
                            && x["port"] == "Backup.s1.tx_b"
                            && x["keys"].as_array().is_some_and(|keys| {
                                keys.iter().any(|key| {
                                    key["switch"] == "Backup.s1"
                                        && decimal(&key["vid"]) == 10
                                        && key["mac"] == source_b
                                })
                            })
                    })
                })
        }));
    }

    fs::remove_dir_all(output.parent().unwrap()).unwrap();
}

#[test]
fn mstp_shared_lan_root_loss_preserves_actual_parent_flow_and_flushes_live_fdb() {
    assert_root_loss_run(
        "cycle-loss-mstp.ini",
        "mstp",
        6_000_000_000_000,
        30_000_000_000_000,
        35_000_000_000_000,
        "mstp",
    );
}

#[test]
fn rstp_shared_lan_root_loss_preserves_actual_parent_flow_and_flushes_live_fdb() {
    assert_root_loss_run(
        "cycle-loss-rstp.ini",
        "rstp",
        36_000_000_000_000,
        65_000_000_000_000,
        70_000_000_000_000,
        "rstp",
    );
}
