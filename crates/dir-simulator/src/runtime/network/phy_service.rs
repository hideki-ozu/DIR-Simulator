//! The existing physical MAC owns deferral/retry; TSN observes its actual service.
use super::*;
use crate::runtime::ethernet::phy::{
    medium::{PhyEffect, PhysicalMode},
    wire::PILOT_BYTES,
};
use crate::runtime::ethernet::tsn::merge::{MacServiceView, Segment, ServiceAccounting};

impl NetworkState {
    pub(super) fn phy_service_view(&self, direction: usize, d: &NetworkDelta) -> MacServiceView {
        if self.wire_phy_active(direction, d) {
            let host = self.wire_host(d).unwrap();
            let id = host.active[direction].as_ref().unwrap();
            let tx = &host.tx[id];
            let priority = (!tx.is_mac_control()).then(|| self.wire_priority(direction, id, d));
            let mut backlog = [false; 8];
            if let Some(p) = priority {
                backlog[p as usize] = true;
            }
            return MacServiceView {
                active_work: Some(id.clone()),
                retained_backlog: backlog,
                transmit_class: if tx.sof.is_some() && tx.eof.is_none() {
                    priority
                } else {
                    None
                },
                busy: true,
                release_ps: None,
                next_transition_ps: None,
                segment: if tx.eof.is_some() {
                    Segment::SampledPhysicalTail
                } else {
                    Segment::Control
                },
                accounting: ServiceAccounting::SampledWire,
            };
        }
        d.phy_services
            .as_ref()
            .unwrap_or(&self.phy_services)
            .get(&direction)
            .cloned()
            .unwrap_or(MacServiceView {
                active_work: None,
                retained_backlog: [false; 8],
                transmit_class: None,
                busy: false,
                release_ps: None,
                next_transition_ps: None,
                segment: Segment::Idle,
                accounting: ServiceAccounting::SampledWire,
            })
    }

    pub(super) fn phy_occupancy_ps(&self, direction: usize, mac_bytes: u64) -> Result<u64> {
        let index = self
            .phy_attachment_for(direction)
            .ok_or_else(|| error("missing physical occupancy owner"))?;
        let cfg = &self.prepared.phy_attachments[index].config.medium;
        let chip_ps = 1_000_000_000_000u128.div_ceil(u128::from(cfg.channel.grid.symbol_rate_hz));
        let (chips, ifg_ps) = match cfg.physical_mode {
            // Exact Program::len plus its final chip boundary; PLCA/carrier
            // deference is governed by permission, outside occupied service.
            PhysicalMode::Ieee10BaseT1sMultidrop2022 => (
                (18 + 2 * u128::from(mac_bytes)) * 10 + 4,
                u128::from(cfg.ifg_bits) * 100_000,
            ),
            PhysicalMode::Ieee10BaseT2022 => (
                (8 + u128::from(mac_bytes)) * 16 + 1,
                u128::from(cfg.ifg_bits) * 100_000,
            ),
            PhysicalMode::ProjectNumerical => (
                (u128::from(PILOT_BYTES as u64) + 8 + u128::from(mac_bytes)) * 16 + 1,
                u128::from(cfg.ifg_bits) * 2 * chip_ps,
            ),
            // Canonical GMII programs start at a complete PHY frame and are
            // retained through its systematic/FEC tail before physical release.
            PhysicalMode::Ieee1000BaseT1PointToPoint2022 => {
                let slots = u128::from(mac_bytes) + 8 + 12;
                (
                    slots.div_ceil(450) * 2700 + 1,
                    u128::from(cfg.ifg_bits) * 1000,
                )
            }
        };
        u64::try_from(
            chips
                .checked_mul(chip_ps)
                .and_then(|n| n.checked_add(ifg_ps))
                .ok_or_else(|| error("physical occupancy overflow"))?,
        )
        .map_err(|_| error("physical occupancy overflow"))
    }

    pub(super) fn phy_service_claim(
        &self,
        direction: usize,
        transfer: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if !self.prepared.has_phy_mac_service() {
            return Ok(());
        }
        let row = &self.transfer(d, transfer).row;
        let mut retained_backlog = [false; 8];
        retained_backlog[row.priority as usize] = true;
        let view = MacServiceView {
            active_work: Some(row.transfer_id.clone()),
            retained_backlog,
            transmit_class: None,
            busy: true,
            release_ps: None,
            next_transition_ps: None,
            segment: Segment::Idle,
            accounting: ServiceAccounting::SampledWire,
        };
        d.phy_services
            .get_or_insert_with(|| self.phy_services.clone())
            .insert(direction, view);
        self.phy_service_transition(direction, d, c)
    }

