//! Control FIFO shares the actual sampled MAC with data traffic.
use super::*;
use crate::runtime::ethernet::phy::{
    attachment::{AttachmentEffect, decode_mac},
    medium::PhyEffect,
};

impl NetworkState {
    pub(super) fn wire_phy_active(&self, direction: usize, d: &NetworkDelta) -> bool {
        self.wire_host(d).is_some_and(|host| {
            host.active[direction].as_ref().is_some_and(|id| {
                host.tx[id].phy_transfer.is_some() && host.tx[id].release.is_none()
            })
        })
    }

    pub(super) fn wire_phy_admit(
        &self,
        direction: usize,
        id: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let index = self
            .phy_attachment_for(direction)
            .ok_or_else(|| error("control PHY owner missing"))?;
        if self.prepared.phy_attachments[index]
            .config
            .medium
            .gigabit_raw_merge
        {
            return Err(error(
                "control FIFO needs the selected raw Merge express producer",
            ));
        }
        let port = self.prepared.ethernet.directions[direction]
            .from_port
            .clone();
        let tx = self.wire_host(d).unwrap().tx[id].clone();
        let maximum = self.prepared.phy_attachments[index]
            .config
            .medium
            .maximum_mac_bytes;
        let wire = decode_mac(&tx.intent.bytes, maximum).map_err(error)?.wire;
        let transfer = format!("wire-phy:{index}:{id}");
        let medium = self.phy_mut(d, index)?;
        // Protocol controls use their FIFO admission, independently of data gates.
        medium
            .set_mac_start_permission(&port, Some(u64::MAX))
            .map_err(error)?;
        medium
            .admit(&port, &transfer, c.now(), &wire)
            .map_err(error)?;
        let host = self.wire_mut(d)?;
        host.tx.get_mut(id).unwrap().phy_transfer = Some(transfer);
        host.active[direction] = Some(id.to_owned());
        self.phy_service_transition(direction, d, c)?;
        if self.prepared.phy_attachments[index]
            .config
            .medium
            .physical_mode
            .is_named()
        {
            self.phy_plan_boundary(index, d, c)?;
        }
        Ok(())
    }

    fn wire_phy_identity(&self, transfer: &str, d: &NetworkDelta) -> Option<(usize, String)> {
        let host = self.wire_host(d)?;
        let (id, tx) = host
            .tx
            .iter()
            .find(|(_, tx)| tx.phy_transfer.as_deref() == Some(transfer))?;
        let direction = self
            .prepared
            .ethernet
            .directions
            .iter()
            .position(|link| link.from_port == tx.intent.port)?;
        Some((direction, id.clone()))
    }

