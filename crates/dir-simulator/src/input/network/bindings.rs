//! Bind independent resource claims after every composed parser has resolved IDs.
use super::{Result, error};
use crate::types::network::{PreparedNetwork, bindings::*};
use std::collections::BTreeMap;

fn claim(
    claims: &mut [Option<(usize, Option<String>)>],
    direction: usize,
    domain: usize,
    endpoint: Option<String>,
    target: &str,
) -> Result<()> {
    let slot = claims
        .get_mut(direction)
        .ok_or_else(|| error("physical claim has unknown direction").with_target(target))?;
    if slot.is_some() {
        return Err(error("one physical direction has two propagation owners").with_target(target));
    }
    *slot = Some((domain, endpoint));
    Ok(())
}

pub(super) fn bind(network: &PreparedNetwork) -> Result<PreparedBindings> {
    let mut result = PreparedBindings::default();
    let mut claims = vec![None; network.ethernet.directions.len()];
    for (index, attachment) in network.phy_attachments.iter().enumerate() {
        let medium = &attachment.config.medium;
        let shared = medium.half_duplex && medium.channel.endpoints.len() > 2;
        let domain = result.domains.len();
        result.domains.push(PhysicalDomain {
            id: format!("sampled:{index}"),
            owner: PhysicalOwner::Sampled { attachment: index },
            duplex: if medium.half_duplex {
                Duplex::Half
            } else {
                Duplex::Full
            },
            point_to_point: !shared,
            mac_endpoints: if attachment.config.physical_ports.is_empty() {
                medium.channel.endpoints.len()
            } else {
                attachment.config.physical_ports.len()
            },
            waveform_endpoints: medium.channel.endpoints.len(),
            timestamps: TimestampSource::SampledMacMarkers,
            retry_owned_by_medium: medium.half_duplex,
            raw_merge_transport: medium.gigabit_raw_merge,
        });
        for &direction in &attachment.directions {
            let port = &network.ethernet.directions[direction].from_port;
            let endpoint = attachment
                .config
                .physical_ports
                .iter()
                .find(|p| &p.tx_port == port)
                .map(|p| p.endpoint.clone())
                .or_else(|| {
                    attachment
                        .config
                        .bindings
                        .iter()
                        .find(|p| &p.from_port == port)
                        .map(|p| p.from_port.clone())
                });
            claim(&mut claims, direction, domain, endpoint, "/phy_attachments")?;
        }
    }
    for (index, segment) in network.shared_lans.iter().enumerate() {
        let domain = result.domains.len();
        result.domains.push(PhysicalDomain {
            id: format!("shared:{}", segment.id),
            owner: PhysicalOwner::IdealShared { segment: index },
            duplex: Duplex::Half,
            point_to_point: false,
            mac_endpoints: segment.members.len(),
            waveform_endpoints: 0,
            timestamps: TimestampSource::IdealMac,
            retry_owned_by_medium: false,
            raw_merge_transport: false,
        });
        for &direction in &segment.members {
            claim(&mut claims, direction, domain, None, "/shared_lans")?;
        }
    }
    if let Some(media) = &network.ethernet.media {
        for (index, link) in media.physical_links.iter().enumerate() {
            let domain = result.domains.len();
            result.domains.push(PhysicalDomain {
                id: format!("media:{}", link.id),
                owner: PhysicalOwner::ConfiguredMedia { link: index },
                duplex: if link.duplex == "half" {
                    Duplex::Half
                } else {
                    Duplex::Full
                },
                point_to_point: true,
                mac_endpoints: 2,
                waveform_endpoints: 0,
                timestamps: TimestampSource::ConfiguredMedia,
                retry_owned_by_medium: link.duplex == "half",
                raw_merge_transport: false,
            });
            for direction in link.directions {
                claim(&mut claims, direction, domain, None, "/base")?;
            }
        }
    }
    let mut ideal_domains = BTreeMap::new();
    for (direction, link) in network.ethernet.directions.iter().enumerate() {
        if claims[direction].is_some() {
            continue;
        }
        let peer = network
            .ethernet
            .port_policies
            .iter()
            .find(|p| p.ingress == link.to_port)
            .map(|p| p.port.as_str())
            .unwrap_or(&link.to_port);
        let mut ends = [link.from_port.as_str(), peer];
        ends.sort_unstable();
        let id = format!("ideal:{}<->{}", ends[0], ends[1]);
        let domain = *ideal_domains.entry(id.clone()).or_insert_with(|| {
            let domain = result.domains.len();
            result.domains.push(PhysicalDomain {
                id,
                owner: PhysicalOwner::IdealPointToPoint,
                duplex: Duplex::Full,
                point_to_point: true,
                mac_endpoints: 2,
                waveform_endpoints: 0,
                timestamps: TimestampSource::IdealMac,
                retry_owned_by_medium: false,
                raw_merge_transport: true,
            });
            domain
        });
        claim(&mut claims, direction, domain, None, "/base")?;
    }
    for (direction, link) in network.ethernet.directions.iter().enumerate() {
        let (physical_domain, physical_endpoint) = claims[direction].clone().unwrap();
        let physical = &result.domains[physical_domain];
        let merge = network
            .extensions
            .as_ref()
            .and_then(|e| e.mac_merge.as_ref())
            .is_some_and(|m| m.participating.get(direction) == Some(&true));
        let preemption = network
            .extensions
            .as_ref()
            .and_then(|e| e.preemption.as_ref())
            .is_some_and(|p| p.participating.get(direction) == Some(&true));
        if matches!(physical.owner, PhysicalOwner::Sampled { .. })
            && physical.raw_merge_transport
            && !merge
        {
            return Err(
                error("raw sampled PHY requires a Clause99 MAC producer on each output")
                    .with_target("/phy_attachments"),
            );
        }
        if preemption
            && network
                .gptp
                .as_ref()
                .is_some_and(|g| g.ports[direction].is_some())
            && network
                .extensions
                .as_ref()
                .and_then(|e| e.preemption.as_ref())
                .is_none_or(|p| {
                    p.outputs[direction].express_mask
                        & (1 << network.gptp.as_ref().unwrap().priority)
                        == 0
                })
        {
            return Err(
                error("gPTP priority requires EXPRESS on project preemption output")
                    .with_target("/gptp"),
            );
        }
        if merge && preemption {
            return Err(error("one output has two MAC serializers").with_target("/preemption"));
        }
        if preemption
            && !(matches!(
                physical.owner,
                PhysicalOwner::IdealPointToPoint | PhysicalOwner::ConfiguredMedia { .. }
            ) && physical.duplex == Duplex::Full)
        {
            return Err(error(
                "project abstract preemption needs a physical service adapter on this output",
            )
            .with_target("/preemption"));
        }
        if merge
            && (physical.duplex != Duplex::Full
                || !physical.point_to_point
                || link.bitrate_bps < 100_000_000)
        {
            return Err(error(
                "Clause99 requires this output to be full-duplex point-to-point at least100Mb/s",
            )
            .with_target("/mac_merge"));
        }
        if let Some(pause) = network
            .mac_control
            .as_ref()
            .and_then(|c| c.ports[direction].as_ref())
        {
            use crate::types::network::mac_control::Action;
            let generates = pause.tx_enabled
                || network
                    .mac_control
                    .as_ref()
                    .unwrap()
                    .controls
                    .iter()
                    .any(|c| {
                        c.direction == direction
                            && matches!(c.action, Action::PauseRequest(_) | Action::TxEnable(true))
                    });
            if generates && physical.duplex != Duplex::Full {
                return Err(
                    error("IEEE PAUSE generation requires full duplex on the target MAC")
                        .with_target("/mac_control/ports"),
                );
            }
            if generates && merge {
                return Err(error("Clause99 prohibits PAUSE generation at eMAC/pMAC")
                    .with_target("/mac_control/ports"));
            }
            if pause.experimental_project_generation && !preemption {
                return Err(error(
                    "project preemption PAUSE policy requires a project preemption owner",
                )
                .with_target("/mac_control/ports"));
            }
            if generates && preemption && !pause.experimental_project_generation {
                return Err(error("abstract preemption PAUSE generation requires explicit dir.experimental.project_preemption_pause.v1 policy").with_target("/mac_control/ports"));
            }
            let receives = pause.rx_enabled
                || network
                    .mac_control
                    .as_ref()
                    .unwrap()
                    .controls
                    .iter()
                    .any(|c| {
                        c.direction == direction && matches!(c.action, Action::RxEnable(true))
                    });
            if (merge || preemption) && receives && !pause.experimental_merge {
                return Err(error("Merge receive inhibition requires explicit dir.experimental.merge_pause.v1 policy").with_target("/mac_control/ports"));
            }
        }
        if matches!(physical.owner, PhysicalOwner::ConfiguredMedia { .. })
            && physical.duplex == Duplex::Half
        {
            let shaped = network.tsn.as_ref().is_some_and(|t| {
                t.outputs.iter().any(|o| {
                    o.port == link.from_port
                        && (o.tas.is_some() || o.cbs.iter().any(Option::is_some))
                })
            });
            if shaped
                && network.config["physical_mac_policy"] != "dir.experimental.half_duplex_tsn.v1"
            {
                return Err(error("half-duplex TAS/CBS requires explicit dir.experimental.half_duplex_tsn.v1 policy").with_target("/physical_mac_policy"));
            }
        }
        let gptp = network
            .gptp
            .as_ref()
            .is_some_and(|g| g.ports[direction].is_some());
        if gptp && (physical.duplex != Duplex::Full || !physical.point_to_point) {
            return Err(
                error("selected gPTP peer requires a full-duplex point-to-point output")
                    .with_target("/gptp"),
            );
        }
        let scripted = network
            .clocks
            .as_ref()
            .is_some_and(|c| c.adjustments.iter().any(|a| a.node == link.source));
        let gptp_clock = network
            .gptp
            .as_ref()
            .is_some_and(|g| g.nodes.iter().any(|n| n.device == link.source));
        if scripted && gptp_clock {
            return Err(error("one node clock has two discipline writers").with_target("/clocks"));
        }
        let initial_nonidentity = network.clocks.as_ref().is_some_and(|c| {
            let clock = &c.initial[link.source];
            clock.offset_ps != 0 || clock.rate_num != clock.rate_den
        });
        let shaper = network
            .tsn
            .as_ref()
            .and_then(|t| t.port_index.get(&link.from_port).copied());
        let external_schedule = network.tsn.as_ref().is_some_and(|t| {
            shaper.is_some_and(|i| t.outputs[i].tas.is_some())
                || t.updates.iter().any(|u| u.port == link.from_port)
        });
        let managed = network
            .qcc
            .as_ref()
            .is_some_and(|q| q.ports.iter().any(|p| p.direction == direction));
        let mut protocol_adapters = network.wire.as_ref().map_or_else(Vec::new, |wire| {
            wire.adapters
                .iter()
                .filter(|a| a.ports.contains(&link.from_port))
                .map(|a| a.id.clone())
                .collect()
        });
        if gptp {
            protocol_adapters.push("gptp".into());
        }
        if merge {
            protocol_adapters.push("mac_merge_lldp".into());
        }
        protocol_adapters.sort();
        result.ports.push(PortBinding {
            direction,
            physical_domain,
            physical_endpoint,
            mac: if merge {
                MacOwner::Clause99
            } else if preemption {
                MacOwner::ProjectPreemption
            } else {
                MacOwner::Ordinary
            },
            bitrate_bps: link.bitrate_bps,
            clock_node: link.source,
            discipline: if gptp_clock {
                ClockDiscipline::Gptp
            } else if scripted {
                ClockDiscipline::Scripted
            } else if initial_nonidentity {
                ClockDiscipline::Static
            } else {
                ClockDiscipline::Identity
            },
            shaper,
            schedule_writer: if external_schedule {
                Some(ScheduleWriter::External)
            } else if managed {
                Some(ScheduleWriter::Qcc)
            } else {
                None
            },
            forwarding_component: network
                .provider_l2
                .as_ref()
                .and_then(|t| t.ports.get(&link.from_port))
                .map(|p| p.component.clone()),
            protocol_adapters,
        });
    }
    Ok(result)
}

#[cfg(test)]
#[path = "bindings_tests.rs"]
mod tests;
