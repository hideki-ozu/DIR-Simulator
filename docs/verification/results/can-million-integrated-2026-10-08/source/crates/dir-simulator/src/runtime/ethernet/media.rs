//! Pair-local media timelines. A physical reservation covers all logical changes at its time.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
enum Change {
    SignalOn(usize, usize),
    SignalOff(usize, usize),
    Eof(usize, usize),
    Release(usize, usize),
    JamEnd(usize, usize),
    Ifg(usize, u64),
    Backoff(usize, usize),
}
#[derive(Clone, Default)]
struct Mac {
    emitting: Option<usize>,
    carrier: Option<usize>,
    busy: bool,
    ifg_ready: bool,
    idle_generation: u64,
    backoff_until: Option<u64>,
    generation: u64,
}
#[derive(Clone)]
struct Pair {
    macs: [Mac; 2],
    timeline: BTreeMap<u64, Vec<Change>>,
    reservation: Option<(u64, u64)>,
    generation: u64,
}
pub(super) struct MediaRuntime {
    pairs: Vec<Pair>,
    pub(super) direction_pairs: Vec<usize>,
    #[cfg(test)]
    injected_slots: Option<u64>,
}
impl MediaRuntime {
    pub(super) fn new(config: &EthernetMediaConfig, directions: usize) -> Self {
        let mut direction_pairs = vec![0; directions];
        let pairs = config
            .physical_links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                for d in link.directions {
                    direction_pairs[d] = index;
                }
                Pair {
                    macs: [
                        Mac {
                            ifg_ready: true,
                            ..Mac::default()
                        },
                        Mac {
                            ifg_ready: true,
                            ..Mac::default()
                        },
                    ],
                    timeline: BTreeMap::new(),
                    reservation: None,
                    generation: 0,
                }
            })
            .collect();
        Self {
            pairs,
            direction_pairs,
            #[cfg(test)]
            injected_slots: None,
        }
    }
}
pub fn backoff_slots(seed: u64, transfer_id: &str, number: u64) -> u64 {
    let mut hash = Sha256::new();
    hash.update(b"dir.ethernet.beb.v1\0");
    hash.update(seed.to_string().as_bytes());
    hash.update([0]);
    hash.update(transfer_id.as_bytes());
    hash.update([0]);
    hash.update(number.to_string().as_bytes());
    let bytes = hash.finalize();
    let value = u64::from_be_bytes(bytes[..8].try_into().unwrap());
    value & ((1u64 << number.min(10)) - 1)
}
impl Pair {
    fn change(&mut self, time: u64, change: Change) {
        self.timeline.entry(time).or_default().push(change);
    }
    fn cancel_attempt(&mut self, attempt: usize) {
        self.timeline.retain(|_, changes| {
            changes.retain(|change| !matches!(change, Change::Eof(_, a) | Change::Release(_, a) | Change::SignalOff(_, a) if *a == attempt));
            !changes.is_empty()
        });
    }
    fn cancel_ifg(&mut self, side: usize) {
        self.timeline.retain(|_, changes| {
            changes.retain(|change| !matches!(change, Change::Ifg(s, _) if *s == side));
            !changes.is_empty()
        });
    }
}
impl Engine<'_> {
    pub(super) fn media_canceled(&self, event: &Event) -> bool {
        if let Event::PairBoundary(pair, generation) = event {
            return self.media.as_ref().unwrap().pairs[*pair].reservation
                != Some((
                    self.media.as_ref().unwrap().pairs[*pair]
                        .timeline
                        .first_key_value()
                        .map_or(0, |(t, _)| *t),
                    *generation,
                ));
        }
        false
    }
    // Preflight a replacement physical reservation before exposing the staged pair/rows.
    fn media_reserve(
        &self,
        index: usize,
        pair: &mut Pair,
        extra: &[(u64, u8, Event)],
    ) -> Result<Vec<(Key, Event)>> {
        let earliest = pair.timeline.first_key_value().map(|(time, _)| *time);
        let mut events = extra.to_vec();
        if earliest != pair.reservation.map(|(t, _)| t) {
            pair.generation = pair
                .generation
                .checked_add(1)
                .ok_or_else(|| overflow("Ethernet pair generation overflow"))?;
            pair.reservation = earliest.map(|time| (time, pair.generation));
            if let Some(time) = earliest {
                events.push((time, 0, Event::PairBoundary(index, pair.generation)));
            }
        }
        self.reserve(&events)
    }
    pub(super) fn media_start(&mut self, index: usize) -> Result<()> {
        let config = self.model.media.as_ref().unwrap();
        let link = &config.physical_links[index];
        let half = link.duplex == "half";
        let mut pair = self.media.as_ref().unwrap().pairs[index].clone();
        let mut starts = Vec::new();
        // Both sides inspect the same state; no carrier produced by this batch is visible yet.
        for (side, &d) in link.directions.iter().enumerate() {
            let active = self.queues[d].active;
            let candidate = active.or_else(|| self.next_waiting(d).map(|(_, t)| t));
            let Some(t) = candidate else {
                continue;
            };
            let status = self.eth().transfers[t].status.as_str();
            if active.is_some() && !matches!(status, "backoff" | "deferred") {
                continue;
            }
            let m = &pair.macs[side];
            if half && (m.busy || !m.ifg_ready || m.backoff_until.is_some_and(|t| t > self.now.0)) {
                continue;
            }
            let transfer = &self.eth().transfers[t];
            let rate = self.model.directions[d].bitrate_bps;
            let eof = add(
                self.now.0,
                duration((transfer.wire.mac_bytes + 8) * 8, rate)?,
            )?;
            let release = add(
                self.now.0,
                duration((transfer.wire.mac_bytes + 20) * 8, rate)?,
            )?;
            let (tx, rx) = if side == 0 {
                (link.a_phy.tx_latency_ps, link.b_phy.rx_latency_ps)
            } else {
                (link.b_phy.tx_latency_ps, link.a_phy.rx_latency_ps)
            };
            let mdi_sof = add(self.now.0, tx)?;
            let peer_sof = add(mdi_sof, self.model.directions[d].delay_ps)?;
            let mdi_eof = add(eof, tx)?;
            let peer_eof = add(mdi_eof, self.model.directions[d].delay_ps)?;
            let arrival = add(peer_eof, rx)?;
            let generation = m
                .generation
                .checked_add(1)
                .ok_or_else(|| overflow("Ethernet attempt generation overflow"))?;
            let number = transfer
                .media
                .as_ref()
                .unwrap()
                .attempt_count
                .checked_add(1)
                .ok_or_else(|| overflow("Ethernet attempt count overflow"))?;
            let attempt = self.eth().attempts.len() + starts.len();
            let row = EthernetAttemptRecord {
                attempt_id: format!("{}#{number}", transfer.transfer_id),
                transfer: t,
                generation,
                time_ps: self.now.0,
                number,
                sof_ps: self.now.0,
                planned_eof_ps: eof,
                planned_release_ps: release,
                planned_arrival_ps: arrival,
                collision_ps: None,
                planned_jam_start_ps: None,
                planned_jam_end_ps: None,
                jam_end_ps: None,
                eof_ps: None,
                release_ps: None,
                arrival_ps: None,
                backoff_slots: None,
                backoff_until_ps: None,
                status: "transmitting".into(),
                planned_mdi_sof_ps: mdi_sof,
                planned_mdi_eof_ps: None,
                planned_peer_mdi_sof_ps: peer_sof,
                planned_peer_mdi_eof_ps: None,
            };
            pair.macs[side].emitting = Some(attempt);
            pair.macs[side].generation = generation;
            pair.macs[side].backoff_until = None;
            pair.change(eof, Change::Eof(side, attempt));
            pair.change(release, Change::Release(side, attempt));
            if half {
                let propagation = self.model.directions[d].delay_ps;
                pair.change(
                    add(self.now.0, propagation)?,
                    Change::SignalOn(1 - side, attempt),
                );
                pair.change(add(eof, propagation)?, Change::SignalOff(1 - side, attempt));
                pair.macs[side].busy = true;
                pair.macs[side].ifg_ready = false;
                pair.cancel_ifg(side);
            }
            starts.push((side, d, t, active.is_some(), row));
        }
        let events = self.media_reserve(index, &mut pair, &[])?;
        // Everything required by both starts has been checked before moving FIFO ownership.
        for (_, d, t, retry, attempt) in starts {
            if !retry {
                let priority = self.eth().transfers[t].priority as usize;
                self.queues[d].waiting[priority].pop_front();
                self.queues[d].bytes[priority] -= self.eth().transfers[t].wire.mac_bytes;
                self.queues[d].active = Some(t);
                self.queue_point(d, self.frame_indices[t], priority as u8);
            }
            let now = self.now.0;
            let transfer = &mut self.eth_mut().transfers[t];
            transfer.time_ps = now;
            transfer.sof_ps = Some(now);
            transfer.status = "transmitting".into();
            transfer.planned_eof_ps = Some(attempt.planned_eof_ps);
            transfer.planned_release_ps = Some(attempt.planned_release_ps);
            transfer.planned_arrival_ps = Some(attempt.planned_arrival_ps);
            let media = transfer.media.as_mut().unwrap();
            media.attempt_count = attempt.number;
            media.last_attempt_id = Some(attempt.attempt_id.clone());
            media.backoff_until_ps = None;
            self.eth_mut().attempts.push(attempt);
        }
        self.media.as_mut().unwrap().pairs[index] = pair;
        self.publish(events);
        self.media_deferred(index);
        Ok(())
    }
    fn media_deferred(&mut self, index: usize) {
        let link = &self.model.media.as_ref().unwrap().physical_links[index];
        let mut changes = Vec::new();
        for (side, &d) in link.directions.iter().enumerate() {
            let Some(t) = self.queues[d]
                .active
                .or_else(|| self.next_waiting(d).map(|(_, t)| t))
            else {
                continue;
            };
            let status = self.eth().transfers[t].status.as_str();
            if !matches!(status, "queued" | "backoff" | "deferred") {
                continue;
            }
            let m = &self.media.as_ref().unwrap().pairs[index].macs[side];
            let status = if m.backoff_until.is_some_and(|t| t > self.now.0) {
                "backoff"
            } else {
                "deferred"
            };
            changes.push((t, status));
        }
        let now = self.now.0;
        for (t, status) in changes {
            if self.eth().transfers[t].status != status {
                let row = &mut self.eth_mut().transfers[t];
                row.time_ps = now;
                row.status = status.into();
            }
        }
    }
    pub(super) fn media_boundary(&mut self, index: usize, generation: u64) -> Result<()> {
        let link = &self.model.media.as_ref().unwrap().physical_links[index];
        let half = link.duplex == "half";
        let rate = self.model.directions[link.directions[0]].bitrate_bps;
        let propagation = self.model.directions[link.directions[0]].delay_ps;
        let bit = 1_000_000_000_000 / rate;
        let mut pair = self.media.as_ref().unwrap().pairs[index].clone();
        if pair.reservation != Some((self.now.0, generation)) {
            return Err(failure("Ethernet pair reservation generation mismatch"));
        }
        pair.reservation = None;
        let changes = pair
            .timeline
            .remove(&self.now.0)
            .ok_or_else(|| failure("Ethernet missing pair timeline"))?;
        let mut attempts = BTreeMap::<usize, EthernetAttemptRecord>::new();
        let mut transfers = BTreeMap::<usize, EthernetTransferRecord>::new();
        let mut clear = Vec::new();
        let mut extra = Vec::new();
        let mut draws = Vec::new();
        // Half-open signal endpoints are applied before collision detection or completion.
        for change in &changes {
            match *change {
                Change::SignalOff(side, a) if pair.macs[side].carrier == Some(a) => {
                    pair.macs[side].carrier = None
                }
                Change::Eof(side, a) | Change::JamEnd(side, a)
                    if pair.macs[side].emitting == Some(a) =>
                {
                    pair.macs[side].emitting = None
                }
                _ => {}
            }
        }
        for change in &changes {
            if let Change::SignalOn(side, a) = *change {
                pair.macs[side].carrier = Some(a);
            }
        }
        if half {
            for side in 0..2 {
                let m = &pair.macs[side];
                if let (Some(a), Some(_)) = (m.emitting, m.carrier) {
                    let original = &self.eth().attempts[a];
                    if original.status != "transmitting" {
                        continue;
                    }
                    let start = self.now.0.max(add(original.sof_ps, 64 * bit)?);
                    let end = add(start, 32 * bit)?;
                    let mut row = original.clone();
                    row.time_ps = self.now.0;
                    row.collision_ps = Some(self.now.0);
                    row.planned_jam_start_ps = Some(start);
                    row.planned_jam_end_ps = Some(end);
                    row.status = "jamming".into();
                    let mut transfer = self.eth().transfers[row.transfer].clone();
                    transfer.time_ps = self.now.0;
                    transfer.status = "jamming".into();
                    transfer.planned_eof_ps = None;
                    transfer.planned_release_ps = None;
                    transfer.planned_arrival_ps = None;
                    transfer.media.as_mut().unwrap().collision_count += 1;
                    pair.cancel_attempt(a);
                    pair.change(end, Change::JamEnd(side, a));
                    pair.change(add(end, propagation)?, Change::SignalOff(1 - side, a));
                    transfers.insert(row.transfer, transfer);
                    attempts.insert(a, row);
                }
            }
        }
        for change in changes {
            match change {
                Change::Eof(side, a)
                    if self.eth().attempts[a].status == "transmitting"
                        && !attempts.contains_key(&a) =>
                {
                    let mut row = self.eth().attempts[a].clone();
                    row.time_ps = self.now.0;
                    row.eof_ps = Some(self.now.0);
                    row.status = "serialized".into();
                    let tx = if side == 0 {
                        link.a_phy.tx_latency_ps
                    } else {
                        link.b_phy.tx_latency_ps
                    };
                    row.planned_mdi_eof_ps = Some(add(self.now.0, tx)?);
                    row.planned_peer_mdi_eof_ps = Some(add(add(self.now.0, tx)?, propagation)?);
                    extra.push((
                        row.planned_arrival_ps,
                        1,
                        Event::MediaArrival(a, row.generation),
                    ));
                    let mut t = self.eth().transfers[row.transfer].clone();
                    t.time_ps = self.now.0;
                    t.eof_ps = Some(self.now.0);
                    t.status = "serialized".into();
                    transfers.insert(row.transfer, t);
                    attempts.insert(a, row);
                }
                Change::Release(side, a) => {
                    let mut row = attempts.get(&a).unwrap_or(&self.eth().attempts[a]).clone();
                    if row.status != "serialized" {
                        continue;
                    }
                    row.time_ps = self.now.0;
                    row.release_ps = Some(self.now.0);
                    let mut t = transfers
                        .get(&row.transfer)
                        .unwrap_or(&self.eth().transfers[row.transfer])
                        .clone();
                    t.time_ps = self.now.0;
                    t.release_ps = Some(self.now.0);
                    clear.push(link.directions[side]);
                    transfers.insert(row.transfer, t);
                    attempts.insert(a, row);
                }
                Change::JamEnd(side, a) => {
                    let mut row = self.eth().attempts[a].clone();
                    row.time_ps = self.now.0;
                    row.jam_end_ps = Some(self.now.0);
                    row.status = "collided".into();
                    let mut t = self.eth().transfers[row.transfer].clone();
                    t.time_ps = self.now.0;
                    if row.number == 16 {
                        t.status = "dropped".into();
                        t.drop_reason = Some("attempt_limit".into());
                        pair.macs[side].backoff_until = None;
                        clear.push(link.directions[side]);
                    } else {
                        let slots = backoff_slots(
                            self.model.media.as_ref().unwrap().seed,
                            &t.transfer_id,
                            row.number,
                        );
                        #[cfg(test)]
                        let slots = self.media.as_ref().unwrap().injected_slots.unwrap_or(slots);
                        let until = add(self.now.0, slots * 512 * bit)?;
                        row.backoff_slots = Some(slots);
                        row.backoff_until_ps = Some(until);
                        t.media.as_mut().unwrap().backoff_until_ps = Some(until);
                        t.status = if until > self.now.0 {
                            "backoff"
                        } else {
                            "deferred"
                        }
                        .into();
                        pair.macs[side].backoff_until = Some(until);
                        if until > self.now.0 {
                            pair.change(until, Change::Backoff(side, a));
                        }
                        draws.push((link.directions[side], row.transfer, slots));
                    }
                    transfers.insert(row.transfer, t);
                    attempts.insert(a, row);
                }
                Change::Ifg(side, g) if pair.macs[side].idle_generation == g => {
                    pair.macs[side].ifg_ready = true
                }
                Change::Backoff(side, a) => {
                    let t = self.eth().attempts[a].transfer;
                    let mut row = transfers
                        .get(&t)
                        .unwrap_or(&self.eth().transfers[t])
                        .clone();
                    row.time_ps = self.now.0;
                    row.status = "deferred".into();
                    transfers.insert(t, row);
                    pair.macs[side].backoff_until = Some(self.now.0);
                }
                _ => {}
            }
        }
        if half {
            for side in 0..2 {
                let busy = pair.macs[side].emitting.is_some() || pair.macs[side].carrier.is_some();
                let before = pair.macs[side].busy;
                pair.macs[side].busy = busy;
                if busy {
                    pair.macs[side].ifg_ready = false;
                    pair.cancel_ifg(side);
                } else if before {
                    pair.macs[side].ifg_ready = false;
                    pair.macs[side].idle_generation = pair.macs[side]
                        .idle_generation
                        .checked_add(1)
                        .ok_or_else(|| overflow("Ethernet IFG generation overflow"))?;
                    pair.change(
                        add(self.now.0, 96 * bit)?,
                        Change::Ifg(side, pair.macs[side].idle_generation),
                    );
                }
            }
        }
        let events = self.media_reserve(index, &mut pair, &extra)?;
        for (i, row) in attempts {
            self.eth_mut().attempts[i] = row;
        }
        for (i, row) in transfers {
            self.eth_mut().transfers[i] = row;
        }
        for d in clear {
            self.queues[d].active = None;
        }
        for (d, t, slots) in draws {
            self.point(
                self.model.directions[d].from_port.clone(),
                "ethernet.media.backoff_slots",
                slots,
                Some(self.frame_indices[t]),
                None,
            );
        }
        self.media.as_mut().unwrap().pairs[index] = pair;
        for d in link.directions {
            self.dirty.insert((self.now.0, self.now.1, d));
        }
        self.publish(events);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_sha256_beb_vectors_match() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../docs/verification/fixtures/ethernet-media/backoff-vectors.json"
        ))
        .unwrap();
        for v in fixture["vectors"].as_array().unwrap() {
            let seed = v["seed"].as_str().unwrap().parse().unwrap();
            let number = v["collision_number"].as_u64().unwrap();
            assert_eq!(
                backoff_slots(seed, v["transfer_id"].as_str().unwrap(), number),
                v["slots"].as_u64().unwrap()
            );
        }
    }
    #[test]
    fn sixteen_collisions_drop_current_without_sixteenth_draw_or_seventeenth_attempt() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/verification/fixtures/ethernet-media/collision.ini");
        let prepared = crate::input::prepare(&path).unwrap();
        let mut engine = initialize(&prepared).unwrap();
        engine.media.as_mut().unwrap().injected_slots = Some(0);
        let snapshot = engine.run();
        assert!(!snapshot.common.partial);
        let eth = snapshot.ethernet.unwrap();
        assert_eq!(eth.attempts.len(), 32);
        for t in &eth.transfers {
            assert_eq!(t.media.as_ref().unwrap().attempt_count, 16);
            assert_eq!(t.media.as_ref().unwrap().collision_count, 16);
            assert_eq!(t.status, "dropped");
            assert_eq!(t.drop_reason.as_deref(), Some("attempt_limit"));
        }
        for a in eth.attempts.iter().filter(|a| a.number == 16) {
            assert_eq!(a.sof_ps, 30_300_000);
            assert_eq!(a.jam_end_ps, Some(31_260_000));
            assert!(a.backoff_slots.is_none());
            assert!(a.backoff_until_ps.is_none());
        }
        assert!(eth.receptions.is_empty());
        assert_eq!(snapshot.common.pending_events, 0);
        assert_eq!(
            snapshot
                .common
                .points
                .iter()
                .filter(|p| p.metric == "ethernet.media.backoff_slots")
                .count(),
            30
        );
    }
    #[test]
    fn cancellation_preserves_unrelated_same_time_logical_changes() {
        let mut pair = Pair {
            macs: [Mac::default(), Mac::default()],
            timeline: BTreeMap::new(),
            reservation: None,
            generation: 0,
        };
        pair.change(100, Change::Eof(0, 7));
        pair.change(100, Change::Release(1, 8));
        pair.change(100, Change::Ifg(1, 3));
        pair.change(101, Change::SignalOff(1, 7));
        pair.cancel_attempt(7);
        assert_eq!(pair.timeline.len(), 1);
        assert!(matches!(
            pair.timeline[&100].as_slice(),
            [Change::Release(1, 8), Change::Ifg(1, 3)]
        ));
    }
}
