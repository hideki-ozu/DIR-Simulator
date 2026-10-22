//! Full bounded causal accounting history. Saved credit is never a replay anchor.
use super::super::*;
use super::integration::{Origin, Packet};
use super::tas::TasControl;
use super::*;
use crate::runtime::ethernet::tsn::{Head, PortInput, TsnState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Cause {
    Bind,
    GateEdge,
    Enqueue(usize),
    WireEnqueue(String),
    WireRemove(String),
    Drop { transfer: usize, reason: String },
    Select { transfer: usize, express: bool },
    Unselect { transfer: usize, express: bool },
    Start(Packet),
    Replan(Packet),
    Eof(u64),
    Release(u64),
    Reset { epoch: u64, up: bool },
    MmCtl(TasControl),
    Advertise,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub order: u64,
    pub at: u64,
    pub policy_epoch: u64,
    pub causes: Vec<Cause>,
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    pub waiting: [Vec<usize>; 8],
    #[serde(default)]
    pub wire_waiting: Vec<String>,
    pub pmac: Option<usize>,
    pub emac: Option<usize>,
    pub busy: Option<Packet>,
    pub released: Option<(u64, Origin, bool)>,
    pub epoch: u64,
    pub ordinal: u64,
    pub last_packet: Option<u64>,
    pub bound: bool,
    pub applied: Option<TasControl>,
    pub lldp_pending: bool,
    pub last_at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    pub entries: Vec<Entry>,
    pub tail: Cursor,
    pub events: usize,
    pub octets: usize,
}
fn bad(s: &str) -> Diagnostic {
    error(format!("MAC Merge accounting history: {s}"))
}
fn normalized(mut p: Packet) -> Packet {
    p.eof_done = false;
    p.release_done = false;
    p.arrival_done = false;
    p.smd_pending = false;
    p.aborted_ps = None;
    p.invalidated_ps = None;
    // Actual producer observations do not change the MAC service plan. The
    // authoritative packet/PHY state retains these fields for exact replay.
    if let Some(raw) = &mut p.raw {
        // These are receive observations, never a MAC transmit/replan command.
        p.received = None;
        p.complete = true;
        p.arrival = 0;
        raw.tx_delimiter_sample = None;
        raw.receive_cut_sample = None;
        raw.decoded_at_sample = None;
        raw.delimiter_slot = None;
        raw.delimiter_generation = None;
        raw.tx_mdi_delimiter_sample = None;
        raw.rx_mdi_delimiter_sample = None;
        raw.coded_end = false;
    }
    p
}
fn rank(c: &Cause) -> u8 {
    match c {
        Cause::Reset { .. } => 0,
        Cause::Bind | Cause::GateEdge => 1,
        Cause::MmCtl(_) | Cause::Advertise => 2,
        Cause::Eof(_) => 3,
        Cause::Release(_) => 4,
        Cause::Unselect { .. } | Cause::Drop { .. } => 5,
        Cause::Enqueue(_) | Cause::WireEnqueue(_) => 6,
        Cause::WireRemove(_) => 7,
        Cause::Select { .. } => 7,
        Cause::Replan(_) => 8,
        Cause::Start(_) => 9,
    }
}
fn weight(causes: &[Cause]) -> Result<usize> {
    causes.iter().try_fold(0usize, |n, c| match c {
        Cause::Start(p) | Cause::Replan(p) => n
            .checked_add(p.bytes.len())
            .and_then(|n| n.checked_add(p.received.as_ref().map_or(0, Vec::len)))
            .and_then(|n| n.checked_add(p.plan.as_ref().map_or(0, |p| p.bytes.len())))
            .ok_or_else(|| bad("proof capacity overflow")),
        _ => Ok(n),
    })
}
impl NetworkState {
    // The accepted Merge+CBS contract currently uses a static clock. Keep its
    // optional initial anchor in replay as well as live TSN state; dynamic
    // clock causes are part of the still-open composed-history integration.
    fn merge_journal_initial_clock(&self, i: usize) -> Result<Option<clock::NodeClock>> {
        self.prepared
            .clocks
            .as_ref()
            .map(|clocks| {
                let source = self.prepared.ethernet.directions[i].source;
                let initial = &clocks.initial[source];
                clock::NodeClock::new(0, initial.offset_ps, initial.rate_num, initial.rate_den)
                    .map_err(|e| bad(&e.to_string()))
            })
            .transpose()
    }
    fn merge_has_cbs(&self, i: usize) -> bool {
        self.prepared
            .tsn
            .as_ref()
            .and_then(|s| {
                s.outputs
                    .iter()
                    .find(|o| o.port == self.prepared.ethernet.directions[i].from_port)
            })
            .is_some_and(|o| o.cbs.iter().any(Option::is_some))
    }
    fn merge_journal_initial(&self, i: usize) -> Cursor {
        Cursor {
            lldp_pending: self
                .mac_merge
                .as_ref()
                .is_some_and(|s| s.identity.ports[i].is_some()),
            ..Cursor::default()
        }
    }
    pub(in crate::runtime::network) fn merge_journal_stage(
        &self,
        d: &mut NetworkDelta,
        delta: &TsnDelta,
        ports: &[String],
    ) -> Result<()> {
        if self.mac_merge.is_none() {
            return Ok(());
        }
        for name in ports {
            let Some(i) = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|p| p.from_port == *name)
            else {
                continue;
            };
            if !self.merge_direction(i) || !self.merge_has_cbs(i) {
                continue;
            }
            let now = self.tsn.as_ref().unwrap().merge_staged_time(delta, name)?;
            let port = &self.merge_state(d).unwrap().ports[i];
            let mut old = port
                .accounting_journal
                .as_ref()
                .map_or_else(|| self.merge_journal_initial(i), |j| j.tail.clone());
            let mut causes = vec![];
            if now > old.last_at && self.merge_journal_gate_edge(i, now) {
                causes.push(Cause::GateEdge);
            }
            if old.epoch != port.tx.epoch {
                causes.push(Cause::Reset {
                    epoch: port.tx.epoch,
                    up: port.link_up,
                });
                old.epoch = port.tx.epoch;
                old.pmac = None;
                old.emac = None;
                old.busy = None;
                old.released = None;
                old.applied = None;
                old.lldp_pending =
                    port.link_up && self.merge_state(d).unwrap().identity.ports[i].is_some();
            }
            if !old.bound && port.accounting.is_some() {
                causes.push(Cause::Bind);
            }
            if old.applied != port.applied_tas {
                if let Some(control) = port.applied_tas {
                    causes.push(Cause::MmCtl(control));
                }
            }
            if !old.lldp_pending && port.lldp_pending {
                causes.push(Cause::Advertise);
            }
            let next_packet = port
                .busy
                .and_then(|id| self.merge_state(d).unwrap().packets.get(&id));
            if let Some(previous) = &old.busy {
                if let Some(next) = next_packet.filter(|p| p.id == previous.id) {
                    if normalized(next.clone()) != normalized(previous.clone()) {
                        causes.push(Cause::Replan(normalized(next.clone())));
                    }
                    if !previous.eof_done && next.eof_done {
                        causes.push(Cause::Eof(next.id));
                    }
                } else {
                    if !previous.eof_done {
                        return Err(bad("serializer removed without EOF/reset"));
                    }
                    causes.push(Cause::Release(previous.id));
                }
            }
            for (express, previous, next) in [
                (false, old.pmac, port.selected_pmac),
                (true, old.emac, port.selected_emac),
            ] {
                if previous != next {
                    if let Some(t) = previous {
                        causes.push(Cause::Unselect {
                            transfer: t,
                            express,
                        });
                    }
                }
            }
            let queue = self.queue(d, i);
            for c in 0..8 {
                for &t in &old.waiting[c] {
                    if !queue.waiting[c].contains(&t) {
                        if port.selected_pmac == Some(t) {
                            causes.push(Cause::Select {
                                transfer: t,
                                express: false,
                            });
                        } else if port.selected_emac == Some(t) {
                            causes.push(Cause::Select {
                                transfer: t,
                                express: true,
                            });
                        } else {
                            causes.push(Cause::Drop {
                                transfer: t,
                                reason: self
                                    .transfer(d, t)
                                    .row
                                    .drop_reason
                                    .clone()
                                    .ok_or_else(|| bad("queue removal lacks source-bound cause"))?,
                            });
                        }
                    }
                }
                for &t in &queue.waiting[c] {
                    if !old.waiting[c].contains(&t) {
                        causes.push(Cause::Enqueue(t));
                    }
                }
            }
            // An offer and selection may share one callback. Preserve the actual
            // source-bound enqueue before selection even if no waiting snapshot exists.
            for (express, previous, next) in [
                (false, old.pmac, port.selected_pmac),
                (true, old.emac, port.selected_emac),
            ] {
                if let Some(t) = next.filter(|t| Some(*t) != previous) {
                    if !old.waiting.iter().any(|q| q.contains(&t)) {
                        causes.push(Cause::Enqueue(t));
                        causes.push(Cause::Select {
                            transfer: t,
                            express,
                        });
                    }
                }
            }
            let wire_queue = self
                .wire_host(d)
                .map(|h| h.queues[i].iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            for id in &wire_queue {
                if !old.wire_waiting.contains(id) {
                    causes.push(Cause::WireEnqueue(id.clone()));
                }
            }
            for id in &old.wire_waiting {
                if !wire_queue.contains(id) {
                    causes.push(Cause::WireRemove(id.clone()));
                }
            }
            if let Some(id) = next_packet
                .and_then(|p| p.wire_control_id.as_ref())
                .filter(|id| {
                    !old.wire_waiting.contains(id)
                        && !wire_queue.contains(id)
                        && old.busy.as_ref().and_then(|p| p.wire_control_id.as_ref()) != Some(id)
                })
            {
                causes.push(Cause::WireEnqueue(id.clone()));
                causes.push(Cause::WireRemove(id.clone()));
            }
            if let Some(next) = next_packet {
                if old.busy.as_ref().is_none_or(|p| p.id != next.id) {
                    causes.push(Cause::Start(normalized(next.clone())));
                }
            }
            causes.sort_by_key(rank);
            if causes
                .iter()
                .any(|c| matches!(c, Cause::Start(_) | Cause::Replan(_)))
            {
                crate::allocation::reservation_checkpoint("mac_merge_accounting_packet_proof")?;
            }
            if causes.iter().any(|c| matches!(c, Cause::Enqueue(_))) {
                crate::allocation::reservation_checkpoint("mac_merge_accounting_queue_cause")?;
            }
            let cfg = &self.merge_state(d).unwrap().identity;
            let existing = port.accounting_journal.as_ref();
            let events = existing
                .map_or(0, |j| j.events)
                .checked_add(1 + causes.len())
                .ok_or_else(|| bad("event capacity overflow"))?;
            let octets = existing
                .map_or(0, |j| j.octets)
                .checked_add(weight(&causes)?)
                .ok_or_else(|| bad("octet capacity overflow"))?;
            if events > cfg.max_accounting_events || octets > cfg.max_accounting_octets {
                return Err(bad(
                    "configured history limit exceeded; causal history cannot be pruned",
                ));
            }
            crate::allocation::reservation_checkpoint("mac_merge_accounting_journal")?;
            let order = existing.map_or(0, |j| j.entries.len()) as u64;
            let entry = Entry {
                order,
                at: now,
                policy_epoch: self.epoch_after(d),
                causes,
            };
            let mut tail = port
                .accounting_journal
                .as_ref()
                .map_or_else(|| self.merge_journal_initial(i), |j| j.tail.clone());
            self.merge_journal_apply(i, &entry, &mut tail, false, Some(d), None)?;
            let port = &mut self.merge_mut(d)?.ports[i];
            let journal = port.accounting_journal.get_or_insert_with(|| Journal {
                entries: vec![],
                tail: Cursor::default(),
                events: 0,
                octets: 0,
            });
            journal.entries.push(entry);
            journal.tail = tail;
            journal.events = events;
            journal.octets = octets;
        }
        Ok(())
    }
    fn merge_journal_transfer<'a>(
        &'a self,
        i: usize,
        t: usize,
        d: Option<&'a NetworkDelta>,
    ) -> Result<&'a Transfer> {
        d.and_then(|d| d.transfers.iter().find(|(n, _)| *n == t).map(|(_, t)| t))
            .or_else(|| self.transfers.get(t))
            .filter(|t| t.direction == i)
            .ok_or_else(|| bad("queue/physical transfer source differs"))
    }
    fn merge_journal_apply(
        &self,
        i: usize,
        e: &Entry,
        tail: &mut Cursor,
        validate: bool,
        d: Option<&NetworkDelta>,
        replay: Option<&TsnState>,
    ) -> Result<()> {
        let cfg = &self.mac_merge.as_ref().unwrap().identity;
        if e.causes.windows(2).any(|p| rank(&p[0]) > rank(&p[1])) {
            return Err(bad("same-time causal order differs"));
        }
        for cause in &e.causes {
            match cause {
                Cause::GateEdge => {
                    if e.at <= tail.last_at || !self.merge_journal_gate_edge(i, e.at) {
                        return Err(bad("gate-freeze edge not derived from static GCL"));
                    }
                }
                Cause::Bind => {
                    if tail.bound || e.at != 0 {
                        return Err(bad("duplicate or late shaper binding"));
                    }
                    tail.bound = true;
                }
                Cause::WireEnqueue(id) => {
                    let empty = self.delta();
                    let tx = self
                        .wire_host(d.unwrap_or(&empty))
                        .and_then(|h| h.tx.get(id))
                        .ok_or_else(|| bad("Wire enqueue source"))?;
                    if tx.queued_ps != Some(e.at)
                        || tx.intent.port != self.prepared.ethernet.directions[i].from_port
                        || tail.wire_waiting.contains(id)
                    {
                        return Err(bad("Wire enqueue source chronology"));
                    }
                    tail.wire_waiting.push(id.clone());
                }
                Cause::WireRemove(id) => {
                    let empty = self.delta();
                    let tx = self
                        .wire_host(d.unwrap_or(&empty))
                        .and_then(|h| h.tx.get(id))
                        .ok_or_else(|| bad("Wire removal source"))?;
                    if tx.sof != Some(e.at) && !tx.dropped {
                        return Err(bad("Wire removal lacks claim/failure"));
                    }
                    let at = tail
                        .wire_waiting
                        .iter()
                        .position(|s| s == id)
                        .ok_or_else(|| bad("Wire removal without FIFO source"))?;
                    tail.wire_waiting.remove(at);
                }
                Cause::Enqueue(t) => {
                    let row = self.merge_journal_transfer(i, *t, d)?;
                    if row.row.queued_ps != e.at
                        || tail.waiting.iter().any(|q| q.contains(t))
                        || tail.pmac == Some(*t)
                        || tail.emac == Some(*t)
                    {
                        return Err(bad("enqueue not derived from original queue offer"));
                    }
                    tail.waiting[row.row.priority as usize].push(*t);
                }
                Cause::Drop { transfer, reason } => {
                    let row = self.merge_journal_transfer(i, *transfer, d)?;
                    if row.row.sof_ps.is_some()
                        || row.row.time_ps != e.at
                        || row.row.drop_reason.as_ref() != Some(reason)
                    {
                        return Err(bad("drop cause not bound to source lifecycle"));
                    }
                    let q = &mut tail.waiting[row.row.priority as usize];
                    let at = q
                        .iter()
                        .position(|t| t == transfer)
                        .ok_or_else(|| bad("drop without queued transfer"))?;
                    q.remove(at);
                }
                Cause::Select { transfer, express } => {
                    let row = self.merge_journal_transfer(i, *transfer, d)?;
                    let class = row.row.priority;
                    if validate {
                        if !*express
                            && (tail.applied.is_some_and(|c| c.hold)
                                || row.row.sof_ps != Some(e.at))
                        {
                            return Err(bad(
                                "fresh pMAC selection lacks actual SOF/release eligibility",
                            ));
                        }
                        let input = self.merge_journal_input(i, tail, e.at, e.policy_epoch)?;
                        let name = &self.prepared.ethernet.directions[i].from_port;
                        let clock = self.merge_journal_initial_clock(i)?;
                        let plan = replay.unwrap().plan_port_clock_in(
                            TsnDelta::default(),
                            name,
                            e.at,
                            &input,
                            clock.as_ref(),
                        )?;
                        if plan.ready.get(name).copied().unwrap_or(0) & (1 << class) == 0 {
                            return Err(bad("new frame was selected without CBS/TAS eligibility"));
                        }
                    }
                    let mask = cfg.ports[i]
                        .as_ref()
                        .map_or(if *express { 255 } else { 0 }, |p| {
                            if *express {
                                p.express_mask
                            } else {
                                p.preemptable_mask
                            }
                        });
                    let q = &mut tail.waiting[class as usize];
                    if mask & (1 << class) == 0 || q.first() != Some(transfer) {
                        return Err(bad("selection not bound to eligible FIFO source"));
                    }
                    q.remove(0);
                    let selected = if *express {
                        &mut tail.emac
                    } else {
                        &mut tail.pmac
                    };
                    if selected.replace(*transfer).is_some() {
                        return Err(bad("selected frame overwritten"));
                    }
                }
                Cause::Unselect { transfer, express } => {
                    let selected = if *express {
                        &mut tail.emac
                    } else {
                        &mut tail.pmac
                    };
                    let valid = tail.released.is_some_and(|(_, origin, final_packet)| {
                        origin
                            == if *express {
                                Origin::Express(*transfer)
                            } else {
                                Origin::Pmac(*transfer)
                            }
                            && final_packet
                    });
                    if *selected != Some(*transfer) || !valid {
                        return Err(bad("selected backlog removed before actual final release"));
                    }
                    *selected = None;
                }
                Cause::Start(p) => {
                    if validate {
                        self.merge_journal_packet(i, e.at, p)?;
                        if p.origin == Origin::Lldp {
                            let input = self.merge_journal_input(i, tail, e.at, e.policy_epoch)?;
                            let name = &self.prepared.ethernet.directions[i].from_port;
                            let clock = self.merge_journal_initial_clock(i)?;
                            let tsn = replay.unwrap();
                            let plan = tsn.plan_port_clock_in(
                                TsnDelta::default(),
                                name,
                                e.at,
                                &input,
                                clock.as_ref(),
                            )?;
                            if tsn
                                .retained_start_permission_clock_in(
                                    &plan,
                                    name,
                                    self.merge_lldp_priority(i),
                                    e.at,
                                    self.merge_lldp_head(i, &self.delta())?.occupancy_ps,
                                    clock.as_ref(),
                                )?
                                .is_none()
                            {
                                return Err(bad("LLDP start without own full-fit/current credit"));
                            }
                        }
                    }
                    if tail.busy.is_some()
                        || p.sof != e.at
                        || p.epoch != tail.epoch
                        || p.ordinal != tail.ordinal
                        || tail.last_packet.is_some_and(|id| id >= p.id)
                    {
                        return Err(bad("physical start chronology/owner differs"));
                    }
                    match p.origin {
                        Origin::Pmac(t) if tail.pmac != Some(t) => {
                            return Err(bad("physical pMAC lacks retained selection"));
                        }
                        Origin::Express(t) if tail.emac != Some(t) => {
                            return Err(bad("physical eMAC lacks retained selection"));
                        }
                        Origin::Lldp => {
                            if !tail.lldp_pending {
                                return Err(bad("LLDP start without pending advertisement"));
                            }
                            tail.lldp_pending = false;
                        }
                        _ => {}
                    }
                    tail.last_packet = Some(p.id);
                    tail.ordinal = tail
                        .ordinal
                        .checked_add(1)
                        .ok_or_else(|| bad("physical ordinal overflow"))?;
                    tail.busy = Some(p.clone());
                    tail.released = None;
                }
                Cause::Replan(p) => {
                    if validate {
                        self.merge_journal_packet(i, e.at, p)?;
                    }
                    let old = tail
                        .busy
                        .as_ref()
                        .ok_or_else(|| bad("replan without physical owner"))?;
                    let rate = self.prepared.ethernet.directions[i].bitrate_bps;
                    let n = usize::try_from(
                        (u128::from(e.at - old.sof) * u128::from(rate)).div_ceil(8_000_000_000_000),
                    )
                    .map_err(|_| bad("prefix overflow"))?;
                    if old.eof_done
                        || e.at >= old.eof
                        || p.id != old.id
                        || p.ordinal != old.ordinal
                        || p.epoch != old.epoch
                        || p.origin != old.origin
                        || p.sof != old.sof
                        || n > old.bytes.len()
                        || n > p.bytes.len()
                        || old.bytes[..n] != p.bytes[..n]
                    {
                        return Err(bad("replan rewrites emitted physical prefix"));
                    }
                    if p.demand_ps != Some(e.at)
                        && e.at
                            != old.sof
                                + super::bit_duration(56, rate).map_err(super::integration::err)?
                    {
                        return Err(bad("replan lacks demand/SMD boundary"));
                    }
                    if p.demand_ps == Some(e.at)
                        && tail.emac.is_none()
                        && !tail.applied.is_some_and(|c| c.hold)
                        && !tail.lldp_pending
                        && tail.wire_waiting.is_empty()
                    {
                        return Err(bad("cut lacks real express/HOLD demand"));
                    }
                    tail.busy = Some(p.clone());
                }
                Cause::Eof(id) => {
                    let p = tail
                        .busy
                        .as_mut()
                        .filter(|p| p.id == *id)
                        .ok_or_else(|| bad("EOF physical owner"))?;
                    if p.eof_done || p.eof != e.at {
                        return Err(bad("EOF time/order differs"));
                    }
                    if validate && p.plan.as_ref().is_none_or(|plan| plan.final_packet) {
                        if let Origin::Pmac(t) | Origin::Express(t) = p.origin {
                            if self.merge_journal_transfer(i, t, None)?.row.eof_ps != Some(e.at) {
                                return Err(bad(
                                    "actual EOF differs from retained source lifecycle",
                                ));
                            }
                        }
                    }
                    p.eof_done = true;
                }
                Cause::Release(id) => {
                    let p = tail
                        .busy
                        .take()
                        .filter(|p| p.id == *id)
                        .ok_or_else(|| bad("release physical owner"))?;
                    if !p.eof_done || p.release != e.at {
                        return Err(bad("release precedes EOF or differs"));
                    }
                    tail.released = Some((
                        p.id,
                        p.origin,
                        p.plan.as_ref().is_none_or(|p| p.final_packet),
                    ));
                }
                Cause::Reset { epoch, up } => {
                    if *epoch != tail.epoch + 1 {
                        return Err(bad("link epoch history differs"));
                    }
                    if validate {
                        use crate::types::ethernet::dynamic::ControlOp;
                        let name = &self.prepared.ethernet.directions[i].from_port;
                        let valid=self.prepared.dynamic.as_ref().is_some_and(|d|d.controls.iter().any(|c|c.at_ps==e.at && matches!(&c.op,ControlOp::LinkSet{link,up:state} if state==up&&d.links.iter().any(|l|l.id==*link&&l.ports.contains(name)))));
                        let initial = e.at == 0
                            && *epoch == 1
                            && self.prepared.dynamic.as_ref().is_some_and(|d| {
                                d.links
                                    .iter()
                                    .any(|l| l.ports.contains(name) && l.up == *up)
                            });
                        if !valid && !initial {
                            return Err(bad("reset not bound to prepared physical link control"));
                        }
                    }
                    tail.epoch = *epoch;
                    tail.busy = None;
                    tail.pmac = None;
                    tail.emac = None;
                    tail.released = None;
                    tail.applied = None;
                    tail.lldp_pending = *up && cfg.ports[i].is_some();
                }
                Cause::MmCtl(c) => {
                    if c.at != e.at {
                        return Err(bad("MM control timestamp differs"));
                    }
                    if validate {
                        self.merge_validate_tas_proof(i, *c, e.at)?;
                    }
                    tail.applied = Some(*c);
                }
                Cause::Advertise => {
                    if tail.lldp_pending
                        || cfg.ports[i].is_none()
                        || e.at % cfg.lldp_interval_ps != 0
                    {
                        return Err(bad("advertisement not bound to periodic source"));
                    }
                    tail.lldp_pending = true;
                }
            }
        }
        tail.last_at = e.at;
        Ok(())
    }
    fn merge_journal_gate_edge(&self, i: usize, at: u64) -> bool {
        self.prepared
            .tsn
            .as_ref()
            .and_then(|s| {
                s.outputs
                    .iter()
                    .find(|o| o.port == self.prepared.ethernet.directions[i].from_port)
            })
            .and_then(|o| o.tas.as_ref())
            .is_some_and(|s| {
                at >= s.base_ps
                    && s.prefix_ps[..s.entries.len()].contains(&((at - s.base_ps) % s.cycle_ps))
            })
    }
    fn merge_journal_packet(&self, i: usize, now: u64, p: &Packet) -> Result<()> {
        let state = self.mac_merge.as_ref().unwrap();
        let cfg = &state.identity;
        let link = &self.prepared.ethernet.directions[i];
        if normalized(p.clone()) != *p
            || p.direction != i
            || p.id >= state.next_packet
            || p.sof > now
            || p.eof <= now
            || p.release <= p.eof
            || p.arrival
                != p.eof
                    .checked_add(self.physical_receive_delay(i)?)
                    .ok_or_else(|| bad("arrival overflow"))?
        {
            return Err(bad("retained physical proof identity/timing differs"));
        }
        let eof = p
            .sof
            .checked_add(
                super::bit_duration(p.bytes.len() as u64 * 8, link.bitrate_bps)
                    .map_err(super::integration::err)?,
            )
            .ok_or_else(|| bad("EOF overflow"))?;
        if p.eof != eof
            || p.release
                != eof
                    .checked_add(
                        super::bit_duration(96, link.bitrate_bps)
                            .map_err(super::integration::err)?,
                    )
                    .ok_or_else(|| bad("IPG overflow"))?
        {
            return Err(bad("retained physical byte duration differs"));
        }
        match p.origin {
            Origin::WireControl => {
                self.validate_wire_merge_source(p, p.sof)?;
            }
            Origin::Pmac(t) => {
                let row = self.merge_journal_transfer(i, t, None)?;
                let mac = ingress::bytes(&row.row.wire.mac_hex).map_err(super::integration::err)?;
                let plan = p
                    .plan
                    .as_ref()
                    .ok_or_else(|| bad("physical pMAC plan missing"))?;
                let kind = decode_packet(&p.bytes)
                    .map_err(super::integration::err)?
                    .kind;
                let (frame, frag, active) = match kind {
                    PacketKind::Start { frame_count } => (frame_count, None, true),
                    PacketKind::Continuation {
                        frame_count,
                        frag_count,
                    } => (frame_count, Some(frag_count), true),
                    PacketKind::Express => (0, None, false),
                    _ => return Err(bad("pMAC proof control delimiter")),
                };
                if plan.start_offset == 0 && row.row.sof_ps != Some(p.sof) {
                    return Err(bad("first physical pMAC SOF differs from source lifecycle"));
                }
                if plan.epoch != p.epoch
                    || plan.bytes != p.bytes
                    || plan.bitrate != link.bitrate_bps
                    || plan.start_ps != p.sof
                    || plan.eof_ps != p.eof
                    || plan.release_ps != p.release
                    || plan.final_packet != (plan.end_offset == mac.len() - 4)
                    || encode_fragment(
                        &mac,
                        plan.start_offset,
                        plan.end_offset,
                        frame,
                        frag,
                        active,
                        0,
                    )
                    .map_err(super::integration::err)?
                        != p.bytes
                {
                    return Err(bad("pMAC proof not derived from actual source bytes/cut"));
                }
            }
            Origin::Express(t) => {
                if self.merge_journal_transfer(i, t, None)?.row.sof_ps != Some(p.sof) {
                    return Err(bad("physical eMAC SOF differs from source lifecycle"));
                }
                if p.plan.is_some()
                    || encode_express(
                        &ingress::bytes(&self.merge_journal_transfer(i, t, None)?.row.wire.mac_hex)
                            .map_err(super::integration::err)?,
                    )
                    .map_err(super::integration::err)?
                        != p.bytes
                {
                    return Err(bad("eMAC proof source bytes differ"));
                }
            }
            Origin::Control(kind) => {
                if p.plan.is_some()
                    || encode_control(kind).map_err(super::integration::err)? != p.bytes
                {
                    return Err(bad("control proof source encoding differs"));
                }
            }
            Origin::Lldp => {
                let configured = cfg.ports[i]
                    .as_ref()
                    .ok_or_else(|| bad("legacy port advertised LLDP"))?;
                let packet = ValidatedMacIngress {
                    epoch: p.epoch,
                    received_at: p.arrival,
                    bytes: p
                        .bytes
                        .get(8..)
                        .ok_or_else(|| bad("LLDP proof truncated"))?
                        .to_vec(),
                    provenance: RxProvenance::ExternalInjection {
                        evidence_id: "accounting".into(),
                    },
                };
                let o = lldp::receive(i, &packet, "accounting".into())
                    .map_err(super::integration::err)?;
                let peer = o
                    .capability
                    .ok_or_else(|| bad("generated LLDP missing capability"))?;
                if p.plan.is_some()
                    || !peer.support
                    || o.ttl != cfg.lldp_ttl_seconds
                    || encode_express(&lldp::advertise(
                        &link.from_port,
                        configured,
                        peer.enabled,
                        peer.active,
                        o.ttl,
                    ))
                    .map_err(super::integration::err)?
                        != p.bytes
                {
                    return Err(bad("LLDP proof not generated by configured source"));
                }
            }
        }
        let mut replay = p.clone();
        self.merge_fault(cfg, &mut replay)?;
        if replay.received != p.received || replay.complete != p.complete {
            return Err(bad("retained physical fault proof differs"));
        }
        Ok(())
    }
    fn merge_journal_input(
        &self,
        i: usize,
        cursor: &Cursor,
        now: u64,
        policy_epoch: u64,
    ) -> Result<PortInput> {
        let rate = self.prepared.ethernet.directions[i].bitrate_bps;
        let mut input = PortInput {
            policy_epoch,
            ..PortInput::default()
        };
        for class in 0..8 {
            input.backlog[class] = !cursor.waiting[class].is_empty();
            if let Some(&t) = cursor.waiting[class].first() {
                let row = self.merge_journal_transfer(i, t, None)?;
                input.heads[class] = Some(Head {
                    occupancy_ps: crate::runtime::ethernet::tsn::wire_times(
                        row.row.wire.mac_bytes,
                        rate,
                    )?
                    .1,
                });
            }
        }
        for id in &cursor.wire_waiting {
            let tx = self
                .wire
                .as_ref()
                .and_then(|h| h.tx.get(id))
                .ok_or_else(|| bad("Wire history FIFO source"))?;
            if tx.is_mac_control() {
                continue;
            }
            let class = self.wire_priority(i, id, &self.delta()) as usize;
            input.backlog[class] = true;
            let head = Head {
                occupancy_ps: crate::runtime::ethernet::tsn::wire_times(
                    tx.intent.bytes.len() as u64,
                    rate,
                )?
                .1,
            };
            if input.heads[class]
                .as_ref()
                .is_none_or(|h| head.occupancy_ps < h.occupancy_ps)
            {
                input.heads[class] = Some(head)
            }
        }
        if cursor.lldp_pending && self.mac_merge.as_ref().unwrap().identity.ports[i].is_some() {
            let class = self.merge_lldp_priority(i) as usize;
            let head = self.merge_lldp_head(i, &self.delta())?;
            input.backlog[class] = true;
            if input.heads[class]
                .as_ref()
                .is_none_or(|h| head.occupancy_ps < h.occupancy_ps)
            {
                input.heads[class] = Some(head)
            }
        }
        for t in [cursor.pmac, cursor.emac].into_iter().flatten() {
            input.backlog[self.merge_journal_transfer(i, t, None)?.row.priority as usize] = true;
        }
        let class = cursor
            .busy
            .as_ref()
            .and_then(|p| self.wire_packet_class(p, &self.delta()));
        if cursor.bound {
            input.merge = Some(super::shaper::packet_view(
                cursor.busy.as_ref(),
                class,
                now,
                rate,
            )?);
        }
        Ok(input)
    }
    pub(super) fn validate_merge_journal(&self, i: usize, horizon: u64) -> Result<()> {
        let port = &self.mac_merge.as_ref().unwrap().ports[i];
        let Some(tsn) = &self.tsn else { return Ok(()) };
        if !self.merge_has_cbs(i) {
            if port.accounting_journal.is_some() {
                return Err(bad("unexpected accounting journal"));
            }
            return Ok(());
        }
        let mut replay = TsnState::new(self.prepared.tsn.as_ref().unwrap());
        let name = &self.prepared.ethernet.directions[i].from_port;
        let clock = self.merge_journal_initial_clock(i)?;
        let initial = replay.plan_settle_clock_in(TsnDelta::default(), name, 0, clock.as_ref())?;
        replay.apply(initial);
        let Some(journal) = &port.accounting_journal else {
            return tsn.validate_merge_replayed_port(&replay, name);
        };
        let cfg = &self.mac_merge.as_ref().unwrap().identity;
        let mut cursor = self.merge_journal_initial(i);
        let mut events = 0usize;
        let mut octets = 0usize;
        let mut previous = 0;
        for (order, e) in journal.entries.iter().enumerate() {
            if e.order != order as u64 || e.at < previous || e.at > horizon {
                return Err(bad("chronological entry order/time differs"));
            }
            let input = self.merge_journal_input(i, &cursor, previous, 0)?;
            if input
                .merge
                .and_then(|v| v.next_boundary)
                .is_some_and(|at| at < e.at)
            {
                return Err(bad(
                    "accounting history omitted or shifted a physical boundary",
                ));
            }
            self.merge_journal_apply(i, e, &mut cursor, true, None, Some(&replay))?;
            let input = self.merge_journal_input(i, &cursor, e.at, e.policy_epoch)?;
            let delta = replay.plan_port_clock_in(
                TsnDelta::default(),
                name,
                e.at,
                &input,
                clock.as_ref(),
            )?;
            replay.apply(delta);
            events = events
                .checked_add(1 + e.causes.len())
                .ok_or_else(|| bad("event count overflow"))?;
            octets = octets
                .checked_add(weight(&e.causes)?)
                .ok_or_else(|| bad("octet count overflow"))?;
            previous = e.at;
        }
        if events != journal.events
            || octets != journal.octets
            || events > cfg.max_accounting_events
            || octets > cfg.max_accounting_octets
            || cursor != journal.tail
        {
            return Err(bad("retained causal journal size/tail differs"));
        }
        let actual_waiting: [Vec<usize>; 8] =
            std::array::from_fn(|c| self.queues[i].waiting[c].iter().copied().collect());
        let busy = port
            .busy
            .and_then(|id| self.mac_merge.as_ref().unwrap().packets.get(&id))
            .map(|p| {
                let mut p = normalized(p.clone());
                p.eof_done = self.mac_merge.as_ref().unwrap().packets[&p.id].eof_done;
                p
            });
        if cursor.wire_waiting
            != self
                .wire
                .as_ref()
                .map(|h| h.queues[i].iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
            || cursor.waiting != actual_waiting
            || cursor.pmac != port.selected_pmac
            || cursor.emac != port.selected_emac
            || cursor.busy != busy
            || cursor.epoch != port.tx.epoch
            || cursor.ordinal != port.ordinal
            || cursor.applied != port.applied_tas
            || cursor.lldp_pending != port.lldp_pending
            || cursor.bound != port.accounting.is_some()
        {
            return Err(bad(&format!(
                "causal replay owner differs: queue={} pMAC={} eMAC={} busy={} epoch={} ordinal={} MM={} LLDP={} bound={}",
                cursor.waiting != actual_waiting,
                cursor.pmac != port.selected_pmac,
                cursor.emac != port.selected_emac,
                cursor.busy != busy,
                cursor.epoch != port.tx.epoch,
                cursor.ordinal != port.ordinal,
                cursor.applied != port.applied_tas,
                cursor.lldp_pending != port.lldp_pending,
                cursor.bound != port.accounting.is_some()
            )));
        }
        tsn.validate_merge_replayed_port(&replay, name)
    }
}

