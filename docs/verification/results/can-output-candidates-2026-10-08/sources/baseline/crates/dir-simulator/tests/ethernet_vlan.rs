//! Public preparation/run checks for the Ethernet VLAN contract.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-ethernet-vlan-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/ethernet")
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn rows<'a>(result: &'a Value, schema: &str) -> Vec<&'a Value> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["schema_name"] == schema)
        .collect()
}

fn ps(row: &Value, field: &str) -> Option<u64> {
    row["data"][field]
        .as_str()
        .map(|value| value.parse().unwrap())
}

fn base_model() -> Value {
    load(&examples().join("model-vlan.json"))
}

fn base_unicast() -> Value {
    load(&examples().join("vlan-unicast.json"))
}

fn base_multicast() -> Value {
    load(&examples().join("vlan-multicast.json"))
}

fn copy_ned(temp: &Temp) {
    let source = examples().join("models/ethdemo");
    let models = temp.0.join("models/ethdemo");
    fs::create_dir_all(&models).unwrap();
    for file in ["Main.ned", "VlanDemo.ned"] {
        fs::copy(source.join(file), models.join(file)).unwrap();
    }
}

fn write_case(model: &str, workload: &str, limit_ps: u64) -> (Temp, PathBuf) {
    let temp = Temp::new();
    copy_ned(&temp);
    fs::write(temp.0.join("model.json"), model).unwrap();
    fs::write(temp.0.join("workload.json"), workload).unwrap();
    let config = temp.0.join("case.ini");
    fs::write(
        &config,
        format!(
            "[General]\nnetwork = ethdemo.VlanDemo\nned-path = \"models\"\nmodel-profile = \"ethernet.l2.vlan.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {limit_ps}ps\n"
        ),
    )
    .unwrap();
    (temp, config)
}

fn run_path(config: &Path) -> Value {
    let output = Temp::new();
    let prepared = prepare(config).unwrap_or_else(|error| panic!("{}: {error}", config.display()));
    let report =
        run(prepared, &output.0).unwrap_or_else(|error| panic!("{}: {error}", config.display()));
    assert_eq!(report.exit_code, 0, "{}", config.display());
    load(&output.0.join("results.json"))
}

fn run_case(model: &Value, workload: &Value, limit_ps: u64) -> Value {
    let model = serde_json::to_string(model).unwrap();
    let workload = serde_json::to_string(workload).unwrap();
    let (temp, config) = write_case(&model, &workload, limit_ps);
    let result = run_path(&config);
    drop(temp);
    result
}

fn port_mut<'a>(model: &'a mut Value, port: &str) -> &'a mut Value {
    model["ports"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["port"] == port)
        .unwrap()
}

fn switch_mut<'a>(model: &'a mut Value, instance: &str) -> &'a mut Value {
    model["switches"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["instance"] == instance)
        .unwrap()
}

fn output_mut<'a>(model: &'a mut Value, port: &str) -> &'a mut Value {
    model["outputs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["port"] == port)
        .unwrap()
}

fn generator_mut<'a>(workload: &'a mut Value, id: &str) -> &'a mut Value {
    workload["generators"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["id"] == id)
        .unwrap()
}

fn transfer_for<'a>(rows: &[&'a Value], frame: &str, port: &str) -> &'a Value {
    rows.iter()
        .copied()
        .find(|row| row["data"]["frame_id"] == frame && row["data"]["from_port"] == port)
        .unwrap()
}

