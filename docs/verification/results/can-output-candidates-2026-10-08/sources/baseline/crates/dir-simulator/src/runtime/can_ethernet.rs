//! Staged composite conversion and CAN adapter; scheduling belongs to the common Engine.
pub(crate) mod protocol;
use super::{can::protocol as can_wire, ethernet};
use crate::{
    snapshot::{Receiver, Request, can_ethernet::*},
    types::{
        Diagnostic, Frame,
        can_ethernet::*,
        ethernet::{EthernetVlanTag, EthernetWireFrame},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(s: impl Into<String>) -> Diagnostic {
    Diagnostic::execution(s).with_reason("composite_bridge")
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| error("bridge time/ID arithmetic overflow"))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum BridgeTimer {
    NativeCan { generator: usize, ordinal: u64 },
    CanReady { request: usize },
    CanEof { request: usize },
    CanRelease { request: usize },
    CanObserve { request: usize, controller: usize },
    CanReceived { request: usize, controller: usize },
    ConversionReady { conversion: usize },
    BranchOffer { conversion: usize, branch: usize },
    RetryCan { source: usize },
    RetryEthernet { source: usize },
}
#[derive(Debug, Clone)]
pub(crate) enum BridgeEvent {
    Timer(BridgeTimer),
    EthernetIngress {
        endpoint: usize,
        ingress_record: String,
        origin_id: String,
        generated_ps: u64,
        wire: EthernetWireFrame,
        vid: u16,
        priority: u8,
        lineage: Option<BridgeLineage>,
    },
    EthernetSof {
        branch_id: Option<String>,
        lineage: BridgeLineage,
        source_record_id: String,
        targets: Vec<String>,
    },
    TerminalReceived {
        lineage: BridgeLineage,
        terminal_id: String,
        reception_id: String,
    },
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct TxQueueView {
    pub capacity_frames: u64,
    pub capacity_bytes: Option<u64>,
    pub queued_frames: u64,
    pub queued_bytes: u64,
    pub port_capacity_frames: u64,
    pub port_queued_frames: u64,
}
#[derive(Debug, Default)]
pub(crate) struct BridgeTxView {
    pub ethernet: BTreeMap<(usize, u8), TxQueueView>,
}
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // Inline owned buffers are moved at the single commit boundary.
pub(crate) enum BridgeAction {
    Schedule {
        at: u64,
        phase: u8,
        event: BridgeTimer,
    },
    DirtyCan {
        bus: usize,
    },
    InjectEthernet {
        source: usize,
        wire: EthernetWireFrame,
        vid: u16,
        priority: u8,
        origin_id: String,
        parent_id: String,
        visited_gateways: Vec<String>,
        branch_lineage: Vec<String>,
        segment_id: String,
        generated_ps: u64,
        branch_id: String,
        child_id: String,
    },
    Record {
        schema: &'static str,
        record_id: String,
        time_ps: u64,
        data: Value,
    },
}
#[derive(Debug, Clone)]
struct Branch {
    row: BranchRecord,
    egress: BridgeEgress,
    packet: DirCanPacket,
    lineage: BridgeLineage,
    wire: Option<EthernetWireFrame>,
}
#[derive(Debug, Clone)]
struct Conversion {
    row: ConversionRecord,
    gateway: usize,
    branches: Vec<Branch>,
}
#[derive(Debug, Clone)]
struct CanRequest {
    row: Request,
    source: usize,
    frame: Frame,
    wire: can_wire::WireFrame,
    lineage: BridgeLineage,
    branch_id: Option<String>,
}
#[derive(Debug, Clone, Default)]
struct CanQueue {
    waiting: VecDeque<usize>,
}
#[derive(Debug)]
pub(crate) struct BridgeState {
    prepared: Arc<PreparedCanEthernet>,
    conversions: Vec<Conversion>,
    requests: Vec<CanRequest>,
    receivers: Vec<Receiver>,
    segments: Vec<SegmentRecord>,
    queues: Vec<CanQueue>,
    active: Vec<Option<usize>>,
    rx: Vec<u64>,
    rx_max: Vec<u64>,
    future_generation: bool,
}
#[derive(Debug)]
pub(crate) struct BridgeDelta {
    conversions: Vec<(usize, Conversion)>,
    requests: Vec<(usize, CanRequest)>,
    receivers: Vec<(usize, Receiver)>,
    segments: Vec<(usize, SegmentRecord)>,
    queues: BTreeMap<usize, CanQueue>,
    active: BTreeMap<usize, Option<usize>>,
    rx: BTreeMap<usize, u64>,
    rx_max: BTreeMap<usize, u64>,
    ethernet_reserved: BTreeMap<(usize, u8), (u64, u64)>,
    pub actions: Vec<BridgeAction>,
    pub future_generation: bool,
}
impl BridgeState {
    pub(crate) fn new(p: &PreparedCanEthernet) -> Result<Self> {
        for g in &p.can.generators {
            can_wire::serialize(&g.frame)?;
        }
        Ok(Self {
            prepared: Arc::new(p.clone()),
            conversions: vec![],
            requests: vec![],
            receivers: vec![],
            segments: vec![],
            queues: vec![CanQueue::default(); p.can.controllers.len()],
            active: vec![None; p.can.buses.len()],
            rx: vec![0; p.gateways.len()],
            rx_max: vec![0; p.gateways.len()],
            future_generation: false,
        })
    }
    pub(crate) fn empty_delta(&self) -> BridgeDelta {
        self.delta()
    }
    fn delta(&self) -> BridgeDelta {
        BridgeDelta {
            conversions: vec![],
            requests: vec![],
            receivers: vec![],
            segments: vec![],
            queues: BTreeMap::new(),
            active: BTreeMap::new(),
            rx: BTreeMap::new(),
            rx_max: BTreeMap::new(),
            ethernet_reserved: BTreeMap::new(),
            actions: vec![],
            future_generation: self.future_generation,
        }
    }
    fn conversion<'a>(&'a self, d: &'a BridgeDelta, i: usize) -> &'a Conversion {
        d.conversions
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.conversions[i])
    }
    fn request<'a>(&'a self, d: &'a BridgeDelta, i: usize) -> &'a CanRequest {
        d.requests
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.requests[i])
    }
    fn queue<'a>(&'a self, d: &'a BridgeDelta, i: usize) -> &'a CanQueue {
        d.queues.get(&i).unwrap_or(&self.queues[i])
    }
    fn queue_mut<'a>(&self, d: &'a mut BridgeDelta, i: usize) -> &'a mut CanQueue {
        d.queues.entry(i).or_insert_with(|| self.queues[i].clone())
    }
    fn conversion_len(&self, d: &BridgeDelta) -> usize {
        d.conversions
            .iter()
            .map(|(i, _)| i + 1)
            .max()
            .unwrap_or(self.conversions.len())
            .max(self.conversions.len())
    }
    fn request_len(&self, d: &BridgeDelta) -> usize {
        d.requests
            .iter()
            .map(|(i, _)| i + 1)
            .max()
            .unwrap_or(self.requests.len())
            .max(self.requests.len())
    }
    fn stage_conversion(
        &self,
        d: &mut BridgeDelta,
        i: usize,
        c: Conversion,
        now: u64,
    ) -> Result<()> {
        let previous = d
            .conversions
            .iter()
            .rev()
            .find(|(index, _)| *index == i)
            .map(|(_, conversion)| conversion)
            .or_else(|| self.conversions.get(i));
        let conversion_changed = previous.is_none_or(|old| old.row != c.row);
        let changed_branches: Vec<_> = c
            .branches
            .iter()
            .enumerate()
            .filter(|(index, branch)| {
                previous
                    .and_then(|old| old.branches.get(*index))
                    .is_none_or(|old| old.row != branch.row)
            })
            .map(|(_, branch)| &branch.row)
            .collect();
        if conversion_changed {
            record(
                d,
                "dir.can_ethernet.conversion",
                c.row.conversion_id.clone(),
                now,
                &c.row,
            )?;
        }
        for row in changed_branches {
            record(
                d,
                "dir.can_ethernet.branch",
                row.branch_id.clone(),
                now,
                row,
            )?;
        }
        d.conversions.push((i, c));
        Ok(())
    }
    pub(crate) fn initialize(&self, limit: u64) -> Result<BridgeDelta> {
        let mut d = self.delta();
        for (i, g) in self.prepared.can.generators.iter().enumerate() {
            if let Some(t) = g.schedule.time(0) {
                if t < u128::from(limit) {
                    schedule(
                        &mut d,
                        t as u64,
                        1,
                        BridgeTimer::NativeCan {
                            generator: i,
                            ordinal: 0,
                        },
                    );
                } else {
                    d.future_generation = true;
                }
            }
        }
        Ok(d)
    }
    #[cfg(test)]
    pub(crate) fn plan(
        &self,
        event: &BridgeEvent,
        now: u64,
        tx: &BridgeTxView,
        limit: u64,
    ) -> Result<BridgeDelta> {
        let mut d = self.delta();
        self.plan_in_delta(&mut d, event, now, tx, limit)?;
        Ok(d)
    }
    pub(crate) fn plan_in_delta(
        &self,
        d: &mut BridgeDelta,
        event: &BridgeEvent,
        now: u64,
        tx: &BridgeTxView,
        limit: u64,
    ) -> Result<()> {
        match event {
            BridgeEvent::Timer(timer) => self.timer(d, timer, now, tx, limit)?,
            BridgeEvent::EthernetIngress {
                endpoint,
                ingress_record,
                origin_id,
                generated_ps,
                wire,
                vid,
                priority,
                lineage,
            } => {
                let gw = self
                    .prepared
                    .gateways
                    .iter()
                    .position(|g| g.ethernet_endpoint == *endpoint);
                if let Some(gw) = gw {
                    let lineage = lineage.clone().unwrap_or_else(|| {
                        let mut lineage =
                            BridgeLineage::native("ethernet", origin_id, *generated_ps);
                        lineage.parent_id = ingress_record.clone();
                        lineage
                    });
                    let packet = if wire.ether_type == protocol::ETHERTYPE {
                        protocol::decode_hex(&wire.data_hex).ok()
                    } else {
                        None
                    };
                    self.ingress(
                        d,
                        gw,
                        &self.prepared.ethernet.devices[*endpoint].id,
                        ingress_record,
                        packet,
                        lineage,
                        Some((*vid, *priority)),
                        now,
                        tx,
                    )?;
                }
            }
            BridgeEvent::EthernetSof {
                branch_id,
                lineage,
                source_record_id,
                targets,
            } => {
                if let Some(id) = branch_id {
                    self.branch_sof(d, id, now)?;
                }
                self.segment_sof(d, lineage, source_record_id, targets, now)?;
            }
            BridgeEvent::TerminalReceived {
                lineage,
                terminal_id,
                reception_id,
            } => self.segment_received(d, lineage, terminal_id, reception_id, now)?,
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn ingress(
        &self,
        d: &mut BridgeDelta,
        gw: usize,
        ingress: &str,
        ingress_record: &str,
        packet: Option<DirCanPacket>,
        mut lineage: BridgeLineage,
        vlan: Option<(u16, u8)>,
        now: u64,
        tx: &BridgeTxView,
    ) -> Result<()> {
        let g = &self.prepared.gateways[gw];
        lineage.parent_id = ingress_record.into();
        let direction = if vlan.is_some() {
            "ethernet_to_can"
        } else {
            "can_to_ethernet"
        };
        let rule = packet.as_ref().and_then(|p| {
            g.rules.iter().find(|r| {
                r.direction == direction
                    && r.ingress == ingress
                    && r.format == p.format
                    && r.can_id == p.id
                    && r.vid == vlan.map(|v| v.0)
                    && r.pcp == vlan.map(|v| v.1)
            })
        });
        let reason = if packet.is_none() {
            Some("invalid_codec")
        } else if lineage.visited_gateways.contains(&g.instance) {
            Some("loop_prevented")
        } else if lineage.visited_gateways.len() >= usize::from(g.max_hops) {
            Some("hop_limit")
        } else if rule.is_none() {
            Some("no_rule")
        } else if *d.rx.get(&gw).unwrap_or(&self.rx[gw]) >= g.rx_capacity {
            Some("rx_full")
        } else {
            None
        };
        let id = lineage_id(&[
            &lineage.origin_id,
            &g.instance,
            ingress_record,
            rule.map(|r| r.id.as_str()).unwrap_or("rejected"),
        ]);
        let index = self.conversion_len(d);
        let mut row = ConversionRecord {
            conversion_id: id.clone(),
            origin_id: lineage.origin_id.clone(),
            parent_id: ingress_record.into(),
            gateway: g.instance.clone(),
            ingress_record: ingress_record.into(),
            rule_id: rule.map(|r| r.id.clone()),
            visited_gateways: lineage.visited_gateways.clone(),
            observed_ps: now,
            ready_ps: None,
            planned_ready_ps: None,
            released_ps: None,
            status: if reason.is_some() {
                "rejected"
            } else {
                "processing"
            }
            .into(),
            reason: reason.map(str::to_owned),
            branch_ids: vec![],
        };
        if reason.is_some() {
            return self.stage_conversion(
                d,
                index,
                Conversion {
                    row,
                    gateway: gw,
                    branches: vec![],
                },
                now,
            );
        }
        let rule = rule.unwrap();
        let packet = packet.unwrap();
        let rx_delay = if vlan.is_some() {
            self.prepared.ethernet.devices[g.ethernet_endpoint].rx_processing_delay_ps
        } else {
            self.prepared.can.controllers[g
                .can_ports
                .iter()
                .copied()
                .find(|i| self.prepared.can.controllers[*i].id == ingress)
                .unwrap()]
            .rx_processing_ps
        };
        let ready = add(add(now, rx_delay)?, g.conversion_delay_ps)?;
        row.planned_ready_ps = Some(ready);
        lineage.visited_gateways.push(g.instance.clone());
        row.visited_gateways = lineage.visited_gateways.clone();
        let mut branches = Vec::new();
        for (i, egress) in rule.egresses.iter().enumerate() {
            let branch_id = lineage_id(&[&id, &i.to_string()]);
            let child_id = lineage_id(&[&branch_id, "child"]);
            let tx_delay = match egress {
                BridgeEgress::Ethernet { source, .. } => {
                    self.prepared.ethernet.devices[*source].tx_processing_delay_ps
                }
                BridgeEgress::Can { source, .. } => {
                    self.prepared.can.controllers[*source].tx_processing_ps
                }
            };
            let offer = add(ready, tx_delay)?;
            let mut branch_lineage = lineage.clone();
            branch_lineage.parent_id = branch_id.clone();
            branch_lineage.branch_lineage.push(branch_id.clone());
            branch_lineage.segment_id = child_id.clone();
            let (format, can_id, pcp) = match egress {
                BridgeEgress::Can { format, can_id, .. } => (format.clone(), *can_id, None),
                BridgeEgress::Ethernet { pcp, .. } => {
                    (packet.format.clone(), packet.id, Some(*pcp))
                }
            };
            let codec = protocol::encode(&packet)?;
            let wire = if let BridgeEgress::Ethernet {
                source,
                dst_mac,
                vid,
                pcp,
                ..
            } = egress
            {
                let port = &self
                    .prepared
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.device == *source)
                    .ok_or_else(|| error("missing Gateway Ethernet port policy"))?;
                let tag = port.vlans[vid].then_some(EthernetVlanTag {
                    vid: *vid,
                    pcp: *pcp,
                    dei: 0,
                });
                Some(ethernet::serialize_vlan_frame(
                    self.prepared.ethernet.devices[*source]
                        .mac
                        .as_deref()
                        .unwrap(),
                    dst_mac,
                    protocol::ETHERTYPE,
                    &protocol::hex(&codec),
                    tag,
                )?)
            } else {
                None
            };
            let record = BranchRecord {
                branch_id: branch_id.clone(),
                conversion_id: id.clone(),
                egress: egress.port().into(),
                child_id: None,
                planned_child_id: child_id,
                codec_length: codec.len() as u64,
                pcp,
                input_format: packet.format.clone(),
                input_can_id: packet.id,
                output_format: format,
                output_can_id: can_id,
                offer_ps: None,
                planned_offer_ps: offer,
                admitted_ps: None,
                sof_ps: None,
                status: "waiting".into(),
                reason: None,
            };
            row.branch_ids.push(branch_id);
            branches.push(Branch {
                row: record,
                egress: egress.clone(),
                packet: packet.clone(),
                lineage: branch_lineage,
                wire,
            });
        }
        let occupancy = add(*d.rx.get(&gw).unwrap_or(&self.rx[gw]), 1)?;
        d.rx.insert(gw, occupancy);
        d.rx_max.insert(
            gw,
            occupancy.max(*d.rx_max.get(&gw).unwrap_or(&self.rx_max[gw])),
        );
        self.stage_conversion(
            d,
            index,
            Conversion {
                row,
                gateway: gw,
                branches,
            },
            now,
        )?;
        if ready == now {
            self.ready(d, index, now, tx)?;
        } else {
            schedule(
                d,
                ready,
                0,
                BridgeTimer::ConversionReady { conversion: index },
            );
        }
        Ok(())
    }
    fn ready(&self, d: &mut BridgeDelta, index: usize, now: u64, tx: &BridgeTxView) -> Result<()> {
        let mut c = self.conversion(d, index).clone();
        if c.row.ready_ps.is_some() {
            return Ok(());
        }
        c.row.ready_ps = Some(now);
        c.row.status = "waiting_tx".into();
        let offers: Vec<_> = c
            .branches
            .iter()
            .enumerate()
            .map(|(b, row)| (b, row.row.planned_offer_ps))
            .collect();
        self.stage_conversion(d, index, c, now)?;
        for (branch, at) in offers {
            if at == now {
                self.offer(d, index, branch, now, tx)?;
            } else {
                schedule(
                    d,
                    at,
                    1,
                    BridgeTimer::BranchOffer {
                        conversion: index,
                        branch,
                    },
                );
            }
        }
        Ok(())
    }
    fn offer(
        &self,
        d: &mut BridgeDelta,
        index: usize,
        branch: usize,
        now: u64,
        tx: &BridgeTxView,
    ) -> Result<()> {
        let mut c = self.conversion(d, index).clone();
        let b = &mut c.branches[branch];
        if b.row.status != "waiting" {
            return Ok(());
        }
        b.row.offer_ps.get_or_insert(now);
        let (unadmittable, available);
        match &b.egress {
            BridgeEgress::Can { source, .. } => {
                let cap = self.prepared.can.controllers[*source].queue_capacity;
                unadmittable = cap == 0;
                available = (self.queue(d, *source).waiting.len() as u64) < cap;
            }
            BridgeEgress::Ethernet { source, pcp, .. } => {
                let view = tx
                    .ethernet
                    .get(&(*source, *pcp))
                    .ok_or_else(|| error("missing Ethernet TX capacity read view"))?;
                let bytes = b.wire.as_ref().unwrap().mac_bytes;
                unadmittable = view.capacity_frames == 0
                    || view.port_capacity_frames == 0
                    || view.capacity_bytes.is_some_and(|cap| bytes > cap);
                let extra = d
                    .ethernet_reserved
                    .get(&(*source, *pcp))
                    .copied()
                    .unwrap_or((0, 0));
                let extra_port = d
                    .ethernet_reserved
                    .iter()
                    .filter(|((s, _), _)| s == source)
                    .try_fold(0u64, |n, (_, v)| add(n, v.0))?;
                available = add(view.port_queued_frames, extra_port)? < view.port_capacity_frames
                    && add(view.queued_frames, extra.0)? < view.capacity_frames
                    && view.capacity_bytes.is_none_or(|cap| {
                        u128::from(view.queued_bytes) + u128::from(extra.1) + u128::from(bytes)
                            <= u128::from(cap)
                    });
            }
        }
        if unadmittable {
            b.row.status = "dropped".into();
            b.row.reason = Some("tx_unadmittable".into());
        } else if available {
            b.row.status = "admitted".into();
            b.row.admitted_ps = Some(now);
            b.row.child_id = Some(b.row.planned_child_id.clone());
            match &b.egress {
                BridgeEgress::Can {
                    source,
                    format,
                    can_id,
                    ..
                } => {
                    let frame = Frame {
                        format: format.name().into(),
                        id: *can_id,
                        data: protocol::hex(&b.packet.data),
                    };
                    self.create_can(
                        d,
                        *source,
                        frame,
                        b.row.planned_child_id.clone(),
                        b.lineage.clone(),
                        Some(b.row.branch_id.clone()),
                        now,
                        now,
                        true,
                    )?;
                }
                BridgeEgress::Ethernet {
                    source, vid, pcp, ..
                } => {
                    let wire = b.wire.clone().unwrap();
                    let extra = d.ethernet_reserved.entry((*source, *pcp)).or_default();
                    extra.0 = add(extra.0, 1)?;
                    extra.1 = add(extra.1, wire.mac_bytes)?;
                    d.actions.push(BridgeAction::InjectEthernet {
                        source: *source,
                        wire,
                        vid: *vid,
                        priority: *pcp,
                        origin_id: b.lineage.origin_id.clone(),
                        parent_id: b.row.branch_id.clone(),
                        visited_gateways: b.lineage.visited_gateways.clone(),
                        branch_lineage: b.lineage.branch_lineage.clone(),
                        segment_id: b.lineage.segment_id.clone(),
                        generated_ps: b.lineage.generated_ps,
                        branch_id: b.row.branch_id.clone(),
                        child_id: b.row.planned_child_id.clone(),
                    });
                }
            }
        }
        if c.branches.iter().all(|b| b.row.status != "waiting") {
            c.row.status = "released".into();
            c.row.released_ps = Some(now);
            let gw = c.gateway;
            let occupancy = *d.rx.get(&gw).unwrap_or(&self.rx[gw]);
            d.rx.insert(
                gw,
                occupancy
                    .checked_sub(1)
                    .ok_or_else(|| error("Gateway RX underflow"))?,
            );
        }
        self.stage_conversion(d, index, c, now)
    }
    #[allow(clippy::too_many_arguments)]
    fn create_can(
        &self,
        d: &mut BridgeDelta,
        source: usize,
        frame: Frame,
        id: String,
        lineage: BridgeLineage,
        branch_id: Option<String>,
        generated: u64,
        ready: u64,
        admit: bool,
    ) -> Result<usize> {
        let controller = &self.prepared.can.controllers[source];
        let bus = self.prepared.can.controller_buses[source];
        let wire = can_wire::serialize(&frame)?;
        let i = self.request_len(d);
        let row = Request {
            request_id: id,
            source: controller.id.clone(),
            bus: self.prepared.can.buses[bus].id.clone(),
            status: if admit { "pending" } else { "processing" }.into(),
            generated_ps: generated,
            ready_ps: admit.then_some(ready),
            tx_enqueued_ps: admit.then_some(ready),
            sof_ps: None,
            eof_ps: None,
            planned_eof_ps: None,
            planned_release_ps: None,
            release_ps: None,
            payload_bits: frame.data.len() as u64 * 4,
            frame_bits: wire.frame.len() as u64,
            crc15: wire.crc15,
            stuff_bits: wire.stuff_positions.len() as u64,
            bitrate_bps: self.prepared.can.buses[bus].bitrate,
        };
        let request = CanRequest {
            row,
            source,
            frame,
            wire,
            lineage,
            branch_id,
        };
        emit_request(d, &request, generated)?;
        d.requests.push((i, request));
        if admit {
            self.queue_mut(d, source).waiting.push_back(i);
            d.actions.push(BridgeAction::DirtyCan { bus });
        }
        Ok(i)
    }
    fn timer(
        &self,
        d: &mut BridgeDelta,
        timer: &BridgeTimer,
        now: u64,
        tx: &BridgeTxView,
        limit: u64,
    ) -> Result<()> {
        let request_handle = match timer {
            BridgeTimer::CanReady { request }
            | BridgeTimer::CanEof { request }
            | BridgeTimer::CanRelease { request }
            | BridgeTimer::CanObserve { request, .. }
            | BridgeTimer::CanReceived { request, .. } => Some(*request),
            _ => None,
        };
        if request_handle.is_some_and(|i| i >= self.request_len(d)) {
            return Err(error("CAN request handle out of range"));
        }
        match timer {
            BridgeTimer::CanObserve { controller, .. }
            | BridgeTimer::CanReceived { controller, .. }
            | BridgeTimer::RetryCan { source: controller }
                if *controller >= self.prepared.can.controllers.len() =>
            {
                return Err(error("CAN Controller handle out of range"));
            }
            BridgeTimer::ConversionReady { conversion }
            | BridgeTimer::BranchOffer { conversion, .. }
                if *conversion >= self.conversion_len(d) =>
            {
                return Err(error("conversion handle out of range"));
            }
            BridgeTimer::BranchOffer { conversion, branch }
                if *branch >= self.conversion(d, *conversion).branches.len() =>
            {
                return Err(error("branch handle out of range"));
            }
            BridgeTimer::RetryEthernet { source }
                if *source >= self.prepared.ethernet.devices.len() =>
            {
                return Err(error("Ethernet retry handle out of range"));
            }
            _ => {}
        }
        match timer {
            BridgeTimer::CanEof { request } => {
                let r = self.request(d, *request);
                if r.row.sof_ps.is_none()
                    || r.row.planned_eof_ps != Some(now)
                    || r.row.eof_ps.is_some()
                {
                    return Err(error("invalid CAN EOF transition/time"));
                }
            }
            BridgeTimer::CanRelease { request } => {
                let r = self.request(d, *request);
                let bus = self.prepared.can.controller_buses[r.source];
                if r.row.eof_ps.is_none()
                    || r.row.planned_release_ps != Some(now)
                    || d.active.get(&bus).copied().unwrap_or(self.active[bus]) != Some(*request)
                {
                    return Err(error("invalid CAN release transition/time"));
                }
            }
            BridgeTimer::CanObserve {
                request,
                controller,
            } => {
                let r = self.request(d, *request);
                let c = &self.prepared.can.controllers[*controller];
                if *controller == r.source
                    || self.prepared.can.controller_buses[*controller]
                        != self.prepared.can.controller_buses[r.source]
                    || !can_wire::accepts(&c.rx_filter, &r.frame)
                    || r.row.eof_ps.and_then(|t| t.checked_add(c.rx_channel_ps)) != Some(now)
                {
                    return Err(error("invalid CAN observation transition/time"));
                }
            }
            BridgeTimer::CanReceived {
                request,
                controller,
            } => {
                let r = self.request(d, *request);
                let c = &self.prepared.can.controllers[*controller];
                let observed = d
                    .receivers
                    .iter()
                    .rev()
                    .find(|(_, v)| v.request_id == r.row.request_id && v.receiver == c.id)
                    .map(|(_, v)| v)
                    .or_else(|| {
                        self.receivers
                            .iter()
                            .find(|v| v.request_id == r.row.request_id && v.receiver == c.id)
                    });
                if observed
                    .and_then(|r| r.observed_ps)
                    .and_then(|t| t.checked_add(c.rx_processing_ps))
                    != Some(now)
                {
                    return Err(error("invalid CAN receive completion transition/time"));
                }
            }
            BridgeTimer::ConversionReady { conversion } => {
                let c = self.conversion(d, *conversion);
                if c.row.planned_ready_ps != Some(now) || c.row.reason.is_some() {
                    return Err(error("invalid conversion ready transition/time"));
                }
            }
            BridgeTimer::BranchOffer { conversion, branch } => {
                let c = self.conversion(d, *conversion);
                if c.row.ready_ps.is_none() || c.branches[*branch].row.planned_offer_ps != now {
                    return Err(error("invalid branch offer transition/time"));
                }
            }
            _ => {}
        }
        match timer {
            BridgeTimer::NativeCan { generator, ordinal } => {
                let g = self
                    .prepared
                    .can
                    .generators
                    .get(*generator)
                    .ok_or_else(|| error("CAN generator handle out of range"))?;
                if g.schedule.time(*ordinal) != Some(u128::from(now)) {
                    return Err(error("CAN generation ordinal/time mismatch"));
                }
                let id = format!("{}:{ordinal}", g.id);
                let lineage = BridgeLineage::native("can", &id, now);
                let ready = add(
                    now,
                    add(
                        self.prepared.can.controllers[g.source].tx_processing_ps,
                        self.prepared.can.controllers[g.source].tx_channel_ps,
                    )?,
                )?;
                let request = self.create_can(
                    d,
                    g.source,
                    g.frame.clone(),
                    id,
                    lineage,
                    None,
                    now,
                    ready,
                    false,
                )?;
                if ready == now {
                    self.can_ready(d, request, now)?;
                } else {
                    schedule(d, ready, 0, BridgeTimer::CanReady { request });
                }
                let next = add(*ordinal, 1)?;
                if let Some(at) = g.schedule.time(next) {
                    if at < u128::from(limit) {
                        schedule(
                            d,
                            at as u64,
                            1,
                            BridgeTimer::NativeCan {
                                generator: *generator,
                                ordinal: next,
                            },
                        );
                    } else {
                        d.future_generation = true;
                    }
                }
            }
            BridgeTimer::CanReady { request } => self.can_ready(d, *request, now)?,
            BridgeTimer::CanEof { request } => {
                let mut r = self.request(d, *request).clone();
                r.row.eof_ps = Some(now);
                r.row.status = "success".into();
                emit_request(d, &r, now)?;
                let bus = self.prepared.can.controller_buses[r.source];
                for (i, c) in self.prepared.can.controllers.iter().enumerate() {
                    if i != r.source
                        && self.prepared.can.controller_buses[i] == bus
                        && can_wire::accepts(&c.rx_filter, &r.frame)
                    {
                        let observed = add(now, c.rx_channel_ps)?;
                        schedule(
                            d,
                            observed,
                            1,
                            BridgeTimer::CanObserve {
                                request: *request,
                                controller: i,
                            },
                        );
                    }
                }
                d.requests.push((*request, r));
            }
            BridgeTimer::CanRelease { request } => {
                let mut r = self.request(d, *request).clone();
                r.row.release_ps = Some(now);
                let bus = self.prepared.can.controller_buses[r.source];
                d.active.insert(bus, None);
                emit_request(d, &r, now)?;
                d.requests.push((*request, r));
                d.actions.push(BridgeAction::DirtyCan { bus });
            }
            BridgeTimer::CanObserve {
                request,
                controller,
            } => {
                let r = self.request(d, *request).clone();
                let c = self
                    .prepared
                    .can
                    .controllers
                    .get(*controller)
                    .ok_or_else(|| error("CAN receive handle out of range"))?;
                let row = Receiver {
                    request_id: r.row.request_id.clone(),
                    receiver: c.id.clone(),
                    status: "pending".into(),
                    observed_ps: Some(now),
                    received_ps: None,
                };
                let i = receiver_index(self, d, &row.request_id, &row.receiver);
                emit_receiver(d, &row, now)?;
                d.receivers.push((i, row));
                let reception_id = format!("{}/{}", r.row.request_id, c.id);
                if let Some(gw) = self
                    .prepared
                    .gateways
                    .iter()
                    .position(|g| g.can_ports.contains(controller))
                {
                    self.ingress(
                        d,
                        gw,
                        &c.id,
                        &reception_id,
                        Some(protocol::from_frame(&r.frame)?),
                        r.lineage.clone(),
                        None,
                        now,
                        tx,
                    )?;
                }
                let received = add(now, c.rx_processing_ps)?;
                if received == now {
                    self.can_received(d, *request, *controller, now)?;
                } else {
                    schedule(
                        d,
                        received,
                        0,
                        BridgeTimer::CanReceived {
                            request: *request,
                            controller: *controller,
                        },
                    );
                }
            }
            BridgeTimer::CanReceived {
                request,
                controller,
            } => self.can_received(d, *request, *controller, now)?,
            BridgeTimer::ConversionReady { conversion } => self.ready(d, *conversion, now, tx)?,
            BridgeTimer::BranchOffer { conversion, branch } => {
                self.offer(d, *conversion, *branch, now, tx)?
            }
            BridgeTimer::RetryCan { source } => self.retry(d, Some(*source), None, now, tx)?,
            BridgeTimer::RetryEthernet { source } => self.retry(d, None, Some(*source), now, tx)?,
        }
        Ok(())
    }
    fn can_ready(&self, d: &mut BridgeDelta, i: usize, now: u64) -> Result<()> {
        let mut r = self.request(d, i).clone();
        r.row.ready_ps = Some(now);
        let capacity = self.prepared.can.controllers[r.source].queue_capacity;
        if self.queue(d, r.source).waiting.len() as u64 >= capacity {
            r.row.status = "dropped".into();
        } else {
            r.row.status = "pending".into();
            r.row.tx_enqueued_ps = Some(now);
            self.queue_mut(d, r.source).waiting.push_back(i);
            d.actions.push(BridgeAction::DirtyCan {
                bus: self.prepared.can.controller_buses[r.source],
            });
        }
        emit_request(d, &r, now)?;
        d.requests.push((i, r));
        Ok(())
    }
    fn can_received(
        &self,
        d: &mut BridgeDelta,
        request: usize,
        controller: usize,
        now: u64,
    ) -> Result<()> {
        let r = self.request(d, request).clone();
        let c = &self.prepared.can.controllers[controller];
        let row = Receiver {
            request_id: r.row.request_id.clone(),
            receiver: c.id.clone(),
            status: "received".into(),
            observed_ps: Some(
                r.row
                    .eof_ps
                    .ok_or_else(|| error("received before CAN EOF"))?
                    .checked_add(c.rx_channel_ps)
                    .ok_or_else(|| error("CAN arrival overflow"))?,
            ),
            received_ps: Some(now),
        };
        let i = receiver_index(self, d, &row.request_id, &row.receiver);
        emit_receiver(d, &row, now)?;
        d.receivers.push((i, row));
        if !self
            .prepared
            .gateways
            .iter()
            .any(|g| g.can_ports.contains(&controller))
        {
            self.segment_received(
                d,
                &r.lineage,
                &c.id,
                &format!("{}/{}", r.row.request_id, c.id),
                now,
            )?;
        }
        Ok(())
    }
    fn retry(
        &self,
        d: &mut BridgeDelta,
        can: Option<usize>,
        eth: Option<usize>,
        now: u64,
        tx: &BridgeTxView,
    ) -> Result<()> {
        let mut waiting = Vec::new();
        for i in 0..self.conversion_len(d) {
            let c = self.conversion(d, i);
            if c.row.ready_ps.is_none() {
                continue;
            }
            for (b, branch) in c.branches.iter().enumerate() {
                let matches = match branch.egress {
                    BridgeEgress::Can { source, .. } => can == Some(source),
                    BridgeEgress::Ethernet { source, .. } => eth == Some(source),
                };
                if matches && branch.row.status == "waiting" && branch.row.planned_offer_ps <= now {
                    waiting.push((
                        c.row.ready_ps.unwrap(),
                        c.row.origin_id.clone(),
                        c.row.conversion_id.clone(),
                        branch.row.egress.clone(),
                        i,
                        b,
                    ));
                }
            }
        }
        waiting.sort();
        for (_, _, _, _, c, b) in waiting {
            self.offer(d, c, b, now, tx)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn plan_can_arbitration(
        &self,
        bus: usize,
        now: u64,
        tx: &BridgeTxView,
    ) -> Result<BridgeDelta> {
        let mut d = self.delta();
        self.plan_can_arbitration_in_delta(&mut d, bus, now, tx)?;
        Ok(d)
    }
    pub(crate) fn plan_can_arbitration_in_delta(
        &self,
        d: &mut BridgeDelta,
        bus: usize,
        now: u64,
        tx: &BridgeTxView,
    ) -> Result<()> {
        if bus >= self.active.len() {
            return Err(error("CAN bus handle out of range"));
        }
        if d.active
            .get(&bus)
            .copied()
            .unwrap_or(self.active[bus])
            .is_some()
        {
            return Ok(());
        }
        let mut candidates = Vec::new();
        for source in 0..self.queues.len() {
            let q = self.queue(d, source);
            if self.prepared.can.controller_buses[source] == bus {
                if let Some((position, i)) =
                    q.waiting.iter().enumerate().min_by(|(_, a), (_, b)| {
                        let a = self.request(d, **a);
                        let b = self.request(d, **b);
                        (&a.wire.arbitration, a.row.generated_ps, &a.row.request_id).cmp(&(
                            &b.wire.arbitration,
                            b.row.generated_ps,
                            &b.row.request_id,
                        ))
                    })
                {
                    let r = self.request(d, *i);
                    candidates.push((
                        r.wire.arbitration.clone(),
                        r.row.tx_enqueued_ps.unwrap(),
                        self.prepared.can.controllers[source].id.clone(),
                        *i,
                        source,
                        position,
                    ));
                }
            }
        }
        candidates.sort();
        if let Some((arb, _, _, i, source, position)) = candidates.first() {
            if candidates.get(1).is_some_and(|c| c.0 == *arb) {
                return Err(error("simultaneous CAN identifier collision"));
            }
            let mut r = self.request(d, *i).clone();
            let eof = can_wire::end_time(now, r.row.frame_bits, r.row.bitrate_bps)?;
            let release = can_wire::end_time(now, add(r.row.frame_bits, 3)?, r.row.bitrate_bps)?;
            self.queue_mut(d, *source).waiting.remove(*position);
            d.active.insert(bus, Some(*i));
            r.row.status = "in_flight".into();
            r.row.sof_ps = Some(now);
            r.row.planned_eof_ps = Some(eof);
            r.row.planned_release_ps = Some(release);
            emit_request(d, &r, now)?;
            if let Some(id) = &r.branch_id {
                self.branch_sof(d, id, now)?;
            }
            let targets = self
                .prepared
                .can
                .controllers
                .iter()
                .enumerate()
                .filter(|(c, v)| {
                    *c != *source
                        && self.prepared.can.controller_buses[*c] == bus
                        && can_wire::accepts(&v.rx_filter, &r.frame)
                        && !self
                            .prepared
                            .gateways
                            .iter()
                            .any(|g| g.can_ports.contains(c))
                })
                .map(|(_, c)| c.id.clone())
                .collect::<Vec<_>>();
            self.segment_sof(d, &r.lineage, &r.row.request_id, &targets, now)?;
            d.requests.push((*i, r));
            schedule(d, eof, 0, BridgeTimer::CanEof { request: *i });
            schedule(d, release, 0, BridgeTimer::CanRelease { request: *i });
            schedule(d, now, 1, BridgeTimer::RetryCan { source: *source });
            let _ = tx;
        }
        Ok(())
    }
    fn branch_sof(&self, d: &mut BridgeDelta, branch_id: &str, now: u64) -> Result<()> {
        for i in 0..self.conversion_len(d) {
            let c = self.conversion(d, i);
            if let Some(branch) = c.branches.iter().position(|b| b.row.branch_id == branch_id) {
                let mut c = c.clone();
                if c.branches[branch].row.sof_ps.is_none() {
                    c.branches[branch].row.sof_ps = Some(now);
                    self.stage_conversion(d, i, c, now)?;
                }
                return Ok(());
            }
        }
        Err(error("unknown composite branch SOF"))
    }
    fn segment_sof(
        &self,
        d: &mut BridgeDelta,
        lineage: &BridgeLineage,
        source_record: &str,
        targets: &[String],
        now: u64,
    ) -> Result<()> {
        if segment_index(self, d, &lineage.segment_id).is_some() {
            return Ok(());
        }
        let owned: BTreeSet<_> = self
            .prepared
            .gateways
            .iter()
            .flat_map(|g| {
                g.can_ports
                    .iter()
                    .map(|c| self.prepared.can.controllers[*c].id.clone())
                    .chain(std::iter::once(
                        self.prepared.ethernet.devices[g.ethernet_endpoint]
                            .id
                            .clone(),
                    ))
            })
            .collect();
        let targets: BTreeSet<_> = targets
            .iter()
            .filter(|t| !owned.contains(*t))
            .cloned()
            .collect();
        let row = SegmentRecord {
            segment_id: lineage.segment_id.clone(),
            origin_id: lineage.origin_id.clone(),
            source_record_id: source_record.into(),
            branch_lineage: lineage.branch_lineage.clone(),
            sof_ps: now,
            target_count: targets.len() as u64,
            targets: targets
                .into_iter()
                .map(|terminal_id| SegmentTarget {
                    origin_id: lineage.origin_id.clone(),
                    segment_id: lineage.segment_id.clone(),
                    branch_lineage: lineage.branch_lineage.clone(),
                    terminal_id,
                    completed_ps: None,
                    completion_reception_id: None,
                })
                .collect(),
        };
        let i = d
            .segments
            .iter()
            .map(|(i, _)| i + 1)
            .max()
            .unwrap_or(self.segments.len())
            .max(self.segments.len());
        record(
            d,
            "dir.can_ethernet.segment",
            row.segment_id.clone(),
            now,
            &row,
        )?;
        d.segments.push((i, row));
        Ok(())
    }
    fn segment_received(
        &self,
        d: &mut BridgeDelta,
        lineage: &BridgeLineage,
        terminal: &str,
        reception: &str,
        now: u64,
    ) -> Result<()> {
        let Some(i) = segment_index(self, d, &lineage.segment_id) else {
            return Err(error("terminal completion references segment before SOF"));
        };
        let mut s = d
            .segments
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, s)| s)
            .unwrap_or_else(|| &self.segments[i])
            .clone();
        if s.origin_id != lineage.origin_id || s.branch_lineage != lineage.branch_lineage {
            return Err(error("segment lineage mismatch"));
        }
        if let Some(target) = s.targets.iter_mut().find(|t| t.terminal_id == terminal) {
            if target.completed_ps.is_none() {
                if now < lineage.generated_ps {
                    return Err(error("negative end-to-end latency"));
                }
                target.completed_ps = Some(now);
                target.completion_reception_id = Some(reception.into());
                record(d, "dir.can_ethernet.segment", s.segment_id.clone(), now, &s)?;
                d.segments.push((i, s));
            }
        }
        Ok(())
    }
    /// Only capacities change here; logical state and records remain uncommitted.
    pub(crate) fn reserve(&mut self, d: &BridgeDelta) -> Result<()> {
        reserve_updates(&mut self.conversions, &d.conversions)?;
        reserve_updates(&mut self.requests, &d.requests)?;
        reserve_updates(&mut self.receivers, &d.receivers)?;
        reserve_updates(&mut self.segments, &d.segments)
    }
    pub(crate) fn apply(&mut self, d: BridgeDelta) {
        self.future_generation = d.future_generation;
        apply_updates(&mut self.conversions, d.conversions);
        apply_updates(&mut self.requests, d.requests);
        apply_updates(&mut self.receivers, d.receivers);
        apply_updates(&mut self.segments, d.segments);
        for (i, q) in d.queues {
            self.queues[i] = q;
        }
        for (i, v) in d.active {
            self.active[i] = v;
        }
        for (i, v) in d.rx {
            self.rx[i] = v;
        }
        for (i, v) in d.rx_max {
            self.rx_max[i] = v;
        }
    }
    pub(crate) fn can_queue_occupancy_after(&self, d: &BridgeDelta) -> Vec<(String, u64)> {
        self.prepared
            .can
            .controllers
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    format!("{}.txQueue", c.id),
                    self.queue(d, i).waiting.len() as u64,
                )
            })
            .collect()
    }
    pub(crate) fn can_snapshot(&self) -> crate::snapshot::CanSnapshot {
        let bus_states = self
            .active
            .iter()
            .map(|r| if r.is_some() { "transmitting" } else { "idle" }.to_owned())
            .collect::<Vec<_>>();
        crate::snapshot::CanSnapshot {
            archive: None,
            bus_state: bus_states.first().cloned().unwrap_or_else(|| "idle".into()),
            bus_states,
            requests: self.requests.iter().map(|r| r.row.clone()).collect(),
            receivers: self.receivers.clone(),
        }
    }
    pub(crate) fn snapshot(&self) -> Value {
        json!({"conversions":self.conversions.iter().map(|c|&c.row).collect::<Vec<_>>(),"branches":self.conversions.iter().flat_map(|c|c.branches.iter().map(|b|&b.row)).collect::<Vec<_>>(),"segments":self.segments,"rx_occupancy":self.rx,"rx_max":self.rx_max,"can_requests":self.requests.iter().map(|r|&r.row).collect::<Vec<_>>(),"can_receivers":self.receivers})
    }
}
fn schedule(d: &mut BridgeDelta, at: u64, phase: u8, event: BridgeTimer) {
    d.actions.push(BridgeAction::Schedule { at, phase, event });
}
fn record<T: Serialize>(
    d: &mut BridgeDelta,
    schema: &'static str,
    id: String,
    now: u64,
    value: &T,
) -> Result<()> {
    d.actions.push(BridgeAction::Record {
        schema,
        record_id: id,
        time_ps: now,
        data: serde_json::to_value(value).map_err(|e| error(e.to_string()))?,
    });
    Ok(())
}
fn receiver_index(s: &BridgeState, d: &BridgeDelta, request: &str, receiver: &str) -> usize {
    d.receivers
        .iter()
        .rev()
        .find(|(_, r)| r.request_id == request && r.receiver == receiver)
        .map(|(i, _)| *i)
        .or_else(|| {
            s.receivers
                .iter()
                .position(|r| r.request_id == request && r.receiver == receiver)
        })
        .unwrap_or_else(|| {
            d.receivers
                .iter()
                .map(|(i, _)| i + 1)
                .max()
                .unwrap_or(s.receivers.len())
                .max(s.receivers.len())
        })
}
fn segment_index(s: &BridgeState, d: &BridgeDelta, id: &str) -> Option<usize> {
    d.segments
        .iter()
        .rev()
        .find(|(_, r)| r.segment_id == id)
        .map(|(i, _)| *i)
        .or_else(|| s.segments.iter().position(|r| r.segment_id == id))
}
fn reserve_updates<T>(values: &mut Vec<T>, updates: &[(usize, T)]) -> Result<()> {
    let len = updates
        .iter()
        .map(|(i, _)| i + 1)
        .max()
        .unwrap_or(values.len())
        .max(values.len());
    values
        .try_reserve(len - values.len())
        .map_err(|_| error("composite state allocation failed"))
}
fn apply_updates<T>(values: &mut Vec<T>, updates: Vec<(usize, T)>) {
    for (i, value) in updates {
        if i == values.len() {
            values.push(value);
        } else {
            values[i] = value;
        }
    }
}
fn emit_receiver(d: &mut BridgeDelta, r: &Receiver, now: u64) -> Result<()> {
    record(
        d,
        "can.receiver",
        format!("{}/{}", r.request_id, r.receiver),
        now,
        &json!({"request_id":r.request_id,"receiver":r.receiver,"status":r.status,"observed_ps":opt(r.observed_ps),"received_ps":opt(r.received_ps)}),
    )
}
fn opt(n: Option<u64>) -> Value {
    n.map(|n| json!(n.to_string())).unwrap_or(Value::Null)
}
fn emit_request(d: &mut BridgeDelta, r: &CanRequest, now: u64) -> Result<()> {
    let row = &r.row;
    let data = json!({"request_id":row.request_id,"source":row.source,"bus":row.bus,"status":row.status,"generated_ps":row.generated_ps.to_string(),"ready_ps":opt(row.ready_ps),"sof_ps":opt(row.sof_ps),"eof_ps":opt(row.eof_ps),"payload_bits":row.payload_bits.to_string(),"serialized_bits":row.frame_bits.to_string(),"attempts":if row.sof_ps.is_some(){"1"}else{"0"},"retries":"0","drop_reason":if row.status=="dropped"{Some("queue_full")}else{None},"model_fields":{"profile":"can.cc.multibus.v1","schema_version":1,"crc15":row.crc15.to_string(),"stuff_bits":row.stuff_bits.to_string(),"frame_bits":row.frame_bits.to_string(),"intermission_bits":"3","bitrate_bps":row.bitrate_bps.to_string(),"planned_eof_ps":opt(row.planned_eof_ps),"planned_release_ps":opt(row.planned_release_ps),"release_ps":opt(row.release_ps),"origin_request_id":row.request_id,"parent_request_id":Value::Null,"gw_hops":"0","tx_enqueued_ps":opt(row.tx_enqueued_ps)}});
    d.actions.push(BridgeAction::Record {
        schema: "can.request",
        record_id: row.request_id.clone(),
        time_ps: now,
        data,
    });
    Ok(())
}

