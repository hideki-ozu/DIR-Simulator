//! Strict wire wrapper and per-protocol preparation, independent of central dispatch.
use crate::runtime::network::provider_l2::{PortRole, ProviderTopology};
use crate::types::{Diagnostic, ethernet::PreparedEthernet, wire::*};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, Diagnostic>;
fn err(s: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(s)
}
fn object<'a>(
    v: &'a Value,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>> {
    let o = v.as_object().ok_or_else(|| err("wire object required"))?;
    for k in required {
        if !o.contains_key(*k) {
            return Err(err(format!("wire missing {k}")));
        }
    }
    for (k, v) in o {
        if v.is_null() || !required.contains(&k.as_str()) && !optional.contains(&k.as_str()) {
            return Err(err(format!("wire unknown/null {k}")));
        }
    }
    Ok(o)
}
fn string<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a str> {
    o.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(format!("wire {k} requires nonempty string")))
}
fn num(o: &Map<String, Value>, k: &str, default: Option<u64>, min: u64, max: u64) -> Result<u64> {
    let n = match o.get(k) {
        None => default,
        Some(v) => v.as_u64(),
    }
    .filter(|v| *v >= min && *v <= max)
    .ok_or_else(|| err(format!("wire {k} outside {min}..{max}")))?;
    Ok(n)
}
fn boolean(o: &Map<String, Value>, k: &str, default: bool) -> Result<bool> {
    match o.get(k) {
        None => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| err(format!("wire {k} requires bool"))),
    }
}
fn decimal(s: &str) -> Result<u64> {
    if s.is_empty() || s.len() > 1 && s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err("wire time/seed requires canonical u64 decimal"));
    }
    s.parse().map_err(|_| err("wire decimal overflow"))
}
fn ps(o: &Map<String, Value>, k: &str, default: u64, zero: bool) -> Result<u64> {
    let n = match o.get(k) {
        None => default,
        Some(v) => decimal(v.as_str().ok_or_else(|| err("wire ps requires string"))?)?,
    };
    if n == 0 && !zero {
        Err(err("wire timer must be positive"))
    } else {
        Ok(n)
    }
}
fn array<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a Vec<Value>> {
    o.get(k)
        .and_then(Value::as_array)
        .ok_or_else(|| err(format!("wire {k} requires array")))
}
fn optional_array<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a [Value]> {
    match o.get(k) {
        None => Ok(&[]),
        Some(v) => v
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| err(format!("wire {k} requires array"))),
    }
}
fn mac(s: &str) -> Result<[u8; 6]> {
    if s.len() != 17
        || s.bytes().enumerate().any(|(i, b)| {
            if i % 3 == 2 {
                b != b':'
            } else {
                !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b)
            }
        })
    {
        return Err(err("wire MAC requires canonical colon lowercase hex"));
    }
    let mut out = [0; 6];
    for (i, x) in s.split(':').enumerate() {
        out[i] = u8::from_str_radix(x, 16).map_err(|_| err("wire invalid MAC"))?;
    }
    Ok(out)
}
fn unicast(s: &str) -> Result<[u8; 6]> {
    let m = mac(s)?;
    if m == [0; 6] || m[0] & 1 != 0 {
        return Err(err("wire source MAC must be nonzero unicast"));
    }
    Ok(m)
}
fn bytes(s: &str) -> Result<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err("wire bytes_hex requires nonempty even ASCII hex"));
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|b| {
            u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).map_err(|_| err("wire hex"))
        })
        .collect()
}
fn serialization(bytes: u64, rate: u64) -> Result<u64> {
    if rate == 0 {
        return Err(err("wire bitrate must be positive"));
    }
    let v = (bytes as u128 * 8 * 1_000_000_000_000u128).div_ceil(rate as u128);
    u64::try_from(v).map_err(|_| err("wire duration overflow"))
}
fn component_binding(
    component: &str,
    provider: &ProviderTopology,
    eth: &PreparedEthernet,
) -> Result<ComponentTreeBinding> {
    let mut attachments = BTreeMap::new();
    let mut devices = BTreeSet::new();
    let mut domain = None;
    let mut customer_edges = 0;
    for (name, port) in provider
        .ports
        .iter()
        .filter(|(_, p)| p.component == component)
    {
        let kind = if port.role.is_customer() {
            TreeDomain::Customer
        } else {
            TreeDomain::Provider
        };
        if domain.replace(kind).is_some_and(|old| old != kind) {
            return Err(err("component mixes customer and provider roles"));
        }
        let attachment = match port.role {
            PortRole::CustomerEdge | PortRole::ProviderNetwork => {
                let (direction, link) = eth
                    .directions
                    .iter()
                    .enumerate()
                    .find(|(_, d)| d.from_port == *name)
                    .ok_or_else(|| err("component physical port lacks direction"))?;
                devices.insert(eth.devices[link.source].id.clone());
                if port.role == PortRole::CustomerEdge {
                    customer_edges += 1;
                }
                let ingress = eth
                    .port_policies
                    .iter()
                    .find(|p| p.port == *name)
                    .map(|p| p.ingress.clone())
                    .ok_or_else(|| err("component physical port lacks policy"))?;
                PortAttachment::Physical {
                    output: name.clone(),
                    ingress,
                    direction,
                }
            }
            PortRole::ProviderEdge | PortRole::CustomerNetwork => {
                let (peer, iss_id) = provider
                    .mappings
                    .iter()
                    .find_map(|m| {
                        if m.pep == *name {
                            Some((&m.cnp, format!("{}:{}", m.pep, m.cnp)))
                        } else if m.cnp == *name {
                            Some((&m.pep, format!("{}:{}", m.pep, m.cnp)))
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| err("component internal port lacks service mapping"))?;
                let peer_port = &provider.ports[peer];
                PortAttachment::Internal {
                    iss_id,
                    peer: ComponentPortKey {
                        component: peer_port.component.clone(),
                        port: peer.clone(),
                    },
                }
            }
        };
        attachments.insert(name.clone(), attachment);
    }
    if attachments.is_empty() || devices.len() != 1 {
        return Err(err("component requires ports on one physical device"));
    }
    let domain = domain.ok_or_else(|| err("component domain missing"))?;
    if domain == TreeDomain::Customer && customer_edges != 1 {
        return Err(err("protocol-enabled C component requires exactly one CEP"));
    }
    let mut service_cardinality = BTreeMap::new();
    if domain == TreeDomain::Customer {
        for map in provider
            .mappings
            .iter()
            .filter(|m| m.c_component == component)
        {
            // A service can span several S-components. Count interfaces across
            // its connected provider network, rather than only this device.
            let mut reached = BTreeSet::from([map.s_component.clone()]);
            loop {
                let previous = reached.len();
                for link in &eth.directions {
                    let Some(from) = provider.ports.get(&link.from_port) else {
                        continue;
                    };
                    let Some(to) = eth
                        .port_policies
                        .iter()
                        .find(|p| p.ingress == link.to_port)
                        .and_then(|p| provider.ports.get(&p.port))
                    else {
                        continue;
                    };
                    if from.role == PortRole::ProviderNetwork
                        && to.role == PortRole::ProviderNetwork
                        && from.member_vids.contains(&map.s_vid)
                        && to.member_vids.contains(&map.s_vid)
                        && reached.contains(&from.component)
                    {
                        reached.insert(to.component.clone());
                    }
                }
                if reached.len() == previous {
                    break;
                }
            }
            let count = provider
                .mappings
                .iter()
                .filter(|other| reached.contains(&other.s_component) && other.s_vid == map.s_vid)
                .map(|other| other.c_component.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            service_cardinality.insert(map.pep.clone(), count);
        }
    }
    Ok(ComponentTreeBinding {
        component: component.to_owned(),
        device: devices.pop_first().unwrap(),
        domain,
        ports: attachments,
        service_cardinality,
    })
}
/// The caller prepares config.ethernet/workload.ethernet with the existing schema4 decoder first.
pub(crate) fn prepare(
    config: &Value,
    workload: &Value,
    eth: &PreparedEthernet,
) -> Result<PreparedWire> {
    prepare_with_provider(config, workload, eth, None)
}

pub(crate) fn prepare_with_provider(
    config: &Value,
    workload: &Value,
    eth: &PreparedEthernet,
    provider: Option<&ProviderTopology>,
) -> Result<PreparedWire> {
    let c = object(
        config,
        &[
            "schema_name",
            "schema_version",
            "ethernet",
            "port_identities",
            "adapters",
        ],
        &[],
    )?;
    let w = object(
        workload,
        &["schema_name", "schema_version", "ethernet", "packets"],
        &[],
    )?;
    if string(c, "schema_name")? != "dir.ethernet_control_wire.config"
        || string(w, "schema_name")? != "dir.ethernet_control_wire.workload"
        || num(c, "schema_version", None, 1, 1)? != 1
        || num(w, "schema_version", None, 1, 1)? != 1
    {
        return Err(err("wire schema mismatch"));
    }
    let mut ports = BTreeMap::new();
    let mut svre_exists = BTreeSet::new();
    let mut registrar_modes = BTreeMap::new();
    for p in &eth.port_policies {
        if eth.devices[p.device].kind != "switch" {
            continue;
        }
        let dirs: Vec<_> = eth
            .directions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.from_port == p.port)
            .collect();
        if dirs.len() != 1 {
            continue;
        }
        let (direction, d) = dirs[0];
        let bridge = eth.devices[p.device].id.clone();
        let mut allowed_vlans = BTreeMap::new();
        if let Some(rows) = config["ethernet"]["dynamic"]["registrable"].as_array() {
            for r in rows {
                if r["port"].as_str() == Some(&p.port) {
                    if let Some(vid) = r["vid"].as_u64() {
                        allowed_vlans.insert(vid as u16, r["tagged"].as_bool().unwrap_or(true));
                    }
                }
            }
        }
        for vid in p.vlans.keys() {
            svre_exists.insert((bridge.clone(), *vid));
            registrar_modes.insert((p.port.clone(), *vid), WireRegistrarMode::FixedIgnoreNew);
        }
        for vid in allowed_vlans.keys() {
            svre_exists.insert((bridge.clone(), *vid));
        }
        ports.insert(
            p.port.clone(),
            WirePort {
                output: p.port.clone(),
                ingress: p.ingress.clone(),
                bridge,
                pvid: p.pvid,
                direction,
                bitrate_bps: d.bitrate_bps,
                source_mac: None,
                static_vlans: p.vlans.clone(),
                allowed_vlans,
            },
        );
    }
    let mut identity_ports = BTreeSet::new();
    let mut identity_macs = BTreeSet::new();
    for identity in array(c, "port_identities")? {
        let o = object(identity, &["port", "source_mac"], &[])?;
        let name = string(o, "port")?;
        let sa = unicast(string(o, "source_mac")?)?;
        if !identity_ports.insert(name.to_string()) || !identity_macs.insert(sa) {
            return Err(err("duplicate wire port identity"));
        }
        ports
            .get_mut(name)
            .ok_or_else(|| err("wire identity is not connected switch output"))?
            .source_mac = Some(sa);
    }
    let mut adapters = vec![];
    let mut ids = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut active_ports = BTreeSet::new();
    let mut rstp_addresses = BTreeSet::new();
    for value in array(c, "adapters")? {
        let protocol = match value.get("protocol").and_then(Value::as_str) {
            Some("igmp") => WireProtocol::Igmp,
            Some("mld") => WireProtocol::Mld,
            Some("rstp") => WireProtocol::Rstp,
            Some("mstp") => WireProtocol::Mstp,
            Some("mvrp") => WireProtocol::Mvrp,
            _ => return Err(err("wire protocol unknown")),
        };
        let mut required = vec![
            "id",
            "protocol",
            "protocol_revision",
            "ports",
            "max_pdu_bytes",
        ];
        let mut optional = vec![];
        optional.push("component");
        if protocol.active() {
            required.extend([
                "max_state_entries",
                "max_pending_timers",
                "max_pending_tx",
                "max_control_steps",
            ]);
            if matches!(protocol, WireProtocol::Rstp | WireProtocol::Mstp) {
                required.push(if protocol == WireProtocol::Rstp {
                    "rstp"
                } else {
                    "mstp"
                });
            } else {
                required.push("max_attributes_per_pdu");
                optional.extend([
                    "port_parameters",
                    "registrar_controls",
                    "local_declarations",
                    "random_seed",
                ]);
            }
        } else {
            required.extend(["role", "max_sources_per_record", "max_groups"]);
            optional.extend([
                "max_filter_leases",
                "max_sources_total",
                "membership_interval_ps",
                "router_interval_ps",
            ]);
        }
        let o = object(value, &required, &optional)?;
        let id = string(o, "id")?.to_string();
        if !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:/@-".contains(&b))
            || !ids.insert(id.clone())
        {
            return Err(err("wire adapter ID invalid/duplicate"));
        }
        if string(o, "protocol_revision")? != protocol.revision() {
            return Err(err("wire revision mismatch"));
        }
        let component_binding = if let Some(component) = o.get("component") {
            if !matches!(protocol, WireProtocol::Rstp | WireProtocol::Mstp) {
                return Err(err("component binding requires spanning-tree protocol"));
            }
            let topology = provider.ok_or_else(|| err("component binding requires provider_l2"))?;
            let binding = component_binding(
                component
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| err("component requires nonempty string"))?,
                topology,
                eth,
            )?;
            if binding.domain == TreeDomain::Customer && protocol != WireProtocol::Rstp {
                return Err(err("C component requires enhanced RSTP adapter"));
            }
            Some(binding)
        } else {
            None
        };
        let mut owned = BTreeSet::new();
        let mut bridge = None;
        for port in array(o, "ports")? {
            let name = port
                .as_str()
                .ok_or_else(|| err("wire port ID requires string"))?;
            if let Some(binding) = &component_binding {
                if !binding.ports.contains_key(name) || !owned.insert(name.to_string()) {
                    return Err(err("component port missing or duplicate"));
                }
                bridge = Some(binding.device.clone());
                continue;
            }
            let p = if protocol.active() {
                ports.get(name)
            } else {
                ports.values().find(|p| p.ingress == name)
            }
            .ok_or_else(|| err("wire port must be connected switch port of correct direction"))?;
            if !owned.insert(p.output.clone()) || bridge.as_ref().is_some_and(|b| b != &p.bridge) {
                return Err(err("wire ports duplicate or span bridges"));
            }
            bridge = Some(p.bridge.clone());
        }
        let bridge = bridge.ok_or_else(|| err("wire ports must be nonempty"))?;
        let owner_id = component_binding
            .as_ref()
            .map_or(bridge.clone(), |b| b.component.clone());
        if !owners.insert((protocol, owner_id)) {
            return Err(err("duplicate wire protocol bridge owner"));
        }
        if protocol.active() {
            let all: BTreeSet<_> = if let Some(binding) = &component_binding {
                binding.ports.keys().cloned().collect()
            } else {
                ports
                    .values()
                    .filter(|p| p.bridge == bridge)
                    .map(|p| p.output.clone())
                    .collect()
            };
            if owned != all {
                return Err(err("active wire owner requires all connected bridge ports"));
            }
            active_ports.extend(
                owned
                    .iter()
                    .filter(|name| ports.contains_key(*name))
                    .cloned(),
            );
        }
        let max_pdu_bytes = num(
            o,
            "max_pdu_bytes",
            None,
            if protocol.active() { 46 } else { 64 },
            1500,
        )? as usize;
        let capacity = |k: &str| {
            num(
                o,
                k,
                if protocol.active() {
                    None
                } else {
                    Some(u32::MAX as u64)
                },
                1,
                u32::MAX as u64,
            )
            .map(|n| n as usize)
        };
        let max_state_entries = capacity("max_state_entries")?;
        let max_pending_timers = capacity("max_pending_timers")?;
        let max_pending_tx = capacity("max_pending_tx")?;
        let max_control_steps = capacity("max_control_steps")?;
        let options = match protocol {
            WireProtocol::MacControl => {
                return Err(err(
                    "MAC Control is configured through composite mac_control",
                ));
            }
            WireProtocol::Igmp | WireProtocol::Mld => {
                if string(o, "role")? != "passive_conservative_snooper_v1" {
                    return Err(err("wire membership role unsupported"));
                }
                WireOptions::Membership(MembershipConfig {
                    max_sources_per_record: num(o, "max_sources_per_record", None, 1, 4096)?
                        as usize,
                    max_groups: num(o, "max_groups", None, 1, 65536)? as usize,
                    max_filter_leases: num(o, "max_filter_leases", Some(4096), 1, 262144)? as usize,
                    max_sources_total: num(o, "max_sources_total", Some(65536), 1, 1048576)?
                        as usize,
                    membership_interval_ps: ps(
                        o,
                        "membership_interval_ps",
                        260_000_000_000_000,
                        false,
                    )?,
                    router_interval_ps: ps(o, "router_interval_ps", 260_000_000_000_000, false)?,
                })
            }
            WireProtocol::Rstp => {
                if max_state_entries < owned.len() {
                    return Err(err("RSTP state capacity"));
                }
                let r = object(
                    &o["rstp"],
                    &["bridge_address", "port_parameters"],
                    &[
                        "bridge_priority",
                        "force_protocol_version",
                        "bridge_max_age_s",
                        "bridge_forward_delay_s",
                        "hello_time_s",
                        "migrate_time_s",
                        "tx_hold_count",
                    ],
                )?;
                let address = string(r, "bridge_address")?;
                if address.len() != 12 || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(err("RSTP bridge address requires 12 hex digits"));
                }
                let address =
                    u64::from_str_radix(address, 16).map_err(|_| err("RSTP bridge address"))?;
                if address == 0 || (address >> 40) & 1 != 0 || !rstp_addresses.insert(address) {
                    return Err(err("RSTP bridge address unicast/nonzero/unique"));
                }
                let priority = num(r, "bridge_priority", Some(32768), 0, 61440)?;
                if priority % 4096 != 0 {
                    return Err(err("RSTP bridge priority granularity"));
                }
                num(r, "force_protocol_version", Some(2), 2, 2)?;
                num(r, "hello_time_s", Some(2), 2, 2)?;
                num(r, "migrate_time_s", Some(3), 3, 3)?;
                let max_age = num(r, "bridge_max_age_s", Some(20), 6, 40)?;
                let forward_delay = num(r, "bridge_forward_delay_s", Some(15), 4, 30)?;
                if max_age > 2 * (forward_delay - 1) {
                    return Err(err("RSTP timer inequality"));
                }
                let mut rports = vec![];
                let mut seen = BTreeSet::new();
                let mut numbers = BTreeSet::new();
                for row in array(r, "port_parameters")? {
                    let q = object(
                        row,
                        &["port", "port_number", "external_path_cost"],
                        &[
                            "port_priority",
                            "admin_edge",
                            "auto_edge",
                            "auto_isolate",
                            "restricted_role",
                            "restricted_tcn",
                        ],
                    )?;
                    let name = string(q, "port")?;
                    if !owned.contains(name) || !seen.insert(name.to_string()) {
                        return Err(err("RSTP port_parameters ownership/duplicate"));
                    }
                    let number = num(q, "port_number", None, 1, 4095)? as u16;
                    if !numbers.insert(number) {
                        return Err(err("RSTP duplicate port number"));
                    }
                    let priority = num(q, "port_priority", Some(128), 0, 240)? as u16;
                    if priority % 16 != 0 {
                        return Err(err("RSTP port priority granularity"));
                    }
                    rports.push(RstpPortConfig {
                        port: name.into(),
                        port_id: (priority << 8) | number,
                        external_path_cost: num(q, "external_path_cost", None, 1, 200_000_000)?
                            as u32,
                        admin_edge: boolean(q, "admin_edge", false)?,
                        auto_edge: boolean(q, "auto_edge", true)?,
                        auto_isolate: boolean(q, "auto_isolate", false)?,
                        restricted_role: boolean(q, "restricted_role", false)?,
                        restricted_tcn: boolean(q, "restricted_tcn", false)?,
                    });
                }
                if seen != owned {
                    return Err(err("RSTP missing port_parameters"));
                }
                if config["ethernet"]["dynamic"]["convergence_ps"] != "0" {
                    return Err(err("wire RSTP requires convergence_ps=0"));
                }
                WireOptions::Rstp(RstpConfig {
                    bridge_id: (priority << 48) | address,
                    max_age: max_age as u16 * 256,
                    forward_delay: forward_delay as u16 * 256,
                    tx_hold_count: num(r, "tx_hold_count", Some(6), 1, 10)? as u32,
                    ports: rports,
                })
            }
            WireProtocol::Mstp => {
                let m = object(
                    &o["mstp"],
                    &["bridge_address", "region", "instances", "port_parameters"],
                    &[
                        "bridge_priority",
                        "bridge_max_age_s",
                        "bridge_forward_delay_s",
                        "hello_time_s",
                        "migrate_time_s",
                        "tx_hold_count",
                        "max_hops",
                    ],
                )?;
                let address = string(m, "bridge_address")?;
                if address.len() != 12 || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(err("MSTP bridge address requires 12 hex digits"));
                }
                let address =
                    u64::from_str_radix(address, 16).map_err(|_| err("MSTP bridge address"))?;
                if address == 0 || (address >> 40) & 1 != 0 || !rstp_addresses.insert(address) {
                    return Err(err("MSTP bridge address unicast/nonzero/unique"));
                }
                let mut cist_priority = num(m, "bridge_priority", Some(32768), 0, 61440)? as u16;
                if cist_priority % 4096 != 0 {
                    return Err(err("MSTP CIST bridge priority granularity"));
                }
                num(m, "hello_time_s", Some(2), 2, 2)?;
                num(m, "migrate_time_s", Some(3), 3, 3)?;
                let max_hops = num(m, "max_hops", Some(20), 1, 255)? as u8;
                let max_age = num(m, "bridge_max_age_s", Some(20), 6, 40)?;
                let forward_delay = num(m, "bridge_forward_delay_s", Some(15), 4, 30)?;
                if max_age > 2 * (forward_delay - 1) {
                    return Err(err("MSTP timer inequality"));
                }
                let region = object(&m["region"], &["name", "revision", "vlan_to_msti"], &[])?;
                let region_name = string(region, "name")?;
                if region_name.len() > 32 {
                    return Err(err("MSTP region name exceeds 32 bytes"));
                }
                let region_revision = num(region, "revision", None, 0, 65535)? as u16;
                let mappings = region["vlan_to_msti"]
                    .as_object()
                    .ok_or_else(|| err("MSTP vlan_to_msti object"))?;
                let mut vlan_map = vec![0u16; 4096];
                for (vid, id) in mappings {
                    let vid: usize = vid.parse().map_err(|_| err("MSTP VLAN key"))?;
                    if !(1..=4094).contains(&vid) {
                        return Err(err("MSTP VLAN out of range"));
                    }
                    let id = id.as_u64().ok_or_else(|| err("MSTP MSTID integer"))?;
                    if id > 4094 {
                        return Err(err("MSTP MSTID out of range"));
                    }
                    vlan_map[vid] = id as u16;
                }
                let mut instances = vec![];
                let mut declared = BTreeSet::new();
                for row in array(m, "instances")? {
                    let q = object(row, &["mstid"], &["bridge_priority"])?;
                    let id = num(q, "mstid", None, 0, 4094)? as u16;
                    if !declared.insert(id) {
                        return Err(err("MSTP duplicate instance"));
                    }
                    let priority = num(q, "bridge_priority", Some(32768), 0, 61440)? as u16;
                    if priority % 4096 != 0 {
                        return Err(err("MSTP instance priority granularity"));
                    }
                    if id == 0 {
                        cist_priority = priority;
                    } else {
                        instances.push(MstpInstanceConfig {
                            mstid: id,
                            bridge_priority: priority,
                        });
                    }
                }
                if instances.len() > 64
                    || vlan_map.iter().any(|id| *id != 0 && !declared.contains(id))
                {
                    return Err(err("MSTP instances do not cover VLAN map"));
                }
                let min_bpdu = 21 + 102 + 16 * instances.len();
                if max_pdu_bytes < min_bpdu {
                    return Err(err("MSTP max_pdu_bytes below encoded BPDU"));
                }
                if max_state_entries < owned.len() * (instances.len() + 1) {
                    return Err(err("MSTP state capacity"));
                }
                let mut mports = vec![];
                let mut seen = BTreeSet::new();
                let mut numbers = BTreeSet::new();
                for row in array(m, "port_parameters")? {
                    let q = object(
                        row,
                        &["port", "port_number", "external_path_cost"],
                        &["port_priority", "internal_path_cost", "admin_edge"],
                    )?;
                    let name = string(q, "port")?;
                    if !owned.contains(name) || !seen.insert(name.to_string()) {
                        return Err(err("MSTP port_parameters ownership/duplicate"));
                    }
                    let number = num(q, "port_number", None, 1, 4095)? as u16;
                    if !numbers.insert(number) {
                        return Err(err("MSTP duplicate port number"));
                    }
                    let priority = num(q, "port_priority", Some(128), 0, 240)? as u16;
                    if priority % 16 != 0 {
                        return Err(err("MSTP port priority granularity"));
                    }
                    let external = num(q, "external_path_cost", None, 1, 200_000_000)? as u32;
                    mports.push(MstpPortConfig {
                        port: name.into(),
                        port_id: (priority << 8) | number,
                        external_path_cost: external,
                        internal_path_cost: num(
                            q,
                            "internal_path_cost",
                            Some(external as u64),
                            1,
                            200_000_000,
                        )? as u32,
                        admin_edge: boolean(q, "admin_edge", false)?,
                    });
                }
                if seen != owned {
                    return Err(err("MSTP missing port_parameters"));
                }
                if config["ethernet"]["dynamic"]["convergence_ps"] != "0" {
                    return Err(err("wire MSTP requires convergence_ps=0"));
                }
                WireOptions::Mstp(MstpConfig {
                    bridge_id: ((cist_priority as u64) << 48) | address,
                    max_age: max_age as u16 * 256,
                    forward_delay: forward_delay as u16 * 256,
                    tx_hold_count: num(m, "tx_hold_count", Some(6), 1, 10)? as u32,
                    max_hops,
                    region_name: region_name.into(),
                    region_revision,
                    vlan_map,
                    instances,
                    ports: mports,
                })
            }
            WireProtocol::Mvrp => {
                if max_state_entries
                    < owned
                        .len()
                        .checked_mul(4094)
                        .ok_or_else(|| err("MVRP state capacity overflow"))?
                {
                    return Err(err("MVRP requires capacity for all port x 4094 states"));
                }
                let mut pp: BTreeMap<String, MvrpPortConfig> = owned
                    .iter()
                    .map(|p| {
                        (
                            p.clone(),
                            MvrpPortConfig {
                                port: p.clone(),
                                join_time_ps: 200_000_000_000,
                                leave_time_ps: 600_000_000_000,
                                leaveall_time_ps: 10_000_000_000_000,
                                periodic_enabled: true,
                                applicant_control: WireApplicantMode::Normal,
                                restricted_vlan_registration: false,
                                transmit_zero: false,
                            },
                        )
                    })
                    .collect();
                let mut seen = BTreeSet::new();
                for row in optional_array(o, "port_parameters")? {
                    let q = object(
                        row,
                        &["port"],
                        &[
                            "join_time_cs",
                            "leave_time_cs",
                            "leaveall_time_cs",
                            "periodic_enabled",
                            "applicant_control",
                            "restricted_vlan_registration",
                            "transmit_zero",
                        ],
                    )?;
                    let name = string(q, "port")?;
                    if !owned.contains(name) || !seen.insert(name.to_string()) {
                        return Err(err("MVRP port_parameters ownership/duplicate"));
                    }
                    let mode = match q.get("applicant_control") {
                        None => WireApplicantMode::Normal,
                        Some(v) => serde_json::from_value(v.clone())
                            .map_err(|_| err("MVRP applicant control"))?,
                    };
                    pp.insert(
                        name.into(),
                        MvrpPortConfig {
                            port: name.into(),
                            join_time_ps: num(q, "join_time_cs", Some(20), 1, 1_844_674_407)?
                                * 10_000_000_000,
                            leave_time_ps: num(q, "leave_time_cs", Some(60), 1, 1_844_674_407)?
                                * 10_000_000_000,
                            leaveall_time_ps: num(
                                q,
                                "leaveall_time_cs",
                                Some(1000),
                                0,
                                1_229_782_938,
                            )? * 10_000_000_000,
                            periodic_enabled: boolean(q, "periodic_enabled", true)?,
                            applicant_control: mode,
                            restricted_vlan_registration: boolean(
                                q,
                                "restricted_vlan_registration",
                                false,
                            )?,
                            transmit_zero: boolean(q, "transmit_zero", false)?,
                        },
                    );
                }
                let mut controls = BTreeMap::new();
                for row in optional_array(o, "registrar_controls")? {
                    let q = object(row, &["port", "vid", "mode"], &[])?;
                    let name = string(q, "port")?;
                    let vid = num(q, "vid", None, 1, 4094)? as u16;
                    let mode: WireRegistrarMode = serde_json::from_value(q["mode"].clone())
                        .map_err(|_| err("MVRP registrar mode"))?;
                    if !owned.contains(name) || controls.insert((name.into(), vid), mode).is_some()
                    {
                        return Err(err("MVRP registrar ownership/duplicate"));
                    }
                    let fixed = matches!(
                        mode,
                        WireRegistrarMode::FixedIgnoreNew | WireRegistrarMode::FixedPropagateNew
                    );
                    if fixed != ports[name].static_vlans.contains_key(&vid) {
                        return Err(err("MVRP fixed mode must agree with static membership"));
                    }
                    registrar_modes.insert((name.into(), vid), mode);
                    svre_exists.insert((bridge.clone(), vid));
                }
                let mut local = BTreeSet::new();
                let mut seen = BTreeSet::new();
                for row in optional_array(o, "local_declarations")? {
                    let q = object(row, &["port", "vid", "declared"], &[])?;
                    let name = string(q, "port")?;
                    let vid = num(q, "vid", None, 1, 4094)? as u16;
                    if !owned.contains(name) || !seen.insert((name.to_string(), vid)) {
                        return Err(err("MVRP local ownership/duplicate"));
                    }
                    if boolean(q, "declared", false)? {
                        local.insert((name.into(), vid));
                    }
                }
                WireOptions::Mvrp(MvrpConfig {
                    max_attributes_per_pdu: num(o, "max_attributes_per_pdu", None, 1, 4094)?
                        as usize,
                    ports: pp.into_values().collect(),
                    registrar_controls: controls,
                    local_declarations: local,
                    random_seed: ps(o, "random_seed", 0, true)?,
                })
            }
        };
        if let Some(binding) = &component_binding {
            if binding.domain == TreeDomain::Customer {
                let WireOptions::Rstp(rstp) = &options else {
                    return Err(err("C component requires RSTP options"));
                };
                let cep = binding
                    .ports
                    .iter()
                    .find_map(|(name, attachment)| {
                        if matches!(attachment, PortAttachment::Physical { .. }) {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| err("C component lacks CEP"))?;
                let expected_mac: [u8; 6] = rstp.bridge_id.to_be_bytes()[2..].try_into().unwrap();
                if ports.get(cep).and_then(|p| p.source_mac) != Some(expected_mac) {
                    return Err(err(
                        "C component bridge address must equal CEP MAC identity",
                    ));
                }
                if binding.service_cardinality.is_empty()
                    || binding.service_cardinality.len() != binding.ports.len() - 1
                {
                    return Err(err("C component PEP/service binding incomplete"));
                }
                for p in &rstp.ports {
                    let number = p.port_id & 0x0fff;
                    if p.port == cep && number != 0x0fff {
                        return Err(err("CEP port number must be 4095"));
                    }
                    if binding.service_cardinality.contains_key(&p.port) {
                        let sv_id = provider
                            .unwrap()
                            .mappings
                            .iter()
                            .find(|m| m.pep == p.port)
                            .unwrap()
                            .s_vid;
                        if number != sv_id {
                            return Err(err("PEP port number must equal service VID"));
                        }
                    }
                }
            }
        }
        adapters.push(PreparedWireAdapter {
            id,
            protocol,
            bridge,
            component_binding,
            ports: owned.into_iter().collect(),
            max_pdu_bytes,
            max_state_entries,
            max_pending_timers,
            max_pending_tx,
            max_control_steps,
            options,
        });
    }
    if adapters.is_empty() || identity_ports != active_ports {
        return Err(err("wire adapters empty or port identity union mismatch"));
    }
    let mut tree_owners = BTreeSet::new();
    for a in adapters
        .iter()
        .filter(|a| matches!(a.protocol, WireProtocol::Rstp | WireProtocol::Mstp))
    {
        let identity = a
            .component_binding
            .as_ref()
            .map_or(a.bridge.as_str(), |b| b.component.as_str());
        if !tree_owners.insert(identity) {
            return Err(err("wire bridge has multiple spanning-tree owners"));
        }
    }
    if let Some(topology) = provider {
        let components: BTreeSet<_> = topology
            .ports
            .values()
            .map(|p| p.component.as_str())
            .collect();
        let owned: BTreeSet<_> = adapters
            .iter()
            .filter_map(|a| a.component_binding.as_ref())
            .map(|b| b.component.as_str())
            .collect();
        if !owned.is_empty() && owned != components {
            return Err(err(
                "provider protocol requires one spanning-tree adapter per component",
            ));
        }
        for adapter in &adapters {
            if let Some(binding) = &adapter.component_binding {
                for port in binding.ports.keys() {
                    for vid in &topology.ports[port].member_vids {
                        registrar_modes
                            .entry((port.clone(), *vid))
                            .or_insert(WireRegistrarMode::FixedIgnoreNew);
                    }
                }
            }
        }
    }
    // A physical bridge component cannot mix active tree/registration owners with abstract bridges.
    for protocol in [WireProtocol::Rstp, WireProtocol::Mstp, WireProtocol::Mvrp] {
        for a in adapters.iter().filter(|a| a.protocol == protocol) {
            if a.component_binding.is_some() {
                continue;
            }
            let mut reached = BTreeSet::from([a.bridge.clone()]);
            loop {
                let old = reached.len();
                for d in &eth.directions {
                    let from = &eth.devices[d.source];
                    let to = &eth.devices[d.destination];
                    if from.kind == "switch"
                        && to.kind == "switch"
                        && (reached.contains(&from.id) || reached.contains(&to.id))
                    {
                        reached.insert(from.id.clone());
                        reached.insert(to.id.clone());
                    }
                }
                if old == reached.len() {
                    break;
                }
            }
            if reached.iter().any(|b| {
                if protocol == WireProtocol::Mvrp {
                    !owners.contains(&(protocol, b.clone()))
                } else {
                    !owners.contains(&(WireProtocol::Rstp, b.clone()))
                        && !owners.contains(&(WireProtocol::Mstp, b.clone()))
                }
            }) {
                return Err(err("wire active component requires every bridge owner"));
            }
        }
    }
    // Active ports must have an unambiguous reverse direction on the same peer.
    for name in &active_ports {
        let p = &ports[name];
        let d = &eth.directions[p.direction];
        let reverse = eth
            .directions
            .iter()
            .filter(|r| {
                r.source == d.destination && r.destination == d.source && r.to_port == p.ingress
            })
            .count();
        if reverse != 1 {
            return Err(err("wire requires bidirectional point-to-point attachment"));
        }
        if let Some(media) = &eth.media {
            if media
                .physical_links
                .iter()
                .any(|l| l.directions.contains(&p.direction) && l.duplex != "full")
            {
                return Err(err("wire shared/half-duplex media unsupported"));
            }
        }
    }
    if let Some(controls) = workload["ethernet"]["controls"].as_array() {
        for control in controls {
            let kind = control["kind"].as_str().unwrap_or("");
            let port = control["port"].as_str().unwrap_or("");
            if matches!(kind, "vlan_register" | "vlan_unregister")
                && adapters
                    .iter()
                    .any(|a| a.protocol == WireProtocol::Mvrp && a.ports.iter().any(|p| p == port))
            {
                return Err(err("abstract registration conflicts with wire MVRP owner"));
            }
            if kind.starts_with("membership_") || kind.starts_with("router_") {
                let protocol = match control["family"].as_str() {
                    Some("ipv4") => WireProtocol::Igmp,
                    Some("ipv6") => WireProtocol::Mld,
                    _ => continue,
                };
                if adapters
                    .iter()
                    .any(|a| a.protocol == protocol && a.ports.iter().any(|p| p == port))
                {
                    return Err(err(
                        "abstract membership conflicts with wire family/port owner",
                    ));
                }
            }
        }
    }
    let mut packets = vec![];
    let mut packet_ids = BTreeSet::new();
    let mut intervals: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for row in array(w, "packets")? {
        let adapter_id = row
            .get("adapter_id")
            .and_then(Value::as_str)
            .ok_or_else(|| err("wire packet adapter required"))?;
        let a = adapters
            .iter()
            .find(|a| a.id == adapter_id)
            .ok_or_else(|| err("wire packet unknown adapter"))?;
        let mut fields = vec![
            "id",
            "adapter_id",
            "ingress_port",
            "arrival_ps",
            "encapsulation",
            "bytes_hex",
        ];
        if !a.protocol.active() {
            fields.extend(["vid", "source_mac", "destination_mac"]);
        }
        let o = object(row, &fields, &[])?;
        let id = string(o, "id")?.to_string();
        if !packet_ids.insert(id.clone()) {
            return Err(err("wire packet duplicate/reserved ID"));
        }
        let ingress = string(o, "ingress_port")?;
        let p = a
            .ports
            .iter()
            .map(|p| &ports[p])
            .find(|p| p.ingress == ingress || a.protocol.active() && p.output == ingress)
            .ok_or_else(|| err("wire packet ingress ownership"))?;
        let arrival = decimal(string(o, "arrival_ps")?)?;
        let data = bytes(string(o, "bytes_hex")?)?;
        if string(o, "encapsulation")?
            != if a.protocol.active() {
                "mac_frame_fcs"
            } else {
                "ip_packet"
            }
        {
            return Err(err("wire encapsulation mismatch"));
        }
        if data.len()
            > if a.protocol.active() {
                a.max_pdu_bytes + 18
            } else {
                a.max_pdu_bytes
            }
        {
            return Err(err("wire packet size limit"));
        }
        let (vid, sa, da, boundary) = if a.protocol.active() {
            (None, None, None, None)
        } else {
            let vid = num(o, "vid", None, 1, 4094)? as u16;
            if !p.static_vlans.contains_key(&vid) && !p.allowed_vlans.contains_key(&vid) {
                return Err(err("wire packet VID not member/registrable"));
            }
            let sa = unicast(string(o, "source_mac")?)?;
            let da = mac(string(o, "destination_mac")?)?;
            if da[0] & 1 == 0 || da[..5] == [1, 128, 194, 0, 0] {
                return Err(err("wire multicast destination MAC invalid/reserved"));
            }
            let mac_bytes = 22 + 46u64.max(data.len() as u64);
            let duration = serialization(mac_bytes + 8, p.bitrate_bps)?;
            let start = arrival
                .checked_sub(duration)
                .ok_or_else(|| err("wire boundary arrival before serialization duration"))?;
            let release = start
                .checked_add(serialization(mac_bytes + 20, p.bitrate_bps)?)
                .ok_or_else(|| err("wire boundary release overflow"))?;
            intervals
                .entry(p.ingress.clone())
                .or_default()
                .push((start, release));
            (
                Some(vid),
                Some(sa),
                Some(da),
                Some(BoundaryPlan {
                    start_ps: start,
                    arrival_ps: arrival,
                    release_ps: release,
                    mac_bytes,
                    bitrate_bps: p.bitrate_bps,
                }),
            )
        };
        packets.push(PreparedWirePacket {
            id,
            adapter_id: adapter_id.into(),
            ingress_port: p.ingress.clone(),
            output_port: p.output.clone(),
            arrival_ps: arrival,
            bytes: data,
            vid,
            source_mac: sa,
            destination_mac: da,
            boundary,
        });
    }
    // Bound the complete finite workload's reachable key ledger before execution.
    // Invalid semantic PDUs stay runtime drops and contribute no candidates.
    let mut candidate_filters: BTreeMap<
        String,
        BTreeSet<(
            crate::runtime::wire::membership::Key,
            crate::runtime::wire::membership::Filter,
        )>,
    > = BTreeMap::new();
    for packet in &packets {
        let source = adapters.iter().find(|a| a.id == packet.adapter_id).unwrap();
        let WireOptions::Membership(limits) = &source.options else {
            continue;
        };
        let Ok(message) = crate::runtime::wire::membership::decode(
            &packet.bytes,
            source.protocol == WireProtocol::Mld,
            limits.max_sources_per_record,
        ) else {
            continue;
        };
        if packet.destination_mac
            != Some(crate::runtime::wire::membership::destination_mac(
                message.destination,
            ))
        {
            continue;
        }
        let mut reached = BTreeSet::from([source.bridge.clone()]);
        loop {
            let previous = reached.len();
            for direction in &eth.directions {
                let from = &eth.devices[direction.source];
                let to = &eth.devices[direction.destination];
                if from.kind == "switch"
                    && to.kind == "switch"
                    && (reached.contains(&from.id) || reached.contains(&to.id))
                {
                    reached.insert(from.id.clone());
                    reached.insert(to.id.clone());
                }
            }
            if reached.len() == previous {
                break;
            }
        }
        for target in adapters
            .iter()
            .filter(|a| a.protocol == source.protocol && reached.contains(&a.bridge))
        {
            let candidates = candidate_filters.entry(target.id.clone()).or_default();
            for port in &target.ports {
                for record in &message.records {
                    if record.kind == 6
                        || matches!(record.kind, 1 | 3 | 5) && record.sources.is_empty()
                    {
                        continue;
                    }
                    candidates.insert((
                        crate::runtime::wire::membership::Key {
                            bridge: target.bridge.clone(),
                            vid: packet.vid.unwrap(),
                            group: record.group,
                            port: port.clone(),
                        },
                        crate::runtime::wire::membership::Filter {
                            exclude: matches!(record.kind, 2 | 4),
                            sources: record.sources.clone(),
                        },
                    ));
                }
            }
        }
    }
    for a in &adapters {
        if let WireOptions::Membership(limits) = &a.options {
            if let Some(keys) = candidate_filters.get(&a.id) {
                let groups: BTreeSet<_> = keys.iter().map(|(k, _)| k).collect();
                let sources = keys
                    .iter()
                    .try_fold(0usize, |n, (_, f)| n.checked_add(f.sources.len()))
                    .ok_or_else(|| err("wire candidate source count overflow"))?;
                if groups.len() > limits.max_groups
                    || keys.len() > limits.max_filter_leases
                    || sources > limits.max_sources_total
                {
                    return Err(err("wire reachable membership ledger capacity"));
                }
            }
        }
    }
    // A boundary stimulus cannot share its incoming physical link with an ordinary source.
    for packet in &packets {
        if packet.boundary.is_none() {
            continue;
        }
        let p = &ports[&packet.output_port];
        let incoming = eth
            .directions
            .iter()
            .find(|d| d.to_port == p.ingress)
            .ok_or_else(|| err("wire boundary incoming direction missing"))?;
        if eth.generators.iter().any(|g| g.source == incoming.source) {
            return Err(err(
                "boundary input conflicts with ordinary source workload on link",
            ));
        }
    }
    for spans in intervals.values_mut() {
        spans.sort_unstable();
        if spans.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(err("BoundaryOverlap"));
        }
    }
    adapters.sort_by(|a, b| {
        (
            &a.bridge,
            match a.protocol {
                WireProtocol::Rstp => 0,
                WireProtocol::Mstp => 1,
                WireProtocol::Mvrp => 2,
                WireProtocol::Igmp => 3,
                WireProtocol::Mld => 4,
                WireProtocol::MacControl => 5,
            },
            &a.id,
        )
            .cmp(&(
                &b.bridge,
                match b.protocol {
                    WireProtocol::Rstp => 0,
                    WireProtocol::Mstp => 1,
                    WireProtocol::Mvrp => 2,
                    WireProtocol::Igmp => 3,
                    WireProtocol::Mld => 4,
                    WireProtocol::MacControl => 5,
                },
                &b.id,
            ))
    });
    Ok(PreparedWire {
        adapters,
        ports,
        packets,
        svre_exists,
        registrar_modes,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ethernet::*;
    use serde_json::json;
    #[test]
    fn provider_service_cardinality_includes_remote_customer_interfaces() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/network-composite/provider-l2/protocol.ini");
        let prepared = crate::prepare(&fixture).unwrap();
        let network = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap();
        let wire = network.wire.as_ref().unwrap();
        let mut customer_components = 0;
        for adapter in &wire.adapters {
            let binding = adapter.component_binding.as_ref().unwrap();
            if binding.domain != TreeDomain::Customer {
                continue;
            }
            customer_components += 1;
            let expected = if binding.component.ends_with(".cb") {
                2
            } else {
                4
            };
            assert_eq!(binding.service_cardinality.len(), 1);
            for (pep, count) in &binding.service_cardinality {
                assert_eq!(*count, expected, "{}", binding.component);
                assert!(
                    wire.registrar_modes.contains_key(&(pep.clone(), 10)),
                    "logical PEP VLAN must participate in TC scope"
                );
            }
        }
        assert_eq!(customer_components, 6);
    }
    pub(super) fn ethernet() -> PreparedEthernet {
        let devices = [("s", "switch"), ("h", "endpoint")]
            .into_iter()
            .map(|(id, kind)| EthernetDevice {
                id: id.into(),
                kind: kind.into(),
                mac: None,
                queue_capacity: 64,
                tx_processing_delay_ps: 0,
                rx_processing_delay_ps: 0,
                forward_delay_ps: 0,
                fdb: BTreeMap::new(),
                vlan_fdb: BTreeMap::new(),
                multicast: BTreeMap::new(),
                subscriptions: BTreeSet::new(),
                unknown_multicast: "flood".into(),
            })
            .collect();
        PreparedEthernet {
            devices,
            directions: vec![
                EthernetDirection {
                    channel_id: "out".into(),
                    from_port: "s/p".into(),
                    to_port: "h/in".into(),
                    source: 0,
                    destination: 1,
                    bitrate_bps: 100_000_000,
                    delay_ps: 0,
                },
                EthernetDirection {
                    channel_id: "in".into(),
                    from_port: "h/out".into(),
                    to_port: "s/in".into(),
                    source: 1,
                    destination: 0,
                    bitrate_bps: 100_000_000,
                    delay_ps: 0,
                },
            ],
            generators: vec![],
            outputs: vec![],
            port_policies: vec![EthernetPortPolicy {
                port: "s/p".into(),
                ingress: "s/in".into(),
                device: 0,
                pvid: 1,
                admit: "all".into(),
                default_priority: 0,
                vlans: BTreeMap::from([(1, true)]),
            }],
            media: None,
        }
    }
    pub(super) fn config() -> Value {
        json!({"schema_name":"dir.ethernet_control_wire.config","schema_version":1,"ethernet":{"dynamic":{"convergence_ps":"0","registrable":[]}},"port_identities":[],"adapters":[{"id":"igmp","protocol":"igmp","protocol_revision":"igmp-rfc9776-2025","ports":["s/in"],"role":"passive_conservative_snooper_v1","max_pdu_bytes":1500,"max_sources_per_record":16,"max_groups":16}]})
    }
    pub(super) fn workload() -> Value {
        json!({"schema_name":"dir.ethernet_control_wire.workload","schema_version":1,"ethernet":{"schema_version":4,"generators":[],"controls":[]},"packets":[]})
    }
    #[test]
    fn strict_root_and_defaults() {
        let eth = ethernet();
        let c = config();
        let w = workload();
        let p = prepare(&c, &w, &eth).unwrap();
        let WireOptions::Membership(m) = &p.adapters[0].options else {
            panic!()
        };
        assert_eq!(m.membership_interval_ps, 260_000_000_000_000);
        for field in ["options", "packet", "limits"] {
            let mut bad = c.clone();
            bad[field] = json!({});
            assert!(prepare(&bad, &w, &eth).is_err());
        }
        let mut bad = c.clone();
        bad["adapters"][0]["max_groups"] = json!(1.0);
        assert!(prepare(&bad, &w, &eth).is_err());
        let mut bad = c;
        bad["adapters"][0]["router_interval_ps"] = Value::Null;
        assert!(prepare(&bad, &w, &eth).is_err());
    }
    #[test]
    fn malformed_ip_is_runtime_drop_and_boundary_exact() {
        let eth = ethernet();
        let mut w = workload();
        w["packets"] = json!([{"id":"bad","adapter_id":"igmp","ingress_port":"s/in","arrival_ps":"10000000","encapsulation":"ip_packet","bytes_hex":"00","vid":1,"source_mac":"02:00:00:00:00:01","destination_mac":"01:00:5e:00:00:16"}]);
        let p = prepare(&config(), &w, &eth).unwrap();
        let b = p.packets[0].boundary.as_ref().unwrap();
        assert_eq!(
            (b.mac_bytes, b.start_ps, b.release_ps),
            (68, 3_920_000, 10_960_000)
        );
        let mut second = w["packets"][0].clone();
        second["id"] = json!("second");
        w["packets"].as_array_mut().unwrap().push(second);
        assert!(prepare(&config(), &w, &eth).is_err());
    }
    #[test]
    fn abstract_membership_cannot_double_own() {
        let mut w = workload();
        w["ethernet"]["controls"] =
            json!([{"kind":"membership_leave","family":"ipv4","port":"s/p"}]);
        assert!(prepare(&config(), &w, &ethernet()).is_err());
    }
    #[test]
    fn mstp_preparation_keeps_complete_vlan_allocation() {
        let mut c = config();
        c["port_identities"] = json!([{"port":"s/p","source_mac":"02:00:00:00:00:01"}]);
        c["adapters"] = json!([{
            "id":"mst", "protocol":"mstp",
            "protocol_revision":WireProtocol::Mstp.revision(),
            "ports":["s/p"],"max_pdu_bytes":1500,
            "max_state_entries":3,"max_pending_timers":8,
            "max_pending_tx":8,"max_control_steps":1000,
            "mstp":{"bridge_address":"020000000001",
                "region":{"name":"region","revision":7,
                    "vlan_to_msti":{"10":1,"20":2}},
                "instances":[{"mstid":1,"bridge_priority":4096},
                    {"mstid":2,"bridge_priority":32768}],
                "port_parameters":[{"port":"s/p","port_number":1,
                    "external_path_cost":20000,"internal_path_cost":10000}]}
        }]);
        let prepared = prepare(&c, &workload(), &ethernet()).unwrap();
        let WireOptions::Mstp(m) = &prepared.adapters[0].options else {
            panic!()
        };
        assert_eq!(m.vlan_map.len(), 4096);
        assert_eq!(
            (
                m.vlan_map[0],
                m.vlan_map[10],
                m.vlan_map[20],
                m.vlan_map[4095]
            ),
            (0, 1, 2, 0)
        );
        assert_eq!(m.ports[0].internal_path_cost, 10000);
        let mut bad = c.clone();
        bad["adapters"][0]["mstp"]["region"]["vlan_to_msti"]["20"] = json!(3);
        assert!(prepare(&bad, &workload(), &ethernet()).is_err());
        let mut bad = c;
        bad["adapters"][0]["mstp"]["region"]["name"] = json!("a".repeat(33));
        assert!(prepare(&bad, &workload(), &ethernet()).is_err());
    }
}

#[cfg(test)]
#[path = "../runtime/wire/prepare_vectors.rs"]
mod supplied_vectors;

#[cfg(test)]
pub(crate) fn vector_serialization(bytes: u64, rate: u64) -> Result<u64> {
    serialization(bytes, rate)
}
