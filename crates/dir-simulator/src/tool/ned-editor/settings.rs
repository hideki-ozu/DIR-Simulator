//! Forms edit the ordinary INI/JSON sources; incomplete graphs remain editable.
use super::analysis::{DeclarationShape, shapes};
use super::controller::strict_json;
use super::input::{CapturedFile, FileRole};
use super::model::EditorSession;
use super::{EditorError, Result, hash, random_id};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn error(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-SETTINGS", message, 422)
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| error(format!("{key} must be a string")))
}
fn keys(v: &Value, allowed: &[&str]) -> Result<()> {
    let object = v.as_object().ok_or_else(|| error("Expected an object"))?;
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(error(format!("Unknown setting: {key}")));
    }
    Ok(())
}
fn integer(v: &Value, key: &str, min: u64, max: u64) -> Result<u64> {
    v[key]
        .as_u64()
        .filter(|n| (*n >= min) && (*n <= max))
        .ok_or_else(|| error(format!("Invalid integer {key}")))
}
fn time(v: &Value, key: &str) -> Result<u64> {
    crate::input::parse_time(field(v, key)?)
        .map_err(|d| EditorError::common("E-EDITOR-SETTINGS", d))
}
fn valid_id(value: &str) -> Result<()> {
    if !crate::input::identifier(value) {
        return Err(error("Invalid identifier"));
    }
    Ok(())
}
pub(crate) fn instances(model: &EditorSession) -> Result<BTreeMap<String, DeclarationShape>> {
    let mut types = BTreeMap::new();
    for f in model.project.ned_files() {
        for d in shapes(model.current_parse(&f.id)?) {
            if types.insert(d.name.clone(), d).is_some() {
                return Err(error("Ambiguous type definition"));
            }
        }
    }
    let network = model
        .project
        .header
        .network
        .as_ref()
        .ok_or_else(|| error("Network is not specified"))?;
    let mut stack = vec![(
        network.rsplit('.').next().unwrap().to_owned(),
        network.clone(),
        BTreeSet::new(),
    )];
    let mut result = BTreeMap::new();
    while let Some((path, name, mut ancestors)) = stack.pop() {
        if result.len() >= 10000 || ancestors.len() >= 128 || !ancestors.insert(name.clone()) {
            return Err(error("Instance hierarchy is cyclic or exceeds the limit"));
        }
        let d = types
            .get(&name)
            .ok_or_else(|| error(format!("Unresolved type: {name}")))?;
        result.insert(path.clone(), d.clone());
        for (child, ty) in &d.children {
            stack.push((format!("{path}.{child}"), ty.clone(), ancestors.clone()));
        }
    }
    Ok(result)
}