#[cfg(test)]
mod configured_native_tests {
    use super::*;

    #[test]
    fn configured_merge_cbs_native_arrival_and_fes_reject_changed_arrival_atomically() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let src = repo.join("examples/mac-merge");
        let root = std::env::temp_dir().join(format!(
            "merge-native-cbs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("models/ethtsn")).unwrap();
        let ned =
            std::fs::read_to_string(repo.join("examples/ethernet/tsn/models/ethtsn/Main.ned"))
                .unwrap()
                .replace(
                    "double delay @unit(s) = default(0ps);",
                    "double delay @unit(s) = default(17000ps);",
                );
        std::fs::write(root.join("models/ethtsn/Main.ned"), ned).unwrap();
        let mut m: Value =
            serde_json::from_slice(&std::fs::read(src.join("model.json")).unwrap()).unwrap();
        m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{
            "priority":0,"idle_slope_bps":"250000000",
            "hi_credit_bits":"10000","lo_credit_bits":"10000"
        }]);
        m["configured_media"] = json!({"seed":"7","physical_links":[{
            "id":"latency","a":"Net.a.tx","b":"Net.b.tx",
            "phy_mode":"1000base-t1","duplex":"full",
            "a_phy":{"role":"master","tx_latency_ps":"30000","rx_latency_ps":"70000"},
            "b_phy":{"role":"slave","tx_latency_ps":"110000","rx_latency_ps":"50000"}
        }]});
        std::fs::write(root.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        std::fs::copy(src.join("workload.json"), root.join("workload.json")).unwrap();
        std::fs::write(root.join("model.ini"), "[General]\nnetwork = ethtsn.Net\nned-path = \"models\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n").unwrap();
        let p = crate::prepare(&root.join("model.ini")).unwrap();
        let mut engine = crate::runtime::registered::Engine::new(Arc::new(p.clone())).unwrap();
        engine.initialize().unwrap();
        let mut checked = false;
        while engine.step() {
            let now = engine.snapshot.common.last_event_time_ps.unwrap_or(0);
            let work = engine.checkpoint_work();
            let keys = work.iter().map(|(k, _)| *k).collect::<Vec<_>>();
            let live = engine.live.clone();
            let sequence = engine.sequence;
            checked = engine.with_network_state_for_test(|state| {
                let merge = state.mac_merge.as_ref().unwrap();
                let Some(packet) = merge.packets.values().find(|packet| {
                    packet.direction == 0 && matches!(packet.origin, Origin::Pmac(_))
                        && packet.sof <= now && packet.eof > now
                }) else { return false };
                assert!(packet.raw.is_none());
                assert_eq!(state.prepared.ethernet.directions[0].delay_ps, 17000);
                assert_eq!(state.physical_receive_delay(0).unwrap(), 97000);
                assert_eq!(packet.arrival, packet.eof + 97000);
                assert!(merge.ports[0].accounting_journal.as_ref().unwrap().entries.iter()
                    .flat_map(|e| &e.causes).any(|c| matches!(c, Cause::Start(p) if p.id == packet.id)));
                let before = state.checkpoint();
                let restored = NetworkState::restore_checkpoint(state.prepared.clone(), &before, now)
                    .unwrap_or_else(|e| panic!("authentic configured Merge CBS Native restore at {now}: {e:?}"));
                restored.validate_checkpoint_reservations(&work, &live, p.common.time_limit_ps).unwrap();
                assert_eq!(restored.checkpoint(), before);
                // Change the actual retained packet AND every journal occurrence,
                // retaining internal agreement so physical timing remains the authority.
                fn change(v: &mut Value, id: u64) -> usize {
                    let mut n = 0;
                    if v.get("id") == Some(&json!(id)) && v.get("sof").is_some()
                        && v.get("eof").is_some() && v.get("arrival").is_some()
                    {
                        v["arrival"] = json!(v["arrival"].as_u64().unwrap() + 1);
                        n += 1;
                    }
                    match v {
                        Value::Array(a) => for v in a { n += change(v, id); },
                        Value::Object(o) => for v in o.values_mut() { n += change(v, id); },
                        _ => {}
                    }
                    n
                }
                let mut bad = before.clone();
                assert!(change(&mut bad["mac_merge"], packet.id) >= 3);
                let error = NetworkState::restore_checkpoint(state.prepared.clone(), &bad, now)
                    .err().expect("changed physical arrival accepted");
                assert!(error.message.contains("retained physical proof identity/timing differs"), "{error:?}");
                assert_eq!(state.checkpoint(), before);
                state.validate_checkpoint_reservations(&work, &live, p.common.time_limit_ps).unwrap();
                eprintln!("Native configured Merge CBS positive + changed-arrival rejection + FES unchanged: now={now} packet={} EOF={} arrival={} delay=97000; {error:?}", packet.id, packet.eof, packet.arrival);
                true
            }).unwrap_or(false);
            assert_eq!(engine.live, live);
            assert_eq!(engine.sequence, sequence);
            assert_eq!(
                engine
                    .checkpoint_work()
                    .iter()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>(),
                keys
            );
            if checked {
                break;
            }
        }
        assert!(
            checked,
            "no actual admitted pMAC packet: {:?}",
            engine.snapshot.common.diagnostics
        );
        while engine.step() {}
        assert_eq!(engine.snapshot.common.termination, "time_limit");
        std::fs::remove_dir_all(root).unwrap();
    }
}
