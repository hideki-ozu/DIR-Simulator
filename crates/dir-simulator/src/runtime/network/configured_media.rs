//! Adapter for configured physical domains; state changes belong to the network delta/FES.
use super::*;
use crate::runtime::ethernet::media::kernel::{Change, Pair, backoff_slots};
use crate::runtime::ethernet::tsn::merge::{MacServiceView, Segment, ServiceAccounting};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Work {
    Data(usize),
    Control(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Attempt {
    pub owner: Work,
    pub direction: usize,
    pub number: u64,
    pub sof: u64,
    pub eof: u64,
    pub release: u64,
    pub arrival: u64,
    pub mdi_sof: u64,
    pub peer_sof: u64,
    pub collision: Option<u64>,
    pub late_collision: bool,
    pub jam_end: Option<u64>,
    pub slots: Option<u64>,
    pub backoff_until: Option<u64>,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct State {
    pub pairs: Vec<Pair>,
    /// Sparse mapping: absence never aliases domain zero.
    pub endpoints: Vec<Option<(usize, usize)>>,
    pub retained: Vec<Option<Work>>,
    /// Synchronous MA_DATA.request owns retries from first SOF until MAC completion.
    pub requests: Vec<bool>,
    pub attempts: Vec<Attempt>,
}
impl State {
    pub fn new(p: &PreparedNetwork) -> Self {
        let n = p.ethernet.directions.len();
        let mut endpoints = vec![None; n];
        let pairs = p
            .ethernet
            .media
            .as_ref()
            .map(|m| {
                m.physical_links
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        for (s, d) in l.directions.iter().enumerate() {
                            endpoints[*d] = Some((i, s));
                        }
                        Pair::default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            pairs,
            endpoints,
            retained: vec![None; n],
            requests: vec![false; n],
            attempts: vec![],
        }
    }
}
impl NetworkState {
    pub(super) fn media_state<'a>(&'a self, d: &'a NetworkDelta) -> &'a State {
        d.configured_media
            .as_ref()
            .unwrap_or(&self.configured_media)
    }
    fn media_mut<'a>(&self, d: &'a mut NetworkDelta) -> &'a mut State {
        d.configured_media
            .get_or_insert_with(|| self.configured_media.clone())
    }
    pub(super) fn configured_endpoint(&self, direction: usize) -> Option<(usize, usize)> {
        self.configured_media.endpoints[direction]
    }
    /// Pure checked reference derivation from a bound historical attempt. Active
    /// MAC ownership is intentionally irrelevant after release and subsequent SOF.
    pub(super) fn configured_gptp_proof(
        &self,
        state: &State,
        binding: gptp::state::ConfiguredAttempt,
        kind: gptp::state::CaptureKind,
    ) -> Result<gptp::state::ConfiguredCaptureProof> {
        use gptp::state::{CaptureKind, ConfiguredCaptureProof, ReferencePlane};
        let a = state
            .attempts
            .get(binding.index)
            .ok_or_else(|| error("configured gPTP attempt missing"))?;
        let endpoint = state.endpoints.get(binding.direction).copied().flatten();
        if a.owner != Work::Data(binding.transfer)
            || a.direction != binding.direction
            || a.number != binding.number
            || binding.number
                != state.attempts[..=binding.index]
                    .iter()
                    .filter(|other| other.owner == a.owner)
                    .count() as u64
            || endpoint != Some((binding.physical_link, binding.side))
            || binding.side > 1
            || a.collision.is_some()
        {
            return Err(error("configured gPTP attempt identity differs"));
        }
        let cfg = self
            .prepared
            .ethernet
            .media
            .as_ref()
            .and_then(|m| m.physical_links.get(binding.physical_link))
            .ok_or_else(|| error("configured gPTP physical link missing"))?;
        if cfg.duplex != "full" || cfg.directions[binding.side] != binding.direction {
            return Err(error("configured gPTP requires full duplex attempt owner"));
        }
        let (tx, rx) = if binding.side == 0 {
            (&cfg.a_phy, &cfg.b_phy)
        } else {
            (&cfg.b_phy, &cfg.a_phy)
        };
        let dir = &self.prepared.ethernet.directions[binding.direction];
        let delta = gptp::integration::reference_duration(dir.bitrate_bps)?;
        let mdi = add(a.sof, tx.tx_latency_ps)?;
        let peer = add(mdi, dir.delay_ps)?;
        let arrival = add(
            add(add(a.eof, tx.tx_latency_ps)?, dir.delay_ps)?,
            rx.rx_latency_ps,
        )?;
        if a.mdi_sof != mdi || a.peer_sof != peer || a.arrival != arrival {
            return Err(error(
                "configured gPTP attempt timeline differs from configuration",
            ));
        }
        let reference = add(if kind == CaptureKind::Tx { mdi } else { peer }, delta)?;
        Ok(ConfiguredCaptureProof {
            source: gptp::state::MacSource::OrdinaryAttempt(binding),
            endpoint: Some((binding.physical_link, binding.side)),
            plane: ReferencePlane::MdiMacSfd,
            reference_ps: reference,
            observed_at_ps: reference,
            sof_ps: a.sof,
            mdi_sof_ps: mdi,
            peer_sof_ps: peer,
            eof_planned_ps: a.eof,
            arrival_planned_ps: arrival,
            bitrate_bps: dir.bitrate_bps,
            propagation_ps: dir.delay_ps,
            tx_latency_ps: tx.tx_latency_ps,
            peer_rx_latency_ps: rx.rx_latency_ps,
        })
    }
    /// MAC EOF to peer MAC receive; selected MAC serializers use this physical adapter once.
    pub(super) fn physical_receive_delay(&self, direction: usize) -> Result<u64> {
        let delay = self.prepared.ethernet.directions[direction].delay_ps;
        let Some((i, side)) = self.configured_endpoint(direction) else {
            return Ok(delay);
        };
        let link = &self
            .prepared
            .ethernet
            .media
            .as_ref()
            .unwrap()
            .physical_links[i];
        let (tx, rx) = if side == 0 {
            (&link.a_phy, &link.b_phy)
        } else {
            (&link.b_phy, &link.a_phy)
        };
        add(add(delay, tx.tx_latency_ps)?, rx.rx_latency_ps)
    }
    fn media_work_id(&self, work: &Work, d: &NetworkDelta) -> String {
        match work {
            Work::Data(t) => self.transfer(d, *t).row.transfer_id.clone(),
            Work::Control(id) => id.clone(),
        }
    }
    fn media_reserve_boundary(&self, index: usize, d: &mut NetworkDelta) -> Result<()> {
        let state = self.media_mut(d);
        let pair = &mut state.pairs[index];
        let at = pair.timeline.first_key_value().map(|(at, _)| *at);
        if at != pair.reservation.map(|(at, _)| at) {
            pair.generation = add(pair.generation, 1)?;
            pair.reservation = at.map(|at| (at, pair.generation));
            let g = pair.generation;
            self.due_mut(d).retain(
                |v| !matches!(v.kind,Completion::ConfiguredMedia{domain,..} if domain==index),
            );
            if let Some(at) = at {
                self.due_mut(d).push(Due {
                    at,
                    kind: Completion::ConfiguredMedia {
                        domain: index,
                        generation: g,
                    },
                });
            }
        }
        Ok(())
    }
    pub(super) fn media_claim(
        &self,
        direction: usize,
        work: Work,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        crate::allocation::reservation_checkpoint("network_configured_media")?;
        let state = self.media_mut(d);
        if state.retained[direction].is_some() {
            return Err(error("configured MAC has two retained requests"));
        }
        state.retained[direction] = Some(work);
        self.media_try_start(direction, d, c)
    }
    pub(super) fn media_try_start(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some((index, side)) = self.configured_endpoint(direction) else {
            return Ok(());
        };
        let state = self.media_state(d);
        let Some(work) = state.retained[direction].clone() else {
            return Ok(());
        };
        let pair = &state.pairs[index];
        if pair.macs[side].emitting.is_some() {
            return Ok(());
        }
        // Release includes IFG even in full duplex.
        if state
            .attempts
            .iter()
            .rev()
            .find(|a| a.direction == direction)
            .is_some_and(|a| a.status == "serialized" && c.now() < a.release)
        {
            return Ok(());
        }
        let link = &self
            .prepared
            .ethernet
            .media
            .as_ref()
            .unwrap()
            .physical_links[index];
        let half = link.duplex == "half";
        if !pair.ready(side, c.now(), half) {
            return Ok(());
        }
        let retry = state.requests[direction];
        let exempt =
            matches!(&work,Work::Control(id) if self.wire_host(d).unwrap().tx[id].is_mac_control());
        if !retry && !self.output_request_allowed(direction, exempt, d, c.now()) {
            return Ok(());
        }
        let (bytes, priority) = match &work {
            Work::Data(t) => (
                self.transfer(d, *t).row.wire.mac_bytes,
                Some(self.transfer(d, *t).row.priority),
            ),
            Work::Control(id) => (
                self.wire_host(d).unwrap().tx[id].intent.bytes.len() as u64,
                (!exempt).then(|| self.wire_priority(direction, id, d)),
            ),
        };
        if !retry && !exempt {
            if let Some(tsn) = &self.tsn {
                let port = &self.prepared.ethernet.directions[direction].from_port;
                let occupancy = l2::timing(
                    0,
                    bytes,
                    self.prepared.ethernet.directions[direction].bitrate_bps,
                    0,
                )?
                .1;
                if tsn
                    .retained_start_permission_clock_in(
                        &d.tsn,
                        port,
                        priority.unwrap_or(7),
                        c.now(),
                        occupancy,
                        self.clock_for_port(d, port),
                    )?
                    .is_none()
                {
                    return Ok(());
                }
            }
        }
        let dir = &self.prepared.ethernet.directions[direction];
        let (eof, release, base_arrival) =
            l2::timing(c.now(), bytes, dir.bitrate_bps, dir.delay_ps)?;
        let (tx, rx) = if side == 0 {
            (&link.a_phy, &link.b_phy)
        } else {
            (&link.b_phy, &link.a_phy)
        };
        let arrival = add(add(base_arrival, tx.tx_latency_ps)?, rx.rx_latency_ps)?;
        let id = self.media_work_id(&work, d);
        let number = state.attempts.iter().filter(|a| a.owner == work).count() as u64 + 1;
        let a = Attempt {
            owner: work.clone(),
            direction,
            number,
            sof: c.now(),
            eof,
            release,
            arrival,
            mdi_sof: add(c.now(), tx.tx_latency_ps)?,
            peer_sof: add(add(c.now(), tx.tx_latency_ps)?, dir.delay_ps)?,
            collision: None,
            late_collision: false,
            jam_end: None,
            slots: None,
            backoff_until: None,
            status: "transmitting".into(),
        };
        let state = self.media_mut(d);
        let ai = state.attempts.len();
        state.attempts.push(a.clone());
        state.requests[direction] = true;
        let pair = &mut state.pairs[index];
        pair.macs[side].emitting = Some(ai);
        pair.macs[side].backoff_until = None;
        pair.change(eof, Change::Eof(side, ai));
        pair.change(release, Change::Release(side, ai));
        if half {
            pair.change(add(c.now(), dir.delay_ps)?, Change::SignalOn(1 - side, ai));
            pair.change(add(eof, dir.delay_ps)?, Change::SignalOff(1 - side, ai));
            pair.macs[side].busy = true;
            pair.macs[side].ifg_ready = false;
            pair.cancel_ifg(side);
        }
        match &work {
            Work::Data(t) => {
                let mut row = self.transfer(d, *t).clone();
                row.row.time_ps = c.now();
                row.row.sof_ps = Some(c.now());
                row.row.planned_eof_ps = Some(eof);
                row.row.planned_release_ps = Some(release);
                row.row.planned_arrival_ps = Some(arrival);
                row.row.status = "transmitting".into();
                row.sof_epoch = Some(self.epoch_after(d));
                let m = row
                    .row
                    .media
                    .as_mut()
                    .ok_or_else(|| error("configured transfer lacks physical identity"))?;
                m.attempt_count = number;
                m.last_attempt_id = Some(format!("{id}#{number}"));
                m.backoff_until_ps = None;
                put(&mut d.transfers, *t, row);
                self.gptp_tx_start(*t, None, self.epoch_after(d), c.now(), d, c)?;
                self.qcc_sof(*t, d, c)?;
                self.replication_sof(*t, d, c.now())?;
            }
            Work::Control(id) => {
                let host = self.wire_mut(d)?;
                let tx = host.tx.get_mut(id).unwrap();
                tx.sof = Some(c.now());
                tx.planned_eof = Some(eof);
                tx.planned_release = Some(release);
                tx.planned_arrival = Some(arrival);
                self.wire_tx_record(direction, id, "tx_started", None, d, c)?;
            }
        }
        self.media_record(ai, "start", d, c)?;
        self.media_check_gate_overruns(d, c)?;
        self.media_reserve_boundary(index, d)?;
        self.media_settle_shaper(direction, d, c)
    }
    /// Observe a real closed gate under an already submitted synchronous request.
    /// This never cancels/requeues its frame, collision retry or MAC claim.
    pub(super) fn media_check_gate_overruns(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(tsn) = &self.tsn else { return Ok(()) };
        for direction in 0..self.queues.len() {
            let Some((domain, side)) = self.configured_endpoint(direction) else {
                continue;
            };
            let Some(ai) = self.media_state(d).pairs[domain].macs[side].emitting else {
                continue;
            };
            let a = &self.media_state(d).attempts[ai];
            if a.status != "transmitting" {
                continue;
            }
            let Work::Data(t) = a.owner else { continue };
            let row = &self.transfer(d, t).row;
            let port = &self.prepared.ethernet.directions[direction].from_port;
            if !tsn.retained_gate_fit_clock_in(
                &d.tsn,
                port,
                row.priority,
                c.now(),
                1,
                self.clock_for_port(d, port),
            )? {
                self.media_record(ai, "gate_overrun", d, c)?;
            }
        }
        Ok(())
    }
    fn media_record(
        &self,
        attempt: usize,
        kind: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let a = self.media_state(d).attempts[attempt].clone();
        let id = self.media_work_id(&a.owner, d);
        Self::record(
            c,
            d,
            "dir.network.configured_media",
            if kind == "gate_overrun" {
                format!("{id}#{}:{kind}:{}", a.number, c.now())
            } else {
                format!("{id}#{}:{kind}", a.number)
            },
            decimal(
                json!({"kind":kind,"work_id":id,"attempt_index":attempt.to_string(),"attempt":a,"physical_link":self.prepared.ethernet.media.as_ref().unwrap().physical_links[self.configured_endpoint(a.direction).unwrap().0].id}),
            ),
        )
    }
    pub(super) fn media_service_view(&self, direction: usize, d: &NetworkDelta) -> MacServiceView {
        let s = self.media_state(d);
        let work = s.retained[direction].as_ref();
        let a = s.attempts.iter().rev().find(|a| a.direction == direction);
        let mut backlog = [false; 8];
        let priority = work.and_then(|w| match w {
            Work::Data(t) => Some(self.transfer(d, *t).row.priority),
            Work::Control(id) => (!self.wire_host(d).unwrap().tx[id].is_mac_control())
                .then(|| self.wire_priority(direction, id, d)),
        });
        if let Some(p) = priority {
            // IFG occupancy outlives the completed frame; it is no longer backlog.
            backlog[p as usize] = a.is_none_or(|a| a.status != "serialized");
        }
        MacServiceView {
            active_work: work.map(|w| self.media_work_id(w, d)),
            retained_backlog: backlog,
            transmit_class: a.filter(|a| a.status == "transmitting").and(priority),
            busy: work.is_some(),
            release_ps: a
                .filter(|a| {
                    work.is_some()
                        && a.collision.is_none()
                        && (a.status == "transmitting" || a.status == "serialized")
                })
                .map(|a| a.release),
            next_transition_ps: self
                .configured_endpoint(direction)
                .and_then(|(i, _)| s.pairs[i].timeline.first_key_value().map(|(t, _)| *t)),
            segment: if a.is_some_and(|a| a.status == "transmitting") && priority.is_some() {
                Segment::OriginalMac
            } else {
                Segment::Idle
            },
            accounting: ServiceAccounting::LegacyCapped,
        }
    }
    fn media_settle_shaper(
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
            self.stage_tsn(d, delta, &[port])?;
        }
        Ok(())
    }
    pub(super) fn media_boundary(
        &self,
        index: usize,
        generation: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        crate::allocation::reservation_checkpoint("network_configured_media_boundary")?;
        let cfg = self.prepared.ethernet.media.as_ref().unwrap();
        let link = &cfg.physical_links[index];
        let half = link.duplex == "half";
        let rate = self.prepared.ethernet.directions[link.directions[0]].bitrate_bps;
        let bit = 1_000_000_000_000 / rate;
        let propagation = self.prepared.ethernet.directions[link.directions[0]].delay_ps;
        let mut state = self.media_state(d).clone();
        let pair = &mut state.pairs[index];
        if pair.reservation != Some((c.now(), generation)) {
            return Ok(());
        }
        pair.reservation = None;
        let changes = pair
            .timeline
            .remove(&c.now())
            .ok_or_else(|| error("configured missing boundary"))?;
        pair.settle_signals(&changes);
        if half {
            for (side, ai, _start, end) in pair
                .collisions(c.now(), bit, |ai| {
                    let a = &state.attempts[ai];
                    (a.status == "transmitting").then_some(a.sof)
                })
                .map_err(error)?
            {
                let a = &mut state.attempts[ai];
                a.collision = Some(c.now());
                a.late_collision =
                    crate::runtime::ethernet::media::kernel::late_collision(rate, a.sof, c.now()).0;
                a.jam_end = Some(end);
                a.status = "jamming".into();
                pair.cancel_attempt(ai);
                pair.change(end, Change::JamEnd(side, ai));
                pair.change(add(end, propagation)?, Change::SignalOff(1 - side, ai));
            }
        }
        let mut finished = Vec::new();
        let mut completed = Vec::new();
        let mut release = Vec::new();
        let mut changed = Vec::new();
        for change in changes {
            match change {
                Change::Eof(_, ai) if state.attempts[ai].status == "transmitting" => {
                    let a = &mut state.attempts[ai];
                    a.status = "serialized".into();
                    state.requests[a.direction] = false;
                    completed.push(a.direction);
                    finished.push(ai);
                    changed.push((ai, "eof"));
                }
                Change::Release(side, ai) if state.attempts[ai].status == "serialized" => {
                    let a = &state.attempts[ai];
                    state.retained[a.direction] = None;
                    release.push((link.directions[side], a.owner.clone(), None));
                    changed.push((ai, "release"));
                }
                Change::JamEnd(side, ai) => {
                    let a = &mut state.attempts[ai];
                    a.status = "collided".into();
                    let id = match &a.owner {
                        Work::Data(t) => self.transfer(d, *t).row.transfer_id.clone(),
                        Work::Control(id) => id.clone(),
                    };
                    // At <=100 Mb/s late collision reporting is not mandated; configured topology bounds ordinary collisions.
                    let late = crate::runtime::ethernet::media::kernel::late_collision(
                        rate,
                        a.sof,
                        a.collision.unwrap(),
                    )
                    .1;
                    if a.number >= 16 || late {
                        state.requests[a.direction] = false;
                        completed.push(a.direction);
                        state.retained[a.direction] = None;
                        release.push((
                            a.direction,
                            a.owner.clone(),
                            Some(if late {
                                "late_collision"
                            } else {
                                "attempt_limit"
                            }),
                        ));
                        pair.macs[side].backoff_until = None;
                    } else {
                        let slots = backoff_slots(cfg.seed, &id, a.number);
                        let until = add(
                            c.now(),
                            slots * if rate == 1_000_000_000 { 4096 } else { 512 } * bit,
                        )?;
                        a.slots = Some(slots);
                        a.backoff_until = Some(until);
                        pair.macs[side].backoff_until = Some(until);
                        if until > c.now() {
                            pair.change(until, Change::Backoff(side, ai));
                        }
                    }
                    changed.push((ai, "jam_end"));
                }
                Change::Ifg(side, g) if pair.macs[side].idle_generation == g => {
                    pair.macs[side].ifg_ready = true;
                }
                Change::Backoff(side, _) => {
                    pair.macs[side].backoff_until = Some(c.now());
                }
                _ => {}
            }
        }
        if half {
            pair.settle_idle(c.now(), bit).map_err(error)?;
        }
        d.configured_media = Some(state);
        // Publish actual attempt status; planned success is never a receive indication.
        for &direction in &link.directions {
            if let Some(Work::Data(t)) = self.media_state(d).retained[direction].clone() {
                if let Some(a) = self
                    .media_state(d)
                    .attempts
                    .iter()
                    .rev()
                    .find(|a| a.direction == direction)
                    .cloned()
                {
                    let mut row = self.transfer(d, t).clone();
                    row.row.time_ps = c.now();
                    row.row.status = if a.status == "collided" {
                        if a.backoff_until.is_some_and(|t| t > c.now()) {
                            "backoff"
                        } else {
                            "deferred"
                        }
                    } else {
                        &a.status
                    }
                    .into();
                    if a.collision.is_some() {
                        row.row.planned_eof_ps = None;
                        row.row.planned_release_ps = None;
                        row.row.planned_arrival_ps = None;
                    }
                    let m = row.row.media.as_mut().unwrap();
                    m.collision_count = self
                        .media_state(d)
                        .attempts
                        .iter()
                        .filter(|a| a.owner == Work::Data(t) && a.collision.is_some())
                        .count() as u64;
                    m.backoff_until_ps = a.backoff_until;
                    put(&mut d.transfers, t, row);
                }
            }
        }
        for ai in finished {
            let a = self.media_state(d).attempts[ai].clone();
            match a.owner {
                Work::Data(t) => {
                    let mut row = self.transfer(d, t).clone();
                    row.row.eof_ps = Some(c.now());
                    put(&mut d.transfers, t, row);
                    self.gptp_tx_eof(t, None, d, c)?;
                    Self::schedule(c, d, a.arrival, Event::Arrival(t))?;
                }
                Work::Control(id) => {
                    self.wire_mut(d)?.tx.get_mut(&id).unwrap().eof = Some(c.now());
                    self.wire_tx_record(a.direction, &id, "tx_completed", None, d, c)?;
                    Self::schedule(
                        c,
                        d,
                        a.arrival,
                        Event::Wire(HostEvent::Arrival {
                            direction: a.direction,
                            id,
                        }),
                    )?;
                }
            }
        }
        for (direction, work, failure) in release {
            match work {
                Work::Data(t) => {
                    let mut row = self.transfer(d, t).clone();
                    row.row.time_ps = c.now();
                    if let Some(reason) = failure {
                        row.row.status = "dropped".into();
                        row.row.drop_reason = Some(reason.into());
                    } else {
                        row.row.release_ps = Some(c.now());
                    }
                    self.queue_mut(d, direction).active = None;
                    put(&mut d.transfers, t, row);
                }
                Work::Control(id) => {
                    let host = self.wire_mut(d)?;
                    host.active[direction] = None;
                    let tx = host.tx.get_mut(&id).unwrap();
                    tx.dropped = failure.is_some();
                    tx.release = Some(c.now());
                    if let Some(reason) = failure {
                        self.wire_tx_record(direction, &id, "tx_completed", Some(reason), d, c)?;
                    }
                }
            }
        }
        for direction in completed {
            self.pause_mac_completed(direction, d, c)?;
        }
        for (ai, kind) in changed {
            self.media_record(ai, kind, d, c)?;
        }
        self.media_reserve_boundary(index, d)?;
        for &direction in &link.directions {
            self.media_settle_shaper(direction, d, c)?;
        }
        c.request_arbitration("network").map_err(model)
    }
}
impl NetworkState {
    pub(super) fn validate_media_pause_checkpoint(&self, horizon: u64) -> Result<()> {
        let expected = State::new(&self.prepared);
        let s = &self.configured_media;
        if s.endpoints != expected.endpoints
            || s.pairs.len() != expected.pairs.len()
            || s.retained.len() != expected.retained.len()
            || s.requests.len() != expected.requests.len()
        {
            return Err(error("configured-media checkpoint owner dimensions"));
        }
        for (i, pair) in s.pairs.iter().enumerate() {
            if pair.timeline.keys().any(|t| *t < horizon)
                || pair.reservation.map(|(t, _)| t)
                    != pair.timeline.first_key_value().map(|(t, _)| *t)
            {
                return Err(error("configured-media checkpoint timeline"));
            }
            for mac in &pair.macs {
                if [mac.emitting, mac.carrier]
                    .into_iter()
                    .flatten()
                    .any(|a| a >= s.attempts.len())
                {
                    return Err(error("configured-media checkpoint attempt reference"));
                }
            }
            let reservations=self.due.iter().filter(|v| matches!(v.kind,Completion::ConfiguredMedia{domain,generation} if domain==i && pair.reservation==Some((v.at,generation)))).count();
            if reservations != usize::from(pair.reservation.is_some()) {
                return Err(error("configured-media checkpoint boundary reservation"));
            }
        }
        for (direction, work) in s.retained.iter().enumerate() {
            if let Some(work) = work {
                if s.endpoints[direction].is_none() {
                    return Err(error("configured-media work has no physical owner"));
                }
                match work {
                    Work::Data(t) => {
                        if self.queues[direction].active != Some(*t)
                            || self
                                .transfers
                                .get(*t)
                                .is_none_or(|t| t.direction != direction)
                        {
                            return Err(error("configured-media checkpoint data claim"));
                        }
                    }
                    Work::Control(id) => {
                        if self
                            .wire
                            .as_ref()
                            .is_none_or(|h| h.active[direction].as_ref() != Some(id))
                        {
                            return Err(error("configured-media checkpoint control claim"));
                        }
                    }
                }
            }
        }
        for a in &s.attempts {
            if a.direction >= s.endpoints.len()
                || s.endpoints[a.direction].is_none()
                || a.number == 0
                || a.number > 16
                || a.sof > horizon
                || a.eof <= a.sof
                || a.release <= a.eof
            {
                return Err(error("configured-media checkpoint attempt identity/time"));
            }
        }
        let initial = mac_control::State::new(&self.prepared);
        if self.mac_control.ports.len() != initial.ports.len() {
            return Err(error("MAC Control checkpoint dimensions"));
        }
        for (direction, (port, prepared)) in self
            .mac_control
            .ports
            .iter()
            .zip(&initial.ports)
            .enumerate()
        {
            match (port, prepared) {
                (Some(p), Some(_)) => {
                    if p.pending.as_ref().is_some_and(|r| r.received_ps > horizon) {
                        return Err(error("MAC Control checkpoint future pending receive"));
                    }
                    if let Some(t) = &p.timer {
                        if t.generation != p.generation
                            || t.loaded_ps > horizon
                            || t.request.received_ps > t.loaded_ps
                            || t.expires_ps < horizon
                            || t.expires_ps
                                != add(
                                    t.loaded_ps,
                                    self.pause_duration(direction, t.request.quanta)?,
                                )?
                        {
                            return Err(error("MAC Control checkpoint timer causality"));
                        }
                        if self.due.iter().filter(|v|matches!(v.kind,Completion::PauseExpire{direction:d,generation} if d==direction&&generation==t.generation)&&v.at==t.expires_ps).count()!=1{return Err(error("MAC Control checkpoint expiry reservation"))}
                    }
                }
                (None, None) => {}
                _ => return Err(error("MAC Control checkpoint port capability")),
            }
        }
        Ok(())
    }
}
