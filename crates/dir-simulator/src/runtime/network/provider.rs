//! Component-scoped forwarding for an explicitly configured provider bridge.
use super::*;
use provider_l2::{ClassifiedFrame, ComponentFdb, ControlDisposition, PortRole, ProviderTopology};
type ProviderPath = Vec<(String, u16, String, String)>;
type ProviderEgress = Vec<(String, ClassifiedFrame, ProviderPath)>;
type ComponentPolicies = BTreeMap<(String, u16), crate::snapshot::wire::WirePortPolicy>;

impl NetworkState {
    #[allow(clippy::too_many_arguments)] // Preserve the established owner transaction interface.
    pub(super) fn provider_record(
        &self,
        kind: &str,
        component: &str,
        vid: u16,
        port: &str,
        frame_id: &str,
        peer: Option<&str>,
        reason: Option<&str>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let id = format!("provider:{}:{}", frame_id, d.record_sequence);
        Self::record(
            c,
            d,
            "dir.network.provider",
            id,
            json!({
                "kind":kind,"component":component,"vid":vid,
                "port":port,"frame_id":frame_id,"peer":peer,"reason":reason,
            }),
        )
    }
    #[allow(clippy::too_many_arguments)] // Preserve the established owner transaction interface.
    fn provider_select(
        topology: &ProviderTopology,
        fdb: &mut ComponentFdb,
        class: ClassifiedFrame,
        now: u64,
        capacity: usize,
        age_ps: u64,
        policies: &ComponentPolicies,
        permitted: Option<&std::collections::BTreeSet<String>>,
        explicit: bool,
        mut visited: std::collections::BTreeSet<(String, String, u16)>,
        path: ProviderPath,
        egress: &mut ProviderEgress,
    ) -> Result<()> {
        if !visited.insert((
            class.component.clone(),
            class.ingress_port.clone(),
            class.vid,
        )) {
            return Ok(());
        }
        if visited.len() > topology.ports.len() * 4 {
            return Err(error("provider handoff traversal bound exceeded"));
        }
        if policies
            .get(&(class.ingress_port.clone(), class.vid))
            .is_some_and(|policy| !policy.link_up || (!explicit && !policy.learning))
        {
            return Ok(());
        }
        fdb.learn_with_limits(
            &class.component,
            class.vid,
            &class.wire.src_mac,
            &class.ingress_port,
            now,
            capacity,
            age_ps,
        );
        if !explicit
            && policies
                .get(&(class.ingress_port.clone(), class.vid))
                .is_some_and(|policy| !policy.forwarding)
        {
            return Ok(());
        }
        let known = fdb
            .lookup_at(&class.component, class.vid, &class.wire.dst_mac, now)
            .map(str::to_owned);
        for (name, port) in &topology.ports {
            if name == &class.ingress_port
                || port.component != class.component
                || !port.member_vids.contains(&class.vid)
                || policies
                    .get(&(name.clone(), class.vid))
                    .is_some_and(|policy| !policy.link_up || (!explicit && !policy.forwarding))
                || permitted.is_none() && known.as_ref().is_some_and(|wanted| wanted != name)
            {
                continue;
            }
            match port.role {
                PortRole::CustomerEdge | PortRole::ProviderNetwork => {
                    if permitted.is_none_or(|ports| ports.contains(name)) {
                        egress.push((name.clone(), class.clone(), path.clone()));
                    }
                }
                PortRole::ProviderEdge | PortRole::CustomerNetwork => {
                    for next in topology
                        .handoff_all(name, &class)
                        .map_err(|e| error(format!("provider service handoff: {}", e.message)))?
                    {
                        let mut next_path = path.clone();
                        next_path.push((
                            class.component.clone(),
                            class.vid,
                            name.clone(),
                            next.component.clone(),
                        ));
                        Self::provider_select(
                            topology,
                            fdb,
                            next,
                            now,
                            capacity,
                            age_ps,
                            policies,
                            permitted,
                            explicit,
                            visited.clone(),
                            next_path,
                            egress,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Apply component validation and tag handoffs to exactly one source-route
    /// output. The explicit adapter owns physical selection; this stage never
    /// adds a learned/flooded copy. Link and VLAN authorization still apply.
    pub(super) fn provider_explicit_egress(
        &self,
        ingress_port: &str,
        wire: &EthernetWireFrame,
        direction: usize,
        d: &mut NetworkDelta,
        frame_id: &str,
        c: &mut Context<'_>,
    ) -> Result<Option<(EthernetWireFrame, l2::Classification)>> {
        let Some(topology) = &self.prepared.provider_l2 else {
            return Ok(None);
        };
        let class = topology
            .classify_ingress(ingress_port, wire)
            .map_err(|e| error(format!("provider explicit ingress: {}", e.message)))?;
        self.provider_record(
            "ingress",
            &class.component,
            class.vid,
            ingress_port,
            frame_id,
            None,
            None,
            d,
            c,
        )?;
        let role = topology.ports[ingress_port].role;
        if matches!(
            provider_l2::control_disposition(role, &wire.dst_mac),
            ControlDisposition::Consume | ControlDisposition::Drop
        ) {
            return Ok(None);
        }
        let policies = topology
            .ports
            .iter()
            .flat_map(|(name, port)| {
                port.member_vids
                    .iter()
                    .filter_map(|vid| {
                        self.provider_tree_policy(name, *vid, d)
                            .map(|policy| ((name.clone(), *vid), policy))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let permitted = std::collections::BTreeSet::from([self.prepared.ethernet.directions
            [direction]
            .from_port
            .clone()]);
        let limits = self
            .prepared
            .dynamic
            .as_ref()
            .ok_or_else(|| error("provider dynamic limits missing"))?;
        let fdb = d
            .provider_fdb
            .get_or_insert_with(|| self.provider_fdb.clone().expect("provider FDB"));
        let mut outputs = Vec::new();
        Self::provider_select(
            topology,
            fdb,
            class,
            c.now(),
            limits.limits.mac_entries,
            limits.mac_age_ps,
            &policies,
            Some(&permitted),
            true,
            std::collections::BTreeSet::new(),
            vec![],
            &mut outputs,
        )?;
        if outputs.len() > 1 {
            return Err(error(
                "provider explicit output has ambiguous service handoff",
            ));
        }
        outputs
            .pop()
            .map(|(port, selected, path)| {
                for (component, vid, from, peer) in path {
                    self.provider_record(
                        "handoff",
                        &component,
                        vid,
                        &from,
                        frame_id,
                        Some(&peer),
                        None,
                        d,
                        c,
                    )?;
                }
                self.provider_record(
                    "egress",
                    &selected.component,
                    selected.vid,
                    &port,
                    frame_id,
                    None,
                    None,
                    d,
                    c,
                )?;
                let classification = l2::Classification {
                    vid: selected.vid,
                    priority: selected.priority,
                    dei: selected.dei,
                };
                topology
                    .encode_egress(&port, &selected)
                    .map(|wire| (wire, classification))
                    .map_err(|e| error(format!("provider explicit egress: {}", e.message)))
            })
            .transpose()
    }

    /// Static byte proof used when restoring a committed explicit relay. Tree
    /// state at restore can differ from the historical forwarding cut.
    pub(super) fn provider_explicit_wire(
        &self,
        ingress: &str,
        wire: &EthernetWireFrame,
        direction: usize,
    ) -> Result<Option<EthernetWireFrame>> {
        let topology = self
            .prepared
            .provider_l2
            .as_ref()
            .ok_or_else(|| error("provider topology"))?;
        let class = topology.classify_ingress(ingress, wire)?;
        let permitted = std::collections::BTreeSet::from([self.prepared.ethernet.directions
            [direction]
            .from_port
            .clone()]);
        let mut outputs = Vec::new();
        Self::provider_select(
            topology,
            &mut ComponentFdb::default(),
            class,
            0,
            provider_l2::FDB_MAX_ENTRIES,
            provider_l2::FDB_AGE_PS,
            &ComponentPolicies::new(),
            Some(&permitted),
            true,
            std::collections::BTreeSet::new(),
            vec![],
            &mut outputs,
        )?;
        if outputs.len() > 1 {
            return Err(error(
                "provider explicit output has ambiguous service handoff",
            ));
        }
        outputs
            .pop()
            .map(|(port, selected, _)| topology.encode_egress(&port, &selected))
            .transpose()
    }

    pub(super) fn provider_forward(
        &self,
        reception: &mut Reception,
        transfer: &Transfer,
        ingress_port: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let topology = self
            .prepared
            .provider_l2
            .as_ref()
            .ok_or_else(|| error("provider topology missing"))?;
        let class = match topology.classify_ingress(ingress_port, &transfer.row.wire) {
            Ok(class) => class,
            Err(e) => {
                reception.row.status = "filtered".into();
                reception.row.reason = Some("provider_ingress".into());
                self.provider_record(
                    "filtered",
                    "unknown",
                    0,
                    ingress_port,
                    &transfer.row.frame_id,
                    None,
                    Some(&e.message),
                    d,
                    c,
                )?;
                return Ok(());
            }
        };
        self.provider_record(
            "ingress",
            &class.component,
            class.vid,
            ingress_port,
            &transfer.row.frame_id,
            None,
            None,
            d,
            c,
        )?;
        let port = &topology.ports[ingress_port];
        match provider_l2::control_disposition(port.role, &class.wire.dst_mac) {
            ControlDisposition::Drop | ControlDisposition::Consume => {
                reception.row.status = "filtered".into();
                reception.row.reason = Some(
                    if provider_l2::control_disposition(port.role, &class.wire.dst_mac)
                        == ControlDisposition::Consume
                    {
                        "provider_control_consumed"
                    } else {
                        "provider_control_filtered"
                    }
                    .into(),
                );
                self.provider_record(
                    "filtered",
                    &class.component,
                    class.vid,
                    ingress_port,
                    &transfer.row.frame_id,
                    None,
                    reception.row.reason.as_deref(),
                    d,
                    c,
                )?;
                return Ok(());
            }
            _ => {}
        }
        let policies: ComponentPolicies = topology
            .ports
            .iter()
            .flat_map(|(name, port)| {
                port.member_vids
                    .iter()
                    .filter_map(|vid| {
                        self.provider_tree_policy(name, *vid, d)
                            .map(|policy| ((name.clone(), *vid), policy))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let link = self.effective_link(transfer.direction, reception.segment_receiver)?;
        let selected_route =
            self.qcc_route_at(transfer.frame, transfer.direction, link.destination, d);
        let permitted = selected_route
            .as_ref()
            .map(|selection| selection.ports.iter().cloned().collect());
        let fdb = d.provider_fdb.get_or_insert_with(|| {
            self.provider_fdb
                .clone()
                .expect("prepared provider has FDB")
        });
        let mut egress = Vec::new();
        let dynamic = self
            .prepared
            .dynamic
            .as_ref()
            .ok_or_else(|| error("provider dynamic limits missing"))?;
        Self::provider_select(
            topology,
            fdb,
            class.clone(),
            c.now(),
            dynamic.limits.mac_entries,
            dynamic.mac_age_ps,
            &policies,
            permitted.as_ref(),
            false,
            std::collections::BTreeSet::new(),
            vec![],
            &mut egress,
        )?;
        let mut ids = Vec::new();
        let mut handoffs = std::collections::BTreeSet::new();
        for (port, selected, path) in egress {
            for (component, vid, from_port, peer) in path {
                if handoffs.insert((component.clone(), vid, from_port.clone(), peer.clone())) {
                    self.provider_record(
                        "handoff",
                        &component,
                        vid,
                        &from_port,
                        &transfer.row.frame_id,
                        Some(&peer),
                        None,
                        d,
                        c,
                    )?;
                }
            }
            let direction = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|d| d.from_port == port)
                .ok_or_else(|| error("provider egress lacks direction"))?;
            let class = l2::Classification {
                vid: selected.vid,
                priority: selected.priority,
                dei: selected.dei,
            };
            let wire = topology
                .encode_egress(&port, &selected)
                .map_err(|e| error(format!("provider egress encoding: {}", e.message)))?;
            ids.extend(self.offer(
                CopyOrigin {
                    circuit: None,
                    frame: transfer.frame,
                    parent: Some(reception.transfer),
                    visit: reception.visit,
                },
                &wire,
                class,
                &[direction],
                d,
                c,
            )?);
            self.provider_record(
                "egress",
                &selected.component,
                selected.vid,
                &port,
                &transfer.row.frame_id,
                None,
                None,
                d,
                c,
            )?;
        }
        reception.row.egress_transfer_ids = ids;
        reception.row.status = if reception.row.egress_transfer_ids.is_empty() {
            "filtered"
        } else {
            "forwarded"
        }
        .into();
        if reception.row.egress_transfer_ids.is_empty() {
            reception.row.reason = Some(
                selected_route
                    .and_then(|selection| selection.reason)
                    .unwrap_or_else(|| "provider_no_egress".into()),
            );
            self.provider_record(
                "filtered",
                &class.component,
                class.vid,
                ingress_port,
                &transfer.row.frame_id,
                None,
                reception.row.reason.as_deref(),
                d,
                c,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn internal_component_tree_gates_learning_and_service_handoff() {
        let model: Value = serde_json::from_str(include_str!(
            "../../../../../examples/network-composite/provider-l2/model.json"
        ))
        .unwrap();
        let topology: ProviderTopology =
            serde_json::from_value(model["provider_l2"].clone()).unwrap();
        let wire = crate::runtime::ethernet::serialize_vlan_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x800,
            "1234",
            Some(crate::types::ethernet::EthernetVlanTag {
                vid: 10,
                pcp: 0,
                dei: 0,
            }),
        )
        .unwrap();
        let classified = topology.classify_ingress("Net.east.tx_a0", &wire).unwrap();
        for (port, vid, learning, forwarding, s_learns, pnp_transmits) in [
            ("Net.east.ca.pep", 10, true, false, false, false),
            ("Net.east.s.cnp_a", 100, false, false, false, false),
            ("Net.east.s.cnp_a", 100, true, false, true, false),
            ("Net.east.tx_pnp", 100, true, false, true, false),
            ("Net.east.tx_pnp", 100, true, true, true, true),
        ] {
            let policies = BTreeMap::from([(
                (port.into(), vid),
                crate::snapshot::wire::WirePortPolicy {
                    role: "Root".into(),
                    link_up: true,
                    learning,
                    forwarding,
                },
            )]);
            let mut fdb = ComponentFdb::default();
            let mut egress = vec![];
            NetworkState::provider_select(
                &topology,
                &mut fdb,
                classified.clone(),
                7,
                128,
                1000,
                &policies,
                None,
                false,
                BTreeSet::new(),
                vec![],
                &mut egress,
            )
            .unwrap();
            assert_eq!(
                fdb.lookup_at("Net.east.s", 100, &wire.src_mac, 7).is_some(),
                s_learns,
                "{port}"
            );
            assert_eq!(
                egress.iter().any(|(name, _, _)| name == "Net.east.tx_pnp"),
                pnp_transmits,
                "{port}"
            );
        }
    }
}
