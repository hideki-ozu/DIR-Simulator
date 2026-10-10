//! Strict composite schema and canonical Gateway ownership/rule preparation.
use super::{JsonDocument, can, ethernet};
use crate::types::{Diagnostic, PreparedCan, can_ethernet::*, ethernet::PreparedEthernet};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(path: &str, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(msg).with_target(path)
}
fn object<'a>(value: &'a Value, fields: &[&str], path: &str) -> Result<&'a Map<String, Value>> {
    let o = value
        .as_object()
        .ok_or_else(|| error(path, "expected object"))?;
    if o.len() != fields.len()
        || fields.iter().any(|f| !o.contains_key(*f))
        || o.keys().any(|k| !fields.contains(&k.as_str()))
    {
        return Err(error(
            path,
            format!("required exact fields: {}", fields.join(", ")),
        ));
    }
    Ok(o)
}
fn string<'a>(o: &'a Map<String, Value>, key: &str, path: &str) -> Result<&'a str> {
    o[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error(&format!("{path}/{key}"), "expected nonempty string"))
}
fn number(o: &Map<String, Value>, key: &str, min: u64, max: u64, path: &str) -> Result<u64> {
    o[key]
        .as_u64()
        .filter(|n| (*n >= min) && (*n <= max))
        .ok_or_else(|| {
            error(
                &format!("{path}/{key}"),
                format!("integer must be {min}..{max}"),
            )
        })
}
fn array<'a>(o: &'a Map<String, Value>, key: &str, path: &str) -> Result<&'a Vec<Value>> {
    o[key]
        .as_array()
        .ok_or_else(|| error(&format!("{path}/{key}"), "expected array"))
}
fn decimal(o: &Map<String, Value>, key: &str, path: &str) -> Result<u64> {
    let s = string(o, key, path)?;
    if !s.bytes().all(|b| b.is_ascii_digit()) || s.len() > 1 && s.starts_with('0') {
        return Err(error(path, "expected canonical u64 decimal string"));
    }
    s.parse().map_err(|_| error(path, "decimal exceeds u64"))
}
fn can_key(o: &Map<String, Value>, path: &str) -> Result<(CanFormat, u32)> {
    let format = match string(o, "format", path)? {
        "standard" => CanFormat::Standard,
        "extended" => CanFormat::Extended,
        _ => return Err(error(path, "unsupported CAN format")),
    };
    let id = number(o, "can_id", 0, u64::from(format.max_id()), path)? as u32;
    Ok((format, id))
}
pub(crate) fn configure(
    content: &str,
    can_base: &PreparedCan,
    ethernet_base: &mut PreparedEthernet,
    paths: &[String],
) -> Result<PreparedCanEthernet> {
    let document = JsonDocument::parse(content)?;
    configure_inner(content, can_base, ethernet_base, paths).map_err(|diagnostic| {
        let pointer = diagnostic.target.clone().unwrap_or_default();
        document.annotate(&pointer, diagnostic)
    })
}
fn configure_inner(
    content: &str,
    can_base: &PreparedCan,
    ethernet_base: &mut PreparedEthernet,
    paths: &[String],
) -> Result<PreparedCanEthernet> {
    let doc = JsonDocument::parse(content)?;
    let root = object(
        &doc.value,
        &["schema_version", "can", "ethernet", "gateways"],
        "",
    )?;
    if root["schema_version"].as_u64() != Some(1) {
        return Err(error(
            "/schema_version",
            "composite config requires schema 1",
        ));
    }
    object(&root["can"], &[], "/can")?;
    ethernet::configure(
        &root["ethernet"].to_string(),
        ethernet_base,
        "ethernet.l2.vlan.v1",
    )
    .map_err(|diagnostic| {
        let pointer = format!("/ethernet{}", diagnostic.target.as_deref().unwrap_or(""));
        diagnostic.with_target(pointer)
    })?;
    let mut gateways = Vec::new();
    let mut instances = BTreeSet::new();
    let mut owned_can = BTreeSet::new();
    let mut owned_eth = BTreeSet::new();
    for (i, value) in array(root, "gateways", "")?.iter().enumerate() {
        let p = format!("/gateways/{i}");
        let o = object(
            value,
            &[
                "instance",
                "can_ports",
                "ethernet_endpoint",
                "rx_capacity",
                "conversion_delay_ps",
                "max_hops",
                "rules",
            ],
            &p,
        )?;
        let instance = string(o, "instance", &p)?;
        if !paths.iter().any(|p| p == instance) || !instances.insert(instance.to_owned()) {
            return Err(error(&p, "Gateway instance unknown or duplicate"));
        }
        let prefix = format!("{instance}.");
        let mut can_ports = Vec::new();
        for v in array(o, "can_ports", &p)? {
            let path = v
                .as_str()
                .ok_or_else(|| error(&p, "CAN port must be Controller path"))?;
            let index = can_base
                .controllers
                .iter()
                .position(|c| c.id == path)
                .ok_or_else(|| error(&p, "unknown Gateway Controller"))?;
            if !path.starts_with(&prefix) || !owned_can.insert(index) {
                return Err(error(&p, "Controller is outside Gateway or multiply owned"));
            }
            can_ports.push(index);
        }
        if can_ports.is_empty() {
            return Err(error(&p, "Gateway requires at least one CAN Controller"));
        }
        can_ports.sort_by_key(|i| &can_base.controllers[*i].id);
        let eth = string(o, "ethernet_endpoint", &p)?;
        let eth_index = ethernet_base
            .devices
            .iter()
            .position(|d| d.id == eth && d.kind == "endpoint")
            .ok_or_else(|| error(&p, "Gateway requires Ethernet Endpoint"))?;
        if !eth.starts_with(&prefix) || !owned_eth.insert(eth_index) {
            return Err(error(&p, "Endpoint is outside Gateway or multiply owned"));
        }
        let mut rules = Vec::new();
        let mut rule_ids = BTreeSet::new();
        let mut matches = BTreeSet::new();
        for (j, value) in array(o, "rules", &p)?.iter().enumerate() {
            let rp = format!("{p}/rules/{j}");
            let r = object(
                value,
                &["id", "direction", "ingress", "match", "egresses"],
                &rp,
            )?;
            let id = string(r, "id", &rp)?;
            if !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !rule_ids.insert(id.to_owned())
            {
                return Err(error(
                    &rp,
                    "rule ID must be unique ASCII alphanumeric/underscore/hyphen",
                ));
            }
            let direction = string(r, "direction", &rp)?;
            let ingress = string(r, "ingress", &rp)?;
            let to_eth = match direction {
                "can_to_ethernet" => true,
                "ethernet_to_can" => false,
                _ => return Err(error(&rp, "only cross-media conversion is supported")),
            };
            if to_eth
                && !can_ports
                    .iter()
                    .any(|i| can_base.controllers[*i].id == ingress)
                || !to_eth && ingress != eth
            {
                return Err(error(&rp, "rule ingress is not owned media endpoint"));
            }
            let m = object(
                &r["match"],
                if to_eth {
                    &["format", "can_id"]
                } else {
                    &["vid", "pcp", "format", "can_id"]
                },
                &format!("{rp}/match"),
            )?;
            let (format, can_id) = can_key(m, &rp)?;
            let vid = if to_eth {
                None
            } else {
                Some(number(m, "vid", 1, 4094, &rp)? as u16)
            };
            let pcp = if to_eth {
                None
            } else {
                Some(number(m, "pcp", 0, 7, &rp)? as u8)
            };
            if !matches.insert((
                direction.to_owned(),
                ingress.to_owned(),
                format.clone(),
                can_id,
                vid,
                pcp,
            )) {
                return Err(error(&rp, "overlapping exact rule match"));
            }
            let mut egresses = Vec::new();
            let mut ports = BTreeSet::new();
            for (k, value) in array(r, "egresses", &rp)?.iter().enumerate() {
                let ep = format!("{rp}/egresses/{k}");
                let e = object(
                    value,
                    if to_eth {
                        &["port", "dst_mac", "vid", "pcp"]
                    } else {
                        &["port", "format", "can_id"]
                    },
                    &ep,
                )?;
                let port = string(e, "port", &ep)?;
                if !ports.insert(port.to_owned()) {
                    return Err(error(&ep, "duplicate rule egress"));
                }
                let egress = if to_eth {
                    if port != format!("{eth}.tx") {
                        return Err(error(&ep, "Ethernet egress is not owned endpoint output"));
                    }
                    let dst_mac = ethernet::destination_mac(string(e, "dst_mac", &ep)?)
                        .map_err(|diagnostic| diagnostic.with_target(format!("{ep}/dst_mac")))?;
                    let v = number(e, "vid", 1, 4094, &ep)? as u16;
                    let priority = number(e, "pcp", 0, 7, &ep)? as u8;
                    if !ethernet_base
                        .port_policies
                        .iter()
                        .any(|p| p.port == port && p.vlans.contains_key(&v))
                    {
                        return Err(error(&ep, "egress VID is not static member"));
                    }
                    BridgeEgress::Ethernet {
                        source: eth_index,
                        port: port.into(),
                        dst_mac,
                        vid: v,
                        pcp: priority,
                    }
                } else {
                    let source = can_ports
                        .iter()
                        .copied()
                        .find(|i| format!("{}.tx", can_base.controllers[*i].id) == port)
                        .ok_or_else(|| error(&ep, "CAN egress is not owned Controller output"))?;
                    let (format, can_id) = can_key(e, &ep)?;
                    BridgeEgress::Can {
                        source,
                        port: port.into(),
                        format,
                        can_id,
                    }
                };
                egresses.push(egress);
            }
            if egresses.is_empty() || to_eth && egresses.len() != 1 {
                return Err(error(
                    &rp,
                    "egresses must be nonempty; CAN-to-Ethernet has one branch",
                ));
            }
            egresses.sort_by(|a, b| a.port().cmp(b.port()));
            rules.push(BridgeRule {
                id: id.into(),
                direction: direction.into(),
                ingress: ingress.into(),
                format,
                can_id,
                vid,
                pcp,
                egresses,
            });
        }
        rules.sort_by(|a, b| a.id.cmp(&b.id));
        gateways.push(BridgeGateway {
            instance: instance.into(),
            can_ports,
            ethernet_endpoint: eth_index,
            rx_capacity: number(o, "rx_capacity", 0, u32::MAX.into(), &p)?,
            conversion_delay_ps: decimal(o, "conversion_delay_ps", &p)?,
            max_hops: number(o, "max_hops", 1, 255, &p)? as u8,
            rules,
        });
    }
    gateways.sort_by(|a, b| a.instance.cmp(&b.instance));
    Ok(PreparedCanEthernet {
        can: can_base.clone(),
        ethernet: ethernet_base.clone(),
        gateways,
    })
}
pub(crate) fn workload(content: &str, p: &mut PreparedCanEthernet) -> Result<()> {
    let document = JsonDocument::parse(content)?;
    workload_inner(content, p).map_err(|diagnostic| {
        let pointer = diagnostic.target.clone().unwrap_or_default();
        document.annotate(&pointer, diagnostic)
    })
}
fn workload_inner(content: &str, p: &mut PreparedCanEthernet) -> Result<()> {
    let doc = JsonDocument::parse(content)?;
    let root = object(&doc.value, &["schema_version", "can", "ethernet"], "")?;
    if root["schema_version"].as_u64() != Some(1) {
        return Err(error(
            "/schema_version",
            "composite workload requires schema 1",
        ));
    }
    p.can.generators = can::workload(
        &root["can"].to_string(),
        &p.can.controllers,
        &p.can.controller_buses,
        "can.cc.multibus.v1",
    )
    .map_err(|diagnostic| {
        let pointer = format!("/can{}", diagnostic.target.as_deref().unwrap_or(""));
        diagnostic.with_target(pointer)
    })?;
    ethernet::workload(
        &root["ethernet"].to_string(),
        &mut p.ethernet,
        "ethernet.l2.vlan.v1",
    )
    .map_err(|diagnostic| {
        let pointer = format!("/ethernet{}", diagnostic.target.as_deref().unwrap_or(""));
        diagnostic.with_target(pointer)
    })?;
    for g in &p.can.generators {
        if p.gateways.iter().any(|gw| gw.can_ports.contains(&g.source)) {
            return Err(error(
                "/can/generators",
                "Gateway owned Controller cannot be native source",
            ));
        }
    }
    for g in &p.ethernet.generators {
        if p.gateways.iter().any(|gw| gw.ethernet_endpoint == g.source) {
            return Err(error(
                "/ethernet/generators",
                "Gateway owned Endpoint cannot be native source",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    fn example(name: &str) -> (crate::types::PreparedSimulation, PreparedCanEthernet, Value) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/can-ethernet");
        let p = crate::input::prepare(&root.join(format!("{name}.ini"))).unwrap();
        let network = p.registered.as_ref().unwrap().network.as_ref().unwrap();
        let bridge = network.bridge.as_ref().unwrap().clone();
        (
            p,
            bridge,
            serde_json::from_str(
                &std::fs::read_to_string(root.join(format!("{name}-model.json"))).unwrap(),
            )
            .unwrap(),
        )
    }
    #[test]
    fn complete_native_r01_r02_inputs_validate() {
        for name in ["r01", "r02"] {
            let (_, p, _) = example(name);
            assert_eq!(p.gateways.len(), 1);
            assert_eq!(p.can.controllers.len(), 2);
            assert_eq!(p.ethernet.devices.len(), 2);
        }
    }
    #[test]
    fn strict_rules_ownership_and_ranges_reject() {
        let (p, bridge, config) = example("r01");
        let mut negatives = Vec::new();
        for (pointer, value) in [
            ("/gateways/0/rules/0/egresses/0/pcp", json!(true)),
            ("/gateways/0/rules/0/egresses/0/pcp", json!(8)),
            ("/gateways/0/rules/0/egresses/0/vid", json!(0)),
            ("/gateways/0/rules/0/match/can_id", json!(2048)),
            ("/gateways/0/max_hops", json!(0)),
            ("/gateways/0/rx_capacity", json!(4294967296u64)),
            ("/gateways/0/conversion_delay_ps", json!("01")),
            ("/gateways/0/can_ports", json!(["R01.src"])),
            ("/gateways/0/ethernet_endpoint", json!("R01.sink")),
            ("/can", json!({"unknown":1})),
        ] {
            let mut c = config.clone();
            *c.pointer_mut(pointer).unwrap() = value;
            negatives.push(c);
        }
        let mut c = config.clone();
        let r = c["gateways"][0]["rules"][0].clone();
        c["gateways"][0]["rules"].as_array_mut().unwrap().push(r);
        negatives.push(c);
        let mut c = config.clone();
        let mut r = c["gateways"][0]["rules"][0].clone();
        r["id"] = json!("duplicate_match");
        c["gateways"][0]["rules"].as_array_mut().unwrap().push(r);
        negatives.push(c);
        let mut c = config.clone();
        c["gateways"][0]["rules"][0]["match"]
            .as_object_mut()
            .unwrap()
            .remove("format");
        negatives.push(c);
        for c in negatives {
            assert!(
                configure(
                    &c.to_string(),
                    &bridge.can,
                    &mut bridge.ethernet.clone(),
                    &p.common.module_paths
                )
                .is_err(),
                "accepted {c}"
            );
        }
    }
    #[test]
    fn source_ownership_checked_for_zero_count_and_future_time() {
        let (_, mut bridge, _) = example("r01");
        let content = json!({"schema_version":1,"can":{"schema_version":2,"generators":[{"id":"owned","kind":"can.periodic.v1","node":"R01.gw.can","start":"0ps","period":"1ms","count":0,"frame":{"format":"standard","id":0,"data":""}}]},"ethernet":{"schema_version":3,"generators":[]}});
        assert!(
            workload(&content.to_string(), &mut bridge)
                .unwrap_err()
                .message
                .contains("Gateway owned")
        );
        let content = json!({"schema_version":1,"can":{"schema_version":2,"generators":[]},"ethernet":{"schema_version":3,"generators":[{"id":"owned","kind":"ethernet.explicit.v1","node":"R01.gw.eth","times_ps":["999999999999"],"frame":{"dst_mac":"02:00:00:00:00:02","ether_type":34997,"data":"4449524301000000000000","tag":null},"flow_id":"owned","priority":3,"deadline_ps":null}]}});
        assert!(
            workload(&content.to_string(), &mut bridge)
                .unwrap_err()
                .message
                .contains("Gateway owned")
        );
    }
}