#[test]
fn readable_examples_run_and_unicast_matches_the_64_68_64_byte_timing() {
    let result = run_path(&examples().join("vlan-unicast.ini"));
    assert_eq!(result["metadata"]["model_profile"], "ethernet.l2.vlan.v1");
    for (schema, version) in [
        ("ethernet.frame", 3),
        ("ethernet.transfer", 3),
        ("ethernet.reception", 2),
    ] {
        assert!(
            rows(&result, schema)
                .iter()
                .all(|row| row["schema_version"] == version)
        );
    }
    let transfers = rows(&result, "ethernet.transfer");
    let hop = |port: &str| {
        transfers
            .iter()
            .find(|row| row["data"]["from_port"] == port)
            .unwrap_or_else(|| panic!("missing transfer from {port}"))
    };
    let source = hop("VlanDemo.a.tx");
    assert_eq!(
        (
            ps(source, "sof_ps"),
            ps(source, "eof_ps"),
            ps(source, "arrival_ps")
        ),
        (Some(0), Some(576_000), Some(577_000))
    );
    let middle = hop("VlanDemo.s1.tx_b");
    assert_eq!(
        (
            ps(middle, "sof_ps"),
            ps(middle, "eof_ps"),
            ps(middle, "arrival_ps")
        ),
        (Some(579_000), Some(1_187_000), Some(1_188_000))
    );
    assert_eq!(middle["data"]["wire"]["mac_bytes"], "68");
    let destination = hop("VlanDemo.s2.tx_c");
    assert_eq!(
        (
            ps(destination, "sof_ps"),
            ps(destination, "eof_ps"),
            ps(destination, "arrival_ps")
        ),
        (Some(1_190_000), Some(1_766_000), Some(1_767_000))
    );
    assert_eq!(destination["data"]["wire"]["mac_bytes"], "64");
    assert!(
        rows(&result, "ethernet.reception")
            .iter()
            .any(|row| row["subject"] == "VlanDemo.b"
                && row["data"]["status"] == "received"
                && row["data"]["vlan_id"] == "10")
    );

    let multicast = run_path(&examples().join("vlan-multicast.ini"));
    let receptions = rows(&multicast, "ethernet.reception");
    assert!(
        receptions
            .iter()
            .any(|row| row["subject"] == "VlanDemo.c" && row["data"]["status"] == "received")
    );
    assert!(
        receptions.iter().any(|row| row["subject"] == "VlanDemo.b"
            && row["data"]["reason"] == "multicast_not_subscribed")
    );
}

#[test]
fn fanout_checks_byte_capacity_after_per_copy_tag_rewrite() {
    let mut model = base_model();
    let s2_b = "VlanDemo.s2.tx_b";
    let membership = port_mut(&mut model, s2_b)["vlans"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["vid"] == 10)
        .unwrap();
    membership["tagged"] = json!(true);
    for port in [s2_b, "VlanDemo.s2.tx_c"] {
        for queue in output_mut(&mut model, port)["queues"]
            .as_array_mut()
            .unwrap()
        {
            queue["capacity_bytes"] = json!("64");
        }
    }
    let mut workload = base_unicast();
    let generator = generator_mut(&mut workload, "vlan_unicast");
    generator["frame"]["dst_mac"] = json!("ff:ff:ff:ff:ff:ff");
    generator["flow_id"] = json!("flow_fanout");
    let result = run_case(&model, &workload, 4_000_000);
    let transfers = rows(&result, "ethernet.transfer");
    let large = transfers
        .iter()
        .find(|row| row["data"]["from_port"] == s2_b)
        .unwrap();
    let small = transfers
        .iter()
        .find(|row| row["data"]["from_port"] == "VlanDemo.s2.tx_c")
        .unwrap();
    assert_eq!(large["data"]["status"], "dropped");
    assert_eq!(large["data"]["drop_reason"], "queue_full");
    assert_eq!(large["data"]["wire"]["mac_bytes"], "68");
    assert_eq!(small["data"]["status"], "serialized");
    assert_eq!(small["data"]["wire"]["mac_bytes"], "64");
    assert!(
        rows(&result, "ethernet.reception")
            .iter()
            .any(|row| row["subject"] == "VlanDemo.b" && row["data"]["status"] == "received")
    );
}

#[test]
fn vlan_fdb_uses_vid_and_broadcast_skips_nonmember_ports() {
    let mut model = base_model();
    for entry in switch_mut(&mut model, "VlanDemo.s2")["fdb"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|row| row["dst_mac"] == "02:00:00:00:00:02")
    {
        entry["egress"] = if entry["vid"] == 10 {
            json!("VlanDemo.s2.tx_b")
        } else {
            json!("VlanDemo.s2.tx_c")
        };
    }
    let mut workload = base_unicast();
    let first = generator_mut(&mut workload, "vlan_unicast");
    first["id"] = json!("vid10");
    first["flow_id"] = json!("flow_vid10");
    let mut second = first.clone();
    second["id"] = json!("vid20");
    second["flow_id"] = json!("flow_vid20");
    second["priority"] = json!(5);
    second["frame"]["tag"] = json!({"vid":20,"pcp":5,"dei":0});
    workload["generators"].as_array_mut().unwrap().push(second);
    let result = run_case(&model, &workload, 5_000_000);
    let transfers = rows(&result, "ethernet.transfer");
    let route = |frame: &str| {
        transfers
            .iter()
            .find(|row| {
                row["data"]["frame_id"] == frame
                    && row["data"]["from_port"]
                        .as_str()
                        .unwrap()
                        .starts_with("VlanDemo.s2.")
            })
            .unwrap()
    };
    assert_eq!(route("vid10:0")["data"]["from_port"], "VlanDemo.s2.tx_b");
    assert_eq!(route("vid10:0")["data"]["vlan_id"], "10");
    assert_eq!(route("vid20:0")["data"]["from_port"], "VlanDemo.s2.tx_c");
    assert_eq!(route("vid20:0")["data"]["vlan_id"], "20");

    let mut model = base_model();
    for port in ["VlanDemo.s2.tx_c", "VlanDemo.b.tx"] {
        let policy = port_mut(&mut model, port);
        policy["pvid"] = json!(20);
        policy["vlans"] = json!([{"vid":20,"tagged":true}]);
    }
    switch_mut(&mut model, "VlanDemo.s2")["fdb"]
        .as_array_mut()
        .unwrap()
        .retain(|row| !(row["vid"] == 10 && row["dst_mac"] == "02:00:00:00:00:02"));
    let mut workload = base_unicast();
    generator_mut(&mut workload, "vlan_unicast")["frame"]["dst_mac"] = json!("ff:ff:ff:ff:ff:ff");
    let result = run_case(&model, &workload, 4_000_000);
    assert!(
        !rows(&result, "ethernet.transfer")
            .iter()
            .any(|row| row["data"]["from_port"] == "VlanDemo.s2.tx_c"
                && row["data"]["vlan_id"] == "10")
    );
    assert!(
        !rows(&result, "ethernet.reception")
            .iter()
            .any(|row| row["subject"] == "VlanDemo.b" && row["data"]["vlan_id"] == "10")
    );
}

