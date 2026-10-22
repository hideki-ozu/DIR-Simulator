//! The active host/router controls must affect real network forwarding.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/active-multicast/model.ini")
}

#[test]
fn igmpv3_and_mldv2_membership_changes_filter_real_receptions() {
    let output = std::env::temp_dir().join(format!(
        "dir-active-multicast-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = run(prepare(&fixture()).unwrap(), &output).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(result.output_path.join("results.json")).unwrap())
            .unwrap();
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let delivered = |request: &str, ingress: &str| {
        rows.iter().any(|row| {
            row["schema_name"] == "ethernet.dynamic.reception"
                && row["request_id"] == request
                && row["data"]["ingress"] == ingress
                && row["data"]["status"] == "received"
        })
    };
    let forwarded = |request: &str, port: &str| {
        rows.iter().any(|row| {
            row["schema_name"] == "ethernet.dynamic.transfer"
                && row["request_id"] == request
                && row["data"]["from_port"] == port
                && row["data"]["status"] == "serialized"
        })
    };

    // Initial INCLUDE permits only one IPv4 source; its later socket change
    // admits the second source. The leave stops delivery on the receiver branch.
    assert!(delivered("ipv4:0", "Net.b.rx"));
    assert!(!forwarded("ipv4_other:0", "Net.s1.tx_b"));
    assert!(delivered("ipv4_other:1", "Net.b.rx"));
    assert!(!forwarded("ipv4:3", "Net.s1.tx_b"));
    assert!(!delivered("ipv4:3", "Net.b.rx"));

    // MLDv2 has the same timed membership effect on the IPv6 branch.
    assert!(delivered("ipv6:0", "Net.b.rx"));
    assert!(delivered("ipv6:2", "Net.b.rx"));
    assert!(!forwarded("ipv6:3", "Net.s1.tx_b"));
    assert!(!delivered("ipv6:3", "Net.b.rx"));

    // These decisions follow accepted control packets, not static endpoint lists.
    for family in ["ipv4", "ipv6"] {
        assert!(rows.iter().any(|row| {
            row["schema_name"] == "ethernet.wire.membership"
                && row["data"]["family"] == family
                && row["data"]["message_kind"] == "report"
                && row["data"]["outcome"] == "accepted"
        }));
    }
    fs::remove_dir_all(output).unwrap();
}

#[test]
fn two_exclude_listeners_share_one_bridge_uplink_for_both_families() {
    let fixture_path = fixture();
    let base = fixture_path.parent().unwrap();
    let work = std::env::temp_dir().join(format!(
        "dir-multicast-shared-port-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&work).unwrap();
    let mut model: Value =
        serde_json::from_slice(&fs::read(base.join("model.json")).unwrap()).unwrap();
    let links = model["base"]["config"]["base"]["config"]["dynamic"]["links"]
        .as_array_mut()
        .unwrap();
    links.retain(|link| link["id"] != "L23" && link["id"] != "L13");
    let c = links.iter_mut().find(|link| link["id"] == "C").unwrap();
    c["ports"][1] = json!("Net.s2.tx_c");
    let l2 = &mut model["base"]["config"]["base"]["config"];
    l2["switches"]
        .as_array_mut()
        .unwrap()
        .retain(|row| row["instance"] != "Net.s3");
    l2["dynamic"]["bridges"]
        .as_array_mut()
        .unwrap()
        .retain(|row| row["instance"] != "Net.s3");
    for name in ["ports", "outputs"] {
        l2[name].as_array_mut().unwrap().retain(|row| {
            !row["port"].as_str().unwrap().starts_with("Net.s3.") && row["port"] != "Net.s1.tx_c"
        });
    }
    l2["dynamic"]["registrable"]
        .as_array_mut()
        .unwrap()
        .retain(|row| {
            !row["port"].as_str().unwrap().starts_with("Net.s3.") && row["port"] != "Net.s1.tx_c"
        });
    model["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
        .retain(|row| {
            !row["port"].as_str().unwrap().starts_with("Net.s3.") && row["port"] != "Net.s1.tx_c"
        });
    let interfaces = model["multicast_active"]["interfaces"]
        .as_array_mut()
        .unwrap();
    for iface in interfaces.iter_mut() {
        if iface["id"] == "router4" {
            iface["id"] = json!("receiver4_c");
            iface["router"] = json!(false);
        } else if iface["id"] == "router6" {
            iface["id"] = json!("receiver6_c");
            iface["router"] = json!(false);
        }
    }
    model["multicast_active"]["changes"] = json!([
        {"at_ps":"0", "interface":"receiver4", "socket":"b4", "group":"239.1.1.1",
            "filter":{"mode":"exclude", "sources":["192.0.2.1"]}},
        {"at_ps":"0", "interface":"receiver4_c", "socket":"c4", "group":"239.1.1.1",
            "filter":{"mode":"exclude", "sources":["192.0.2.44"]}},
        {"at_ps":"0", "interface":"receiver6", "socket":"b6", "group":"ff3e::1",
            "filter":{"mode":"exclude", "sources":["2001:db8::1"]}},
        {"at_ps":"0", "interface":"receiver6_c", "socket":"c6", "group":"ff3e::1",
            "filter":{"mode":"exclude", "sources":["2001:db8::2"]}}
    ]);
    let mut workload: Value =
        serde_json::from_slice(&fs::read(base.join("workload.json")).unwrap()).unwrap();
    let generators = workload["generators"].as_array_mut().unwrap();
    for generator in generators.iter_mut() {
        generator["times_ps"] = json!(["500000000000", "1200000000000"]);
    }
    let mut other6 = generators
        .iter()
        .find(|g| g["id"] == "ipv6")
        .unwrap()
        .clone();
    other6["id"] = json!("ipv6_other");
    other6["flow_id"] = json!("ipv6_other");
    other6["frame"]["ip_multicast"]["source"] = json!("2001:db8::2");
    generators.push(other6);
    fs::write(
        work.join("model.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    fs::write(
        work.join("workload.json"),
        serde_json::to_vec_pretty(&workload).unwrap(),
    )
    .unwrap();
    let ned_dir = work.join("models/ethdynamic");
    fs::create_dir_all(&ned_dir).unwrap();
    let ned = r#"package ethdynamic;
simple Endpoint {
 parameters:
  @class("dir.ethernet.Endpoint");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple Switch2 {
 parameters:
  @class("dir.ethernet.Switch");
  int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b;
}
simple Switch3 {
 parameters:
  @class("dir.ethernet.Switch");
  int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b; input rx_c; output tx_c;
}
channel Wire {
 parameters:
  @class("dir.ethernet.Link");
  double bitrate @unit(bps) = default(1Gbps);
  double delay @unit(s) = default(1000ps);
}
network Net {
 submodules:
  a: ethdynamic.Endpoint; b: ethdynamic.Endpoint; c: ethdynamic.Endpoint;
  s1: ethdynamic.Switch2; s2: ethdynamic.Switch3;
 connections:
  a.tx --> ethdynamic.Wire --> s1.rx_a;
  s1.tx_a --> ethdynamic.Wire --> a.rx;
  b.tx --> ethdynamic.Wire --> s2.rx_a;
  s2.tx_a --> ethdynamic.Wire --> b.rx;
  c.tx --> ethdynamic.Wire --> s2.rx_c;
  s2.tx_c --> ethdynamic.Wire --> c.rx;
  s1.tx_b --> ethdynamic.Wire --> s2.rx_b;
  s2.tx_b --> ethdynamic.Wire --> s1.rx_b;
}
"#;
    fs::write(ned_dir.join("Main.ned"), ned).unwrap();
    fs::write(work.join("model.ini"), format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 2000000000000ps\n",
        work.join("models").display()
    )).unwrap();
    let result = run(prepare(&work.join("model.ini")).unwrap(), &work.join("out")).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(result.output_path.join("results.json")).unwrap())
            .unwrap();
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let delivered = |request: &str, ingress: &str| {
        rows.iter().any(|row| {
            row["schema_name"] == "ethernet.dynamic.reception"
                && row["request_id"] == request
                && row["data"]["ingress"] == ingress
                && row["data"]["status"] == "received"
        })
    };
    let upstream = |request: &str| {
        rows.iter().any(|row| {
            row["schema_name"] == "ethernet.dynamic.transfer"
                && row["request_id"] == request
                && row["data"]["from_port"] == "Net.s1.tx_b"
                && row["data"]["status"] == "serialized"
        })
    };
    for request in ["ipv4:0", "ipv4:1", "ipv6:0", "ipv6:1"] {
        assert!(upstream(request), "{request} must cross the shared uplink");
        assert!(
            delivered(request, "Net.c.rx"),
            "{request} must reach the second listener"
        );
        assert!(
            !delivered(request, "Net.b.rx"),
            "{request} is excluded by the first listener"
        );
    }
    for request in [
        "ipv4_other:0",
        "ipv4_other:1",
        "ipv6_other:0",
        "ipv6_other:1",
    ] {
        assert!(upstream(request), "{request} must cross the shared uplink");
        assert!(
            delivered(request, "Net.b.rx"),
            "{request} must reach the first listener"
        );
        assert!(
            !delivered(request, "Net.c.rx"),
            "{request} is excluded by the second listener"
        );
    }

    // After b narrows its filter to source B and c leaves, no listener requests
    // source A. Current-state answers now cannot refresh A during the probe.
    let interfaces = model["multicast_active"]["interfaces"]
        .as_array_mut()
        .unwrap();
    for (template, id, address) in [
        ("receiver4", "querier4", "192.0.2.1"),
        ("receiver6", "querier6", "fe80::1"),
    ] {
        let mut router = interfaces
            .iter()
            .find(|iface| iface["id"] == template)
            .unwrap()
            .clone();
        router["id"] = json!(id);
        router["output_port"] = json!("Net.a.tx");
        router["address"] = json!(address);
        router["mac"] = json!("02:00:00:00:00:01");
        router["router"] = json!(true);
        router["query_interval_ps"] = json!("8000000000000");
        interfaces.push(router);
    }
    model["multicast_active"]["changes"].as_array_mut().unwrap().extend([
        json!({"at_ps":"1400000000000", "interface":"receiver4", "socket":"b4", "group":"239.1.1.1", "filter":{"mode":"include", "sources":["192.0.2.44"]}}),
        json!({"at_ps":"1400000000000", "interface":"receiver6", "socket":"b6", "group":"ff3e::1", "filter":{"mode":"include", "sources":["2001:db8::2"]}}),
        json!({"at_ps":"1500000000000", "interface":"receiver4_c", "socket":"c4", "group":"239.1.1.1", "filter":null}),
        json!({"at_ps":"1500000000000", "interface":"receiver6_c", "socket":"c6", "group":"ff3e::1", "filter":null}),
    ]);
    for generator in workload["generators"].as_array_mut().unwrap() {
        generator["times_ps"] = json!(["500000000000", "1200000000000", "1800000000000"]);
    }
    fs::write(
        work.join("model-loss.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    fs::write(
        work.join("workload-loss.json"),
        serde_json::to_vec_pretty(&workload).unwrap(),
    )
    .unwrap();
    fs::write(work.join("model-loss.ini"), format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model-loss.json\"\nworkload = \"workload-loss.json\"\nsim-time-limit = 2000000000000ps\n",
        work.join("models").display()
    )).unwrap();
    let loss = run(
        prepare(&work.join("model-loss.ini")).unwrap(),
        &work.join("out-loss"),
    )
    .unwrap();
    let loss: Value =
        serde_json::from_slice(&fs::read(loss.output_path.join("results.json")).unwrap()).unwrap();
    let loss_rows = loss["simulation"]["model_records"].as_array().unwrap();
    for family in ["ipv4", "ipv6"] {
        assert!(
            loss_rows
                .iter()
                .any(|row| row["schema_name"] == "ethernet.wire.membership"
                    && row["data"]["family"] == family
                    && row["data"]["message_kind"] == "query"
                    && row["data"]["switch"] == "Net.s1"
                    && row["time_ps"]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok())
                        .is_some_and(|t| t > 1_500_000_000_000)),
            "{family} last-member query must reach the upstream bridge"
        );
        let request = format!("{family}:2");
        assert!(
            loss_rows
                .iter()
                .any(|row| row["schema_name"] == "ethernet.dynamic.reception"
                    && row["request_id"] == request
                    && row["data"]["ingress"] == "Net.s1.rx_a"
                    && row["data"]["reason"] == "multicast_filtered"),
            "{family} unanswered query must prune forwarding"
        );
        let other = format!("{family}_other:2");
        assert!(
            loss_rows
                .iter()
                .any(|row| row["schema_name"] == "ethernet.dynamic.reception"
                    && row["request_id"] == other
                    && row["data"]["ingress"] == "Net.b.rx"
                    && row["data"]["status"] == "received"),
            "{family} other source must retain delivery"
        );
    }

    workload["controls"] = json!([
        {"id":"query_loss", "at_ps":"1500002800000", "kind":"link_set", "link":"L12", "up":false},
        {"id":"restore", "at_ps":"1750000000000", "kind":"link_set", "link":"L12", "up":true}
    ]);
    fs::write(
        work.join("workload-loss-link.json"),
        serde_json::to_vec_pretty(&workload).unwrap(),
    )
    .unwrap();
    fs::write(work.join("model-loss-link.ini"), format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model-loss.json\"\nworkload = \"workload-loss-link.json\"\nsim-time-limit = 2000000000000ps\n",
        work.join("models").display()
    )).unwrap();
    let loss_link = run(
        prepare(&work.join("model-loss-link.ini")).unwrap(),
        &work.join("out-loss-link"),
    )
    .unwrap();
    let loss_link: Value =
        serde_json::from_slice(&fs::read(loss_link.output_path.join("results.json")).unwrap())
            .unwrap();
    let link_rows = loss_link["simulation"]["model_records"].as_array().unwrap();
    assert!(
        link_rows
            .iter()
            .any(|row| row["schema_name"] == "ethernet.wire.membership"
                && row["data"]["message_kind"] == "query"
                && row["data"]["outcome"] == "control_drop"
                && row["data"]["reason"] == "ingress_ineligible"),
        "failed uplink must drop an actual query control frame"
    );
    for family in ["ipv4", "ipv6"] {
        let in_window = |row: &&Value| {
            row["time_ps"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .is_some_and(|t| (1_500_000_000_000..1_750_000_000_000).contains(&t))
        };
        assert!(
            link_rows
                .iter()
                .filter(in_window)
                .any(|row| row["schema_name"] == "ethernet.wire.membership"
                    && row["data"]["family"] == family
                    && row["data"]["message_kind"] == "query"
                    && row["data"]["switch"] == "Net.s1"),
            "{family} probe must reach the upstream side of failed link"
        );
        assert!(
            !link_rows
                .iter()
                .filter(in_window)
                .any(|row| row["schema_name"] == "ethernet.wire.membership"
                    && row["data"]["family"] == family
                    && row["data"]["message_kind"] == "query"
                    && row["data"]["switch"] == "Net.s2"
                    && row["data"]["outcome"] == "accepted"),
            "{family} probe must not be accepted downstream"
        );
        let request = format!("{family}:2");
        assert!(
            link_rows
                .iter()
                .any(|row| row["schema_name"] == "ethernet.dynamic.reception"
                    && row["request_id"] == request
                    && row["data"]["ingress"] == "Net.s1.rx_a"
                    && row["data"]["reason"] == "multicast_filtered"),
            "{family} lost query must let the source timer expire"
        );
    }

    for interface in model["multicast_active"]["interfaces"]
        .as_array_mut()
        .unwrap()
    {
        if interface["id"] == "receiver4_c" || interface["id"] == "receiver6_c" {
            interface["router"] = json!(true);
        }
    }
    model["multicast_active"]["changes"]
        .as_array_mut()
        .unwrap()
        .retain(|change| change["interface"] == "receiver4" || change["interface"] == "receiver6");
    workload["generators"] = json!([]);
    workload["controls"] = json!([
        {"id":"isolate_lower_querier", "at_ps":"1000000000000", "kind":"link_set", "link":"L12", "up":false},
        {"id":"restore_lower_querier", "at_ps":"17500000000000", "kind":"link_set", "link":"L12", "up":true}
    ]);
    fs::write(
        work.join("model-takeover.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    fs::write(
        work.join("workload-takeover.json"),
        serde_json::to_vec_pretty(&workload).unwrap(),
    )
    .unwrap();
    fs::write(work.join("model-takeover.ini"), format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model-takeover.json\"\nworkload = \"workload-takeover.json\"\nsim-time-limit = 18000000000000ps\n",
        work.join("models").display()
    )).unwrap();
    let takeover = run(
        prepare(&work.join("model-takeover.ini")).unwrap(),
        &work.join("out-takeover"),
    )
    .unwrap();
    let takeover: Value =
        serde_json::from_slice(&fs::read(takeover.output_path.join("results.json")).unwrap())
            .unwrap();
    let takeover_rows = takeover["simulation"]["model_records"].as_array().unwrap();
    for family in ["ipv4", "ipv6"] {
        let queries: Vec<u64> = takeover_rows
            .iter()
            .filter(|row| {
                row["schema_name"] == "ethernet.wire.membership"
                    && row["data"]["family"] == family
                    && row["data"]["message_kind"] == "query"
                    && row["data"]["switch"] == "Net.s2"
                    && row["data"]["ingress_port"] == "Net.s2.tx_c"
                    && row["data"]["outcome"] == "accepted"
            })
            .filter_map(|row| row["time_ps"].as_str().and_then(|s| s.parse().ok()))
            .collect();
        assert!(
            queries.iter().any(|t| *t < 1_000_000_000_000),
            "{family} upper-address router must initially query"
        );
        assert!(
            !queries
                .iter()
                .any(|t| (1_000_000_000_000..16_000_000_000_000).contains(t)),
            "{family} upper-address router must yield to lower querier"
        );
        assert!(
            queries
                .iter()
                .any(|t| (16_000_000_000_000..17_500_000_000_000).contains(t)),
            "{family} upper-address router must take over after other-querier timeout"
        );
    }
    fs::remove_dir_all(work).unwrap();
}
