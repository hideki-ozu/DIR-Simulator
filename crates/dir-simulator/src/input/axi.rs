//! Strict AXI profile preparation, including all future workload entries.
use super::ned::{self, Declaration, ModelRules};
use super::{Result, StrictJson, error, identifier, object, quantity, required_string};
use crate::types::axi::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
struct Rules;
const CHANNELS: [&str; 5] = ["aw", "w", "b", "ar", "r"];
fn gates(manager: bool) -> BTreeMap<String, bool> {
    CHANNELS
        .into_iter()
        .map(|c| (c.into(), matches!(c, "aw" | "w" | "ar") == manager))
        .collect()
}
impl ModelRules for Rules {
    fn validate_value(&self, d: &Declaration, name: &str, _value: &ned::TypedValue) -> Result<()> {
        if d.implementation() == Some("dir.link.FixedDelay") && name == "delay" {
            Ok(())
        } else {
            Err(error("AXI NED parameters are empty"))
        }
    }
    fn validate_schema(&self, d: &Declaration) -> Result<()> {
        d.require_parameters(&[])?;
        match d.implementation() {
            Some("dir.axi.Manager") if d.simple() && *d.gates() == gates(true) => Ok(()),
            Some("dir.axi.Ram") if d.simple() && *d.gates() == gates(false) => Ok(()),
            Some("dir.axi.Interconnect") if d.simple() => {
                let mut expected = gates(true);
                let mut suffixes = BTreeSet::new();
                for key in d.gates().keys() {
                    if let Some((channel, suffix)) = key.split_once('_') {
                        if CHANNELS.contains(&channel) && !suffix.is_empty() {
                            suffixes.insert(suffix);
                        } else {
                            return Err(error("invalid AXI Interconnect gate"));
                        }
                    }
                }
                if suffixes.is_empty() {
                    return Err(error("AXI Interconnect requires Manager gate groups"));
                }
                for suffix in suffixes {
                    for (c, output) in gates(false) {
                        expected.insert(format!("{c}_{suffix}"), output);
                    }
                }
                if *d.gates() != expected {
                    return Err(error("incomplete AXI Interconnect gate groups"));
                }
                Ok(())
            }
            _ => Err(error("unknown or wrong-kind AXI implementation/gates")),
        }
    }
    fn payload(&self, _d: &Declaration, gate: &str) -> Option<&'static str> {
        match gate.split('_').next()? {
            "aw" => Some("axi4.transaction.v1.Aw"),
            "w" => Some("axi4.transaction.v1.W"),
            "b" => Some("axi4.transaction.v1.B"),
            "ar" => Some("axi4.transaction.v1.Ar"),
            "r" => Some("axi4.transaction.v1.R"),
            _ => None,
        }
    }
}
pub(super) fn validate_parameter_literal(d: &Declaration, name: &str, value: &str) -> Result<()> {
    if d.implementation() == Some("dir.link.FixedDelay")
        && name == "delay"
        && quantity(value, "s")? == 0
    {
        Ok(())
    } else {
        Err(error(
            "AXI devices have no NED parameters; FixedDelay must be zero",
        ))
    }
}
pub(super) fn resolve(
    types: &BTreeMap<String, Declaration>,
    network: &str,
    overrides: &BTreeMap<String, String>,
    channels: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(PreparedAxi, Vec<String>, usize)> {
    let r = ned::resolve(types, network, overrides, &Rules)?;
    let c = r.resolve_channels(channels, &Rules)?;
    let instances = |class| {
        r.instances()
            .filter(|(_, d)| d.implementation() == Some(class))
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
    };
    let managers = instances("dir.axi.Manager");
    let buses = instances("dir.axi.Interconnect");
    let rams = instances("dir.axi.Ram");
    if managers.is_empty() || buses.len() != 1 || rams.len() != 1 {
        return Err(error(
            "AXI requires Manager(s), one Interconnect and one Ram",
        ));
    }
    let bus = &buses[0];
    let ram = &rams[0];
    let mut suffixes = BTreeSet::new();
    for manager in &managers {
        let aw = r.trace(&format!("{manager}.aw"))?;
        let prefix = format!("{bus}.aw_");
        let suffix = aw
            .end
            .strip_prefix(&prefix)
            .ok_or_else(|| error("AXI Manager AW must terminate at Interconnect"))?;
        if suffix.is_empty() || !suffixes.insert(suffix.to_string()) {
            return Err(error("AXI Manager suffix is not unique"));
        }
        for channel in CHANNELS {
            let (source, target) = if matches!(channel, "aw" | "w" | "ar") {
                (
                    format!("{manager}.{channel}"),
                    format!("{bus}.{channel}_{suffix}"),
                )
            } else {
                (
                    format!("{bus}.{channel}_{suffix}"),
                    format!("{manager}.{channel}"),
                )
            };
            let path = r.trace(&source)?;
            if path.end != target || path.delay(&c)? != 0 {
                return Err(error(
                    "AXI Manager five paths must share a suffix with zero delay",
                ));
            }
        }
    }
    if r.declaration(bus).gates().len() != 5 * (managers.len() + 1) {
        return Err(error("AXI Interconnect has unpaired gates"));
    }
    for channel in CHANNELS {
        let (source, target) = if matches!(channel, "aw" | "w" | "ar") {
            (format!("{bus}.{channel}"), format!("{ram}.{channel}"))
        } else {
            (format!("{ram}.{channel}"), format!("{bus}.{channel}"))
        };
        let path = r.trace(&source)?;
        if path.end != target || path.delay(&c)? != 0 {
            return Err(error("AXI Ram five paths require zero delay"));
        }
    }
    Ok((
        PreparedAxi {
            managers: managers
                .into_iter()
                .map(|id| AxiManager {
                    id,
                    max_outstanding: 1,
                    b_ready: "1".into(),
                    r_ready: "1".into(),
                })
                .collect(),
            interconnect: bus.clone(),
            ram: AxiRam {
                id: ram.clone(),
                base: 0,
                size: 4,
                initial: vec![0; 4],
                aw_ready: "1".into(),
                w_ready: "1".into(),
                ar_ready: "1".into(),
                read_latency_cycles: 1,
                write_response_cycles: 1,
                error_ranges: vec![],
            },
            clock_period_ps: 1,
            generators: vec![],
        },
        r.module_paths(),
        c.len(),
    ))
}
fn decode(content: &str) -> Result<Value> {
    let StrictJson(v) = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|e| error(e.to_string()))?;
    Ok(v)
}
fn exact<'a>(v: &'a Value, keys: &[&str], target: &str) -> Result<&'a Map<String, Value>> {
    let o = object(v, keys, target)?;
    if o.len() != keys.len() {
        return Err(error(format!("missing {target} field")));
    }
    Ok(o)
}
fn integer(o: &Map<String, Value>, key: &str, min: u64, max: u64) -> Result<u64> {
    o.get(key)
        .and_then(Value::as_u64)
        .filter(|n| (*n >= min) && (*n <= max))
        .ok_or_else(|| error(format!("{key} requires integer {min}..{max}")))
}
fn array<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a Vec<Value>> {
    o.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| error(format!("{key} requires array")))
}
fn pattern(o: &Map<String, Value>, key: &str) -> Result<String> {
    let s = required_string(o, key)?;
    if !(1..=1024).contains(&s.len())
        || !s.bytes().all(|c| matches!(c, b'0' | b'1'))
        || !s.contains('1')
    {
        return Err(error(format!("invalid AXI READY pattern {key}")));
    }
    Ok(s.into())
}
fn hex(s: &str) -> Result<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error("AXI hex requires nonempty even hexadecimal"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| error("invalid AXI hex")))
        .collect()
}
pub(super) fn configure(content: &str, p: &mut PreparedAxi, profile: &str) -> Result<()> {
    let v = decode(content)?;
    let o = exact(
        &v,
        &[
            "schema_version",
            "profile",
            "clock_period",
            "managers",
            "interconnect",
            "ram",
        ],
        "AXI model-config",
    )?;
    if integer(o, "schema_version", 1, 1)? != 1 || required_string(o, "profile")? != profile {
        return Err(error("AXI profile mismatch"));
    }
    let period = quantity(required_string(o, "clock_period")?, "s")?;
    if period == 0 {
        return Err(error("AXI clock must be positive"));
    }
    if required_string(o, "interconnect")? != p.interconnect {
        return Err(error("unknown AXI Interconnect node"));
    }
    let mut managers = p.managers.clone();
    let mut seen = BTreeSet::new();
    for value in array(o, "managers")? {
        let m = exact(
            value,
            &["node", "max_outstanding", "b_ready", "r_ready"],
            "AXI Manager",
        )?;
        let id = required_string(m, "node")?;
        let manager = managers
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| error("unknown AXI Manager node"))?;
        if !seen.insert(id) {
            return Err(error("duplicate AXI Manager node"));
        }
        manager.max_outstanding = integer(m, "max_outstanding", 1, 256)?;
        manager.b_ready = pattern(m, "b_ready")?;
        manager.r_ready = pattern(m, "r_ready")?;
    }
    if seen.len() != managers.len() {
        return Err(error("AXI Manager config must cover every Manager"));
    }
    let r = exact(
        &o["ram"],
        &[
            "node",
            "base",
            "size",
            "initial",
            "aw_ready",
            "w_ready",
            "ar_ready",
            "read_latency_cycles",
            "write_response_cycles",
            "error_ranges",
        ],
        "AXI Ram",
    )?;
    if required_string(r, "node")? != p.ram.id {
        return Err(error("unknown AXI Ram node"));
    }
    let base = integer(r, "base", 0, (1u64 << 32) - 4)?;
    let size = integer(r, "size", 4, 65536)?;
    if base % 4 != 0 || size % 4 != 0 || base + size > 1u64 << 32 {
        return Err(error("AXI Ram alignment/address range"));
    }
    let mut initial = vec![0u8; size as usize];
    let mut used = vec![false; size as usize];
    for value in array(r, "initial")? {
        let init = exact(value, &["offset", "data"], "AXI initial")?;
        let offset = integer(init, "offset", 0, size)? as usize;
        let bytes = hex(required_string(init, "data")?)?;
        let end = offset
            .checked_add(bytes.len())
            .filter(|&end| end <= size as usize)
            .ok_or_else(|| error("AXI initial range exceeds Ram"))?;
        if used[offset..end].iter().any(|b| *b) {
            return Err(error("overlapping AXI initial ranges"));
        }
        used[offset..end].fill(true);
        initial[offset..end].copy_from_slice(&bytes);
    }
    let mut errors = Vec::new();
    for value in array(r, "error_ranges")? {
        let e = exact(value, &["start", "end", "access"], "AXI error range")?;
        let start = integer(e, "start", base, base + size)?;
        let end = integer(e, "end", base, base + size)?;
        let access = required_string(e, "access")?;
        if start >= end || !matches!(access, "read" | "write" | "both") {
            return Err(error("invalid AXI error range"));
        }
        errors.push(AxiErrorRange {
            start,
            end,
            access: access.into(),
        });
    }
    errors.sort_by_key(|e| e.start);
    if errors.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(error("overlapping AXI error ranges"));
    }
    let ram = AxiRam {
        id: p.ram.id.clone(),
        base,
        size,
        initial,
        aw_ready: pattern(r, "aw_ready")?,
        w_ready: pattern(r, "w_ready")?,
        ar_ready: pattern(r, "ar_ready")?,
        read_latency_cycles: integer(r, "read_latency_cycles", 1, 65535)?,
        write_response_cycles: integer(r, "write_response_cycles", 1, 65535)?,
        error_ranges: errors,
    };
    p.managers = managers;
    p.ram = ram;
    p.clock_period_ps = period;
    Ok(())
}
pub(super) fn workload(content: &str, p: &mut PreparedAxi, _profile: &str) -> Result<()> {
    let v = decode(content)?;
    let o = exact(&v, &["schema_version", "generators"], "AXI workload")?;
    integer(o, "schema_version", 2, 2)?;
    let mut ids = BTreeSet::new();
    let mut generators = vec![];
    for value in array(o, "generators")? {
        let g = exact(
            value,
            &["id", "kind", "node", "times", "transaction"],
            "AXI generator",
        )?;
        let id = required_string(g, "id")?;
        if !identifier(id)
            || !ids.insert(id.to_string())
            || required_string(g, "kind")? != "axi.explicit.v1"
        {
            return Err(error("invalid AXI generator ID/kind"));
        }
        let source = p
            .managers
            .iter()
            .position(|m| m.id == required_string(g, "node").unwrap_or(""))
            .ok_or_else(|| error("unknown AXI generator Manager"))?;
        let times = array(g, "times")?
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| error("AXI times must be time strings"))
                    .and_then(|s| quantity(s, "s"))
            })
            .collect::<Result<Vec<_>>>()?;
        if times.windows(2).any(|w| w[0] > w[1]) {
            return Err(error("AXI times must be nondecreasing"));
        }
        let tv = &g["transaction"];
        let operation = tv
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| error("missing AXI operation"))?;
        let keys = match operation {
            "read" => vec!["operation", "address", "beats"],
            "write" => vec![
                "operation",
                "address",
                "beats",
                "write_data",
                "write_strobes",
            ],
            _ => return Err(error("invalid AXI operation")),
        };
        let t = exact(tv, &keys, "AXI transaction")?;
        let address = integer(t, "address", 0, u32::MAX as u64)?;
        let beats = integer(t, "beats", 1, 256)?;
        let end = address + 4 * beats;
        if address % 4 != 0 || end > 1u64 << 32 || address / 4096 != (end - 1) / 4096 {
            return Err(error("AXI aligned INCR burst violates address/4KiB bounds"));
        }
        let (mut data, mut strobes) = (vec![], vec![]);
        if operation == "write" {
            for v in array(t, "write_data")? {
                let s = v
                    .as_str()
                    .ok_or_else(|| error("AXI write_data requires strings"))?;
                if s.len() != 8 {
                    return Err(error("AXI write_data must have eight hex digits"));
                }
                hex(s)?;
                data.push(s.to_ascii_lowercase());
            }
            for v in array(t, "write_strobes")? {
                strobes.push(
                    v.as_u64()
                        .filter(|n| *n <= 15)
                        .ok_or_else(|| error("AXI strobe requires integer 0..15"))?
                        as u8,
                );
            }
            if data.len() != beats as usize || strobes.len() != beats as usize {
                return Err(error("AXI data/strobes length must equal beats"));
            }
        }
        generators.push(AxiGenerator {
            id: id.into(),
            source,
            times_ps: times,
            transaction: AxiTransaction {
                operation: operation.into(),
                address,
                beats,
                write_data: data,
                write_strobes: strobes,
            },
        });
    }
    generators.sort_by(|a, b| a.id.cmp(&b.id));
    p.generators = generators;
    Ok(())
}
