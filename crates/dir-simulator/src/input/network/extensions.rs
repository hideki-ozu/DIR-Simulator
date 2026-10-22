//! Exact validators for the explicitly selected network extensions.
use super::{Result, error, exact};
mod gptp;
mod mac_control;
mod mac_merge;
mod phy;
use crate::types::{PreparedCan, ethernet::PreparedEthernet, network::*};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn integer(v: &Value, min: u64, max: u64, pointer: &str) -> Result<u64> {
    v.as_u64()
        .filter(|n| *n >= min && *n <= max)
        .ok_or_else(|| error(format!("expected integer {min}..{max}")).with_target(pointer))
}
fn string<'a>(v: &'a Value, pointer: &str) -> Result<&'a str> {
    v.as_str()
        .filter(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:/@-".contains(&b))
        })
        .ok_or_else(|| error("expected nonempty ASCII identifier").with_target(pointer))
}
fn array<'a>(v: &'a Value, pointer: &str) -> Result<&'a Vec<Value>> {
    v.as_array()
        .ok_or_else(|| error("expected array").with_target(pointer))
}
fn decimal(v: &Value, pointer: &str) -> Result<u64> {
    let s = v
        .as_str()
        .ok_or_else(|| error("expected canonical u64 decimal string").with_target(pointer))?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || s.len() > 1 && s.starts_with('0') {
        return Err(error("expected canonical u64 decimal string").with_target(pointer));
    }
    s.parse()
        .map_err(|_| error("decimal exceeds u64").with_target(pointer))
}
fn limits(v: &Value) -> Result<NetworkLimits> {
    exact(
        v,
        &["max_pending_branches", "max_control_events"],
        "extension limits",
    )
    .map_err(|d| d.with_target("/limits"))?;
    Ok(NetworkLimits {
        max_pending_branches: integer(
            &v["max_pending_branches"],
            1,
            u32::MAX.into(),
            "/limits/max_pending_branches",
        )? as u32,
        max_control_events: integer(
            &v["max_control_events"],
            1,
            u32::MAX.into(),
            "/limits/max_control_events",
        )? as u32,
    })
}
fn static_base(network: &PreparedNetwork, workload: &Value, allow_controls: bool) -> Result<()> {
    if !allow_controls && !array(&workload["controls"], "/controls")?.is_empty() {
        return Err(error("abstract extension requires controls=[]").with_target("/controls"));
    }
    if network
        .ethernet
        .directions
        .iter()
        .any(|d| d.bitrate_bps == 0)
    {
        return Err(error(
            "abstract extension requires static full-duplex links with positive bitrate",
        )
        .with_target("/base"));
    }
    Ok(())
}
pub(super) fn prepare(
    ethernet: PreparedEthernet,
    config: Value,
    workload: Value,
    profile: &str,
    config_source: (&str, &Path),
    workload_source: Option<(&str, &Path)>,
) -> Result<PreparedNetwork> {
    let doc = crate::input::JsonDocument::parse(config_source.0)?;
    if let Some((text, _)) = workload_source {
        crate::input::JsonDocument::parse(text)?;
    }
    let prepare_inner = || -> Result<PreparedNetwork> {
        let composite = profile == "ethernet.network.composite.v1";
        let replication = matches!(
            profile,
            "ethernet.redundancy.abstract.v1" | "ethernet.frer.rtag.v1"
        ) || composite;
        let rtag = profile == "ethernet.frer.rtag.v1" || composite;
        let extension = if replication {
            "replication"
        } else {
            "preemption"
        };
        if composite {
            let mut fields = vec![
                "schema_name",
                "schema_version",
                "base",
                "replication",
                "preemption",
                "clocks",
                "limits",
            ];
            if config.get("mac_merge").is_some() {
                fields.push("mac_merge");
            }
            if config.get("gptp").is_some() {
                fields.push("gptp");
            }
            if config.get("phy_attachments").is_some() {
                fields.push("phy_attachments");
            }
            if config.get("physical_mac_policy").is_some() {
                fields.push("physical_mac_policy");
            }
            if config.get("multicast_active").is_some() {
                fields.push("multicast_active");
            }
            if config.get("qcc").is_some() {
                fields.push("qcc");
            }
            if config.get("shared_lans").is_some() {
                fields.push("shared_lans");
            }
            if config.get("wire_control").is_some() {
                fields.push("wire_control");
            }
            if config.get("provider_l2").is_some() {
                fields.push("provider_l2");
            }
            for key in ["configured_media", "mac_control"] {
                if config.get(key).is_some() {
                    fields.push(key);
                }
            }
            exact(&config, &fields, "network composition")?;
        } else {
            exact(
                &config,
                &["schema_name", "schema_version", "base", extension, "limits"],
                "network extension",
            )?;
        }
        let schema = if composite {
            "dir.ethernet_network_composite.config"
        } else if replication {
            "dir.ethernet_redundancy.config"
        } else {
            "dir.ethernet_preemption.config"
        };
        if config["schema_name"] != schema {
            return Err(error("unknown network extension schema").with_target("/schema_name"));
        }
        integer(&config["schema_version"], 1, 1, "/schema_version")?;
        exact(&config["base"], &["profile", "config"], "extension base")
            .map_err(|d| d.with_target("/base"))?;
        if config["base"]["profile"] != "ethernet.tsn.v1" {
            return Err(
                error("extension base must be ethernet.tsn.v1").with_target("/base/profile")
            );
        }
        let raw = config["base"]["config"].to_string();
        let work = workload.to_string();
        let mut network = super::prepare_with_mac_scope(
            ethernet.clone(),
            config["base"]["config"].clone(),
            workload.clone(),
            "ethernet.tsn.v1",
            (&raw, config_source.1),
            Some((&work, workload_source.map_or(config_source.1, |s| s.1))),
            composite && config.get("provider_l2").is_some_and(|v| !v.is_null()),
            composite && config.get("mac_merge").is_some_and(|v| !v.is_null()),
        )
        .map_err(|d| {
            let target = d.target.clone().unwrap_or_default();
            d.with_target(format!("/base/config{target}"))
        })?;
        if let Some(policy) = config.get("physical_mac_policy") {
            if policy != "dir.experimental.half_duplex_tsn.v1"
                && policy != "dir.experimental.sampled_wire_cbs.v1"
            {
                return Err(error("unknown physical MAC shaping policy")
                    .with_target("/physical_mac_policy"));
            }
        }
        if composite {
            if let Some(media) = config.get("configured_media").filter(|v| !v.is_null()) {
                crate::input::ethernet::configure_sparse_media(media, &mut network.ethernet)
                    .map_err(|d| d.with_target("/configured_media"))?;
            }
        }
        network.config = config.clone();
        let distributed = config["replication"]["streams"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|s| s.get("placement").is_some()));
        if distributed {
            for (i, control) in array(&workload["controls"], "/controls")?
                .iter()
                .enumerate()
            {
                if control["kind"] != "link_set" {
                    return Err(error("bridge placement permits only link_set controls")
                        .with_target(format!("/controls/{i}")));
                }
            }
            for key in ["preemption"] {
                if config
                    .get(key)
                    .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                {
                    return Err(error("unsupported distributed placement composition")
                        .with_target(format!("/{key}")));
                }
            }
        }
        let provider_controls =
            composite && config.get("provider_l2").is_some_and(|v| !v.is_null());
        if provider_controls {
            for (i, control) in array(&workload["controls"], "/controls")?
                .iter()
                .enumerate()
            {
                if control["kind"] != "link_set" {
                    return Err(error("provider components permit only link_set controls")
                        .with_target(format!("/controls/{i}")));
                }
            }
        }
        let abstract_link_controls = composite
            && config.get("wire_control").is_some_and(|v| !v.is_null())
            && config
                .get("preemption")
                .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()));
        if abstract_link_controls {
            for (i, control) in array(&workload["controls"], "/controls")?
                .iter()
                .enumerate()
            {
                if control["kind"] != "link_set" {
                    return Err(
                        error("abstract Wire serializer permits only link_set controls")
                            .with_target(format!("/controls/{i}")),
                    );
                }
            }
        }
        static_base(
            &network,
            &workload,
            abstract_link_controls
                || provider_controls
                || config.get("gptp").is_some_and(|v| !v.is_null())
                || distributed
                || config.get("mac_merge").is_some_and(|v| !v.is_null())
                || composite
                    && (config.get("mac_control").is_some_and(|v| !v.is_null())
                        || config.get("configured_media").is_some_and(|v| !v.is_null())
                        || config.get("qcc").is_some_and(|v| !v.is_null())
                        || config.get("shared_lans").is_some_and(|v| !v.is_null())
                        || config.get("multicast_active").is_some_and(|v| !v.is_null())),
        )?;
        let limits = limits(&config["limits"])?;
        let mut extensions = PreparedNetworkExtensions {
            limits,
            replication: None,
            preemption: None,
            mac_merge: None,
        };
        if composite {
            // Resolve physical membership before explicit routes: their next
            // MAC can be another receiver on a shared electrical domain.
            if let Some(rows) = config.get("phy_attachments") {
                phy::prepare(rows, &mut network, &extensions)?;
            }
            if let Some(rows) = config.get("shared_lans").filter(|v| !v.is_null()) {
                network.shared_lans = shared_lans_config(rows, &network)?;
                for segment in &network.shared_lans {
                    let hub = network.ethernet.directions[segment.members[0]].destination;
                    if let Some(dynamic) = network.dynamic.as_mut() {
                        dynamic.bridges.remove(&network.ethernet.devices[hub].id);
                    }
                }
            }
            if let Some(value) = config.get("provider_l2").filter(|v| !v.is_null()) {
                network.provider_l2 = Some(provider_l2_config(value, &network)?);
            }
        }
        if replication && (!composite || !config["replication"].is_null()) {
            extensions.replication = Some(replication_config(&config[extension], &network, rtag)?);
        }
        if !replication || composite && !config["preemption"].is_null() {
            extensions.preemption = Some(preemption_config(
                &config["preemption"],
                &network,
                composite,
            )?);
        }
        if composite {
            if let Some(value) = config.get("multicast_active") {
                let active = super::multicast_active::prepare(value, &network.ethernet)?;
                network.wire = Some(super::multicast_active::synthesize_wire(
                    &active,
                    &network.ethernet,
                )?);
                network.multicast_active = Some(active);
            }
            if extensions.replication.is_none()
                && extensions.preemption.is_none()
                && config.get("mac_merge").is_none_or(Value::is_null)
                && network.multicast_active.is_none()
                && config.get("qcc").is_none_or(Value::is_null)
                && config.get("shared_lans").is_none_or(Value::is_null)
                && config.get("wire_control").is_none_or(Value::is_null)
                && config.get("provider_l2").is_none_or(Value::is_null)
                && config.get("configured_media").is_none_or(Value::is_null)
                && config.get("mac_control").is_none_or(Value::is_null)
                && config.get("clocks").is_none_or(Value::is_null)
                && config
                    .get("phy_attachments")
                    .is_none_or(|v| v.as_array().is_none_or(Vec::is_empty))
            {
                if config.get("gptp").is_some_and(|v| !v.is_null()) {
                    // Packet-derived clock discipline is an active extension.
                } else {
                    return Err(error("composition requires an active network extension")
                        .with_target("/replication"));
                }
            }
            if !config["clocks"].is_null() {
                let clocks = super::clocks::prepare(&config["clocks"], &network)?;
                network.clocks = Some(clocks);
            }
            if config.get("gptp").is_some_and(|v| !v.is_null()) && network.clocks.is_none() {
                network.clocks = Some(PreparedClocks {
                    initial: vec![
                        PreparedNodeClock {
                            offset_ps: 0,
                            rate_num: 1,
                            rate_den: 1
                        };
                        network.ethernet.devices.len()
                    ],
                    adjustments: vec![],
                });
            }
            if let Some(wire) = config.get("wire_control").filter(|v| !v.is_null()) {
                exact(
                    wire,
                    &["port_identities", "adapters", "packets"],
                    "wire_control",
                )
                .map_err(|d| d.with_target("/wire_control"))?;
                let wrapper = json!({"schema_name":"dir.ethernet_control_wire.config","schema_version":1,
                    "ethernet":config["base"]["config"]["base"]["config"],
                    "port_identities":wire["port_identities"],"adapters":wire["adapters"]});
                let work = json!({"schema_name":"dir.ethernet_control_wire.workload","schema_version":1,
                    "ethernet":workload,"packets":wire["packets"]});
                let mut wire_base = network.ethernet.clone();
                for segment in &network.shared_lans {
                    let hub = wire_base.directions[segment.members[0]].destination;
                    wire_base.devices[hub].kind = "endpoint".into();
                }
                for attachment in &network.phy_attachments {
                    if attachment.config.medium.half_duplex
                        && !attachment.config.physical_ports.is_empty()
                        && (attachment.directions.len() > 2
                            || attachment.config.medium.repeater.is_some())
                    {
                        let junction =
                            network.ethernet.directions[attachment.directions[0]].destination;
                        wire_base.devices[junction].kind = "endpoint".into();
                    }
                }
                let provider_for_wire = config
                    .get("provider_l2")
                    .filter(|v| !v.is_null())
                    .map(|v| {
                        serde_json::from_value::<
                            crate::runtime::network::provider_l2::ProviderTopology,
                        >(v.clone())
                        .map_err(|e| {
                            error(format!("invalid provider_l2: {e}")).with_target("/provider_l2")
                        })
                    })
                    .transpose()?;
                let explicit_wire = crate::input::wire::prepare_with_provider(
                    &wrapper,
                    &work,
                    &wire_base,
                    provider_for_wire.as_ref(),
                )
                .map_err(|d| {
                    let target = d.target.clone().unwrap_or_default();
                    d.with_target(format!("/wire_control{target}"))
                })?;
                network.wire = Some(match network.wire.take() {
                    Some(active) => super::wire_merge::merge(active, explicit_wire)
                        .map_err(|d| d.with_target("/wire_control"))?,
                    None => explicit_wire,
                });
            }
        }
        if let Some(replication) = &extensions.replication {
            if replication
                .streams
                .iter()
                .any(|s| s.paths.len() > extensions.limits.max_pending_branches as usize)
            {
                return Err(error("one origin exceeds max_pending_branches")
                    .with_target("/limits/max_pending_branches"));
            }
        }
        let initial = network.tsn.as_ref().map_or(0, |t| t.updates.len())
            + usize::from(
                network
                    .ethernet
                    .generators
                    .iter()
                    .any(|g| g.time(0).is_some()),
            );
        if initial > extensions.limits.max_control_events as usize {
            return Err(error("initial reservations exceed max_control_events")
                .with_target("/limits/max_control_events"));
        }
        network.extensions = Some(extensions);
        if composite {
            if let Some(value) = config.get("provider_l2").filter(|v| !v.is_null()) {
                network.provider_l2 = Some(provider_l2_config(value, &network)?);
            }
        }
        if let Some(value) = config.get("mac_merge").filter(|v| !v.is_null()) {
            let prepared = mac_merge::prepare(
                value,
                &network,
                &workload,
                composite && config.get("gptp").is_some_and(|v| !v.is_null()),
            )?;
            network.extensions.as_mut().unwrap().mac_merge = Some(prepared);
        }
        if composite {
            if let Some(value) = config.get("gptp").filter(|v| !v.is_null()) {
                network.gptp = Some(gptp::prepare(value, &network, &workload)?);
            }
        }
        if composite {
            if let Some(value) = config.get("mac_control").filter(|v| !v.is_null()) {
                network.mac_control = Some(mac_control::prepare(value, &network)?);
                mac_control::ensure_fifo(&mut network);
            }
            network.bindings = Some(super::bindings::bind(&network)?);
            if let Some(value) = config.get("qcc").filter(|v| !v.is_null()) {
                network.qcc = Some(super::configuration::prepare(value, &network)?);
                // Admission resolves physical domains and schedule owners from
                // the complete graph; publish its claims in the same table.
                network.bindings = Some(super::bindings::bind(&network)?);
            }
        }
        network.config = config.clone();
        network.workload = workload.clone();
        Ok(network)
    };
    prepare_inner().map_err(|d| {
        let pointer = d.target.clone().unwrap_or_default();
        doc.annotate(&pointer, d).with_source(config_source.1)
    })
}
fn provider_l2_config(
    v: &Value,
    network: &PreparedNetwork,
) -> Result<crate::runtime::network::provider_l2::ProviderTopology> {
    use crate::runtime::network::provider_l2::{PortRole, ProviderTopology};
    let topology: ProviderTopology = serde_json::from_value(v.clone())
        .map_err(|e| error(format!("invalid provider_l2: {e}")).with_target("/provider_l2"))?;
    topology
        .validate()
        .map_err(|e| e.with_target("/provider_l2"))?;
    if network.wire.as_ref().is_some_and(|w| {
        w.adapters.iter().any(|a| {
            a.component_binding.is_none() && a.ports.iter().any(|p| topology.ports.contains_key(p))
        })
    }) {
        return Err(error(
            "provider output needs its forwarding adapter and component-bound control recipient",
        )
        .with_target("/provider_l2"));
    }
    let mut physical = BTreeSet::new();
    for (name, port) in &topology.ports {
        match port.role {
            PortRole::CustomerEdge | PortRole::ProviderNetwork => {
                let direction = network
                    .ethernet
                    .directions
                    .iter()
                    .find(|d| d.from_port == *name)
                    .ok_or_else(|| {
                        error("provider physical port is not a resolved Ethernet output")
                            .with_target("/provider_l2/ports")
                    })?;
                if network.ethernet.devices[direction.source].kind != "switch" {
                    return Err(error("provider physical port must be owned by a bridge")
                        .with_target("/provider_l2/ports"));
                }
                let expected = if port.role == PortRole::CustomerEdge {
                    "endpoint"
                } else {
                    "switch"
                };
                if network.ethernet.devices[direction.destination].kind != expected {
                    return Err(error("provider port role does not match the physical peer")
                        .with_target("/provider_l2/ports"));
                }
                physical.insert(name.clone());
            }
            PortRole::ProviderEdge | PortRole::CustomerNetwork => {
                if network
                    .ethernet
                    .directions
                    .iter()
                    .any(|d| d.from_port == *name)
                {
                    return Err(
                        error("PEP/CNP is an internal handoff, not a physical output")
                            .with_target("/provider_l2/ports"),
                    );
                }
            }
        }
    }
    let provider_devices: BTreeSet<_> = network
        .ethernet
        .directions
        .iter()
        .filter(|direction| physical.contains(&direction.from_port))
        .map(|direction| direction.source)
        .collect();
    for direction in &network.ethernet.directions {
        if network.ethernet.devices[direction.source].kind == "switch"
            && provider_devices.contains(&direction.source)
            && !physical.contains(&direction.from_port)
        {
            return Err(
                error("provider bridge output lacks a component/role binding")
                    .with_target("/provider_l2/ports"),
            );
        }
    }
    let mut endpoint_macs = BTreeSet::new();
    for (index, endpoint) in network.ethernet.devices.iter().enumerate() {
        if endpoint.kind != "endpoint" {
            continue;
        }
        let directions: Vec<_> = network
            .ethernet
            .directions
            .iter()
            .filter(|d| d.source == index)
            .collect();
        if !directions.iter().any(|direction| {
            let peer = super::super::ethernet::paired(&direction.to_port);
            topology.ports.contains_key(&peer)
        }) {
            continue;
        }
        if directions.len() != 1 {
            return Err(
                error("provider customer endpoint requires one physical attachment")
                    .with_target("/provider_l2/ports"),
            );
        }
        let peer = network
            .ethernet
            .directions
            .iter()
            .find(|d| d.from_port == super::super::ethernet::paired(&directions[0].to_port));
        let peer = peer.ok_or_else(|| {
            error("provider endpoint lacks reverse PE port").with_target("/provider_l2/ports")
        })?;
        let port = topology.ports.get(&peer.from_port).ok_or_else(|| {
            error("provider endpoint lacks CEP binding").with_target("/provider_l2/ports")
        })?;
        if port.role != PortRole::CustomerEdge
            || !endpoint_macs.insert((port.component.clone(), endpoint.mac.clone()))
        {
            return Err(
                error("provider endpoint MAC must be unique within its C component")
                    .with_target("/provider_l2/ports"),
            );
        }
    }
    Ok(topology)
}
fn shared_lans_config(v: &Value, network: &PreparedNetwork) -> Result<Vec<PreparedSharedLan>> {
    let rows = array(v, "/shared_lans")?;
    if rows.is_empty() || rows.len() > 8 {
        return Err(error("shared_lans requires 1..8 segments").with_target("/shared_lans"));
    }
    let mut ids = BTreeSet::new();
    let mut claimed = BTreeSet::new();
    let mut segments = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let ptr = format!("/shared_lans/{i}");
        exact(
            row,
            &["id", "ports", "bitrate_bps", "delay_ps"],
            "shared LAN",
        )
        .map_err(|d| d.with_target(&ptr))?;
        let id = string(&row["id"], &format!("{ptr}/id"))?;
        if !ids.insert(id.to_owned()) {
            return Err(error("duplicate shared LAN ID").with_target(&ptr));
        }
        let ports = array(&row["ports"], &format!("{ptr}/ports"))?;
        if ports.len() < 3 || ports.len() > 64 {
            return Err(error("shared LAN requires 3..64 physical ports").with_target(&ptr));
        }
        let bitrate = decimal(&row["bitrate_bps"], &format!("{ptr}/bitrate_bps"))?;
        let delay = decimal(&row["delay_ps"], &format!("{ptr}/delay_ps"))?;
        if bitrate == 0 {
            return Err(error("shared LAN bitrate must be positive").with_target(&ptr));
        }
        let mut members = Vec::new();
        let mut hub = None;
        for (j, value) in ports.iter().enumerate() {
            let port = string(value, &format!("{ptr}/ports/{j}"))?;
            let direction = network
                .ethernet
                .directions
                .iter()
                .position(|d| d.from_port == port)
                .ok_or_else(|| {
                    error("shared LAN port has no physical carrier direction")
                        .with_target(format!("{ptr}/ports/{j}"))
                })?;
            let link = &network.ethernet.directions[direction];
            if !claimed.insert(direction) || link.bitrate_bps != bitrate || link.delay_ps != delay {
                return Err(
                    error("shared LAN port duplicated or carrier bitrate/delay differs")
                        .with_target(format!("{ptr}/ports/{j}")),
                );
            }
            if network.ethernet.devices[link.source].kind != "switch"
                || network.ethernet.devices[link.destination].kind != "switch"
            {
                return Err(
                    error("shared LAN carrier must connect a bridge port to a hub switch")
                        .with_target(format!("{ptr}/ports/{j}")),
                );
            }
            if hub.is_some_and(|previous| previous != link.destination) {
                return Err(
                    error("shared LAN members must connect to one NED hub").with_target(&ptr)
                );
            }
            hub = Some(link.destination);
            members.push(direction);
        }
        let _hub = hub.unwrap();
        segments.push(PreparedSharedLan {
            id: id.to_owned(),
            members,
            bitrate_bps: bitrate,
            delay_ps: delay,
        });
    }
    Ok(segments)
}
fn replication_config(v: &Value, n: &PreparedNetwork, rtag: bool) -> Result<PreparedReplication> {
    exact(v, &["streams"], "replication").map_err(|d| d.with_target("/replication"))?;
    let rows = array(&v["streams"], "/replication/streams")?;
    if rows.is_empty() {
        return Err(
            error("replication requires at least one stream").with_target("/replication/streams")
        );
    }
    let mut streams = Vec::new();
    let mut ids = BTreeSet::new();
    let mut generators = BTreeSet::new();
    let mut use_count = BTreeMap::<String, usize>::new();
    for (i, row) in rows.iter().enumerate() {
        let p = format!("/replication/streams/{i}");
        let mut fields = vec![
            "stream_id",
            if row.get("source").is_some() {
                "source"
            } else {
                "source_generator"
            },
            "replicate_at",
            "eliminate_at",
            "history_capacity",
            "egress_paths",
        ];
        if rtag {
            fields.extend(["wire_format", "recovery_timeout_ps"]);
        }
        if row.get("placement").is_some() {
            fields.push("placement");
        }
        if row.get("recovery_algorithm").is_some() {
            fields.push("recovery_algorithm");
        }
        exact(row, &fields, "replication stream").map_err(|d| d.with_target(&p))?;
        if rtag && row["wire_format"] != "rtag_8021cb_2017" {
            return Err(error("wire_format must be rtag_8021cb_2017")
                .with_target(format!("{p}/wire_format")));
        }
        let id = string(&row["stream_id"], &format!("{p}/stream_id"))?;
        let selector: ReplicationSource = if let Some(value) = row.get("source") {
            serde_json::from_value(value.clone()).map_err(|e| {
                error(format!("invalid replication source: {e}")).with_target(format!("{p}/source"))
            })?
        } else {
            ReplicationSource::Generator {
                id: string(&row["source_generator"], &format!("{p}/source_generator"))?.into(),
            }
        };
        if !ids.insert(id.to_owned()) || !generators.insert(selector.clone()) {
            return Err(error("duplicate stream or replication source").with_target(&p));
        }
        let (
            generator,
            source_device,
            source_port,
            source_vid,
            dst_mac,
            source_wire,
            source_priority,
        ) = match &selector {
            ReplicationSource::Generator { id } => {
                let g = n
                    .ethernet
                    .generators
                    .iter()
                    .find(|g| g.id == *id)
                    .ok_or_else(|| {
                        error("unknown source generator")
                            .with_target(format!("{p}/source_generator"))
                    })?;
                if n.dynamic
                    .as_ref()
                    .unwrap()
                    .generator_ip
                    .get(id)
                    .is_some_and(Option::is_some)
                {
                    return Err(error("replication requires null IP multicast").with_target(&p));
                }
                (
                    id.clone(),
                    g.source,
                    None,
                    g.source_vlan_id.unwrap(),
                    g.frame.dst_mac.clone(),
                    g.frame.clone(),
                    g.priority,
                )
            }
            ReplicationSource::GatewayEgress {
                gateway_id,
                rule_id,
                egress_index,
            } => {
                let gw = n
                    .bridge
                    .as_ref()
                    .and_then(|b| b.gateways.iter().find(|g| g.instance == *gateway_id))
                    .ok_or_else(|| {
                        error("unknown Gateway replication source")
                            .with_target(format!("{p}/source"))
                    })?;
                let rule = gw.rules.iter().find(|r| r.id == *rule_id).ok_or_else(|| {
                    error("unknown Gateway rule").with_target(format!("{p}/source"))
                })?;
                if rule.direction != "can_to_ethernet" {
                    return Err(error("Gateway FRER source requires CAN ingress").with_target(&p));
                }
                let Some(crate::types::can_ethernet::BridgeEgress::Ethernet {
                    source,
                    port,
                    dst_mac,
                    vid,
                    pcp,
                }) = rule.egresses.get(*egress_index)
                else {
                    return Err(
                        error("Gateway FRER source requires an Ethernet egress index")
                            .with_target(&p),
                    );
                };
                // Classification depends on this declared header, never on the
                // CAN payload. This representative packet is preparation-only;
                // runtime serialization still uses the actual received CAN data.
                let policy = n
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|policy| policy.port == *port)
                    .ok_or_else(|| error("Gateway source policy missing").with_target(&p))?;
                let source_wire = crate::runtime::ethernet::serialize_vlan_frame(
                    n.ethernet.devices[*source]
                        .mac
                        .as_deref()
                        .ok_or_else(|| error("Gateway source MAC missing").with_target(&p))?,
                    dst_mac,
                    crate::runtime::can_ethernet::protocol::ETHERTYPE,
                    "",
                    policy.vlans.get(vid).copied().unwrap_or(false).then_some(
                        crate::types::ethernet::EthernetVlanTag {
                            vid: *vid,
                            pcp: *pcp,
                            dei: 0,
                        },
                    ),
                )?;
                (
                    String::new(),
                    *source,
                    Some(port.clone()),
                    *vid,
                    dst_mac.clone(),
                    source_wire,
                    *pcp,
                )
            }
        };
        let source = string(&row["replicate_at"], &format!("{p}/replicate_at"))?;
        let destination = string(&row["eliminate_at"], &format!("{p}/eliminate_at"))?;
        let placement = row.get("placement");
        if placement.is_some() && !rtag {
            return Err(
                error("bridge placement requires R-TAG").with_target(format!("{p}/placement"))
            );
        }
        let source_index = if placement.is_some() {
            n.ethernet
                .devices
                .iter()
                .position(|d| d.id == source && d.kind == "switch")
                .ok_or_else(|| {
                    error("replicate_at must be switch").with_target(format!("{p}/replicate_at"))
                })?
        } else {
            source_device
        };
        let destination_index = n
            .ethernet
            .devices
            .iter()
            .position(|d| {
                d.id == destination
                    && d.kind
                        == if placement.is_some() {
                            "switch"
                        } else {
                            "endpoint"
                        }
            })
            .ok_or_else(|| {
                error("eliminate_at must be endpoint").with_target(format!("{p}/eliminate_at"))
            })?;
        let final_destination = if let Some(placement) = placement {
            exact(
                placement,
                &[
                    "kind",
                    "forwarding",
                    "prefix_ports",
                    "suffix_ports",
                    "destination",
                ],
                "bridge placement",
            )
            .map_err(|d| d.with_target(format!("{p}/placement")))?;
            if placement["kind"] != "bridge_pair_v1"
                || placement["forwarding"] != "explicit_physical_route_v1"
            {
                return Err(error("unsupported placement or forwarding adapter")
                    .with_target(format!("{p}/placement")));
            }
            n.ethernet
                .devices
                .iter()
                .position(|d| d.id == placement["destination"] && d.kind == "endpoint")
                .ok_or_else(|| {
                    error("destination must be endpoint")
                        .with_target(format!("{p}/placement/destination"))
                })?
        } else {
            destination_index
        };
        if n.ethernet.devices[source_index].id != source
            || source == destination
            || n.ethernet.devices[final_destination].mac.as_ref() != Some(&dst_mac)
            || source_device == final_destination
            || n.ethernet.devices[source_device].kind != "endpoint"
        {
            return Err(error("replication requires generator source and distinct individual destination endpoint with null IP multicast").with_target(&p));
        }
        let recovery_algorithm = match row.get("recovery_algorithm").and_then(Value::as_str) {
            None if row.get("recovery_algorithm").is_none() => {
                ReplicationRecoveryAlgorithm::ProjectHistoryV1
            }
            Some("project_history_v1") => ReplicationRecoveryAlgorithm::ProjectHistoryV1,
            Some("vector_8021cb_2017") if rtag => ReplicationRecoveryAlgorithm::Vector8021cb2017,
            _ => {
                return Err(error("unsupported recovery_algorithm or R-TAG required")
                    .with_target(format!("{p}/recovery_algorithm")));
            }
        };
        let normative = recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017;
        let history_capacity = integer(
            &row["history_capacity"],
            if normative { 2 } else { 1 },
            if rtag { 32767 } else { 65536 },
            &format!("{p}/history_capacity"),
        )? as usize;
        let recovery_timeout_ps = if rtag {
            let timeout = decimal(
                &row["recovery_timeout_ps"],
                &format!("{p}/recovery_timeout_ps"),
            )?;
            if timeout == 0 {
                return Err(error("recovery_timeout_ps must be positive")
                    .with_target(format!("{p}/recovery_timeout_ps")));
            }
            if normative && timeout % 1_000_000_000 != 0 {
                return Err(
                    error("vector recovery timeout must be integral milliseconds")
                        .with_target(format!("{p}/recovery_timeout_ps")),
                );
            }
            Some(timeout)
        } else {
            None
        };
        let paths = array(&row["egress_paths"], &format!("{p}/egress_paths"))?;
        if !(2..=16).contains(&paths.len()) {
            return Err(error("path count must be 2..16").with_target(format!("{p}/egress_paths")));
        }
        let mut prepared_paths = Vec::new();
        let mut path_ids = BTreeSet::new();
        let mut path_ports = BTreeSet::new();
        for (j, path) in paths.iter().enumerate() {
            let pp = format!("{p}/egress_paths/{j}");
            exact(path, &["id", "ports"], "replication path").map_err(|d| d.with_target(&pp))?;
            let path_id = string(&path["id"], &format!("{pp}/id"))?;
            if !path_ids.insert(path_id.to_owned()) {
                return Err(error("duplicate path ID").with_target(&pp));
            }
            let route = physical_route(
                &path["ports"],
                n,
                source_index,
                destination_index,
                source_vid,
                &pp,
            )?;
            if !path_ports.insert(route.ports.clone()) {
                return Err(error("duplicate replication path").with_target(&pp));
            }
            for port in &route.ports {
                *use_count.entry(port.clone()).or_default() += 1;
            }
            prepared_paths.push(ReplicationPath {
                id: path_id.into(),
                ports: route.ports,
                endpoints: route.endpoints,
                directions: route.directions,
            });
        }
        prepared_paths.sort_by(|a, b| a.id.cmp(&b.id));
        let placement = if let Some(placement) = placement {
            let prefix = physical_route(
                &placement["prefix_ports"],
                n,
                source_device,
                source_index,
                source_vid,
                &format!("{p}/placement/prefix_ports"),
            )?;
            let suffix = physical_route(
                &placement["suffix_ports"],
                n,
                destination_index,
                final_destination,
                source_vid,
                &format!("{p}/placement/suffix_ports"),
            )?;
            for branch in &prepared_paths {
                let mut visited = BTreeSet::from([source_device]);
                for &(_, destination) in prefix
                    .endpoints
                    .iter()
                    .chain(&branch.endpoints)
                    .chain(&suffix.endpoints)
                {
                    if !visited.insert(destination) {
                        return Err(error("complete distributed route revisits device")
                            .with_target(format!("{p}/placement")));
                    }
                }
            }
            ReplicationPlacement::BridgePair(PreparedBridgePair {
                replicate_device: source_index,
                eliminate_device: destination_index,
                destination_device: final_destination,
                prefix,
                suffix,
            })
        } else {
            ReplicationPlacement::LegacyEndpoint
        };
        if let ReplicationSource::GatewayEgress { .. } = selector {
            let ReplicationPlacement::BridgePair(pair) = &placement else {
                return Err(
                    error("Gateway replication requires bridge-pair placement").with_target(&p)
                );
            };
            if n.bridge.as_ref().is_some_and(|b| {
                b.gateways
                    .iter()
                    .any(|g| g.ethernet_endpoint == pair.destination_device)
            }) {
                return Err(error(
                    "Gateway FRER terminal destination must be an external endpoint",
                )
                .with_target(&p));
            }
            if recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
                || pair.prefix.ports.len() != 1
                || source_port.as_ref() != pair.prefix.ports.first()
                || n.ethernet
                    .directions
                    .iter()
                    .filter(|d| d.source == source_device)
                    .count()
                    != 1
            {
                return Err(error(
                    "Gateway replication requires vector recovery and one physical prefix egress",
                )
                .with_target(&p));
            }
        }
        if n.provider_l2.is_some() {
            let initial = crate::runtime::ethernet::dynamic::DynamicState::new(
                n.dynamic.as_ref().unwrap(),
                &n.ethernet,
            )?
            .snapshot_policy();
            let mut paths = Vec::new();
            let mut receivers = BTreeMap::<usize, BTreeSet<usize>>::new();
            for branch in &prepared_paths {
                let (mut directions, mut endpoints) = (Vec::new(), Vec::new());
                if let ReplicationPlacement::BridgePair(pair) = &placement {
                    directions.extend_from_slice(&pair.prefix.directions);
                    endpoints.extend_from_slice(&pair.prefix.endpoints);
                }
                directions.extend_from_slice(&branch.directions);
                endpoints.extend_from_slice(&branch.endpoints);
                if let ReplicationPlacement::BridgePair(pair) = &placement {
                    directions.extend_from_slice(&pair.suffix.directions);
                    endpoints.extend_from_slice(&pair.suffix.endpoints);
                }
                for (&direction, &(_, receiver)) in directions.iter().zip(&endpoints) {
                    receivers.entry(direction).or_default().insert(receiver);
                }
                paths.push(directions);
            }
            let (classes, ingress) = super::configuration::hop_classes(
                n,
                &paths,
                &receivers,
                &source_wire,
                source_vid,
                source_priority,
            )
            .map_err(|_| {
                error("replication provider route has invalid or ambiguous mapped classification")
                    .with_target(&p)
            })?;
            for (direction, (_, vid)) in classes {
                let port = &n.ethernet.directions[direction].from_port;
                if !initial
                    .effective_vlans
                    .get(port)
                    .is_some_and(|allowed| allowed.contains_key(&vid))
                {
                    return Err(error("replication mapped egress VID not initially allowed")
                        .with_target(&p));
                }
            }
            for key in ingress {
                let port = n
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|policy| policy.ingress == key.ingress)
                    .ok_or_else(|| {
                        error("replication mapped ingress policy missing").with_target(&p)
                    })?;
                if !initial
                    .effective_vlans
                    .get(&port.port)
                    .is_some_and(|allowed| allowed.contains_key(&key.vid))
                {
                    return Err(
                        error("replication mapped ingress VID not initially allowed")
                            .with_target(&p),
                    );
                }
            }
        }
        let mut initial_history_words = Vec::new();
        initial_history_words
            .try_reserve_exact(history_capacity.div_ceil(64))
            .map_err(|_| {
                error("history bitmap allocation failed")
                    .with_target(format!("{p}/history_capacity"))
            })?;
        initial_history_words.resize(history_capacity.div_ceil(64), 0);
        streams.push(ReplicationStream {
            stream_id: id.into(),
            source_generator: generator,
            source: selector,
            source_device,
            source_port,
            replicate_at: source.into(),
            eliminate_at: destination.into(),
            history_capacity,
            wire_format: if rtag {
                ReplicationWireFormat::Rtag8021cb2017
            } else {
                ReplicationWireFormat::Abstract
            },
            recovery_timeout_ps,
            recovery_algorithm,
            initial_history_words,
            paths: prepared_paths,
            placement,
        });
    }
    streams.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
    let generator_stream = streams
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match &s.source {
            ReplicationSource::Generator { id } => Some((id.clone(), i)),
            _ => None,
        })
        .collect();
    Ok(PreparedReplication {
        streams,
        generator_stream,
        shared_ports: use_count
            .into_iter()
            .filter_map(|(p, n)| (n > 1).then_some(p))
            .collect(),
    })
}
fn physical_route(
    v: &Value,
    n: &PreparedNetwork,
    source: usize,
    destination: usize,
    vid: u16,
    pointer: &str,
) -> Result<PreparedPhysicalRoute> {
    let initial = crate::runtime::ethernet::dynamic::DynamicState::new(
        n.dynamic.as_ref().unwrap(),
        &n.ethernet,
    )?
    .snapshot_policy();
    let ports = array(v, pointer)?
        .iter()
        .map(|port| string(port, pointer).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let directions = ports
        .iter()
        .map(|port| {
            n.ethernet
                .directions
                .iter()
                .position(|d| &d.from_port == port)
                .ok_or_else(|| error("unknown physical output").with_target(pointer))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut current = source;
    let mut visited = BTreeSet::from([source]);
    let mut endpoints = Vec::new();
    for (hop, &dir) in directions.iter().enumerate() {
        let link = &n.ethernet.directions[dir];
        let intended = directions
            .get(hop + 1)
            .map(|next| n.ethernet.directions[*next].source)
            .unwrap_or(destination);
        let ingress = logical_receiving_port(n, dir, intended, pointer)?;
        if link.source != current
            || !visited.insert(intended)
            || current != source && n.ethernet.devices[current].kind != "switch"
        {
            return Err(
                error("physical route disconnected, cyclic or traverses endpoint")
                    .with_target(pointer),
            );
        }
        for policy in n
            .ethernet
            .port_policies
            .iter()
            .filter(|p| p.port == link.from_port || p.ingress == ingress)
        {
            if n.provider_l2.is_none()
                && !initial
                    .effective_vlans
                    .get(&policy.port)
                    .is_some_and(|v| v.contains_key(&vid))
            {
                return Err(error("route VID not initially allowed").with_target(pointer));
            }
        }
        if !initial
            .link_up
            .get(&link.from_port)
            .copied()
            .unwrap_or(true)
        {
            return Err(error("route link initially down").with_target(pointer));
        }
        endpoints.push((current, intended));
        current = intended;
    }
    if directions.is_empty() || current != destination {
        return Err(error("physical route empty or wrong destination").with_target(pointer));
    }
    Ok(PreparedPhysicalRoute {
        ports,
        endpoints,
        directions,
    })
}

/// Resolve a declared next MAC across a physical fanout. The NED junction is
/// never inserted as a fictitious store-and-forward bridge in an explicit path.
pub(super) fn logical_receiving_port(
    n: &PreparedNetwork,
    direction: usize,
    intended: usize,
    pointer: &str,
) -> Result<String> {
    let physical_junction = n
        .shared_lans
        .iter()
        .any(|segment| n.ethernet.directions[segment.members[0]].destination == intended)
        || n.phy_attachments.iter().any(|attachment| {
            attachment.config.physical_ports.len() > 2
                && !attachment.directions.is_empty()
                && attachment
                    .directions
                    .iter()
                    .all(|direction| n.ethernet.directions[*direction].destination == intended)
        });
    if physical_junction {
        return Err(
            error("explicit route names physical junction instead of receiving MAC")
                .with_target(pointer),
        );
    }
    let link = &n.ethernet.directions[direction];
    if link.destination == intended {
        return Ok(link.to_port.clone());
    }
    let members: Vec<_> = n
        .shared_lans
        .iter()
        .find(|s| s.members.contains(&direction))
        .map(|segment| {
            segment
                .members
                .iter()
                .copied()
                .filter(|receiver| {
                    *receiver != direction && n.ethernet.directions[*receiver].source == intended
                })
                .collect()
        })
        .unwrap_or_default();
    if members.len() > 1 {
        return Err(error("explicit route has ambiguous shared receiving MAC").with_target(pointer));
    }
    let member = members.first().copied();
    if let Some(receiver) = member {
        let output = &n.ethernet.directions[receiver].from_port;
        let policy = n
            .ethernet
            .port_policies
            .iter()
            .find(|p| &p.port == output)
            .ok_or_else(|| {
                error("shared route receiver lacks ingress policy").with_target(pointer)
            })?;
        return Ok(policy.ingress.clone());
    }
    for attachment in &n.phy_attachments {
        if !attachment.directions.contains(&direction)
            || attachment.config.physical_ports.is_empty()
        {
            continue;
        }
        let candidates: Vec<_> = attachment
            .config
            .physical_ports
            .iter()
            .filter(|port| {
                n.ethernet
                    .directions
                    .iter()
                    .any(|d| d.source == intended && d.from_port == port.tx_port)
            })
            .collect();
        if candidates.len() > 1 {
            return Err(
                error("explicit route has ambiguous physical receiving MAC").with_target(pointer)
            );
        }
        if let Some(port) = candidates.first() {
            return Ok(port.rx_port.clone());
        }
    }
    Err(error("explicit physical route does not reach intended receiving MAC").with_target(pointer))
}

fn mask(v: &Value, p: &str) -> Result<u8> {
    let mut mask = 0u8;
    for c in array(v, p)? {
        let c = integer(c, 0, 7, p)?;
        let bit = 1 << c;
        if mask & bit != 0 {
            return Err(error("duplicate class").with_target(p));
        }
        mask |= bit;
    }
    Ok(mask)
}
fn preemption_config(v: &Value, n: &PreparedNetwork, sparse: bool) -> Result<PreparedPreemption> {
    exact(v, &["outputs"], "preemption").map_err(|d| d.with_target("/preemption"))?;
    let mut ports = BTreeMap::new();
    for (i, row) in array(&v["outputs"], "/preemption/outputs")?
        .iter()
        .enumerate()
    {
        let p = format!("/preemption/outputs/{i}");
        exact(
            row,
            &[
                "port",
                "express_classes",
                "preemptable_classes",
                "fragment_payload_bytes",
                "fragment_overhead_bytes",
                "resume_gap_ps",
            ],
            "preemption output",
        )
        .map_err(|d| d.with_target(&p))?;
        let port = string(&row["port"], &format!("{p}/port"))?;
        let express_mask = mask(&row["express_classes"], &format!("{p}/express_classes"))?;
        let preemptable_mask = mask(
            &row["preemptable_classes"],
            &format!("{p}/preemptable_classes"),
        )?;
        if express_mask & preemptable_mask != 0 || express_mask | preemptable_mask != 255 {
            return Err(error("classes must partition 0..7").with_target(&p));
        }
        let tsn = n
            .tsn
            .as_ref()
            .unwrap()
            .outputs
            .iter()
            .find(|o| o.port == port)
            .ok_or_else(|| error("unknown preemption output").with_target(&p))?;
        if tsn
            .cbs
            .iter()
            .enumerate()
            .any(|(c, v)| v.is_some() && preemptable_mask & (1 << c) != 0)
        {
            return Err(error("CBS on preemptable class is unsupported").with_target(&p));
        }
        let output = PreemptionOutput {
            port: port.into(),
            express_mask,
            preemptable_mask,
            fragment_payload_bytes: integer(
                &row["fragment_payload_bytes"],
                1,
                u32::MAX.into(),
                &format!("{p}/fragment_payload_bytes"),
            )?,
            fragment_overhead_bytes: integer(
                &row["fragment_overhead_bytes"],
                0,
                u32::MAX.into(),
                &format!("{p}/fragment_overhead_bytes"),
            )?,
            resume_gap_ps: decimal(&row["resume_gap_ps"], &format!("{p}/resume_gap_ps"))?,
        };
        if ports.insert(port.to_owned(), output).is_some() {
            return Err(error("duplicate preemption output").with_target(&p));
        }
    }
    if ports.is_empty() {
        return Err(
            error("preemption requires at least one output").with_target("/preemption/outputs")
        );
    }
    let participating = n
        .ethernet
        .directions
        .iter()
        .map(|d| ports.contains_key(&d.from_port))
        .collect();
    let outputs = n
        .ethernet
        .directions
        .iter()
        .map(|d| {
            ports.remove(&d.from_port).map(Ok).unwrap_or_else(|| {
                if sparse {
                    Ok(PreemptionOutput {
                        port: d.from_port.clone(),
                        express_mask: 255,
                        preemptable_mask: 0,
                        fragment_payload_bytes: 1,
                        fragment_overhead_bytes: 0,
                        resume_gap_ps: 0,
                    })
                } else {
                    Err(error("every output must configure preemption")
                        .with_target("/preemption/outputs"))
                }
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if !ports.is_empty() {
        return Err(error("unknown preemption output").with_target("/preemption/outputs"));
    }
    for (d, o) in n.ethernet.directions.iter().zip(&outputs) {
        let bytes = o
            .fragment_payload_bytes
            .checked_add(o.fragment_overhead_bytes)
            .ok_or_else(|| error("fragment bytes overflow"))?;
        let duration =
            (u128::from(bytes) * 8 * 1_000_000_000_000).div_ceil(u128::from(d.bitrate_bps));
        if duration > u128::from(u64::MAX) {
            return Err(error("fragment duration exceeds u64").with_target("/preemption/outputs"));
        }
    }
    Ok(PreparedPreemption {
        outputs,
        participating,
    })
}
pub(crate) fn prepare_composite(
    ethernet: PreparedEthernet,
    can: PreparedCan,
    paths: &[String],
    config: Value,
    workload: Value,
    config_source: (&str, &Path),
    workload_source: Option<(&str, &Path)>,
) -> Result<PreparedNetwork> {
    let doc = crate::input::JsonDocument::parse(config_source.0)?;
    let workload_doc = workload_source
        .map(|(text, _)| crate::input::JsonDocument::parse(text))
        .transpose()?;
    let inner = || -> Result<PreparedNetwork> {
        let mut fields = vec![
            "schema_name",
            "schema_version",
            "bridge",
            "ethernet",
            "tsn",
            "limits",
        ];
        for key in ["replication", "mac_merge"] {
            if config.get(key).is_some() {
                fields.push(key);
            }
        }
        exact(&config, &fields, "composite configuration")?;
        if config["schema_name"] != "dir.can_ethernet_tsn.config" {
            return Err(error("unknown composite schema").with_target("/schema_name"));
        }
        integer(&config["schema_version"], 1, 1, "/schema_version")?;
        exact(
            &workload,
            &["schema_version", "can", "ethernet"],
            "composite workload",
        )?;
        integer(&workload["schema_version"], 1, 1, "/schema_version")?;
        let limits = limits(&config["limits"])?;
        let adapter = json!({"schema_version":1,"base":{"profile":"ethernet.l2.dynamic.v1","config":config["ethernet"]},"tsn":config["tsn"]});
        let text = adapter.to_string();
        let work = workload["ethernet"].to_string();
        let mut n = super::prepare_with_mac_scope(
            ethernet.clone(),
            adapter,
            workload["ethernet"].clone(),
            "ethernet.tsn.v1",
            (&text, config_source.1),
            Some((&work, workload_source.map_or(config_source.1, |s| s.1))),
            false,
            config.get("mac_merge").is_some_and(|v| !v.is_null()),
        )
        .map_err(|d| {
            let target = d.target.clone().unwrap_or_default();
            if workload_source.is_some_and(|(_, path)| {
                d.source.as_deref() == Some(path.to_string_lossy().as_ref())
            }) {
                d.with_target(format!("/ethernet{target}"))
            } else {
                d.with_target(if let Some(t) = target.strip_prefix("/base/config") {
                    format!("/ethernet{t}")
                } else {
                    target
                })
            }
        })?;
        let mut bridge = crate::input::can_ethernet::configure_projected(
            &config["bridge"],
            &can,
            &mut n.ethernet,
            paths,
        )
        .map_err(|d| {
            let target = d.target.clone().unwrap_or_default();
            if target.starts_with("/bridge") {
                d
            } else {
                d.with_target(format!("/bridge{target}"))
            }
        })?;
        bridge.can.generators = crate::input::can::workload(
            &workload["can"].to_string(),
            &bridge.can.controllers,
            &bridge.can.controller_buses,
            "can.cc.multibus.v1",
        )
        .map_err(|d| {
            let target = d.target.clone().unwrap_or_default();
            let d = d.with_target(format!("/can{target}"));
            if let Some((_, path)) = workload_source {
                d.with_source(path)
            } else {
                d
            }
        })?;
        if bridge.can.generators.iter().any(|g| {
            bridge
                .gateways
                .iter()
                .any(|gw| gw.can_ports.contains(&g.source))
        }) || n.ethernet.generators.iter().any(|g| {
            bridge
                .gateways
                .iter()
                .any(|gw| gw.ethernet_endpoint == g.source)
        }) {
            return Err(error("Gateway owned source cannot have native generator")
                .with_target("/generators"));
        }
        if bridge
            .gateways
            .iter()
            .flat_map(|g| &g.rules)
            .any(|r| r.egresses.len() > limits.max_pending_branches as usize)
        {
            return Err(error("static branch fanout exceeds pending limit")
                .with_target("/limits/max_pending_branches"));
        }
        let initial = n.tsn.as_ref().map_or(0, |t| t.updates.len())
            + n.dynamic.as_ref().map_or(0, |d| d.controls.len())
            + bridge
                .can
                .generators
                .iter()
                .filter(|g| g.schedule.time(0).is_some())
                .count()
            + usize::from(n.ethernet.generators.iter().any(|g| g.time(0).is_some()));
        if initial > limits.max_control_events as usize {
            return Err(error("initial reservations exceed max_control_events")
                .with_target("/limits/max_control_events"));
        }
        bridge.ethernet = n.ethernet.clone();
        n.bridge = Some(bridge);
        n.extensions = Some(PreparedNetworkExtensions {
            limits,
            replication: None,
            preemption: None,
            mac_merge: None,
        });
        if config.get("replication").is_some_and(|v| !v.is_null()) {
            let replication = replication_config(&config["replication"], &n, true)?;
            n.extensions.as_mut().unwrap().replication = Some(replication);
        }
        if n.has_gateway_replication() {
            for (i, control) in array(&workload["ethernet"]["controls"], "/ethernet/controls")?
                .iter()
                .enumerate()
            {
                if control["kind"] != "link_set" {
                    return Err(error(
                        "Gateway bridge-pair composition permits only link_set controls",
                    )
                    .with_target(format!("/ethernet/controls/{i}")));
                }
            }
            if n.tsn.as_ref().is_some_and(|t| !t.updates.is_empty()) {
                return Err(
                    error("Gateway bridge-pair composition requires static CBS/TAS")
                        .with_target("/tsn/gcl_updates"),
                );
            }
        }
        if config.get("mac_merge").is_some_and(|v| !v.is_null()) {
            let merge = mac_merge::prepare(&config["mac_merge"], &n, &workload["ethernet"], false)?;
            if let Some(r) = n.extensions.as_ref().and_then(|e| e.replication.as_ref()) {
                for stream in &r.streams {
                    if let ReplicationPlacement::BridgePair(pair) = &stream.placement {
                        if matches!(stream.source, ReplicationSource::GatewayEgress { .. })
                            && pair
                                .prefix
                                .directions
                                .iter()
                                .any(|&dir| merge.participating[dir])
                        {
                            return Err(error("Gateway FRER prefix must use ordinary Ethernet")
                                .with_target("/mac_merge"));
                        }
                    }
                }
            }
            n.extensions.as_mut().unwrap().mac_merge = Some(merge);
        }
        n.config = config.clone();
        n.workload = workload.clone();
        n.bindings = Some(super::bindings::bind(&n)?);
        Ok(n)
    };
    inner().map_err(|d| {
        let pointer = d.target.clone().unwrap_or_default();
        if let Some((_, path)) = workload_source {
            if d.source.as_deref() == Some(path.to_string_lossy().as_ref()) {
                return workload_doc
                    .as_ref()
                    .unwrap()
                    .annotate(&pointer, d)
                    .with_source(path);
            }
        }
        doc.annotate(&pointer, d).with_source(config_source.1)
    })
}
