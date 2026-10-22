//! Strict composite-only active IGMPv3/MLDv2 configuration.
use crate::runtime::network::multicast_active::{BridgePort, Change, Config, Filter, Interface};
use crate::types::{Diagnostic, ethernet::PreparedEthernet, wire::*};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

type Result<T> = std::result::Result<T, Diagnostic>;
fn invalid(path: &str, message: &str) -> Diagnostic {
    Diagnostic::prepare(message).with_target(path)
}
fn object<'a>(
    v: &'a Value,
    path: &str,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, Value>> {
    let o = v
        .as_object()
        .ok_or_else(|| invalid(path, "multicast object required"))?;
    if o.len() != keys.len() || keys.iter().any(|k| !o.contains_key(*k)) {
        return Err(invalid(path, "multicast object fields differ"));
    }
    Ok(o)
}
fn str_field<'a>(o: &'a serde_json::Map<String, Value>, key: &str, path: &str) -> Result<&'a str> {
    o[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(path, "nonempty string required"))
}
fn time(o: &serde_json::Map<String, Value>, key: &str, path: &str) -> Result<u64> {
    let s = str_field(o, key, path)?;
    if s.is_empty() || s.len() > 1 && s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(
            path,
            "canonical picosecond decimal string required",
        ));
    }
    s.parse()
        .map_err(|_| invalid(path, "picosecond value overflow"))
}
fn number(
    o: &serde_json::Map<String, Value>,
    key: &str,
    lo: u64,
    hi: u64,
    path: &str,
) -> Result<u64> {
    o[key]
        .as_u64()
        .filter(|n| (lo..=hi).contains(n))
        .ok_or_else(|| invalid(path, "multicast integer out of range"))
}
fn ip(o: &serde_json::Map<String, Value>, key: &str, path: &str) -> Result<IpAddr> {
    str_field(o, key, path)?
        .parse()
        .map_err(|_| invalid(path, "invalid IP address"))
}
fn mac(value: &str, path: &str) -> Result<[u8; 6]> {
    if value.len() != 17
        || value.as_bytes().iter().enumerate().any(|(i, b)| {
            if i % 3 == 2 {
                *b != b':'
            } else {
                !b.is_ascii_hexdigit() || b.is_ascii_uppercase()
            }
        })
    {
        return Err(invalid(path, "canonical lowercase MAC required"));
    }
    let mut out = [0; 6];
    for (i, part) in value.split(':').enumerate() {
        out[i] = u8::from_str_radix(part, 16).map_err(|_| invalid(path, "invalid MAC"))?;
    }
    if out == [0; 6] || out[0] & 1 != 0 {
        return Err(invalid(path, "unicast source MAC required"));
    }
    Ok(out)
}
fn valid_source(a: IpAddr) -> bool {
    !a.is_multicast()
        && !a.is_unspecified()
        && !a.is_loopback()
        && !matches!(a, IpAddr::V4(v) if v == Ipv4Addr::BROADCAST)
}
fn valid_interface(a: IpAddr) -> bool {
    match a {
        IpAddr::V4(_) => valid_source(a),
        IpAddr::V6(v) => v.segments()[0] & 0xffc0 == 0xfe80,
    }
}