    /// Return true when this attachment effect belongs to the control FIFO.
    pub(super) fn wire_phy_effect(
        &self,
        index: usize,
        effect: &AttachmentEffect,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<bool> {
        let transfer = match effect {
            AttachmentEffect::Physical {
                effect:
                    PhyEffect::TxStarted { origin, .. }
                    | PhyEffect::TxMacPreamble { origin, .. }
                    | PhyEffect::TxMacSfd { origin, .. }
                    | PhyEffect::TxMacFcsEnd { origin, .. }
                    | PhyEffect::TxCodedEnd { origin, .. }
                    | PhyEffect::TxFinished { origin, .. }
                    | PhyEffect::Collision { origin, .. }
                    | PhyEffect::Retry { origin, .. }
                    | PhyEffect::RetryExhausted { origin, .. },
            } => Some(origin.frame_id.as_str()),
            AttachmentEffect::TxSerialized { transfer_id, .. }
            | AttachmentEffect::TxReleased { transfer_id, .. }
            | AttachmentEffect::Received { transfer_id, .. } => Some(transfer_id.as_str()),
            AttachmentEffect::PhysicalIngress {
                admitted_transfer_id,
                ..
            } => Some(admitted_transfer_id.as_str()),
            _ => None,
        };
        let Some((direction, id)) = transfer.and_then(|t| self.wire_phy_identity(t, d)) else {
            return Ok(false);
        };
        if self.phy_attachment_for(direction) != Some(index) {
            return Err(error("control physical observation crossed its owner"));
        }
        self.wire_mut(d)?.phase = 1;
        match effect {
            AttachmentEffect::Physical {
                effect: PhyEffect::TxStarted { sample_index, .. },
            } => {
                let sof = d.phy.as_ref().unwrap_or(&self.phy)[index]
                    .sample_time_ps(*sample_index)
                    .map_err(error)?;
                self.wire_mut(d)?
                    .tx
                    .get_mut(&id)
                    .unwrap()
                    .sof
                    .get_or_insert(sof);
                self.wire_tx_record(direction, &id, "tx_started", None, d, c)?;
            }
            AttachmentEffect::Physical {
                effect: PhyEffect::TxMacFcsEnd { sample_index, .. },
            } => {
                let eof = d.phy.as_ref().unwrap_or(&self.phy)[index]
                    .sample_time_ps(*sample_index)
                    .map_err(error)?;
                self.wire_mut(d)?.tx.get_mut(&id).unwrap().eof = Some(eof);
                self.pause_mac_completed(direction, d, c)?;
            }
            AttachmentEffect::TxSerialized {
                success, reason, ..
            } => {
                let tx = self.wire_mut(d)?.tx.get_mut(&id).unwrap();
                tx.eof.get_or_insert(c.now());
                if !success {
                    tx.dropped = true;
                }
                self.pause_mac_completed(direction, d, c)?;
                self.wire_tx_record(direction, &id, "tx_completed", reason.as_deref(), d, c)?;
            }
            AttachmentEffect::TxReleased { .. } => {
                let host = self.wire_mut(d)?;
                if host.active[direction].as_deref() != Some(&id) {
                    return Err(error("physical control release lacks FIFO claim"));
                }
                host.tx.get_mut(&id).unwrap().release = Some(c.now());
                host.active[direction] = None;
                self.phy_service_release(direction, d, c)?;
                c.request_arbitration("network").map_err(model)?;
            }
            AttachmentEffect::PhysicalIngress {
                receiver_tx_port,
                receiver_rx_port,
                frame,
                ..
            } => {
                let receiver = self
                    .prepared
                    .ethernet
                    .directions
                    .iter()
                    .position(|link| link.from_port == *receiver_tx_port)
                    .ok_or_else(|| error("physical control receiver is not an output"))?;
                if self
                    .phy_receiver_port(direction, receiver)
                    .is_none_or(|port| port.rx_port != *receiver_rx_port)
                {
                    return Err(error(
                        "physical control receiver crossed an electrical domain",
                    ));
                }
                let tx = self.wire_host(d).unwrap().tx[&id].clone();
                let actual = crate::runtime::ethernet::phy::attachment::canonical_bytes(
                    &frame.wire,
                    self.prepared.phy_attachments[index]
                        .config
                        .medium
                        .maximum_mac_bytes,
                )
                .map_err(error)?;
                if actual != tx.intent.bytes {
                    self.wire_tx_record(
                        direction,
                        &id,
                        "receive",
                        Some("phy_byte_mismatch"),
                        d,
                        c,
                    )?;
                    return Ok(true);
                }
                self.wire_phy_receive(direction, &id, receiver, receiver_rx_port, d, c)?;
            }
            AttachmentEffect::Received { frame, .. } => {
                let tx = self.wire_host(d).unwrap().tx[&id].clone();
                if frame.wire.mac_hex != crate::runtime::wire::codec::hex(&tx.intent.bytes) {
                    return Err(error("direct physical control bytes differ"));
                }
                let link = &self.prepared.ethernet.directions[direction];
                let peer = self
                    .prepared
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.ingress == link.to_port)
                    .map(|p| p.port.as_str())
                    .ok_or_else(|| error("direct physical control peer MAC missing"))?;
                let receiver = self
                    .prepared
                    .ethernet
                    .directions
                    .iter()
                    .position(|other| other.from_port == peer)
                    .ok_or_else(|| error("direct physical control reverse MAC missing"))?;
                let ingress = self.prepared.ethernet.directions[direction].to_port.clone();
                self.wire_phy_receive(direction, &id, receiver, &ingress, d, c)?;
            }
            _ => {}
        }
        Ok(true)
    }

    fn wire_phy_receive(
        &self,
        direction: usize,
        id: &str,
        receiver: usize,
        ingress: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let tx = self.wire_mut(d)?.tx.get_mut(id).unwrap();
        if tx
            .phy_receivers
            .insert(ingress.to_owned(), c.now())
            .is_some()
        {
            return Err(error("duplicate decoded physical control ingress"));
        }
        tx.arrival.get_or_insert(c.now());
        let tx = tx.clone();
        Self::record(
            c,
            d,
            "dir.network.phy",
            format!("phy:control:{id}@{ingress}"),
            json!({"kind":"control_decoded_ingress","packet_id":id,"receiver":ingress,
                "mac_hex":crate::runtime::wire::codec::hex(&tx.intent.bytes)}),
        )?;
        self.wire_deliver(
            direction,
            id,
            &tx,
            vec![(Some(receiver), ingress.to_owned())],
            d,
            c,
        )
    }
}
