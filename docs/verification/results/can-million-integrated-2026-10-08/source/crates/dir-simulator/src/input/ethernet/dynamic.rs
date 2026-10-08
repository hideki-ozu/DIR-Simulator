//! Strict schema-4 dynamic policy preparation; every scheduled control is checked.
use crate::types::{
    Diagnostic,
    ethernet::{PreparedEthernet, dynamic::*},
};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};
type Result<T> = std::result::Result<T, Diagnostic>;
fn err(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(message).with_target(path)
}
fn obj<'a>(v: &'a Value, fields: &[&str], path: &str) -> Result<&'a Map<String, Value>> {
    let o = v.as_object().ok_or_else(|| err(path, "expected object"))?;
    if o.len() != fields.len()
        || fields.iter().any(|f| !o.contains_key(*f))
        || o.keys().any(|f| !fields.contains(&f.as_str()))
    {
        return Err(err(
            path,
            format!("required exact fields: {}", fields.join(", ")),
        ));
    }
    Ok(o)
}
fn s<'a>(o: &'a Map<String, Value>, field: &str, path: &str) -> Result<&'a str> {
    o[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(&format!("{path}.{field}"), "expected nonempty string"))
}
fn d(o: &Map<String, Value>, field: &str, positive: bool, path: &str) -> Result<u64> {
    let s = s(o, field, path)?;
    if !s.bytes().all(|b| b.is_ascii_digit()) || s.len() > 1 && s.starts_with('0') {
        return Err(err(
            &format!("{path}.{field}"),
            "expected canonical u64 decimal string",
        ));
    }
    let n = s.parse::<u64>().map_err(|_| err(path, "u64 overflow"))?;
    if positive && n == 0 {
        return Err(err(path, format!("{field} must be positive")));
    }
    Ok(n)
}
fn n(o: &Map<String, Value>, field: &str, path: &str) -> Result<usize> {
    let n = o[field]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| err(path, format!("{field} must be positive JSON integer")))?;
    usize::try_from(n).map_err(|_| err(path, "integer exceeds addressable range"))
}
fn arr<'a>(o: &'a Map<String, Value>, field: &str, path: &str) -> Result<&'a Vec<Value>> {
    o[field]
        .as_array()
        .ok_or_else(|| err(path, format!("{field} must be array")))
}
fn vid(o: &Map<String, Value>, path: &str) -> Result<u16> {
    o["vid"]
        .as_u64()
        .filter(|v| (1..=4094).contains(v))
        .map(|v| v as u16)
        .ok_or_else(|| err(path, "VID must be integer 1..4094"))
}
fn boolean(o: &Map<String, Value>, field: &str, path: &str) -> Result<bool> {
    o[field]
        .as_bool()
        .ok_or_else(|| err(path, format!("{field} must be boolean")))
}
fn family(o: &Map<String, Value>, path: &str) -> Result<IpFamily> {
    match s(o, "family", path)? {
        "ipv4" => Ok(IpFamily::Ipv4),
        "ipv6" => Ok(IpFamily::Ipv6),
        _ => Err(err(path, "unknown IP family")),
    }
}
fn ip(text: &str, f: IpFamily, multicast: bool, path: &str) -> Result<IpAddr> {
    let a = text
        .parse::<IpAddr>()
        .map_err(|_| err(path, "invalid IP address"))?;
    if matches!(
        (f, a),
        (IpFamily::Ipv4, IpAddr::V4(_)) | (IpFamily::Ipv6, IpAddr::V6(_))
    ) && a.is_multicast() == multicast
        && !a.is_unspecified()
        && !matches!(a,IpAddr::V4(v) if v.is_broadcast())
    {
        Ok(a)
    } else {
        Err(err(path, "IP family or unicast/multicast class mismatch"))
    }
}
fn expiry(o: &Map<String, Value>, at: u64, path: &str) -> Result<u64> {
    at.checked_add(d(o, "lifetime_ps", true, path)?)
        .ok_or_else(|| err(path, "at_ps+lifetime_ps overflow"))
}
fn port_owner<'a>(base: &'a PreparedEthernet, port: &str, path: &str) -> Result<&'a str> {
    base.port_policies
        .iter()
        .find(|p| p.port == port)
        .map(|p| base.devices[p.device].id.as_str())
        .ok_or_else(|| err(path, "unknown output port"))
}
fn member_port(
    base: &PreparedEthernet,
    registrable: &BTreeSet<RegistrationKey>,
    switch: &str,
    port: &str,
    vid: u16,
    path: &str,
) -> Result<()> {
    if port_owner(base, port, path)? != switch
        || !base
            .devices
            .iter()
            .any(|d| d.id == switch && d.kind == "switch")
    {
        return Err(err(path, "port does not belong to referenced Switch"));
    }
    if !base
        .port_policies
        .iter()
        .any(|p| p.port == port && p.vlans.contains_key(&vid))
        && !registrable.contains(&RegistrationKey {
            port: port.into(),
            vid,
        })
    {
        return Err(err(path, "VID is neither static nor registrable on port"));
    }
    Ok(())
}
pub(crate) fn prepare(
    config: &Value,
    workload: &Value,
    base: &PreparedEthernet,
) -> Result<PreparedDynamicEthernet> {
    let c = obj(
        config,
        &[
            "mac_age_ps",
            "convergence_ps",
            "bridges",
            "links",
            "registrable",
            "limits",
        ],
        "dynamic",
    )?;
    let mac_age_ps = d(c, "mac_age_ps", true, "dynamic")?;
    let convergence_ps = d(c, "convergence_ps", false, "dynamic")?;
    let l = obj(
        &c["limits"],
        &[
            "mac_entries",
            "membership_entries",
            "sources_per_entry",
            "registrations",
            "control_events",
            "pending_timers",
            "visits_per_frame",
        ],
        "dynamic.limits",
    )?;
    let limits = DynamicLimits {
        mac_entries: n(l, "mac_entries", "limits")?,
        membership_entries: n(l, "membership_entries", "limits")?,
        sources_per_entry: n(l, "sources_per_entry", "limits")?,
        registrations: n(l, "registrations", "limits")?,
        control_events: n(l, "control_events", "limits")?,
        pending_timers: n(l, "pending_timers", "limits")?,
        visits_per_frame: u64::try_from(n(l, "visits_per_frame", "limits")?)
            .map_err(|_| err("limits", "visit range"))?,
    };
    let mut bridges: BTreeMap<String, u64> = BTreeMap::new();
    let mut bridge_ids = BTreeSet::new();
    for (i, row) in arr(c, "bridges", "dynamic")?.iter().enumerate() {
        let p = format!("dynamic.bridges[{i}]");
        let o = obj(row, &["instance", "bridge_id"], &p)?;
        let instance = s(o, "instance", &p)?;
        let id = d(o, "bridge_id", false, &p)?;
        if !base
            .devices
            .iter()
            .any(|d| d.id == instance && d.kind == "switch")
            || bridges.insert(instance.into(), id).is_some()
            || !bridge_ids.insert(id)
        {
            return Err(err(&p, "unknown Switch or duplicate bridge ID/instance"));
        }
    }
    if bridges.len() != base.devices.iter().filter(|d| d.kind == "switch").count() {
        return Err(err("dynamic.bridges", "all Switch bridges are required"));
    }
    let mut links = Vec::new();
    let mut ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    for (i, row) in arr(c, "links", "dynamic")?.iter().enumerate() {
        let p = format!("dynamic.links[{i}]");
        let o = obj(row, &["id", "ports", "up", "cost"], &p)?;
        let id = s(o, "id", &p)?;
        let ps = arr(o, "ports", &p)?;
        if ps.len() != 2 {
            return Err(err(&p, "link needs exactly two output ports"));
        }
        let a = ps[0].as_str().ok_or_else(|| err(&p, "invalid link port"))?;
        let b = ps[1].as_str().ok_or_else(|| err(&p, "invalid link port"))?;
        port_owner(base, a, &p)?;
        port_owner(base, b, &p)?;
        if !base.directions.iter().any(|d| {
            d.from_port == a
                && base
                    .port_policies
                    .iter()
                    .any(|q| q.port == b && q.ingress == d.to_port)
        }) || !base.directions.iter().any(|d| {
            d.from_port == b
                && base
                    .port_policies
                    .iter()
                    .any(|q| q.port == a && q.ingress == d.to_port)
        }) || !ids.insert(id.to_owned())
            || !covered.insert(a.to_owned())
            || !covered.insert(b.to_owned())
        {
            return Err(err(&p, "unknown, repeated, or nonphysical link pair"));
        }
        links.push(DynamicLink {
            id: id.into(),
            ports: [a.into(), b.into()],
            up: boolean(o, "up", &p)?,
            cost: d(o, "cost", true, &p)?,
        });
    }
    if covered.len() != base.directions.len() {
        return Err(err(
            "dynamic.links",
            "every physical link must be listed exactly once",
        ));
    }
    links.sort_by(|a, b| a.id.cmp(&b.id));
    // Bound all possible shortest path additions, including graphs enabled by future controls.
    let total_cost = links
        .iter()
        .try_fold(0u64, |sum, l| sum.checked_add(l.cost))
        .ok_or_else(|| err("dynamic.links", "tree cost overflow"))?;
    let _ = total_cost;
    let mut registrable = BTreeSet::new();
    for (i, row) in arr(c, "registrable", "dynamic")?.iter().enumerate() {
        let p = format!("dynamic.registrable[{i}]");
        let o = obj(row, &["port", "vid", "tagged"], &p)?;
        let port = s(o, "port", &p)?;
        port_owner(base, port, &p)?;
        let v = vid(o, &p)?;
        if !boolean(o, "tagged", &p)?
            || base
                .port_policies
                .iter()
                .any(|q| q.port == port && q.vlans.contains_key(&v))
            || !registrable.insert(RegistrationKey {
                port: port.into(),
                vid: v,
            })
        {
            return Err(err(&p, "registrable must be unique dynamic tagged VID"));
        }
    }
    let w = obj(
        workload,
        &["schema_version", "generators", "controls"],
        "workload",
    )?;
    if w["schema_version"].as_u64() != Some(4) {
        return Err(err(
            "workload.schema_version",
            "dynamic workload requires schema 4",
        ));
    }
    let controls_raw = arr(w, "controls", "workload")?;
    if controls_raw.len() > limits.control_events {
        return Err(err("workload.controls", "control_events capacity exceeded"));
    }
    let mut controls = Vec::new();
    let mut control_ids = BTreeSet::new();
    for (i, row) in controls_raw.iter().enumerate() {
        let p = format!("workload.controls[{i}]");
        let o = row
            .as_object()
            .ok_or_else(|| err(&p, "expected control object"))?;
        let kind = o
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| err(&p, "missing control kind"))?;
        let specific: &[&str] = match kind {
            "membership_set" => &[
                "switch",
                "port",
                "vid",
                "family",
                "group",
                "mode",
                "sources",
                "lifetime_ps",
            ],
            "membership_leave" => &["switch", "port", "vid", "family", "group"],
            "router_set" => &["switch", "port", "vid", "family", "lifetime_ps"],
            "router_leave" => &["switch", "port", "vid", "family"],
            "vlan_register" => &["port", "vid", "lifetime_ps"],
            "vlan_unregister" => &["port", "vid"],
            "link_set" => &["link", "up"],
            _ => return Err(err(&p, "unknown control kind")),
        };
        let fields: Vec<_> = ["id", "at_ps", "kind"]
            .into_iter()
            .chain(specific.iter().copied())
            .collect();
        let o = obj(row, &fields, &p)?;
        let id = s(o, "id", &p)?;
        if !control_ids.insert(id.to_owned()) {
            return Err(err(&p, "duplicate control ID"));
        }
        let at = d(o, "at_ps", false, &p)?;
        let op = match kind {
            "link_set" => {
                let link = s(o, "link", &p)?;
                if !ids.contains(link) {
                    return Err(err(&p, "unknown link"));
                }
                at.checked_add(convergence_ps)
                    .ok_or_else(|| err(&p, "at_ps+convergence_ps overflow"))?;
                ControlOp::LinkSet {
                    link: link.into(),
                    up: boolean(o, "up", &p)?,
                }
            }
            "vlan_register" | "vlan_unregister" => {
                let key = RegistrationKey {
                    port: s(o, "port", &p)?.into(),
                    vid: vid(o, &p)?,
                };
                if !registrable.contains(&key) {
                    return Err(err(&p, "VID registration not authorized or is static"));
                }
                if kind == "vlan_register" {
                    ControlOp::VlanRegister {
                        key,
                        expires_at: expiry(o, at, &p)?,
                    }
                } else {
                    ControlOp::VlanUnregister { key }
                }
            }
            _ => {
                let switch = s(o, "switch", &p)?;
                let port = s(o, "port", &p)?;
                let v = vid(o, &p)?;
                member_port(base, &registrable, switch, port, v, &p)?;
                let f = family(o, &p)?;
                if kind.starts_with("membership") {
                    let key = MembershipKey {
                        switch: switch.into(),
                        vid: v,
                        family: f,
                        group: ip(s(o, "group", &p)?, f, true, &p)?,
                        port: port.into(),
                    };
                    if kind == "membership_leave" {
                        ControlOp::MembershipLeave { key }
                    } else {
                        let mode = match s(o, "mode", &p)? {
                            "include" => FilterMode::Include,
                            "exclude" => FilterMode::Exclude,
                            _ => return Err(err(&p, "unknown membership mode")),
                        };
                        let mut sources = BTreeSet::new();
                        let raw = arr(o, "sources", &p)?;
                        if raw.len() > limits.sources_per_entry {
                            return Err(err(&p, "sources_per_entry capacity exceeded"));
                        }
                        for v in raw {
                            let a = ip(
                                v.as_str()
                                    .ok_or_else(|| err(&p, "source must be IP string"))?,
                                f,
                                false,
                                &p,
                            )?;
                            if !sources.insert(a) {
                                return Err(err(&p, "duplicate normalized source"));
                            }
                        }
                        ControlOp::MembershipSet {
                            key,
                            mode,
                            sources,
                            expires_at: expiry(o, at, &p)?,
                        }
                    }
                } else {
                    let key = RouterKey {
                        switch: switch.into(),
                        vid: v,
                        family: f,
                        port: port.into(),
                    };
                    if kind == "router_set" {
                        ControlOp::RouterSet {
                            key,
                            expires_at: expiry(o, at, &p)?,
                        }
                    } else {
                        ControlOp::RouterLeave { key }
                    }
                }
            }
        };
        controls.push(DynamicControl {
            id: id.into(),
            at_ps: at,
            input_index: i,
            op,
        });
    }
    controls
        .sort_by(|a, b| (a.at_ps, a.op.subphase(), &a.id).cmp(&(b.at_ps, b.op.subphase(), &b.id)));
    let mut generator_ip = BTreeMap::new();
    let mut flows = BTreeMap::new();
    for (i, g) in arr(w, "generators", "workload")?.iter().enumerate() {
        let p = format!("workload.generators[{i}].frame.ip_multicast");
        let id = g
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| err(&p, "generator ID missing"))?;
        let frame = g
            .get("frame")
            .and_then(Value::as_object)
            .ok_or_else(|| err(&p, "frame missing"))?;
        let value = frame
            .get("ip_multicast")
            .ok_or_else(|| err(&p, "mandatory nullable ip_multicast missing"))?;
        let metadata = if value.is_null() {
            None
        } else {
            let o = obj(value, &["family", "source", "group"], &p)?;
            let f = family(o, &p)?;
            let source = ip(s(o, "source", &p)?, f, false, &p)?;
            let group = ip(s(o, "group", &p)?, f, true, &p)?;
            let (expected_mac, ether_type) = match group {
                IpAddr::V4(a) => {
                    let b = a.octets();
                    (
                        format!("01:00:5e:{:02x}:{:02x}:{:02x}", b[1] & 127, b[2], b[3]),
                        2048,
                    )
                }
                IpAddr::V6(a) => {
                    let b = a.octets();
                    (
                        format!(
                            "33:33:{:02x}:{:02x}:{:02x}:{:02x}",
                            b[12], b[13], b[14], b[15]
                        ),
                        34525,
                    )
                }
            };
            if frame
                .get("dst_mac")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
                != Some(expected_mac)
                || frame.get("ether_type").and_then(Value::as_u64) != Some(ether_type)
            {
                return Err(err(&p, "IP multicast MAC mapping or EtherType mismatch"));
            }
            Some(IpMulticast {
                family: f,
                source,
                group,
            })
        };
        if generator_ip.insert(id.into(), metadata.clone()).is_some() {
            return Err(err(&p, "duplicate generator ID"));
        }
        if let Some(flow) = g.get("flow_id").and_then(Value::as_str) {
            if let Some(old) = flows.insert(flow.to_owned(), metadata.clone()) {
                if old != metadata {
                    return Err(err(&p, "IP metadata differs within flow"));
                }
            }
        }
    }
    let mut vids: BTreeSet<_> = base
        .port_policies
        .iter()
        .flat_map(|p| p.vlans.keys().copied())
        .collect();
    vids.extend(registrable.iter().map(|r| r.vid));
    let macs: Vec<_> = base.devices.iter().filter_map(|d| d.mac.as_ref()).collect();
    let candidates = bridges
        .len()
        .checked_mul(vids.len())
        .and_then(|n| n.checked_mul(macs.len()))
        .ok_or_else(|| err("dynamic", "MAC generation ledger size overflow"))?;
    // Include owned strings and both prepared-key and runtime-generation tree nodes.
    let mut ledger_bytes = 0usize;
    for switch in bridges.keys() {
        for mac in &macs {
            let entry = std::mem::size_of::<MacKey>()
                .checked_add(128)
                .and_then(|n| n.checked_add(switch.len()))
                .and_then(|n| n.checked_add(mac.len()))
                .and_then(|n| n.checked_mul(2))
                .and_then(|n| n.checked_mul(vids.len()))
                .ok_or_else(|| err("dynamic", "MAC generation ledger byte size overflow"))?;
            ledger_bytes = ledger_bytes
                .checked_add(entry)
                .ok_or_else(|| err("dynamic", "MAC generation ledger byte size overflow"))?;
        }
    }
    if candidates > 1_000_000 || controls.len() > 1_000_000 || ledger_bytes > 256 * 1024 * 1024 {
        return Err(err("dynamic", "generation ledger memory limit exceeded"));
    }
    let mut mac_keys = BTreeSet::new();
    for switch in bridges.keys() {
        for v in &vids {
            for mac in &macs {
                mac_keys.insert(MacKey {
                    switch: switch.clone(),
                    vid: *v,
                    mac: (*mac).clone(),
                });
            }
        }
    }
    Ok(PreparedDynamicEthernet {
        mac_age_ps,
        convergence_ps,
        bridges,
        links,
        registrable,
        limits,
        controls,
        generator_ip,
        mac_keys,
    })
}
