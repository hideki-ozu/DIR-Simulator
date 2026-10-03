//! Static routing validation, including reservations for traffic that never fires.
use super::{Result, StrictJson, error, identifier, object, parse_time, required_string};
use crate::types::{Controller, Gateway, Generator, Route};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

fn integer(obj: &Map<String, Value>, key: &str, max: u64) -> Result<u64> {
    obj.get(key)
        .and_then(Value::as_u64)
        .filter(|&n| n <= max)
        .ok_or_else(|| {
            error(format!(
                "routing field {key} must be an integer in 0..{max}"
            ))
        })
}
fn array<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>> {
    obj.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("routing field {key} must be an array")))
}
fn port(value: &Value, controllers: &[Controller]) -> Result<usize> {
    let path = value
        .as_str()
        .ok_or_else(|| error("routing port must be a string"))?;
    controllers
        .iter()
        .position(|c| c.id == path)
        .ok_or_else(|| error(format!("unknown routing port: {path}")))
}

pub(super) fn parse(
    content: &str,
    controllers: &[Controller],
    buses: &[usize],
    modules: &[String],
) -> Result<Vec<Gateway>> {
    let StrictJson(value) = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|e| error(e.to_string()))?;
    let root = object(&value, &["schema_version", "gateways"], "routing")?;
    if root.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err(error("routing schema_version must be integer 1"));
    }
    let mut gateways = Vec::new();
    let mut nodes = BTreeSet::new();
    let mut assigned = BTreeSet::new();
    for value in array(root, "gateways")? {
        let obj = object(
            value,
            &[
                "node",
                "ports",
                "routes",
                "processing_delay",
                "hop_limit",
                "rx_queue_capacity",
            ],
            "Gateway",
        )?;
        let node = required_string(obj, "node")?;
        if !modules.iter().any(|m| m == node) || !nodes.insert(node.to_string()) {
            return Err(error(format!(
                "unknown or duplicate Gateway module: {node}"
            )));
        }
        let mut ports = Vec::new();
        let mut port_buses = BTreeSet::new();
        for value in array(obj, "ports")? {
            let p = port(value, controllers)?;
            if !controllers[p].id.starts_with(&format!("{node}."))
                || !assigned.insert(p)
                || !port_buses.insert(buses[p])
            {
                return Err(error(format!(
                    "Gateway {node} ports must be distinct descendants on distinct buses, owned by one Gateway"
                )));
            }
            ports.push(p);
        }
        if ports.len() < 2 {
            return Err(error(format!("Gateway {node} needs at least two ports")));
        }
        ports.sort_unstable();
        let processing_delay_ps = obj
            .get("processing_delay")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| error("processing_delay must be a string"))
                    .and_then(parse_time)
            })
            .transpose()?
            .unwrap_or(0);
        let hop_limit = if obj.contains_key("hop_limit") {
            integer(obj, "hop_limit", 65535)? as u32
        } else {
            16
        };
        if hop_limit == 0 {
            return Err(error(format!("Gateway {node} hop_limit must be positive")));
        }
        let mut routes: Vec<Route> = Vec::new();
        let mut ids = BTreeSet::new();
        for value in array(obj, "routes")? {
            let row = object(
                value,
                &["id", "ingress", "egress", "format", "id_min", "id_max"],
                "Route",
            )?;
            let id = required_string(row, "id")?;
            if !identifier(id) || !ids.insert(id.to_string()) {
                return Err(error(format!(
                    "invalid or duplicate route id at {node}: {id}"
                )));
            }
            let ingress = port(
                row.get("ingress")
                    .ok_or_else(|| error("missing route ingress"))?,
                controllers,
            )?;
            if !ports.contains(&ingress) {
                return Err(error(format!(
                    "route {node}/{id} ingress is not a Gateway port"
                )));
            }
            let mut egress = Vec::new();
            for value in array(row, "egress")? {
                let p = port(value, controllers)?;
                if !ports.contains(&p) || buses[p] == buses[ingress] || egress.contains(&p) {
                    return Err(error(format!("route {node}/{id} invalid egress")));
                }
                egress.push(p);
            }
            if egress.is_empty() {
                return Err(error(format!("route {node}/{id} needs egress")));
            }
            egress.sort_unstable();
            let format = required_string(row, "format")?;
            let max = match format {
                "standard" => 2047,
                "extended" => 536_870_911,
                _ => return Err(error(format!("route {node}/{id} unknown format"))),
            };
            let id_min = integer(row, "id_min", max)? as u32;
            let id_max = integer(row, "id_max", max)? as u32;
            if id_min > id_max {
                return Err(error(format!("route {node}/{id} id_min exceeds id_max")));
            }
            if routes.iter().any(|r| {
                r.ingress == ingress
                    && r.format == format
                    && r.id_min <= id_max
                    && id_min <= r.id_max
            }) {
                return Err(error(format!("route_overlap at {node}/{id}")));
            }
            routes.push(Route {
                id: id.into(),
                ingress,
                egress,
                format: format.into(),
                id_min,
                id_max,
            });
        }
        routes.sort_by(|a, b| {
            (a.ingress, &a.format, a.id_min, &a.id).cmp(&(b.ingress, &b.format, b.id_min, &b.id))
        });
        gateways.push(Gateway {
            node: node.into(),
            ports,
            rx_queue_capacity: if obj.contains_key("rx_queue_capacity") {
                integer(obj, "rx_queue_capacity", u32::MAX as u64)?
            } else {
                64
            },
            routes,
            processing_delay_ps,
            hop_limit,
        });
    }
    gateways.sort_by(|a, b| a.node.cmp(&b.node));
    Ok(gateways)
}