pub(crate) fn prepare(value: &Value, ethernet: &PreparedEthernet) -> Result<Config> {
    let o = object(
        value,
        "/multicast_active",
        &["random_seed", "interfaces", "changes"],
    )?;
    let random_seed = number(
        o,
        "random_seed",
        1,
        u64::MAX,
        "/multicast_active/random_seed",
    )?;
    let rows = o["interfaces"]
        .as_array()
        .ok_or_else(|| invalid("/multicast_active/interfaces", "interfaces array required"))?;
    if rows.is_empty() || rows.len() > 64 {
        return Err(invalid(
            "/multicast_active/interfaces",
            "1..64 interfaces required",
        ));
    }
    let mut interfaces = Vec::with_capacity(rows.len());
    let mut ids = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut addresses = BTreeSet::new();
    for (i, value) in rows.iter().enumerate() {
        let path = format!("/multicast_active/interfaces/{i}");
        let o = object(
            value,
            &path,
            &[
                "id",
                "output_port",
                "vid",
                "address",
                "mac",
                "router",
                "router_version",
                "robustness",
                "query_interval_ps",
                "query_response_ps",
                "last_member_interval_ps",
                "unsolicited_interval_ps",
                "max_groups",
                "max_sources",
            ],
        )?;
        let id = str_field(o, "id", &format!("{path}/id"))?.to_string();
        let output_port = str_field(o, "output_port", &format!("{path}/output_port"))?.to_string();
        let vid = number(o, "vid", 1, 4094, &format!("{path}/vid"))? as u16;
        let address = ip(o, "address", &format!("{path}/address"))?;
        let mac = mac(
            str_field(o, "mac", &format!("{path}/mac"))?,
            &format!("{path}/mac"),
        )?;
        let router = o["router"]
            .as_bool()
            .ok_or_else(|| invalid(&format!("{path}/router"), "router boolean required"))?;
        let router_version =
            number(o, "router_version", 1, 3, &format!("{path}/router_version"))? as u8;
        if address.is_ipv6() && router_version == 3 {
            return Err(invalid(
                &format!("{path}/router_version"),
                "MLD version must be 1 or 2",
            ));
        }
        if !valid_interface(address) {
            return Err(invalid(
                &format!("{path}/address"),
                "IGMP requires IPv4 unicast; MLD requires IPv6 link-local",
            ));
        }
        let query_interval_ps = time(o, "query_interval_ps", &format!("{path}/query_interval_ps"))?;
        let query_response_ps = time(o, "query_response_ps", &format!("{path}/query_response_ps"))?;
        let last_member_interval_ps = time(
            o,
            "last_member_interval_ps",
            &format!("{path}/last_member_interval_ps"),
        )?;
        let unsolicited_interval_ps = time(
            o,
            "unsolicited_interval_ps",
            &format!("{path}/unsolicited_interval_ps"),
        )?;
        let max_groups = number(o, "max_groups", 1, 4096, &format!("{path}/max_groups"))? as usize;
        let max_sources =
            number(o, "max_sources", 1, 4096, &format!("{path}/max_sources"))? as usize;
        let robustness = number(o, "robustness", 2, 7, &format!("{path}/robustness"))? as u8;
        if query_interval_ps == 0
            || query_interval_ps % 1_000_000_000_000 != 0
            || query_interval_ps > 255_000_000_000_000
            || query_response_ps == 0
            || query_response_ps >= query_interval_ps
            || last_member_interval_ps == 0
            || unsolicited_interval_ps < 2
            || (address.is_ipv4()
                && (query_response_ps > 12_700_000_000_000
                    || last_member_interval_ps > 12_700_000_000_000
                    || query_response_ps % 100_000_000_000 != 0
                    || last_member_interval_ps % 100_000_000_000 != 0))
            || (address.is_ipv6()
                && (query_response_ps > 65_535_000_000_000
                    || last_member_interval_ps > 65_535_000_000_000
                    || query_response_ps % 1_000_000_000 != 0
                    || last_member_interval_ps % 1_000_000_000 != 0))
        {
            return Err(invalid(&path, "unrepresentable multicast timer settings"));
        }
        if router_version >= if address.is_ipv6() { 2 } else { 3 } {
            use crate::runtime::network::multicast_active::{encode_float8, encode_float16};
            if encode_float8(query_interval_ps / 1_000_000_000_000).is_none()
                || address.is_ipv6()
                    && (encode_float16(query_response_ps / 1_000_000_000).is_none()
                        || encode_float16(last_member_interval_ps / 1_000_000_000).is_none())
            {
                return Err(invalid(
                    &path,
                    "unrepresentable multicast floating-code timer settings",
                ));
            }
        }
        let direction = ethernet
            .directions
            .iter()
            .find(|d| d.from_port == output_port)
            .ok_or_else(|| {
                invalid(
                    &format!("{path}/output_port"),
                    "multicast output is not a connected Ethernet port",
                )
            })?;
        let device = &ethernet.devices[direction.source];
        if device.kind != "endpoint" {
            return Err(invalid(
                &format!("{path}/output_port"),
                "active host/router must be an endpoint",
            ));
        }
        let policy = ethernet
            .port_policies
            .iter()
            .find(|p| p.port == output_port)
            .ok_or_else(|| {
                invalid(
                    &format!("{path}/output_port"),
                    "multicast output has no VLAN policy",
                )
            })?;
        if !policy.vlans.contains_key(&vid) {
            return Err(invalid(&format!("{path}/vid"), "VID absent on output port"));
        }
        if !ids.insert(id.clone())
            || !owners.insert((output_port.clone(), vid, address.is_ipv6()))
            || !addresses.insert((address, vid))
        {
            return Err(invalid(&path, "duplicate multicast identity/port/address"));
        }
        interfaces.push(Interface {
            id,
            output_port,
            ingress_port: policy.ingress.clone(),
            vid,
            address,
            mac,
            router,
            router_version,
            robustness,
            query_interval_ps,
            query_response_ps,
            last_member_interval_ps,
            unsolicited_interval_ps,
            max_groups,
            max_sources,
        });
    }
    let rows = o["changes"]
        .as_array()
        .ok_or_else(|| invalid("/multicast_active/changes", "changes array required"))?;
    if rows.len() > 4096 {
        return Err(invalid(
            "/multicast_active/changes",
            "too many multicast changes",
        ));
    }
    let mut changes = Vec::with_capacity(rows.len());
    let mut previous = None;
    for (i, value) in rows.iter().enumerate() {
        let path = format!("/multicast_active/changes/{i}");
        let o = object(
            value,
            &path,
            &["at_ps", "interface", "socket", "group", "filter"],
        )?;
        let at_ps = time(o, "at_ps", &format!("{path}/at_ps"))?;
        if previous.is_some_and(|p| at_ps < p) {
            return Err(invalid(&path, "multicast changes must be time ordered"));
        }
        previous = Some(at_ps);
        let interface = str_field(o, "interface", &format!("{path}/interface"))?.to_string();
        let iface = interfaces
            .iter()
            .find(|f| f.id == interface)
            .ok_or_else(|| invalid(&format!("{path}/interface"), "unknown multicast interface"))?;
        let socket = str_field(o, "socket", &format!("{path}/socket"))?.to_string();
        let group = ip(o, "group", &format!("{path}/group"))?;
        if !group.is_multicast()
            || group.is_ipv6() != iface.address.is_ipv6()
            || group
                == if group.is_ipv6() {
                    IpAddr::V6("ff02::1".parse::<Ipv6Addr>().unwrap())
                } else {
                    IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))
                }
        {
            return Err(invalid(
                &format!("{path}/group"),
                "invalid multicast group/family",
            ));
        }
        let filter = if o["filter"].is_null() {
            None
        } else {
            let f = object(
                &o["filter"],
                &format!("{path}/filter"),
                &["mode", "sources"],
            )?;
            let exclude = match str_field(f, "mode", &format!("{path}/filter/mode"))? {
                "include" => false,
                "exclude" => true,
                _ => {
                    return Err(invalid(
                        &format!("{path}/filter/mode"),
                        "mode must be include/exclude",
                    ));
                }
            };
            let sources = f["sources"].as_array().ok_or_else(|| {
                invalid(&format!("{path}/filter/sources"), "source array required")
            })?;
            if sources.len() > iface.max_sources {
                return Err(invalid(
                    &format!("{path}/filter/sources"),
                    "source count exceeds interface limit",
                ));
            }
            let mut unique = BTreeSet::new();
            for (j, value) in sources.iter().enumerate() {
                let source: IpAddr = value
                    .as_str()
                    .ok_or_else(|| {
                        invalid(
                            &format!("{path}/filter/sources/{j}"),
                            "source address required",
                        )
                    })?
                    .parse()
                    .map_err(|_| {
                        invalid(
                            &format!("{path}/filter/sources/{j}"),
                            "invalid source address",
                        )
                    })?;
                if !valid_source(source)
                    || source.is_ipv6() != group.is_ipv6()
                    || !unique.insert(source)
                {
                    return Err(invalid(
                        &format!("{path}/filter/sources/{j}"),
                        "invalid/duplicate source",
                    ));
                }
            }
            if !exclude && unique.is_empty() {
                None
            } else {
                Some(Filter {
                    exclude,
                    sources: unique,
                })
            }
        };
        changes.push(Change {
            at_ps,
            interface,
            socket,
            group,
            filter,
        });
    }
    let bridge_ports = ethernet
        .port_policies
        .iter()
        .filter(|p| ethernet.devices[p.device].kind == "switch")
        .flat_map(|p| {
            p.vlans.keys().map(|vid| BridgePort {
                bridge: ethernet.devices[p.device].id.clone(),
                output_port: p.port.clone(),
                ingress_port: p.ingress.clone(),
                vid: *vid,
            })
        })
        .collect();
    Ok(Config {
        interfaces,
        bridge_ports,
        changes,
        random_seed,
    })
}

