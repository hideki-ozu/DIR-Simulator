//! IEEE 802.3 Annex 31B PAUSE: actual receive, synchronous MAC completion, physical-rate timer.
use super::wire::{ControlTransport, ControlTx};
use super::*;
use crate::runtime::wire::{WireTxIntent, codec::crc32};
use crate::types::network::mac_control::Action;
use crate::types::wire::WireProtocol;
pub(super) const PAUSE_DA: [u8; 6] = [1, 0x80, 0xc2, 0, 0, 1];
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Request {
    pub quanta: u16,
    pub received_ps: u64,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Timer {
    pub generation: u64,
    pub loaded_ps: u64,
    pub expires_ps: u64,
    pub request: Request,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Port {
    pub rx_enabled: bool,
    pub tx_enabled: bool,
    pub transmit_enabled: bool,
    pub link_up: bool,
    pub generation: u64,
    pub epoch: u64,
    pub ordinal: u64,
    pub pending: Option<Request>,
    pub timer: Option<Timer>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct State {
    pub ports: Vec<Option<Port>>,
}
impl State {
    pub fn new(p: &PreparedNetwork) -> Self {
        Self {
            ports: p
                .mac_control
                .as_ref()
                .map(|cfg| {
                    cfg.ports
                        .iter()
                        .map(|port| {
                            port.as_ref().map(|p| Port {
                                rx_enabled: p.rx_enabled,
                                tx_enabled: p.tx_enabled,
                                transmit_enabled: true,
                                link_up: true,
                                generation: 0,
                                epoch: 0,
                                ordinal: 0,
                                pending: None,
                                timer: None,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}
#[cfg(test)]
pub(super) fn duration(quanta: u16, rate: u64) -> Result<u64> {
    duration_ratio(quanta, rate, 1)
}
fn duration_ratio(quanta: u16, rate: u64, den: u64) -> Result<u64> {
    if rate == 0 {
        return Err(error("PAUSE needs positive physical MAC rate"));
    }
    let n = u128::from(quanta)
        .checked_mul(512)
        .and_then(|n| n.checked_mul(u128::from(den)))
        .and_then(|n| n.checked_mul(1_000_000_000_000))
        .ok_or_else(|| error("PAUSE duration overflow"))?;
    u64::try_from(n.div_ceil(u128::from(rate))).map_err(|_| error("PAUSE duration overflow"))
}
pub(super) fn frame(source: [u8; 6], quanta: u16) -> Vec<u8> {
    let mut b = Vec::with_capacity(64);
    b.extend(PAUSE_DA);
    b.extend(source);
    b.extend([0x88, 0x08, 0, 1]);
    b.extend(quanta.to_be_bytes());
    b.resize(60, 0);
    b.extend(crc32(&b).to_le_bytes());
    b
}
/// All 0x8808 frames are consumed locally; malformed, unsupported and oversized frames are discarded.
pub(super) fn parse(
    bytes: &[u8],
    station: [u8; 6],
) -> std::result::Result<Option<u16>, &'static str> {
    if bytes.len() < 14 || bytes[12..14] != [0x88, 8] {
        return Ok(None);
    }
    if bytes.len() != 64 {
        return Err(if bytes.len() > 64 {
            "oversized_control"
        } else {
            "truncated_control"
        });
    }
    if crc32(&bytes[..60]).to_le_bytes() != bytes[60..] {
        return Err("bad_fcs");
    }
    if bytes[14..16] != [0, 1] {
        return Err("unsupported_opcode");
    }
    if bytes[..6] != PAUSE_DA && bytes[..6] != station {
        return Err("wrong_destination");
    }
    Ok(Some(u16::from_be_bytes([bytes[16], bytes[17]])))
}
impl NetworkState {
    pub(super) fn pause_physical_rate(&self, direction: usize) -> (u64, u64) {
        if let Some(i) = self.phy_attachment_for(direction) {
            let medium = &self.prepared.phy_attachments[i].config.medium;
            if medium.physical_mode
                == crate::runtime::ethernet::phy::medium::PhysicalMode::ProjectNumerical
            {
                return (medium.channel.grid.symbol_rate_hz, 2);
            }
        }
        (self.prepared.ethernet.directions[direction].bitrate_bps, 1)
    }
    pub(super) fn pause_duration(&self, direction: usize, quanta: u16) -> Result<u64> {
        let (n, d) = self.pause_physical_rate(direction);
        duration_ratio(quanta, n, d)
    }
    fn pause_state<'a>(&'a self, d: &'a NetworkDelta) -> &'a State {
        d.mac_control.as_ref().unwrap_or(&self.mac_control)
    }
    fn pause_mut<'a>(&self, d: &'a mut NetworkDelta) -> &'a mut State {
        d.mac_control
            .get_or_insert_with(|| self.mac_control.clone())
    }
    pub(super) fn pause_port<'a>(
        &'a self,
        direction: usize,
        d: &'a NetworkDelta,
    ) -> Option<&'a Port> {
        self.pause_state(d).ports.get(direction)?.as_ref()
    }
    pub(super) fn output_request_allowed(
        &self,
        direction: usize,
        mac_control: bool,
        d: &NetworkDelta,
        now: u64,
    ) -> bool {
        let up = self
            .policy(d)
            .and_then(|p| {
                p.link_up
                    .get(&self.prepared.ethernet.directions[direction].from_port)
                    .copied()
            })
            .unwrap_or(true);
        up && self.pause_port(direction, d).is_none_or(|p| {
            p.transmit_enabled
                && p.link_up
                && (mac_control || p.timer.as_ref().is_none_or(|t| t.expires_ps <= now))
        })
    }
    pub(super) fn pause_request_in_progress(&self, direction: usize, d: &NetworkDelta) -> bool {
        if self.configured_endpoint(direction).is_some() {
            return self.media_state(d).requests[direction];
        }
        if self.merge_direction(direction) {
            let state = d.mac_merge.as_ref().or(self.mac_merge.as_ref()).unwrap();
            let p = &state.ports[direction];
            if [p.selected_pmac, p.selected_emac]
                .into_iter()
                .flatten()
                .any(|t| {
                    self.transfer(d, t).row.sof_ps.is_some()
                        && self.transfer(d, t).row.eof_ps.is_none()
                })
            {
                return true;
            }
        }
        if self.preemption_direction(direction) {
            let p = self.preemption_port(d, direction);
            if p.suspended.is_some() {
                return true;
            }
        }
        if let Some(id) = self.wire_host(d).and_then(|h| h.active[direction].as_ref()) {
            let tx = &self.wire_host(d).unwrap().tx[id];
            if tx.eof.is_none() {
                return true;
            }
        }
        if let Some(t) = self.queue(d, direction).active {
            if self.phy_attachment_for(direction).is_some()
                && self.phy_service_view(direction, d).segment
                    == crate::runtime::ethernet::tsn::merge::Segment::SampledPhysicalTail
            {
                return false;
            }
            let row = &self.transfer(d, t).row;
            if row.eof_ps.is_none() {
                return row.sof_ps.is_some() || self.phy_attachment_for(direction).is_some();
            }
        }
        false
    }
    fn pause_refresh_permission(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.merge_permission_changed(direction, d)?;
        if self.phy_attachment_for(direction).is_none() {
            return Ok(());
        }
        if let Some(tsn) = &self.tsn {
            // A phase0 schedule replacement settles and refreshes all permissions below it.
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
        self.phy_set_start_permission(direction, d, c, true)
    }
    pub(super) fn initialize_pause(&self, d: &mut NetworkDelta) -> Result<()> {
        if let Some(cfg) = &self.prepared.mac_control {
            for (i, control) in cfg.controls.iter().enumerate() {
                self.due_mut(d).push(Due {
                    at: control.at_ps,
                    kind: Completion::PauseAdmin(i),
                });
            }
        }
        Ok(())
    }
    fn pause_record(
        &self,
        direction: usize,
        kind: &str,
        extra: Value,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let port = self.prepared.ethernet.directions[direction]
            .from_port
            .clone();
        Self::record(
            c,
            d,
            "dir.network.mac_control",
            format!("pause:{port}:{}", d.record_sequence),
            json!({"port":port,"kind":kind,"state":decimal(json!(self.pause_port(direction,d))),"evidence":extra}),
        )
    }
    pub(super) fn pause_receive(
        &self,
        direction: usize,
        bytes: &[u8],
        evidence: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<bool> {
        if bytes.len() < 14 || bytes[12..14] != [0x88, 8] {
            return Ok(false);
        }
        let config = self
            .prepared
            .mac_control
            .as_ref()
            .and_then(|cfg| cfg.ports[direction].as_ref());
        let Some(config) = config else {
            return Ok(true);
        };
        let parsed = parse(bytes, config.station_mac);
        let enabled = self
            .pause_port(direction, d)
            .is_some_and(|p| p.rx_enabled && p.link_up);
        let quanta = match parsed {
            Ok(Some(q)) if enabled => q,
            Ok(_) => {
                self.pause_record(
                    direction,
                    "rx_discard",
                    json!({"reason":"rx_disabled","packet_id":evidence}),
                    d,
                    c,
                )?;
                return Ok(true);
            }
            Err(reason) => {
                self.pause_record(
                    direction,
                    "rx_discard",
                    json!({"reason":reason,"packet_id":evidence}),
                    d,
                    c,
                )?;
                return Ok(true);
            }
        };
        crate::allocation::reservation_checkpoint("network_mac_control_receive")?;
        let pending = self.pause_request_in_progress(direction, d);
        self.pause_mut(d).ports[direction].as_mut().unwrap().pending = Some(Request {
            quanta,
            received_ps: c.now(),
            evidence: evidence.to_owned(),
        });
        self.pause_record(direction,"receive",json!({"packet_id":evidence,"quanta":quanta.to_string(),"mac_hex":crate::runtime::wire::codec::hex(bytes),"pending_mac_request":pending}),d,c)?;
        if !pending {
            self.pause_mac_completed(direction, d, c)?;
        }
        self.pause_refresh_permission(direction, d, c)?;
        c.request_arbitration("network").map_err(model)?;
        Ok(true)
    }
    pub(super) fn pause_mac_completed(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(port) = self.pause_port(direction, d) else {
            return Ok(());
        };
        let Some(request) = port.pending.clone() else {
            return Ok(());
        };
        let dt = self.pause_duration(direction, request.quanta)?;
        let end = add(c.now(), dt)?;
        let p = self.pause_mut(d).ports[direction].as_mut().unwrap();
        p.pending = None;
        p.generation = add(p.generation, 1)?;
        let generation = p.generation;
        p.timer = (dt != 0).then_some(Timer {
            generation,
            loaded_ps: c.now(),
            expires_ps: end,
            request,
        });
        // Obsolete expiry is removed, and any independently delivered stale token checks its generation.
        self.due_mut(d).retain(
            |due| !matches!(due.kind,Completion::PauseExpire{direction:dir,..} if dir==direction),
        );
        if dt != 0 {
            self.due_mut(d).push(Due {
                at: end,
                kind: Completion::PauseExpire {
                    direction,
                    generation,
                },
            });
        }
        self.pause_record(
            direction,
            "timer_load",
            json!({"duration_ps":dt.to_string(),"expires_ps":end.to_string(),"physical_rate_num":self.pause_physical_rate(direction).0.to_string(),"physical_rate_den":self.pause_physical_rate(direction).1.to_string()}),
            d,
            c,
        )?;
        c.request_arbitration("network").map_err(model)
    }
    pub(super) fn pause_expire(
        &self,
        direction: usize,
        generation: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self
            .pause_port(direction, d)
            .and_then(|p| p.timer.as_ref())
            .is_some_and(|t| t.generation == generation && t.expires_ps == c.now())
        {
            self.pause_mut(d).ports[direction].as_mut().unwrap().timer = None;
            self.pause_record(
                direction,
                "expiry",
                json!({"generation":generation.to_string()}),
                d,
                c,
            )?;
            c.request_arbitration("network").map_err(model)?;
        }
        Ok(())
    }
    pub(super) fn pause_admin(
        &self,
        index: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let control = &self.prepared.mac_control.as_ref().unwrap().controls[index];
        let direction = control.direction;
        match control.action {
            Action::PauseRequest(quanta) => {
                let p = self.pause_port(direction, d).unwrap();
                if p.tx_enabled && p.transmit_enabled && p.link_up {
                    let epoch = p.epoch;
                    let ordinal = p.ordinal;
                    let id = format!("@mac-control/{direction}/{epoch}/{ordinal}");
                    let cfg = self.prepared.mac_control.as_ref().unwrap().ports[direction]
                        .as_ref()
                        .unwrap();
                    let bytes = frame(cfg.station_mac, quanta);
                    let intent = WireTxIntent {
                        adapter_id: "@mac-control".into(),
                        protocol: WireProtocol::MacControl,
                        port: self.prepared.ethernet.directions[direction]
                            .from_port
                            .clone(),
                        packet_id: id.clone(),
                        bytes: bytes.clone(),
                        cause_id: format!("pause_request:{index}"),
                        ordinal,
                    };
                    self.pause_mut(d).ports[direction].as_mut().unwrap().ordinal = add(ordinal, 1)?;
                    self.wire_enqueue(
                        ControlTx {
                            queued_ps: None,
                            arrival_retired_ps: None,
                            intent,
                            payload: bytes,
                            vid: None,
                            destination: Some(PAUSE_DA),
                            source: Some(cfg.station_mac),
                            sof: None,
                            eof: None,
                            release: None,
                            arrival: None,
                            planned_eof: None,
                            planned_release: None,
                            planned_arrival: None,
                            dropped: false,
                            transport: Some(ControlTransport::MacControl { epoch }),
                            phy_transfer: None,
                            phy_receivers: BTreeMap::new(),
                        },
                        d,
                        c,
                    )?;
                } else {
                    self.pause_record(
                        direction,
                        "request_discard",
                        json!({"reason":"generation_disabled","control":index}),
                        d,
                        c,
                    )?;
                }
            }
            action => {
                let p = self.pause_mut(d).ports[direction].as_mut().unwrap();
                match action {
                    Action::RxEnable(enabled) => {
                        p.rx_enabled = enabled;
                        if !enabled {
                            p.pending = None;
                            p.timer = None;
                            p.generation = add(p.generation, 1)?;
                        }
                    }
                    Action::TxEnable(enabled) => p.tx_enabled = enabled,
                    Action::TransmitEnable(enabled) => p.transmit_enabled = enabled,
                    _ => unreachable!(),
                }
                if !self.pause_port(direction, d).unwrap().tx_enabled
                    || !self.pause_port(direction, d).unwrap().transmit_enabled
                {
                    self.pause_cancel_queued(direction, d, c)?;
                }
                self.pause_record(direction, "administration", json!({"control":index}), d, c)?;
            }
        }
        self.pause_refresh_permission(direction, d, c)?;
        c.request_arbitration("network").map_err(model)
    }
    fn pause_cancel_queued(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let ids = self
            .wire_host(d)
            .map(|h| {
                h.queues[direction]
                    .iter()
                    .filter(|id| h.tx[*id].is_mac_control())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in ids {
            let h = self.wire_mut(d)?;
            h.queues[direction].retain(|s| s != &id);
            h.tx.get_mut(&id).unwrap().dropped = true;
            self.wire_tx_record(
                direction,
                &id,
                "tx_completed",
                Some("generation_disabled"),
                d,
                c,
            )?;
        }
        Ok(())
    }
    pub(super) fn sync_pause_links(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let Some(cfg) = &self.prepared.mac_control else {
            return Ok(());
        };
        for direction in 0..cfg.ports.len() {
            if cfg.ports[direction].is_none() {
                continue;
            }
            let up = self
                .policy(d)
                .and_then(|p| {
                    p.link_up
                        .get(&self.prepared.ethernet.directions[direction].from_port)
                        .copied()
                })
                .unwrap_or(true);
            if self.pause_port(direction, d).unwrap().link_up != up {
                let p = self.pause_mut(d).ports[direction].as_mut().unwrap();
                p.link_up = up;
                p.epoch = add(p.epoch, 1)?;
                p.generation = add(p.generation, 1)?;
                p.pending = None;
                p.timer = None;
                self.pause_cancel_queued(direction, d, c)?;
                self.pause_record(direction, "link", json!({"up":up}), d, c)?;
            }
        }
        Ok(())
    }
    pub(super) fn pause_wire_record(
        &self,
        direction: usize,
        id: &str,
        kind: &str,
        reason: Option<&str>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let tx = self.wire_host(d).unwrap().tx[id].clone();
        self.pause_record(direction,kind,json!({"packet_id":id,"reason":reason,"mac_hex":crate::runtime::wire::codec::hex(&tx.intent.bytes),"mac_bytes":"64","sof_ps":opt(tx.sof),"eof_ps":opt(tx.eof),"release_ps":opt(tx.release),"arrival_ps":opt(tx.arrival),"planned_eof_ps":opt(tx.planned_eof),"planned_release_ps":opt(tx.planned_release),"planned_arrival_ps":opt(tx.planned_arrival)}),d,c)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_quanta_rounding_and_bounds() {
        assert_eq!(duration(0, 3).unwrap(), 0);
        assert_eq!(duration(1, 1_000_000_000).unwrap(), 512000);
        assert_eq!(duration(65535, 100_000_000).unwrap(), 335539200000);
        assert_eq!(duration(1, 3).unwrap(), 170666666666667);
        assert!(duration(65535, 1).is_err());
        assert!(duration(1, 0).is_err());
    }
    #[test]
    fn actual_control_bytes_fcs_address_opcode_size() {
        let station = [2, 0, 0, 0, 0, 1];
        let mut b = frame(station, 0xabcd);
        assert_eq!(&b[12..18], &[0x88, 8, 0, 1, 0xab, 0xcd]);
        assert_eq!(b.len(), 64);
        assert_eq!(parse(&b, station).unwrap(), Some(0xabcd));
        b[..6].copy_from_slice(&station);
        let f = crc32(&b[..60]);
        b[60..].copy_from_slice(&f.to_le_bytes());
        assert_eq!(parse(&b, station).unwrap(), Some(0xabcd));
        b[0] = 4;
        let f = crc32(&b[..60]);
        b[60..].copy_from_slice(&f.to_le_bytes());
        assert_eq!(parse(&b, station), Err("wrong_destination"));
        b = frame(station, 1);
        b[15] = 2;
        let f = crc32(&b[..60]);
        b[60..].copy_from_slice(&f.to_le_bytes());
        assert_eq!(parse(&b, station), Err("unsupported_opcode"));
        b = frame(station, 1);
        b[17] ^= 1;
        assert_eq!(parse(&b, station), Err("bad_fcs"));
        assert_eq!(parse(&b[..40], station), Err("truncated_control"));
        b.push(0);
        assert_eq!(parse(&b, station), Err("oversized_control"));
    }
}
#[cfg(test)]
mod atomic_tests {
    #[test]
    fn mac_control_generation_reservation_failure_preserves_owned_prefix() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/pause-media/model.ini"),
        )
        .unwrap();
        crate::allocation::fail_next_reservation("network_wire_fifo");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        assert!(
            !failed
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "dir.network.mac_control")
        );
        let backend = failed.network.as_ref().unwrap();
        assert!(
            backend["configured_media"]["attempts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            backend["mac_control"]["ports"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| !p.is_null())
                .all(|p| p["ordinal"] == "0")
        );
    }
    #[test]
    fn configured_boundary_reservation_failure_keeps_last_actual_attempt_unfinished() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/pause-media/model.ini"),
        )
        .unwrap();
        crate::allocation::fail_next_reservation("network_configured_media_boundary");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        let records = &failed.registered.as_ref().unwrap().model_records;
        assert!(
            records
                .values()
                .any(|r| r.schema.name == "dir.network.mac_control"
                    && r.data["kind"] == "tx_started")
        );
        assert!(
            !records
                .values()
                .any(|r| r.schema.name == "dir.network.mac_control"
                    && matches!(r.data["kind"].as_str(), Some("receive" | "tx_completed")))
        );
        assert!(
            failed.network.as_ref().unwrap()["configured_media"]["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["status"] == "transmitting")
        );
    }
}
#[cfg(test)]
mod commit_atomic_tests {
    #[test]
    fn staged_pause_and_fifo_commit_failure_publishes_no_owner_or_record_changes() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/pause-media/model.ini"),
        )
        .unwrap();
        crate::allocation::fail_next_reservation("network_media_pause_reserve");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        assert!(
            !failed
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "dir.network.mac_control")
        );
        let net = failed.network.as_ref().unwrap();
        assert!(
            net["configured_media"]["attempts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            net["mac_control"]["ports"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| !p.is_null())
                .all(|p| p["ordinal"] == "0" && p["pending"].is_null() && p["timer"].is_null())
        );
    }
}
