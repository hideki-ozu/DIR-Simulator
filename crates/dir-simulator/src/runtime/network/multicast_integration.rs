//! Active multicast actor scheduling inside the network owner's control ledger.
use super::*;
use multicast_active::{Effects, Event as ActiveEvent};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MulticastAudit {
    pub at_ps: u64,
    pub event: ActiveEvent,
    pub receipt_id: Option<String>,
}

impl NetworkState {
    pub(super) fn validate_multicast_history(&self, horizon: u64) -> Result<()> {
        let config = self
            .prepared
            .multicast_active
            .as_ref()
            .ok_or_else(|| error("active multicast config missing"))?;
        let prepared_wire = self
            .prepared
            .wire
            .as_ref()
            .ok_or_else(|| error("active multicast wire missing"))?;
        let wire = self
            .wire
            .as_ref()
            .ok_or_else(|| error("active multicast wire state missing"))?;
        let mut actor = multicast_active::Actor::new(config);
        let mut wake = None;
        let mut change_cursor = 0;
        let mut last_at = 0;
        let mut seen_start = false;
        let mut reserved_timer = None;
        let mut receipts = std::collections::BTreeSet::new();
        for entry in &self.multicast_history {
            if entry.at_ps < last_at || entry.at_ps > horizon {
                return Err(error("active multicast checkpoint event chronology"));
            }
            last_at = entry.at_ps;
            if !matches!(entry.event, ActiveEvent::Timer) && wake == Some(entry.at_ps) {
                reserved_timer = wake;
            }
            match &entry.event {
                ActiveEvent::Start => {
                    if seen_start
                        || !self
                            .multicast_history
                            .first()
                            .is_some_and(|first| std::ptr::eq(first, entry))
                        || entry.at_ps != 0
                        || entry.receipt_id.is_some()
                    {
                        return Err(error("active multicast checkpoint start provenance"));
                    }
                    seen_start = true;
                }
                ActiveEvent::Change(index) => {
                    if !seen_start
                        || *index != change_cursor
                        || config
                            .changes
                            .get(*index)
                            .is_none_or(|change| change.at_ps != entry.at_ps)
                        || entry.receipt_id.is_some()
                    {
                        return Err(error("active multicast checkpoint change provenance"));
                    }
                    change_cursor += 1;
                }
                ActiveEvent::Timer => {
                    if !seen_start
                        || (wake != Some(entry.at_ps) && reserved_timer != Some(entry.at_ps))
                        || entry.receipt_id.is_some()
                    {
                        return Err(error("active multicast checkpoint timer provenance"));
                    }
                    reserved_timer = None;
                }
                ActiveEvent::Receive {
                    ingress_port,
                    vid,
                    bytes,
                } => {
                    let id = entry.receipt_id.as_ref().ok_or_else(|| {
                        error("active multicast checkpoint receive lacks packet ID")
                    })?;
                    if !seen_start || !receipts.insert(id.clone()) {
                        return Err(error(
                            "active multicast checkpoint duplicate/unstarted receive",
                        ));
                    }
                    if let Some(packet) = prepared_wire.packets.iter().find(|p| &p.id == id) {
                        if packet.arrival_ps != entry.at_ps
                            || packet.ingress_port != *ingress_port
                            || packet.vid != Some(*vid)
                            || packet.bytes != *bytes
                            || prepared_wire.adapter(&packet.adapter_id).is_none_or(|a| {
                                !matches!(
                                    a.protocol,
                                    crate::types::wire::WireProtocol::Igmp
                                        | crate::types::wire::WireProtocol::Mld
                                )
                            })
                        {
                            return Err(error(
                                "active multicast checkpoint external packet differs",
                            ));
                        }
                    } else {
                        let base = id.strip_suffix(&format!("@{ingress_port}")).unwrap_or(id);
                        let tx = wire.tx.get(base).ok_or_else(|| {
                            error("active multicast checkpoint receive TX missing")
                        })?;
                        let direction = self
                            .prepared
                            .ethernet
                            .directions
                            .iter()
                            .find(|d| d.from_port == tx.intent.port)
                            .ok_or_else(|| {
                                error("active multicast checkpoint TX output missing")
                            })?;
                        if tx.arrival != Some(entry.at_ps)
                            || tx.dropped
                            || tx.vid != Some(*vid)
                            || tx.payload != *bytes
                            || direction.to_port != *ingress_port
                            || !matches!(
                                tx.intent.protocol,
                                crate::types::wire::WireProtocol::Igmp
                                    | crate::types::wire::WireProtocol::Mld
                            )
                        {
                            return Err(error(
                                "active multicast checkpoint receive TX causality differs",
                            ));
                        }
                    }
                }
            }
            wake = actor
                .step(config, entry.at_ps, entry.event.clone())
                .map_err(error)?
                .next_deadline;
        }
        for packet in &prepared_wire.packets {
            if packet.arrival_ps >= horizon || packet.vid.is_none() {
                continue;
            }
            let Some(adapter) = prepared_wire.adapter(&packet.adapter_id) else {
                continue;
            };
            if !matches!(
                adapter.protocol,
                crate::types::wire::WireProtocol::Igmp | crate::types::wire::WireProtocol::Mld
            ) {
                continue;
            }
            let ipv6 = packet.bytes.first().is_some_and(|b| *b >> 4 == 6);
            if crate::runtime::wire::membership::decode(&packet.bytes, ipv6, 4096).is_ok()
                && !receipts.contains(&packet.id)
            {
                return Err(error(
                    "active multicast checkpoint missing external receive",
                ));
            }
        }
        if self.prepared.shared_lans.is_empty() {
            for (id, tx) in &wire.tx {
                let Some(at) = tx.arrival else {
                    continue;
                };
                if at >= horizon
                    || tx.vid.is_none()
                    || !matches!(
                        tx.intent.protocol,
                        crate::types::wire::WireProtocol::Igmp
                            | crate::types::wire::WireProtocol::Mld
                    )
                {
                    continue;
                }
                let Some(direction) = self
                    .prepared
                    .ethernet
                    .directions
                    .iter()
                    .find(|d| d.from_port == tx.intent.port)
                else {
                    continue;
                };
                let ipv6 = tx.payload.first().is_some_and(|b| *b >> 4 == 6);
                if prepared_wire
                    .owner(tx.intent.protocol, &direction.to_port)
                    .is_some()
                    && crate::runtime::wire::membership::decode(&tx.payload, ipv6, 4096).is_ok()
                    && !receipts.contains(id)
                {
                    return Err(error(
                        "active multicast checkpoint missing TX-derived receive",
                    ));
                }
            }
        }
        if change_cursor != self.multicast_cursor
            || wake != self.multicast_wake
            || serde_json::to_value(&actor).map_err(|e| error(e.to_string()))?
                != serde_json::to_value(self.multicast_active.as_ref().unwrap())
                    .map_err(|e| error(e.to_string()))?
        {
            return Err(error(
                "active multicast checkpoint actor differs from causal history",
            ));
        }
        Ok(())
    }
    fn active_step(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        event: ActiveEvent,
        receipt_id: Option<String>,
    ) -> Result<Effects> {
        let config = self
            .prepared
            .multicast_active
            .as_ref()
            .ok_or_else(|| error("active multicast config missing"))?;
        let effects = self
            .active_mut(d)?
            .step(config, c.now(), event.clone())
            .map_err(error)?;
        d.multicast_history
            .get_or_insert_with(|| self.multicast_history.clone())
            .push(MulticastAudit {
                at_ps: c.now(),
                event,
                receipt_id,
            });
        Ok(effects)
    }
    fn active_mut<'a>(&self, d: &'a mut NetworkDelta) -> Result<&'a mut multicast_active::Actor> {
        if d.multicast_active.is_none() {
            d.multicast_active = self.multicast_active.clone();
        }
        d.multicast_active
            .as_mut()
            .ok_or_else(|| error("active multicast state missing"))
    }

    pub(super) fn initialize_multicast(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self.prepared.multicast_active.is_none() {
            return Ok(());
        }
        let effects = self.active_step(d, c, ActiveEvent::Start, None)?;
        self.active_effects(effects, d, c)?;
        self.multicast_changes(d, c)?;
        Ok(())
    }

    fn multicast_changes(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let Some(config) = self.prepared.multicast_active.as_ref() else {
            return Ok(());
        };
        let mut cursor = d.multicast_cursor.unwrap_or(self.multicast_cursor);
        while config
            .changes
            .get(cursor)
            .is_some_and(|change| change.at_ps == c.now())
        {
            let effects = self.active_step(d, c, ActiveEvent::Change(cursor), None)?;
            cursor += 1;
            d.multicast_cursor = Some(cursor);
            self.active_effects(effects, d, c)?;
        }
        Ok(())
    }

    pub(super) fn multicast_timer(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if d.multicast_wake.unwrap_or(self.multicast_wake) != Some(c.now()) {
            return Err(error("multicast control wake differs"));
        }
        d.multicast_wake = Some(None);
        self.multicast_changes(d, c)?;
        let effects = self.active_step(d, c, ActiveEvent::Timer, None)?;
        self.active_effects(effects, d, c)
    }

    pub(super) fn multicast_receive(
        &self,
        ingress_port: &str,
        vid: u16,
        bytes: Vec<u8>,
        receipt_id: String,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self.prepared.multicast_active.is_none() {
            return Ok(());
        }
        let effects = self.active_step(
            d,
            c,
            ActiveEvent::Receive {
                ingress_port: ingress_port.to_owned(),
                vid,
                bytes,
            },
            Some(receipt_id),
        )?;
        self.active_effects(effects, d, c)
    }

    fn active_effects(
        &self,
        effects: Effects,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        for tx in effects.tx {
            self.active_wire_tx(tx, d, c)?;
        }
        let current = d.multicast_wake.unwrap_or(self.multicast_wake);
        if current != effects.next_deadline {
            self.due_mut(d)
                .retain(|row| !matches!(row.kind, Completion::MulticastTimer));
            if let Some(at) = effects.next_deadline {
                if at <= c.now() {
                    return Err(error("active multicast deadline did not advance"));
                }
                self.due_mut(d).push(Due {
                    at,
                    kind: Completion::MulticastTimer,
                });
            }
            d.multicast_wake = Some(effects.next_deadline);
        }
        Ok(())
    }
}
