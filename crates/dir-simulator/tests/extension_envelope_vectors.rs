//! Public prepare boundary vectors. Names in synthetic fixtures expand to valid sample topology.
use dir_simulator::prepare;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn sample(kind: &str) -> (Value, Value) {
    let p = root().join("examples/ethernet-wire");
    let read = |suffix| {
        serde_json::from_slice(&std::fs::read(p.join(format!("{kind}-{suffix}.json"))).unwrap())
            .unwrap()
    };
    (read("model"), read("workload"))
}
fn prepare_fixture(
    c: &Value,
    w: &Value,
) -> Result<dir_simulator::PreparedSimulation, dir_simulator::Diagnostic> {
    prepare_fixture_with_ned(c, w, None)
}
fn prepare_fixture_with_ned(
    c: &Value,
    w: &Value,
    ned: Option<&str>,
) -> Result<dir_simulator::PreparedSimulation, dir_simulator::Diagnostic> {
    let f = Fixture(std::env::temp_dir().join(format!(
        "dir-envelope-vector-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir_all(&f.0).unwrap();
    std::fs::write(f.0.join("model.json"), c.to_string()).unwrap();
    std::fs::write(f.0.join("workload.json"), w.to_string()).unwrap();
    let ned_root = if let Some(ned) = ned {
        let root = f.0.join("ned");
        std::fs::create_dir_all(root.join("ethdynamic")).unwrap();
        std::fs::write(root.join("ethdynamic/Main.ned"), ned).unwrap();
        root
    } else {
        root().join("examples/ethernet/dynamic/models")
    };
    std::fs::write(f.0.join("run.ini"),format!("[General]\nnetwork=ethdynamic.Net\nned-path=\"{}\"\nmodel-profile=\"ethernet.control.wire.experimental.v1\"\nmodel-config=\"model.json\"\nworkload=\"workload.json\"\nsim-time-limit=100000000ps\n",ned_root.display())).unwrap();
    prepare(&f.0.join("run.ini"))
}
fn vectors(name: &str) -> Vec<Value> {
    serde_json::from_slice::<Value>(
        &std::fs::read(root().join(format!("docs/proposals/fixtures/{name}-v1.json"))).unwrap(),
    )
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn record_observation(
    id: &str,
    input: &Value,
    result: &Result<dir_simulator::PreparedSimulation, dir_simulator::Diagnostic>,
) {
    if let Some(root) = std::env::var_os("DIR_VECTOR_CLOSURE_OBSERVATIONS") {
        let root = PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        let observed = match result {
            Ok(_) => json!({"prepare":"accept","diagnostic":null}),
            Err(e) => {
                json!({"prepare":"reject","diagnostic":{"reason":e.reason,"message":e.message,"target":e.target}})
            }
        };
        std::fs::write(root.join(format!("{id}.json")),serde_json::to_vec_pretty(&json!({"id":id,"input":input,"observed":observed,"classification_basis":"Fixture condition-category labels classify the exact input and actual generic prepare diagnostic; they are not serialized Diagnostic.reason values."})).unwrap()).unwrap();
    }
}
#[test]
fn vector_envelope_discriminants_and_unknown_fields_public_prepare() {
    for v in vectors("common-contract")
        .into_iter()
        .filter(|v| v["kind"] == "wire_envelope")
    {
        let protocol = v["input"]["protocol"].as_str().unwrap();
        let active = protocol == "rstp";
        let (c, mut w) = sample(if active { "active" } else { "passive" });
        let mut packet = if active {
            json!({"id":"opaque","adapter_id":"rstp-s1","ingress_port":"Net.s1.rx_a","arrival_ps":"10000000","bytes_hex":"00Aa10ff"})
        } else {
            w["packets"][if protocol == "igmp" { 0 } else { 1 }].clone()
        };
        for field in ["vid", "source_mac", "destination_mac"] {
            packet.as_object_mut().unwrap().remove(field);
        }
        for field in v["input"]["extra_fields"].as_array().unwrap() {
            let field = field.as_str().unwrap();
            packet[field] = match field {
                "vid" => json!(10),
                "source_mac" => json!("02:00:00:00:00:01"),
                "destination_mac" => json!("01:00:5e:00:00:16"),
                _ => json!("unexpected"),
            };
        }
        packet["encapsulation"] = v["input"]["encapsulation"].clone();
        w["packets"] = json!([packet]);
        let result = prepare_fixture(&c, &w);
        record_observation(v["id"].as_str().unwrap(), &v["input"], &result);
        assert_eq!(
            result.is_ok(),
            v["expected"]["prepare"] == "accept",
            "{}: {:?}",
            v["id"],
            result.err()
        );
    }
    let (c, mut w) = sample("passive");
    w["packets"].as_array_mut().unwrap().truncate(1);
    w["packets"][0]["unexpected"] = json!(1);
    let result = prepare_fixture(&c, &w);
    record_observation(
        "P-MVRP-T02-V06",
        &json!({"packet_extra_field":"unexpected"}),
        &result,
    );
    let e = result.unwrap_err();
    assert!(e.message.contains("unknown"), "{e:?}");
}
#[test]
fn vector_outer_hex_and_ownership_rejected_by_public_prepare() {
    for v in vectors("bridge-wire")
        .into_iter()
        .filter(|v| v["kind"] == "hex_validation")
    {
        let (c, mut w) = sample("active");
        w["packets"] = json!([{"id":"hex","adapter_id":"rstp-s1","ingress_port":"Net.s1.rx_a","arrival_ps":"10000000","encapsulation":"mac_frame_fcs","bytes_hex":v["input"]["bytes_hex"]}]);
        let result = prepare_fixture(&c, &w);
        record_observation(v["id"].as_str().unwrap(), &v["input"], &result);
        let e = result.unwrap_err();
        assert!(e.message.contains("hex"), "{} {e:?}", v["id"]);
    }
    let (c, mut w) = sample("active");
    w["packets"] = json!([{"id":"foreign","adapter_id":"rstp-s1","ingress_port":"Net.s2.rx_a","arrival_ps":"10000000","encapsulation":"mac_frame_fcs","bytes_hex":"00"}]);
    let e = prepare_fixture(&c, &w).unwrap_err();
    assert!(e.message.contains("ownership"), "{e:?}");
}
#[test]
fn vector_bridge_scope_public_prepare() {
    for v in vectors("bridge-integration")
        .into_iter()
        .filter(|v| v["kind"] == "bridge_scope")
    {
        let (mut c, w) = sample("active");
        let count = v["input"]["bridges"].as_array().unwrap().len();
        let ports = if count == 1 {
            vec!["Net.a.tx", "Net.b.tx", "Net.s1.tx_a", "Net.s1.tx_b"]
        } else {
            vec![
                "Net.a.tx",
                "Net.b.tx",
                "Net.s1.tx_a",
                "Net.s1.tx_b",
                "Net.s2.tx_a",
                "Net.s2.tx_b",
            ]
        };
        c["ethernet"]["endpoints"]
            .as_array_mut()
            .unwrap()
            .retain(|d| d["instance"] == "Net.a" || d["instance"] == "Net.b");
        c["ethernet"]["switches"]
            .as_array_mut()
            .unwrap()
            .retain(|d| d["instance"] == "Net.s1" || count == 2 && d["instance"] == "Net.s2");
        for sw in c["ethernet"]["switches"].as_array_mut().unwrap() {
            sw["fdb"]
                .as_array_mut()
                .unwrap()
                .retain(|f| ports.contains(&f["egress"].as_str().unwrap()));
        }
        for field in ["ports", "outputs"] {
            c["ethernet"][field]
                .as_array_mut()
                .unwrap()
                .retain(|p| ports.contains(&p["port"].as_str().unwrap()));
        }
        c["ethernet"]["dynamic"]["bridges"]
            .as_array_mut()
            .unwrap()
            .retain(|b| b["instance"] == "Net.s1" || count == 2 && b["instance"] == "Net.s2");
        if count == 1 {
            for link in c["ethernet"]["dynamic"]["links"].as_array_mut().unwrap() {
                if link["id"] == "B" {
                    link["ports"] = json!(["Net.b.tx", "Net.s1.tx_b"]);
                }
            }
        }
        c["ethernet"]["dynamic"]["links"]
            .as_array_mut()
            .unwrap()
            .retain(|l| {
                l["ports"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|p| ports.contains(&p.as_str().unwrap()))
            });
        c["ethernet"]["dynamic"]["registrable"]
            .as_array_mut()
            .unwrap()
            .retain(|p| ports.contains(&p["port"].as_str().unwrap()));
        c["port_identities"]
            .as_array_mut()
            .unwrap()
            .retain(|p| ports.contains(&p["port"].as_str().unwrap()));
        c["adapters"].as_array_mut().unwrap().retain(|a| {
            let id = a["id"].as_str().unwrap();
            let b = if id.ends_with("s1") {
                0
            } else if id.ends_with("s2") {
                1
            } else {
                2
            };
            b < count
                && (a["protocol"] == "rstp"
                    || v["input"]["mvrp_bridges"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|x| x == if b == 0 { "a" } else { "b" }))
        });
        for a in c["adapters"].as_array_mut().unwrap() {
            a["ports"]
                .as_array_mut()
                .unwrap()
                .retain(|p| ports.contains(&p.as_str().unwrap()));
            if a["protocol"] == "rstp" {
                a["rstp"]["port_parameters"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|p| ports.contains(&p["port"].as_str().unwrap()));
            }
        }
        c["ethernet"]["dynamic"]["convergence_ps"] = v["input"]["convergence_ps"].clone();
        let source = std::fs::read_to_string(
            root().join("examples/ethernet/dynamic/models/ethdynamic/Main.ned"),
        )
        .unwrap();
        let prefix = source
            .split("network Net")
            .next()
            .unwrap()
            .replace("  input rx_c; output tx_c;", "");
        let topology = if count == 1 {
            "network Net { submodules: a: ethdynamic.Endpoint; b: ethdynamic.Endpoint; s1: ethdynamic.Switch; connections: a.tx --> ethdynamic.Wire --> s1.rx_a; s1.tx_a --> ethdynamic.Wire --> a.rx; b.tx --> ethdynamic.Wire --> s1.rx_b; s1.tx_b --> ethdynamic.Wire --> b.rx; }"
        } else {
            "network Net { submodules: a: ethdynamic.Endpoint; b: ethdynamic.Endpoint; s1: ethdynamic.Switch; s2: ethdynamic.Switch; connections: a.tx --> ethdynamic.Wire --> s1.rx_a; s1.tx_a --> ethdynamic.Wire --> a.rx; b.tx --> ethdynamic.Wire --> s2.rx_a; s2.tx_a --> ethdynamic.Wire --> b.rx; s1.tx_b --> ethdynamic.Wire --> s2.rx_b; s2.tx_b --> ethdynamic.Wire --> s1.rx_b; }"
        };
        let result = prepare_fixture_with_ned(&c, &w, Some(&format!("{prefix}{topology}")));
        record_observation(v["id"].as_str().unwrap(), &v["input"], &result);
        assert_eq!(
            result.is_ok(),
            v["expected"]["accept"].as_bool().unwrap(),
            "{} {:?}",
            v["id"],
            result.as_ref().err()
        );
        if let Err(error) = result {
            assert!(
                error.message.contains(if count == 1 {
                    "convergence"
                } else {
                    "every bridge owner"
                }),
                "{} {error:?}",
                v["id"]
            );
        }
    }
}
