//! Strict profile configuration, complete NED placement and workload validation.
use super::ned::{self, Declaration, ModelRules, TypedValue};
use super::{JsonDocument, Result, error, identifier, object, required_string};
use crate::types::soc::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
struct Rules<'a>(&'a str);
impl ModelRules for Rules<'_> {
    fn validate_schema(&self, d: &Declaration) -> Result<()> {
        let p = if self.0.starts_with("soc.") {
            "dir.soc."
        } else if self.0.starts_with("ahb.") {
            "dir.ahb."
        } else {
            "dir.noc."
        };
        let imp = d.implementation().unwrap_or("");
        if !d.simple() || !imp.starts_with(p) {
            return Err(error("unregistered SoC/AHB/NoC implementation"));
        }
        let role = &imp[p.len()..];
        if !matches!(
            role,
            "Source" | "Manager" | "Bus" | "Target" | "Endpoint" | "Router"
        ) {
            return Err(error("unknown SoC module"));
        }
        if (self.0.starts_with("noc.") && !matches!(role, "Endpoint" | "Router"))
            || (self.0.starts_with("soc.") && role == "Manager")
            || (self.0.starts_with("ahb.") && role == "Source")
        {
            return Err(error("module does not belong to profile"));
        }
        d.require_parameters(&[])?;
        let expected = match role {
            "Source" | "Manager" => Some(BTreeMap::from([
                ("request".into(), true),
                ("response".into(), false),
            ])),
            "Target" => Some(BTreeMap::from([
                ("request".into(), false),
                ("response".into(), true),
            ])),
            "Endpoint" => Some(BTreeMap::from([
                ("packet".into(), true),
                ("delivery".into(), false),
            ])),
            _ => None,
        };
        if expected.as_ref().is_some_and(|g| g != d.gates()) {
            return Err(error("invalid profile module gates"));
        }
        for (gate, out) in d.gates() {
            if role == "Bus" {
                if !(gate.starts_with("request_") || gate.starts_with("response_")) {
                    return Err(error("invalid Bus port"));
                }
            } else if role == "Router" {
                let valid = if *out {
                    gate == "local_out"
                        || ["out_east", "out_west", "out_north", "out_south"]
                            .contains(&gate.as_str())
                } else {
                    gate == "local_in"
                        || ["in_east", "in_west", "in_north", "in_south"].contains(&gate.as_str())
                };
                if !valid {
                    return Err(error("invalid Router port"));
                }
            }
        }
        Ok(())
    }
    fn validate_value(
        &self,
        declaration: &Declaration,
        name: &str,
        value: &TypedValue,
    ) -> Result<()> {
        if declaration.implementation() == Some("dir.link.FixedDelay")
            && name == "delay"
            && matches!(value, TypedValue::Quantity(0))
        {
            Ok(())
        } else {
            Err(error(
                "profile modules have no parameters; FixedDelay must be zero",
            ))
        }
    }
    fn payload(&self, d: &Declaration, gate: &str) -> Option<&'static str> {
        let i = d.implementation()?;
        if i.starts_with("dir.noc.") {
            Some("noc.Packet")
        } else if gate.starts_with("request") {
            Some(if i.starts_with("dir.ahb.") {
                "ahb.Request"
            } else {
                "soc.Request"
            })
        } else {
            Some(if i.starts_with("dir.ahb.") {
                "ahb.Response"
            } else {
                "soc.Response"
            })
        }
    }
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
        .ok_or_else(|| error("unknown parameter"))?;
    Rules(profile).validate_value(declaration, name, &ned::typed_value(parameter, value)?)
}
pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
    profile: &str,
) -> Result<(PreparedSoc, Vec<String>, usize)> {
    let r = ned::resolve(types, network, overrides, &Rules(profile))?;
    let c = r.resolve_channels(channels, &Rules(profile))?;
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeMap::new();
    for (id, d) in r.instances() {
        if let Some(i) = d.implementation() {
            nodes.insert(id.into(), i.into());
            for (gate, out) in d.gates() {
                if *out {
                    let path = r.trace(&format!("{id}.{gate}"))?;
                    if path.delay(&c)? != 0 {
                        return Err(error("SoC/AHB/NoC requires zero-delay connections"));
                    }
                    edges.insert(format!("{id}.{gate}"), path.end.into());
                }
            }
        }
    }
    Ok((
        PreparedSoc {
            profile: profile.into(),
            clock_period: 1,
            bus: String::new(),
            arbitration: "round_robin".into(),
            bytes_per_cycle: 1,
            link_cycles: 0,
            input_capacity: 1,
            sources: Vec::new(),
            targets: Vec::new(),
            routers: Vec::new(),
            generators: Vec::new(),
            nodes,
            edges,
        },
        r.module_paths(),
        c.len(),
    ))
}
fn exact<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>> {
    let m = object(v, keys, "profile JSON")?;
    if m.len() != keys.len() {
        return Err(error("missing required profile JSON key"));
    }
    Ok(m)
}
fn num(m: &Map<String, Value>, k: &str, min: u64, max: u64) -> Result<u64> {
    super::json_diagnostics::field_check(m, k, || {
        m.get(k)
            .and_then(Value::as_u64)
            .filter(|n| *n >= min && *n <= max)
            .ok_or_else(|| error(format!("invalid integer {k}")))
    })
}
fn array<'a>(m: &'a Map<String, Value>, k: &str) -> Result<&'a Vec<Value>> {
    super::json_diagnostics::field_check(m, k, || {
        m.get(k)
            .and_then(Value::as_array)
            .ok_or_else(|| error(format!("invalid array {k}")))
    })
}
fn parse(content: &str) -> Result<JsonDocument<'_>> {
    JsonDocument::parse(content)
}
fn node(
    m: &Map<String, Value>,
    model: &PreparedSoc,
    role: &str,
    seen: &mut BTreeSet<String>,
) -> Result<String> {
    let n = required_string(m, "node")?;
    let expected = format!("dir.{}.{}", model.prefix(), role);
    if model.nodes.get(n) != Some(&expected) || !seen.insert(n.into()) {
        return Err(error("unknown, wrong type, or duplicate configured node"));
    }
    Ok(n.into())
}
fn edge(model: &PreparedSoc, a: String, b: String) -> Result<()> {
    if model.edges.get(&a) != Some(&b) {
        return Err(error(format!("invalid profile connection {a} -> {b}")));
    }
    Ok(())
}
pub(super) fn configure(content: &str, model: &mut PreparedSoc, profile: &str) -> Result<()> {
    let document = parse(content)?;
    let _scope = document.enter("model_config_invalid");
    let v = &document.value;
    let noc = profile.starts_with("noc.");
    let ahb = profile.starts_with("ahb.");
    let m = exact(
        v,
        if noc {
            &[
                "schema_version",
                "profile",
                "clock_period",
                "columns",
                "rows",
                "bytes_per_cycle",
                "link_cycles",
                "input_capacity",
                "source_capacity",
                "routers",
                "endpoints",
            ]
        } else if ahb {
            &[
                "schema_version",
                "profile",
                "clock_period",
                "managers",
                "bus",
                "targets",
            ]
        } else {
            &[
                "schema_version",
                "profile",
                "clock_period",
                "arbitration",
                "sources",
                "bus",
                "targets",
                "bytes_per_cycle",
            ]
        },
    )?;
    num(m, "schema_version", 1, 1)?;
    if required_string(m, "profile")? != profile {
        return Err(error("profile mismatch"));
    }
    model.clock_period = super::parse_time(required_string(m, "clock_period")?)?;
    if model.clock_period == 0 {
        return Err(error("clock_period must be positive"));
    }
    let mut seen = BTreeSet::new();
    if noc {
        let columns = num(m, "columns", 1, 32)?;
        let rows = num(m, "rows", 1, 32)?;
        model.bytes_per_cycle = num(m, "bytes_per_cycle", 1, 4096)?;
        model.link_cycles = num(m, "link_cycles", 0, 65535)?;
        model.input_capacity = num(m, "input_capacity", 1, 65535)?;
        let capacity = num(m, "source_capacity", 1, 65535)?;
        let mut coordinates = BTreeSet::new();
        for r in array(m, "routers")? {
            let _object_scope = super::json_diagnostics::scoped_value(r);
            let r = exact(r, &["node", "x", "y"])?;
            let n = node(r, model, "Router", &mut seen)?;
            let x = num(r, "x", 0, columns - 1)?;
            let y = num(r, "y", 0, rows - 1)?;
            if !coordinates.insert((x, y)) {
                return Err(error("duplicate mesh coordinate"));
            }
            model.routers.push(Router { node: n, x, y });
        }
        if coordinates.len() as u64 != columns * rows {
            return Err(error("incomplete mesh"));
        }
        model.routers.sort_by(|a, b| a.node.cmp(&b.node));
        let mut attached = BTreeSet::new();
        for e in array(m, "endpoints")? {
            let _object_scope = super::json_diagnostics::scoped_value(e);
            let e = exact(e, &["node", "router"])?;
            let n = node(e, model, "Endpoint", &mut seen)?;
            let router = model
                .routers
                .iter()
                .position(|r| Some(r.node.as_str()) == e["router"].as_str())
                .ok_or_else(|| error("invalid endpoint router"))?;
            if !attached.insert(router) {
                return Err(error("multiple endpoints per router"));
            }
            model.sources.push(Source {
                node: n,
                capacity,
                priority: 0,
                router: Some(router),
            });
        }
        if attached.len() != model.routers.len() {
            return Err(error("missing router endpoint"));
        }
        model.sources.sort_by(|a, b| a.node.cmp(&b.node));
        for s in &model.sources {
            let r = &model.routers[s.router.unwrap()];
            edge(
                model,
                format!("{}.packet", s.node),
                format!("{}.local_in", r.node),
            )?;
            edge(
                model,
                format!("{}.local_out", r.node),
                format!("{}.delivery", s.node),
            )?;
        }
        for r in &model.routers {
            let mut gates = BTreeSet::from(["local_in".to_string(), "local_out".to_string()]);
            for (d, opp, dx, dy) in [
                ("east", "west", 1, 0),
                ("west", "east", -1, 0),
                ("north", "south", 0, 1),
                ("south", "north", 0, -1),
            ] {
                if let Some(next) = model
                    .routers
                    .iter()
                    .find(|n| n.x as i64 == r.x as i64 + dx && n.y as i64 == r.y as i64 + dy)
                {
                    gates.insert(format!("in_{d}"));
                    gates.insert(format!("out_{d}"));
                    edge(
                        model,
                        format!("{}.out_{d}", r.node),
                        format!("{}.in_{opp}", next.node),
                    )?;
                }
            }
            let actual: BTreeSet<_> = model
                .edges
                .keys()
                .chain(model.edges.values())
                .filter_map(|e| e.strip_prefix(&format!("{}.", r.node)))
                .map(str::to_string)
                .collect();
            if actual != gates {
                return Err(error("router ports do not match mesh neighbors"));
            }
        }
    } else {
        model.bus = required_string(m, "bus")?.into();
        if model.nodes.get(&model.bus) != Some(&format!("dir.{}.Bus", model.prefix()))
            || !seen.insert(model.bus.clone())
        {
            return Err(error("invalid unique Bus"));
        }
        if !ahb {
            model.bytes_per_cycle = num(m, "bytes_per_cycle", 1, 4096)?;
            model.arbitration = required_string(m, "arbitration")?.into();
            if !matches!(model.arbitration.as_str(), "round_robin" | "fixed_priority") {
                return Err(error("invalid arbitration"));
            }
        }
        for s in array(m, if ahb { "managers" } else { "sources" })? {
            let _object_scope = super::json_diagnostics::scoped_value(s);
            let s = exact(
                s,
                if ahb {
                    &["node", "capacity"]
                } else {
                    &["node", "capacity", "priority"]
                },
            )?;
            let n = node(s, model, if ahb { "Manager" } else { "Source" }, &mut seen)?;
            model.sources.push(Source {
                node: n,
                capacity: num(s, "capacity", 1, 65535)?,
                priority: if ahb {
                    0
                } else {
                    num(s, "priority", 0, 65535)?
                },
                router: None,
            });
        }
        for t in array(m, "targets")? {
            let _object_scope = super::json_diagnostics::scoped_value(t);
            let t = exact(
                t,
                if ahb {
                    &["node", "base", "size", "wait_cycles", "error_ranges"]
                } else {
                    &["node", "base", "size", "service_cycles", "error_ranges"]
                },
            )?;
            let n = node(t, model, "Target", &mut seen)?;
            let base = num(t, "base", 0, (1u64 << 32) - 1)?;
            let size = num(t, "size", 1, 1u64 << 32)?;
            if base + size > 1u64 << 32 || (ahb && (base % 4 != 0 || size % 4 != 0)) {
                return Err(error("invalid target address interval"));
            }
            let mut errors = Vec::new();
            for e in array(t, "error_ranges")? {
                let _object_scope = super::json_diagnostics::scoped_value(e);
                let e = exact(e, &["start", "end"])?;
                let start = num(e, "start", base, base + size - 1)?;
                let end = num(e, "end", start + 1, base + size)?;
                errors.push((start, end));
            }
            errors.sort();
            if errors.windows(2).any(|e| e[0].1 > e[1].0) {
                return Err(error("overlapping error ranges"));
            }
            model.targets.push(Target {
                node: n,
                base,
                size,
                cycles: num(
                    t,
                    if ahb { "wait_cycles" } else { "service_cycles" },
                    0,
                    65535,
                )?,
                errors,
            });
        }
        if model.sources.is_empty() || model.targets.is_empty() {
            return Err(error("sources and targets must be nonempty"));
        }
        for (i, a) in model.targets.iter().enumerate() {
            if model.targets[i + 1..]
                .iter()
                .any(|b| a.base < b.base + b.size && b.base < a.base + a.size)
            {
                return Err(error("overlapping target ranges"));
            }
        }
        model.sources.sort_by(|a, b| a.node.cmp(&b.node));
        model.targets.sort_by(|a, b| a.node.cmp(&b.node));
        let mut suffixes = BTreeSet::new();
        for (n, is_source) in model
            .sources
            .iter()
            .map(|s| (&s.node, true))
            .chain(model.targets.iter().map(|t| (&t.node, false)))
        {
            let suffix = n.rsplit('.').next().unwrap();
            if !suffixes.insert(suffix) {
                return Err(error("duplicate source/target suffix"));
            }
            if is_source {
                edge(
                    model,
                    format!("{n}.request"),
                    format!("{}.request_{suffix}", model.bus),
                )?;
                edge(
                    model,
                    format!("{}.response_{suffix}", model.bus),
                    format!("{n}.response"),
                )?;
            } else {
                edge(
                    model,
                    format!("{}.request_{suffix}", model.bus),
                    format!("{n}.request"),
                )?;
                edge(
                    model,
                    format!("{n}.response"),
                    format!("{}.response_{suffix}", model.bus),
                )?;
            }
        }
        if model.edges.len() != 2 * (model.sources.len() + model.targets.len()) {
            return Err(error("extra or missing Bus ports"));
        }
    }
    if seen.len() != model.nodes.len() {
        return Err(error(
            "configuration must reference every registered module once",
        ));
    }
    Ok(())
}
pub(super) fn workload(content: &str, model: &mut PreparedSoc, _: &str) -> Result<()> {
    let document = parse(content)?;
    let _scope = document.enter("workload_invalid");
    let v = &document.value;
    let m = exact(v, &["schema_version", "generators"])?;
    num(m, "schema_version", 2, 2)?;
    let mut ids = BTreeSet::new();
    for g in array(m, "generators")? {
        let _object_scope = super::json_diagnostics::scoped_value(g);
        let g = exact(g, &["id", "kind", "node", "times", "transaction"])?;
        let id = required_string(g, "id")?;
        if !identifier(id) || !ids.insert(id.to_string()) {
            return Err(error("invalid or duplicate generator ID"));
        }
        if required_string(g, "kind")? != format!("{}.explicit.v1", model.prefix()) {
            return Err(error("wrong generator kind"));
        }
        let source = model
            .sources
            .iter()
            .position(|s| Some(s.node.as_str()) == g["node"].as_str())
            .ok_or_else(|| error("unknown workload source"))?;
        let times = array(g, "times")?
            .iter()
            .map(|t| super::parse_time(t.as_str().ok_or_else(|| error("time must be string"))?))
            .collect::<Result<Vec<_>>>()?;
        if times.windows(2).any(|t| t[0] > t[1]) {
            return Err(error("times must be nondecreasing"));
        }
        let noc = model.prefix() == "noc";
        let t = exact(
            &g["transaction"],
            if noc {
                &["destination", "bytes"]
            } else {
                &["operation", "address", "bytes"]
            },
        )?;
        let bytes = num(t, "bytes", 1, 65535)?;
        let (operation, address, destination) = if noc {
            let d = model
                .sources
                .iter()
                .position(|s| Some(s.node.as_str()) == t["destination"].as_str())
                .ok_or_else(|| error("unknown destination Endpoint"))?;
            (None, None, Some(d))
        } else {
            let op = required_string(t, "operation")?;
            if !matches!(op, "read" | "write") {
                return Err(error("invalid operation"));
            }
            let a = num(t, "address", 0, (1u64 << 32) - 1)?;
            if a + bytes > 1u64 << 32 || (model.prefix() == "ahb" && (bytes != 4 || a % 4 != 0)) {
                return Err(error("invalid transaction address or byte width"));
            }
            (Some(op.into()), Some(a), None)
        };
        model.generators.push(Generator {
            id: id.into(),
            source,
            times,
            transaction: Transaction {
                operation,
                address,
                bytes,
                destination,
            },
        });
    }
    model.generators.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}
