//! Physical segment boundaries feed the existing TSN planner.
use super::super::*;
use super::integration::MergeEvent;
use crate::runtime::ethernet::tsn::merge::{MergeShaperView, Segment};
impl NetworkState {
    pub(super) fn merge_accounting_view(
        &self,
        i: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<MergeShaperView> {
        let state = self
            .merge_state(d)
            .ok_or_else(|| error("missing Merge owner"))?;
        let packet = state.ports[i].busy.and_then(|id| state.packets.get(&id));
        let class = packet.and_then(|p| self.wire_packet_class(p, d));
        packet_view(
            packet,
            class,
            now,
            self.prepared.ethernet.directions[i].bitrate_bps,
        )
    }
    pub(super) fn merge_accounting_schedule(
        &self,
        id: u64,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        self.due_mut(d).retain(
            |due| !matches!(&due.kind,Completion::MacMerge(MergeEvent::Accounting(x)) if *x==id),
        );
        let packet = self.merge_state(d).unwrap().packets[&id].clone();
        if let Some(plan) = packet.plan {
            let rate = self.prepared.ethernet.directions[packet.direction].bitrate_bps;
            let mut boundaries = vec![];
            if plan.start_offset > 0 {
                boundaries.push(add(
                    packet.sof,
                    super::bit_duration(64, rate).map_err(super::integration::err)?,
                )?);
            }
            if !plan.final_packet {
                boundaries.push(
                    packet.eof - super::bit_duration(32, rate).map_err(super::integration::err)?,
                );
            }
            for at in boundaries {
                if at > now {
                    self.merge_due(d, at, MergeEvent::Accounting(id));
                }
            }
        }
        Ok(())
    }
    pub(super) fn merge_shaper_transition(
        &self,
        i: usize,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        let view = self.merge_accounting_view(i, d, now)?;
        if let Some(tsn) = &self.tsn {
            if let Some(packet) = self.merge_state(d).unwrap().ports[i]
                .busy
                .and_then(|id| self.merge_state(d).unwrap().packets.get(&id))
                .cloned()
            {
                if let Some(class) = self.wire_packet_class(&packet, d) {
                    let port = &self.merge_state(d).unwrap().ports[i];
                    let name = &self.prepared.ethernet.directions[i].from_port;
                    let open = tsn.gate_open_clock_in(
                        &d.tsn,
                        name,
                        class,
                        now,
                        self.clock_for_port(d, name),
                    )?;
                    if now < packet.eof && !open && port.overrun_packet != Some(packet.id) {
                        self.merge_mut(d)?.ports[i].overrun_packet = Some(packet.id);
                        self.merge_port_record(d, i, now, "transmission_overrun")?;
                        let extra = json!({"packet_id":packet.id.to_string(),"priority":class.to_string(),"observed_ps":now.to_string()});
                        self.merge_mut(d)?
                            .records
                            .last_mut()
                            .unwrap()
                            .2
                            .as_object_mut()
                            .unwrap()
                            .extend(extra.as_object().unwrap().clone());
                    }
                }
            }
        }
        self.merge_mut(d)?.ports[i].accounting = Some(view);
        self.merge_mut(d)?.ports[i].accounting_at = now;
        if let Some(tsn) = &self.tsn {
            // The common phase-0 GCL owner integrates the old segment before
            // replacing the schedule, then consumes this post-change view.
            if tsn.merge_control_due(now) {
                return Ok(());
            }
            let port = self.prepared.ethernet.directions[i].from_port.clone();
            let input = self.port_input(i, d)?;
            let clock = self.clock_for_port(d, &port).cloned();
            let delta = tsn.plan_merge_transition_clock_in(
                std::mem::take(&mut d.tsn),
                &port,
                now,
                &input,
                clock.as_ref(),
            )?;
            self.stage_tsn(d, delta, &[port])?;
        }
        Ok(())
    }
}

pub(super) fn packet_view(
    packet: Option<&super::integration::Packet>,
    class: Option<u8>,
    now: u64,
    rate: u64,
) -> Result<MergeShaperView> {
    let Some(packet) = packet else {
        return Ok(MergeShaperView {
            medium_busy_until: None,
            transmit_class: None,
            next_boundary: None,
            segment: Segment::Idle,
        });
    };
    let bits = |n| super::bit_duration(n, rate).map_err(super::integration::err);
    let mut transmit = class;
    let mut segment = if transmit.is_some() {
        Segment::OriginalMac
    } else {
        Segment::Control
    };
    let mut next = packet.release;
    if now < packet.eof {
        next = packet.eof;
    }
    if let Some(plan) = &packet.plan {
        if plan.start_offset > 0 && now < add(packet.sof, bits(64)?)? {
            segment = Segment::AddedContinuationHeader;
            transmit = None;
            next = add(packet.sof, bits(64)?)?;
        } else if !plan.final_packet && now >= packet.eof - bits(32)? && now < packet.eof {
            segment = Segment::AddedMcrc;
            transmit = None;
        } else if !plan.final_packet && now < packet.eof - bits(32)? {
            next = packet.eof - bits(32)?;
        }
        if now >= packet.eof {
            if plan.final_packet {
                segment = Segment::FinalOriginalIpg;
            } else {
                segment = Segment::AddedInterruptionIpg;
                transmit = None;
            }
        }
    } else if transmit.is_some() && now >= packet.eof {
        segment = Segment::FinalOriginalIpg;
    }
    Ok(MergeShaperView {
        medium_busy_until: Some(packet.release),
        transmit_class: transmit,
        next_boundary: (next > now).then_some(next),
        segment,
    })
}
