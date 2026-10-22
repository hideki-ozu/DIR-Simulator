//! Physical port ownership for sampled point-to-point and shared named PHYs.
use super::*;
use crate::runtime::ethernet::phy::{attachment::AttachmentConfig, medium::PhysicalMode};

pub(super) fn prepare(
    rows: &Value,
    network: &mut PreparedNetwork,
    extensions: &PreparedNetworkExtensions,
) -> Result<()> {
    let rows = array(rows, "/phy_attachments")?;
    if rows.len() > 8 {
        return Err(error("at most eight sampled PHY attachments").with_target("/phy_attachments"));
    }
    let mut claimed = BTreeSet::new();
    for (i, row) in rows.iter().enumerate() {
        let path = format!("/phy_attachments/{i}");
        let cfg: AttachmentConfig = serde_json::from_value(row.clone())
            .map_err(|e| error(e.to_string()).with_target(&path))?;
        cfg.validate().map_err(|e| error(e).with_target(&path))?;
        let period = cfg
            .medium
            .channel
            .grid
            .time(1)
            .map_err(|e| error(e).with_target(&path))?;
        if period.numerator_ps < period.denominator {
            return Err(error("sample grid has multiple samples per integer ps").with_target(&path));
        }
        let mut directions = vec![];
        match cfg.medium.physical_mode {
            PhysicalMode::Ieee1000BaseT1PointToPoint2022 => {
                directions = point_to_point_physical_ports(
                    &cfg,
                    network,
                    extensions,
                    &mut claimed,
                    &path,
                    1_000_000_000,
                )?;
            }
            PhysicalMode::Ieee10BaseT2022 => {
                if cfg.medium.repeater.is_some() {
                    directions =
                        shared_physical_ports(&cfg, network, extensions, &mut claimed, &path)?;
                } else {
                    directions = point_to_point_physical_ports(
                        &cfg,
                        network,
                        extensions,
                        &mut claimed,
                        &path,
                        10_000_000,
                    )?;
                }
            }
            PhysicalMode::ProjectNumerical => {
                if cfg.bindings.len() != 1
                    || cfg.medium.endpoints.len() != 2
                    || cfg.medium.half_duplex
                {
                    return Err(error("initial network PHY graph requires one full-duplex two-endpoint directed binding per attachment").with_target(&path));
                }
                for (j, binding) in cfg.bindings.iter().enumerate() {
                    let at = format!("{path}/bindings/{j}");
                    let direction = network
                        .ethernet
                        .directions
                        .iter()
                        .position(|d| {
                            d.from_port == binding.from_port && d.to_port == binding.to_port
                        })
                        .ok_or_else(|| {
                            error("PHY binding does not match an Ethernet direction")
                                .with_target(&at)
                        })?;
                    if !claimed.insert(direction) || binding.direction_id != binding.from_port {
                        return Err(error(
                            "PHY direction is duplicated or its ID differs from from_port",
                        )
                        .with_target(&at));
                    }
                    require_queue_only(
                        network,
                        extensions,
                        &binding.from_port,
                        &at,
                        cfg.medium.half_duplex,
                    )?;
                    directions.push(direction);
                }
            }
            PhysicalMode::Ieee10BaseT1sMultidrop2022 => {
                directions = shared_physical_ports(&cfg, network, extensions, &mut claimed, &path)?;
            }
        }
        network.phy_attachments.push(PreparedPhyAttachment {
            config: cfg,
            directions,
        });
    }
    Ok(())
}

fn validate_admin_owner(cfg: &AttachmentConfig, network: &PreparedNetwork) -> Result<()> {
    use crate::types::ethernet::dynamic::ControlOp;
    if cfg.medium.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
        return Ok(());
    }
    if let Some(dynamic) = &network.dynamic {
        for control in &dynamic.controls {
            if let ControlOp::LinkSet { link, .. } = &control.op {
                if dynamic
                    .links
                    .iter()
                    .find(|l| &l.id == link)
                    .is_some_and(|link| {
                        cfg.physical_ports
                            .iter()
                            .any(|p| link.ports.contains(&p.tx_port))
                    })
                {
                    return Err(error(
                        "selected physical link needs an administrative disconnect adapter",
                    )
                    .with_target("/controls"));
                }
            }
        }
    }
    Ok(())
}

fn point_to_point_physical_ports(
    cfg: &AttachmentConfig,
    network: &PreparedNetwork,
    extensions: &PreparedNetworkExtensions,
    claimed: &mut BTreeSet<usize>,
    path: &str,
    bitrate_bps: u64,
) -> Result<Vec<usize>> {
    let mut directions = vec![];
    validate_admin_owner(cfg, network)?;
    for (j, physical) in cfg.physical_ports.iter().enumerate() {
        let at = format!("{path}/physical_ports/{j}");
        let direction = network
            .ethernet
            .directions
            .iter()
            .position(|d| d.from_port == physical.tx_port)
            .ok_or_else(|| error("physical tx_port is not an Ethernet output").with_target(&at))?;
        let link = &network.ethernet.directions[direction];
        if !claimed.insert(direction) || link.bitrate_bps != bitrate_bps {
            return Err(error("point-to-point physical output duplicated or carrier bitrate differs from selected MAC rate")
            .with_target(&at));
        }
        if !network
            .ethernet
            .port_policies
            .iter()
            .any(|p| p.port == physical.tx_port && p.ingress == physical.rx_port)
        {
            return Err(
                error("physical rx_port does not belong to the same MAC as tx_port")
                    .with_target(&at),
            );
        }
        require_queue_only(
            network,
            extensions,
            &physical.tx_port,
            &at,
            cfg.medium.half_duplex,
        )?;
        directions.push(direction);
    }
    if directions.len() != 2 {
        return Err(
            error("point-to-point attachment requires exactly two physical ports")
                .with_target(path),
        );
    }
    for (j, direction) in directions.iter().enumerate() {
        let link = &network.ethernet.directions[*direction];
        let peer = &network.ethernet.directions[directions[1 - j]];
        if link.source != peer.destination
            || link.destination != peer.source
            || link.to_port != cfg.physical_ports[1 - j].rx_port
        {
            return Err(error(
                "point-to-point physical ports must be the two ends of one NED carrier",
            )
            .with_target(path));
        }
    }
    Ok(directions)
}

