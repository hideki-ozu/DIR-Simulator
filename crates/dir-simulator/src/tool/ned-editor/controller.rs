//! HTTP-independent sequencing and exclusive asynchronous editor operations.
use super::analysis::{parse_project, prepare, shapes};
use super::input::{FileRole, ProjectSnapshot, load_project};
use super::model::EditorSession;
use super::output::{SaveOutcome, SaveReport, TargetRegistry, build_plan, recover, save_plan};
use super::{EditorError, Result, hash, random_id};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) type Shared = Arc<Mutex<Controller>>;
const BODY_BYTES: usize = 64 * 1024 * 1024;
const LEDGER_LIMIT: usize = 1000;

/// Deserialize every object ourselves, including objects nested inside arrays.
struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueJson(Value::Number(n)))
                    .ok_or_else(|| E::custom("Non-finite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = seq.next_element::<UniqueJson>()? {
                    values.push(v.0);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!("Duplicate JSON key: {key}")));
                    }
                    values.insert(key, map.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}
pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value> {
    // Persisted output manifests also use this parser; their size is not an HTTP limit.
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueJson::deserialize(&mut parser).map_err(|e| malformed(e.to_string()))?;
    parser.end().map_err(|e| malformed(e.to_string()))?;
    Ok(value.0)
}
fn malformed(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-REQUEST", message, 400)
}
fn field<'a>(v: &'a Value, name: &str) -> Result<&'a str> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(format!("{name} must be a string")))
}
fn decimal(v: &Value, name: &str) -> Result<u64> {
    let s = field(v, name)?;
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(malformed(format!(
            "{name} must be a canonical decimal u64 string"
        )));
    }
    s.parse()
        .map_err(|_| malformed(format!("{name} exceeds u64")))
}
fn fields(v: &Value, required: &[&str], optional: &[&str]) -> Result<()> {
    let object = v
        .as_object()
        .ok_or_else(|| malformed("Expected an object"))?;
    for name in object.keys() {
        if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
            return Err(malformed(format!("Unknown field: {name}")));
        }
    }
    for name in required {
        if !object.contains_key(*name) {
            return Err(malformed(format!("Missing field: {name}")));
        }
    }
    Ok(())
}
fn text_fields(v: &Value, names: &[&str]) -> Result<()> {
    for name in names {
        field(v, name)?;
    }
    Ok(())
}
fn optional_text(v: &Value, names: &[&str]) -> Result<()> {
    for name in names {
        if let Some(value) = v.get(*name) {
            if !value.is_string() {
                return Err(malformed(format!("{name} must be a string")));
            }
        }
    }
    Ok(())
}
fn optional_channels(v: &Value, names: &[&str]) -> Result<()> {
    for name in names {
        if v.get(*name).is_some_and(|v| !v.is_string() && !v.is_null()) {
            return Err(malformed(format!("{name} must be string or null")));
        }
    }
    Ok(())
}
fn position(v: &Value, collapsed: bool) -> Result<()> {
    fields(v, &["x", "y"], if collapsed { &["collapsed"] } else { &[] })?;
    for axis in ["x", "y"] {
        if !v[axis].as_f64().is_some_and(f64::is_finite) {
            return Err(malformed("Position must contain finite numbers"));
        }
    }
    if v.get("collapsed").is_some_and(|v| !v.is_boolean()) {
        return Err(malformed("collapsed must be boolean"));
    }
    Ok(())
}
fn graph_kind(kind: &str) -> bool {
    matches!(
        kind,
        "add_child"
            | "create_module"
            | "add_gates"
            | "delete_gate"
            | "add_port"
            | "delete_child"
            | "connect"
            | "reconnect"
            | "disconnect"
            | "connect_can_pair"
            | "set_default"
            | "set_layout"
    )
}
fn settings_kind(kind: &str) -> bool {
    matches!(
        kind,
        "set_gateway"
            | "delete_gateway"
            | "set_workload"
            | "set_project_settings"
            | "set_instance_parameter"
    )
}
fn validate_payload(kind: &str, p: &Value) -> Result<()> {
    match kind {
        "create_module" => {
            fields(
                p,
                &["parent_type", "type_name", "child_name", "position"],
                &[],
            )?;
            text_fields(p, &["parent_type", "type_name", "child_name"])?;
            position(&p["position"], false)?;
        }
        "add_port" => {
            fields(
                p,
                &[
                    "type_key",
                    "child_name",
                    "input_gate",
                    "output_gate",
                    "position",
                ],
                &[],
            )?;
            text_fields(p, &["type_key", "child_name", "input_gate", "output_gate"])?;
            position(&p["position"], false)?;
        }
        "add_gates" => {
            fields(p, &["type_key", "gates"], &[])?;
            field(p, "type_key")?;
            let gates = p["gates"]
                .as_array()
                .ok_or_else(|| malformed("gates must be a list"))?;
            if gates.is_empty() || gates.len() > 10000 {
                return Err(malformed("Invalid gate count"));
            }
            for gate in gates {
                fields(gate, &["name", "output"], &[])?;
                field(gate, "name")?;
                if !gate["output"].is_boolean() {
                    return Err(malformed("output must be boolean"));
                }
            }
        }
        "delete_gate" => {
            fields(p, &["type_key", "gate_name"], &["paired_gate"])?;
            text_fields(p, &["type_key", "gate_name"])?;
            optional_text(p, &["paired_gate"])?;
        }
        "set_instance_parameter" => {
            fields(p, &["instance_path", "parameter_name", "literal"], &[])?;
            text_fields(p, &["instance_path", "parameter_name"])?;
            if !p["literal"].is_null() && !p["literal"].is_string() {
                return Err(malformed("literal must be string or null"));
            }
        }
        "set_project_settings" => {
            fields(
                p,
                &[
                    "sim_time_limit",
                    "metrics_window",
                    "max_events",
                    "max_delta_cycles",
                ],
                &[],
            )?;
            text_fields(
                p,
                &[
                    "sim_time_limit",
                    "metrics_window",
                    "max_events",
                    "max_delta_cycles",
                ],
            )?;
        }
        "delete_gateway" => {
            fields(p, &["node"], &[])?;
            field(p, "node")?;
        }
        "set_gateway" => {
            fields(
                p,
                &[
                    "node",
                    "ports",
                    "processing_delay",
                    "hop_limit",
                    "rx_queue_capacity",
                    "routes",
                ],
                &[],
            )?;
            text_fields(p, &["node", "processing_delay"])?;
            for key in ["hop_limit", "rx_queue_capacity"] {
                if p[key].as_u64().is_none() {
                    return Err(malformed(
                        "Capacity and hop values must be nonnegative integers",
                    ));
                }
            }
            if p["ports"]
                .as_array()
                .is_none_or(|a| a.iter().any(|v| !v.is_string()))
            {
                return Err(malformed("ports must be a string list"));
            }
            for r in p["routes"]
                .as_array()
                .ok_or_else(|| malformed("routes must be a list"))?
            {
                fields(
                    r,
                    &["id", "ingress", "egress", "format", "id_min", "id_max"],
                    &[],
                )?;
                text_fields(r, &["id", "ingress", "format"])?;
                if r["egress"]
                    .as_array()
                    .is_none_or(|a| a.iter().any(|v| !v.is_string()))
                    || r["id_min"].as_u64().is_none()
                    || r["id_max"].as_u64().is_none()
                {
                    return Err(malformed("Invalid route DTO"));
                }
            }
        }
        "set_workload" => {
            fields(p, &["generators"], &[])?;
            for g in p["generators"]
                .as_array()
                .ok_or_else(|| malformed("generators must be a list"))?
            {
                match field(g, "kind")? {
                    "can.explicit.v1" => {
                        fields(g, &["id", "kind", "node", "frame", "times"], &[])?;
                        if g["times"]
                            .as_array()
                            .is_none_or(|a| a.iter().any(|v| !v.is_string()))
                        {
                            return Err(malformed("times must be a string list"));
                        }
                    }
                    "can.periodic.v1" => {
                        fields(
                            g,
                            &["id", "kind", "node", "frame", "start", "period"],
                            &["phase", "end", "count"],
                        )?;
                        text_fields(g, &["start", "period"])?;
                        optional_text(g, &["phase", "end"])?;
                        if g.get("count").is_some_and(|v| v.as_u64().is_none()) {
                            return Err(malformed("count must be a nonnegative integer"));
                        }
                    }
                    _ => return Err(malformed("Unknown generator kind")),
                }
                text_fields(g, &["id", "kind", "node"])?;
                fields(&g["frame"], &["format", "id", "data"], &[])?;
                text_fields(&g["frame"], &["format", "data"])?;
                if g["frame"]["id"].as_u64().is_none() {
                    return Err(malformed("Frame id must be a nonnegative integer"));
                }
            }
        }
        "replace_source" => {
            fields(p, &["file_id", "expected_hash", "text"], &[])?;
            text_fields(p, &["file_id", "expected_hash", "text"])?;
        }
        "add_child" => {
            fields(
                p,
                &["parent_type", "child_name", "type_name", "position"],
                &[],
            )?;
            text_fields(p, &["parent_type", "child_name", "type_name"])?;
            position(&p["position"], false)?;
        }
        "delete_child" | "disconnect" => {
            let key = if kind == "delete_child" {
                "element_key"
            } else {
                "connection_key"
            };
            fields(p, &[key], &[])?;
            field(p, key)?;
        }
        "connect" => {
            fields(p, &["parent_type", "from", "to"], &["channel"])?;
            text_fields(p, &["parent_type", "from", "to"])?;
            optional_channels(p, &["channel"])?;
        }
        "reconnect" => {
            fields(
                p,
                &["connection_key", "endpoint_side", "new_endpoint"],
                &["channel"],
            )?;
            text_fields(p, &["connection_key", "endpoint_side", "new_endpoint"])?;
            optional_channels(p, &["channel"])?;
            if !matches!(p["endpoint_side"].as_str(), Some("start" | "end")) {
                return Err(malformed("Invalid endpoint_side"));
            }
        }
        "connect_can_pair" => {
            fields(
                p,
                &[
                    "parent_type",
                    "controller",
                    "bus",
                    "input_gate",
                    "output_gate",
                ],
                &["tx_channel", "rx_channel"],
            )?;
            text_fields(
                p,
                &[
                    "parent_type",
                    "controller",
                    "bus",
                    "input_gate",
                    "output_gate",
                ],
            )?;
            optional_channels(p, &["tx_channel", "rx_channel"])?;
        }
        "set_default" => {
            fields(p, &["parameter_key", "literal"], &[])?;
            field(p, "parameter_key")?;
            if !p["literal"].is_null() && !p["literal"].is_string() {
                return Err(malformed("literal must be string or null"));
            }
        }
        "set_layout" => {
            fields(p, &["type_key", "positions"], &[])?;
            field(p, "type_key")?;
            for value in p["positions"]
                .as_object()
                .ok_or_else(|| malformed("positions must be an object"))?
                .values()
            {
                position(value, true)?;
            }
        }
        "undo" | "redo" | "validate" => fields(p, &[], &[])?,
        "save_as_project" => {
            fields(p, &["destination_id"], &["replace_layout_ack"])?;
            text_fields(p, &["destination_id"])?;
            optional_text(p, &["replace_layout_ack"])?;
        }
        "overwrite_project" => {
            fields(
                p,
                &["destination_id", "overwrite_ack"],
                &["replace_layout_ack"],
            )?;
            text_fields(p, &["destination_id", "overwrite_ack"])?;
            optional_text(p, &["replace_layout_ack"])?;
        }
        "reload" => {
            fields(p, &[], &["discard_ack"])?;
            optional_text(p, &["discard_ack"])?;
        }
        "new_project" => {
            fields(p, &["template", "project_name"], &["discard_ack"])?;
            text_fields(p, &["template", "project_name"])?;
            optional_text(p, &["discard_ack"])?;
            if !matches!(
                field(p, "template")?,
                "can" | "multibus" | "multibus-gateway" | "multibus-empty"
            ) {
                return Err(malformed("Unknown project template"));
            }
        }
        "recover" => {
            fields(p, &["recovery_id", "action"], &[])?;
            text_fields(p, &["recovery_id", "action"])?;
            if !matches!(p["action"].as_str(), Some("complete" | "restore")) {
                return Err(malformed("Invalid recovery action"));
            }
        }
        _ => return Err(malformed("Unknown command kind")),
    }
    Ok(())
}
fn payload_digest(p: &Value) -> String {
    let mut p = p.clone();
    if let Some(object) = p.as_object_mut() {
        for key in ["overwrite_ack", "replace_layout_ack", "discard_ack"] {
            object.remove(key);
        }
    }
    hash(&serde_json::to_vec(&p).expect("JSON serializes"))
}