    pub(super) fn phy_service_effect(
        &self,
        effect: &PhyEffect,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if !self.prepared.has_phy_mac_service() {
            return Ok(());
        }
        let (origin, phase) = match effect {
            PhyEffect::TxStarted { origin, .. } => (origin, 0),
            PhyEffect::Collision { origin, .. } => (origin, 1),
            PhyEffect::Retry { origin, .. } => (origin, 2),
            PhyEffect::TxMacFcsEnd { origin, .. } => (origin, 3),
            PhyEffect::RetryExhausted { origin, .. } => (origin, 4),
            _ => return Ok(()),
        };
        let transfer = self
            .transfers
            .iter()
            .position(|t| t.row.transfer_id == origin.frame_id)
            .or_else(|| {
                d.transfers
                    .iter()
                    .find(|(_, t)| t.row.transfer_id == origin.frame_id)
                    .map(|(i, _)| *i)
            })
            .ok_or_else(|| error("physical service references unknown work"))?;
        let row = &self.transfer(d, transfer).row;
        let direction = self.transfer(d, transfer).direction;
        let priority = row.priority;
        let mut view = self.phy_service_view(direction, d);
        if view.active_work.as_deref() != Some(row.transfer_id.as_str()) || !view.busy {
            return Err(error(
                "physical service effect does not belong to retained MAC claim",
            ));
        }
        match phase {
            0 => {
                view.transmit_class = Some(priority);
                view.segment = Segment::OriginalMac;
            }
            1 => {
                view.transmit_class = None;
                view.segment = Segment::Control;
            }
            2 => {
                view.transmit_class = None;
                view.segment = Segment::Idle;
            }
            3 => {
                self.pause_mac_completed(direction, d, c)?;
                view.segment = Segment::SampledPhysicalTail;
            }
            4 => {
                view.transmit_class = None;
                view.retained_backlog = [false; 8];
                view.segment = Segment::Idle;
            }
            _ => unreachable!(),
        }
        d.phy_services
            .get_or_insert_with(|| self.phy_services.clone())
            .insert(direction, view);
        self.phy_service_transition(direction, d, c)
    }

    pub(super) fn phy_service_release(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if !self.prepared.has_phy_mac_service() {
            return Ok(());
        }
        d.phy_services
            .get_or_insert_with(|| self.phy_services.clone())
            .remove(&direction);
        self.phy_service_transition(direction, d, c)
    }

    pub(super) fn phy_service_transition(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if let Some(tsn) = &self.tsn {
            if tsn.merge_control_due(c.now()) {
                return Ok(());
            }
            let port = self.prepared.ethernet.directions[direction]
                .from_port
                .clone();
            let input = self.port_input(direction, d)?;
            let clock = self.clock_for_port(d, &port).cloned();
            let delta = tsn.plan_port_clock_in(
                std::mem::take(&mut d.tsn),
                &port,
                c.now(),
                &input,
                clock.as_ref(),
            )?;
            self.stage_tsn(d, delta, std::slice::from_ref(&port))?;
        }
        self.phy_set_start_permission(direction, d, c, false)
    }

    pub(super) fn phy_set_start_permission(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        replan: bool,
    ) -> Result<()> {
        if !self.prepared.has_phy_mac_service() {
            return Ok(());
        }
        let Some(index) = self.phy_attachment_for(direction) else {
            return Ok(());
        };
        let port = self.prepared.ethernet.directions[direction]
            .from_port
            .clone();
        let allowed = if self.wire_phy_active(direction, d) {
            let tx = &self.wire_host(d).unwrap().tx[self.wire_host(d).unwrap().active[direction]
                .as_ref()
                .unwrap()];
            if tx.sof.is_some() && tx.eof.is_none() {
                Some(u64::MAX)
            } else {
                self.wire_start_allowed(
                    direction,
                    self.wire_host(d).unwrap().active[direction]
                        .as_ref()
                        .unwrap(),
                    d,
                    c.now(),
                )?
                .then_some(u64::MAX)
            }
        } else if let Some(t) = self.queue(d, direction).active {
            if self.transfer(d, t).row.sof_ps.is_some() && self.transfer(d, t).row.eof_ps.is_none()
            {
                Some(u64::MAX)
            } else if !self.output_request_allowed(direction, false, d, c.now()) {
                None
            } else if let Some(tsn) = &self.tsn {
                let row = &self.transfer(d, t).row;
                tsn.retained_start_permission_clock_in(
                    &d.tsn,
                    &port,
                    row.priority,
                    c.now(),
                    self.phy_occupancy_ps(direction, row.wire.mac_bytes)?,
                    self.clock_for_port(d, &port),
                )?
            } else {
                Some(u64::MAX)
            }
        } else {
            None
        };
        if d.phy.as_ref().unwrap_or(&self.phy)[index]
            .mac_start_permission(&port)
            .map_err(error)?
            != allowed
        {
            self.phy_mut(d, index)?
                .set_mac_start_permission(&port, allowed)
                .map_err(error)?;
            if replan
                && self.prepared.phy_attachments[index]
                    .config
                    .medium
                    .physical_mode
                    .is_named()
            {
                self.phy_plan_boundary(index, d, c)?;
            }
        }
        Ok(())
    }
}