fn shared_physical_ports(
    cfg: &AttachmentConfig,
    network: &mut PreparedNetwork,
    extensions: &PreparedNetworkExtensions,
    claimed: &mut BTreeSet<usize>,
    path: &str,
) -> Result<Vec<usize>> {
    let path = path.to_owned();
    let mut directions = vec![];
    validate_admin_owner(cfg, network)?;
    let mut hub = None;
    for (j, physical) in cfg.physical_ports.iter().enumerate() {
        let at = format!("{path}/physical_ports/{j}");
        let direction = network
            .ethernet
            .directions
            .iter()
            .position(|d| d.from_port == physical.tx_port)
            .ok_or_else(|| error("physical tx_port is not an Ethernet output").with_target(&at))?;
        let link = &network.ethernet.directions[direction];
        if !claimed.insert(direction) || link.bitrate_bps != 10_000_000 {
            return Err(error(
                "shared PHY output duplicated or carrier bitrate differs from 10Mb/s",
            )
            .with_target(&at));
        }
        if !network
            .ethernet
            .port_policies
            .iter()
            .any(|p| p.port == physical.tx_port && p.ingress == physical.rx_port)
        {
            return Err(
                error("physical rx_port does not belong to the same MAC as tx_port")
                    .with_target(&at),
            );
        }
        if !network.ethernet.directions.iter().any(|reverse| {
            reverse.source == link.destination
                && reverse.destination == link.source
                && reverse.to_port == physical.rx_port
        }) {
            return Err(
                error("physical port needs its reverse NED carrier attachment").with_target(&at),
            );
        }
        if network.ethernet.devices[link.destination].kind != "switch"
            || hub.is_some_and(|h| h != link.destination)
        {
            return Err(
                error("shared PHY ports must attach to one NED medium junction").with_target(&at),
            );
        }
        hub = Some(link.destination);
        require_queue_only(
            network,
            extensions,
            &physical.tx_port,
            &at,
            cfg.medium.half_duplex,
        )?;
        directions.push(direction);
    }
    let hub = hub.ok_or_else(|| error("shared PHY has no physical ports").with_target(&path))?;
    let hub_directions = network
        .ethernet
        .directions
        .iter()
        .filter(|d| d.destination == hub)
        .count();
    if hub_directions != directions.len() {
        return Err(
            error("shared PHY must own every carrier attached to its NED junction")
                .with_target(&path),
        );
    }
    if let Some(dynamic) = network.dynamic.as_mut() {
        dynamic.bridges.remove(&network.ethernet.devices[hub].id);
    }
    Ok(directions)
}

fn require_queue_only(
    network: &PreparedNetwork,
    extensions: &PreparedNetworkExtensions,
    port: &str,
    at: &str,
    half_duplex: bool,
) -> Result<()> {
    let shaped = network.tsn.as_ref().is_some_and(|tsn| {
        tsn.outputs
            .iter()
            .any(|o| o.port == port && (o.tas.is_some() || o.cbs.iter().any(Option::is_some)))
    });
    let cbs = network.tsn.as_ref().is_some_and(|tsn| {
        tsn.outputs
            .iter()
            .any(|o| o.port == port && o.cbs.iter().any(Option::is_some))
    });
    let experimental = network
        .config
        .get("physical_mac_policy")
        .and_then(Value::as_str);
    if cbs
        && !half_duplex
        && !matches!(
            experimental,
            Some("dir.experimental.sampled_wire_cbs.v1" | "dir.experimental.half_duplex_tsn.v1")
        )
    {
        return Err(error(
            "sampled-wire CBS requires explicit dir.experimental.sampled_wire_cbs.v1 accounting",
        )
        .with_target("/physical_mac_policy"));
    }
    if shaped
        && half_duplex
        && network
            .config
            .get("physical_mac_policy")
            .and_then(Value::as_str)
            != Some("dir.experimental.half_duplex_tsn.v1")
    {
        return Err(error(
            "half-duplex TAS/CBS requires explicit dir.experimental.half_duplex_tsn.v1 policy",
        )
        .with_target("/physical_mac_policy"));
    }
    if extensions
        .preemption
        .as_ref()
        .is_some_and(|p| p.outputs.iter().any(|o| o.port == port))
    {
        return Err(
            error("sampled PHY port needs an adapter for project abstract preemption")
                .with_target(at),
        );
    }
    Ok(())
}