#[derive(Default)]
struct ClientLedger {
    high_watermark: u64,
    inflight: Option<String>,
    terminal: VecDeque<String>,
}
struct LedgerEntry {
    request: Value,
    response: Value,
    job: Option<String>,
}
struct Confirmation {
    epoch: u64,
    revision: u64,
    kind: String,
    payload_digest: String,
    plan_digest: Option<String>,
    replace_layout: bool,
}
#[derive(Clone)]
struct Stamp {
    revision: u64,
    input_revision: u64,
    context_epoch: u64,
    snapshot: String,
}
impl Stamp {
    fn of(m: &EditorSession) -> Self {
        Self {
            revision: m.revision,
            input_revision: m.input_revision,
            context_epoch: m.context_epoch,
            snapshot: m.project.id.clone(),
        }
    }
    fn matches(&self, m: &EditorSession) -> bool {
        self.revision == m.revision
            && self.input_revision == m.input_revision
            && self.context_epoch == m.context_epoch
            && self.snapshot == m.project.id
    }
}
#[derive(Clone)]
struct Command {
    id: String,
    client: String,
    sequence: u64,
    base_revision: u64,
    kind: String,
    payload: Value,
    impact_ack: Option<String>,
}
impl Command {
    fn parse(v: &Value) -> Result<Self> {
        fields(
            v,
            &[
                "schema_version",
                "session_id",
                "client_id",
                "writer_epoch",
                "client_sequence",
                "command_id",
                "base_revision",
                "kind",
                "payload",
            ],
            &["impact_ack"],
        )?;
        if v["schema_version"] != 1 {
            return Err(malformed("Unsupported schema_version"));
        }
        text_fields(v, &["session_id", "client_id", "command_id", "kind"])?;
        optional_text(v, &["impact_ack"])?;
        decimal(v, "writer_epoch")?;
        let sequence = decimal(v, "client_sequence")?;
        let client = field(v, "client_id")?.to_string();
        let id = field(v, "command_id")?.to_string();
        if id != format!("{client}:{sequence}") {
            return Err(malformed("command_id must match client_id:client_sequence"));
        }
        let kind = field(v, "kind")?.to_string();
        validate_payload(&kind, &v["payload"])?;
        Ok(Self {
            id,
            client,
            sequence,
            base_revision: decimal(v, "base_revision")?,
            kind,
            payload: v["payload"].clone(),
            impact_ack: v["impact_ack"].as_str().map(str::to_owned),
        })
    }
}

