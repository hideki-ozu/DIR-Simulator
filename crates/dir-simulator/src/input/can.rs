//! Classical CAN schemas, topology, profiles and workload validation.
use super::ned::{self, Declaration, ModelRules, TypedValue, Values};
use super::{
    Result, StrictJson, error, identifier, json_time, object, parse_time, required_string,
    string_literal,
};
use crate::types::{Bus, Controller, Diagnostic, Frame, Generator, Schedule};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

struct CanRules<'a> {
    profile: &'a str,
}
pub(super) fn validate_parameter_literal(
    declaration: &Declaration,
    name: &str,
    value: &str,
    profile: &str,
) -> Result<()> {
    let parameter = declaration
        .parameters()
        .get(name)
        .ok_or_else(|| error("Unknown parameter"))?;
    CanRules { profile }.validate_value(declaration, name, &ned::typed_value(parameter, value)?)
}
impl ModelRules for CanRules<'_> {
    fn validate_value(
        &self,
        declaration: &Declaration,
        name: &str,
        value: &TypedValue,
    ) -> Result<()> {
        let invalid = match (declaration.implementation(), name, value) {
            (
                Some("dir.can.Controller" | "dir.can.MultibusController"),
                "queueCapacity",
                TypedValue::Integer(n),
            ) => !(0..=4_294_967_295).contains(n),
            (
                Some("dir.can.Controller" | "dir.can.MultibusController"),
                "rxFilter",
                TypedValue::String(s),
            ) => {
                validate_filter(s)?;
                false
            }
            (Some("dir.can.Bus" | "dir.can.MultibusBus"), "bitrate", TypedValue::Quantity(n)) => {
                *n == 0 || *n > 1_000_000
            }
            (Some("dir.can.Bus"), "profile", TypedValue::String(s)) => s != "can.cc.ideal.v1",
            (Some("dir.can.MultibusBus"), "profile", TypedValue::String(s)) => {
                s != "can.cc.multibus.v1"
            }
            _ => false,
        };
        if invalid {
            Err(declaration.fail(format!(
                "out of range or unsupported value for {name}: {value:?}"
            )))
        } else {
            Ok(())
        }
    }
    fn validate_schema(&self, declaration: &Declaration) -> Result<()> {
        let schema: &[(&str, &str, Option<&str>)] = match declaration.implementation() {
            Some("dir.can.Controller" | "dir.can.MultibusController") if declaration.simple() => &[
                ("queueCapacity", "int", None),
                ("txProcessingDelay", "double", Some("s")),
                ("rxProcessingDelay", "double", Some("s")),
                ("rxFilter", "string", None),
            ],
            Some("dir.can.Bus" | "dir.can.MultibusBus") if declaration.simple() => &[
                ("bitrate", "double", Some("bps")),
                ("profile", "string", None),
            ],
            _ => {
                return Err(
                    declaration.fail("missing, unknown, or wrong-kind @class implementation")
                );
            }
        };
        declaration.require_parameters(schema)?;
        match declaration.implementation() {
            Some("dir.can.Controller" | "dir.can.MultibusController") => {
                if *declaration.gates()
                    != BTreeMap::from([("tx".into(), true), ("rx".into(), false)])
                {
                    return Err(declaration.fail("Controller requires output tx and input rx only"));
                }
            }
            Some("dir.can.Bus" | "dir.can.MultibusBus") => {
                let outputs = declaration
                    .gates()
                    .values()
                    .filter(|&&output| output)
                    .count();
                let inputs = declaration.gates().len() - outputs;
                if inputs < 2 || inputs != outputs {
                    return Err(declaration.fail(
                        "Bus needs equal counts of input/output scalar gates (at least two each)",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn payload(&self, owner: &Declaration, gate: &str) -> Option<&'static str> {
        match owner.implementation() {
            Some("dir.can.Controller") => Some(if gate == "tx" { "ideal.tx" } else { "ideal.rx" }),
            Some("dir.can.MultibusController") => Some(if gate == "tx" {
                "multibus.tx"
            } else {
                "multibus.rx"
            }),
            Some("dir.can.Bus") => Some(if owner.gates()[gate] {
                "ideal.rx"
            } else {
                "ideal.tx"
            }),
            Some("dir.can.MultibusBus") => Some(if owner.gates()[gate] {
                "multibus.rx"
            } else {
                "multibus.tx"
            }),
            _ => None,
        }
    }

    fn validate_instance(
        &self,
        instance: &str,
        declaration: &Declaration,
        _: &Values,
    ) -> Result<()> {
        if let Some(
            class @ ("dir.can.Controller"
            | "dir.can.MultibusController"
            | "dir.can.Bus"
            | "dir.can.MultibusBus"),
        ) = declaration.implementation()
        {
            if class.contains("Multibus") != (self.profile == "can.cc.multibus.v1") {
                return Err(error(format!(
                    "profile/class mismatch at {instance}: {class}"
                )));
            }
        }
        Ok(())
    }
    fn incompatible_payload(&self, start: &str, end: &str) -> Diagnostic {
        error(format!("incompatible CAN payload path: {start} --> {end}"))
    }
}

pub(super) fn profile(general: &BTreeMap<String, String>) -> Result<String> {
    let profile = general
        .get("model-profile")
        .map(|s| string_literal(s))
        .transpose()?
        .unwrap_or_else(|| "can.cc.ideal.v1".into());
    if !matches!(
        profile.as_str(),
        "can.cc.ideal.v1"
            | "can.cc.multibus.v1"
            | "ethernet.l2.store-forward.v1"
            | "ethernet.l2.qos.v1"
            | "ethernet.l2.vlan.v1"
            | "ethernet.l2.store-forward.v2"
            | "ethernet.l2.100base-t1.v1"
            | "can.fd.precomputed.v1"
    ) {
        return Err(error(format!("unsupported model-profile: {profile}")));
    }
    if profile == "can.cc.ideal.v1" && general.contains_key("model-config") {
        return Err(error("model-config is unsupported for can.cc.ideal.v1"));
    }
    Ok(profile)
}

fn number(values: &Values, name: &str) -> u64 {
    match &values[name] {
        TypedValue::Integer(n) => *n as u64,
        TypedValue::Quantity(n) => *n,
        _ => unreachable!(),
    }
}
fn text(values: &Values, name: &str) -> String {
    match &values[name] {
        TypedValue::String(s) => s.clone(),
        _ => unreachable!(),
    }
}
pub(super) struct Resolved {
    pub bus_id: String,
    pub bitrate: u64,
    pub controllers: Vec<Controller>,
    pub channel_count: usize,
    pub buses: Vec<Bus>,
    pub controller_buses: Vec<usize>,
    pub module_paths: Vec<String>,
}

pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
    profile: &str,
) -> Result<Resolved> {
    let rules = CanRules { profile };
    let resolved = ned::resolve(types, network, overrides, &rules)?;
    let mut controller_paths = Vec::new();
    let mut buses = Vec::new();
    for (instance, declaration) in resolved.instances() {
        match declaration.implementation() {
            Some("dir.can.Controller" | "dir.can.MultibusController") => {
                controller_paths.push(instance.to_string())
            }
            Some("dir.can.Bus" | "dir.can.MultibusBus") => buses.push(instance.to_string()),
            _ => {}
        }
    }
    if buses.is_empty()
        || (profile == "can.cc.ideal.v1" && buses.len() != 1)
        || controller_paths.len() < 2
    {
        return Err(error(
            "can.cc.ideal.v1 requires exactly one Bus and at least two Controllers",
        ));
    }
    let bus_id = buses[0].clone();
    let mut controller_buses = Vec::new();
    let channel_delays = resolved.resolve_channels(channels, &rules)?;
    let mut controllers = Vec::new();
    let bus_indices: BTreeMap<_, _> = buses
        .iter()
        .enumerate()
        .map(|(index, bus)| (bus.as_str(), index))
        .collect();
    // Resolve each Bus output once, keeping the full boundary/channel path for RX delay.
    // Gate pairing is determined by Controller identity, never by the gate names.
    let mut rx_paths = BTreeMap::new();
    for (index, bus) in buses.iter().enumerate() {
        let bus_type = resolved.declaration(bus);
        for (gate, output) in bus_type.gates() {
            if !output {
                continue;
            }
            let path = resolved.trace(&format!("{bus}.{gate}"))?;
            let sink = path.end;
            let (instance, gate) = sink.rsplit_once('.').unwrap();
            let owner = resolved.declaration(instance);
            if gate != "rx"
                || !matches!(
                    owner.implementation(),
                    Some("dir.can.Controller" | "dir.can.MultibusController")
                )
            {
                return Err(error(format!(
                    "Bus {bus} output path must end at Controller rx"
                )));
            }
            if rx_paths.insert(sink, (index, path)).is_some() {
                return Err(error(format!("duplicate Bus output path to {sink}")));
            }
        }
    }
    let mut used_inputs = BTreeSet::new();
    let mut bus_controller_counts = vec![0usize; buses.len()];
    for id in controller_paths {
        let tx_path = resolved.trace(&format!("{id}.tx"))?;
        let tx_sink = tx_path.end;
        let (bus, tx_gate) = tx_sink.rsplit_once('.').unwrap();
        let bus_index = *bus_indices
            .get(bus)
            .ok_or_else(|| error(format!("Controller {id} tx path must end at Bus input")))?;
        let bus_type = resolved.declaration(bus);
        if bus_type.gates().get(tx_gate) != Some(&false) {
            return Err(error(format!(
                "Controller {id} tx path must end at Bus input"
            )));
        }
        if !used_inputs.insert(tx_sink) {
            return Err(error(format!(
                "Controller {id} must connect to a unique Bus input"
            )));
        }
        let (rx_bus_index, rx_path) = rx_paths
            .remove(format!("{id}.rx").as_str())
            .ok_or_else(|| error(format!("Controller {id} rx path must start at Bus output")))?;
        if rx_bus_index != bus_index {
            return Err(error(format!(
                "Controller {id} tx/rx must use the same Bus"
            )));
        }
        bus_controller_counts[bus_index] += 1;
        controller_buses.push(bus_index);
        let values = resolved.values(&id);
        controllers.push(Controller {
            id,
            queue_capacity: number(values, "queueCapacity"),
            tx_processing_ps: number(values, "txProcessingDelay"),
            rx_processing_ps: number(values, "rxProcessingDelay"),
            rx_filter: text(values, "rxFilter"),
            tx_channel_ps: tx_path.delay(&channel_delays)?,
            rx_channel_ps: rx_path.delay(&channel_delays)?,
        });
    }
    for (index, bus) in buses.iter().enumerate() {
        let bus_type = resolved.declaration(bus);
        let count = bus_controller_counts[index];
        if count < 2 || count * 2 != bus_type.gates().len() {
            return Err(error(format!(
                "Bus {bus} gate pairs must each connect to one Controller (at least two)"
            )));
        }
    }
    let buses = buses
        .into_iter()
        .map(|id| Bus {
            bitrate: number(resolved.values(&id), "bitrate"),
            id,
        })
        .collect();
    Ok(Resolved {
        bitrate: number(resolved.values(&bus_id), "bitrate"),
        bus_id,
        controllers,
        channel_count: channel_delays.len(),
        buses,
        controller_buses,
        module_paths: resolved.module_paths(),
    })
}

pub(super) fn workload(
    content: &str,
    controllers: &[crate::types::Controller],
    buses: &[usize],
    profile: &str,
) -> Result<Vec<Generator>> {
    let StrictJson(value) = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|e| error(e.to_string()))?;
    let root = object(&value, &["schema_version", "generators"], "workload")?;
    let version = root.get("schema_version").and_then(Value::as_u64);
    if version != Some(1) && !(profile == "can.cc.multibus.v1" && version == Some(2)) {
        return Err(error("workload schema_version must be integer 1"));
    }
    let generators = root
        .get("generators")
        .and_then(Value::as_array)
        .ok_or_else(|| error("generators must be an array"))?;
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    let mut owners = BTreeMap::new();
    for value in generators {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| error("missing generator kind"))?;
        let allowed: &[&str] = match kind {
            "can.explicit.v1" => &["id", "kind", "node", "frame", "times"],
            "can.periodic.v1" => &[
                "id", "kind", "node", "frame", "start", "phase", "period", "end", "count",
            ],
            _ => return Err(error(format!("unsupported generator kind: {kind}"))),
        };
        let generator = object(value, allowed, "generator")?;
        let id = required_string(generator, "id")?;
        if !identifier(id) || !ids.insert(id.to_string()) {
            return Err(error(format!("invalid or duplicate generator id: {id}")));
        }
        let node = required_string(generator, "node")?;
        let source = controllers
            .iter()
            .position(|c| c.id == node)
            .ok_or_else(|| error(format!("unknown Controller node: {node}")))?;
        let frame = generator
            .get("frame")
            .ok_or_else(|| error("missing frame"))?;
        object(frame, &["format", "id", "data"], "frame")?;
        let mut frame: Frame =
            serde_json::from_value(frame.clone()).map_err(|e| error(e.to_string()))?;
        let maximum = match frame.format.as_str() {
            "standard" => 2047,
            "extended" => 536_870_911,
            _ => return Err(error(format!("unknown frame format: {}", frame.format))),
        };
        if frame.id > maximum
            || frame.data.len() > 16
            || frame.data.len() % 2 != 0
            || !frame.data.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(error(format!("invalid frame for generator {id}")));
        }
        frame.data.make_ascii_lowercase();
        if owners
            .insert((buses[source], frame.format.clone(), frame.id), source)
            .is_some_and(|previous| previous != source)
        {
            return Err(error(format!(
                "CAN frame ownership conflict: {} id {}",
                frame.format, frame.id
            )));
        }
        let schedule = if kind == "can.explicit.v1" {
            let times = generator
                .get("times")
                .and_then(Value::as_array)
                .ok_or_else(|| error("explicit times must be an array"))?;
            let times = times
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| error("explicit time must be a string"))
                        .and_then(parse_time)
                })
                .collect::<Result<Vec<_>>>()?;
            if times.windows(2).any(|pair| pair[0] > pair[1]) {
                return Err(error(format!("unsorted explicit times for {id}")));
            }
            Schedule::Explicit(times)
        } else {
            let start = json_time(generator, "start")?;
            let phase = if generator.contains_key("phase") {
                json_time(generator, "phase")?
            } else {
                0
            };
            let period = json_time(generator, "period")?;
            let end = generator
                .contains_key("end")
                .then(|| json_time(generator, "end"))
                .transpose()?;
            let count = generator
                .get("count")
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| error("count must be a u64 JSON integer"))
                })
                .transpose()?;
            if period == 0 || phase >= period || end.is_some_and(|end| end < start) {
                return Err(error(format!("invalid periodic bounds for {id}")));
            }
            Schedule::Periodic {
                start,
                phase,
                period,
                end,
                count,
            }
        };
        out.push(Generator {
            id: id.into(),
            source,
            frame,
            schedule,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

pub(super) fn validate_filter(value: &str) -> Result<()> {
    if matches!(value, "*" | "none") {
        return Ok(());
    }
    let mut ids = BTreeSet::new();
    for item in value.split(',') {
        let (kind, hex) = item
            .split_once(":0x")
            .ok_or_else(|| error(format!("invalid rxFilter: {value}")))?;
        let limit = match kind {
            "std" => 2047,
            "ext" => 536_870_911,
            _ => return Err(error(format!("invalid rxFilter: {value}"))),
        };
        if hex.is_empty() || hex.len() > 8 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(error(format!("invalid rxFilter: {value}")));
        }
        let id = u32::from_str_radix(hex, 16)
            .map_err(|_| error(format!("invalid rxFilter: {value}")))?;
        if id > limit || !ids.insert((kind, id)) {
            return Err(error(format!(
                "out of range or duplicate rxFilter: {value}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