#[test]
fn static_multicast_distinguishes_known_unknown_empty_and_unsubscribed() {
    let mut model = base_model();
    switch_mut(&mut model, "VlanDemo.s1")["unknown_multicast"] = json!("drop");
    let base = base_multicast()["generators"][0].clone();
    let generators = [
        ("known", "01:00:5e:00:00:01"),
        ("unknown", "01:00:5e:00:00:02"),
        ("empty", "01:00:5e:00:00:03"),
    ]
    .into_iter()
    .map(|(id, group)| {
        let mut row = base.clone();
        row["id"] = json!(id);
        row["flow_id"] = json!(format!("flow_{id}"));
        row["frame"]["dst_mac"] = json!(group);
        row
    })
    .collect::<Vec<_>>();
    let workload = json!({"schema_version":3,"generators":generators});
    let result = run_case(&model, &workload, 5_000_000);
    let frames = rows(&result, "ethernet.frame");
    let destinations: std::collections::BTreeMap<_, _> = frames
        .iter()
        .map(|row| {
            (
                row["record_id"].as_str().unwrap(),
                row["data"]["dst_mac"].as_str().unwrap(),
            )
        })
        .collect();
    let s1 = rows(&result, "ethernet.reception")
        .into_iter()
        .filter(|row| row["subject"] == "VlanDemo.s1")
        .collect::<Vec<_>>();
    let reason = |group: &str| {
        s1.iter()
            .find(|row| destinations[row["data"]["frame_id"].as_str().unwrap()] == group)
            .unwrap()["data"]["reason"]
            .as_str()
            .unwrap()
    };
    assert_eq!(reason("01:00:5e:00:00:02"), "unknown_multicast");
    assert_eq!(reason("01:00:5e:00:00:03"), "multicast_no_egress");
    assert!(s1.iter().any(
        |row| destinations[row["data"]["frame_id"].as_str().unwrap()] == "01:00:5e:00:00:01"
            && row["data"]["status"] == "forwarded"
    ));
    let receptions = rows(&result, "ethernet.reception");
    assert!(
        receptions
            .iter()
            .any(|row| row["subject"] == "VlanDemo.c" && row["data"]["status"] == "received")
    );
    assert!(
        receptions.iter().any(|row| row["subject"] == "VlanDemo.b"
            && row["data"]["reason"] == "multicast_not_subscribed")
    );
}

