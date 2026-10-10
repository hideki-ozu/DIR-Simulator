//! Strict preparation for the memory/IPC transaction profile.
use super::ned::{self, Declaration, ModelRules, TypedValue, Values};
use super::{JsonDocument, Result, error, identifier, object, required_string, unsigned};
use crate::types::memory_ipc::*;
fn violation(rule: &str, target: &str, message: impl Into<String>) -> crate::types::Diagnostic {
    let mut d = error(message);
    d.details = Some(serde_json::json!({"rule":rule,"target":target}));
    d
}
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
fn implementation(s: &str) -> Option<Kind> {
    Some(match s {
        "dir.memory.DdrV1" => Kind::Ddr,
        "dir.memory.SramV1" => Kind::Sram,
        "dir.ipc.SharedV1" => Kind::Shared,
        "dir.dma.EngineV1" => Kind::Dma,
        "dir.ipc.MailboxV1" => Kind::Mailbox,
        _ => return None,
    })
}
struct Rules;
impl ModelRules for Rules {
    fn validate_schema(&self, d: &Declaration) -> Result<()> {
        if !d.simple() || d.implementation().and_then(implementation).is_none() {
            return Err(error("unknown memory/IPC implementation"));
        }
        d.require_parameters(&[])?;
        if !d.gates().is_empty() {
            return Err(error("memory/IPC resources require empty gates"));
        }
        Ok(())
    }
    fn validate_value(&self, _: &Declaration, _: &str, _: &TypedValue) -> Result<()> {
        Ok(())
    }
    fn validate_instance(&self, _: &str, _: &Declaration, _: &Values) -> Result<()> {
        Ok(())
    }
    fn payload(&self, _: &Declaration, _: &str) -> Option<&'static str> {
        None
    }
}
pub(super) fn validate_parameter_literal(d: &Declaration, name: &str, _: &str) -> Result<()> {
    Err(error(format!(
        "memory/IPC {} has no parameter {name}",
        d.name()
    )))
}
pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(PreparedMemoryIpc, Vec<String>, usize)> {
    let r = ned::resolve(types, network, overrides, &Rules)?;
    let c = r.resolve_channels(channels, &Rules)?;
    let placements = r
        .instances()
        .filter_map(|(id, d)| {
            d.implementation()
                .and_then(implementation)
                .map(|kind| (id.to_string(), kind))
        })
        .collect();
    Ok((
        PreparedMemoryIpc {
            placements,
            ..Default::default()
        },
        r.module_paths(),
        c.len(),
    ))
}
fn decode(s: &str) -> Result<JsonDocument<'_>> {
    JsonDocument::parse(s)
}
fn exact<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>> {
    let o = object(v, keys, "memory/IPC")?;
    for key in keys {
        if !o.contains_key(*key) {
            return Err(error(format!("missing field {key}")));
        }
    }
    Ok(o)
}
fn num(o: &Map<String, Value>, key: &str, min: u64, max: u64) -> Result<u64> {
    super::json_diagnostics::field_check(o, key, || {
        o[key]
            .as_u64()
            .filter(|n| *n >= min && *n <= max)
            .ok_or_else(|| error(format!("{key} must be integer {min}..{max}")))
    })
}
fn array<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>> {
    super::json_diagnostics::field_check(o, key, || {
        o[key]
            .as_array()
            .ok_or_else(|| error(format!("{key} must be array")))
    })
}
fn bytes(s: &str) -> Result<Vec<u8>> {
    if s.is_empty()
        || s.len() % 2 != 0
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error("hex must be nonempty even lowercase hexadecimal"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| error(e.to_string())))
        .collect()
}
fn actors(o: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    super::json_diagnostics::field_check(o, key, || {
        let mut a = Vec::new();
        for v in array(o, key)? {
            let _object_scope = super::json_diagnostics::scoped_value(v);
            let s = v.as_str().ok_or_else(|| error("actor must be string"))?;
            if !identifier(s) || !s.is_ascii() || a.iter().any(|a| a == s) {
                return Err(error("invalid or duplicate actor"));
            }
            a.push(s.into());
        }
        if a.is_empty() {
            return Err(error("actor array must be nonempty"));
        }
        a.sort();
        Ok(a)
    })
}
pub(super) fn configure(content: &str, p: &mut PreparedMemoryIpc, profile: &str) -> Result<()> {
    let document = decode(content)?;
    let _scope = document.enter("model_config_invalid");
    let v = &document.value;
    let o = exact(
        v,
        &[
            "schema_version",
            "profile",
            "ddr",
            "sram",
            "shared",
            "dma",
            "mailboxes",
        ],
    )?;
    if num(o, "schema_version", 1, 1)? != 1 || required_string(o, "profile")? != profile {
        return Err(error("memory/IPC profile mismatch"));
    }
    let mut resources = Vec::new();
    let mut seen = BTreeSet::new();
    for (key, kind) in [
        ("ddr", Kind::Ddr),
        ("sram", Kind::Sram),
        ("shared", Kind::Shared),
        ("dma", Kind::Dma),
        ("mailboxes", Kind::Mailbox),
    ] {
        let keys: &[&str] = match kind {
            Kind::Ddr => &[
                "node",
                "size",
                "initial",
                "queue_capacity",
                "banks",
                "row_bytes",
                "rows_per_bank",
                "width_bytes",
                "open_ps",
                "close_ps",
                "column_ps",
                "beat_ps",
                "refresh_interval_ps",
                "refresh_ps",
                "fault_ranges",
            ],
            Kind::Sram => &[
                "node",
                "size",
                "initial",
                "queue_capacity",
                "ports",
                "read_ps",
                "write_ps",
                "fault_ranges",
            ],
            Kind::Shared => &[
                "node",
                "slots",
                "slot_bytes",
                "queue_capacity",
                "publish_ps",
                "consume_ps",
                "producers",
                "consumers",
            ],
            Kind::Dma => &[
                "node",
                "queue_capacity",
                "chunk_bytes",
                "setup_ps",
                "notify_ps",
            ],
            Kind::Mailbox => &[
                "node",
                "capacity",
                "payload_bytes",
                "queue_capacity",
                "service_ps",
                "notify_ps",
                "senders",
                "receivers",
            ],
        };
        for v in array(o, key)? {
            let _object_scope = super::json_diagnostics::scoped_value(v);
            let c = exact(v, keys)?;
            let node = required_string(c, "node")?.to_string();
            if p.placements.get(&node) != Some(&kind) || !seen.insert(node.clone()) {
                return Err(error("model-config node/type does not match placement"));
            }
            let mut r = Resource {
                node,
                kind,
                queue_capacity: num(c, "queue_capacity", 0, 1024)? as usize,
                size: 0,
                initial: Vec::new(),
                faults: Vec::new(),
                banks: 0,
                row_bytes: 0,
                width_bytes: 0,
                ports: 1,
                slots: 0,
                payload_bytes: 0,
                capacity: 0,
                chunk_bytes: 0,
                times: BTreeMap::new(),
                producers: Vec::new(),
                consumers: Vec::new(),
            };
            for key in keys.iter().filter(|k| k.ends_with("_ps")) {
                r.times.insert(
                    (*key).into(),
                    super::json_diagnostics::field_result(
                        c,
                        key,
                        unsigned(required_string(c, key)?, true),
                    )?,
                );
            }
            if matches!(kind, Kind::Ddr | Kind::Sram) {
                r.size = num(c, "size", 1, 65536)? as usize;
                r.initial = vec![0; r.size];
                let mut occupied = vec![false; r.size];
                for init in array(c, "initial")? {
                    let _object_scope = super::json_diagnostics::scoped_value(init);
                    let i = exact(init, &["offset", "hex"])?;
                    let at = num(i, "offset", 0, u64::MAX)?;
                    let b = super::json_diagnostics::field_result(
                        i,
                        "hex",
                        bytes(required_string(i, "hex")?),
                    )?;
                    if at as u128 + b.len() as u128 > r.size as u128 {
                        return Err(error("initial range out of memory"));
                    }
                    let at = at as usize;
                    if occupied[at..at + b.len()].iter().any(|b| *b) {
                        return Err(error("overlapping initial ranges"));
                    }
                    occupied[at..at + b.len()].fill(true);
                    r.initial[at..at + b.len()].copy_from_slice(&b);
                }
                for range in array(c, "fault_ranges")? {
                    let _object_scope = super::json_diagnostics::scoped_value(range);
                    let f = exact(range, &["offset", "length"])?;
                    let at = num(f, "offset", 0, u64::MAX)?;
                    let len = num(f, "length", 1, u64::MAX)?;
                    if at as u128 + len as u128 > r.size as u128
                        || r.faults.iter().any(|&(a, l)| at < a + l && a < at + len)
                    {
                        return Err(error("invalid or overlapping fault ranges"));
                    }
                    r.faults.push((at, len));
                }
            }
            match kind {
                Kind::Ddr => {
                    r.banks = num(c, "banks", 1, 16)? as usize;
                    r.row_bytes = num(c, "row_bytes", 1, 65536)?;
                    r.width_bytes = num(c, "width_bytes", 1, 65536)?;
                    let rows = num(c, "rows_per_bank", 1, 65536)?;
                    if r.banks as u128 * r.row_bytes as u128 * rows as u128 != r.size as u128
                        || r.row_bytes % r.width_bytes != 0
                    {
                        return Err(violation("geometry_size", &r.node, "invalid DDR geometry"));
                    }
                    if r.times["refresh_ps"] >= r.times["refresh_interval_ps"] {
                        return Err(violation(
                            "refresh_interval",
                            &r.node,
                            "refresh_ps must be less than refresh_interval_ps",
                        ));
                    }
                }
                Kind::Sram => {
                    r.ports = num(c, "ports", 1, 16)
                        .map_err(|d| violation("port_count", &r.node, d.message))?
                        as usize
                }
                Kind::Shared => {
                    r.slots = num(c, "slots", 1, 1024)
                        .map_err(|d| violation("slot_count", &r.node, d.message))?
                        as usize;
                    r.payload_bytes = num(c, "slot_bytes", 1, 65536)? as usize;
                    r.producers = actors(c, "producers")?;
                    r.consumers = actors(c, "consumers")?;
                    if r.slots * r.payload_bytes > 65536 {
                        return Err(error("shared capacity exceeds 65536"));
                    }
                }
                Kind::Dma => {
                    r.chunk_bytes = num(c, "chunk_bytes", 1, 65536)
                        .map_err(|d| violation("chunk_size", &r.node, d.message))?
                }
                Kind::Mailbox => {
                    r.capacity = num(c, "capacity", 0, 1024)
                        .map_err(|d| violation("mailbox_capacity", &r.node, d.message))?
                        as usize;
                    r.payload_bytes = num(c, "payload_bytes", 1, 65536)? as usize;
                    r.producers = actors(c, "senders")?;
                    r.consumers = actors(c, "receivers")?;
                    if r.capacity * r.payload_bytes > 65536 {
                        return Err(error("mailbox capacity exceeds 65536"));
                    }
                }
            }
            resources.push(r);
        }
    }
    if seen.len() != p.placements.len() || seen.is_empty() {
        return Err(error("model-config must cover every resource exactly once"));
    }
    resources.sort_by(|a, b| a.node.cmp(&b.node));
    p.resources = resources;
    Ok(())
}
pub(super) fn workload(content: &str, p: &mut PreparedMemoryIpc, _: &str) -> Result<()> {
    let document = decode(content)?;
    let _scope = document.enter("workload_invalid");
    let v = &document.value;
    let o = exact(v, &["schema_version", "generators"])?;
    num(o, "schema_version", 2, 2)?;
    let mut ids = BTreeSet::new();
    let mut gs = Vec::new();
    for v in array(o, "generators")? {
        let _object_scope = super::json_diagnostics::scoped_value(v);
        let g = exact(v, &["id", "kind", "node", "times", "request"])?;
        let id = required_string(g, "id")?;
        if !identifier(id)
            || !ids.insert(id.to_string())
            || required_string(g, "kind")? != "memory-ipc.explicit.v1"
        {
            return Err(error("invalid generator id/kind"));
        }
        let node = p
            .resources
            .iter()
            .position(|r| r.node == required_string(g, "node").unwrap_or(""))
            .ok_or_else(|| error("unknown generator node"))?;
        let r = &p.resources[node];
        let req = &g["request"];
        let op = req
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| error("missing request op"))?;
        let keys: &[&str] = match (r.kind, op) {
            (Kind::Ddr | Kind::Sram, "read") => &["op", "address", "length"],
            (Kind::Ddr | Kind::Sram, "write") => &["op", "address", "length", "hex"],
            (Kind::Shared, "publish") | (Kind::Mailbox, "send") => &["op", "actor", "hex"],
            (Kind::Shared, "consume") | (Kind::Mailbox, "receive") => &["op", "actor"],
            (Kind::Dma, "copy") => &["op", "src", "dst", "src_address", "dst_address", "length"],
            _ => return Err(error("request op does not match node kind")),
        };
        let q = exact(req, keys)?;
        let mut request = Request {
            op: op.into(),
            ..Default::default()
        };
        if q.contains_key("length") {
            request.length = Some(num(q, "length", 1, 65536)?);
        }
        if q.contains_key("address") {
            request.address = Some(num(q, "address", 0, u64::MAX)?);
        }
        if q.contains_key("actor") {
            let a = required_string(q, "actor")?;
            if !a.is_ascii() || !identifier(a) {
                return Err(error("invalid actor syntax"));
            }
            request.actor = Some(a.into());
        }
        if q.contains_key("hex") {
            let b =
                super::json_diagnostics::field_result(q, "hex", bytes(required_string(q, "hex")?))?;
            if request.length.is_some_and(|n| n as usize != b.len())
                || matches!(r.kind, Kind::Shared | Kind::Mailbox) && b.len() > r.payload_bytes
            {
                return Err(error("request payload length mismatch"));
            }
            request.bytes = Some(b);
        }
        if r.kind == Kind::Dma {
            for key in ["src", "dst"] {
                let n = p
                    .resources
                    .iter()
                    .position(|r| {
                        r.node == required_string(q, key).unwrap_or("")
                            && matches!(r.kind, Kind::Ddr | Kind::Sram)
                    })
                    .ok_or_else(|| error("DMA references must be configured memories"))?;
                if key == "src" {
                    request.src = Some(n);
                } else {
                    request.dst = Some(n);
                }
            }
            request.src_address = Some(num(q, "src_address", 0, u64::MAX)?);
            request.dst_address = Some(num(q, "dst_address", 0, u64::MAX)?);
        }
        let times = array(g, "times")?
            .iter()
            .map(|v| {
                super::json_diagnostics::value_check(v, || {
                    v.as_str()
                        .ok_or_else(|| error("times must contain strings"))
                        .and_then(super::parse_time)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if times.windows(2).any(|w| w[0] > w[1]) {
            return Err(error("times must be nondecreasing"));
        }
        gs.push(Generator {
            id: id.into(),
            node,
            times,
            request,
        });
    }
    gs.sort_by(|a, b| a.id.cmp(&b.id));
    p.generators = gs;
    Ok(())
}