pub(super) fn validate(
    gateways: &[Gateway],
    generators: &[Generator],
    buses: &[usize],
    controllers: &[Controller],
    bus_names: &[crate::types::Bus],
) -> Result<Vec<Option<usize>>> {
    let mut memberships = vec![None; controllers.len()];
    for (g, gateway) in gateways.iter().enumerate() {
        for &p in &gateway.ports {
            memberships[p] = Some(g);
        }
    }
    // Each reservation carries its owner; reservations for the same owner may overlap.
    type Reservations = BTreeMap<(usize, String), Vec<(u32, u32, usize)>>;
    let mut owners = Reservations::new();
    for generator in generators {
        if memberships[generator.source].is_some() {
            return Err(error(format!(
                "native generator {} targets Gateway port {}",
                generator.id, controllers[generator.source].id
            )));
        }
        owners
            .entry((buses[generator.source], generator.frame.format.clone()))
            .or_default()
            .push((generator.frame.id, generator.frame.id, generator.source));
    }
    for gateway in gateways {
        for route in &gateway.routes {
            for &p in &route.egress {
                owners
                    .entry((buses[p], route.format.clone()))
                    .or_default()
                    .push((route.id_min, route.id_max, p));
            }
        }
    }
    for rows in owners.values() {
        for (i, &(lo, hi, owner)) in rows.iter().enumerate() {
            for &(other_lo, other_hi, other) in &rows[..i] {
                if owner != other && lo <= other_hi && other_lo <= hi {
                    return Err(error(format!(
                        "owner_overlap: {} and {} reserve intersecting CAN IDs",
                        controllers[owner].id, controllers[other].id
                    )));
                }
            }
        }
    }
    for format in ["standard", "extended"] {
        let routes: Vec<_> = gateways
            .iter()
            .flat_map(|g| g.routes.iter().map(move |r| (g, r)))
            .filter(|(_, r)| r.format == format)
            .collect();
        let boundaries: BTreeSet<u64> = routes
            .iter()
            .flat_map(|(_, r)| [r.id_min as u64, r.id_max as u64 + 1])
            .collect();
        let boundaries: Vec<_> = boundaries.into_iter().collect();
        for interval in boundaries.windows(2) {
            let id = interval[0];
            let mut graph: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
            for (_, r) in &routes {
                if r.id_min as u64 <= id && id <= r.id_max as u64 {
                    for &p in &r.egress {
                        graph.entry(buses[r.ingress]).or_default().insert(buses[p]);
                    }
                }
            }
            fn cycle(
                node: usize,
                graph: &BTreeMap<usize, BTreeSet<usize>>,
                stack: &mut Vec<usize>,
                done: &mut BTreeSet<usize>,
            ) -> Option<Vec<usize>> {
                if let Some(index) = stack.iter().position(|&n| n == node) {
                    let mut path = stack[index..].to_vec();
                    path.push(node);
                    return Some(path);
                }
                if done.contains(&node) {
                    return None;
                }
                stack.push(node);
                if let Some(next) = graph.get(&node) {
                    for &n in next {
                        if let Some(path) = cycle(n, graph, stack, done) {
                            return Some(path);
                        }
                    }
                }
                stack.pop();
                done.insert(node);
                None
            }
            let mut done = BTreeSet::new();
            for &bus in graph.keys() {
                if let Some(path) = cycle(bus, &graph, &mut Vec::new(), &mut done) {
                    let edges: Vec<_> = path
                        .windows(2)
                        .map(|pair| {
                            let (gw, route) = routes
                                .iter()
                                .find(|(_, r)| {
                                    r.id_min as u64 <= id
                                        && id <= r.id_max as u64
                                        && buses[r.ingress] == pair[0]
                                        && r.egress.iter().any(|&p| buses[p] == pair[1])
                                })
                                .unwrap();
                            format!(
                                "{} --{}/{}--> {}",
                                bus_names[pair[0]].id, gw.node, route.id, bus_names[pair[1]].id
                            )
                        })
                        .collect();
                    return Err(error(format!(
                        "route_cycle for {format} ID interval [{id}, {}]: {}",
                        interval[1] - 1,
                        edges.join("; ")
                    )));
                }
            }
        }
    }
    Ok(memberships)
}
