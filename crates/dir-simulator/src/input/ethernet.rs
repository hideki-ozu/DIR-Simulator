//! Ethernet schema, tree topology, static FDB and explicit workload validation.
use super::ned::{self, Declaration, ModelRules, TypedValue, Values};
use super::{Result, StrictJson, error, identifier, object, required_string, unsigned};
use crate::types::ethernet::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
struct EthernetRules;
impl ModelRules for EthernetRules {
    fn validate_schema(&self, d: &Declaration) -> Result<()> {
        match d.implementation() {
            Some("dir.ethernet.Endpoint") if d.simple() => {
                d.require_parameters(&[
                    ("queueCapacity", "int", None),
                    ("txProcessingDelay", "double", Some("s")),
                    ("rxProcessingDelay", "double", Some("s")),
                ])?;
                if *d.gates() != BTreeMap::from([("tx".into(), true), ("rx".into(), false)]) {
                    return Err(d.fail("Endpoint requires output tx/input rx"));
                }
            }
            Some("dir.ethernet.Switch") if d.simple() => {
                d.require_parameters(&[("queueCapacity", "int", None)])?;
                let outputs: Vec<_> = d.gates().iter().filter(|(_, output)| **output).collect();
                if outputs.len() < 2
                    || outputs.len() * 2 != d.gates().len()
                    || outputs.iter().any(|(gate, _)| {
                        gate.strip_prefix("tx_").is_none_or(|suffix| {
                            suffix.is_empty()
                                || d.gates().get(&format!("rx_{suffix}")) != Some(&false)
                        })
                    })
                {
                    return Err(d.fail("Switch requires at least two paired tx_/rx_ ports"));
                }
            }
            Some("dir.ethernet.Link") if d.kind() == "channel" => d.require_parameters(&[
                ("bitrate", "double", Some("bps")),
                ("delay", "double", Some("s")),
            ])?,
            _ => return Err(d.fail("unknown or wrong-kind Ethernet implementation")),
        }
        Ok(())
    }
    fn validate_value(&self, d: &Declaration, name: &str, value: &TypedValue) -> Result<()> {
        if matches!((name,value),("queueCapacity",TypedValue::Integer(n)) if !(0..=4_294_967_295).contains(n))
            || matches!((d.implementation(),name,value),(Some("dir.ethernet.Link"),"bitrate",TypedValue::Quantity(n)) if ![10_000_000,100_000_000,1_000_000_000,10_000_000_000].contains(n))
        {
            return Err(d.fail(format!("unsupported range for {name}")));
        }
        Ok(())
    }
    fn payload(&self, d: &Declaration, _: &str) -> Option<&'static str> {
        matches!(
            d.implementation(),
            Some("dir.ethernet.Endpoint" | "dir.ethernet.Switch")
        )
        .then_some("ethernet.l2.frame.v1")
    }
}
pub(super) fn validate_parameter_literal(d: &Declaration, name: &str, value: &str) -> Result<()> {
    let p = d
        .parameters()
        .get(name)
        .ok_or_else(|| error("unknown parameter"))?;
    EthernetRules.validate_value(d, name, &ned::typed_value(p, value)?)
}
fn number(values: &Values, name: &str) -> u64 {
    match &values[name] {
        TypedValue::Integer(n) => *n as u64,
        TypedValue::Quantity(n) => *n,
        _ => unreachable!(),
    }
}
fn paired(port: &str) -> String {
    let (owner, gate) = port.rsplit_once('.').unwrap();
    format!(
        "{owner}.{}",
        if gate == "tx" {
            "rx".into()
        } else if gate == "rx" {
            "tx".into()
        } else if let Some(s) = gate.strip_prefix("tx_") {
            format!("rx_{s}")
        } else {
            format!("tx_{}", gate.strip_prefix("rx_").unwrap())
        }
    )
}
pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(PreparedEthernet, Vec<String>, usize)> {
    let rules = EthernetRules;
    let r = ned::resolve(types, network, overrides, &rules)?;
    let c = r.resolve_channels(channels, &rules)?;
    let mut devices = Vec::new();
    for (id, d) in r.instances() {
        let kind = match d.implementation() {
            Some("dir.ethernet.Endpoint") => "endpoint",
            Some("dir.ethernet.Switch") => "switch",
            _ => continue,
        };
        let v = r.values(id);
        devices.push(EthernetDevice {
            id: id.into(),
            kind: kind.into(),
            mac: None,
            queue_capacity: number(v, "queueCapacity"),
            tx_processing_delay_ps: if kind == "endpoint" {
                number(v, "txProcessingDelay")
            } else {
                0
            },
            rx_processing_delay_ps: if kind == "endpoint" {
                number(v, "rxProcessingDelay")
            } else {
                0
            },
            forward_delay_ps: 0,
            fdb: BTreeMap::new(),
        });
    }
    if devices.iter().filter(|d| d.kind == "endpoint").count() < 2 {
        return Err(error("Ethernet requires at least two Endpoints"));
    }
    let ids: BTreeMap<_, _> = devices
        .iter()
        .enumerate()
        .map(|(i, d)| (d.id.as_str(), i))
        .collect();
    let mut directions = Vec::new();
    for (source, d) in devices.iter().enumerate() {
        for (gate, _) in r
            .declaration(&d.id)
            .gates()
            .iter()
            .filter(|(_, output)| **output)
        {
            let from_port = format!("{}.{}", d.id, gate);
            let path = r.trace(&from_port)?;
            let (owner, _) = path.end.rsplit_once('.').unwrap();
            let destination = *ids
                .get(owner)
                .ok_or_else(|| error("invalid Ethernet peer"))?;
            if source == destination {
                return Err(error("Ethernet self connection"));
            }
            let (channel_id, bitrate_bps, delay_ps) = path.ethernet_link(&c)?;
            directions.push(EthernetDirection {
                channel_id,
                from_port,
                to_port: path.end.into(),
                source,
                destination,
                bitrate_bps,
                delay_ps,
            });
        }
    }
    directions.sort_by(|a, b| a.from_port.cmp(&b.from_port));
    let mut edges = BTreeSet::new();
    for d in &directions {
        let reverse = directions
            .iter()
            .find(|x| x.from_port == paired(&d.to_port));
        if reverse.is_none_or(|x| {
            x.to_port != paired(&d.from_port)
                || x.bitrate_bps != d.bitrate_bps
                || x.delay_ps != d.delay_ps
        }) {
            return Err(error("Ethernet reverse port/link mismatch"));
        }
        if d.source < d.destination && !edges.insert((d.source, d.destination)) {
            return Err(error("Ethernet parallel connection"));
        }
    }
    let mut seen = BTreeSet::from([0]);
    loop {
        let n = seen.len();
        for &(a, b) in &edges {
            if seen.contains(&a) || seen.contains(&b) {
                seen.insert(a);
                seen.insert(b);
            }
        }
        if seen.len() == n {
            break;
        }
    }
    if seen.len() != devices.len() || edges.len() + 1 != devices.len() {
        return Err(error("Ethernet topology must be a connected tree"));
    }
    Ok((
        PreparedEthernet {
            devices,
            directions,
            generators: Vec::new(),
            outputs: Vec::new(),
        },
        r.module_paths(),
        c.len(),
    ))
}
pub(super) fn mac(s: &str, broadcast: bool) -> Result<String> {
    let parts: Vec<_> = s.split(':').collect();
    if parts.len() != 6
        || parts
            .iter()
            .any(|p| p.len() != 2 || !p.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(error("invalid MAC address"));
    }
    let bytes: Vec<_> = parts
        .iter()
        .map(|p| u8::from_str_radix(p, 16).unwrap())
        .collect();
    if bytes.iter().all(|b| *b == 0)
        || (bytes[0] & 1 != 0 && !(broadcast && bytes.iter().all(|b| *b == 255)))
    {
        return Err(error("invalid unicast or broadcast MAC"));
    }
    Ok(s.to_ascii_lowercase())
}
fn json(content: &str) -> Result<Value> {
    let StrictJson(value) = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|e| error(e.to_string()))?;
    Ok(value)
}
fn array<'a>(o: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a Vec<Value>> {
    o.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("missing array {key}")))
}
pub(super) fn configure(content: &str, p: &mut PreparedEthernet, profile: &str) -> Result<()> {
    let qos = profile == "ethernet.l2.qos.v1";
    let value = json(content)?;
    let root = object(
        &value,
        if qos {
            &["schema_version", "endpoints", "switches", "outputs"]
        } else {
            &["schema_version", "endpoints", "switches"]
        },
        "model-config",
    )?;
    if root.get("schema_version").and_then(Value::as_u64) != Some(if qos { 2 } else { 1 }) {
        return Err(error(
            "model-config schema_version does not match Ethernet profile",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut macs = BTreeSet::new();
    for (key, kind) in [("endpoints", "endpoint"), ("switches", "switch")] {
        for row in array(root, key)? {
            let o = object(
                row,
                if kind == "endpoint" {
                    &["instance", "mac"]
                } else {
                    &["instance", "forward_delay_ps", "fdb"]
                },
                key,
            )?;
            let id = required_string(o, "instance")?;
            let index = p
                .devices
                .iter()
                .position(|d| d.id == id && d.kind == kind)
                .ok_or_else(|| error(format!("unknown {kind}: {id}")))?;
            if !seen.insert(index) {
                return Err(error("duplicate device configuration"));
            }
            if kind == "endpoint" {
                let m = mac(required_string(o, "mac")?, false)?;
                if !macs.insert(m.clone()) {
                    return Err(error("duplicate endpoint MAC"));
                }
                p.devices[index].mac = Some(m);
            } else {
                p.devices[index].forward_delay_ps =
                    unsigned(required_string(o, "forward_delay_ps")?, false)?;
                for row in array(o, "fdb")? {
                    let e = object(row, &["dst_mac", "egress"], "fdb")?;
                    let m = mac(required_string(e, "dst_mac")?, false)?;
                    let output = required_string(e, "egress")?;
                    if !p
                        .directions
                        .iter()
                        .any(|d| d.source == index && d.from_port == output)
                    {
                        return Err(error(format!("invalid FDB egress connection: {output}")));
                    }
                    if p.devices[index].fdb.insert(m, output.into()).is_some() {
                        return Err(error("duplicate FDB MAC"));
                    }
                }
            }
        }
    }
    if qos {
        let mut outputs = BTreeSet::new();
        for row in array(root, "outputs")? {
            let o = object(row, &["port", "scheduler", "queues"], "output")?;
            let port = required_string(o, "port")?;
            if !p.directions.iter().any(|d| d.from_port == port)
                || !outputs.insert(port.to_string())
            {
                return Err(error("unknown or duplicate QoS output port"));
            }
            let scheduler = required_string(o, "scheduler")?;
            if !matches!(scheduler, "fifo" | "strict_priority") {
                return Err(error("unsupported QoS scheduler"));
            }
            let mut priorities = BTreeSet::new();
            let mut queues = Vec::new();
            for row in array(o, "queues")? {
                let q = object(
                    row,
                    &["priority", "capacity_frames", "capacity_bytes"],
                    "queue",
                )?;
                let priority = priority(q)?;
                if !priorities.insert(priority) {
                    return Err(error("duplicate QoS queue priority"));
                }
                let capacity_frames = unsigned(required_string(q, "capacity_frames")?, false)?;
                if capacity_frames > u32::MAX as u64 {
                    return Err(error("QoS frame capacity exceeds u32"));
                }
                let capacity_bytes = optional_decimal(q, "capacity_bytes")?;
                queues.push(EthernetQueueConfig {
                    priority,
                    capacity_frames,
                    capacity_bytes,
                });
            }
            if queues.len() != 8 {
                return Err(error("QoS output requires all eight priorities"));
            }
            queues.sort_by_key(|q| q.priority);
            p.outputs.push(EthernetOutputConfig {
                port: port.into(),
                scheduler: scheduler.into(),
                queues,
            });
        }
        if outputs.len() != p.directions.len() {
            return Err(error("QoS config must enumerate all output ports"));
        }
        p.outputs.sort_by(|a, b| a.port.cmp(&b.port));
    }
    if seen.len() != p.devices.len() {
        return Err(error("model-config must enumerate all Ethernet devices"));
    }
    Ok(())
}
fn optional_decimal(o: &serde_json::Map<String, Value>, key: &str) -> Result<Option<u64>> {
    let value = o
        .get(key)
        .ok_or_else(|| error(format!("missing field: {key}")))?;
    if value.is_null() {
        Ok(None)
    } else {
        value
            .as_str()
            .ok_or_else(|| error(format!("{key} must be decimal string or null")))
            .and_then(|s| unsigned(s, false))
            .map(Some)
    }
}
fn priority(o: &serde_json::Map<String, Value>) -> Result<u8> {
    o.get("priority")
        .and_then(Value::as_u64)
        .filter(|n| *n <= 7)
        .map(|n| n as u8)
        .ok_or_else(|| error("priority must be integer 0 through 7"))
}
fn decimal(o: &serde_json::Map<String, Value>, key: &str) -> Result<u64> {
    unsigned(required_string(o, key)?, false)
}
pub(super) fn workload(content: &str, p: &mut PreparedEthernet, profile: &str) -> Result<()> {
    let qos = profile == "ethernet.l2.qos.v1";
    let value = json(content)?;
    let root = object(&value, &["schema_version", "generators"], "workload")?;
    if root.get("schema_version").and_then(Value::as_u64) != Some(2) {
        return Err(error("Ethernet workload schema_version must be 2"));
    }
    let mut ids = BTreeSet::new();
    let mut flows = BTreeMap::new();
    for row in array(root, "generators")? {
        let kind = row
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| error("missing generator kind"))?;
        let mut fields = vec!["id", "node", "kind", "frame"];
        if qos {
            fields.extend(["flow_id", "priority", "deadline_ps"]);
        }
        match kind {
            "ethernet.explicit.v1" => fields.push("times_ps"),
            "ethernet.periodic.v1" if qos => {
                fields.extend(["start_ps", "phase_ps", "period_ps", "end_ps", "count"])
            }
            "ethernet.burst.v1" if qos => fields.extend([
                "start_ps",
                "period_ps",
                "burst_count",
                "frames_per_burst",
                "spacing_ps",
                "end_ps",
            ]),
            _ => return Err(error("unsupported Ethernet generator kind")),
        }
        let o = object(row, &fields, "generator")?;
        let id = required_string(o, "id")?;
        if !identifier(id) || !ids.insert(id.to_string()) {
            return Err(error("invalid or duplicate generator id"));
        }
        let node = required_string(o, "node")?;
        let source = p
            .devices
            .iter()
            .position(|d| d.id == node && d.kind == "endpoint")
            .ok_or_else(|| error("unknown Endpoint node"))?;
        let f = object(
            o.get("frame").ok_or_else(|| error("missing frame"))?,
            &["dst_mac", "ether_type", "data"],
            "frame",
        )?;
        let ether_type = f
            .get("ether_type")
            .and_then(Value::as_u64)
            .filter(|n| (1536..=65535).contains(n) && *n != 0x8100 && *n != 0x88a8)
            .ok_or_else(|| error("invalid EtherType range"))?;
        let frame = crate::runtime::ethernet::serialize_frame(
            p.devices[source].mac.as_ref().unwrap(),
            required_string(f, "dst_mac")?,
            ether_type as u16,
            required_string(f, "data")?,
        )?;
        let (flow_id, priority, deadline_ps) = if qos {
            let flow_id = required_string(o, "flow_id")?;
            if !identifier(flow_id) {
                return Err(error("flow_id must be ASCII identifier"));
            }
            let priority = priority(o)?;
            let deadline_ps = optional_decimal(o, "deadline_ps")?;
            let contract = (priority, deadline_ps, frame.dst_mac.clone());
            if flows
                .insert(flow_id.to_string(), contract.clone())
                .is_some_and(|prior| prior != contract)
            {
                return Err(error("inconsistent shared Ethernet flow contract"));
            }
            (Some(flow_id.to_string()), priority, deadline_ps)
        } else {
            (None, 0, None)
        };
        let mut times_ps = Vec::new();
        let schedule = match kind {
            "ethernet.explicit.v1" => {
                times_ps = array(o, "times_ps")?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .ok_or_else(|| error("time must be decimal string"))
                            .and_then(|s| unsigned(s, false))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if times_ps.windows(2).any(|w| w[0] > w[1]) {
                    return Err(error("unsorted Ethernet times"));
                }
                None
            }
            "ethernet.periodic.v1" => {
                let start_ps = decimal(o, "start_ps")?;
                let phase_ps = decimal(o, "phase_ps")?;
                let period_ps = decimal(o, "period_ps")?;
                let end_ps = optional_decimal(o, "end_ps")?;
                let count = optional_decimal(o, "count")?;
                if period_ps == 0 || phase_ps >= period_ps {
                    return Err(error("invalid periodic period or phase"));
                }
                Some(EthernetSchedule::Periodic {
                    start_ps,
                    phase_ps,
                    period_ps,
                    end_ps,
                    count,
                })
            }
            "ethernet.burst.v1" => {
                let start_ps = decimal(o, "start_ps")?;
                let period_ps = decimal(o, "period_ps")?;
                let frames_per_burst = decimal(o, "frames_per_burst")?;
                let spacing_ps = decimal(o, "spacing_ps")?;
                let burst_count = optional_decimal(o, "burst_count")?;
                let end_ps = optional_decimal(o, "end_ps")?;
                if period_ps == 0
                    || frames_per_burst == 0
                    || (frames_per_burst.saturating_sub(1) as u128 * spacing_ps as u128)
                        >= period_ps as u128
                {
                    return Err(error("invalid burst count, period or spacing"));
                }
                Some(EthernetSchedule::Burst {
                    start_ps,
                    period_ps,
                    burst_count,
                    frames_per_burst,
                    spacing_ps,
                    end_ps,
                })
            }
            _ => unreachable!(),
        };
        p.generators.push(EthernetGenerator {
            id: id.into(),
            source,
            times_ps,
            frame,
            schedule,
            flow_id,
            priority,
            deadline_ps,
        });
    }
    p.generators.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}

#[cfg(test)]
mod tests;