pub(crate) struct Controller {
    pub session_id: String,
    pub model: Option<EditorSession>,
    pub registry: TargetRegistry,
    pub busy: Option<String>,
    pub writer_client: Option<String>,
    pub writer_epoch: u64,
    pub view_sequence: u64,
    pub diagnostics: Vec<Value>,
    pub recovery: Vec<Value>,
    pub recovery_only: bool,
    pub config: PathBuf,
    pub cwd: PathBuf,
    clients: BTreeMap<String, ClientLedger>,
    ledger: BTreeMap<String, LedgerEntry>,
    jobs: BTreeMap<String, Value>,
    expired_jobs: VecDeque<String>,
    confirmations: BTreeMap<String, Confirmation>,
    parse_running: bool,
    parse_pending: Option<Instant>,
    recovery_stamps: BTreeMap<String, (Stamp, String, String)>,
    #[cfg(test)]
    worker_gate: Option<Arc<std::sync::Barrier>>,
}
impl Controller {
    #[cfg(test)]
    pub fn new(config: PathBuf, cwd: PathBuf, registry: TargetRegistry) -> Result<Self> {
        Self::with_config(Some(config), cwd, registry)
    }
    pub fn with_config(
        config: Option<PathBuf>,
        cwd: PathBuf,
        mut registry: TargetRegistry,
    ) -> Result<Self> {
        let mut diagnostics = Vec::new();
        let (recovery, mut recovery_only) = match registry.pending_recoveries() {
            Ok(r) => {
                let pending = !r.is_empty();
                (r, pending)
            }
            Err(e) => {
                diagnostics.push(diagnostic("io", &e, 0));
                (vec![], true)
            }
        };
        let model = if recovery_only {
            None
        } else {
            match if let Some(path) = &config {
                load_project(path, &cwd).and_then(|project| {
                    registry.register_source(&project)?;
                    Ok(project)
                })
            } else {
                super::template::new_project("multibus", "Untitled", &cwd)
            } {
                Ok(project) => Some(EditorSession::new(project)),
                Err(e) => {
                    diagnostics.push(diagnostic("load", &e, 0));
                    None
                }
            }
        };
        // An invalid recovery record remains protected even when no valid record can be listed.
        recovery_only |= !recovery.is_empty();
        let config = model
            .as_ref()
            .map(|m| m.project.config.clone())
            .or(config)
            .unwrap_or_else(|| cwd.join("project.ini"));
        Ok(Self {
            session_id: random_id("session-")?,
            model,
            registry,
            busy: None,
            writer_client: None,
            writer_epoch: 0,
            view_sequence: 1,
            diagnostics,
            recovery,
            recovery_only,
            config,
            cwd,
            clients: BTreeMap::new(),
            ledger: BTreeMap::new(),
            jobs: BTreeMap::new(),
            expired_jobs: VecDeque::new(),
            confirmations: BTreeMap::new(),
            parse_running: false,
            parse_pending: None,
            recovery_stamps: BTreeMap::new(),
            #[cfg(test)]
            worker_gate: None,
        })
    }
    fn revision(&self) -> u64 {
        self.model.as_ref().map_or(0, |m| m.revision)
    }
    fn changed(&mut self) {
        self.view_sequence = self.view_sequence.saturating_add(1);
    }
    fn envelope(
        &self,
        status: u16,
        state: (bool, bool, bool),
        command: Option<&str>,
        job: Option<&str>,
        error: Option<&EditorError>,
    ) -> Value {
        let (accepted, terminal, consumed) = state;
        let mut v = json!({"schema_version":1,"session_id":self.session_id,"accepted":accepted,"terminal":terminal,"operation_status":status,"current_revision":self.revision().to_string(),"sequence_consumed":consumed,"view_sequence":self.view_sequence.to_string()});
        if let Some(id) = command {
            v["command_id"] = id.into();
        }
        if let Some(id) = job {
            v["job_id"] = id.into();
        }
        if let Some(e) = error {
            v["code"] = e.code.clone().into();
            v["message"] = e.message.clone().into();
        }
        v
    }
    fn reject(&self, e: EditorError, id: Option<&str>) -> (u16, Value) {
        (
            e.status,
            self.envelope(e.status, (false, true, false), id, None, Some(&e)),
        )
    }
    fn writer_check(&self, client: &str, epoch: u64) -> Result<()> {
        if !self.clients.contains_key(client)
            || self.writer_client.as_deref() != Some(client)
            || self.writer_epoch != epoch
        {
            return Err(EditorError::new(
                "E-EDITOR-WRITER",
                "This client does not hold the current writer lease",
                403,
            ));
        }
        Ok(())
    }
    fn idle(&self) -> Result<()> {
        if self.busy.is_some() || self.clients.values().any(|c| c.inflight.is_some()) {
            Err(EditorError::new(
                "E-EDITOR-BUSY",
                "An operation is in progress",
                409,
            ))
        } else {
            Ok(())
        }
    }
    fn writable(&self, kind: &str) -> Result<()> {
        if self.recovery_only && kind != "recover" {
            return Err(EditorError::new(
                "E-EDITOR-RECOVERY",
                "Complete or restore pending output recovery first",
                409,
            ));
        }
        Ok(())
    }
    /// Caller must authenticate the HTTP session header, Host, and Origin first.
    pub fn handle(
        shared: &Shared,
        method: &str,
        path: &str,
        query: &BTreeMap<String, String>,
        body: Option<Value>,
    ) -> (u16, Value) {
        if method == "GET" {
            let c = shared.lock().unwrap_or_else(|e| e.into_inner());
            if path == "/api/session" {
                return (200, super::view::project(&c, query));
            }
            if let Some(id) = path.strip_prefix("/api/commands/") {
                return c.ledger.get(id).map_or_else(
                    || {
                        c.reject(
                            EditorError::new("E-EDITOR-UNKNOWN", "Unknown command", 404),
                            Some(id),
                        )
                    },
                    |entry| (200, entry.response.clone()),
                );
            }
            if let Some(id) = path.strip_prefix("/api/jobs/") {
                return c.jobs.get(id).map_or_else(
                    || {
                        c.reject(
                            EditorError::new(
                                if c.expired_jobs.iter().any(|j| j == id) {
                                    "E-EDITOR-EXPIRED"
                                } else {
                                    "E-EDITOR-UNKNOWN"
                                },
                                "Job is unavailable",
                                if c.expired_jobs.iter().any(|j| j == id) {
                                    410
                                } else {
                                    404
                                },
                            ),
                            None,
                        )
                    },
                    |response| (200, response.clone()),
                );
            }
            return c.reject(
                EditorError::new("E-EDITOR-ROUTE", "Unknown API route", 404),
                None,
            );
        }
        if method != "POST" {
            return shared.lock().unwrap_or_else(|e| e.into_inner()).reject(
                EditorError::new("E-EDITOR-METHOD", "Method not allowed", 405),
                None,
            );
        }
        let Some(body) = body else {
            return shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reject(malformed("Missing JSON body"), None);
        };
        if serde_json::to_vec(&body).map_or(true, |b| b.len() > BODY_BYTES) {
            return shared.lock().unwrap_or_else(|e| e.into_inner()).reject(
                EditorError::new("E-EDITOR-BODY-LIMIT", "JSON body exceeds 64 MiB", 413),
                None,
            );
        }
        match path {
            "/api/writer" => shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .writer(&body),
            "/api/commands" => Self::command(shared, body),
            "/api/confirmations" => Self::confirmation(shared, &body),
            "/api/destinations" => Self::destination(shared, &body),
            _ => shared.lock().unwrap_or_else(|e| e.into_inner()).reject(
                EditorError::new("E-EDITOR-ROUTE", "Unknown API route", 404),
                None,
            ),
        }
    }
    fn writer(&mut self, body: &Value) -> (u16, Value) {
        let result = (|| {
            let action = field(body, "action")?;
            if action == "register" {
                fields(body, &["action"], &[])?;
                let id = random_id("client-")?;
                self.clients.insert(id.clone(), ClientLedger::default());
                if self.writer_client.is_none() {
                    self.idle()?;
                    self.writer_epoch = self
                        .writer_epoch
                        .checked_add(1)
                        .ok_or_else(|| malformed("Writer epoch exhausted"))?;
                    self.writer_client = Some(id.clone());
                    self.changed();
                }
                return Ok(
                    json!({"client_id":id,"writer_epoch":self.writer_epoch.to_string(),"next_sequence":"1","writer":self.writer_client.as_ref()==Some(&id)}),
                );
            }
            if action != "claim" {
                return Err(malformed("Unknown writer action"));
            }
            fields(body, &["action", "client_id", "expected_writer_epoch"], &[])?;
            let client = field(body, "client_id")?;
            let epoch = decimal(body, "expected_writer_epoch")?;
            if !self.clients.contains_key(client) || epoch != self.writer_epoch {
                return Err(EditorError::new(
                    "E-EDITOR-WRITER",
                    "Unknown client or outdated writer epoch",
                    403,
                ));
            }
            self.idle()?;
            self.writer_epoch = self
                .writer_epoch
                .checked_add(1)
                .ok_or_else(|| malformed("Writer epoch exhausted"))?;
            self.writer_client = Some(client.into());
            self.confirmations.clear();
            self.changed();
            Ok(
                json!({"client_id":client,"writer_epoch":self.writer_epoch.to_string(),"next_sequence":self.clients[client].high_watermark.saturating_add(1).to_string(),"writer":true}),
            )
        })();
        match result {
            Ok(v) => (200, v),
            Err(e) => self.reject(e, None),
        }
    }
    fn finish(
        &mut self,
        command: &Command,
        result: Result<()>,
        applied: bool,
        extra: Option<Value>,
    ) -> (u16, Value) {
        let status = result.as_ref().err().map_or(200, |e| e.status);
        self.changed();
        let job = self.ledger.get(&command.id).and_then(|e| e.job.clone());
        let mut response = self.envelope(
            status,
            (true, true, true),
            Some(&command.id),
            job.as_deref(),
            result.as_ref().err(),
        );
        response["next_sequence"] = command.sequence.saturating_add(1).to_string().into();
        if applied {
            response["applied_revision"] = self.revision().to_string().into();
        }
        if let Some(v) = extra {
            response["result"] = v;
        }
        if let Some(id) = &job {
            self.jobs.insert(id.clone(), response.clone());
        }
        if let Some(entry) = self.ledger.get_mut(&command.id) {
            entry.response = response.clone();
        }
        let client = self
            .clients
            .get_mut(&command.client)
            .expect("Accepted client exists");
        client.high_watermark = command.sequence;
        client.inflight = None;
        client.terminal.push_back(command.id.clone());
        while client.terminal.len() > LEDGER_LIMIT {
            if let Some(old) = client.terminal.pop_front() {
                if let Some(entry) = self.ledger.remove(&old) {
                    if let Some(id) = entry.job {
                        let pending_recovery = entry.response["result"]["recovery_id"]
                            .as_str()
                            .is_some_and(|recovery| {
                                self.recovery.iter().any(|r| r["id"] == recovery)
                            });
                        if !pending_recovery {
                            self.jobs.remove(&id);
                            self.expired_jobs.push_back(id);
                        }
                    }
                }
            }
        }
        while self.expired_jobs.len() > LEDGER_LIMIT {
            self.expired_jobs.pop_front();
        }
        (status, response)
    }
    fn consume_confirmation(
        &mut self,
        nonce: Option<&str>,
        kind: &str,
        payload: &Value,
        revision: u64,
    ) -> Result<Confirmation> {
        let nonce = nonce.ok_or_else(|| {
            EditorError::new(
                "E-EDITOR-CONFIRMATION",
                "Explicit confirmation is required",
                409,
            )
        })?;
        let value = self.confirmations.remove(nonce).ok_or_else(|| {
            EditorError::new(
                "E-EDITOR-CONFIRMATION",
                "Confirmation is unknown or already used",
                409,
            )
        })?;
        if value.epoch != self.writer_epoch
            || value.revision != revision
            || value.kind != kind
            || value.payload_digest != payload_digest(payload)
        {
            return Err(EditorError::new(
                "E-EDITOR-CONFIRMATION",
                "Confirmation no longer matches this operation",
                409,
            ));
        }
        Ok(value)
    }
    fn command(shared: &Shared, request: Value) -> (u16, Value) {
        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
        let command = match Command::parse(&request) {
            Ok(command) => command,
            Err(e) => return c.reject(e, request["command_id"].as_str()),
        };
        if request["session_id"] != c.session_id {
            return c.reject(
                EditorError::new("E-EDITOR-SESSION", "Session does not match", 403),
                Some(&command.id),
            );
        }
        if let Some(entry) = c.ledger.get(&command.id) {
            if entry.request != request {
                return c.reject(
                    EditorError::new(
                        "E-EDITOR-SEQUENCE",
                        "Command ID was reused with different contents",
                        409,
                    ),
                    Some(&command.id),
                );
            }
            return (
                entry.response["operation_status"].as_u64().unwrap_or(500) as u16,
                entry.response.clone(),
            );
        }
        let Some(client) = c.clients.get(&command.client) else {
            return c.reject(
                EditorError::new("E-EDITOR-WRITER", "Unregistered client", 403),
                Some(&command.id),
            );
        };
        if command.sequence <= client.high_watermark {
            return c.reject(
                EditorError::new(
                    "E-EDITOR-EXPIRED",
                    "Command response expired; refresh before issuing another operation",
                    410,
                ),
                Some(&command.id),
            );
        }
        if client.inflight.is_some()
            || client.high_watermark.checked_add(1) != Some(command.sequence)
            || command.sequence == u64::MAX
        {
            return c.reject(
                EditorError::new(
                    "E-EDITOR-SEQUENCE",
                    "Unexpected or in-flight client sequence",
                    409,
                ),
                Some(&command.id),
            );
        }
        if let Err(e) = c.writer_check(
            &command.client,
            decimal(&request, "writer_epoch").expect("Already checked"),
        ) {
            return c.reject(e, Some(&command.id));
        }
        if c.clients.values().filter(|l| l.inflight.is_some()).count() >= 256 {
            return c.reject(
                EditorError::new("E-EDITOR-BUSY", "Command queue is full", 503),
                Some(&command.id),
            );
        }
        c.clients.get_mut(&command.client).unwrap().inflight = Some(command.id.clone());
        let response = c.envelope(202, (true, false, false), Some(&command.id), None, None);
        c.ledger.insert(
            command.id.clone(),
            LedgerEntry {
                request,
                response,
                job: None,
            },
        );
        let preconditions = (|| {
            if c.busy.is_some() {
                return Err(EditorError::new(
                    "E-EDITOR-BUSY",
                    "An operation is in progress",
                    409,
                ));
            }
            c.writable(&command.kind)?;
            if command.base_revision != c.revision() {
                return Err(EditorError::new(
                    "E-EDITOR-REVISION",
                    "The model changed; refresh before applying this operation",
                    409,
                ));
            }
            if c.model.is_none()
                && !matches!(command.kind.as_str(), "reload" | "recover" | "new_project")
            {
                return Err(EditorError::new(
                    "E-EDITOR-NO-PROJECT",
                    "Reload a project before editing",
                    409,
                ));
            }
            Ok(())
        })();
        if let Err(e) = preconditions {
            return c.finish(&command, Err(e), false, None);
        }
        let mut authorized_digest = None;
        let mut replace_layout = false;
        let confirmation = (|| {
            if (graph_kind(&command.kind) || settings_kind(&command.kind))
                && command.kind != "set_layout"
            {
                c.consume_confirmation(
                    command.impact_ack.as_deref(),
                    &command.kind,
                    &command.payload,
                    command.base_revision,
                )?;
            }
            if command.kind == "reload" && c.model.as_ref().is_some_and(EditorSession::dirty) {
                c.consume_confirmation(
                    command.payload["discard_ack"].as_str(),
                    "reload",
                    &command.payload,
                    command.base_revision,
                )?;
            }
            if command.kind == "new_project" && c.model.is_some() {
                c.consume_confirmation(
                    command.payload["discard_ack"].as_str(),
                    "new_project",
                    &command.payload,
                    command.base_revision,
                )?;
            }
            if command.kind == "overwrite_project" {
                let confirm = c.consume_confirmation(
                    command.payload["overwrite_ack"].as_str(),
                    "overwrite_project",
                    &command.payload,
                    command.base_revision,
                )?;
                authorized_digest = confirm.plan_digest;
                if command.payload["replace_layout_ack"] == command.payload["overwrite_ack"] {
                    replace_layout = confirm.replace_layout;
                }
            }
            if let Some(nonce) = command.payload["replace_layout_ack"].as_str() {
                if !replace_layout {
                    let confirmation_kind = if c
                        .confirmations
                        .get(nonce)
                        .is_some_and(|v| v.kind == "replace_layout")
                    {
                        "replace_layout"
                    } else {
                        command.kind.as_str()
                    };
                    let confirm = c.consume_confirmation(
                        Some(nonce),
                        confirmation_kind,
                        &command.payload,
                        command.base_revision,
                    )?;
                    replace_layout = confirm.replace_layout;
                    if authorized_digest.is_some() && authorized_digest != confirm.plan_digest {
                        return Err(EditorError::new(
                            "E-EDITOR-CONFIRMATION",
                            "Layout and overwrite confirmations refer to different output plans",
                            409,
                        ));
                    }
                    if authorized_digest.is_none() {
                        authorized_digest = confirm.plan_digest;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = confirmation {
            return c.finish(&command, Err(e), false, None);
        }
        if matches!(
            command.kind.as_str(),
            "replace_source" | "undo" | "redo" | "set_layout"
        ) {
            let result = (|| {
                let model = c.model.as_mut().expect("Checked model");
                model.check_revision(command.base_revision)?;
                match command.kind.as_str() {
                    "replace_source" => {
                        let id = field(&command.payload, "file_id")?;
                        let file = model.project.files.get(id).ok_or_else(|| {
                            EditorError::new("E-EDITOR-PATCH", "Unknown source file", 422)
                        })?;
                        if file.hash != field(&command.payload, "expected_hash")? {
                            return Err(EditorError::new(
                                "E-EDITOR-PATCH",
                                "Source hash changed",
                                409,
                            ));
                        }
                        if file.roles.iter().any(|r| matches!(r, FileRole::Config)) {
                            if let Ok(header) = crate::input::inspect_config(
                                field(&command.payload, "text")?,
                                &model.project.config,
                                &model.project.cwd,
                            ) {
                                if header.roots != model.project.header.roots
                                    || header.workload != model.project.header.workload
                                    || header.model_config != model.project.header.model_config
                                {
                                    return Err(EditorError::new(
                                        "E-EDITOR-INPUT-PATH",
                                        "Input paths cannot be changed in the source editor; change the files externally and reload",
                                        422,
                                    ));
                                }
                            }
                        }
                        model.commit(
                            vec![(id.into(), field(&command.payload, "text")?.into())],
                            vec![],
                        )
                    }
                    "undo" => model.undo(),
                    "redo" => model.redo(),
                    _ => model.graph_command("set_layout", &command.payload),
                }
            })();
            let applied = result.as_ref().is_ok_and(|changed| *changed);
            let pending = c.model.as_ref().is_some_and(|m| m.analysis == "pending");
            let response = c.finish(&command, result.map(|_| ()), applied, None);
            if pending {
                Self::schedule_parse(shared, &mut c);
            }
            return response;
        }
        let job = match random_id("job-") {
            Ok(id) => id,
            Err(e) => return c.finish(&command, Err(e), false, None),
        };
        let stamp = c.model.as_ref().map(Stamp::of);
        let project = c.model.as_ref().map(|m| m.project.clone());
        let registry = c.registry.clone();
        let config = c.config.clone();
        let cwd = c.cwd.clone();
        c.busy = Some(command.kind.clone());
        c.changed();
        let response = c.envelope(
            202,
            (true, false, false),
            Some(&command.id),
            Some(&job),
            None,
        );
        let entry = c.ledger.get_mut(&command.id).unwrap();
        entry.job = Some(job.clone());
        entry.response = response.clone();
        c.jobs.insert(job.clone(), response.clone());
        let worker_shared = Arc::clone(shared);
        let worker_command = command.clone();
        let input_revision = stamp.as_ref().map_or(0, |s| s.input_revision);
        #[cfg(test)]
        let worker_gate = c.worker_gate.take();
        let spawn = std::thread::Builder::new()
            .name("ned-editor-operation".into())
            .spawn(move || {
                #[cfg(test)]
                if let Some(gate) = worker_gate {
                    gate.wait();
                }
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_operation(
                        &worker_command,
                        OperationInput {
                            project,
                            input_revision,
                            registry,
                            config,
                            cwd,
                            authorized_digest,
                            replace_layout,
                        },
                    )
                }));
                let mut c = worker_shared.lock().unwrap_or_else(|e| e.into_inner());
                c.complete_operation(&worker_command, stamp.as_ref(), outcome);
                if c.model.as_ref().is_some_and(|m| m.analysis == "pending") {
                    Self::schedule_parse(&worker_shared, &mut c);
                }
            });
        if let Err(e) = spawn {
            c.busy = None;
            return c.finish(
                &command,
                Err(EditorError::new("E-EDITOR-WORKER", e.to_string(), 500)),
                false,
                None,
            );
        }
        (202, response)
    }
    fn schedule_parse(shared: &Shared, c: &mut Self) {
        c.parse_pending = Some(Instant::now());
        if c.parse_running {
            return;
        }
        c.parse_running = true;
        let weak = Arc::downgrade(shared);
        let spawn = std::thread::Builder::new()
            .name("ned-editor-parse".into())
            .spawn(move || {
                loop {
                    let Some(shared) = weak.upgrade() else {
                        return;
                    };
                    let wait = {
                        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
                        match c.parse_pending {
                            Some(at) => Duration::from_millis(300).saturating_sub(at.elapsed()),
                            None => {
                                c.parse_running = false;
                                return;
                            }
                        }
                    };
                    if !wait.is_zero() {
                        drop(shared);
                        std::thread::sleep(wait);
                        continue;
                    }
                    let snapshot = {
                        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
                        // Input may have changed between the debounce check and this lock.
                        if c.parse_pending
                            .is_some_and(|at| at.elapsed() < Duration::from_millis(300))
                        {
                            continue;
                        }
                        c.parse_pending = None;
                        c.model
                            .as_ref()
                            .map(|m| (m.project.clone(), m.context_epoch))
                    };
                    if let Some((project, epoch)) = snapshot {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            parse_project(&project)
                        }));
                        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
                        match result {
                            Ok(batch) => {
                                if let Some(m) = &mut c.model {
                                    m.adopt_analysis(batch, epoch);
                                }
                            }
                            Err(_) => {
                                let rev = c.model.as_ref().map_or(0, |m| m.input_revision);
                                c.diagnostics.push(diagnostic(
                                    "syntax",
                                    &EditorError::new(
                                        "E-EDITOR-WORKER",
                                        "Parsing worker failed",
                                        500,
                                    ),
                                    rev,
                                ));
                            }
                        }
                        c.changed();
                    }
                }
            });
        if let Err(e) = spawn {
            c.parse_running = false;
            c.diagnostics.push(diagnostic(
                "syntax",
                &EditorError::new("E-EDITOR-WORKER", e.to_string(), 500),
                c.model.as_ref().map_or(0, |m| m.input_revision),
            ));
            c.changed();
        }
    }
    fn destination(shared: &Shared, body: &Value) -> (u16, Value) {
        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
        let precondition = (|| {
            fields(
                body,
                &[
                    "client_id",
                    "writer_epoch",
                    "export_root_id",
                    "relative_directory",
                ],
                &[],
            )?;
            text_fields(body, &["export_root_id", "relative_directory"])?;
            c.writer_check(field(body, "client_id")?, decimal(body, "writer_epoch")?)?;
            c.idle()?;
            c.writable("destination")?;
            if !c
                .registry
                .export_roots()
                .iter()
                .any(|r| r["id"] == body["export_root_id"])
            {
                return Err(EditorError::new(
                    "E-EDITOR-TARGET-DENIED",
                    "Unknown registered export root",
                    403,
                ));
            }
            Ok(())
        })();
        if let Err(e) = precondition {
            return c.reject(e, None);
        }
        let mut registry = c.registry.clone();
        c.busy = Some("destination".into());
        c.changed();
        drop(c);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            registry
                .register_destination(body["relative_directory"].as_str().expect("Checked string"))
        }));
        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
        c.busy = None;
        c.changed();
        match result {
            Ok(Ok(value)) => {
                c.registry = registry;
                (200, value)
            }
            Ok(Err(e)) => c.reject(e, None),
            Err(_) => c.reject(
                EditorError::new("E-EDITOR-WORKER", "Destination registration failed", 500),
                None,
            ),
        }
    }
    fn confirmation(shared: &Shared, body: &Value) -> (u16, Value) {
        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
        let precondition = (|| {
            fields(
                body,
                &[
                    "client_id",
                    "writer_epoch",
                    "base_revision",
                    "kind",
                    "payload",
                ],
                &[],
            )?;
            let client = field(body, "client_id")?;
            c.writer_check(client, decimal(body, "writer_epoch")?)?;
            c.idle()?;
            c.writable("confirmation")?;
            let revision = decimal(body, "base_revision")?;
            if revision != c.revision() {
                return Err(EditorError::new(
                    "E-EDITOR-REVISION",
                    "The model changed before confirmation",
                    409,
                ));
            }
            let kind = field(body, "kind")?;
            if !graph_kind(kind)
                && !settings_kind(kind)
                && !matches!(
                    kind,
                    "overwrite_project"
                        | "save_as_project"
                        | "replace_layout"
                        | "reload"
                        | "new_project"
                )
            {
                return Err(malformed("This command does not need confirmation"));
            }
            // Preview has no acknowledgement yet. Validate the same DTO after supplying a dummy nonce.
            let mut payload = body["payload"].clone();
            if kind == "overwrite_project" && payload.get("overwrite_ack").is_none() {
                fields(&payload, &["destination_id"], &["replace_layout_ack"])?;
                payload["overwrite_ack"] = "preview".into();
            }
            if kind == "replace_layout" {
                fields(&payload, &["destination_id"], &[])?;
                field(&payload, "destination_id")?;
            } else {
                validate_payload(kind, &payload)?;
            }
            if c.model.is_none() && !matches!(kind, "reload" | "new_project") {
                return Err(EditorError::new(
                    "E-EDITOR-NO-PROJECT",
                    "No loaded project to confirm",
                    409,
                ));
            }
            Ok((kind.to_string(), revision))
        })();
        let (kind, revision) = match precondition {
            Ok(v) => v,
            Err(e) => return c.reject(e, None),
        };
        if c.model.is_none() {
            let nonce = match random_id("confirmation-") {
                Ok(v) => v,
                Err(e) => return c.reject(e, None),
            };
            let epoch = c.writer_epoch;
            c.confirmations.insert(
                nonce.clone(),
                Confirmation {
                    epoch,
                    revision,
                    kind,
                    payload_digest: payload_digest(&body["payload"]),
                    plan_digest: None,
                    replace_layout: false,
                },
            );
            return (
                200,
                json!({"confirmation":nonce,"impacts":[format!("プロジェクトを読み込みます: {}",c.config.display())],"plan_digest":null,"requires_layout_confirmation":false}),
            );
        }
        let model = c.model.as_ref().unwrap();
        let project = model.project.clone();
        let stamp = Stamp::of(model);
        let epoch = c.writer_epoch;
        let registry = c.registry.clone();
        let payload = body["payload"].clone();
        c.busy = Some("confirmation".into());
        c.changed();
        drop(c);
        let preview = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<(Vec<String>, Option<String>, bool)> {
                if matches!(
                    kind.as_str(),
                    "overwrite_project" | "save_as_project" | "replace_layout"
                ) {
                    let target = field(&payload, "destination_id")?;
                    if kind != "replace_layout" {
                        check_target_kind(&registry, target, &kind)?;
                    }
                    let plan = build_plan(
                        &project,
                        stamp.revision,
                        stamp.input_revision,
                        target,
                        &registry,
                    )?;
                    let mut impacts = vec![format!(
                        "プロジェクト全体を保存します: {}",
                        plan.output_path.display()
                    )];
                    for file in project.files.values() {
                        impacts.push(format!("対象ファイル: {}", file.path.display()));
                    }
                    if plan.requires_layout_confirmation {
                        impacts.push("保護された不正な配置ファイルを置き換えます。配置の破棄を確認してください。".into());
                    }
                    Ok((
                        impacts,
                        Some(plan.digest),
                        plan.requires_layout_confirmation,
                    ))
                } else if settings_kind(&kind) {
                    let mut preview_model = super::model::EditorSession::new(project.clone());
                    preview_model
                        .adopt_analysis(parse_project(&project), preview_model.context_epoch);
                    preview_model.settings_command(&kind, &payload)?;
                    let mut impacts =
                        vec!["選択した実体の設定を変更します。共有型定義は変更しません。".into()];
                    for file in preview_model.project.files.values() {
                        if project
                            .files
                            .get(&file.id)
                            .is_none_or(|f| f.hash != file.hash)
                        {
                            impacts.push(format!("設定ファイル: {}", file.path.display()));
                        }
                    }
                    Ok((impacts, None, false))
                } else if kind == "new_project" {
                    // Validate before offering a destructive replacement confirmation.
                    super::template::new_project(
                        field(&payload, "template")?,
                        field(&payload, "project_name")?,
                        &project.cwd,
                    )?;
                    Ok((vec!["現在のプロジェクトと編集履歴を閉じ、テンプレートから新規作成します。保存先にはまだ書き込みません。".into()], None, false))
                } else if kind == "reload" {
                    Ok((
                        project
                            .files
                            .values()
                            .map(|f| {
                                format!("内部の変更を破棄し再読込します: {}", f.path.display())
                            })
                            .collect(),
                        None,
                        false,
                    ))
                } else {
                    // Build impact information from the common parser; do not infer type identity from a UI label.
                    let batch = parse_project(&project);
                    let key = payload
                        .get("parent_type")
                        .or_else(|| payload.get("type_key"))
                        .and_then(Value::as_str)
                        .or_else(|| {
                            payload
                                .get("element_key")
                                .or_else(|| payload.get("connection_key"))
                                .or_else(|| payload.get("parameter_key"))
                                .and_then(Value::as_str)
                                .map(|s| s.split("::").next().unwrap_or(s))
                        })
                        .ok_or_else(|| {
                            EditorError::new("E-EDITOR-UNMAPPED", "No declaration selected", 422)
                        })?;
                    let mut selected = None;
                    let mut declarations = Vec::new();
                    for (id, file) in batch.files {
                        if let Ok(parsed) = file.result {
                            for (ordinal, shape) in shapes(&parsed).into_iter().enumerate() {
                                if super::analysis::type_key(&id, ordinal, &shape.name) == key {
                                    selected = Some((id.clone(), shape.name.clone()));
                                }
                                declarations.push((id.clone(), shape));
                            }
                        }
                    }
                    let (id, name) = selected.ok_or_else(|| {
                        EditorError::new(
                            "E-EDITOR-UNMAPPED",
                            "Selected declaration is unavailable",
                            422,
                        )
                    })?;
                    let mut impacts = vec![format!(
                        "型定義 {name} を変更します: {}",
                        project.files[&id].path.display()
                    )];
                    if super::template::requires_multibus(
                        payload["type_name"].as_str().unwrap_or(""),
                    ) && project.header.profile.as_deref() != Some("can.cc.multibus.v1")
                    {
                        impacts.push("Gateway/Multibus型の追加に合わせ、Controller・Busの実装とINIをMultibusへ変更します。既存のパラメータ値・gate名は保持します。新しい経路設定ファイルを含む場合は別フォルダへ保存してください。".into());
                    }
                    for (file_id, shape) in declarations {
                        for (child, ty) in shape.children {
                            if ty == name || ty.rsplit('.').next() == Some(name.as_str()) {
                                impacts.push(format!(
                                    "共有定義の利用箇所: {}.{} ({})",
                                    shape.name,
                                    child,
                                    project.files[&file_id].path.display()
                                ));
                            }
                        }
                    }
                    Ok((impacts, None, false))
                }
            },
        ));
        let mut c = shared.lock().unwrap_or_else(|e| e.into_inner());
        c.busy = None;
        c.changed();
        if c.writer_epoch != epoch || c.model.as_ref().is_none_or(|m| !stamp.matches(m)) {
            return c.reject(
                EditorError::new(
                    "E-EDITOR-REVISION",
                    "Confirmation preview became stale",
                    409,
                ),
                None,
            );
        }
        let (impacts, plan_digest, replace_layout) = match preview {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return c.reject(e, None),
            Err(_) => {
                return c.reject(
                    EditorError::new("E-EDITOR-WORKER", "Confirmation preview failed", 500),
                    None,
                );
            }
        };
        let nonce = match random_id("confirmation-") {
            Ok(id) => id,
            Err(e) => return c.reject(e, None),
        };
        if c.confirmations.len() >= LEDGER_LIMIT {
            c.confirmations.clear();
        }
        c.confirmations.insert(
            nonce.clone(),
            Confirmation {
                epoch,
                revision,
                kind,
                payload_digest: payload_digest(&payload),
                plan_digest: plan_digest.clone(),
                replace_layout,
            },
        );
        (
            200,
            json!({"confirmation":nonce,"impacts":impacts,"plan_digest":plan_digest,"requires_layout_confirmation":replace_layout}),
        )
    }
    fn complete_operation(
        &mut self,
        command: &Command,
        stamp: Option<&Stamp>,
        outcome: std::thread::Result<Result<Work>>,
    ) {
        let mut applied = false;
        let mut extra = None;
        let origin = if command.kind == "reload" {
            "load"
        } else if matches!(
            command.kind.as_str(),
            "save_as_project" | "overwrite_project" | "recover"
        ) {
            "io"
        } else {
            "semantic"
        };
        self.diagnostics.retain(|d| d["origin"] != origin);
        let result = (|| {
            let work = match outcome {
                Ok(v) => v?,
                Err(_) => {
                    if matches!(
                        command.kind.as_str(),
                        "save_as_project" | "overwrite_project" | "recover"
                    ) {
                        self.recovery_only = true;
                    }
                    return Err(EditorError::new(
                        "E-EDITOR-WORKER",
                        "Operation worker failed",
                        500,
                    ));
                }
            };
            if self
                .model
                .as_ref()
                .zip(stamp)
                .is_some_and(|(m, stamp)| !stamp.matches(m))
                || (self.model.is_some() != stamp.is_some())
            {
                if matches!(work, Work::Saved { .. }) {
                    self.recovery_only = true;
                }
                return Err(EditorError::new(
                    "E-EDITOR-REVISION",
                    "Operation result no longer matches the model",
                    409,
                ));
            }
            match work {
                Work::Graph(batch) => {
                    if batch.snapshot_id != stamp.unwrap().snapshot {
                        return Err(EditorError::new(
                            "E-EDITOR-REVISION",
                            "Graph analysis snapshot changed",
                            409,
                        ));
                    }
                    let model = self.model.as_mut().expect("Graph requires model");
                    model.adopt_analysis(batch, stamp.unwrap().context_epoch);
                    model.check_revision(command.base_revision)?;
                    applied = if settings_kind(&command.kind) {
                        model.settings_command(&command.kind, &command.payload)?
                    } else {
                        model.graph_command(&command.kind, &command.payload)?
                    };
                }
                Work::Validated(batch, prepared) => {
                    if batch.snapshot_id != stamp.unwrap().snapshot {
                        return Err(EditorError::new(
                            "E-EDITOR-REVISION",
                            "Validation snapshot changed",
                            409,
                        ));
                    }
                    let model = self.model.as_mut().expect("Validation requires model");
                    model.adopt_analysis(batch, stamp.unwrap().context_epoch);
                    model.diagnostics.retain(|d| d["origin"] != "semantic");
                    match prepared {
                        Ok(p) => {
                            model.prepared = Some(Arc::new(p));
                            model.analysis = "ready".into();
                        }
                        Err(e) => {
                            model.prepared = None;
                            model.analysis =
                                if model.syntax.values().any(|s| s == "syntax_error") {
                                    "syntax_error"
                                } else {
                                    "semantic_error"
                                }
                                .into();
                            model.diagnostics.push(diagnostic(
                                "semantic",
                                &e,
                                model.input_revision,
                            ));
                            return Err(e);
                        }
                    }
                }
                Work::Reloaded(project, registry) => {
                    self.config = project.config.clone();
                    if let Some(m) = &mut self.model {
                        m.reload(project)?;
                    } else {
                        self.model = Some(EditorSession::new(project));
                    }
                    self.registry = registry;
                    self.diagnostics.retain(|d| d["origin"] != "load");
                    self.parse_pending = None;
                    self.confirmations.clear();
                    applied = true;
                }
                Work::New(project) => {
                    self.config = project.config.clone();
                    if let Some(m) = &mut self.model {
                        m.reload(project)?;
                        m.never_exported = true;
                        m.last_output = None;
                    } else {
                        self.model = Some(EditorSession::new(project));
                    }
                    self.diagnostics.retain(|d| d["origin"] != "load");
                    self.parse_pending = None;
                    self.confirmations.clear();
                    applied = true;
                }
                Work::Saved {
                    report,
                    registry,
                    pending,
                    expected,
                } => {
                    self.registry = registry;
                    let was_protected = self.recovery_only;
                    match pending {
                        Ok(r) => {
                            self.recovery = r;
                            self.recovery_only = !self.recovery.is_empty()
                                || (was_protected
                                    && !matches!(
                                        report.state,
                                        SaveOutcome::Complete | SaveOutcome::Restored
                                    ));
                        }
                        Err(e) => {
                            self.recovery_only = true;
                            self.diagnostics.push(diagnostic(
                                "io",
                                &e,
                                self.model.as_ref().map_or(0, |m| m.input_revision),
                            ));
                        }
                    }
                    extra = Some(report_value(&report));
                    if let Some((save_id, digest)) = expected {
                        let stamp = stamp.expect("Save requires loaded model");
                        if report.save_id != save_id
                            || report.plan_digest != digest
                            || report.revision != stamp.revision
                            || report.input_revision != stamp.input_revision
                            || report.snapshot_id != stamp.snapshot
                        {
                            self.recovery_only = true;
                            return Err(EditorError::new(
                                "E-EDITOR-SAVE",
                                "Output report did not match the authorized snapshot and plan",
                                500,
                            ));
                        }
                        if let Some(id) = &report.recovery_id {
                            self.recovery_stamps
                                .insert(id.clone(), (stamp.clone(), save_id, digest));
                        }
                        if report.state == SaveOutcome::Complete
                            && report.error.is_none()
                            && !self.recovery_only
                        {
                            let model = self.model.as_mut().unwrap();
                            if stamp.matches(model) {
                                model.mark_saved(report.output_path.to_string_lossy().into_owned());
                            }
                        }
                    } else if command.kind == "recover"
                        && report.state == SaveOutcome::Complete
                        && report.error.is_none()
                        && !self.recovery_only
                    {
                        if let Some((saved, save_id, digest)) = self
                            .recovery_stamps
                            .get(field(&command.payload, "recovery_id")?)
                        {
                            if report.save_id == *save_id
                                && report.plan_digest == *digest
                                && report.snapshot_id == saved.snapshot
                                && report.revision == saved.revision
                                && report.input_revision == saved.input_revision
                            {
                                if let Some(model) = &mut self.model {
                                    if saved.matches(model) {
                                        model.mark_saved(
                                            report.output_path.to_string_lossy().into_owned(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if report.state == SaveOutcome::RecoveryRequired {
                        self.recovery_only = true;
                    }
                    if matches!(report.state, SaveOutcome::Complete | SaveOutcome::Restored)
                        && report.error.is_none()
                    {
                        if command.kind == "recover" {
                            self.recovery_stamps
                                .remove(field(&command.payload, "recovery_id")?);
                        }
                    } else {
                        return Err(report.error.unwrap_or_else(|| {
                            EditorError::new(
                                "E-EDITOR-RECOVERY",
                                "Output operation is incomplete",
                                409,
                            )
                        }));
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = &result {
            // Validation normally records the exact diagnostic on the model.
            // Preserve failures that happened before that result was adopted.
            let entry = diagnostic(
                origin,
                e,
                self.model.as_ref().map_or(0, |m| m.input_revision),
            );
            if !self
                .model
                .as_ref()
                .is_some_and(|m| m.diagnostics.contains(&entry))
            {
                self.diagnostics.push(entry);
            }
        }
        self.busy = None;
        self.finish(command, result, applied, extra);
    }
}

enum Work {
    New(ProjectSnapshot),
    Graph(super::analysis::AnalysisBatch),
    Validated(
        super::analysis::AnalysisBatch,
        Result<crate::types::PreparedSimulation>,
    ),
    Reloaded(ProjectSnapshot, TargetRegistry),
    Saved {
        report: SaveReport,
        registry: TargetRegistry,
        pending: Result<Vec<Value>>,
        expected: Option<(String, String)>,
    },
}
fn check_target_kind(registry: &TargetRegistry, id: &str, kind: &str) -> Result<()> {
    let target = registry
        .targets()
        .into_iter()
        .find(|t| t["id"] == id)
        .ok_or_else(|| {
            EditorError::new(
                "E-EDITOR-TARGET-DENIED",
                "Unknown registered destination",
                403,
            )
        })?;
    let valid = if kind == "save_as_project" {
        target["kind"] == "new_project"
    } else {
        matches!(
            target["kind"].as_str(),
            Some("source_project" | "managed_export")
        )
    };
    if !valid {
        return Err(EditorError::new(
            "E-EDITOR-TARGET-DENIED",
            "Destination kind does not match the requested save operation",
            403,
        ));
    }
    Ok(())
}
struct OperationInput {
    project: Option<ProjectSnapshot>,
    input_revision: u64,
    registry: TargetRegistry,
    config: PathBuf,
    cwd: PathBuf,
    authorized_digest: Option<String>,
    replace_layout: bool,
}
fn run_operation(command: &Command, input: OperationInput) -> Result<Work> {
    let OperationInput {
        project,
        input_revision,
        mut registry,
        config,
        cwd,
        authorized_digest,
        replace_layout,
    } = input;
    if graph_kind(&command.kind) || settings_kind(&command.kind) {
        return Ok(Work::Graph(parse_project(
            project.as_ref().expect("Checked model"),
        )));
    }
    match command.kind.as_str() {
        "new_project" => Ok(Work::New(super::template::new_project(
            field(&command.payload, "template")?,
            field(&command.payload, "project_name")?,
            &cwd,
        )?)),
        "validate" => {
            let project = project.as_ref().expect("Checked model");
            Ok(Work::Validated(
                parse_project(project),
                prepare(project).map_err(|d| EditorError::common("E-EDITOR-VALIDATION", d)),
            ))
        }
        "reload" => {
            if project
                .as_ref()
                .is_some_and(|p| matches!(p.origin, super::input::ProjectOrigin::New { .. }))
            {
                return Err(EditorError::new(
                    "E-EDITOR-NO-ORIGIN",
                    "A new project has no disk input; use New or Save As",
                    409,
                ));
            }
            let project = load_project(&config, &cwd)?;
            registry.register_source(&project)?;
            Ok(Work::Reloaded(project, registry))
        }
        "save_as_project" | "overwrite_project" => {
            let project = project.as_ref().expect("Checked model");
            let target = field(&command.payload, "destination_id")?;
            check_target_kind(&registry, target, &command.kind)?;
            let plan = build_plan(
                project,
                command.base_revision,
                input_revision,
                target,
                &registry,
            )?;
            let expected = Some((plan.id.clone(), plan.digest.clone()));
            if authorized_digest
                .as_ref()
                .is_some_and(|digest| digest != &plan.digest)
            {
                return Err(EditorError::new(
                    "E-EDITOR-CONFIRMATION",
                    "Authorized output plan changed after preview",
                    409,
                ));
            }
            let report = save_plan(
                plan,
                &mut registry,
                authorized_digest.as_deref(),
                replace_layout,
            );
            let pending = registry.pending_recoveries();
            Ok(Work::Saved {
                report,
                registry,
                pending,
                expected,
            })
        }
        "recover" => {
            let report = recover(
                &mut registry,
                field(&command.payload, "recovery_id")?,
                field(&command.payload, "action")?,
            );
            let pending = registry.pending_recoveries();
            Ok(Work::Saved {
                report,
                registry,
                pending,
                expected: None,
            })
        }
        _ => Err(malformed("Unsupported operation")),
    }
}
fn report_value(r: &SaveReport) -> Value {
    json!({"save_id":r.save_id,"revision":r.revision.to_string(),"input_revision":r.input_revision.to_string(),"snapshot_id":r.snapshot_id,"plan_digest":r.plan_digest,"state":r.state,"files":r.files,"recovery_id":r.recovery_id,"output_path":r.output_path})
}

fn diagnostic(origin: &str, e: &EditorError, revision: u64) -> Value {
    json!({"origin":origin,"code":e.diagnostic.as_ref().map_or(e.code.as_str(),|d|d.code.as_str()),"severity":"error","message":e.message,"input_revision":revision.to_string()})
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
