//! Strict admission for the selected domain-0 two-step physical tree profile.
use super::*;
use crate::types::network::gptp::{GptpNode, GptpPort, PreparedGptp};

fn octets<const N: usize>(v: &Value, at: &str) -> Result<[u8; N]> {
    let s = v
        .as_str()
        .ok_or_else(|| error("expected colon-separated hexadecimal octets").with_target(at))?;
    let parts = s.split(':').collect::<Vec<_>>();
    if parts.len() != N
        || parts.iter().any(|p| {
            p.len() != 2
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
    {
        return Err(
            error("expected canonical lowercase colon-separated hexadecimal octets")
                .with_target(at),
        );
    }
    let mut out = [0; N];
    for (index, item) in parts.iter().enumerate() {
        out[index] = u8::from_str_radix(item, 16)
            .map_err(|_| error("invalid hexadecimal octet").with_target(at))?;
    }
    Ok(out)
}

fn log_interval(v: &Value, at: &str) -> Result<(i8, u64)> {
    let exponent = v
        .as_i64()
        .filter(|x| (-12..=10).contains(x))
        .ok_or_else(|| {
            error("log interval must be an integer -12..10 with exact picosecond period")
                .with_target(at)
        })?;
    let period = if exponent < 0 {
        1_000_000_000_000u64 >> (-exponent as u32)
    } else {
        1_000_000_000_000u64
            .checked_shl(exponent as u32)
            .ok_or_else(|| error("interval overflow").with_target(at))?
    };
    Ok((exponent as i8, period))
}

pub(super) fn prepare(v: &Value, n: &PreparedNetwork, workload: &Value) -> Result<PreparedGptp> {
    let at = "/gptp";
    exact(
        v,
        &[
            "profile",
            "domain",
            "nodes",
            "log_sync_interval",
            "log_announce_interval",
            "log_pdelay_req_interval",
            "announce_receipt_timeout",
            "sync_receipt_timeout",
            "allowed_lost_responses",
            "mean_link_delay_threshold_ps",
            "priority",
            "max_pending_exchanges",
            "max_journal_entries",
        ],
        "gPTP selected profile",
    )
    .map_err(|d| d.with_target(at))?;
    if v["profile"] != "as2020.domain0.two_step_tree.v1" {
        return Err(error("unsupported gPTP packet profile").with_target("/gptp/profile"));
    }
    if v["domain"] != 0 {
        return Err(
            error("selected gPTP profile supports domain 0 only").with_target("/gptp/domain")
        );
    }
    for (i, control) in array(&workload["controls"], "/controls")?
        .iter()
        .enumerate()
    {
        if control["kind"] != "link_set" {
            return Err(error("gPTP permits only physical link_set controls")
                .with_target(format!("/controls/{i}")));
        }
    }
    let (log_sync_interval, sync_interval_ps) =
        log_interval(&v["log_sync_interval"], "/gptp/log_sync_interval")?;
    let (log_announce_interval, announce_interval_ps) =
        log_interval(&v["log_announce_interval"], "/gptp/log_announce_interval")?;
    let (log_pdelay_req_interval, pdelay_interval_ps) = log_interval(
        &v["log_pdelay_req_interval"],
        "/gptp/log_pdelay_req_interval",
    )?;
    let mut out = PreparedGptp {
        nodes: vec![],
        ports: vec![None; n.ethernet.directions.len()],
        reverse: vec![0; n.ethernet.directions.len()],
        sync_interval_ps,
        announce_interval_ps,
        pdelay_interval_ps,
        announce_receipt_timeout: integer(
            &v["announce_receipt_timeout"],
            1,
            255,
            "/gptp/announce_receipt_timeout",
        )? as u32,
        sync_receipt_timeout: integer(
            &v["sync_receipt_timeout"],
            1,
            255,
            "/gptp/sync_receipt_timeout",
        )? as u32,
        allowed_lost_responses: integer(
            &v["allowed_lost_responses"],
            1,
            255,
            "/gptp/allowed_lost_responses",
        )? as u32,
        mean_link_delay_threshold_ps: decimal(
            &v["mean_link_delay_threshold_ps"],
            "/gptp/mean_link_delay_threshold_ps",
        )?,
        priority: integer(&v["priority"], 0, 7, "/gptp/priority")? as u8,
        max_pending_exchanges: integer(
            &v["max_pending_exchanges"],
            1,
            100_000,
            "/gptp/max_pending_exchanges",
        )? as usize,
        max_journal_entries: integer(
            &v["max_journal_entries"],
            2,
            1_000_000,
            "/gptp/max_journal_entries",
        )? as usize,
        log_sync_interval,
        log_announce_interval,
        log_pdelay_req_interval,
    };
    let mut seen_nodes = BTreeSet::new();
    let mut seen_clock = BTreeSet::new();
    for (i, row) in array(&v["nodes"], "/gptp/nodes")?.iter().enumerate() {
        let path = format!("/gptp/nodes/{i}");
        exact(
            row,
            &[
                "node",
                "clock_identity",
                "priority1",
                "clock_class",
                "clock_accuracy",
                "offset_scaled_log_variance",
                "priority2",
                "time_source",
                "ports",
            ],
            "gPTP node",
        )
        .map_err(|d| d.with_target(&path))?;
        let id = string(&row["node"], &format!("{path}/node"))?;
        let device = n
            .ethernet
            .devices
            .iter()
            .position(|d| d.id == id)
            .ok_or_else(|| error("unknown gPTP node").with_target(format!("{path}/node")))?;
        if !seen_nodes.insert(device) {
            return Err(error("duplicate gPTP node").with_target(&path));
        }
        let identity = octets::<8>(&row["clock_identity"], &format!("{path}/clock_identity"))?;
        if !seen_clock.insert(identity) {
            return Err(error("duplicate clockIdentity").with_target(&path));
        }
        let node = GptpNode {
            device,
            clock_identity: identity,
            priority1: integer(&row["priority1"], 0, 255, &format!("{path}/priority1"))? as u8,
            clock_class: integer(&row["clock_class"], 0, 255, &format!("{path}/clock_class"))?
                as u8,
            clock_accuracy: integer(
                &row["clock_accuracy"],
                0,
                255,
                &format!("{path}/clock_accuracy"),
            )? as u8,
            offset_scaled_log_variance: integer(
                &row["offset_scaled_log_variance"],
                0,
                65535,
                &format!("{path}/offset_scaled_log_variance"),
            )? as u16,
            priority2: integer(&row["priority2"], 0, 255, &format!("{path}/priority2"))? as u8,
            time_source: integer(&row["time_source"], 0, 255, &format!("{path}/time_source"))?
                as u8,
        };
        let mut numbers = BTreeSet::new();
        for (j, port) in array(&row["ports"], &format!("{path}/ports"))?
            .iter()
            .enumerate()
        {
            let ptr = format!("{path}/ports/{j}");
            exact(port, &["port", "port_number", "source_mac"], "gPTP port")
                .map_err(|d| d.with_target(&ptr))?;
            let name = string(&port["port"], &format!("{ptr}/port"))?;
            let direction = n
                .ethernet
                .directions
                .iter()
                .position(|d| d.from_port == name && d.source == device)
                .ok_or_else(|| {
                    error("gPTP port does not own an Ethernet output")
                        .with_target(format!("{ptr}/port"))
                })?;
            if out.ports[direction].is_some() {
                return Err(error("duplicate physical gPTP port").with_target(&ptr));
            }
            let number = integer(
                &port["port_number"],
                1,
                65534,
                &format!("{ptr}/port_number"),
            )? as u16;
            if !numbers.insert(number) {
                return Err(error("duplicate PortNumber").with_target(&ptr));
            }
            let mac = octets::<6>(&port["source_mac"], &format!("{ptr}/source_mac"))?;
            if mac[0] & 1 != 0 || mac == [0; 6] {
                return Err(error("source MAC must be nonzero unicast")
                    .with_target(format!("{ptr}/source_mac")));
            }
            if let Some(merge) = n.extensions.as_ref().and_then(|e| e.mac_merge.as_ref()) {
                if merge.ports[direction]
                    .as_ref()
                    .is_some_and(|m| m.source_mac != mac)
                {
                    return Err(error("gPTP and MAC Merge source MAC disagree")
                        .with_target(format!("{ptr}/source_mac")));
                }
                if merge.participating[direction]
                    && merge.ports[direction]
                        .as_ref()
                        .is_some_and(|m| m.express_mask & (1 << out.priority) == 0)
                {
                    return Err(error("gPTP priority must map to express MAC")
                        .with_target("/gptp/priority"));
                }
            }
            out.ports[direction] = Some(GptpPort {
                node: i,
                port_number: number,
                source_mac: mac,
            });
        }
        if !out
            .ports
            .iter()
            .any(|p| p.as_ref().is_some_and(|p| p.node == i))
        {
            return Err(error("gPTP node requires at least one physical port")
                .with_target(format!("{path}/ports")));
        }
        out.nodes.push(node);
    }
    if out.nodes.is_empty() {
        return Err(error("gPTP requires participating nodes").with_target("/gptp/nodes"));
    }
    if out.nodes.iter().all(|n| n.priority1 == 255) {
        return Err(
            error("each gPTP component needs a GM-capable candidate").with_target("/gptp/nodes")
        );
    }
    for (i, d) in n.ethernet.directions.iter().enumerate() {
        let reverse = n
            .ethernet
            .directions
            .iter()
            .position(|r| {
                r.source == d.destination
                    && r.destination == d.source
                    && r.from_port == crate::input::ethernet::paired(&d.to_port)
                    && r.to_port == crate::input::ethernet::paired(&d.from_port)
            })
            .ok_or_else(|| {
                error("gPTP physical link requires a reverse direction").with_target(at)
            })?;
        out.reverse[i] = reverse;
        if out.ports[i].is_some() != out.ports[reverse].is_some() {
            return Err(
                error("both ends of every participating PTP Link must be configured")
                    .with_target(at),
            );
        }
        if out.ports[i].is_some()
            && (d.bitrate_bps != n.ethernet.directions[reverse].bitrate_bps
                || d.delay_ps != n.ethernet.directions[reverse].delay_ps)
        {
            return Err(
                error("gPTP selected profile requires symmetric physical links").with_target(at),
            );
        }
    }
    // Validate the physical service of each participating output. Unrelated
    // domains and scripted clocks retain their independent owners; final typed
    // binding rejects shared/half-duplex peers and same-node clock writers.
    for (direction, port) in out.ports.iter().enumerate() {
        if port.is_none() {
            continue;
        }
        if n.ethernet.media.as_ref().is_some_and(|media| {
            media
                .physical_links
                .iter()
                .any(|link| link.directions.contains(&direction) && link.duplex != "full")
        }) {
            return Err(
                error("gPTP configured timestamps require a full-duplex physical link")
                    .with_target("/gptp/nodes"),
            );
        }
        if let Some(attachment) = n
            .phy_attachments
            .iter()
            .find(|a| a.directions.contains(&direction))
        {
            if !attachment.config.medium.physical_mode.is_named() {
                return Err(
                    error("gPTP sampled timestamps require actual named PHY MAC markers")
                        .with_target("/gptp/nodes"),
                );
            }
            if !attachment
                .config
                .medium
                .exact_mdi_timestamp_mapping_supported()
            {
                return Err(error("gPTP exact MDI timestamps require the certified fixed-clock noiseless unity single-tap channel profile")
                    .with_target("/gptp/nodes"));
            }
            let paths = &attachment.config.medium.channel.paths;
            if paths[0][1].as_ref().unwrap().delay_samples
                != paths[1][0].as_ref().unwrap().delay_samples
            {
                return Err(error(
                    "gPTP selected profile requires symmetric sampled physical paths",
                )
                .with_target("/gptp/nodes"));
            }
            if attachment.config.medium.gigabit_raw_merge
                && !n
                    .extensions
                    .as_ref()
                    .and_then(|e| e.mac_merge.as_ref())
                    .is_some_and(|m| m.participating[direction])
            {
                return Err(error(
                    "gPTP raw PHY requires the participating Clause99 express owner",
                )
                .with_target("/gptp/nodes"));
            }
        }
    }
    // Forest validation is compile-time admission only; runtime BMCA consumes
    // decoded Announce frames, never this graph to elect a master.
    let mut parent = (0..out.nodes.len()).collect::<Vec<_>>();
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            x = parent[x];
        }
        x
    }
    for i in 0..out.ports.len() {
        if let Some(a) = out.ports[i].as_ref() {
            let r = out.reverse[i];
            if i < r {
                let b = out.ports[r].as_ref().unwrap();
                let aa = root(&mut parent, a.node);
                let bb = root(&mut parent, b.node);
                if aa == bb {
                    return Err(error(
                        "cyclic gPTP participating topology is outside selected profile",
                    )
                    .with_target(at));
                }
                parent[aa] = bb;
            }
        }
    }
    for i in 0..out.nodes.len() {
        let component = root(&mut parent, i);
        if !out
            .nodes
            .iter()
            .enumerate()
            .any(|(j, n)| root(&mut parent, j) == component && n.priority1 != 255)
        {
            return Err(error("gPTP component has no GM-capable candidate").with_target(at));
        }
    }
    if let Some(clocks) = &n.clocks {
        for (i, node) in out.nodes.iter().enumerate() {
            if clocks.initial[node.device].offset_ps < 0 {
                return Err(error("gPTP raw oscillator must be nonnegative at start")
                    .with_target(format!("/gptp/nodes/{i}")));
            }
        }
    }
    Ok(out)
}
