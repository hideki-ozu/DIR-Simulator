//! Independent chronological reconstruction of the bounded packet protocol.
//! Physical ownership is checked in checkpoint.rs; this layer never trusts a
//! saved winner, sample index, pending exchange, or application clock anchor.
use super::super::*;
use super::codec::{Body, Message};
use super::integration::{
    application_rate, application_target, from_wire_correction, gerr, stamp_with_correction,
};
use super::math::{self, Rat};
use super::state::{self, CauseKind, Role, Timer};
use crate::types::network::gptp::PreparedGptp;
use std::collections::{BTreeMap, BTreeSet};

struct Replay<'a> {
    owner: &'a NetworkState,
    cfg: &'a PreparedGptp,
    saved: &'a state::State,
    state: state::State,
    clocks: Vec<clock::NodeClock>,
    seen: BTreeSet<(u8, usize)>,
    receptions: BTreeSet<usize>,
    expected_timers: BTreeMap<(u64, Timer), usize>,
    last_at: u64,
}
impl<'a> Replay<'a> {
    fn new(
        owner: &'a NetworkState,
        saved: &'a state::State,
        initial: &[clock::NodeClock],
    ) -> Result<Self> {
        let cfg = owner
            .gcfg()
            .ok_or_else(|| gerr("replay has no prepared instance"))?;
        let mut state = state::State::new(cfg);
        for (direction, prepared) in cfg.ports.iter().enumerate() {
            if let Some(prepared) = prepared {
                let due = owner.gptp_raw(prepared.node, 0)?;
                let port = state.ports[direction].as_mut().unwrap();
                port.pdelay_due_raw = Some(due);
                port.announce_due_raw = Some(due);
                port.sync_due_raw = Some(due);
            }
        }
        super::selection::apply(cfg, &mut state, 0)?;
        Ok(Self {
            owner,
            cfg,
            saved,
            state,
            clocks: initial.to_vec(),
            seen: BTreeSet::new(),
            receptions: BTreeSet::new(),
            expected_timers: BTreeMap::new(),
            last_at: 0,
        })
    }
    fn select(&mut self, at: u64) -> Result<()> {
        super::selection::apply(self.cfg, &mut self.state, at)?;
        Ok(())
    }
    fn port(&self, direction: usize) -> Result<&state::Port> {
        self.state
            .ports
            .get(direction)
            .and_then(Option::as_ref)
            .ok_or_else(|| gerr("causal replay port absent"))
    }
    fn port_mut(&mut self, direction: usize) -> Result<&mut state::Port> {
        self.state
            .ports
            .get_mut(direction)
            .and_then(Option::as_mut)
            .ok_or_else(|| gerr("causal replay port absent"))
    }
    fn packet(&self, transfer: usize) -> Result<&state::Packet> {
        self.state
            .packets
            .get(&transfer)
            .ok_or_else(|| gerr("causal replay packet absent"))
    }
    fn saved_packet(&self, transfer: usize) -> Result<&state::Packet> {
        self.saved
            .packets
            .get(&transfer)
            .ok_or_else(|| gerr("causal history references absent packet"))
    }
    fn raw_period(&self, node: usize, raw: Rat, interval: u64, at: u64) -> Result<Rat> {
        if self.owner.gptp_global_deadline(node, raw)? != at {
            return Err(gerr("causal timer differs from raw LocalClock phase"));
        }
        raw.add(Rat::new(i128::from(interval), 1).map_err(gerr)?)
            .map_err(gerr)
    }
    fn reset_pdelay(&mut self, direction: usize, generation: u64, at: u64) -> Result<()> {
        if self
            .port(direction)?
            .pending
            .as_ref()
            .is_none_or(|p| p.generation != generation)
        {
            return Ok(());
        }
        let allowed = self.cfg.allowed_lost_responses;
        let p = self.port_mut(direction)?;
        p.reset_after_lost_response(allowed);
        self.select(at)
    }
    fn timer(&mut self, timer: Timer, at: u64) -> Result<()> {
        match timer {
            Timer::Pdelay {
                direction,
                generation,
            } => {
                let p = self.port(direction)?.clone();
                if p.pdelay_generation != generation {
                    return Ok(());
                }
                let node = self.cfg.ports[direction].as_ref().unwrap().node;
                let due = p
                    .pdelay_due_raw
                    .ok_or_else(|| gerr("replay Pdelay raw deadline absent"))?;
                let next = self.raw_period(node, due, self.cfg.pdelay_interval_ps, at)?;
                if let Some(exchange) = p.pending {
                    self.reset_pdelay(direction, exchange.generation, at)?;
                }
                let port = self.port_mut(direction)?;
                port.pdelay_generation = generation
                    .checked_add(1)
                    .ok_or_else(|| gerr("replay generation overflow"))?;
                port.pdelay_due_raw = Some(next);
            }
            Timer::PdelayTimeout {
                direction,
                generation,
            } => {
                if self
                    .port(direction)?
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.generation == generation && p.deadline_ps <= at)
                {
                    self.reset_pdelay(direction, generation, at)?;
                }
            }
            Timer::Announce {
                direction,
                generation,
            } => {
                let p = self.port(direction)?.clone();
                if p.announce_generation != generation {
                    return Ok(());
                }
                let node = self.cfg.ports[direction].as_ref().unwrap().node;
                let due = p
                    .announce_due_raw
                    .ok_or_else(|| gerr("replay Announce raw deadline absent"))?;
                let next = self.raw_period(node, due, self.cfg.announce_interval_ps, at)?;
                let port = self.port_mut(direction)?;
                port.announce_generation = generation
                    .checked_add(1)
                    .ok_or_else(|| gerr("replay generation overflow"))?;
                port.announce_due_raw = Some(next);
            }
            Timer::AnnounceTimeout {
                direction,
                generation,
            } => {
                if self
                    .port(direction)?
                    .candidate
                    .as_ref()
                    .is_some_and(|a| a.generation == generation && a.deadline_ps <= at)
                {
                    self.port_mut(direction)?.candidate = None;
                    self.select(at)?;
                }
            }
            Timer::Sync {
                direction,
                generation,
            } => {
                let p = self.port(direction)?.clone();
                if p.sync_generation != generation {
                    return Ok(());
                }
                let node = self.cfg.ports[direction].as_ref().unwrap().node;
                let due = p
                    .sync_due_raw
                    .ok_or_else(|| gerr("replay Sync raw deadline absent"))?;
                let next = self.raw_period(node, due, self.cfg.sync_interval_ps, at)?;
                let port = self.port_mut(direction)?;
                port.sync_generation = generation
                    .checked_add(1)
                    .ok_or_else(|| gerr("replay generation overflow"))?;
                port.sync_due_raw = Some(next);
            }
            Timer::SyncTimeout { node, generation } => {
                let n = self
                    .state
                    .nodes
                    .get(node)
                    .ok_or_else(|| gerr("replay Sync node absent"))?
                    .clone();
                if n.sync_timeout_generation == generation
                    && n.sync_deadline_ps.is_some_and(|t| t <= at)
                {
                    self.state.nodes[node].synchronized = false;
                    self.state.nodes[node].relay = None;
                    if let Some(parent) = n.parent {
                        self.port_mut(parent)?.candidate = None;
                    }
                    self.select(at)?;
                }
            }
        }
        Ok(())
    }
    fn sent(&mut self, transfer: usize, at: u64) -> Result<()> {
        let original = self.saved_packet(transfer)?.clone();
        let row = self
            .owner
            .transfers
            .get(transfer)
            .ok_or_else(|| gerr("replay sent transfer absent"))?;
        if self.state.packets.contains_key(&transfer)
            || original.id != self.state.next_packet
            || row.row.queued_ps != at
            || original.link_epoch != self.port(original.direction)?.link_epoch
            || original.message.source != self.owner.gptp_port_identity(original.direction)
        {
            return Err(gerr("causal packet send owner/order differs"));
        }
        let direction = original.direction;
        let node = self.cfg.ports[direction].as_ref().unwrap().node;
        let queued = original.admitted;
        match &original.message.body {
            Body::PdelayReq => {
                let p = self.port(direction)?.clone();
                if !p.link_up
                    || original.message.sequence != p.pdelay_seq
                    || p.pending.is_some()
                    || p.pdelay_generation == 0
                {
                    return Err(gerr("causal Pdelay request state differs"));
                }
                if queued {
                    let deadline_raw = p
                        .pdelay_due_raw
                        .ok_or_else(|| gerr("Pdelay phase absent"))?;
                    let deadline_ps = self.owner.gptp_global_deadline(node, deadline_raw)?;
                    let port = self.port_mut(direction)?;
                    port.pdelay_seq = port.pdelay_seq.wrapping_add(1);
                    port.pending = Some(state::PdelayPending {
                        generation: p.pdelay_generation,
                        sequence: original.message.sequence,
                        request_transfer: transfer,
                        t1: None,
                        t2: None,
                        t3: None,
                        t4: None,
                        peer: None,
                        response_transfer: None,
                        followup_transfer: None,
                        deadline_raw,
                        deadline_ps,
                    });
                    self.expect_timer(
                        deadline_ps,
                        Timer::PdelayTimeout {
                            direction,
                            generation: p.pdelay_generation,
                        },
                    );
                }
            }
            Body::Announce(_) => {
                let p = self.port(direction)?.clone();
                if p.role != Role::Master
                    || !p.link_up
                    || !p.as_capable
                    || original.message.sequence != p.announce_seq
                {
                    return Err(gerr("causal Announce send state differs"));
                }
                self.port_mut(direction)?.announce_seq = p.announce_seq.wrapping_add(1);
            }
            Body::Sync => {
                let p = self.port(direction)?.clone();
                let n = self.state.nodes[node].clone();
                if p.role != Role::Master
                    || !p.link_up
                    || !p.as_capable
                    || !n.synchronized
                    || (n.parent.is_some() && n.relay.is_none())
                    || original.message.sequence != p.sync_seq
                {
                    return Err(gerr("causal Sync send state differs"));
                }
                self.port_mut(direction)?.sync_seq = p.sync_seq.wrapping_add(1);
                if serde_json::to_value(&original.relay).map_err(|e| gerr(e.to_string()))?
                    != serde_json::to_value(&n.relay).map_err(|e| gerr(e.to_string()))?
                {
                    return Err(gerr("causal relay source differs"));
                }
            }
            _ => {}
        }
        let mut packet = original;
        packet.mac_packet = None;
        packet.physical_epoch = None;
        packet.received_mac_hex = None;
        packet.received_message = None;
        packet.tx = None;
        packet.rx = None;
        packet.rx_validated = false;
        packet.tx_eof = false;
        packet.mac_source = None;
        packet.tx_completed_at_ps = None;
        self.state.next_packet = self
            .state
            .next_packet
            .checked_add(1)
            .ok_or_else(|| gerr("packet id overflow"))?;
        self.state.packets.insert(transfer, packet);
        Ok(())
    }
    fn link(&mut self, direction: usize, up: bool, at: u64) -> Result<()> {
        let prepared = self
            .owner
            .prepared
            .dynamic
            .as_ref()
            .ok_or_else(|| gerr("link cause has no prepared dynamic control"))?;
        let port_name = &self.owner.prepared.ethernet.directions[direction].from_port;
        let initial_down = at == 0
            && !up
            && prepared
                .links
                .iter()
                .any(|l| l.ports.contains(port_name) && !l.up);
        if !initial_down && !prepared.controls.iter().any(|control| control.at_ps == at
            && matches!(&control.op, crate::types::ethernet::dynamic::ControlOp::LinkSet { link, up: value }
                if *value == up && prepared.links.iter().any(|l| &l.id == link && l.ports.contains(port_name)))) {
            return Err(gerr("causal link transition has no prepared control"));
        }
        let p = self.port_mut(direction)?;
        if p.link_up == up {
            return Err(gerr("duplicate causal link transition"));
        }
        p.link_up = up;
        p.link_epoch = p
            .link_epoch
            .checked_add(1)
            .ok_or_else(|| gerr("link epoch overflow"))?;
        p.pending = None;
        p.last_sample = None;
        p.last_sample_peer = None;
        p.last_sample_index = None;
        p.rate = None;
        p.delay = None;
        p.as_capable = false;
        p.lost_responses = 0;
        p.candidate = None;
        p.peer = None;
        p.role = Role::Disabled;
        self.select(at)
    }
    fn track(&mut self, kind: u8, transfer: usize) -> Result<()> {
        if !self.seen.insert((kind, transfer)) {
            return Err(gerr("duplicate causal physical event"));
        }
        Ok(())
    }
    fn expect_timer(&mut self, at: u64, timer: Timer) {
        *self.expected_timers.entry((at, timer)).or_default() += 1;
    }
    fn step(&mut self, cause: &state::Cause, horizon: u64) -> Result<()> {
        let at = cause.at_ps;
        if at < self.last_at || at > horizon {
            return Err(gerr("causal history time out of order/horizon"));
        }
        self.last_at = at;
        match cause.kind {
            CauseKind::Timer(timer) => self.timer(timer, at)?,
            CauseKind::Sent { transfer } => {
                self.track(0, transfer)?;
                self.sent(transfer, at)?;
            }
            CauseKind::Sof { transfer } => {
                self.track(1, transfer)?;
                let saved = self.saved_packet(transfer)?.clone();
                if self.owner.transfers[transfer].row.sof_ps != Some(at)
                    || self.packet(transfer)?.link_epoch != self.port(saved.direction)?.link_epoch
                {
                    return Err(gerr("causal SOF differs from physical transfer"));
                }
                let packet = self.state.packets.get_mut(&transfer).unwrap();
                packet.mac_packet = saved.mac_packet;
                packet.physical_epoch = saved.physical_epoch;
                packet.mac_source = saved.mac_source;
            }
            CauseKind::Capture { transfer, kind } => {
                self.track(if kind == state::CaptureKind::Tx { 2 } else { 3 }, transfer)?;
                let evidence = self.saved_packet(transfer)?.clone();
                let capture = if kind == state::CaptureKind::Tx {
                    evidence.tx
                } else {
                    evidence.rx
                }
                .ok_or_else(|| gerr("causal timestamp lacks physical capture"))?;
                if capture.at_ps != at || self.packet(transfer)?.link_epoch != capture.link_epoch {
                    return Err(gerr("causal capture time/link differs"));
                }
                let packet = self.state.packets.get_mut(&transfer).unwrap();
                if kind == state::CaptureKind::Tx {
                    packet.tx = Some(capture);
                } else {
                    packet.rx = Some(capture);
                }
            }
            CauseKind::SampledCapture { .. } => {
                // This private arithmetic replay certifies the ideal adapter.
                // Sampled markers are source-bound by fresh Engine prefix replay.
                return Err(gerr(
                    "sampled capture requires source-bound engine prefix replay",
                ));
            }
            CauseKind::PhysicalReadiness { .. } => {
                return Err(gerr(
                    "sampled readiness requires source-bound engine prefix replay",
                ));
            }
            CauseKind::Eof { transfer } => {
                self.track(4, transfer)?;
                if self.owner.transfers[transfer].row.eof_ps != Some(at) {
                    return Err(gerr("causal EOF differs from physical transfer"));
                }
                self.state.packets.get_mut(&transfer).unwrap().tx_eof = true;
            }
            CauseKind::TxComplete { transfer } => {
                self.track(6, transfer)?;
                let p = self.packet(transfer)?;
                let capture =
                    p.tx.as_ref()
                        .ok_or_else(|| gerr("completion before TX capture"))?;
                let eof = self.owner.transfers[transfer]
                    .row
                    .eof_ps
                    .ok_or_else(|| gerr("completion before EOF"))?;
                if !p.tx_eof || p.tx_completed_at_ps.is_some() || at != eof.max(capture.at_ps) {
                    return Err(gerr("causal TX completion join differs"));
                }
                self.state
                    .packets
                    .get_mut(&transfer)
                    .unwrap()
                    .tx_completed_at_ps = Some(at);
            }
            CauseKind::Receive { transfer } => {
                self.track(5, transfer)?;
                self.receptions.insert(transfer);
                self.receive(transfer, at)?;
            }
            CauseKind::Link { direction, up } => self.link(direction, up, at)?,
        }
        Ok(())
    }
}
impl Replay<'_> {
    fn receive(&mut self, transfer: usize, at: u64) -> Result<()> {
        let saved = self.saved_packet(transfer)?.clone();
        let packet = self.packet(transfer)?.clone();
        let direction = packet.direction;
        let p = self.port(direction)?;
        let stale = p.link_epoch != packet.link_epoch || !p.link_up;
        let reason = stale || !packet.tx_eof || packet.rx.is_none();
        let mac_hex = saved
            .received_mac_hex
            .as_ref()
            .ok_or_else(|| gerr("causal ingress lacks received MAC bytes"))?;
        let data = super::receive::bytes(mac_hex)?;
        let decoded = if reason {
            None
        } else {
            super::receive::validate_mac(&data)
                .ok()
                .and_then(|payload| Message::decode(payload).ok())
        };
        let replay = self.state.packets.get_mut(&transfer).unwrap();
        replay.received_mac_hex = Some(mac_hex.clone());
        replay.received_message = decoded.clone();
        let Some(message) = decoded else {
            return Ok(());
        };
        if message.source != packet.message.source
            || message.sequence != packet.message.sequence
            || message.body.message_type() != packet.message.body.message_type()
        {
            return Ok(());
        }
        self.state.packets.get_mut(&transfer).unwrap().rx_validated = true;
        let raw = packet.rx.as_ref().unwrap().raw;
        let ingress = self.cfg.reverse[direction];
        self.process(ingress, transfer, message, raw, at)
    }
    fn process(
        &mut self,
        ingress: usize,
        transfer: usize,
        m: Message,
        rx_raw: Rat,
        at: u64,
    ) -> Result<()> {
        let node = self.cfg.ports[ingress].as_ref().unwrap().node;
        let own = self.owner.gptp_port_identity(ingress);
        if m.source.clock == own.clock {
            if matches!(
                m.body,
                Body::PdelayResp { .. } | Body::PdelayRespFollowUp { .. }
            ) {
                if let Some(generation) = self.port(ingress)?.pending.as_ref().map(|p| p.generation)
                {
                    self.reset_pdelay(ingress, generation, at)?;
                }
            }
            return Ok(());
        }
        match m.body {
            Body::PdelayReq => {}
            Body::PdelayResp {
                request_receipt,
                requester,
            } => {
                if requester != own {
                    if let Some(generation) =
                        self.port(ingress)?.pending.as_ref().map(|p| p.generation)
                    {
                        self.reset_pdelay(ingress, generation, at)?;
                    }
                    return Ok(());
                }
                let Some(pending) = self.port(ingress)?.pending.clone() else {
                    return Ok(());
                };
                if pending.sequence != m.sequence || pending.deadline_ps <= at {
                    if pending.sequence != m.sequence {
                        self.reset_pdelay(ingress, pending.generation, at)?;
                    }
                    return Ok(());
                }
                if pending.response_transfer.is_some() {
                    self.reset_pdelay(ingress, pending.generation, at)?;
                    return Ok(());
                }
                let Some(t1) = self
                    .packet(pending.request_transfer)?
                    .tx
                    .as_ref()
                    .map(|capture| capture.raw)
                else {
                    return Ok(());
                };
                let t2 = stamp_with_correction(request_receipt, m.correction)?;
                let changed = self
                    .port(ingress)?
                    .last_sample_peer
                    .as_ref()
                    .is_some_and(|old| old != &m.source);
                if changed {
                    let port = self.port_mut(ingress)?;
                    port.last_sample = None;
                    port.last_sample_peer = None;
                    port.last_sample_index = None;
                    port.rate = None;
                    port.delay = None;
                    port.as_capable = false;
                    port.candidate = None;
                }
                let pending = self.port_mut(ingress)?.pending.as_mut().unwrap();
                pending.t1 = Some(t1);
                pending.t2 = Some(t2);
                pending.t4 = Some(rx_raw);
                pending.peer = Some(m.source);
                pending.response_transfer = Some(transfer);
                if changed {
                    self.select(at)?;
                }
            }
            Body::PdelayRespFollowUp {
                response_origin,
                requester,
            } => {
                if requester != own {
                    if let Some(generation) =
                        self.port(ingress)?.pending.as_ref().map(|p| p.generation)
                    {
                        self.reset_pdelay(ingress, generation, at)?;
                    }
                    return Ok(());
                }
                let port = self.port(ingress)?.clone();
                let Some(exchange) = port.pending else {
                    return Ok(());
                };
                if exchange.sequence != m.sequence || exchange.deadline_ps <= at {
                    if exchange.sequence != m.sequence {
                        self.reset_pdelay(ingress, exchange.generation, at)?;
                    }
                    return Ok(());
                }
                if exchange.response_transfer.is_none() || exchange.followup_transfer.is_some() {
                    return Ok(());
                }
                if exchange.peer.as_ref() != Some(&m.source) {
                    self.reset_pdelay(ingress, exchange.generation, at)?;
                    return Ok(());
                }
                let t1 = exchange.t1.ok_or_else(|| gerr("replay Pdelay t1 absent"))?;
                let t2 = exchange.t2.ok_or_else(|| gerr("replay Pdelay t2 absent"))?;
                let t4 = exchange.t4.ok_or_else(|| gerr("replay Pdelay t4 absent"))?;
                let t3 = stamp_with_correction(response_origin, m.correction)?;
                let same_peer = port.last_sample_peer.as_ref() == Some(&m.source);
                let old = if same_peer { port.last_sample } else { None };
                let previous = if same_peer {
                    port.last_sample_index
                } else {
                    None
                };
                let calculated = math::pdelay(old, t1, t2, t3, t4);
                let Ok(next) = calculated else {
                    let port = self.port_mut(ingress)?;
                    port.pending = None;
                    port.last_sample = None;
                    port.last_sample_peer = None;
                    port.last_sample_index = None;
                    port.rate = None;
                    port.delay = None;
                    port.as_capable = false;
                    port.candidate = None;
                    self.select(at)?;
                    return Ok(());
                };
                let accepted = next.filter(|(_, delay)| {
                    delay.within_integer_threshold(self.cfg.mean_link_delay_threshold_ps)
                });
                if self.state.samples.len() >= self.cfg.max_journal_entries {
                    return Err(gerr("replayed Pdelay sample limit exceeded"));
                }
                let index = self.state.samples.len();
                self.state.samples.push(state::PdelaySample {
                    direction: ingress,
                    link_epoch: port.link_epoch,
                    previous,
                    request_transfer: exchange.request_transfer,
                    response_transfer: exchange.response_transfer.unwrap(),
                    followup_transfer: transfer,
                    t1,
                    t2,
                    t3,
                    t4,
                    rate: accepted.map(|v| v.0),
                    delay: accepted.map(|v| v.1),
                    quantization: None,
                });
                let threshold = self.cfg.mean_link_delay_threshold_ps;
                let port = self.port_mut(ingress)?;
                port.pending = None;
                port.last_sample = Some((t3, t4));
                port.last_sample_peer = Some(m.source.clone());
                port.last_sample_index = Some(index);
                port.lost_responses = 0;
                port.peer = Some(m.source);
                if let Some((rate, delay)) = next {
                    if delay.within_integer_threshold(threshold) {
                        port.rate = Some(rate);
                        port.delay = Some(delay);
                        port.as_capable = true;
                    } else {
                        port.as_capable = false;
                        port.rate = None;
                        port.delay = None;
                        port.candidate = None;
                    }
                }
                self.select(at)?;
            }
            Body::Announce(a) => {
                let p = self.port(ingress)?.clone();
                if !p.as_capable
                    || p.peer.as_ref() != Some(&m.source)
                    || a.path_trace.first() != Some(&a.grandmaster_identity)
                    || a.path_trace.last() != Some(&m.source.clock)
                    || a.path_trace.contains(&own.clock)
                    || a.steps_removed >= 255
                {
                    return Ok(());
                }
                let mut identity = [0u8; 14];
                identity[0] = a.priority1;
                identity[1] = a.clock_class;
                identity[2] = a.clock_accuracy;
                identity[3..5].copy_from_slice(&a.offset_scaled_log_variance.to_be_bytes());
                identity[5] = a.priority2;
                identity[6..14].copy_from_slice(&a.grandmaster_identity);
                let generation = p
                    .candidate
                    .as_ref()
                    .map_or(1, |v| v.generation.saturating_add(1));
                let interval = self
                    .cfg
                    .announce_interval_ps
                    .checked_mul(u64::from(self.cfg.announce_receipt_timeout))
                    .ok_or_else(|| gerr("replay Announce interval overflow"))?;
                let (deadline_raw, deadline_ps) =
                    self.owner.gptp_local_deadline(node, at, interval)?;
                self.port_mut(ingress)?.candidate = Some(state::Candidate {
                    source: m.source,
                    grandmaster: a.grandmaster_identity,
                    identity,
                    steps_removed: a.steps_removed,
                    path: a.path_trace,
                    sequence: m.sequence,
                    transfer,
                    receipt_ps: at,
                    deadline_raw,
                    deadline_ps,
                    generation,
                    time_source: a.time_source,
                });
                self.expect_timer(
                    deadline_ps,
                    Timer::AnnounceTimeout {
                        direction: ingress,
                        generation,
                    },
                );
                self.select(at)?;
            }
            Body::Sync => {
                let n = self.state.nodes[node].clone();
                let p = self.port(ingress)?;
                if n.parent != Some(ingress)
                    || !p.as_capable
                    || p.peer.as_ref() != Some(&m.source)
                    || p.candidate.as_ref().is_none_or(|v| v.source != m.source)
                    || (n.sync_pair.is_none()
                        && self.state.pending_exchange_count() >= self.cfg.max_pending_exchanges)
                {
                    return Ok(());
                }
                let interval = self
                    .cfg
                    .sync_interval_ps
                    .checked_mul(u64::from(self.cfg.sync_receipt_timeout))
                    .ok_or_else(|| gerr("replay Sync pair interval overflow"))?;
                let (deadline_raw, deadline_ps) =
                    self.owner.gptp_local_deadline(node, at, interval)?;
                self.state.nodes[node].sync_pair = Some(state::SyncPair {
                    parent: ingress,
                    source: m.source,
                    sequence: m.sequence,
                    transfer,
                    rx: rx_raw,
                    epoch: n.epoch,
                    deadline_raw,
                    deadline_ps,
                });
            }
            Body::FollowUp {
                precise_origin,
                info,
            } => {
                let n = self.state.nodes[node].clone();
                if n.parent != Some(ingress) {
                    return Ok(());
                }
                let Some(pair) = n.sync_pair else {
                    return Ok(());
                };
                if pair.epoch != n.epoch
                    || pair.parent != ingress
                    || pair.sequence != m.sequence
                    || pair.source != m.source
                    || pair.deadline_ps <= at
                {
                    return Ok(());
                }
                let p = self.port(ingress)?;
                let (Some(rate_neighbor), Some(delay), Some(sample_index)) =
                    (p.rate, p.delay, p.last_sample_index)
                else {
                    return Ok(());
                };
                let ratio = math::gm_ratio(info.cumulative_scaled_rate_offset, rate_neighbor)
                    .map_err(gerr)?;
                let sync = self.packet(pair.transfer)?;
                let sync_message = sync
                    .received_message
                    .as_ref()
                    .ok_or_else(|| gerr("replay paired Sync lacks received body"))?;
                let correction = from_wire_correction(sync_message.correction)?
                    .add(from_wire_correction(m.correction)?)
                    .map_err(gerr)?;
                let origin = Rat::new(precise_origin.floor_ps().map_err(gerr)?, 1).map_err(gerr)?;
                let now_raw = self.owner.gptp_raw(node, at)?;
                let (upstream, target) = math::sync_target(
                    origin,
                    correction,
                    delay,
                    rate_neighbor,
                    ratio,
                    pair.rx,
                    now_raw,
                )
                .map_err(gerr)?;
                let initial = &self.owner.prepared.clocks.as_ref().unwrap().initial
                    [self.cfg.nodes[node].device];
                let own_rate = Rat::new(i128::from(initial.rate_num), i128::from(initial.rate_den))
                    .map_err(gerr)?;
                let rate = own_rate.mul(ratio).map_err(gerr)?;
                let (rate_num, rate_den) = application_rate(rate)?;
                let clock = &mut self.clocks[self.cfg.nodes[node].device];
                clock
                    .reanchor_and_rate(at, application_target(target)?, rate_num, rate_den)
                    .map_err(|e| gerr(e.to_string()))?;
                let clock_generation = clock.generation();
                let interval = self
                    .cfg
                    .sync_interval_ps
                    .checked_mul(u64::from(self.cfg.sync_receipt_timeout))
                    .ok_or_else(|| gerr("replay Sync timeout overflow"))?;
                let (deadline_raw, deadline_ps) =
                    self.owner.gptp_local_deadline(node, at, interval)?;
                let node_state = &mut self.state.nodes[node];
                node_state.synchronized = true;
                node_state.sync_pair = None;
                node_state.last_sync_ps = Some(at);
                node_state.relay = Some(state::Relay {
                    origin: precise_origin,
                    correction,
                    upstream_raw: upstream,
                    gm_local_ratio: ratio,
                    epoch: node_state.epoch,
                    gm: node_state.grandmaster,
                    sync_transfer: pair.transfer,
                    followup_transfer: transfer,
                });
                node_state.sync_timeout_generation = node_state
                    .sync_timeout_generation
                    .checked_add(1)
                    .ok_or_else(|| gerr("replay Sync timeout generation overflow"))?;
                node_state.sync_deadline_ps = Some(deadline_ps);
                node_state.sync_deadline_raw = Some(deadline_raw);
                if self.state.journal.len() >= self.cfg.max_journal_entries {
                    return Err(gerr("replayed correction journal limit exceeded"));
                }
                self.state.journal.push(state::Correction {
                    node,
                    at_ps: at,
                    target,
                    rate,
                    sync_transfer: pair.transfer,
                    followup_transfer: transfer,
                    parent: ingress,
                    epoch: node_state.epoch,
                    captured_rx: pair.rx,
                    delay_neighbor: delay,
                    neighbor_rate: rate_neighbor,
                    pdelay_sample: sample_index,
                    origin: precise_origin,
                    correction,
                    cumulative_scaled_rate_offset: info.cumulative_scaled_rate_offset,
                    clock_generation,
                });
                self.expect_timer(
                    deadline_ps,
                    Timer::SyncTimeout {
                        node,
                        generation: self.state.nodes[node].sync_timeout_generation,
                    },
                );
            }
        }
        Ok(())
    }
}
impl Replay<'_> {
    fn finish(mut self, horizon: u64) -> Result<()> {
        self.verify_link_cause_completeness(horizon)?;
        for cause in &self.saved.causes.clone() {
            self.step(cause, horizon)?;
        }
        self.verify_timer_cause_completeness(horizon)?;
        for (&transfer, packet) in &self.saved.packets {
            let row = &self.owner.transfers[transfer].row;
            for (kind, expected) in [
                (0, true),
                (1, row.sof_ps.is_some()),
                (2, packet.tx.is_some()),
                (3, packet.rx.is_some()),
                (4, row.eof_ps.is_some()),
            ] {
                if self.seen.contains(&(kind, transfer)) != expected {
                    return Err(gerr(
                        "causal history omits or invents physical packet event",
                    ));
                }
            }
            let physical_receipt = self.owner.receptions.iter().any(|r| {
                r.transfer == transfer && r.row.reception_id == format!("{}@rx", row.transfer_id)
            });
            if self.receptions.contains(&transfer) != physical_receipt {
                return Err(gerr("causal history omits or invents packet ingress"));
            }
        }
        for (direction, prepared) in self.cfg.ports.iter().enumerate() {
            let Some(prepared) = prepared else { continue };
            let port = self.port(direction)?;
            for raw in [
                port.pdelay_due_raw,
                port.announce_due_raw,
                port.sync_due_raw,
            ] {
                let due = raw.ok_or_else(|| gerr("replay protocol period absent"))?;
                if self.owner.gptp_global_deadline(prepared.node, due)? < horizon {
                    return Err(gerr("causal history omits a due protocol timer"));
                }
            }
        }
        if self.state.next_packet != self.saved.next_packet {
            return Err(gerr("replay packet id differs"));
        }
        for (name, actual, expected) in [
            (
                "ports",
                serde_json::to_value(&self.state.ports),
                serde_json::to_value(&self.saved.ports),
            ),
            (
                "nodes",
                serde_json::to_value(&self.state.nodes),
                serde_json::to_value(&self.saved.nodes),
            ),
            (
                "packets",
                serde_json::to_value(&self.state.packets),
                serde_json::to_value(&self.saved.packets),
            ),
            (
                "Pdelay samples",
                serde_json::to_value(&self.state.samples),
                serde_json::to_value(&self.saved.samples),
            ),
            (
                "selection journal",
                serde_json::to_value(&self.state.selections),
                serde_json::to_value(&self.saved.selections),
            ),
            (
                "correction journal",
                serde_json::to_value(&self.state.journal),
                serde_json::to_value(&self.saved.journal),
            ),
        ] {
            if actual.map_err(|e| gerr(e.to_string()))?
                != expected.map_err(|e| gerr(e.to_string()))?
            {
                return Err(gerr(format!(
                    "causal replay {name} differs from saved state"
                )));
            }
        }
        if self.owner.clocks.as_deref() != Some(self.clocks.as_slice()) {
            return Err(gerr("causal replay application clocks differ"));
        }
        Ok(())
    }

    fn verify_link_cause_completeness(&self, horizon: u64) -> Result<()> {
        let actual = self
            .saved
            .causes
            .iter()
            .filter_map(|cause| match cause.kind {
                CauseKind::Link { direction, up } => Some((cause.at_ps, direction, up)),
                _ => None,
            })
            .filter(|(at, _, _)| *at < horizon)
            .collect::<Vec<_>>();
        let Some(dynamic) = self.owner.prepared.dynamic.as_ref() else {
            if !actual.is_empty() {
                return Err(gerr("link causes without prepared controls"));
            }
            return Ok(());
        };
        let mut links = dynamic
            .links
            .iter()
            .map(|link| (link.id.as_str(), link.up))
            .collect::<BTreeMap<_, _>>();
        let mut expected = Vec::new();
        for link in &dynamic.links {
            if !link.up && horizon > 0 {
                for (direction, configured) in self.cfg.ports.iter().enumerate() {
                    if configured.is_some()
                        && link
                            .ports
                            .contains(&self.owner.prepared.ethernet.directions[direction].from_port)
                    {
                        expected.push((0, direction, false));
                    }
                }
            }
        }
        expected.sort_by_key(|(_, direction, _)| *direction);
        let mut controls = dynamic.controls.iter().collect::<Vec<_>>();
        controls.sort_by_key(|control| (control.at_ps, control.input_index));
        for control in controls {
            if control.at_ps >= horizon {
                continue;
            }
            let crate::types::ethernet::dynamic::ControlOp::LinkSet { link, up } = &control.op
            else {
                continue;
            };
            let old = *links
                .get(link.as_str())
                .ok_or_else(|| gerr("prepared link control has no link"))?;
            if old == *up {
                continue;
            }
            links.insert(link, *up);
            let physical = dynamic
                .links
                .iter()
                .find(|candidate| candidate.id == *link)
                .unwrap();
            for (direction, configured) in self.cfg.ports.iter().enumerate() {
                if configured.is_some()
                    && physical
                        .ports
                        .contains(&self.owner.prepared.ethernet.directions[direction].from_port)
                {
                    expected.push((control.at_ps, direction, *up));
                }
            }
        }
        if expected != actual {
            return Err(gerr("causal link history differs from prepared controls"));
        }
        Ok(())
    }
    fn verify_timer_cause_completeness(&self, horizon: u64) -> Result<()> {
        let mut expected = self.expected_timers.clone();
        let limit = self
            .cfg
            .max_journal_entries
            .checked_mul(32)
            .ok_or_else(|| gerr("causal timer history limit overflow"))?;
        for (direction, prepared) in self.cfg.ports.iter().enumerate() {
            let Some(prepared) = prepared else { continue };
            type PeriodicTimer = (u64, fn(usize, u64) -> Timer);
            let periodic: [PeriodicTimer; 3] = [
                (self.cfg.pdelay_interval_ps, |direction, generation| {
                    Timer::Pdelay {
                        direction,
                        generation,
                    }
                }),
                (self.cfg.announce_interval_ps, |direction, generation| {
                    Timer::Announce {
                        direction,
                        generation,
                    }
                }),
                (self.cfg.sync_interval_ps, |direction, generation| {
                    Timer::Sync {
                        direction,
                        generation,
                    }
                }),
            ];
            for (interval, make) in periodic {
                let mut raw = self.owner.gptp_raw(prepared.node, 0)?;
                for generation in 0..=limit {
                    let at = self.owner.gptp_global_deadline(prepared.node, raw)?;
                    if at >= horizon {
                        break;
                    }
                    *expected
                        .entry((at, make(direction, generation as u64)))
                        .or_default() += 1;
                    if generation == limit {
                        return Err(gerr("complete causal timer history limit exceeded"));
                    }
                    raw = raw
                        .add(Rat::new(i128::from(interval), 1).map_err(gerr)?)
                        .map_err(gerr)?;
                }
            }
        }
        let mut actual = BTreeMap::new();
        for cause in &self.saved.causes {
            if cause.at_ps < horizon {
                if let CauseKind::Timer(timer) = cause.kind {
                    *actual.entry((cause.at_ps, timer)).or_default() += 1;
                }
            }
        }
        expected.retain(|(at, _), _| *at < horizon);
        if expected != actual {
            return Err(gerr(
                "causal timer history omits or invents a scheduled deadline",
            ));
        }
        Ok(())
    }
}
impl NetworkState {
    pub(super) fn replay_gptp_checkpoint(
        &self,
        horizon: u64,
        initial: &[clock::NodeClock],
    ) -> Result<()> {
        let saved = self
            .gptp
            .as_ref()
            .ok_or_else(|| gerr("replay state absent"))?;
        Replay::new(self, saved, initial)?.finish(horizon)
    }
}
