//! Exact validators for the explicitly selected network extensions.
use super::{Result, error, exact};
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
fn static_base(network: &PreparedNetwork, workload: &Value) -> Result<()> {
    if !array(&workload["controls"], "/controls")?.is_empty() {
        return Err(error("abstract extension requires controls=[]").with_target("/controls"));
    }
    if network.ethernet.media.is_some()
        || network
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
        let replication = profile == "ethernet.redundancy.abstract.v1";
        let extension = if replication {
            "replication"
        } else {
            "preemption"
        };
        exact(
            &config,
            &["schema_name", "schema_version", "base", extension, "limits"],
            "network extension",
        )?;
        let schema = if replication {
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
        let mut network = super::prepare(
            ethernet.clone(),
            config["base"]["config"].clone(),
            workload.clone(),
            "ethernet.tsn.v1",
            (&raw, config_source.1),
            Some((&work, workload_source.map_or(config_source.1, |s| s.1))),
        )
        .map_err(|d| {
            let target = d.target.clone().unwrap_or_default();
            d.with_target(format!("/base/config{target}"))
        })?;
        static_base(&network, &workload)?;
        let limits = limits(&config["limits"])?;
        let mut extensions = PreparedNetworkExtensions {
            limits,
            replication: None,
            preemption: None,
        };
        if replication {
            extensions.replication = Some(replication_config(&config[extension], &network)?);
        } else {
            extensions.preemption = Some(preemption_config(&config[extension], &network)?);
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
        network.config = config.clone();
        network.workload = workload.clone();
        Ok(network)
    };
    prepare_inner().map_err(|d| {
        let pointer = d.target.clone().unwrap_or_default();
        doc.annotate(&pointer, d).with_source(config_source.1)
    })
}
fn replication_config(v: &Value, n: &PreparedNetwork) -> Result<PreparedReplication> {
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
    let initial = crate::runtime::ethernet::dynamic::DynamicState::new(
        n.dynamic.as_ref().unwrap(),
        &n.ethernet,
    )?
    .snapshot_policy();
    for (i, row) in rows.iter().enumerate() {
        let p = format!("/replication/streams/{i}");
        exact(
            row,
            &[
                "stream_id",
                "source_generator",
                "replicate_at",
                "eliminate_at",
                "history_capacity",
                "egress_paths",
            ],
            "replication stream",
        )
        .map_err(|d| d.with_target(&p))?;
        let id = string(&row["stream_id"], &format!("{p}/stream_id"))?;
        let generator = string(&row["source_generator"], &format!("{p}/source_generator"))?;
        if !ids.insert(id.to_owned()) || !generators.insert(generator.to_owned()) {
            return Err(error("duplicate stream or source generator").with_target(&p));
        }
        let g = n
            .ethernet
            .generators
            .iter()
            .find(|g| g.id == generator)
            .ok_or_else(|| {
                error("unknown source generator").with_target(format!("{p}/source_generator"))
            })?;
        let source = string(&row["replicate_at"], &format!("{p}/replicate_at"))?;
        let destination = string(&row["eliminate_at"], &format!("{p}/eliminate_at"))?;
        let source_index = g.source;
        let destination_index = n
            .ethernet
            .devices
            .iter()
            .position(|d| d.id == destination && d.kind == "endpoint")
            .ok_or_else(|| {
                error("eliminate_at must be endpoint").with_target(format!("{p}/eliminate_at"))
            })?;
        if n.ethernet.devices[source_index].id != source
            || source == destination
            || n.ethernet.devices[destination_index].mac.as_ref() != Some(&g.frame.dst_mac)
            || n.dynamic
                .as_ref()
                .unwrap()
                .generator_ip
                .get(generator)
                .is_some_and(Option::is_some)
        {
            return Err(error("replication requires generator source and distinct individual destination endpoint with null IP multicast").with_target(&p));
        }
        let history_capacity = integer(
            &row["history_capacity"],
            1,
            65536,
            &format!("{p}/history_capacity"),
        )? as usize;
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
            let mut current = source_index;
            let mut visited = BTreeSet::from([current]);
            let mut directions = Vec::new();
            let mut ports = Vec::new();
            for port in array(&path["ports"], &format!("{pp}/ports"))? {
                let port = string(port, &format!("{pp}/ports"))?;
                let index = n
                    .ethernet
                    .directions
                    .iter()
                    .position(|d| d.from_port == port)
                    .ok_or_else(|| error("unknown output port").with_target(&pp))?;
                let link = &n.ethernet.directions[index];
                if link.source != current || !visited.insert(link.destination) {
                    return Err(error("path disconnected or revisits device").with_target(&pp));
                }
                if current != source_index && n.ethernet.devices[current].kind != "switch" {
                    return Err(error("intermediate path device must be switch").with_target(&pp));
                }
                let vid = g.source_vlan_id.unwrap();
                for policy in n
                    .ethernet
                    .port_policies
                    .iter()
                    .filter(|p| p.port == link.from_port || p.ingress == link.to_port)
                {
                    if !initial
                        .effective_vlans
                        .get(&policy.port)
                        .is_some_and(|v| v.contains_key(&vid))
                    {
                        return Err(error("path VID not initially allowed").with_target(&pp));
                    }
                }
                if !initial.link_up.get(port).copied().unwrap_or(true) {
                    return Err(error("path link initially down").with_target(&pp));
                }
                directions.push(index);
                ports.push(port.to_owned());
                current = link.destination;
            }
            if directions.is_empty()
                || current != destination_index
                || !path_ports.insert(ports.clone())
            {
                return Err(error("path empty, duplicate or wrong endpoint").with_target(&pp));
            }
            for port in &ports {
                *use_count.entry(port.clone()).or_default() += 1;
            }
            prepared_paths.push(ReplicationPath {
                id: path_id.into(),
                ports,
                directions,
            });
        }
        prepared_paths.sort_by(|a, b| a.id.cmp(&b.id));
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
            source_generator: generator.into(),
            replicate_at: source.into(),
            eliminate_at: destination.into(),
            history_capacity,
            initial_history_words,
            paths: prepared_paths,
        });
    }
    streams.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
    let generator_stream = streams
        .iter()
        .enumerate()
        .map(|(i, s)| (s.source_generator.clone(), i))
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
fn preemption_config(v: &Value, n: &PreparedNetwork) -> Result<PreparedPreemption> {
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
    let outputs = n
        .ethernet
        .directions
        .iter()
        .map(|d| {
            ports.remove(&d.from_port).ok_or_else(|| {
                error("every output must configure preemption").with_target("/preemption/outputs")
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
    Ok(PreparedPreemption { outputs })
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
        exact(
            &config,
            &[
                "schema_name",
                "schema_version",
                "bridge",
                "ethernet",
                "tsn",
                "limits",
            ],
            "composite configuration",
        )?;
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
        let mut n = super::prepare(
            ethernet.clone(),
            adapter,
            workload["ethernet"].clone(),
            "ethernet.tsn.v1",
            (&text, config_source.1),
            Some((&work, workload_source.map_or(config_source.1, |s| s.1))),
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
        });
        n.config = config.clone();
        n.workload = workload.clone();
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