/// Build passive bridge adapters and endpoint wire ownership for generated packets.
/// The caller installs this only in the composite profile when active multicast is configured.
pub(crate) fn synthesize_wire(
    config: &Config,
    ethernet: &PreparedEthernet,
) -> Result<PreparedWire> {
    let mut ports = BTreeMap::new();
    let mut svre_exists = BTreeSet::new();
    let mut registrar_modes = BTreeMap::new();
    for policy in &ethernet.port_policies {
        let Some((direction, line)) = ethernet
            .directions
            .iter()
            .enumerate()
            .find(|(_, d)| d.from_port == policy.port)
        else {
            continue;
        };
        let device = &ethernet.devices[policy.device];
        for vid in policy.vlans.keys() {
            svre_exists.insert((device.id.clone(), *vid));
            registrar_modes.insert(
                (policy.port.clone(), *vid),
                WireRegistrarMode::FixedIgnoreNew,
            );
        }
        ports.insert(
            policy.port.clone(),
            WirePort {
                output: policy.port.clone(),
                ingress: policy.ingress.clone(),
                bridge: device.id.clone(),
                pvid: policy.pvid,
                direction,
                bitrate_bps: line.bitrate_bps,
                source_mac: config
                    .interfaces
                    .iter()
                    .find(|f| f.output_port == policy.port)
                    .map(|f| f.mac),
                static_vlans: policy.vlans.clone(),
                allowed_vlans: policy.vlans.clone(),
            },
        );
    }
    let mut adapters = Vec::new();
    for (device_index, device) in ethernet.devices.iter().enumerate() {
        for v6 in [false, true] {
            let device_interfaces: Vec<_> = config
                .interfaces
                .iter()
                .filter(|f| {
                    f.address.is_ipv6() == v6
                        && ethernet
                            .port_policies
                            .iter()
                            .any(|p| p.device == device_index && p.port == f.output_port)
                })
                .collect();
            if device_interfaces.is_empty() && device.kind != "switch" {
                continue;
            }
            if device.kind == "switch"
                && !config.interfaces.iter().any(|f| f.address.is_ipv6() == v6)
            {
                continue;
            }
            let adapter_ports: Vec<_> = ethernet
                .port_policies
                .iter()
                .filter(|p| p.device == device_index && ports.contains_key(&p.port))
                .map(|p| p.port.clone())
                .collect();
            if adapter_ports.is_empty() {
                continue;
            }
            let reference = config
                .interfaces
                .iter()
                .find(|f| f.address.is_ipv6() == v6)
                .ok_or_else(|| invalid("/multicast_active", "missing family interface"))?;
            let max_groups = config
                .interfaces
                .iter()
                .filter(|f| f.address.is_ipv6() == v6)
                .map(|f| f.max_groups)
                .sum::<usize>()
                .min(4096);
            let max_sources = config
                .interfaces
                .iter()
                .filter(|f| f.address.is_ipv6() == v6)
                .map(|f| f.max_sources)
                .sum::<usize>()
                .min(4096);
            let membership_interval_ps = (reference.robustness as u64)
                .checked_mul(reference.query_interval_ps)
                .and_then(|v| v.checked_add(2 * reference.query_response_ps))
                .ok_or_else(|| invalid("/multicast_active", "membership interval overflow"))?;
            let router_interval_ps = (reference.robustness as u64)
                .checked_mul(reference.query_interval_ps)
                .and_then(|v| v.checked_add(reference.query_response_ps / 2))
                .ok_or_else(|| invalid("/multicast_active", "router interval overflow"))?;
            adapters.push(PreparedWireAdapter {
                id: format!(
                    "@multicast/{}/{}/{}",
                    device.id,
                    if v6 { "mld" } else { "igmp" },
                    device_index
                ),
                protocol: if v6 {
                    WireProtocol::Mld
                } else {
                    WireProtocol::Igmp
                },
                bridge: device.id.clone(),
                component_binding: None,
                ports: adapter_ports,
                max_pdu_bytes: 1522,
                max_state_entries: max_groups * max_sources.max(1),
                max_pending_timers: 8192,
                max_pending_tx: 8192,
                max_control_steps: 32768,
                options: WireOptions::Membership(MembershipConfig {
                    max_sources_per_record: max_sources,
                    max_groups,
                    max_filter_leases: max_groups * 4,
                    max_sources_total: max_sources * 4,
                    membership_interval_ps,
                    router_interval_ps,
                }),
            });
        }
    }
    Ok(PreparedWire {
        adapters,
        ports,
        packets: Vec::new(),
        svre_exists,
        registrar_modes,
    })
}