#[test]
fn priority_tracks_hop_class_and_untagged_reception_resets_dei() {
    let mut workload = base_unicast();
    let source = generator_mut(&mut workload, "vlan_unicast");
    source["frame"]["dst_mac"] = json!("02:00:00:00:00:04");
    source["flow_id"] = json!("flow_untagged_priority");
    let result = run_case(&base_model(), &workload, 6_000_000);
    let transfers = rows(&result, "ethernet.transfer");
    let source = transfer_for(&transfers, "vlan_unicast:0", "VlanDemo.a.tx");
    let s1 = transfer_for(&transfers, "vlan_unicast:0", "VlanDemo.s1.tx_b");
    let s2 = transfer_for(&transfers, "vlan_unicast:0", "VlanDemo.s2.tx_b");
    let s3 = transfer_for(&transfers, "vlan_unicast:0", "VlanDemo.s3.tx_b");
    for (row, priority) in [(source, 7), (s1, 2), (s2, 2), (s3, 0)] {
        assert_eq!(row["data"]["priority"], priority.to_string());
    }
    assert_eq!(s1["data"]["wire"]["tag"]["pcp"], "2");

    let mut model = base_model();
    let s2_b = port_mut(&mut model, "VlanDemo.s2.tx_b");
    s2_b["pvid"] = json!(20);
    s2_b["vlans"] = json!([{"vid":10,"tagged":true},{"vid":20,"tagged":false}]);
    let s3_a = port_mut(&mut model, "VlanDemo.s3.tx_a");
    s3_a["pvid"] = json!(20);
    s3_a["default_priority"] = json!(0);
    s3_a["vlans"] = json!([{"vid":10,"tagged":true},{"vid":20,"tagged":false}]);
    let mut workload = base_unicast();
    let source = generator_mut(&mut workload, "vlan_unicast");
    source["id"] = json!("tagged_dei");
    source["flow_id"] = json!("flow_tagged_dei");
    source["frame"]["dst_mac"] = json!("02:00:00:00:00:04");
    source["frame"]["tag"] = json!({"vid":20,"pcp":4,"dei":1});
    source["priority"] = json!(4);
    let result = run_case(&model, &workload, 6_000_000);
    let transfers = rows(&result, "ethernet.transfer");
    let s1 = transfer_for(&transfers, "tagged_dei:0", "VlanDemo.s1.tx_b");
    let s2 = transfer_for(&transfers, "tagged_dei:0", "VlanDemo.s2.tx_b");
    let s3 = transfer_for(&transfers, "tagged_dei:0", "VlanDemo.s3.tx_b");
    for (row, priority) in [(s1, 4), (s2, 4), (s3, 0)] {
        assert_eq!(row["data"]["priority"], priority.to_string());
    }
    assert_eq!(s1["data"]["wire"]["tag"]["dei"], "1");
    assert!(s2["data"]["wire"]["tag"].is_null());
    assert_eq!(s3["data"]["wire"]["tag"]["pcp"], "0");
    assert_eq!(s3["data"]["wire"]["tag"]["dei"], "0");
}