pub(crate) fn document(model: &EditorSession, role: FileRole) -> Result<Value> {
    let file = model
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&role));
    let value = if let Some(file) = file {
        strict_json(file.text.as_bytes())?
    } else if role == FileRole::ModelConfig {
        json!({"schema_version":1,"gateways":[]})
    } else {
        json!({"schema_version":1,"generators":[]})
    };
    let array = if role == FileRole::ModelConfig {
        "gateways"
    } else {
        "generators"
    };
    keys(&value, &["schema_version", array])?;
    let version = value["schema_version"]
        .as_u64()
        .ok_or_else(|| error("Missing schema_version"))?;
    if version != 1
        && !(array == "generators"
            && version == 2
            && model.project.header.profile.as_deref() == Some("can.cc.multibus.v1"))
    {
        return Err(error("Unsupported settings schema"));
    }
    let mut row_ids = BTreeSet::new();
    for row in value[array]
        .as_array()
        .ok_or_else(|| error(format!("{array} must be an array")))?
    {
        if array == "gateways" {
            keys(
                row,
                &[
                    "node",
                    "ports",
                    "routes",
                    "processing_delay",
                    "hop_limit",
                    "rx_queue_capacity",
                ],
            )?;
            if !row_ids.insert(field(row, "node")?) {
                return Err(error("Duplicate Gateway node"));
            }
            let ports = strings(&row["ports"])?;
            if ports.iter().collect::<BTreeSet<_>>().len() != ports.len() {
                return Err(error("Duplicate Gateway ports"));
            }
            if row.get("processing_delay").is_some() {
                time(row, "processing_delay")?;
            }
            if row.get("hop_limit").is_some() {
                integer(row, "hop_limit", 1, 65535)?;
            }
            if row.get("rx_queue_capacity").is_some() {
                integer(row, "rx_queue_capacity", 0, 4294967295)?;
            }
            let mut route_ids = BTreeSet::new();
            for route in row["routes"]
                .as_array()
                .ok_or_else(|| error("Missing routes"))?
            {
                keys(
                    route,
                    &["id", "ingress", "egress", "format", "id_min", "id_max"],
                )?;
                let id = field(route, "id")?;
                valid_id(id)?;
                if !route_ids.insert(id) {
                    return Err(error("Duplicate route id"));
                }
                let ingress = field(route, "ingress")?;
                let egress = strings(&route["egress"])?;
                if !ports.iter().any(|port| port == ingress)
                    || egress.is_empty()
                    || egress
                        .iter()
                        .any(|port| port == ingress || !ports.contains(port))
                    || egress.iter().collect::<BTreeSet<_>>().len() != egress.len()
                {
                    return Err(error("Invalid or duplicate route ports"));
                }
                let max = match field(route, "format")? {
                    "standard" => 2047,
                    "extended" => 536870911,
                    _ => return Err(error("Unsupported frame format")),
                };
                let min = integer(route, "id_min", 0, max)?;
                integer(route, "id_max", min, max)?;
            }
        } else {
            let kind = field(row, "kind")?;
            let fields = if kind == "can.explicit.v1" {
                &["id", "kind", "node", "frame", "times"][..]
            } else if kind == "can.periodic.v1" {
                &[
                    "id", "kind", "node", "frame", "start", "phase", "period", "end", "count",
                ][..]
            } else {
                return Err(error("Unknown generator kind"));
            };
            keys(row, fields)?;
            let id = field(row, "id")?;
            valid_id(id)?;
            if !row_ids.insert(id) {
                return Err(error("Duplicate generator id"));
            }
            field(row, "node")?;
            keys(&row["frame"], &["format", "id", "data"])?;
            let max = match field(&row["frame"], "format")? {
                "standard" => 2047,
                "extended" => 536870911,
                _ => return Err(error("Unsupported frame format")),
            };
            integer(&row["frame"], "id", 0, max)?;
            let data = field(&row["frame"], "data")?;
            if data.len() > 16
                || data.len() % 2 != 0
                || !data.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(error("Invalid CAN payload"));
            }
            if kind == "can.explicit.v1" {
                for t in strings(&row["times"])? {
                    crate::input::parse_time(&t)
                        .map_err(|d| EditorError::common("E-EDITOR-SETTINGS", d))?;
                }
            } else {
                time(row, "start")?;
                time(row, "period")?;
                if row.get("phase").is_some() {
                    time(row, "phase")?;
                }
                if row.get("end").is_some() {
                    time(row, "end")?;
                }
                if row.get("count").is_some() {
                    integer(row, "count", 0, 9007199254740991)?;
                }
            }
        }
    }
    Ok(value)
}
fn strings(value: &Value) -> Result<Vec<String>> {
    value
        .as_array()
        .ok_or_else(|| error("Expected a list"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| error("List values must be strings"))
        })
        .collect()
}
fn json_edit(
    model: &EditorSession,
    role: FileRole,
    value: Value,
    text: &mut Vec<(String, String)>,
    added: &mut Vec<CapturedFile>,
    ini: &mut BTreeMap<String, String>,
) -> Result<()> {
    let raw = format!(
        "{}\n",
        serde_json::to_string_pretty(&value).map_err(|e| error(e.to_string()))?
    );
    if let Some(file) = model
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&role))
    {
        text.push((file.id.clone(), raw));
    } else {
        let key = if role == FileRole::ModelConfig {
            "model-config"
        } else {
            "workload"
        };
        let basename = format!("{}.json", random_id(&format!("editor-{key}-"))?);
        let path = model
            .project
            .config
            .parent()
            .ok_or_else(|| error("Config parent missing"))?
            .join(&basename);
        let id = format!("f{}", &hash(path.to_string_lossy().as_bytes())[..24]);
        let digest = hash(raw.as_bytes());
        let raw: std::sync::Arc<str> = raw.into();
        added.push(CapturedFile {
            id,
            path,
            roles: vec![role],
            text: raw.clone(),
            hash: digest.clone(),
            origin_text: raw,
            origin_hash: digest,
            stat: None,
        });
        ini.insert(key.into(), format!("\"{basename}\""));
    }
    Ok(())
}