impl BridgeDelta {
    pub(crate) fn clear_ethernet_reservations(&mut self) {
        self.ethernet_reserved.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Bus, Controller, Generator, PreparedCan, Schedule, ethernet::*};
    fn fixture(reverse: bool) -> PreparedCanEthernet {
        let can = PreparedCan {
            bus_id: "Net.bus".into(),
            bitrate: 500_000,
            buses: vec![Bus {
                id: "Net.bus".into(),
                bitrate: 500_000,
            }],
            controller_buses: vec![0, 0],
            controllers: vec![
                Controller {
                    id: "Net.src".into(),
                    queue_capacity: 64,
                    tx_processing_ps: 0,
                    rx_processing_ps: if reverse { 5_000_000 } else { 0 },
                    rx_filter: "*".into(),
                    tx_channel_ps: 0,
                    rx_channel_ps: 0,
                },
                Controller {
                    id: "Net.gw.can".into(),
                    queue_capacity: 64,
                    tx_processing_ps: if reverse { 4_000_000 } else { 0 },
                    rx_processing_ps: if reverse { 0 } else { 1_000_000 },
                    rx_filter: "*".into(),
                    tx_channel_ps: 0,
                    rx_channel_ps: 0,
                },
            ],
            generators: if reverse {
                vec![]
            } else {
                vec![Generator {
                    id: "can_source".into(),
                    source: 0,
                    frame: Frame {
                        format: "standard".into(),
                        id: 0,
                        data: String::new(),
                    },
                    schedule: Schedule::Explicit(vec![0]),
                }]
            },
        };
        let devices = vec![
            EthernetDevice {
                id: "Net.gw.eth".into(),
                kind: "endpoint".into(),
                mac: Some("02:00:00:00:00:01".into()),
                queue_capacity: 64,
                tx_processing_delay_ps: if reverse { 0 } else { 3_000_000 },
                rx_processing_delay_ps: if reverse { 2_000_000 } else { 0 },
                forward_delay_ps: 0,
                fdb: BTreeMap::new(),
                vlan_fdb: BTreeMap::new(),
                multicast: BTreeMap::new(),
                subscriptions: BTreeSet::new(),
                unknown_multicast: "flood".into(),
            },
            EthernetDevice {
                id: "Net.sink".into(),
                kind: "endpoint".into(),
                mac: Some("02:00:00:00:00:02".into()),
                queue_capacity: 64,
                tx_processing_delay_ps: 0,
                rx_processing_delay_ps: if reverse { 0 } else { 5_000_000 },
                forward_delay_ps: 0,
                fdb: BTreeMap::new(),
                vlan_fdb: BTreeMap::new(),
                multicast: BTreeMap::new(),
                subscriptions: BTreeSet::new(),
                unknown_multicast: "flood".into(),
            },
        ];
        let ethernet = PreparedEthernet {
            directions: vec![
                EthernetDirection {
                    channel_id: "forward".into(),
                    from_port: "Net.gw.eth.tx".into(),
                    to_port: "Net.sink.rx".into(),
                    source: 0,
                    destination: 1,
                    bitrate_bps: 1_000_000_000,
                    delay_ps: if reverse { 1_000_000 } else { 4_000_000 },
                },
                EthernetDirection {
                    channel_id: "reverse".into(),
                    from_port: "Net.sink.tx".into(),
                    to_port: "Net.gw.eth.rx".into(),
                    source: 1,
                    destination: 0,
                    bitrate_bps: 1_000_000_000,
                    delay_ps: if reverse { 1_000_000 } else { 4_000_000 },
                },
            ],
            port_policies: devices
                .iter()
                .enumerate()
                .map(|(i, d)| EthernetPortPolicy {
                    port: format!("{}.tx", d.id),
                    ingress: format!("{}.rx", d.id),
                    device: i,
                    pvid: 10,
                    admit: "all".into(),
                    default_priority: 3,
                    vlans: BTreeMap::from([(10, reverse)]),
                })
                .collect(),
            devices,
            generators: vec![],
            outputs: vec![],
            media: None,
        };
        PreparedCanEthernet {
            can,
            ethernet,
            gateways: vec![BridgeGateway {
                instance: "Net.gw".into(),
                can_ports: vec![1],
                ethernet_endpoint: 0,
                rx_capacity: 2,
                conversion_delay_ps: if reverse { 3_000_000 } else { 2_000_000 },
                max_hops: 8,
                rules: vec![
                    BridgeRule {
                        id: "to_eth".into(),
                        direction: "can_to_ethernet".into(),
                        ingress: "Net.gw.can".into(),
                        format: CanFormat::Standard,
                        can_id: 0,
                        vid: None,
                        pcp: None,
                        egresses: vec![BridgeEgress::Ethernet {
                            source: 0,
                            port: "Net.gw.eth.tx".into(),
                            dst_mac: "02:00:00:00:00:02".into(),
                            vid: 10,
                            pcp: 3,
                        }],
                    },
                    BridgeRule {
                        id: "to_can".into(),
                        direction: "ethernet_to_can".into(),
                        ingress: "Net.gw.eth".into(),
                        format: CanFormat::Standard,
                        can_id: 0,
                        vid: Some(10),
                        pcp: Some(3),
                        egresses: vec![BridgeEgress::Can {
                            source: 1,
                            port: "Net.gw.can.tx".into(),
                            format: CanFormat::Standard,
                            can_id: 0,
                        }],
                    },
                ],
            }],
        }
    }
    fn tx() -> BridgeTxView {
        BridgeTxView {
            ethernet: BTreeMap::from([(
                (0, 3),
                TxQueueView {
                    capacity_frames: 64,
                    capacity_bytes: None,
                    queued_frames: 0,
                    queued_bytes: 0,
                    port_capacity_frames: 64,
                    port_queued_frames: 0,
                },
            )]),
        }
    }
    fn commit(s: &mut BridgeState, d: BridgeDelta) {
        s.reserve(&d).unwrap();
        s.apply(d);
    }
    fn tick(s: &mut BridgeState, timer: BridgeTimer, now: u64) {
        let d = s
            .plan(&BridgeEvent::Timer(timer), now, &tx(), 1_000_000_000)
            .unwrap();
        commit(s, d);
    }
    fn ingress(lineage: Option<BridgeLineage>) -> BridgeEvent {
        BridgeEvent::EthernetIngress {
            endpoint: 0,
            ingress_record: "eth_source/v1/c1".into(),
            origin_id: "eth_source".into(),
            generated_ps: 0,
            wire: ethernet::serialize_vlan_frame(
                "02:00:00:00:00:02",
                "02:00:00:00:00:01",
                protocol::ETHERTYPE,
                "4449524301000000000000",
                Some(EthernetVlanTag {
                    vid: 10,
                    pcp: 3,
                    dei: 0,
                }),
            )
            .unwrap(),
            vid: 10,
            priority: 3,
            lineage,
        }
    }
    #[test]
    fn r01_exact_store_and_forward_and_rx_hold() {
        let p = fixture(false);
        let mut s = BridgeState::new(&p).unwrap();
        tick(
            &mut s,
            BridgeTimer::NativeCan {
                generator: 0,
                ordinal: 0,
            },
            0,
        );
        let d = s.plan_can_arbitration(0, 0, &tx()).unwrap();
        commit(&mut s, d);
        tick(&mut s, BridgeTimer::CanEof { request: 0 }, 100_000_000);
        tick(
            &mut s,
            BridgeTimer::CanObserve {
                request: 0,
                controller: 1,
            },
            100_000_000,
        );
        assert_eq!(s.rx[0], 1);
        assert_eq!(s.conversions[0].row.planned_ready_ps, Some(103_000_000));
        tick(
            &mut s,
            BridgeTimer::ConversionReady { conversion: 0 },
            103_000_000,
        );
        assert_eq!(s.rx[0], 1);
        let d = s
            .plan(
                &BridgeEvent::Timer(BridgeTimer::BranchOffer {
                    conversion: 0,
                    branch: 0,
                }),
                106_000_000,
                &tx(),
                1_000_000_000,
            )
            .unwrap();
        let injection = d
            .actions
            .iter()
            .find_map(|a| {
                if let BridgeAction::InjectEthernet { wire, .. } = a {
                    Some(wire)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(injection.mac_bytes, 64);
        assert_eq!(injection.data_hex, "4449524301000000000000");
        commit(&mut s, d);
        assert_eq!(s.rx[0], 0);
        assert_eq!(s.conversions[0].row.released_ps, Some(106_000_000));
        let b = s.conversions[0].branches[0].clone();
        let event = BridgeEvent::EthernetSof {
            branch_id: Some(b.row.branch_id.clone()),
            lineage: b.lineage.clone(),
            source_record_id: b.row.child_id.clone().unwrap(),
            targets: vec!["Net.sink".into()],
        };
        let d = s.plan(&event, 106_000_000, &tx(), 1_000_000_000).unwrap();
        commit(&mut s, d);
        let d = s
            .plan(
                &BridgeEvent::TerminalReceived {
                    lineage: b.lineage,
                    terminal_id: "Net.sink".into(),
                    reception_id: "sink-rx".into(),
                },
                115_576_000,
                &tx(),
                1_000_000_000,
            )
            .unwrap();
        commit(&mut s, d);
        assert_eq!(s.segments[1].targets[0].completed_ps, Some(115_576_000));
    }
    #[test]
    fn r02_exact_timing_native_can_and_terminal_completion() {
        let p = fixture(true);
        let mut s = BridgeState::new(&p).unwrap();
        let d = s
            .plan(&ingress(None), 1_608_000, &tx(), 1_000_000_000)
            .unwrap();
        commit(&mut s, d);
        tick(
            &mut s,
            BridgeTimer::ConversionReady { conversion: 0 },
            6_608_000,
        );
        tick(
            &mut s,
            BridgeTimer::BranchOffer {
                conversion: 0,
                branch: 0,
            },
            10_608_000,
        );
        assert_eq!(s.rx[0], 0);
        let d = s.plan_can_arbitration(0, 10_608_000, &tx()).unwrap();
        commit(&mut s, d);
        assert_eq!(s.requests[0].row.planned_eof_ps, Some(110_608_000));
        assert_eq!(s.requests[0].row.planned_release_ps, Some(116_608_000));
        assert_eq!(s.segments[0].target_count, 1);
        assert!(s.segments[0].targets[0].completed_ps.is_none());
        tick(&mut s, BridgeTimer::CanEof { request: 0 }, 110_608_000);
        tick(
            &mut s,
            BridgeTimer::CanObserve {
                request: 0,
                controller: 0,
            },
            110_608_000,
        );
        assert!(s.segments[0].targets[0].completed_ps.is_none());
        tick(
            &mut s,
            BridgeTimer::CanReceived {
                request: 0,
                controller: 0,
            },
            115_608_000,
        );
        assert_eq!(s.segments[0].targets[0].completed_ps, Some(115_608_000));
    }
    #[test]
    fn rx_capacity_and_loop_hop_rule_order() {
        let mut p = fixture(true);
        p.gateways[0].rx_capacity = 0;
        let s = BridgeState::new(&p).unwrap();
        let d = s.plan(&ingress(None), 1, &tx(), 100_000).unwrap();
        assert_eq!(d.conversions[0].1.row.reason.as_deref(), Some("rx_full"));
        let l = BridgeLineage {
            origin_id: "origin".into(),
            parent_id: "parent".into(),
            visited_gateways: vec!["Net.gw".into()],
            branch_lineage: vec![],
            segment_id: "segment".into(),
            generated_ps: 0,
        };
        let d = s
            .plan(&ingress(Some(l.clone())), 1, &tx(), 100_000)
            .unwrap();
        assert_eq!(
            d.conversions[0].1.row.reason.as_deref(),
            Some("loop_prevented")
        );
        p.gateways[0].max_hops = 1;
        let s = BridgeState::new(&p).unwrap();
        let d = s
            .plan(
                &ingress(Some(BridgeLineage {
                    visited_gateways: vec!["other".into()],
                    ..l
                })),
                1,
                &tx(),
                100_000,
            )
            .unwrap();
        assert_eq!(d.conversions[0].1.row.reason.as_deref(), Some("hop_limit"));
    }
    #[test]
    fn staged_multiple_ingress_capacity_and_error_atomicity() {
        let p = fixture(true);
        let s = BridgeState::new(&p).unwrap();
        let mut d = s.empty_delta();
        for i in 0..3 {
            let mut e = ingress(None);
            if let BridgeEvent::EthernetIngress {
                ingress_record,
                origin_id,
                ..
            } = &mut e
            {
                *ingress_record = format!("rx{i}");
                *origin_id = format!("origin{i}");
            }
            s.plan_in_delta(&mut d, &e, 100, &tx(), 1_000_000_000)
                .unwrap();
        }
        assert_eq!(d.rx[&0], 2);
        assert_eq!(
            d.conversions.last().unwrap().1.row.reason.as_deref(),
            Some("rx_full")
        );
        assert_eq!(s.rx[0], 0);
        assert!(s.conversions.is_empty());
        let before = s.snapshot();
        assert!(s.plan(&ingress(None), u64::MAX, &tx(), u64::MAX).is_err());
        assert_eq!(s.snapshot(), before);
    }
    #[test]
    fn zero_delay_admits_and_unadmittable_terminates() {
        let mut p = fixture(true);
        p.gateways[0].conversion_delay_ps = 0;
        p.ethernet.devices[0].rx_processing_delay_ps = 0;
        p.can.controllers[1].tx_processing_ps = 0;
        p.can.controllers[1].queue_capacity = 0;
        let mut s = BridgeState::new(&p).unwrap();
        let d = s.plan(&ingress(None), 0, &tx(), 1_000).unwrap();
        commit(&mut s, d);
        assert_eq!(s.conversions[0].row.status, "released");
        assert_eq!(s.conversions[0].row.released_ps, Some(0));
        assert_eq!(
            s.conversions[0].branches[0].row.reason.as_deref(),
            Some("tx_unadmittable")
        );
        assert!(s.requests.is_empty());
        assert_eq!(s.rx[0], 0);
    }
    #[test]
    fn segment_denominator_freezes_and_duplicate_completion_does_not_increment() {
        let p = fixture(true);
        let mut s = BridgeState::new(&p).unwrap();
        let l = BridgeLineage {
            origin_id: "origin".into(),
            parent_id: "parent".into(),
            visited_gateways: vec![],
            branch_lineage: vec![],
            segment_id: "native".into(),
            generated_ps: 0,
        };
        let event = BridgeEvent::EthernetSof {
            branch_id: None,
            lineage: l.clone(),
            source_record_id: "frame".into(),
            targets: vec!["A".into(), "B".into(), "Net.gw.eth".into(), "A".into()],
        };
        let d = s.plan(&event, 10, &tx(), 100).unwrap();
        commit(&mut s, d);
        assert_eq!(s.segments[0].target_count, 2);
        for _ in 0..2 {
            let d = s
                .plan(
                    &BridgeEvent::TerminalReceived {
                        lineage: l.clone(),
                        terminal_id: "A".into(),
                        reception_id: "rx-A".into(),
                    },
                    20,
                    &tx(),
                    100,
                )
                .unwrap();
            commit(&mut s, d);
        }
        assert_eq!(
            s.segments[0]
                .targets
                .iter()
                .filter(|t| t.completed_ps.is_some())
                .count(),
            1
        );
        assert_eq!(s.segments[0].targets[1].completed_ps, None);
    }
    fn fanout_fixture() -> PreparedCanEthernet {
        let mut p = fixture(true);
        p.can.buses.push(Bus {
            id: "Net.bus_b".into(),
            bitrate: 250_000,
        });
        let mut owned = p.can.controllers[1].clone();
        owned.id = "Net.gw.can_b".into();
        owned.queue_capacity = 1;
        p.can.controllers.push(owned);
        p.can.controller_buses.push(1);
        let mut terminal = p.can.controllers[0].clone();
        terminal.id = "Net.sink_b".into();
        p.can.controllers.push(terminal);
        p.can.controller_buses.push(1);
        p.gateways[0].can_ports.push(2);
        p.gateways[0].rules[1].egresses.push(BridgeEgress::Can {
            source: 2,
            port: "Net.gw.can_b.tx".into(),
            format: CanFormat::Standard,
            can_id: 0,
        });
        p
    }
    #[test]
    fn acceptance_last_can_branch_id_failure_preserves_processing_prefix() {
        let mut p = fanout_fixture();
        p.can.controllers[1].tx_processing_ps = 0;
        p.can.controllers[2].tx_processing_ps = 0;
        let mut s = BridgeState::new(&p).unwrap();
        let d = s.plan(&ingress(None), 1_608_000, &tx(), u64::MAX).unwrap();
        commit(&mut s, d);
        // Private fault injection after the successful ingress callback. The first
        // child can stage successfully before the final branch rejects its ID.
        if let BridgeEgress::Can { can_id, .. } = &mut s.conversions[0].branches[1].egress {
            *can_id = 2048;
        } else {
            panic!("expected final CAN branch");
        }
        let before = s.snapshot();
        assert!(
            s.plan(
                &BridgeEvent::Timer(BridgeTimer::ConversionReady { conversion: 0 }),
                6_608_000,
                &tx(),
                u64::MAX
            )
            .is_err()
        );
        assert_eq!(s.snapshot(), before);
        assert_eq!(s.conversions[0].row.ready_ps, None);
        assert_eq!(s.conversions[0].row.status, "processing");
        assert!(s.requests.is_empty());
        assert!(s.queues.iter().all(|q| q.waiting.is_empty()));
        assert_eq!(s.rx, [1]);
    }

    #[test]
    fn acceptance_last_ethernet_branch_byte_overflow_does_not_commit_first_branch() {
        let mut p = fixture(false);
        p.ethernet.devices[0].tx_processing_delay_ps = 0;
        let mut second = p.gateways[0].rules[0].egresses[0].clone();
        if let BridgeEgress::Ethernet { pcp, .. } = &mut second {
            *pcp = 4;
        }
        p.gateways[0].rules[0].egresses.push(second);
        let mut s = BridgeState::new(&p).unwrap();
        let mut view = tx();
        view.ethernet.insert((0, 4), view.ethernet[&(0, 3)]);
        let mut d = s.empty_delta();
        s.ingress(
            &mut d,
            0,
            "Net.gw.can",
            "native/Net.gw.can",
            Some(DirCanPacket {
                format: CanFormat::Standard,
                id: 0,
                data: vec![],
            }),
            BridgeLineage::native("can", "native", 0),
            None,
            100_000_000,
            &view,
        )
        .unwrap();
        commit(&mut s, d);
        let before = s.snapshot();
        let mut d = s.empty_delta();
        d.ethernet_reserved.insert((0, 4), (0, u64::MAX));
        assert!(
            s.plan_in_delta(
                &mut d,
                &BridgeEvent::Timer(BridgeTimer::ConversionReady { conversion: 0 }),
                103_000_000,
                &view,
                u64::MAX
            )
            .is_err()
        );
        assert_eq!(s.snapshot(), before);
        assert_eq!(s.conversions[0].row.ready_ps, None);
        assert!(
            s.conversions[0]
                .branches
                .iter()
                .all(|b| b.row.child_id.is_none())
        );
        assert_eq!(s.rx, [1]);
    }

    #[test]
    fn fanout_buses_start_together_with_independent_wire_times() {
        let p = fanout_fixture();
        let mut s = BridgeState::new(&p).unwrap();
        let d = s
            .plan(&ingress(None), 1_608_000, &tx(), 1_000_000_000)
            .unwrap();
        commit(&mut s, d);
        tick(
            &mut s,
            BridgeTimer::ConversionReady { conversion: 0 },
            6_608_000,
        );
        tick(
            &mut s,
            BridgeTimer::BranchOffer {
                conversion: 0,
                branch: 0,
            },
            10_608_000,
        );
        assert_eq!(s.rx[0], 1);
        tick(
            &mut s,
            BridgeTimer::BranchOffer {
                conversion: 0,
                branch: 1,
            },
            10_608_000,
        );
        assert_eq!(s.rx[0], 0);
        let mut d = s.empty_delta();
        s.plan_can_arbitration_in_delta(&mut d, 0, 10_608_000, &tx())
            .unwrap();
        s.plan_can_arbitration_in_delta(&mut d, 1, 10_608_000, &tx())
            .unwrap();
        commit(&mut s, d);
        assert_eq!(s.requests[0].row.planned_eof_ps, Some(110_608_000));
        assert_eq!(s.requests[1].row.planned_eof_ps, Some(210_608_000));
        assert_eq!(s.requests[1].row.planned_release_ps, Some(222_608_000));
        assert_eq!(s.segments.len(), 2);
    }
    #[test]
    fn fanout_wait_keeps_rx_and_retries_only_unadmitted_branch_at_sof() {
        let p = fanout_fixture();
        let mut s = BridgeState::new(&p).unwrap();
        let mut seed = s.empty_delta();
        let lineage = BridgeLineage {
            origin_id: "prior".into(),
            parent_id: "prior".into(),
            visited_gateways: vec![],
            branch_lineage: vec![],
            segment_id: "prior".into(),
            generated_ps: 0,
        };
        s.create_can(
            &mut seed,
            2,
            Frame {
                format: "standard".into(),
                id: 0,
                data: "".into(),
            },
            "prior".into(),
            lineage,
            None,
            0,
            0,
            true,
        )
        .unwrap();
        commit(&mut s, seed);
        let d = s
            .plan(&ingress(None), 1_608_000, &tx(), 1_000_000_000)
            .unwrap();
        commit(&mut s, d);
        tick(
            &mut s,
            BridgeTimer::ConversionReady { conversion: 0 },
            6_608_000,
        );
        tick(
            &mut s,
            BridgeTimer::BranchOffer {
                conversion: 0,
                branch: 0,
            },
            10_608_000,
        );
        tick(
            &mut s,
            BridgeTimer::BranchOffer {
                conversion: 0,
                branch: 1,
            },
            10_608_000,
        );
        assert_eq!(s.rx[0], 1);
        assert_eq!(s.conversions[0].branches[0].row.status, "admitted");
        assert_eq!(s.conversions[0].branches[1].row.status, "waiting");
        let d = s.plan_can_arbitration(1, 20_000_000, &tx()).unwrap();
        assert!(d.actions.iter().any(|a| matches!(
            a,
            BridgeAction::Schedule {
                at: 20_000_000,
                phase: 1,
                event: BridgeTimer::RetryCan { source: 2 }
            }
        )));
        commit(&mut s, d);
        tick(&mut s, BridgeTimer::RetryCan { source: 2 }, 20_000_000);
        assert_eq!(s.rx[0], 0);
        assert_eq!(s.conversions[0].row.released_ps, Some(20_000_000));
        assert_eq!(s.requests.iter().filter(|r| r.source == 1).count(), 1);
        assert_eq!(s.requests.iter().filter(|r| r.source == 2).count(), 2);
        assert_eq!(
            s.conversions[0].branches[0].row.admitted_ps,
            Some(10_608_000)
        );
    }
    #[test]
    fn conversion_upserts_preserve_milestone_times_on_retries_and_late_sof() {
        fn publish(state: &mut BridgeState, delta: BridgeDelta, times: &mut BTreeMap<String, u64>) {
            for action in &delta.actions {
                if let BridgeAction::Record {
                    schema,
                    record_id,
                    time_ps,
                    ..
                } = action
                {
                    if matches!(
                        *schema,
                        "dir.can_ethernet.conversion" | "dir.can_ethernet.branch"
                    ) {
                        times.insert(record_id.clone(), *time_ps);
                    }
                }
            }
            commit(state, delta);
        }
        let mut state = BridgeState::new(&fanout_fixture()).unwrap();
        let mut seed = state.empty_delta();
        state
            .create_can(
                &mut seed,
                2,
                Frame {
                    format: "standard".into(),
                    id: 0,
                    data: "".into(),
                },
                "prior".into(),
                BridgeLineage::native("can", "prior", 0),
                None,
                0,
                0,
                true,
            )
            .unwrap();
        commit(&mut state, seed);
        let mut times = BTreeMap::new();
        let delta = state
            .plan(&ingress(None), 1_608_000, &tx(), 1_000_000_000)
            .unwrap();
        publish(&mut state, delta, &mut times);
        let conversion = state.conversions[0].row.conversion_id.clone();
        let first = state.conversions[0].branches[0].row.branch_id.clone();
        let second = state.conversions[0].branches[1].row.branch_id.clone();
        let delta = state
            .plan(
                &BridgeEvent::Timer(BridgeTimer::ConversionReady { conversion: 0 }),
                6_608_000,
                &tx(),
                1_000_000_000,
            )
            .unwrap();
        publish(&mut state, delta, &mut times);
        for branch in 0..2 {
            let delta = state
                .plan(
                    &BridgeEvent::Timer(BridgeTimer::BranchOffer {
                        conversion: 0,
                        branch,
                    }),
                    10_608_000,
                    &tx(),
                    1_000_000_000,
                )
                .unwrap();
            publish(&mut state, delta, &mut times);
        }
        assert_eq!(times[&conversion], 6_608_000);
        assert_eq!(times[&first], 10_608_000);
        assert_eq!(times[&second], 10_608_000);
        let delta = state
            .plan(
                &BridgeEvent::Timer(BridgeTimer::RetryCan { source: 2 }),
                12_000_000,
                &tx(),
                1_000_000_000,
            )
            .unwrap();
        assert!(
            !delta
                .actions
                .iter()
                .any(|action| matches!(action, BridgeAction::Record { .. }))
        );
        publish(&mut state, delta, &mut times);
        assert_eq!(state.rx[0], 1);
        let mut delta = state.empty_delta();
        state.branch_sof(&mut delta, &first, 15_000_000).unwrap();
        assert_eq!(delta.actions.len(), 1);
        assert!(
            matches!(&delta.actions[0], BridgeAction::Record { schema: "dir.can_ethernet.branch", record_id, .. } if record_id == &first)
        );
        assert_eq!(state.conversions[0].branches[0].row.sof_ps, None);
        publish(&mut state, delta, &mut times);
        assert_eq!(times[&conversion], 6_608_000);
        assert_eq!(times[&first], 15_000_000);
        assert_eq!(times[&second], 10_608_000);
        let delta = state.plan_can_arbitration(1, 20_000_000, &tx()).unwrap();
        publish(&mut state, delta, &mut times);
        let delta = state
            .plan(
                &BridgeEvent::Timer(BridgeTimer::RetryCan { source: 2 }),
                20_000_000,
                &tx(),
                1_000_000_000,
            )
            .unwrap();
        let dto_ids: Vec<_> = delta
            .actions
            .iter()
            .filter_map(|action| {
                if let BridgeAction::Record {
                    schema, record_id, ..
                } = action
                {
                    matches!(
                        *schema,
                        "dir.can_ethernet.conversion" | "dir.can_ethernet.branch"
                    )
                    .then_some(record_id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(dto_ids, [conversion.as_str(), second.as_str()]);
        publish(&mut state, delta, &mut times);
        assert_eq!(state.rx[0], 0);
        assert_eq!(state.conversions[0].row.released_ps, Some(20_000_000));
        assert_eq!(times[&conversion], 20_000_000);
        assert_eq!(times[&first], 15_000_000);
        assert_eq!(times[&second], 20_000_000);
        let mut delta = state.empty_delta();
        state.branch_sof(&mut delta, &second, 25_000_000).unwrap();
        assert_eq!(delta.actions.len(), 1);
        publish(&mut state, delta, &mut times);
        assert_eq!(times[&conversion], 20_000_000);
        assert_eq!(times[&first], 15_000_000);
        assert_eq!(times[&second], 25_000_000);
        assert_eq!(
            state.conversions[0].branches[1].row.sof_ps,
            Some(25_000_000)
        );
    }
    #[test]
    fn same_batch_branch_sof_emits_each_changed_branch_once() {
        let mut state = BridgeState::new(&fanout_fixture()).unwrap();
        let delta = state
            .plan(&ingress(None), 1_608_000, &tx(), 1_000_000_000)
            .unwrap();
        commit(&mut state, delta);
        tick(
            &mut state,
            BridgeTimer::ConversionReady { conversion: 0 },
            6_608_000,
        );
        for branch in 0..2 {
            tick(
                &mut state,
                BridgeTimer::BranchOffer {
                    conversion: 0,
                    branch,
                },
                10_608_000,
            );
        }
        let branches: Vec<_> = state.conversions[0]
            .branches
            .iter()
            .map(|branch| branch.row.branch_id.clone())
            .collect();
        let mut delta = state.empty_delta();
        for branch in &branches {
            state.branch_sof(&mut delta, branch, 20_000_000).unwrap();
        }
        // Duplicate notifications read the staged row, preserving the first SOF.
        state
            .branch_sof(&mut delta, &branches[0], 21_000_000)
            .unwrap();
        assert_eq!(delta.actions.len(), 2);
        let emitted: Vec<_> = delta
            .actions
            .iter()
            .map(|action| match action {
                BridgeAction::Record {
                    schema: "dir.can_ethernet.branch",
                    record_id,
                    time_ps: 20_000_000,
                    ..
                } => record_id.clone(),
                _ => panic!("unchanged conversion or sibling DTO was emitted"),
            })
            .collect();
        assert_eq!(emitted, branches);
        assert!(
            state.conversions[0]
                .branches
                .iter()
                .all(|branch| branch.row.sof_ps.is_none())
        );
        commit(&mut state, delta);
        assert!(
            state.conversions[0]
                .branches
                .iter()
                .all(|branch| branch.row.sof_ps == Some(20_000_000))
        );
        assert_eq!(state.conversions[0].row.released_ps, Some(10_608_000));
    }
    #[test]
    fn shared_ethernet_port_retry_wakes_all_classes_in_stable_order() {
        let mut p = fixture(false);
        p.gateways[0].conversion_delay_ps = 0;
        p.can.controllers[1].rx_processing_ps = 0;
        p.ethernet.devices[0].tx_processing_delay_ps = 0;
        if let BridgeEgress::Ethernet { pcp, .. } = &mut p.gateways[0].rules[0].egresses[0] {
            *pcp = 4;
        }
        let mut other = p.gateways[0].rules[0].clone();
        other.id = "other".into();
        other.can_id = 1;
        if let BridgeEgress::Ethernet { pcp, .. } = &mut other.egresses[0] {
            *pcp = 3;
        }
        p.gateways[0].rules.push(other);
        let mut full = tx();
        full.ethernet.get_mut(&(0, 3)).unwrap().port_capacity_frames = 1;
        full.ethernet.get_mut(&(0, 3)).unwrap().port_queued_frames = 1;
        let class4 = *full.ethernet.get(&(0, 3)).unwrap();
        full.ethernet.insert((0, 4), class4);
        let mut state = BridgeState::new(&p).unwrap();
        let mut delta = state.empty_delta();
        // Reverse insertion order proves retries use ready/origin order across PCPs.
        for (origin, id) in [("z", 0), ("a", 1)] {
            state
                .ingress(
                    &mut delta,
                    0,
                    "Net.gw.can",
                    origin,
                    Some(DirCanPacket {
                        format: CanFormat::Standard,
                        id,
                        data: vec![],
                    }),
                    BridgeLineage::native("can", origin, 0),
                    None,
                    0,
                    &full,
                )
                .unwrap();
        }
        commit(&mut state, delta);
        assert_eq!(state.rx[0], 2);
        assert!(
            state
                .conversions
                .iter()
                .all(|conversion| conversion.branches[0].row.status == "waiting")
        );
        let mut available = full;
        for view in available.ethernet.values_mut() {
            view.port_queued_frames = 0;
        }
        let delta = state
            .plan(
                &BridgeEvent::Timer(BridgeTimer::RetryEthernet { source: 0 }),
                10,
                &available,
                100,
            )
            .unwrap();
        assert_eq!(
            delta
                .actions
                .iter()
                .filter(|action| matches!(action, BridgeAction::InjectEthernet { .. }))
                .count(),
            1
        );
        commit(&mut state, delta);
        assert_eq!(state.conversions[1].branches[0].row.admitted_ps, Some(10));
        assert_eq!(state.conversions[0].branches[0].row.status, "waiting");
        assert_eq!(state.rx[0], 1);
        let delta = state
            .plan(
                &BridgeEvent::Timer(BridgeTimer::RetryEthernet { source: 0 }),
                20,
                &available,
                100,
            )
            .unwrap();
        assert!(
            delta
                .actions
                .iter()
                .any(|action| matches!(action, BridgeAction::InjectEthernet { priority: 4, .. }))
        );
        commit(&mut state, delta);
        assert_eq!(state.conversions[0].branches[0].row.admitted_ps, Some(20));
        assert_eq!(state.rx[0], 0);
        assert!(
            state
                .conversions
                .iter()
                .all(|conversion| conversion.row.status == "released")
        );
        let delta = state
            .plan(
                &BridgeEvent::Timer(BridgeTimer::RetryEthernet { source: 0 }),
                30,
                &available,
                100,
            )
            .unwrap();
        assert!(
            !delta
                .actions
                .iter()
                .any(|action| matches!(action, BridgeAction::InjectEthernet { .. }))
        );
    }
    #[test]
    fn can_controller_queue_uses_can_arbitration_priority() {
        let p = fixture(true);
        let mut s = BridgeState::new(&p).unwrap();
        let mut d = s.empty_delta();
        for id in [2, 1] {
            let name = format!("id{id}");
            let lineage = BridgeLineage {
                origin_id: name.clone(),
                parent_id: name.clone(),
                visited_gateways: vec![],
                branch_lineage: vec![],
                segment_id: name.clone(),
                generated_ps: 0,
            };
            s.create_can(
                &mut d,
                1,
                Frame {
                    format: "standard".into(),
                    id,
                    data: "".into(),
                },
                name,
                lineage,
                None,
                0,
                0,
                true,
            )
            .unwrap();
        }
        commit(&mut s, d);
        let d = s.plan_can_arbitration(0, 0, &tx()).unwrap();
        commit(&mut s, d);
        assert!(s.requests[0].row.sof_ps.is_none());
        assert_eq!(s.requests[1].row.sof_ps, Some(0));
    }
    #[test]
    fn future_native_generation_flag_without_synthesized_rows() {
        let mut p = fixture(false);
        p.can.generators[0].schedule = Schedule::Explicit(vec![100]);
        let mut s = BridgeState::new(&p).unwrap();
        let d = s.initialize(100).unwrap();
        assert!(d.future_generation);
        assert!(d.actions.is_empty());
        commit(&mut s, d);
        assert!(s.future_generation);
        assert!(s.requests.is_empty());
        p.can.generators[0].schedule = Schedule::Explicit(vec![0, 100]);
        let s = BridgeState::new(&p).unwrap();
        let d = s
            .plan(
                &BridgeEvent::Timer(BridgeTimer::NativeCan {
                    generator: 0,
                    ordinal: 0,
                }),
                0,
                &tx(),
                100,
            )
            .unwrap();
        assert!(d.future_generation);
    }
    #[test]
    fn malformed_timer_handles_and_impossible_transition_do_not_mutate() {
        let p = fixture(false);
        let s = BridgeState::new(&p).unwrap();
        let before = s.snapshot();
        assert!(
            s.plan(
                &BridgeEvent::Timer(BridgeTimer::CanEof {
                    request: usize::MAX
                }),
                0,
                &tx(),
                1000
            )
            .is_err()
        );
        assert_eq!(s.snapshot(), before);
        let mut s = s;
        tick(
            &mut s,
            BridgeTimer::NativeCan {
                generator: 0,
                ordinal: 0,
            },
            0,
        );
        let before = s.snapshot();
        assert!(
            s.plan(
                &BridgeEvent::Timer(BridgeTimer::CanEof { request: 0 }),
                100,
                &tx(),
                1000
            )
            .is_err()
        );
        assert_eq!(s.snapshot(), before);
    }
}