#[test]
fn exact_wire_and_processing_stops_exclude_t_and_commit_at_t_plus_one() {
    // All expectations come from the documented 1 Gbps lengths and fixed delays.
    let mut model = base_model();
    let mut workload = base_unicast();
    generator_mut(&mut workload, "vlan_unicast")["frame"]["dst_mac"] = json!("ff:ff:ff:ff:ff:ff");
    for membership in port_mut(&mut model, "VlanDemo.s2.tx_b")["vlans"]
        .as_array_mut()
        .unwrap()
    {
        if membership["vid"] == 10 {
            membership["tagged"] = json!(true);
        }
    }
    let hops = [
        (
            "VlanDemo.a.tx",
            "VlanDemo.s1",
            [0, 576_000, 672_000, 577_000],
            579_000,
        ),
        (
            "VlanDemo.s1.tx_b",
            "VlanDemo.s2",
            [579_000, 1_187_000, 1_283_000, 1_188_000],
            1_190_000,
        ),
        (
            "VlanDemo.s2.tx_c",
            "VlanDemo.b",
            [1_190_000, 1_766_000, 1_862_000, 1_767_000],
            1_767_000,
        ),
        (
            "VlanDemo.s2.tx_b",
            "VlanDemo.s3",
            [1_190_000, 1_798_000, 1_894_000, 1_799_000],
            1_801_000,
        ),
    ];
    let boundaries: std::collections::BTreeSet<u64> = hops
        .iter()
        .flat_map(|(_, _, times, ready)| times.iter().copied().chain([*ready]))
        .filter(|time| *time > 0)
        .collect();
    for boundary in boundaries {
        for limit in [boundary, boundary + 1] {
            let result = run_case(&model, &workload, limit);
            let transfers = rows(&result, "ethernet.transfer");
            let receptions = rows(&result, "ethernet.reception");
            for (port, device, times, ready) in hops {
                let transfer = transfers.iter().copied().find(|row| {
                    row["data"]["frame_id"] == "vlan_unicast:0" && row["data"]["from_port"] == port
                });
                if times[0] >= limit {
                    assert!(transfer.is_none(), "{port} must not be offered at {limit}");
                    continue;
                }
                let transfer = transfer.unwrap();
                for (field, expected) in ["sof_ps", "eof_ps", "release_ps", "arrival_ps"]
                    .into_iter()
                    .zip(times)
                {
                    assert_eq!(
                        ps(transfer, field),
                        (expected < limit).then_some(expected),
                        "{port} {field} at {limit}"
                    );
                    if field != "sof_ps" {
                        assert_eq!(ps(transfer, &format!("planned_{field}")), Some(expected));
                    }
                }
                let reception = receptions.iter().copied().find(|row| {
                    row["subject"] == device && row["data"]["transfer_id"] == transfer["record_id"]
                });
                if times[3] >= limit {
                    assert!(reception.is_none(), "{device} must not arrive at {limit}");
                } else {
                    let reception = reception.unwrap();
                    assert_eq!(ps(reception, "planned_ready_ps"), Some(ready));
                    assert_eq!(ps(reception, "ready_ps"), (ready < limit).then_some(ready));
                    if ready >= limit {
                        assert_eq!(reception["data"]["status"], "processing");
                        assert!(
                            reception["data"]["egress_transfer_ids"]
                                .as_array()
                                .unwrap()
                                .is_empty()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn invalid_vlan_types_boundaries_duplicate_and_missing_keys_fail_prepare() {
    let base_model = base_model();
    let base_workload = base_unicast();
    let rejects = |model: Value, workload: Value| {
        let model = serde_json::to_string(&model).unwrap();
        let workload = serde_json::to_string(&workload).unwrap();
        let (_temp, config) = write_case(&model, &workload, 4_000_000);
        assert!(
            prepare(&config).is_err(),
            "invalid input was accepted: {}",
            config.display()
        );
    };
    for invalid in [json!(0), json!(4095), json!(true)] {
        let mut model = base_model.clone();
        port_mut(&mut model, "VlanDemo.a.tx")["pvid"] = invalid;
        rejects(model, base_workload.clone());
    }
    let mut model = base_model.clone();
    port_mut(&mut model, "VlanDemo.a.tx")
        .as_object_mut()
        .unwrap()
        .remove("admit");
    rejects(model, base_workload.clone());
    for vid in [json!(0), json!(4095), json!(true)] {
        let mut workload = base_workload.clone();
        let generator = generator_mut(&mut workload, "vlan_unicast");
        generator["frame"]["tag"] = json!({"vid":vid,"pcp":4,"dei":0});
        generator["priority"] = json!(4);
        rejects(base_model.clone(), workload);
    }
    let mut workload = base_workload.clone();
    generator_mut(&mut workload, "vlan_unicast")["frame"]["tag"] =
        json!({"vid":20,"pcp":3,"dei":0});
    rejects(base_model.clone(), workload);
    let mut workload = base_workload.clone();
    generator_mut(&mut workload, "vlan_unicast")["frame"]["tag"] =
        json!({"vid":10,"pcp":7,"dei":1});
    rejects(base_model.clone(), workload);
    for tag in [
        json!({"vid":20,"pcp":true,"dei":0}),
        json!({"vid":20,"pcp":4,"dei":true}),
    ] {
        let mut workload = base_workload.clone();
        generator_mut(&mut workload, "vlan_unicast")["frame"]["tag"] = tag;
        generator_mut(&mut workload, "vlan_unicast")["priority"] = json!(4);
        rejects(base_model.clone(), workload);
    }
    let mut workload = base_workload.clone();
    generator_mut(&mut workload, "vlan_unicast")["frame"]
        .as_object_mut()
        .unwrap()
        .remove("tag");
    rejects(base_model.clone(), workload);
    let mut workload = base_workload.clone();
    workload["unexpected"] = json!(true);
    rejects(base_model.clone(), workload);
    let mut model = base_model.clone();
    model["unexpected"] = json!(true);
    rejects(model, base_workload.clone());

    let model = serde_json::to_string(&base_model).unwrap();
    let workload = serde_json::to_string(&base_workload).unwrap();
    let duplicate_model = model.replacen(
        "\"schema_version\":3",
        "\"schema_version\":3,\"schema_version\":3",
        1,
    );
    let (_temp, config) = write_case(&duplicate_model, &workload, 4_000_000);
    assert!(
        prepare(&config).is_err(),
        "duplicate model key was accepted"
    );
    let duplicate_workload = workload.replacen("\"tag\":null", "\"tag\":null,\"tag\":null", 1);
    let (_temp, config) = write_case(&model, &duplicate_workload, 4_000_000);
    assert!(
        prepare(&config).is_err(),
        "duplicate workload key was accepted"
    );
    let mut empty = base_workload;
    let generator = generator_mut(&mut empty, "vlan_unicast");
    generator["times_ps"] = json!([]);
    generator["frame"]["tag"] = json!({"vid":4095,"pcp":7,"dei":0});
    rejects(base_model, empty);
}
