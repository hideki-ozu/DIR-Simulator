//! Unify generated active-membership and explicit wire adapters into one host.
use super::{Result, error};
use crate::types::wire::{PreparedWire, WireProtocol};
use serde::Serialize;

fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).expect("serializable prepared wire value")
        == serde_json::to_value(b).expect("serializable prepared wire value")
}
fn tree(protocol: WireProtocol) -> bool {
    matches!(protocol, WireProtocol::Rstp | WireProtocol::Mstp)
}

pub(super) fn merge(mut base: PreparedWire, added: PreparedWire) -> Result<PreparedWire> {
    for (name, mut port) in added.ports {
        if let Some(old) = base.ports.get_mut(&name) {
            let mac = old.source_mac.or(port.source_mac);
            if old.source_mac.is_none() {
                old.source_mac = mac;
            }
            if port.source_mac.is_none() {
                port.source_mac = mac;
            }
            let mut allowed = old.allowed_vlans.clone();
            for (vid, tagged) in &port.allowed_vlans {
                if allowed
                    .insert(*vid, *tagged)
                    .is_some_and(|previous| previous != *tagged)
                {
                    return Err(error(format!(
                        "conflicting wire VLAN treatment at {name} VID {vid}"
                    )));
                }
            }
            old.allowed_vlans = allowed.clone();
            port.allowed_vlans = allowed;
            if !same(old, &port) {
                return Err(error(format!("conflicting wire port identity {name}")));
            }
        } else {
            base.ports.insert(name, port);
        }
    }
    let added_mvrp: std::collections::BTreeSet<_> = added
        .adapters
        .iter()
        .filter(|a| a.protocol == WireProtocol::Mvrp)
        .flat_map(|a| a.ports.iter().cloned())
        .collect();
    let base_mvrp: std::collections::BTreeSet<_> = base
        .adapters
        .iter()
        .filter(|a| a.protocol == WireProtocol::Mvrp)
        .flat_map(|a| a.ports.iter().cloned())
        .collect();
    for adapter in added.adapters {
        if let Some(old) = base.adapters.iter().find(|old| old.id == adapter.id) {
            if !same(old, &adapter) {
                return Err(error(format!("conflicting wire adapter ID {}", adapter.id)));
            }
            continue;
        }
        for old in &base.adapters {
            if (old.protocol == adapter.protocol || tree(old.protocol) && tree(adapter.protocol))
                && old.ports.iter().any(|port| adapter.ports.contains(port))
            {
                return Err(error(format!(
                    "wire recipient conflict between {} and {}",
                    old.id, adapter.id
                )));
            }
        }
        base.adapters.push(adapter);
    }
    for packet in added.packets {
        if let Some(old) = base.packets.iter().find(|old| old.id == packet.id) {
            if !same(old, &packet) {
                return Err(error(format!("conflicting wire packet ID {}", packet.id)));
            }
        } else {
            base.packets.push(packet);
        }
    }
    base.svre_exists.extend(added.svre_exists);
    for (key, mode) in added.registrar_modes {
        if let Some(old) = base
            .registrar_modes
            .get(&key)
            .filter(|old| !same(*old, &mode))
        {
            if base_mvrp.contains(&key.0) && added_mvrp.contains(&key.0) {
                return Err(error(format!(
                    "conflicting wire registrar at {} VID {}",
                    key.0, key.1
                )));
            }
            if base_mvrp.contains(&key.0) {
                let _ = old;
                continue;
            }
        }
        base.registrar_modes.insert(key, mode);
    }
    let rank = |protocol| match protocol {
        WireProtocol::Rstp => 0,
        WireProtocol::Mstp => 1,
        WireProtocol::Mvrp => 2,
        WireProtocol::Igmp => 3,
        WireProtocol::Mld => 4,
        WireProtocol::MacControl => 5,
    };
    base.adapters.sort_by(|a, b| {
        (&a.bridge, rank(a.protocol), &a.id).cmp(&(&b.bridge, rank(b.protocol), &b.id))
    });
    base.packets
        .sort_by(|a, b| (a.arrival_ps, &a.id).cmp(&(b.arrival_ps, &b.id)));
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PreparedWire {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/active-multicast/model.ini"),
        )
        .unwrap()
        .registered
        .unwrap()
        .network
        .as_ref()
        .unwrap()
        .wire
        .clone()
        .unwrap()
    }
    #[test]
    fn identical_declarations_merge_and_conflicts_are_precise() {
        let prepared = fixture();
        let merged = merge(prepared.clone(), prepared.clone()).unwrap();
        assert_eq!(merged.adapters.len(), prepared.adapters.len());
        assert_eq!(merged.ports.len(), prepared.ports.len());
        let mut changed = prepared.clone();
        changed.adapters[0].max_pending_timers += 1;
        assert!(
            merge(prepared.clone(), changed)
                .unwrap_err()
                .message
                .contains("adapter ID")
        );
        let mut changed = prepared.clone();
        changed.adapters[0].id.push_str("_other");
        assert!(
            merge(prepared.clone(), changed)
                .unwrap_err()
                .message
                .contains("recipient conflict")
        );
        let mut changed = prepared.clone();
        changed.ports.values_mut().next().unwrap().pvid += 1;
        assert!(
            merge(prepared, changed)
                .unwrap_err()
                .message
                .contains("port identity")
        );
    }
}
