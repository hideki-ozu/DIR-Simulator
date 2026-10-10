//! Shared, scheduler-independent L2 arithmetic and VLAN/queue rules.
use crate::types::{Diagnostic, ethernet::*};
type Result<T> = std::result::Result<T, Diagnostic>;
#[derive(Debug, Clone, Copy)]
pub(crate) struct Classification {
    pub vid: u16,
    pub priority: u8,
    pub dei: u8,
}
pub(crate) fn classify(policy: &EthernetPortPolicy, wire: &EthernetWireFrame) -> Classification {
    wire.tag.map_or(
        Classification {
            vid: policy.pvid,
            priority: policy.default_priority,
            dei: 0,
        },
        |t| Classification {
            vid: t.vid,
            priority: t.pcp,
            dei: t.dei,
        },
    )
}
pub(crate) fn rewrite(
    policy: &EthernetPortPolicy,
    wire: &EthernetWireFrame,
    class: Classification,
    tagged: Option<bool>,
) -> Result<EthernetWireFrame> {
    let tagged = tagged
        .or_else(|| policy.vlans.get(&class.vid).copied())
        .ok_or_else(|| Diagnostic::execution("missing egress VLAN membership"))?;
    super::serialize_vlan_frame(
        &wire.src_mac,
        &wire.dst_mac,
        wire.ether_type,
        &wire.data_hex,
        tagged.then_some(EthernetVlanTag {
            vid: class.vid,
            pcp: class.priority,
            dei: class.dei,
        }),
    )
}
pub(crate) fn admission(
    total: u64,
    port_capacity: u64,
    frames: u64,
    bytes: u64,
    config: Option<&EthernetQueueConfig>,
    mac_bytes: u64,
) -> Result<bool> {
    if total >= port_capacity {
        return Ok(false);
    }
    if let Some(config) = config {
        if frames >= config.capacity_frames {
            return Ok(false);
        }
        let bytes = bytes
            .checked_add(mac_bytes)
            .ok_or_else(|| super::overflow("Ethernet queue byte overflow"))?;
        if config.capacity_bytes.is_some_and(|n| bytes > n) {
            return Ok(false);
        }
    }
    Ok(true)
}
pub(crate) fn timing(
    now: u64,
    mac_bytes: u64,
    bitrate: u64,
    delay: u64,
) -> Result<(u64, u64, u64)> {
    let wire = mac_bytes
        .checked_add(8)
        .and_then(|n| n.checked_mul(8))
        .ok_or_else(|| super::overflow("Ethernet wire length overflow"))?;
    let occupied = mac_bytes
        .checked_add(20)
        .and_then(|n| n.checked_mul(8))
        .ok_or_else(|| super::overflow("Ethernet occupied length overflow"))?;
    let eof = super::add(now, super::duration(wire, bitrate)?)?;
    let release = super::add(now, super::duration(occupied, bitrate)?)?;
    Ok((eof, release, super::add(eof, delay)?))
}

/// Legacy static forwarding selection, also used by the composite media adapter.
pub(crate) fn static_egress(
    model: &PreparedEthernet,
    incoming: usize,
    wire: &EthernetWireFrame,
    vid: Option<u16>,
) -> Result<(Vec<usize>, Option<&'static str>)> {
    let d = &model.directions[incoming];
    let device = &model.devices[d.destination];
    if device.kind == "endpoint" {
        return Ok((vec![], None));
    }
    let ingress = model
        .port_policies
        .iter()
        .find(|p| p.ingress == d.to_port)
        .map(|p| p.port.as_str());
    let eligible: Vec<_> = model
        .directions
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.source == d.destination
                && if vid.is_some() {
                    Some(e.from_port.as_str()) != ingress
                } else {
                    e.to_port != paired(&d.from_port)
                }
        })
        .filter(|(_, e)| {
            vid.is_none_or(|vid| {
                model
                    .port_policies
                    .iter()
                    .any(|p| p.port == e.from_port && p.vlans.contains_key(&vid))
            })
        })
        .map(|(i, _)| i)
        .collect();
    if let Some(vid) = vid {
        if wire.dst_mac != "ff:ff:ff:ff:ff:ff" && super::is_group(&wire.dst_mac) {
            if let Some(ports) = device.multicast.get(&(vid, wire.dst_mac.clone())) {
                let selected: Vec<_> = eligible
                    .into_iter()
                    .filter(|i| ports.contains(&model.directions[*i].from_port))
                    .collect();
                let reason = selected.is_empty().then_some("multicast_no_egress");
                return Ok((selected, reason));
            }
            if device.unknown_multicast == "drop" {
                return Ok((vec![], Some("unknown_multicast")));
            }
            let reason = eligible.is_empty().then_some("no_vlan_egress");
            return Ok((eligible, reason));
        }
        if let Some(port) = device.vlan_fdb.get(&(vid, wire.dst_mac.clone())) {
            let selected: Vec<_> = eligible
                .into_iter()
                .filter(|i| model.directions[*i].from_port == *port)
                .collect();
            let reason = selected.is_empty().then_some("same_ingress");
            return Ok((selected, reason));
        }
        let reason = eligible.is_empty().then_some("no_vlan_egress");
        return Ok((eligible, reason));
    }
    if let Some(port) = device.fdb.get(&wire.dst_mac) {
        let selected: Vec<_> = eligible
            .into_iter()
            .filter(|i| model.directions[*i].from_port == *port)
            .collect();
        let reason = selected.is_empty().then_some("same_ingress");
        return Ok((selected, reason));
    }
    let reason = eligible.is_empty().then_some("same_ingress");
    Ok((eligible, reason))
}
fn paired(port: &str) -> String {
    let (owner, gate) = port.rsplit_once('.').unwrap();
    format!(
        "{owner}.{}",
        if gate == "tx" {
            "rx".into()
        } else {
            format!("rx_{}", gate.strip_prefix("tx_").unwrap())
        }
    )
}