impl EditorSession {
    pub fn settings_command(&mut self, kind: &str, p: &Value) -> Result<bool> {
        let mut text = vec![];
        let mut added = vec![];
        let mut changes = BTreeMap::new();
        let mut remove = None;
        match kind {
            "set_project_settings" => {
                time(p, "sim_time_limit")?;
                if time(p, "metrics_window")? == 0 {
                    return Err(error("metrics_window must be positive"));
                }
                for (from, to) in [
                    ("sim_time_limit", "sim-time-limit"),
                    ("metrics_window", "metrics-window"),
                    ("max_events", "max-events"),
                    ("max_delta_cycles", "max-delta-cycles"),
                ] {
                    let value = field(p, from)?;
                    if from.starts_with("max_")
                        && (value.starts_with('0')
                            || !value.bytes().all(|b| b.is_ascii_digit())
                            || value.parse::<u64>().ok().filter(|n| *n > 0).is_none())
                    {
                        return Err(error(format!("Invalid {from}")));
                    }
                    changes.insert(to.into(), value.into());
                }
            }
            "set_instance_parameter" => {
                let path = field(p, "instance_path")?;
                let name = field(p, "parameter_name")?;
                let all = instances(self)?;
                let shape = all
                    .get(path)
                    .ok_or_else(|| error("Selected instance no longer exists"))?;
                if !shape.parameters.contains_key(name) {
                    return Err(error("Unknown instance parameter"));
                }
                let key = format!("{path}.{name}");
                if p["literal"].is_null() {
                    remove = Some(key);
                } else {
                    let literal = field(p, "literal")?;
                    if literal.contains(['\r', '\n']) {
                        return Err(error("Parameter must be a single literal"));
                    }
                    let declaration = self
                        .project
                        .ned_files()
                        .find_map(|f| {
                            self.current_parse(&f.id).ok().and_then(|parsed| {
                                parsed
                                    .declarations()
                                    .iter()
                                    .find(|d| d.name() == shape.name)
                            })
                        })
                        .ok_or_else(|| error("Unknown declaration"))?;
                    crate::input::validate_parameter_literal(
                        declaration,
                        name,
                        literal,
                        self.project
                            .header
                            .profile
                            .as_deref()
                            .unwrap_or("can.cc.ideal.v1"),
                    )
                    .map_err(|d| EditorError::common("E-EDITOR-SETTINGS", d))?;
                    changes.insert(key, literal.into());
                }
            }
            "set_gateway" | "delete_gateway" => {
                if self.project.header.profile.as_deref() != Some("can.cc.multibus.v1") {
                    return Err(error("Gateway settings require a Multibus project"));
                }
                let node = field(p, "node")?;
                let mut doc = document(self, FileRole::ModelConfig)?;
                let rows = doc["gateways"].as_array_mut().unwrap();
                if kind == "delete_gateway" {
                    rows.retain(|g| g["node"] != node);
                } else {
                    let all = instances(self)?;
                    if all.get(node).is_none_or(|d| d.kind != "module") {
                        return Err(error("Select an actual compound module instance"));
                    }
                    time(p, "processing_delay")?;
                    integer(p, "hop_limit", 1, 65535)?;
                    integer(p, "rx_queue_capacity", 0, 4294967295)?;
                    let ports = strings(&p["ports"])?;
                    let mut seen = BTreeSet::new();
                    if ports.len() < 2 {
                        return Err(error("Gateway needs at least two ports"));
                    }
                    for port in &ports {
                        if !port.starts_with(&format!("{node}."))
                            || !seen.insert(port.clone())
                            || all.get(port).is_none_or(|d| {
                                d.implementation.as_deref() != Some("dir.can.MultibusController")
                            })
                        {
                            return Err(error(
                                "Gateway ports must be distinct descendant Multibus Controllers",
                            ));
                        }
                        if rows.iter().any(|g| {
                            g["node"] != node
                                && g["ports"]
                                    .as_array()
                                    .is_some_and(|list| list.iter().any(|v| v == port))
                        }) {
                            return Err(error(
                                "Controller port is already owned by another Gateway",
                            ));
                        }
                    }
                    let workload = document(self, FileRole::Workload)?;
                    if workload["generators"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|g| g["node"].as_str().is_some_and(|node| seen.contains(node)))
                    {
                        return Err(error("A Gateway port cannot own native workload traffic"));
                    }
                    let routes = p["routes"]
                        .as_array()
                        .ok_or_else(|| error("Missing routes"))?;
                    let mut ids = BTreeSet::new();
                    for (index, r) in routes.iter().enumerate() {
                        let id = field(r, "id")?;
                        valid_id(id)?;
                        if !ids.insert(id) {
                            return Err(error("Duplicate route id"));
                        }
                        let ingress = field(r, "ingress")?;
                        let egress = strings(&r["egress"])?;
                        if !seen.contains(ingress)
                            || egress.is_empty()
                            || egress.iter().any(|n| !seen.contains(n) || n == ingress)
                            || egress.iter().collect::<BTreeSet<_>>().len() != egress.len()
                        {
                            return Err(error("Invalid route ports"));
                        }
                        let max = match field(r, "format")? {
                            "standard" => 2047,
                            "extended" => 536870911,
                            _ => return Err(error("Invalid frame format")),
                        };
                        let lo = integer(r, "id_min", 0, max)?;
                        let hi = integer(r, "id_max", lo, max)?;
                        if routes[..index].iter().any(|old| {
                            old["ingress"] == r["ingress"]
                                && old["format"] == r["format"]
                                && old["id_min"].as_u64().unwrap() <= hi
                                && lo <= old["id_max"].as_u64().unwrap()
                        }) {
                            return Err(error("Overlapping Gateway routes"));
                        }
                    }
                    let value = json!({"node":node,"ports":ports,"processing_delay":p["processing_delay"],"hop_limit":p["hop_limit"],"rx_queue_capacity":p["rx_queue_capacity"],"routes":routes});
                    if let Some(index) = rows.iter().position(|g| g["node"] == node) {
                        rows[index] = value;
                    } else {
                        rows.push(value);
                    }
                }
                json_edit(
                    self,
                    FileRole::ModelConfig,
                    doc,
                    &mut text,
                    &mut added,
                    &mut changes,
                )?;
            }
            "set_workload" => {
                let all = instances(self)?;
                let routing = document(self, FileRole::ModelConfig)?;
                let reserved: BTreeSet<_> = routing["gateways"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|g| {
                        g["ports"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter_map(Value::as_str)
                    })
                    .collect();
                let generators = p["generators"]
                    .as_array()
                    .ok_or_else(|| error("Missing generators"))?;
                let mut ids = BTreeSet::new();
                for g in generators {
                    let id = field(g, "id")?;
                    valid_id(id)?;
                    if !ids.insert(id) {
                        return Err(error("Duplicate generator id"));
                    }
                    let node = field(g, "node")?;
                    if reserved.contains(node)
                        || all.get(node).is_none_or(|d| {
                            !matches!(
                                d.implementation.as_deref(),
                                Some("dir.can.Controller" | "dir.can.MultibusController")
                            )
                        })
                    {
                        return Err(error("Choose a non-Gateway Controller for native traffic"));
                    }
                    let max = match field(&g["frame"], "format")? {
                        "standard" => 2047,
                        "extended" => 536870911,
                        _ => return Err(error("Invalid frame format")),
                    };
                    integer(&g["frame"], "id", 0, max)?;
                    let data = field(&g["frame"], "data")?;
                    if data.len() > 16
                        || data.len() % 2 != 0
                        || !data.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        return Err(error(
                            "Payload must be at most 8 bytes of hexadecimal digits",
                        ));
                    }
                    match field(g, "kind")? {
                        "can.explicit.v1" => {
                            let mut previous = 0;
                            for (i, t) in strings(&g["times"])?.iter().enumerate() {
                                let n = crate::input::parse_time(t)
                                    .map_err(|d| EditorError::common("E-EDITOR-SETTINGS", d))?;
                                if i > 0 && n < previous {
                                    return Err(error("Explicit times must be sorted"));
                                }
                                previous = n;
                            }
                        }
                        "can.periodic.v1" => {
                            let start = time(g, "start")?;
                            let period = time(g, "period")?;
                            let phase = if g.get("phase").is_some() {
                                time(g, "phase")?
                            } else {
                                0
                            };
                            if period == 0 || phase >= period {
                                return Err(error("Invalid periodic schedule"));
                            }
                            if g.get("end").is_some() && time(g, "end")? < start {
                                return Err(error("Periodic end precedes start"));
                            }
                            if g.get("count").is_some() {
                                integer(g, "count", 0, 9007199254740991)?;
                            }
                        }
                        _ => return Err(error("Unknown generator kind")),
                    }
                }
                let mut doc = document(self, FileRole::Workload)?;
                doc["generators"] = p["generators"].clone();
                json_edit(
                    self,
                    FileRole::Workload,
                    doc,
                    &mut text,
                    &mut added,
                    &mut changes,
                )?;
            }
            _ => return Err(error("Unknown settings command")),
        }
        let config = self
            .project
            .file_by_path(&self.project.config)
            .ok_or_else(|| error("Missing INI"))?;
        let mut ini = super::builtin::ini_values(&config.text, &changes)?;
        if let Some(key) = remove {
            ini = ini
                .split_inclusive('\n')
                .filter(|line| {
                    line.split_once('=')
                        .is_none_or(|(left, _)| left.trim() != key)
                })
                .collect();
        }
        crate::input::inspect_config(&ini, &self.project.config, &self.project.cwd)
            .map_err(|d| EditorError::common("E-EDITOR-SETTINGS", d))?;
        if ini != config.text.as_ref() {
            text.push((config.id.clone(), ini));
        }
        self.commit_with_files(text, vec![], added)
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
