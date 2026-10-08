//! CAN FD uses strict external phase counts, never the Classical CAN codec.
use super::ned::{self, Declaration, ModelRules, TypedValue, Values};
use super::{JsonDocument, Result, error, identifier, object, required_string, unsigned};
use crate::types::canfd::*;
use crate::types::{Controller, Diagnostic};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn violation(rule: &str, target: &str, message: impl Into<String>) -> Diagnostic {
    let mut d = error(message);
    d.details = Some(json!({"rule":rule,"target":target}));
    match rule {
        "frame_format" => {
            super::json_diagnostics::current_field("format", d.with_reason("invalid_type"))
        }
        "frame_data" | "frame_dlc" => {
            super::json_diagnostics::current_field("data", d.with_reason("invalid_range"))
        }
        "frame_brs" => super::json_diagnostics::current_field("brs", d.with_reason("invalid_type")),
        "wire_evidence" => {
            super::json_diagnostics::current_field("evidence", d.with_reason("invalid_range"))
        }
        "wire_binding" => {
            super::json_diagnostics::current_field("binding_sha256", d.with_reason("invalid_type"))
        }
        "workload_schema" => {
            super::json_diagnostics::current_field("schema_version", d.with_reason("invalid_type"))
        }
        "generators" => {
            super::json_diagnostics::current_field("generators", d.with_reason("invalid_type"))
        }
        _ => d,
    }
}
fn context(mut d: Diagnostic, rule: &str, target: &str) -> Diagnostic {
    if d.details.is_none() {
        d.details = Some(json!({"rule":rule,"target":target}));
    }
    d
}
struct Rules;
impl ModelRules for Rules {
    fn validate_schema(&self, d: &Declaration) -> Result<()> {
        match d.implementation() {
            Some("dir.canfd.ControllerV1") if d.simple() => {
                d.require_parameters(&[
                    ("queueCapacity", "int", None),
                    ("txProcessingDelay", "double", Some("s")),
                    ("rxProcessingDelay", "double", Some("s")),
                    ("rxFilter", "string", None),
                ])?;
                if *d.gates() != BTreeMap::from([("tx".into(), true), ("rx".into(), false)]) {
                    return Err(violation(
                        "controller_gates",
                        d.name(),
                        "CAN FD Controller requires output tx/input rx",
                    ));
                }
            }
            Some("dir.canfd.BusV1") if d.simple() => {
                d.require_parameters(&[
                    ("nominalBitrate", "double", Some("bps")),
                    ("dataBitrate", "double", Some("bps")),
                    ("profile", "string", None),
                ])?;
                let outputs = d.gates().values().filter(|&&v| v).count();
                if outputs < 2 || outputs * 2 != d.gates().len() {
                    return Err(violation(
                        "bus_gates",
                        d.name(),
                        "CAN FD Bus requires at least two input/output pairs",
                    ));
                }
            }
            _ => {
                return Err(violation(
                    "implementation",
                    d.name(),
                    "unknown or wrong-kind CAN FD implementation",
                ));
            }
        }
        Ok(())
    }
    fn validate_value(&self, d: &Declaration, name: &str, value: &TypedValue) -> Result<()> {
        let invalid = match (name, value) {
            ("queueCapacity", TypedValue::Integer(n)) => !(0..=u32::MAX as i64).contains(n),
            ("nominalBitrate", TypedValue::Quantity(n)) => !(1..=1_000_000).contains(n),
            ("dataBitrate", TypedValue::Quantity(n)) => !(1..=8_000_000).contains(n),
            ("profile", TypedValue::String(s)) => s != "can.fd.precomputed.v1",
            ("rxFilter", TypedValue::String(s)) => {
                super::can::validate_filter(s).map_err(|e| context(e, "rx_filter", d.name()))?;
                false
            }
            _ => false,
        };
        if invalid {
            Err(violation(
                "parameter_range",
                d.name(),
                format!("invalid CAN FD parameter {name}"),
            ))
        } else {
            Ok(())
        }
    }
    fn validate_instance(&self, id: &str, d: &Declaration, v: &Values) -> Result<()> {
        if d.implementation() == Some("dir.canfd.BusV1")
            && number(v, "dataBitrate") < number(v, "nominalBitrate")
        {
            return Err(violation(
                "data_rate",
                id,
                "dataBitrate must be at least nominalBitrate",
            ));
        }
        Ok(())
    }
    fn payload(&self, d: &Declaration, gate: &str) -> Option<&'static str> {
        match d.implementation() {
            Some("dir.canfd.ControllerV1") => Some(if gate == "tx" {
                "dir.canfd.TxRequestV1"
            } else {
                "dir.canfd.NotificationV1"
            }),
            Some("dir.canfd.BusV1") => Some(if d.gates()[gate] {
                "dir.canfd.NotificationV1"
            } else {
                "dir.canfd.TxRequestV1"
            }),
            _ => None,
        }
    }
}
pub(super) fn validate_parameter_literal(d: &Declaration, name: &str, value: &str) -> Result<()> {
    let p = d
        .parameters()
        .get(name)
        .ok_or_else(|| violation("parameter", d.name(), "unknown parameter"))?;
    let value = ned::typed_value(p, value).map_err(|e| context(e, "parameter_type", d.name()))?;
    Rules
        .validate_value(d, name, &value)
        .map_err(|e| context(e, "parameter", d.name()))
}
fn number(v: &Values, name: &str) -> u64 {
    match v[name] {
        TypedValue::Integer(n) => n as u64,
        TypedValue::Quantity(n) => n,
        _ => unreachable!(),
    }
}
pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(PreparedCanFd, Vec<String>, usize)> {
    let r = ned::resolve(types, network, overrides, &Rules)
        .map_err(|e| context(e, "ned_topology", network))?;
    let c = r
        .resolve_channels(channels, &Rules)
        .map_err(|e| context(e, "channel", network))?;
    let buses: Vec<_> = r
        .instances()
        .filter(|(_, d)| d.implementation() == Some("dir.canfd.BusV1"))
        .map(|(id, _)| id.to_string())
        .collect();
    let controllers: Vec<_> = r
        .instances()
        .filter(|(_, d)| d.implementation() == Some("dir.canfd.ControllerV1"))
        .map(|(id, _)| id.to_string())
        .collect();
    if buses.len() != 1 || controllers.len() < 2 {
        return Err(violation(
            "topology_count",
            network,
            "CAN FD requires one Bus and at least two Controllers",
        ));
    }
    let bus_id = buses[0].clone();
    let mut rx = BTreeMap::new();
    for (gate, output) in r.declaration(&bus_id).gates() {
        if *output {
            let path = r.trace(&format!("{bus_id}.{gate}"))?;
            if !controllers.iter().any(|id| path.end == format!("{id}.rx"))
                || rx.insert(path.end, path).is_some()
            {
                return Err(violation(
                    "bus_peer",
                    &bus_id,
                    "Bus outputs must connect to unique Controller rx paths",
                ));
            }
        }
    }
    let mut inputs = BTreeSet::new();
    let mut nodes = Vec::new();
    for id in controllers {
        let path = r.trace(&format!("{id}.tx"))?;
        let (owner, gate) = path.end.rsplit_once('.').unwrap();
        if owner != bus_id
            || r.declaration(&bus_id).gates().get(gate) != Some(&false)
            || !inputs.insert(path.end)
        {
            return Err(violation(
                "controller_peer",
                &id,
                "Controller tx requires a unique input on its Bus",
            ));
        }
        let reverse = rx.remove(format!("{id}.rx").as_str()).ok_or_else(|| {
            violation(
                "controller_peer",
                &id,
                "Controller rx must connect to the same Bus",
            )
        })?;
        let v = r.values(&id);
        let TypedValue::String(filter) = &v["rxFilter"] else {
            unreachable!()
        };
        nodes.push(Controller {
            id: id.clone(),
            queue_capacity: number(v, "queueCapacity"),
            tx_processing_ps: number(v, "txProcessingDelay"),
            rx_processing_ps: number(v, "rxProcessingDelay"),
            rx_filter: filter.clone(),
            tx_channel_ps: path
                .delay(&c)
                .map_err(|e| context(e, "channel_delay", &id))?,
            rx_channel_ps: reverse
                .delay(&c)
                .map_err(|e| context(e, "channel_delay", &id))?,
        });
    }
    if nodes.len() * 2 != r.declaration(&bus_id).gates().len() || !rx.is_empty() {
        return Err(violation(
            "bus_peer",
            &bus_id,
            "every Bus pair must connect to one Controller",
        ));
    }
    Ok((
        PreparedCanFd {
            nominal_rate: number(r.values(&bus_id), "nominalBitrate"),
            data_rate: number(r.values(&bus_id), "dataBitrate"),
            bus_id,
            controllers: nodes,
            generators: Vec::new(),
        },
        r.module_paths(),
        c.len(),
    ))
}
fn decode(content: &str) -> Result<JsonDocument<'_>> {
    JsonDocument::parse(content)
}
pub(super) fn configure(content: &str, _p: &mut PreparedCanFd, profile: &str) -> Result<()> {
    let result = (|| {
        let document = decode(content)?;
        let _scope = document.enter("model_config_invalid");
        let v = &document.value;
        let o = object(v, &["schema_version", "profile"], "CAN FD model-config")?;
        if o.get("schema_version").and_then(Value::as_u64) != Some(1)
            || required_string(o, "profile")? != profile
        {
            let field = if o.get("schema_version").and_then(Value::as_u64) != Some(1) {
                "schema_version"
            } else {
                "profile"
            };
            return Err(super::json_diagnostics::current_field(
                field,
                error("CAN FD model-config profile/schema mismatch").with_reason("invalid_type"),
            ));
        }
        Ok(())
    })();
    result.map_err(|e| context(e, "model_config", "model-config"))
}
fn integer(o: &serde_json::Map<String, Value>, key: &str, maximum: u64) -> Result<u64> {
    super::json_diagnostics::field_check(o, key, || {
        o.get(key)
            .and_then(Value::as_u64)
            .filter(|n| *n <= maximum)
            .ok_or_else(|| error(format!("{key} must be integer 0..{maximum}")))
    })
}
fn duration(n: u64, d: u64, rn: u64, rd: u64) -> Result<u64> {
    let checked = || {
        let numerator = (n as u128)
            .checked_mul(rd as u128)?
            .checked_add((d as u128).checked_mul(rn as u128)?)?
            .checked_mul(1_000_000_000_000)?;
        let denominator = (rn as u128).checked_mul(rd as u128)?;
        u64::try_from(numerator / denominator + u128::from(numerator % denominator != 0)).ok()
    };
    checked().ok_or_else(|| error("CAN FD duration overflows u64"))
}
fn frame(v: &Value, p: &PreparedCanFd, target: &str) -> Result<CanFdFrame> {
    let _object_scope = super::json_diagnostics::scoped_value(v);
    let o = object(v, &["format", "id", "data", "brs", "wire"], "CAN FD frame")
        .map_err(|e| context(e, "frame_schema", target))?;
    let format = required_string(o, "format")?;
    let maximum = match format {
        "standard" => 2047,
        "extended" => 536_870_911,
        _ => {
            return Err(violation(
                "frame_format",
                target,
                "invalid CAN FD frame format",
            ));
        }
    };
    let id = integer(o, "id", maximum).map_err(|e| context(e, "frame_id", target))? as u32;
    let data = required_string(o, "data")?.to_ascii_lowercase();
    if data.len() % 2 != 0 || !data.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(violation(
            "frame_data",
            target,
            "CAN FD data must be even hexadecimal",
        ));
    }
    const LENGTHS: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];
    let dlc = LENGTHS
        .iter()
        .position(|n| *n == data.len() / 2)
        .ok_or_else(|| {
            violation(
                "frame_dlc",
                target,
                "unsupported CAN FD payload byte length",
            )
        })? as u8;
    let brs = o
        .get("brs")
        .and_then(Value::as_bool)
        .ok_or_else(|| violation("frame_brs", target, "brs must be bool"))?;
    let w = object(
        o.get("wire").ok_or_else(|| error("missing wire"))?,
        &["nominal_bits", "data_bits", "evidence", "binding_sha256"],
        "CAN FD wire",
    )
    .map_err(|e| context(e, "wire_schema", target))?;
    let n =
        integer(w, "nominal_bits", 1_000_000).map_err(|e| context(e, "nominal_bits", target))?;
    let d = integer(w, "data_bits", 1_000_000).map_err(|e| context(e, "data_bits", target))?;
    let payload = data.len() as u64 * 4;
    if n == 0
        || if brs {
            d == 0 || d < payload
        } else {
            d != 0 || n < payload
        }
    {
        return Err(violation(
            "wire_bits",
            target,
            "CAN FD wire bit counts do not match BRS/payload bounds",
        ));
    }
    let evidence = required_string(w, "evidence")?;
    if !(1..=512).contains(&evidence.chars().count()) {
        return Err(violation(
            "wire_evidence",
            target,
            "evidence must contain 1..512 characters",
        ));
    }
    let binding = required_string(w, "binding_sha256")?;
    let text = format!(
        "{format}|{id}|{data}|{}|{n}|{d}|{}|{}",
        u8::from(brs),
        p.nominal_rate,
        p.data_rate
    );
    let actual = format!("{:x}", Sha256::digest(text.as_bytes()));
    if binding.len() != 64
        || !binding
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || binding != actual
    {
        return Err(violation(
            "wire_binding",
            target,
            "CAN FD binding_sha256 does not match normalized frame and rates",
        ));
    }
    let mut arbitration = Vec::new();
    let bits = |out: &mut Vec<u8>, value: u32, width: u32| {
        out.extend((0..width).rev().map(|i| ((value >> i) & 1) as u8))
    };
    if format == "standard" {
        bits(&mut arbitration, id, 11);
        arbitration.extend([0, 0]);
    } else {
        bits(&mut arbitration, id >> 18, 11);
        arbitration.extend([1, 1]);
        bits(&mut arbitration, id & 0x3ffff, 18);
        arbitration.push(0);
    }
    Ok(CanFdFrame {
        format: format.into(),
        id,
        data,
        dlc,
        brs,
        nominal_bits: n,
        data_bits: d,
        evidence: evidence.into(),
        binding_sha256: binding.into(),
        nominal_rate: p.nominal_rate,
        data_rate: p.data_rate,
        duration_ps: duration(n, d, p.nominal_rate, p.data_rate)?,
        occupancy_ps: duration(n + 3, d, p.nominal_rate, p.data_rate)?,
        arbitration,
    })
}
pub(super) fn workload(content: &str, p: &mut PreparedCanFd, _profile: &str) -> Result<()> {
    let document = decode(content).map_err(|e| context(e, "json", "workload"))?;
    let _scope = document.enter("workload_invalid");
    let v = &document.value;
    let o = object(v, &["schema_version", "generators"], "CAN FD workload")
        .map_err(|e| context(e, "workload_schema", "workload"))?;
    if o.get("schema_version").and_then(Value::as_u64) != Some(2) {
        return Err(violation(
            "workload_schema",
            "workload",
            "CAN FD workload requires schema_version 2",
        ));
    }
    let rows = o
        .get("generators")
        .and_then(Value::as_array)
        .ok_or_else(|| violation("generators", "workload", "generators must be array"))?;
    let mut ids = BTreeSet::new();
    let mut owners = BTreeMap::new();
    let mut generators = Vec::new();
    for row in rows {
        let _object_scope = super::json_diagnostics::scoped_value(row);
        let target = row.get("id").and_then(Value::as_str).unwrap_or("generator");
        let result = (|| {
            let g = object(
                row,
                &["id", "kind", "node", "times_ps", "frame"],
                "CAN FD generator",
            )?;
            let id = required_string(g, "id")?;
            if !identifier(id) || !ids.insert(id.to_string()) {
                return Err(error("invalid or duplicate CAN FD generator id"));
            }
            if required_string(g, "kind")? != "can.fd.explicit.v1" {
                return Err(error("unsupported CAN FD generator kind"));
            }
            let node = required_string(g, "node")?;
            let source = p
                .controllers
                .iter()
                .position(|c| c.id == node)
                .ok_or_else(|| error("unknown CAN FD Controller node"))?;
            let f = frame(g.get("frame").ok_or_else(|| error("missing frame"))?, p, id)
                .map_err(|e| context(e, "frame_binding", id))?;
            if owners
                .insert((f.format.clone(), f.id), source)
                .is_some_and(|old| old != source)
            {
                return Err(violation(
                    "frame_owner",
                    id,
                    "CAN FD format/id has multiple sender Controllers",
                ));
            }
            let times = g
                .get("times_ps")
                .and_then(Value::as_array)
                .ok_or_else(|| error("times_ps must be array"))?
                .iter()
                .map(|v| {
                    super::json_diagnostics::value_check(v, || {
                        v.as_str()
                            .ok_or_else(|| error("times_ps must contain decimal strings"))
                            .and_then(|s| unsigned(s, false))
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if times.windows(2).any(|w| w[0] > w[1]) {
                return Err(error("CAN FD times_ps must be nondecreasing"));
            }
            Ok(CanFdGenerator {
                id: id.into(),
                source,
                times_ps: times,
                frame: f,
            })
        })();
        generators.push(result.map_err(|e| context(e, "generator", target))?);
    }
    generators.sort_by(|a, b| a.id.cmp(&b.id));
    p.generators = generators;
    Ok(())
}
