//! Adapters of the existing WireHost FIFO to the two shared MAC serializers.
use super::*;

impl NetworkState {
    pub(super) fn gptp_merge_source(
        &self,
        t: usize,
        id: u64,
        epoch: u64,
        d: &NetworkDelta,
    ) -> Result<gptp::state::ExpressSourceEvidence> {
        let source = d
            .mac_merge
            .as_ref()
            .or(self.mac_merge.as_ref())
            .and_then(|s| s.express_sources.get(&t))
            .ok_or_else(|| error("retained Merge EXPRESS source missing"))?;
        let row = self.transfer(d, t);
        if source.source
            != (gptp::state::MacSource::MergeExpress {
                transfer: t,
                direction: row.direction,
                packet_id: id,
                epoch,
            })
            || row.row.sof_ps != Some(source.sof_ps)
            || row.row.wire.mac_hex != source.mac_hex
            || source
                .eof_actual_ps
                .is_some_and(|at| row.row.eof_ps != Some(at))
        {
            return Err(error("retained Merge EXPRESS source identity differs"));
        }
        if let Some(p) = d
            .mac_merge
            .as_ref()
            .or(self.mac_merge.as_ref())
            .and_then(|s| s.packets.get(&id))
        {
            if p.origin != mac_merge::integration::Origin::Express(t)
                || p.epoch != epoch
                || p.direction != row.direction
                || p.sof != source.sof_ps
            {
                return Err(error("live Merge EXPRESS source differs"));
            }
        }
        Ok(source.clone())
    }
    pub(super) fn gptp_abstract_source(
        &self,
        t: usize,
        d: &NetworkDelta,
    ) -> Result<gptp::state::ExpressSourceEvidence> {
        let source = d
            .preemption
            .as_ref()
            .or(self.preemption.as_ref())
            .and_then(|s| s.express_sources.get(&t))
            .ok_or_else(|| error("retained abstract EXPRESS source missing"))?;
        let row = self.transfer(d, t);
        if !matches!(source.source,gptp::state::MacSource::AbstractExpress{transfer,direction,..} if transfer==t&&direction==row.direction)
            || self.preemptable(row)
            || row.row.sof_ps != Some(source.sof_ps)
            || row.row.wire.mac_hex != source.mac_hex
            || source
                .eof_actual_ps
                .is_some_and(|at| row.row.eof_ps != Some(at))
        {
            return Err(error("retained abstract EXPRESS source identity differs"));
        }
        Ok(source.clone())
    }
    pub(super) fn validate_producer_express_sources(&self, horizon: u64) -> Result<()> {
        let d = self.delta();
        let merge = self.mac_merge.as_ref().map(|s| &s.express_sources);
        let abstract_sources = self.preemption.as_ref().map(|s| &s.express_sources);
        for sources in [merge, abstract_sources].into_iter().flatten() {
            if sources.len()
                > self
                    .prepared
                    .gptp
                    .as_ref()
                    .map_or(0, |g| g.max_journal_entries)
            {
                return Err(error("producer EXPRESS source history capacity"));
            }
            for (&t, source) in sources {
                if !self.gptp_packet(&d, t)
                    || source.sof_ps > horizon
                    || source
                        .eof_actual_ps
                        .is_some_and(|at| at > horizon || at != source.eof_planned_ps)
                    || source
                        .arrival_actual_ps
                        .is_some_and(|at| at > horizon || source.eof_actual_ps.is_none())
                {
                    return Err(error("producer EXPRESS source history causal cut differs"));
                }
                let expected = match source.source {
                    gptp::state::MacSource::MergeExpress {
                        packet_id, epoch, ..
                    } => self.gptp_merge_source(t, packet_id, epoch, &d)?,
                    gptp::state::MacSource::AbstractExpress { .. } => {
                        self.gptp_abstract_source(t, &d)?
                    }
                    _ => return Err(error("producer EXPRESS source history kind differs")),
                };
                if &expected != source {
                    return Err(error("producer EXPRESS source history owner differs"));
                }
            }
        }
        Ok(())
    }
    /// Read-only oldest eligible Wire candidate. Ownership is claimed at start.
    pub(super) fn wire_candidate(
        &self,
        direction: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<Option<String>> {
        let Some(host) = self.wire_host(d) else {
            return Ok(None);
        };
        if host.active[direction].is_some() {
            return Ok(None);
        }
        for id in &host.queues[direction] {
            if self.wire_transport_allowed(direction, id, d)?
                && self.wire_start_allowed(direction, id, d, now)?
            {
                return Ok(Some(id.clone()));
            }
        }
        Ok(None)
    }
    /// Shared tree-policy filter; a relay cannot escape its original tree.
    pub(super) fn wire_transport_allowed(
        &self,
        direction: usize,
        id: &str,
        d: &NetworkDelta,
    ) -> Result<bool> {
        use wire::ControlTransport;
        let link = &self.prepared.ethernet.directions[direction];
        if !self
            .policy(d)
            .and_then(|p| p.link_up.get(&link.from_port).copied())
            .unwrap_or(true)
        {
            return Ok(false);
        }
        let tx = &self.wire_host(d).unwrap().tx[id];
        let Some(ControlTransport::RelayedService {
            component,
            ingress,
            egress,
            vid,
            tree,
        }) = &tx.transport
        else {
            return Ok(true);
        };
        let topology = self
            .prepared
            .provider_l2
            .as_ref()
            .ok_or_else(|| error("relayed service topology missing"))?;
        Ok(topology
            .ports
            .get(ingress)
            .is_some_and(|p| p.component == *component && p.member_vids.contains(vid))
            && topology
                .ports
                .get(egress)
                .is_some_and(|p| p.component == *component && p.member_vids.contains(vid))
            && *egress == link.from_port
            && self
                .prepared
                .wire
                .as_ref()
                .unwrap()
                .adapters
                .iter()
                .find(|a| {
                    a.component_binding
                        .as_ref()
                        .is_some_and(|b| b.component == *component)
                })
                .is_some_and(|a| match &a.options {
                    crate::types::wire::WireOptions::Mstp(m) => m.vlan_map[*vid as usize] == *tree,
                    _ => *tree == 0,
                })
            && self
                .provider_tree_policy(egress, *vid, d)
                .is_some_and(|p| p.forwarding && p.link_up))
    }
    pub(super) fn wire_prune_candidates(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let ids = self
            .wire_host(d)
            .map(|h| h.queues[direction].iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let up = self
            .policy(d)
            .and_then(|p| {
                p.link_up
                    .get(&self.prepared.ethernet.directions[direction].from_port)
                    .copied()
            })
            .unwrap_or(true);
        for id in ids {
            // Preserve the original oldest-eligible scan: blocked entries stay
            // queued, and a valid selected head stops policy pruning behind it.
            if up && !self.wire_start_allowed(direction, &id, d, c.now())? {
                continue;
            }
            if self.wire_transport_allowed(direction, &id, d)? {
                break;
            }
            self.wire_mut(d)?.queues[direction].retain(|s| s != &id);
            self.wire_mut(d)?.tx.get_mut(&id).unwrap().dropped = true;
            self.wire_tx_record(
                direction,
                &id,
                "tx_completed",
                Some(if up {
                    "service_tree_changed"
                } else {
                    "link_down"
                }),
                d,
                c,
            )?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // One atomic claim carries all serializer boundaries and transaction context.
    pub(super) fn wire_claim(
        &self,
        direction: usize,
        id: &str,
        eof: u64,
        release: u64,
        arrival: Option<u64>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self.wire_candidate(direction, d, c.now())?.as_deref() != Some(id) {
            return Err(error("Wire serializer claim differs from eligible FIFO"));
        }
        crate::allocation::reservation_checkpoint("network_wire_serializer_claim")?;
        let h = self.wire_mut(d)?;
        h.queues[direction].retain(|s| s != id);
        h.active[direction] = Some(id.to_owned());
        let tx = h.tx.get_mut(id).unwrap();
        tx.sof = Some(c.now());
        tx.planned_eof = Some(eof);
        tx.planned_release = Some(release);
        tx.planned_arrival = arrival;
        self.wire_tx_record(direction, id, "tx_started", None, d, c)
    }
    pub(super) fn wire_owner_eof(
        &self,
        direction: usize,
        id: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let h = self.wire_mut(d)?;
        if h.active[direction].as_deref() != Some(id) {
            return if h.tx.get(id).is_some_and(|tx| tx.dropped) {
                Ok(())
            } else {
                Err(error("Wire EOF active owner differs"))
            };
        }
        let tx =
            h.tx.get_mut(id)
                .ok_or_else(|| error("Wire EOF missing source"))?;
        if tx.dropped || tx.eof.is_some() {
            return Ok(());
        }
        tx.eof = Some(c.now());
        tx.planned_eof = Some(c.now());
        self.pause_mac_completed(direction, d, c)?;
        self.wire_tx_record(direction, id, "tx_completed", None, d, c)
    }
    pub(super) fn wire_owner_release(
        &self,
        direction: usize,
        id: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let h = self.wire_mut(d)?;
        if h.active[direction].as_deref() != Some(id) {
            return if h.tx.get(id).is_some_and(|tx| tx.dropped) {
                Ok(())
            } else {
                Err(error("Wire release active owner differs"))
            };
        }
        h.tx.get_mut(id).unwrap().release = Some(c.now());
        h.active[direction] = None;
        Ok(())
    }
    pub(super) fn wire_owner_receive(
        &self,
        direction: usize,
        id: &str,
        bytes: &[u8],
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let tx = self
            .wire_host(d)
            .and_then(|h| h.tx.get(id))
            .ok_or_else(|| error("Wire ingress source missing"))?
            .clone();
        if tx.dropped || tx.arrival.is_some() {
            return Ok(());
        }
        if tx.eof.is_none() || tx.intent.bytes != bytes {
            return self.wire_owner_fail(direction, id, "mac_merge_receive_error", d, c);
        }
        self.wire_mut(d)?.tx.get_mut(id).unwrap().arrival = Some(c.now());
        let to = self.prepared.ethernet.directions[direction].to_port.clone();
        self.wire_deliver(direction, id, &tx, vec![(None, to)], d, c)
    }
    pub(super) fn wire_owner_fail(
        &self,
        direction: usize,
        id: &str,
        reason: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let h = self.wire_mut(d)?;
        let tx =
            h.tx.get_mut(id)
                .ok_or_else(|| error("Wire failure source missing"))?;
        if tx.dropped {
            return Ok(());
        }
        let completed_tx = tx.eof.is_some();
        tx.dropped = true;
        if tx.eof.is_none() && h.active[direction].as_deref() == Some(id) {
            h.active[direction] = None
        }
        let reason = match reason {
            "mac_merge_receive_error" | "mac_merge_packet_lost" | "link_epoch_reset" => reason,
            _ => "mac_merge_transport_failure",
        };
        self.wire_tx_record(
            direction,
            id,
            if completed_tx {
                "rx_failed"
            } else {
                "tx_completed"
            },
            Some(reason),
            d,
            c,
        )
    }
    pub(super) fn wire_owner_reset(
        &self,
        direction: usize,
        id: &str,
        reason: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self
            .wire_host(d)
            .and_then(|h| h.tx.get(id))
            .is_none_or(|tx| tx.arrival.is_none())
        {
            self.wire_owner_fail(direction, id, reason, d, c)?;
        }
        let host = self.wire_mut(d)?;
        // A real reset retires physical occupation even after MAC EOF;
        // it is not a completed nominal IPG or a synthetic receive.
        host.tx.get_mut(id).unwrap().release.get_or_insert(c.now());
        if host.active[direction].as_deref() == Some(id) {
            host.active[direction] = None;
        }
        Ok(())
    }
    pub(super) fn validate_wire_merge_source(
        &self,
        p: &mac_merge::integration::Packet,
        horizon: u64,
    ) -> Result<()> {
        let id = p
            .wire_control_id
            .as_ref()
            .ok_or_else(|| error("Wire Merge checkpoint identity missing"))?;
        let h = self
            .wire
            .as_ref()
            .ok_or_else(|| error("Wire Merge checkpoint host missing"))?;
        let tx =
            h.tx.get(id)
                .ok_or_else(|| error("Wire Merge checkpoint source missing"))?;
        if p.origin != mac_merge::integration::Origin::WireControl
            || tx.intent.port != self.prepared.ethernet.directions[p.direction].from_port
            || tx.queued_ps.is_none_or(|at| at > p.sof)
            || tx.sof != Some(p.sof)
            || p.sof > horizon
            || p.plan.is_some()
            || p.token.is_some()
            || mac_merge::encode_express(&tx.intent.bytes)
                .map_err(|e| error(format!("MAC Merge: {e:?}")))?
                != p.bytes
        {
            return Err(error(
                "Wire Merge checkpoint source/bytes/lifecycle differs",
            ));
        }
        Ok(())
    }
    pub(super) fn wire_packet_class(
        &self,
        p: &mac_merge::integration::Packet,
        d: &NetworkDelta,
    ) -> Option<u8> {
        match p.origin {
            mac_merge::integration::Origin::Pmac(t)
            | mac_merge::integration::Origin::Express(t) => Some(self.transfer(d, t).row.priority),
            mac_merge::integration::Origin::Lldp => Some(self.merge_lldp_priority(p.direction)),
            mac_merge::integration::Origin::WireControl => {
                p.wire_control_id.as_ref().and_then(|id| {
                    self.wire_host(d)
                        .and_then(|h| h.tx.get(id))
                        .filter(|t| !t.is_mac_control())
                        .map(|_| self.wire_priority(p.direction, id, d))
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wire_serializer_reservation_failure_is_atomic_and_retry_has_no_stale_owner() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/network-composite/wire-serializers/merge.ini");
        let p = crate::prepare(&source).unwrap();
        crate::allocation::fail_next_reservation("network_wire_serializer_claim");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        assert!(
            failed
                .common
                .diagnostics
                .iter()
                .any(|d| d.message.contains("memory allocation failed"))
        );
        let rows = &failed.registered.as_ref().unwrap().model_records;
        assert!(
            !rows
                .values()
                .any(|r| r.schema.name == "dir.mac_merge.packet"
                    && r.data["wire_control_id"].is_string())
        );
        let retry = crate::runtime::simulate(&p).unwrap();
        assert_eq!(retry.common.termination, "time_limit");
        assert!(
            retry
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "dir.mac_merge.packet"
                    && r.data["wire_control_id"].is_string()
                    && r.data["arrival_ps"].is_string())
        );
    }
    #[test]
    #[ignore = "actual sampled startup and post-PHY-admission transactional reservation rollback"]
    fn raw_wire_post_admission_failure_rolls_back_phy_fifo_ids_timers_and_retries() {
        use super::*;
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/wire-serializers/sampled.ini"),
        )
        .unwrap();
        let mut engine = crate::runtime::registered::Engine::new(Arc::new(p.clone())).unwrap();
        engine.initialize().unwrap();
        let mut failed_id = None;
        while failed_id.is_none() {
            assert!(engine.step());
            let now = engine.snapshot.common.last_event_time_ps.unwrap_or(0);
            let before_fes = serde_json::to_value(
                engine
                    .checkpoint_work()
                    .iter()
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let before_sequence = engine.sequence;
            let before_live = engine.live.clone();
            failed_id = engine
                .with_network_state_for_test(|state| {
                    let i = state
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|p| p.from_port == "Net.s1.tx_b")
                        .unwrap();
                    if now < 6_002_000_000
                        || state.mac_merge.as_ref().unwrap().ports[i].busy.is_some()
                    {
                        return None;
                    }
                    let medium = &state.phy[state.phy_attachment_for(i).unwrap()];
                    let status = medium.raw_ingress_status("Net.s1.tx_b").unwrap();
                    if !status.ready
                        || status.next_cut_sample != medium.next_sample_index()
                        || medium.sample_time_ps(status.next_cut_sample).unwrap() != now
                    {
                        return None;
                    }
                    let mut delta = state.delta();
                    let id = state
                        .wire_host(&delta)
                        .and_then(|h| h.queues[i].front())
                        .cloned()?;
                    let before = state.checkpoint();
                    let mut batch = crate::registry::EffectBatch::default();
                    let mut next = 0;
                    let channels = BTreeMap::new();
                    let mut context = Context {
                        now,
                        config: &engine.registered.models[0],
                        registry: &engine.registered.registry,
                        profile: engine
                            .registered
                            .registry
                            .profile(&p.common.profile)
                            .unwrap(),
                        channels: &channels,
                        channel_configs: &[],
                        live_timers: &engine.live,
                        executing_timer: None,
                        next_id: &mut next,
                        batch: &mut batch,
                    };
                    crate::allocation::fail_next_reservation("network_wire_serializer_claim");
                    let result = state.merge_arbitrate(i, &mut delta, &mut context);
                    if result.is_ok() {
                        crate::allocation::fail_next_reservation("unused_wire_test_checkpoint");
                        return None;
                    }
                    let error = result.unwrap_err();
                    assert_eq!(error.reason, "allocation_failed", "{error:?}");
                    assert!(
                        delta.phy.is_some(),
                        "failure must follow actual raw physical admission"
                    );
                    assert!(
                        delta.mac_merge.as_ref().unwrap().next_packet
                            > state.mac_merge.as_ref().unwrap().next_packet
                    );
                    assert_eq!(
                        state.checkpoint(),
                        before,
                        "committed PHY/FIFO/packet IDs/timers/records changed"
                    );
                    Some(id)
                })
                .flatten();
            assert_eq!(engine.sequence, before_sequence);
            assert_eq!(engine.live, before_live);
            assert_eq!(
                serde_json::to_value(
                    engine
                        .checkpoint_work()
                        .iter()
                        .map(|(key, _)| *key)
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                before_fes
            );
        }
        while engine.step() {}
        assert_eq!(engine.snapshot.common.termination, "time_limit");
        let id = failed_id.unwrap();
        assert!(
            engine
                .snapshot
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "dir.mac_merge.packet"
                    && r.data["wire_control_id"] == id
                    && r.data["arrival_ps"].is_string()),
            "same unclaimed FIFO work must retry and receive exactly once"
        );
    }
    #[test]
    fn native_raw_restore_requires_authenticated_engine_prefix() {
        use super::*;
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/wire-serializers/sampled.ini"),
        )
        .unwrap();
        let network = p.registered.as_ref().unwrap().network.clone().unwrap();
        let state = NetworkState::new(network.clone()).unwrap();
        let before = state.checkpoint();
        let error = NetworkState::restore_checkpoint(network, &before, 0)
            .err()
            .expect("native raw replay must never trust ideal receive proof");
        assert!(
            error
                .message
                .contains("raw Merge restore requires exact frozen-input Engine prefix")
        );
        assert_eq!(state.checkpoint(), before);
    }
}
