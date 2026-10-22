//! Private immutable planning for composed Ethernet profiles. The registered engine owns the FES.
use super::can_ethernet::{
    BridgeAction, BridgeDelta, BridgeEvent, BridgeState, BridgeTimer, BridgeTxView, TxQueueView,
};
use super::ethernet::{
    dynamic::{DynamicDelta, DynamicState},
    l2,
    tsn::{Head, PortInput, Sending, TsnDelta, TsnState, WakeToken},
};
use crate::registry::{Context, Effect, Envelope, ModelError, ModelRecord, Schema, TimerToken};
use crate::snapshot::Point;
use crate::snapshot::ethernet::*;
use crate::types::can_ethernet::BridgeLineage;
use crate::types::{
    Diagnostic,
    ethernet::dynamic::*,
    ethernet::*,
    network::{PreparedClockAction, PreparedNetwork, ReplicationSource, ReplicationWireFormat},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
pub(crate) mod checkpoint;
pub mod clock;
pub(crate) mod configuration;
mod configured_media;
mod mac_control;
pub(crate) mod multicast_active;
mod phy;
mod phy_boundary;
mod phy_service;
mod provider;
pub(crate) mod provider_l2;
mod qcc;
mod shared_lan;
use shared_lan::{SegmentOwner, SegmentReservation};
mod frer_wire;
mod multicast_integration;
mod preemption;
mod wire;
mod wire_merge;
mod wire_phy;
use wire::{HostEvent, WireHost};
mod replication;
use preemption::{PreemptionState, TransferFragments};
use replication::{Circuit, ReplicationState};
mod distributed;
mod gptp;
pub(crate) mod mac_merge;
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(s: impl Into<String>) -> Diagnostic {
    Diagnostic::execution(s)
}
fn model(e: ModelError) -> Diagnostic {
    error(e.0)
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| {
        let mut e = error("network time/ID overflow");
        e.code = "E-0004".into();
        e
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Event {
    Dispatch,
    Generate(usize, u64),
    Ready(usize),
    Arrival(usize),
    MacMergeArrival {
        direction: usize,
        epoch: u64,
        packet: u64,
    },
    MacMergeTimer {
        direction: usize,
        token: mac_merge::VerifyTimer,
        commit: bool,
    },
    SegmentArrival {
        transfer: usize,
        receiver: usize,
    },
    Complete(usize),
    Bridge(BridgeTimer),
    Control(u64),
    Wire(HostEvent),
    Clock(usize),
    PhyBoundary {
        attachment: usize,
        generation: u64,
        sample_cut: u64,
    },
    Gptp(gptp::state::Event),
    PhySample {
        attachment: usize,
        sample_index: u64,
    },
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    generator: Option<usize>,
    row: EthernetFrameRecord,
    ip: Option<IpMulticast>,
    visits: u64,
    lineage: Option<BridgeLineage>,
    branch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gateway_injection: Option<GatewayInjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    terminal_indications: Vec<TerminalIndication>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayInjection {
    conversion_index: usize,
    branch_index: usize,
    selector: ReplicationSource,
    child_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalIndication {
    reception: usize,
    recovery_round: Option<usize>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    circuit: Option<Circuit>,
    fragments: Option<TransferFragments>,
    row: EthernetTransferRecord,
    frame: usize,
    direction: usize,
    visit: u64,
    offer_epoch: u64,
    sof_epoch: Option<u64>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Reception {
    row: EthernetReceptionRecord,
    transfer: usize,
    visit: u64,
    epoch: u64,
    // Processing delay must not turn Learning ingress into a relay opportunity.
    ingress_no_relay_reason: Option<IngressNoRelayReason>,
    #[serde(default)]
    segment_receiver: Option<usize>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IngressNoRelayReason {
    Learning,
    Discarding,
}
impl IngressNoRelayReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Learning => "stp_learning",
            Self::Discarding => "stp_discarding",
        }
    }
}
struct CopyOrigin {
    circuit: Option<Circuit>,
    frame: usize,
    parent: Option<usize>,
    visit: u64,
}
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Queue {
    waiting: [VecDeque<usize>; 8],
    bytes: [u64; 8],
    active: Option<usize>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum Completion {
    Eof(usize),
    Release(usize),
    Ready(usize),
    Processed(usize),
    Fragment {
        direction: usize,
        transfer: usize,
        epoch: u64,
        id: String,
        payload: u64,
    },
    Gap {
        direction: usize,
        epoch: u64,
        id: String,
    },
    Bridge(BridgeTimer),
    Wire(HostEvent),
    RecoveryTimeout {
        stream: usize,
        generation: u64,
    },
    MulticastTimer,
    MacMerge(mac_merge::integration::MergeEvent),
    ConfiguredMedia {
        domain: usize,
        generation: u64,
    },
    PauseAdmin(usize),
    PauseExpire {
        direction: usize,
        generation: u64,
    },
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Due {
    at: u64,
    kind: Completion,
}
pub(crate) struct NetworkState {
    prepared: Arc<PreparedNetwork>,
    cursors: Vec<u64>,
    frames: Vec<Frame>,
    transfers: Vec<Transfer>,
    receptions: Vec<Reception>,
    queues: Vec<Queue>,
    dynamic: Option<DynamicState>,
    bridge: Option<BridgeState>,
    replication: Option<ReplicationState>,
    preemption: Option<PreemptionState>,
    mac_merge: Option<mac_merge::integration::MergeState>,
    wire: Option<WireHost>,
    multicast_active: Option<multicast_active::Actor>,
    multicast_history: Vec<multicast_integration::MulticastAudit>,
    multicast_cursor: usize,
    multicast_wake: Option<u64>,
    qcc: Option<configuration::QccState>,
    qcc_sources: BTreeMap<usize, qcc::SourceBinding>,
    segment_active: Vec<Option<SegmentReservation>>,
    segment_arrivals: BTreeMap<(usize, usize), u64>,
    provider_fdb: Option<provider_l2::ComponentFdb>,
    tsn: Option<TsnState>,
    clocks: Option<Vec<clock::NodeClock>>,
    gptp: Option<gptp::state::State>,
    clock_cursor: usize,
    phy: Vec<crate::runtime::ethernet::phy::attachment::AttachedMedium>,
    phy_wakes: Vec<Option<u64>>,
    phy_boundaries: Vec<phy_boundary::BoundaryState>,
    phy_arrivals: BTreeMap<usize, u64>,
    phy_admitted: BTreeMap<usize, usize>,
    configured_media: configured_media::State,
    mac_control: mac_control::State,
    phy_services: BTreeMap<usize, super::ethernet::tsn::merge::MacServiceView>,
    tsn_wakes: Vec<Option<WakeToken>>,
    due: Vec<Due>,
    control: Option<(u64, TimerToken)>,
    control_generation: u64,
    record_sequence: u64,
    next_visit: u64,
    future_generation: bool,
    pending_direct: u64,
}
pub(crate) struct NetworkDelta {
    pub(crate) points: Vec<Point>,
    cursors: Option<Vec<u64>>,
    frames: Vec<(usize, Frame)>,
    transfers: Vec<(usize, Transfer)>,
    receptions: Vec<(usize, Reception)>,
    queues: BTreeMap<usize, Queue>,
    dynamic: Option<DynamicDelta>,
    bridge: Option<BridgeDelta>,
    replication: Option<ReplicationState>,
    preemption: Option<PreemptionState>,
    mac_merge: Option<mac_merge::integration::MergeState>,
    wire: Option<WireHost>,
    multicast_active: Option<multicast_active::Actor>,
    multicast_history: Option<Vec<multicast_integration::MulticastAudit>>,
    multicast_cursor: Option<usize>,
    multicast_wake: Option<Option<u64>>,
    qcc: Option<configuration::QccState>,
    qcc_sources: Option<BTreeMap<usize, qcc::SourceBinding>>,
    segment_active: Option<Vec<Option<SegmentReservation>>>,
    segment_arrivals: Option<BTreeMap<(usize, usize), u64>>,
    provider_fdb: Option<provider_l2::ComponentFdb>,
    tsn: TsnDelta,
    clocks: Option<Vec<clock::NodeClock>>,
    gptp: Option<gptp::state::State>,
    clock_cursor: Option<usize>,
    phy: Option<Vec<crate::runtime::ethernet::phy::attachment::AttachedMedium>>,
    phy_wakes: Option<Vec<Option<u64>>>,
    phy_boundaries: Option<Vec<phy_boundary::BoundaryState>>,
    phy_arrivals: Option<BTreeMap<usize, u64>>,
    phy_admitted: Option<BTreeMap<usize, usize>>,
    configured_media: Option<configured_media::State>,
    mac_control: Option<mac_control::State>,
    phy_services: Option<BTreeMap<usize, super::ethernet::tsn::merge::MacServiceView>>,
    tsn_wakes: BTreeMap<usize, Option<WakeToken>>,
    due: Option<Vec<Due>>,
    control: Option<Option<(u64, TimerToken)>>,
    control_generation: u64,
    record_sequence: u64,
    next_visit: u64,
    future_generation: bool,
    pending_direct: u64,
}
impl NetworkState {
    pub(crate) fn new(p: Arc<PreparedNetwork>) -> Result<Self> {
        let dynamic = p
            .dynamic
            .as_ref()
            .map(|d| DynamicState::new_with_wire(d, &p.ethernet, p.wire.as_ref()))
            .transpose()?;
        let clocks = p
            .clocks
            .as_ref()
            .map(|cfg| {
                cfg.initial
                    .iter()
                    .map(|v| {
                        clock::NodeClock::new(0, v.offset_ps, v.rate_num, v.rate_den)
                            .map_err(|e| error(e.to_string()))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let mut tsn = p.tsn.as_ref().map(TsnState::new);
        if let (Some(tsn), Some(clocks)) = (&mut tsn, &clocks) {
            for output in &p.tsn.as_ref().unwrap().outputs {
                let source = p
                    .ethernet
                    .directions
                    .iter()
                    .find(|d| d.from_port == output.port)
                    .ok_or_else(|| error("initial shaper clock has unknown output"))?
                    .source;
                let delta = tsn.plan_settle_clock_in(
                    TsnDelta::default(),
                    &output.port,
                    0,
                    Some(&clocks[source]),
                )?;
                tsn.apply(delta);
            }
        }
        let tsn_wakes = vec![None; p.tsn.as_ref().map_or(0, |t| t.outputs.len())];
        let bridge = p
            .bridge
            .as_ref()
            .map(|bridge| {
                if p.dynamic.is_some() && p.extensions.is_some() {
                    BridgeState::new_with_policing(bridge, true)
                } else {
                    BridgeState::new(bridge)
                }
            })
            .transpose()?;
        Ok(Self {
            cursors: vec![0; p.ethernet.generators.len()],
            bridge,
            tsn,
            clocks,
            gptp: p.gptp.as_ref().map(gptp::state::State::new),
            clock_cursor: 0,
            phy: p
                .phy_attachments
                .iter()
                .map(|a| {
                    crate::runtime::ethernet::phy::attachment::AttachedMedium::new(a.config.clone())
                        .map_err(error)
                })
                .collect::<Result<Vec<_>>>()?,
            phy_wakes: vec![None; p.phy_attachments.len()],
            phy_boundaries: vec![Default::default(); p.phy_attachments.len()],
            phy_arrivals: BTreeMap::new(),
            phy_admitted: BTreeMap::new(),
            configured_media: configured_media::State::new(&p),
            mac_control: mac_control::State::new(&p),
            phy_services: BTreeMap::new(),
            tsn_wakes,
            queues: vec![Queue::default(); p.ethernet.directions.len()],
            replication: p
                .extensions
                .as_ref()
                .and_then(|e| e.replication.as_ref())
                .map(ReplicationState::new),
            preemption: p
                .extensions
                .as_ref()
                .and_then(|e| e.preemption.as_ref())
                .map(|e| PreemptionState::new(e.outputs.len())),
            mac_merge: p
                .extensions
                .as_ref()
                .and_then(|e| e.mac_merge.as_ref())
                .map(mac_merge::integration::MergeState::new)
                .transpose()?,
            wire: p
                .wire
                .as_ref()
                .map(|w| WireHost::new(w, p.ethernet.directions.len())),
            multicast_active: p
                .multicast_active
                .as_ref()
                .map(multicast_active::Actor::new),
            multicast_history: vec![],
            multicast_cursor: 0,
            multicast_wake: None,
            qcc: p
                .qcc
                .as_ref()
                .map(|q| {
                    configuration::QccState::new(q).map_err(|e| error(format!("Qcc state: {e:?}")))
                })
                .transpose()?,
            qcc_sources: BTreeMap::new(),
            segment_active: vec![None; p.shared_lans.len()],
            segment_arrivals: BTreeMap::new(),
            provider_fdb: p
                .provider_l2
                .as_ref()
                .map(|_| provider_l2::ComponentFdb::default()),
            prepared: p,
            frames: vec![],
            transfers: vec![],
            receptions: vec![],
            dynamic,
            due: vec![],
            control: None,
            control_generation: 0,
            record_sequence: 0,
            next_visit: 1,
            future_generation: false,
            pending_direct: 0,
        })
    }
    fn delta(&self) -> NetworkDelta {
        NetworkDelta {
            points: vec![],
            cursors: None,
            frames: vec![],
            transfers: vec![],
            receptions: vec![],
            queues: BTreeMap::new(),
            dynamic: None,
            bridge: self.bridge.as_ref().map(BridgeState::empty_delta),
            replication: None,
            preemption: None,
            mac_merge: None,
            wire: None,
            multicast_active: None,
            multicast_history: None,
            multicast_cursor: None,
            multicast_wake: None,
            qcc: None,
            qcc_sources: None,
            segment_active: None,
            segment_arrivals: None,
            provider_fdb: None,
            tsn: TsnDelta::default(),
            clocks: None,
            gptp: None,
            clock_cursor: None,
            phy: None,
            phy_wakes: None,
            phy_boundaries: None,
            phy_arrivals: None,
            phy_admitted: None,
            configured_media: None,
            mac_control: None,
            phy_services: None,
            tsn_wakes: BTreeMap::new(),
            due: None,
            control: None,
            control_generation: self.control_generation,
            record_sequence: self.record_sequence,
            next_visit: self.next_visit,
            future_generation: self.future_generation,
            pending_direct: self.pending_direct,
        }
    }
    fn frame<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Frame {
        d.frames
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.frames[i])
    }
    fn transfer<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Transfer {
        d.transfers
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.transfers[i])
    }
    fn reception<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Reception {
        d.receptions
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.receptions[i])
    }
    fn queue<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Queue {
        d.queues.get(&i).unwrap_or(&self.queues[i])
    }
    fn queue_mut<'a>(&self, d: &'a mut NetworkDelta, i: usize) -> &'a mut Queue {
        d.queues.entry(i).or_insert_with(|| self.queues[i].clone())
    }
    fn due_mut<'a>(&self, d: &'a mut NetworkDelta) -> &'a mut Vec<Due> {
        d.due.get_or_insert_with(|| self.due.clone())
    }
    fn policy(&self, d: &NetworkDelta) -> Option<PolicySnapshot> {
        self.dynamic.as_ref().map(|s| {
            d.dynamic.as_ref().map_or_else(
                || s.snapshot_policy(),
                |delta| s.snapshot_policy_after(delta),
            )
        })
    }
    fn epoch_after(&self, d: &NetworkDelta) -> u64 {
        self.policy(d).map_or(0, |p| p.policy_epoch)
    }
    fn epoch(&self) -> u64 {
        self.dynamic.as_ref().map_or(0, DynamicState::policy_epoch)
    }
    fn clocks_after<'a>(&'a self, d: &'a NetworkDelta) -> Option<&'a [clock::NodeClock]> {
        d.clocks.as_deref().or(self.clocks.as_deref())
    }
    fn clock_for_port<'a>(
        &'a self,
        d: &'a NetworkDelta,
        port: &str,
    ) -> Option<&'a clock::NodeClock> {
        let direction = self
            .prepared
            .ethernet
            .directions
            .iter()
            .position(|direction| direction.from_port == port)?;
        let source = self
            .prepared
            .bindings
            .as_ref()
            .and_then(|bindings| bindings.ports.get(direction))
            .map_or(self.prepared.ethernet.directions[direction].source, |p| {
                p.clock_node
            });
        self.clocks_after(d)?.get(source)
    }
    fn schedule(
        c: &mut Context<'_>,
        d: &mut NetworkDelta,
        at: u64,
        event: Event,
    ) -> Result<TimerToken> {
        let phase = matches!(event, Event::Control(_));
        if !phase {
            d.pending_direct = add(d.pending_direct, 1)?;
        }
        let bytes = serde_json::to_vec(&event).map_err(|e| error(e.to_string()))?;
        c.schedule_at(
            at,
            Schema::new(
                if phase {
                    "dir.network.control"
                } else {
                    "dir.network.event"
                },
                1,
            ),
            bytes,
        )
        .map_err(model)
    }
    fn record(
        c: &mut Context<'_>,
        d: &mut NetworkDelta,
        schema: &str,
        id: String,
        mut data: Value,
    ) -> Result<()> {
        let seq = d.record_sequence;
        d.record_sequence = add(seq, 1)?;
        if !matches!(
            schema,
            "can.request"
                | "can.receiver"
                | "ethernet.frame"
                | "ethernet.transfer"
                | "ethernet.reception"
        ) {
            data["effect_seq"] = json!(seq.to_string());
        }
        c.upsert_model_record(ModelRecord {
            id,
            subject: c.subject().into(),
            time_ps: c.now(),
            schema: Schema::new(
                schema,
                match schema {
                    "ethernet.frame" | "ethernet.transfer" => 3,
                    "ethernet.reception" => 2,
                    _ => 1,
                },
            ),
            data,
        })
        .map_err(model)
    }
    fn emit_dynamic(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if let Some(delta) = d.dynamic.as_ref() {
            let mut records = Vec::new();
            for r in &delta.control_records {
                let mut v = decimal(serde_json::to_value(r).map_err(|e| error(e.to_string()))?);
                let mut id = if r.synthetic {
                    format!(
                        "{}:{}",
                        r.control_id,
                        add(d.record_sequence, records.len() as u64)?
                    )
                } else {
                    r.control_id.clone()
                };
                if r.synthetic {
                    while self
                        .prepared
                        .dynamic
                        .as_ref()
                        .unwrap()
                        .controls
                        .iter()
                        .any(|control| control.id == id)
                    {
                        id.push(':');
                    }
                }
                v["control_id"] = json!(id);
                records.push(("ethernet.dynamic.control", id, v));
            }
            if let Some(r) = &delta.policy_record {
                records.push((
                    "ethernet.dynamic.policy",
                    format!("policy:{}", r.policy_epoch),
                    decimal(serde_json::to_value(r).map_err(|e| error(e.to_string()))?),
                ));
            }
            for (s, i, v) in records {
                Self::record(c, d, s, i, v)?;
            }
        }
        Ok(())
    }
    fn arm_control(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let due = d
            .due
            .as_ref()
            .unwrap_or(&self.due)
            .iter()
            .map(|v| v.at)
            .min();
        let policy = d
            .dynamic
            .as_ref()
            .and_then(|v| v.next_deadline)
            .or_else(|| {
                if d.dynamic.is_none() {
                    self.dynamic.as_ref().and_then(DynamicState::next_deadline)
                } else {
                    None
                }
            });
        let tsn_control = self.tsn.as_ref().and_then(|s| s.next_control_after(&d.tsn));
        let wake = (0..self.tsn_wakes.len())
            .filter_map(|i| {
                d.tsn_wakes
                    .get(&i)
                    .unwrap_or(&self.tsn_wakes[i])
                    .as_ref()
                    .map(|w| w.time_ps)
            })
            .min();
        let at = due
            .into_iter()
            .chain(policy)
            .chain(tsn_control)
            .chain(wake)
            .chain(
                self.prepared
                    .qcc
                    .as_ref()
                    .and_then(|p| self.qcc_state(d).and_then(|s| s.next_deadline(p))),
            )
            .min();
        let old = d.control.as_ref().unwrap_or(&self.control);
        if old.as_ref().map(|v| v.0) == at {
            return Ok(());
        }
        if let Some((_, token)) = old {
            c.batch.effects.push(Effect::Cancel(token.clone()));
        }
        d.control_generation = add(d.control_generation, 1)?;
        d.control = Some(if let Some(at) = at {
            Some((
                at,
                Self::schedule(c, d, at, Event::Control(d.control_generation))?,
            ))
        } else {
            None
        });
        Ok(())
    }
    pub(crate) fn initialize(&self, c: &mut Context<'_>, limit: u64) -> Result<NetworkDelta> {
        let mut d = self.delta();
        for i in 0..self.queues.len() {
            d.queues.insert(i, Queue::default());
        }
        // Initial queue samples belong to the observation interval [0, limit).
        // A later failed or paused prefix at time zero still retains them when
        // the configured interval is nonempty.
        if limit > 0 {
            self.queue_points(&mut d, c.now());
        }
        if let Some(state) = &self.dynamic {
            Self::record(
                c,
                &mut d,
                "ethernet.dynamic.policy",
                "policy:0".into(),
                decimal(
                    serde_json::to_value(state.initial_record())
                        .map_err(|e| error(e.to_string()))?,
                ),
            )?;
        }
        if let Some(clocks) = &self.clocks {
            if limit > 0 {
                for (node, clock) in clocks.iter().enumerate() {
                    self.record_clock(node, clock, "initial", &mut d, c)?;
                }
            }
            if let Some(first) = self.prepared.clocks.as_ref().unwrap().adjustments.first() {
                Self::schedule(c, &mut d, first.at_ps, Event::Clock(0))?;
            }
        }
        self.initialize_mac_merge(&mut d, c)?;
        self.initialize_gptp(&mut d, c)?;
        self.sync_mac_merge_links(&mut d, c)?;
        self.initialize_phy(&mut d, c, limit)?;
        self.initialize_wire(&mut d, c)?;
        self.initialize_pause(&mut d)?;
        self.initialize_multicast(&mut d, c)?;
        self.initialize_qcc(&mut d, c)?;
        if let Some(state) = &self.bridge {
            d.bridge = Some(state.initialize(limit)?);
            self.bridge_actions(&mut d, c, limit)?;
        }
        if let Some(time) = self.next_generation(&self.cursors) {
            if let Ok(time) = u64::try_from(time) {
                // Reserving the next cursor event is independent of the run
                // horizon, so earlier callbacks retain identical FES sequences.
                Self::schedule(c, &mut d, time, Event::Dispatch)?;
            } else {
                d.future_generation = true;
            }
        }
        if let Some(tsn) = &self.tsn {
            if tsn.next_control() != Some(0) {
                self.refresh_tsn(&mut d, c, true)?;
            }
        }
        self.emit_tsn(&mut d, c)?;
        self.sync_mac_merge_links(&mut d, c)?;
        self.emit_extensions(&mut d, c)?;
        self.emit_mac_merge(&mut d, c)?;
        self.check_extension_limits(&d)?;
        self.arm_control(&mut d, c)?;
        Ok(d)
    }
    pub(crate) fn event(
        &self,
        envelope: &Envelope,
        c: &mut Context<'_>,
        limit: u64,
    ) -> Result<NetworkDelta> {
        let event: Event =
            serde_json::from_slice(&envelope.payload).map_err(|e| error(e.to_string()))?;
        let mut d = self.delta();
        let now = c.now();
        let control_event = matches!(event, Event::Control(_));
        if !control_event {
            d.pending_direct = d
                .pending_direct
                .checked_sub(1)
                .ok_or_else(|| error("network direct reservation underflow"))?;
        }
        self.merge_capture_fractional_permissions(&mut d, now)?;
        self.phy_synchronize_boundaries(&mut d, c, !control_event)?;
        match event {
            Event::PhyBoundary {
                attachment,
                generation,
                sample_cut,
            } => {
                self.phy_boundary_event(attachment, generation, sample_cut, &d, c)?;
                if self.prepared.phy_attachments[attachment]
                    .config
                    .medium
                    .gigabit_raw_merge
                {
                    c.request_arbitration("network").map_err(model)?;
                }
            }
            Event::MacMergeTimer {
                direction,
                token,
                commit,
            } => self.merge_timer(direction, token, commit, &mut d, c)?,
            Event::MacMergeArrival {
                direction,
                epoch,
                packet,
            } => self.merge_arrival(direction, epoch, packet, &mut d, c)?,
            Event::Wire(event) => self.wire_event(event, &mut d, c)?,
            Event::Clock(index) => self.clock_event(index, &mut d, c)?,
            Event::Gptp(event) => self.gptp_event(event, &mut d, c)?,
            Event::PhySample {
                attachment,
                sample_index,
            } => self.phy_sample(attachment, sample_index, &mut d, c, limit)?,
            Event::Dispatch => {
                let mut cursors = self.cursors.clone();
                let g = self
                    .prepared
                    .ethernet
                    .generators
                    .iter()
                    .enumerate()
                    .filter(|(i, g)| g.time(cursors[*i]) == Some(u128::from(now)))
                    .min_by(|(_, a), (_, b)| a.id.cmp(&b.id))
                    .map(|(i, _)| i)
                    .ok_or_else(|| error("network generation cursor invariant"))?;
                let ordinal = cursors[g];
                cursors[g] = add(ordinal, 1)?;
                Self::schedule(c, &mut d, now, Event::Generate(g, ordinal))?;
                if let Some(time) = self.next_generation(&cursors) {
                    if let Ok(time) = u64::try_from(time) {
                        Self::schedule(c, &mut d, time, Event::Dispatch)?;
                    } else {
                        d.future_generation = true;
                    }
                }
                d.cursors = Some(cursors);
            }
            Event::Generate(g, ordinal) => {
                let generator = self
                    .prepared
                    .ethernet
                    .generators
                    .get(g)
                    .ok_or_else(|| error("unknown generator"))?;
                let device = &self.prepared.ethernet.devices[generator.source];
                let ready = add(now, device.tx_processing_delay_ps)?;
                let f = self.frames.len();
                let ip = self
                    .prepared
                    .dynamic
                    .as_ref()
                    .and_then(|p| p.generator_ip.get(&generator.id))
                    .cloned()
                    .flatten();
                let row = EthernetFrameRecord {
                    frame_id: format!("{}:{ordinal}", generator.id),
                    time_ps: now,
                    source: device.id.clone(),
                    source_vlan_id: generator.source_vlan_id,
                    wire: generator.frame.clone(),
                    flow_id: generator.flow_id.clone(),
                    priority: generator.priority,
                    deadline_ps: generator.deadline_ps,
                    generated_ps: now,
                    ready_ps: None,
                };
                let lineage = self
                    .bridge
                    .as_ref()
                    .map(|_| BridgeLineage::native("ethernet", &row.frame_id, now));
                d.frames.push((
                    f,
                    Frame {
                        generator: Some(g),
                        row,
                        ip,
                        visits: 0,
                        lineage,
                        branch_id: None,
                        gateway_injection: None,
                        terminal_indications: Vec::new(),
                    },
                ));
                self.preemption_created(f, &mut d)?;
                if ready == now {
                    self.source_offer(f, &mut d, c)?;
                } else {
                    self.due_mut(&mut d).push(Due {
                        at: ready,
                        kind: Completion::Ready(f),
                    });
                }
            }
            Event::Bridge(timer) => {
                self.bridge_event(&BridgeEvent::Timer(timer), &mut d, c, limit)?
            }
            Event::Ready(f) => self.source_offer(f, &mut d, c)?,
            Event::Arrival(t) => {
                if self.preemption_direction(self.transfer(&d, t).direction)
                    && self.transfer(&d, t).row.status == "dropped"
                {
                    let mut transfer = self.transfer(&d, t).clone();
                    transfer
                        .fragments
                        .as_mut()
                        .ok_or_else(|| error("retired abstract arrival missing state"))?
                        .arrival_retired_ps = Some(now);
                    put(&mut d.transfers, t, transfer);
                    return Ok(d);
                }
                if self
                    .phy_attachment_for(self.transfer(&d, t).direction)
                    .is_some()
                {
                    let mut arrivals = self.phy_arrivals.clone();
                    if arrivals.remove(&t) != Some(now) {
                        return Err(error(
                            "sampled PHY arrival lacks received-frame reservation",
                        ));
                    }
                    d.phy_arrivals = Some(arrivals);
                }
                if self.gptp_packet(&d, t) {
                    if self.preemption_direction(self.transfer(&d, t).direction) {
                        self.preemption_mut(&mut d)?
                            .express_sources
                            .get_mut(&t)
                            .ok_or_else(|| error("abstract gPTP RX source missing"))?
                            .arrival_actual_ps = Some(now);
                    }
                    self.gptp_arrival(t, None, &mut d, c)?;
                } else {
                    self.arrive(t, None, &mut d, c)?;
                }
            }
            Event::SegmentArrival { transfer, receiver } => {
                let mut arrivals = d
                    .segment_arrivals
                    .clone()
                    .unwrap_or_else(|| self.segment_arrivals.clone());
                if arrivals.remove(&(transfer, receiver)) != Some(now) {
                    return Err(error("shared LAN arrival lacks receiver reservation"));
                }
                d.segment_arrivals = Some(arrivals);
                let direction = self.transfer(&d, transfer).direction;
                let segment = self
                    .segment_for(direction)
                    .ok_or_else(|| error("shared LAN arrival sender"))?;
                self.segment_record(
                    segment,
                    &SegmentOwner::Data(transfer),
                    "receive",
                    direction,
                    Some(receiver),
                    &mut d,
                    c,
                )?;
                self.arrive(transfer, Some(receiver), &mut d, c)?;
            }
            Event::Complete(r) => self.complete(r, &mut d, c)?,
            Event::Control(generation) => {
                if generation != self.control_generation {
                    return Ok(d);
                }
                d.control = Some(None);
                let mut due = self.due.clone();
                let mut current = Vec::new();
                due.retain(|v| {
                    if v.at == now {
                        current.push(v.kind.clone());
                        false
                    } else {
                        true
                    }
                });
                d.due = Some(due);
                let mut wire_timers = Vec::new();
                for kind in current {
                    match kind {
                        Completion::ConfiguredMedia { domain, generation } => {
                            self.media_boundary(domain, generation, &mut d, c)?
                        }
                        Completion::PauseAdmin(index) => self.pause_admin(index, &mut d, c)?,
                        Completion::PauseExpire {
                            direction,
                            generation,
                        } => self.pause_expire(direction, generation, &mut d, c)?,
                        Completion::Wire(event @ HostEvent::Timer { .. }) => {
                            wire_timers.push(event)
                        }
                        Completion::Wire(event) => self.wire_event(event, &mut d, c)?,
                        Completion::MacMerge(event) => self.merge_event(event, &mut d, c)?,
                        Completion::RecoveryTimeout { stream, generation } => {
                            self.replication_timeout(stream, generation, &mut d, c)?
                        }
                        Completion::MulticastTimer => self.multicast_timer(&mut d, c)?,
                        Completion::Fragment {
                            direction,
                            transfer,
                            epoch,
                            id,
                            payload,
                        } => self.fragment_complete(
                            direction, transfer, epoch, &id, payload, &mut d, c,
                        )?,
                        Completion::Gap {
                            direction,
                            epoch,
                            id,
                        } => self.gap_complete(direction, epoch, &id, &mut d, c)?,
                        Completion::Bridge(timer @ BridgeTimer::ConversionReady { .. }) => {
                            // Positive-delay conversion completion notifies phase 1.
                            // Admission (and RX release) must follow already-reserved
                            // arrivals, just like Ethernet processing completions.
                            Self::schedule(c, &mut d, now, Event::Bridge(timer))?;
                        }
                        Completion::Bridge(timer) => {
                            self.bridge_event(&BridgeEvent::Timer(timer), &mut d, c, limit)?
                        }
                        Completion::Eof(t) => {
                            let mut row = self.transfer(&d, t).clone();
                            row.row.time_ps = now;
                            row.row.eof_ps = Some(now);
                            self.preemption_eof(&mut row);
                            row.row.status = "serialized".into();
                            let arrival = row
                                .row
                                .planned_arrival_ps
                                .ok_or_else(|| error("missing arrival"))?;
                            put(&mut d.transfers, t, row);
                            self.pause_mac_completed(self.transfer(&d, t).direction, &mut d, c)?;
                            if self.gptp_packet(&d, t)
                                && self.preemption_direction(self.transfer(&d, t).direction)
                            {
                                self.preemption_mut(&mut d)?
                                    .express_sources
                                    .get_mut(&t)
                                    .ok_or_else(|| error("abstract gPTP EOF source missing"))?
                                    .eof_actual_ps = Some(now);
                            }
                            self.gptp_tx_eof(t, None, &mut d, c)?;
                            if self.segment_for(self.transfer(&d, t).direction).is_some() {
                                let mut ledger = d
                                    .segment_arrivals
                                    .clone()
                                    .unwrap_or_else(|| self.segment_arrivals.clone());
                                for receiver in
                                    self.segment_receivers(self.transfer(&d, t).direction)
                                {
                                    if ledger.insert((t, receiver), arrival).is_some() {
                                        return Err(error(
                                            "shared LAN duplicate receive reservation",
                                        ));
                                    }
                                    Self::schedule(
                                        c,
                                        &mut d,
                                        arrival,
                                        Event::SegmentArrival {
                                            transfer: t,
                                            receiver,
                                        },
                                    )?;
                                }
                                d.segment_arrivals = Some(ledger);
                            } else {
                                Self::schedule(c, &mut d, arrival, Event::Arrival(t))?;
                            }
                        }
                        Completion::Release(t) => {
                            let mut row = self.transfer(&d, t).clone();
                            row.row.time_ps = now;
                            row.row.release_ps = Some(now);
                            self.preemption_release(&mut row, &mut d, now)?;
                            self.queue_mut(&mut d, row.direction).active = None;
                            self.segment_release(row.direction, SegmentOwner::Data(t), &mut d, c)?;
                            put(&mut d.transfers, t, row);
                            c.request_arbitration("network").map_err(model)?;
                        }
                        Completion::Ready(f) => {
                            Self::schedule(c, &mut d, now, Event::Ready(f))?;
                        }
                        Completion::Processed(r) => {
                            Self::schedule(c, &mut d, now, Event::Complete(r))?;
                        }
                    }
                }
                if let Some(state) = &self.dynamic {
                    if state.next_deadline().is_some_and(|t| t <= now) {
                        // Source-bound per-output epochs also catch same-cut ABA controls.
                        if let Some(cfg) = &self.prepared.dynamic {
                            let mut affected = std::collections::BTreeSet::new();
                            for control in
                                cfg.controls.iter().filter(|control| control.at_ps == now)
                            {
                                use crate::types::ethernet::dynamic::ControlOp;
                                match &control.op {
                                    ControlOp::LinkSet { link, .. } => {
                                        for l in cfg.links.iter().filter(|l| l.id == *link) {
                                            affected.extend(l.ports.iter().cloned());
                                        }
                                    }
                                    ControlOp::VlanRegister { key, .. }
                                    | ControlOp::VlanUnregister { key } => {
                                        affected.insert(key.port.clone());
                                    }
                                    _ => {}
                                }
                            }
                            for name in affected {
                                if let Some(i) = self
                                    .prepared
                                    .ethernet
                                    .directions
                                    .iter()
                                    .position(|p| p.from_port == name)
                                {
                                    self.merge_permission_changed(i, &mut d)?;
                                }
                            }
                        }
                        if let Some(cfg) = &self.prepared.dynamic {
                            for control in
                                cfg.controls.iter().filter(|control| control.at_ps == now)
                            {
                                if let crate::types::ethernet::dynamic::ControlOp::LinkSet {
                                    link,
                                    up,
                                } = &control.op
                                {
                                    for l in cfg.links.iter().filter(|l| l.id == *link) {
                                        for name in &l.ports {
                                            if let Some(i) = self
                                                .prepared
                                                .ethernet
                                                .directions
                                                .iter()
                                                .position(|p| p.from_port == *name)
                                            {
                                                if !up {
                                                    self.preemption_link_down(i, &mut d, c)?;
                                                }
                                                self.gptp_source_link(i, *up, &mut d, c)?;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let delta = state.plan_controls(now)?;
                        if !delta.dirty_ports.is_empty() {
                            c.request_arbitration("network").map_err(model)?;
                        }
                        d.dynamic = Some(delta);
                    }
                }
                self.qcc_control(&mut d, c)?;
                self.wire_links(&mut d, c)?;
                for timer in wire_timers {
                    self.wire_event(timer, &mut d, c)?;
                }
                self.refresh_tsn(&mut d, c, true)?;
            }
        }
        self.sync_pause_links(&mut d, c)?;
        self.sync_preemption_wire_links(&mut d, c)?;
        self.sync_phy_links(&mut d, c)?;
        self.sync_mac_merge_links(&mut d, c)?;
        self.sync_gptp_links(&mut d, c)?;
        if !control_event {
            self.refresh_tsn(&mut d, c, false)?;
        }
        if control_event {
            // Publish the same phase-0 order used to build the staged state:
            // wire completions, dynamic policy, then GCL/credit observations.
            self.emit_rows(&mut d, c)?;
            self.emit_dynamic(&mut d, c)?;
            self.emit_tsn(&mut d, c)?;
        } else {
            self.emit_tsn(&mut d, c)?;
            self.emit_dynamic(&mut d, c)?;
            self.emit_rows(&mut d, c)?;
        }
        self.emit_extensions(&mut d, c)?;
        self.emit_mac_merge(&mut d, c)?;
        self.check_extension_limits(&d)?;
        self.arm_control(&mut d, c)?;
        self.queue_points(&mut d, c.now());
        Ok(d)
    }
    fn record_clock(
        &self,
        node: usize,
        clock: &clock::NodeClock,
        cause: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let local = clock.local_at(c.now()).map_err(|e| error(e.to_string()))?;
        let (rate_num, rate_den) = clock.rate();
        let name = &self.prepared.ethernet.devices[node].id;
        Self::record(
            c,
            d,
            "dir.network.clock",
            format!("clock:{name}:{}", clock.generation()),
            json!({"node":name,"global_ps":c.now().to_string(),"local_numerator":local.numerator().to_string(),
                "local_denominator":local.denominator().to_string(),"rate_num":rate_num.to_string(),
                "rate_den":rate_den.to_string(),"generation":clock.generation().to_string(),"cause":cause}),
        )
    }
    /// Settle physical credit intervals under the current clock anchor before
    /// its owner applies a discontinuity or changes rate in this transaction.
    fn before_clock_change(
        &self,
        node: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(state) = &self.tsn else {
            return Ok(());
        };
        let prepared = self.prepared.tsn.as_ref().unwrap();
        for output in &prepared.outputs {
            let direction = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|p| p.from_port == output.port)
                .ok_or_else(|| error("clock settlement has unknown TSN direction"))?;
            if self.prepared.ethernet.directions[direction].source != node {
                continue;
            }
            let clock = self.clock_for_port(d, &output.port).cloned();
            let delta = state.plan_settle_clock_in(
                std::mem::take(&mut d.tsn),
                &output.port,
                c.now(),
                clock.as_ref(),
            )?;
            self.stage_tsn(d, delta, std::slice::from_ref(&output.port))?;
        }
        Ok(())
    }
    /// Reevaluate gates at the same physical cut after the clock owner has
    /// staged its new anchor. Elapsed credit was already settled above.
    fn after_clock_change(
        &self,
        node: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(state) = &self.tsn else {
            return Ok(());
        };
        self.qcc_clock_changed(node, d, c)?;
        for output in &self.prepared.tsn.as_ref().unwrap().outputs {
            let direction = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|p| p.from_port == output.port)
                .ok_or_else(|| error("clock reanchor has unknown TSN direction"))?;
            if self.prepared.ethernet.directions[direction].source != node {
                continue;
            }
            self.merge_permission_changed(direction, d)?;
            let input = self.port_input(direction, d)?;
            let clock = self.clock_for_port(d, &output.port).cloned();
            let delta = state.plan_reanchor_clock_in(
                std::mem::take(&mut d.tsn),
                &output.port,
                c.now(),
                &input,
                clock.as_ref(),
            )?;
            self.stage_tsn(d, delta, std::slice::from_ref(&output.port))?;
            if self.merge_direction(direction) {
                self.merge_plan_tas(direction, d, c.now())?;
            }
            self.phy_set_start_permission(direction, d, c, true)?;
        }
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    fn clock_event(&self, index: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let prepared = self
            .prepared
            .clocks
            .as_ref()
            .ok_or_else(|| error("clock event without clock configuration"))?;
        if index != self.clock_cursor
            || prepared
                .adjustments
                .get(index)
                .is_none_or(|a| a.at_ps != c.now())
        {
            return Err(error("clock adjustment cursor/time mismatch"));
        }
        let adjustment = &prepared.adjustments[index];
        self.before_clock_change(adjustment.node, d, c)?;
        let clocks = d
            .clocks
            .get_or_insert_with(|| self.clocks.as_ref().unwrap().clone());
        let clock = &mut clocks[adjustment.node];
        match adjustment.action {
            PreparedClockAction::StepBy { offset_ps } => clock.step_by(c.now(), offset_ps),
            PreparedClockAction::Slew { rate_num, rate_den } => {
                clock.slew(c.now(), rate_num, rate_den)
            }
        }
        .map_err(|e| error(e.to_string()))?;
        let changed = clock.clone();
        d.clock_cursor = Some(index + 1);
        self.record_clock(
            adjustment.node,
            &changed,
            match adjustment.action {
                PreparedClockAction::StepBy { .. } => "step",
                PreparedClockAction::Slew { .. } => "slew",
            },
            d,
            c,
        )?;
        if let Some(next) = prepared.adjustments.get(index + 1) {
            Self::schedule(c, d, next.at_ps, Event::Clock(index + 1))?;
        }
        self.after_clock_change(adjustment.node, d, c)?;
        Ok(())
    }
    fn source_offer(&self, f: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut frame = self.frame(d, f).clone();
        frame.row.time_ps = c.now();
        frame.row.ready_ps = Some(c.now());
        let source = self
            .prepared
            .ethernet
            .devices
            .iter()
            .position(|v| v.id == frame.row.source)
            .ok_or_else(|| error("unknown source"))?;
        let class = l2::Classification {
            vid: frame
                .row
                .source_vlan_id
                .ok_or_else(|| error("missing source VID"))?,
            priority: frame.row.priority,
            dei: frame.row.wire.tag.map_or(0, |t| t.dei),
        };
        put(&mut d.frames, f, frame.clone());
        let dirs: Vec<_> = self
            .prepared
            .ethernet
            .directions
            .iter()
            .enumerate()
            .filter(|(_, v)| v.source == source)
            .map(|(i, _)| i)
            .collect();
        let dirs = self.qcc_source(f, d, c)?.unwrap_or(dirs);
        if dirs.is_empty() {
            return Ok(());
        }
        if self.replication_offer(f, source, class, d, c)? {
            return Ok(());
        }
        self.offer(
            CopyOrigin {
                circuit: None,
                frame: f,
                parent: None,
                visit: 0,
            },
            &frame.row.wire,
            class,
            &dirs,
            d,
            c,
        )?;
        Ok(())
    }
    fn offer(
        &self,
        origin: CopyOrigin,
        wire: &EthernetWireFrame,
        class: l2::Classification,
        dirs: &[usize],
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<Vec<String>> {
        let CopyOrigin {
            circuit,
            frame: f,
            parent,
            visit,
        } = origin;
        let mut ids = Vec::new();
        for &direction in dirs {
            let link = &self.prepared.ethernet.directions[direction];
            let port = &link.from_port;
            let policy = self
                .prepared
                .ethernet
                .port_policies
                .iter()
                .find(|p| &p.port == port)
                .ok_or_else(|| error("missing output policy"))?;
            let ps = self.policy(d);
            let tagged = ps
                .as_ref()
                .and_then(|p| p.effective_vlans.get(port))
                .and_then(|v| v.get(&class.vid))
                .copied()
                .or_else(|| {
                    self.prepared.dynamic.as_ref().and_then(|p| {
                        p.registrable
                            .contains(&RegistrationKey {
                                port: port.clone(),
                                vid: class.vid,
                            })
                            .then_some(true)
                    })
                });
            let outgoing = if let Some(topology) = &self.prepared.provider_l2 {
                if topology.ports.contains_key(port) {
                    // Ingress/component traversal has already selected the
                    // egress classification. Do not classify its encoded tags
                    // a second time as though this output were a new ingress.
                    let classified = provider_l2::ClassifiedFrame {
                        component: topology.ports[port].component.clone(),
                        ingress_port: port.clone(),
                        vid: class.vid,
                        priority: class.priority,
                        dei: class.dei,
                        wire: wire.clone(),
                    };
                    topology
                        .encode_egress(port, &classified)
                        .map_err(|e| error(format!("provider egress encoding: {}", e.message)))?
                } else {
                    l2::rewrite(policy, wire, class, tagged)?
                }
            } else {
                l2::rewrite(policy, wire, class, tagged)?
            };
            let outgoing = if let Some(cursor) = &circuit {
                if let Some(distributed) = &cursor.distributed {
                    if matches!(distributed.leg, replication::DistributedLeg::Branch { .. }) {
                        let state = d
                            .replication
                            .as_ref()
                            .or(self.replication.as_ref())
                            .ok_or_else(|| error("missing distributed state"))?;
                        let sequence = state
                            .distributed
                            .get(&distributed.flow)
                            .and_then(|f| f.sequence)
                            .ok_or_else(|| error("missing distributed sequence"))?;
                        frer_wire::transmit(&outgoing, sequence)
                            .map_err(|reason| error(format!("R-TAG transmit: {reason}")))?
                    } else {
                        outgoing
                    }
                } else {
                    let state = d
                        .replication
                        .as_ref()
                        .or(self.replication.as_ref())
                        .ok_or_else(|| error("missing replication state"))?;
                    let branch = state
                        .branches
                        .get(&cursor.branch)
                        .ok_or_else(|| error("unknown circuit branch"))?;
                    let stream = &self
                        .prepared
                        .extensions
                        .as_ref()
                        .unwrap()
                        .replication
                        .as_ref()
                        .unwrap()
                        .streams[branch.stream];
                    if stream.wire_format == ReplicationWireFormat::Rtag8021cb2017 {
                        frer_wire::transmit(&outgoing, branch.sequence as u16)
                            .map_err(|reason| error(format!("R-TAG transmit: {reason}")))?
                    } else {
                        outgoing
                    }
                }
            } else {
                outgoing
            };
            let t = self.transfers.len()
                + d.transfers
                    .iter()
                    .filter(|(i, _)| *i >= self.transfers.len())
                    .count();
            let frame = self.frame(d, f);
            let id = if self.bridge.is_some() && circuit.is_none() {
                format!("{}@{port}", frame.row.frame_id)
            } else {
                format!("{}/v{visit}/c{t}", frame.row.frame_id)
            };
            let q = self.queue(d, direction);
            let p = class.priority as usize;
            let config = self
                .prepared
                .ethernet
                .outputs
                .iter()
                .find(|o| o.port == *port)
                .and_then(|o| o.queues.iter().find(|q| q.priority == class.priority));
            let unbound_reserved = self.prepared.qcc.as_ref().is_some_and(|prepared| {
                prepared.managed_priority_mask(direction) & (1 << class.priority) != 0
                    && !d
                        .qcc_sources
                        .as_ref()
                        .unwrap_or(&self.qcc_sources)
                        .contains_key(&f)
            });
            let policy_reason = self
                .data_egress_reason(d, port, class.vid)
                .filter(|reason| {
                    !self.explicit_replication_route(circuit.as_ref(), d)
                        || !matches!(*reason, "stp_discarding" | "stp_learning")
                })
                .or(unbound_reserved.then_some("qcc_unbound_class"));
            let unadmittable = self.prepared.ethernet.devices[link.source].queue_capacity == 0
                || config.is_some_and(|q| {
                    q.capacity_frames == 0
                        || q.capacity_bytes.is_some_and(|n| outgoing.mac_bytes > n)
                });
            let admitted = policy_reason.is_none()
                && l2::admission(
                    q.waiting.iter().map(|v| v.len() as u64).sum(),
                    self.prepared.ethernet.devices[link.source].queue_capacity,
                    q.waiting[p].len() as u64,
                    q.bytes[p],
                    config,
                    outgoing.mac_bytes,
                )?
                && self
                    .prepared
                    .qcc
                    .as_ref()
                    .zip(self.qcc_state(d))
                    .is_none_or(|(prepared, state)| {
                        let sources = d.qcc_sources.as_ref().unwrap_or(&self.qcc_sources);
                        sources.contains_key(&f)
                            || state.background_buffer_allowed(
                                prepared,
                                direction,
                                class.priority,
                                q.waiting.iter().map(|v| v.len() as u64).sum(),
                                std::array::from_fn(|p| q.waiting[p].len() as u64),
                            )
                    });
            let row = EthernetTransferRecord {
                transfer_id: id.clone(),
                time_ps: c.now(),
                frame_id: frame.row.frame_id.clone(),
                parent_transfer_id: parent.map(|t| self.transfer(d, t).row.transfer_id.clone()),
                queue_id: Some(format!("{port}.queue.{}", class.priority)),
                priority: class.priority,
                wire: outgoing.clone(),
                vlan_id: Some(class.vid),
                from_port: port.clone(),
                to_port: link.to_port.clone(),
                queued_ps: c.now(),
                sof_ps: None,
                eof_ps: None,
                release_ps: None,
                arrival_ps: None,
                planned_eof_ps: None,
                planned_release_ps: None,
                planned_arrival_ps: None,
                status: if admitted { "queued" } else { "dropped" }.into(),
                drop_reason: (!admitted).then(|| {
                    policy_reason
                        .unwrap_or(if self.prepared.extensions.is_some() && unadmittable {
                            "tx_unadmittable"
                        } else {
                            "queue_full"
                        })
                        .into()
                }),
                media: self
                    .configured_endpoint(direction)
                    .map(|(i, _)| EthernetMediaTransfer {
                        physical_link: self
                            .prepared
                            .ethernet
                            .media
                            .as_ref()
                            .unwrap()
                            .physical_links[i]
                            .id
                            .clone(),
                        ..Default::default()
                    }),
            };
            if admitted {
                let q = self.queue_mut(d, direction);
                q.bytes[p] = add(q.bytes[p], outgoing.mac_bytes)?;
                q.waiting[p].push_back(t);
                c.request_arbitration("network").map_err(model)?;
            }
            d.transfers.push((
                t,
                Transfer {
                    circuit: circuit.clone(),
                    fragments: self
                        .preemption_direction(direction)
                        .then(TransferFragments::default),
                    row,
                    frame: f,
                    direction,
                    visit,
                    offer_epoch: self.epoch_after(d),
                    sof_epoch: None,
                },
            ));
            self.replication_offered(t, d, c.now())?;
            ids.push(id);
        }
        Ok(ids)
    }
    fn arrive(
        &self,
        t: usize,
        receiver: Option<usize>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.arrive_with_id(t, receiver, None, d, c)
    }
    fn arrive_with_id(
        &self,
        t: usize,
        receiver: Option<usize>,
        reception_id: Option<String>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let mut transfer = self.transfer(d, t).clone();
        let link = self.effective_link(transfer.direction, receiver)?;
        let reception_id = reception_id.unwrap_or_else(|| {
            if receiver.is_some() {
                format!("{}@rx@{}", transfer.row.transfer_id, link.to_port)
            } else {
                format!("{}@rx", transfer.row.transfer_id)
            }
        });
        let raw = crate::runtime::wire::codec::unhex(&transfer.row.wire.mac_hex)
            .map_err(|e| error(e.0))?;
        let rx_direction = receiver.or_else(|| {
            self.prepared
                .ethernet
                .port_policies
                .iter()
                .find(|p| p.ingress == link.to_port)
                .and_then(|p| {
                    self.prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|d| d.from_port == p.port)
                })
        });
        if let Some(rx) = rx_direction {
            if self.pause_receive(rx, &raw, &transfer.row.transfer_id, d, c)? {
                transfer.row.time_ps = c.now();
                transfer.row.arrival_ps.get_or_insert(c.now());
                transfer.row.status = "arrived".into();
                put(&mut d.transfers, t, transfer);
                return Ok(());
            }
        }
        let device = &self.prepared.ethernet.devices[link.destination];
        let policy = self
            .prepared
            .ethernet
            .port_policies
            .iter()
            .find(|p| p.ingress == link.to_port)
            .ok_or_else(|| error("missing ingress policy"))?;
        let provider_class = self
            .prepared
            .provider_l2
            .as_ref()
            .filter(|topology| topology.ports.contains_key(&policy.port))
            .map(|topology| topology.classify_ingress(&policy.port, &transfer.row.wire));
        let class = provider_class
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map_or_else(
                || l2::classify(policy, &transfer.row.wire),
                |classified| l2::Classification {
                    vid: classified.vid,
                    priority: classified.priority,
                    dei: classified.dei,
                },
            );
        let mut reason = self
            .data_ingress_reason(d, &policy.port, class.vid)
            .filter(|reason| {
                *reason != "stp_discarding"
                    || !self.explicit_replication_route(transfer.circuit.as_ref(), d)
                        && self.dynamic.as_ref().and_then(|s| {
                            s.wire_ingress_state_for_vid(
                                &link.to_port,
                                class.vid,
                                d.dynamic.as_ref(),
                            )
                        }) != Some((true, false))
            })
            .map(str::to_string);
        if reason.is_none() {
            if let Some(Err(diagnostic)) = provider_class.as_ref() {
                let _ = diagnostic;
                reason = Some("provider_ingress".into());
            }
        }
        if reason.is_none() && self.dynamic.is_none() && !policy.vlans.contains_key(&class.vid) {
            reason = Some("ingress_vlan_membership".into());
        }
        if reason.is_none()
            && ((policy.admit == "tagged_only"
                && transfer
                    .row
                    .wire
                    .canonical_tags()
                    .is_ok_and(|tags| tags.is_empty()))
                || (policy.admit == "untagged_only"
                    && transfer
                        .row
                        .wire
                        .canonical_tags()
                        .is_ok_and(|tags| !tags.is_empty())))
        {
            reason = Some("ingress_frame_type".into());
        }
        if reason.is_none()
            && device.kind == "endpoint"
            && device.mac.as_ref() != Some(&transfer.row.wire.dst_mac)
            && transfer.row.wire.dst_mac != "ff:ff:ff:ff:ff:ff"
        {
            let group =
                u8::from_str_radix(&transfer.row.wire.dst_mac[..2], 16).unwrap_or(0) & 1 != 0;
            if !group {
                reason = Some("destination_mismatch".into());
            } else if !device
                .subscriptions
                .contains(&(class.vid, transfer.row.wire.dst_mac.clone()))
                && !self
                    .prepared
                    .multicast_active
                    .as_ref()
                    .zip(
                        d.multicast_active
                            .as_ref()
                            .or(self.multicast_active.as_ref()),
                    )
                    .and_then(|(config, actor)| {
                        self.frame(d, transfer.frame).ip.as_ref().map(|ip| {
                            actor.interested(
                                config,
                                &link.to_port,
                                class.vid,
                                ip.source,
                                ip.group,
                                c.now(),
                            )
                        })
                    })
                    .unwrap_or(false)
            {
                reason = Some("multicast_not_subscribed".into());
            }
        }
        if reason.is_none() {
            if let Some(tsn) = &self.tsn {
                let ingress_clock = self.clock_for_port(d, &policy.port).cloned();
                let delta = tsn.plan_arrival_clock_in(
                    std::mem::take(&mut d.tsn),
                    &link.to_port,
                    &transfer.row.wire.dst_mac,
                    class.vid,
                    class.priority,
                    transfer.row.wire.mac_bytes,
                    &reception_id,
                    c.now(),
                    ingress_clock.as_ref(),
                )?;
                if let Some(p) = &delta.psfp {
                    if p.verdict == "drop" {
                        reason = Some(p.reason.clone());
                    }
                }
                d.tsn = delta;
            }
        }
        let mut frame = self.frame(d, transfer.frame).clone();
        let visit = d.next_visit;
        d.next_visit = add(d.next_visit, 1)?;
        if reason.is_none()
            && self
                .prepared
                .dynamic
                .as_ref()
                .is_some_and(|p| frame.visits >= p.limits.visits_per_frame)
        {
            reason = Some("visit_limit".into());
        }
        if reason.is_none() {
            frame.visits = add(frame.visits, 1)?;
            put(&mut d.frames, transfer.frame, frame);
        }
        let ready = if reason.is_none() {
            Some(add(
                c.now(),
                if device.kind == "endpoint" {
                    device.rx_processing_delay_ps
                } else {
                    device.forward_delay_ps
                },
            )?)
        } else {
            None
        };
        let r = self.receptions.len()
            + d.receptions
                .iter()
                .filter(|(i, _)| *i >= self.receptions.len())
                .count();
        let row = EthernetReceptionRecord {
            reception_id,
            time_ps: c.now(),
            frame_id: transfer.row.frame_id.clone(),
            transfer_id: transfer.row.transfer_id.clone(),
            device: device.id.clone(),
            ingress: link.to_port.clone(),
            vlan_id: Some(class.vid),
            priority: class.priority,
            observed_ps: c.now(),
            ready_ps: reason.as_ref().map(|_| c.now()),
            planned_ready_ps: ready,
            status: if reason.is_some() {
                "filtered"
            } else {
                "processing"
            }
            .into(),
            reason,
            egress_transfer_ids: vec![],
        };
        transfer.row.time_ps = c.now();
        transfer.row.arrival_ps = Some(c.now());
        put(&mut d.transfers, t, transfer.clone());
        d.receptions.push((
            r,
            Reception {
                row,
                transfer: t,
                visit,
                epoch: self.epoch(),
                ingress_no_relay_reason: self
                    .mstp_tree_policy(d, &policy.port, class.vid)
                    .map(|tree| {
                        if !tree.learning {
                            Some(IngressNoRelayReason::Discarding)
                        } else if !tree.forwarding {
                            Some(IngressNoRelayReason::Learning)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| {
                        self.dynamic.as_ref().and_then(|s| {
                            match s.wire_ingress_state_for_vid(
                                &link.to_port,
                                class.vid,
                                d.dynamic.as_ref(),
                            ) {
                                Some((true, false)) => Some(IngressNoRelayReason::Learning),
                                Some((false, false)) => Some(IngressNoRelayReason::Discarding),
                                _ => None,
                            }
                        })
                    }),
                segment_receiver: receiver,
            },
        ));
        if ready.is_some()
            && !self.explicit_replication_route(transfer.circuit.as_ref(), d)
            && provider_class.is_none()
        {
            if let Some(state) = &self.dynamic {
                d.dynamic = Some(state.plan_learning(
                    &link.to_port,
                    class.vid,
                    &transfer.row.wire.src_mac,
                    c.now(),
                )?);
            }
        }
        if ready.is_none() && self.replication_receiver_is_intended(&transfer, receiver, d)? {
            self.replication_drop(t, d, c.now(), "filtered")?;
        }
        if ready.is_some() && self.bridge.is_some() {
            let frame = self.frame(d, transfer.frame);
            let event = BridgeEvent::EthernetIngress {
                endpoint: link.destination,
                ingress_record: format!("{}@rx", transfer.row.transfer_id),
                origin_id: frame
                    .lineage
                    .as_ref()
                    .map_or_else(|| frame.row.frame_id.clone(), |l| l.origin_id.clone()),
                generated_ps: frame
                    .lineage
                    .as_ref()
                    .map_or(frame.row.generated_ps, |l| l.generated_ps),
                wire: transfer.row.wire.clone(),
                vid: class.vid,
                priority: class.priority,
                lineage: frame.lineage.clone(),
            };
            self.bridge_event(&event, d, c, u64::MAX)?;
        }

        if ready == Some(c.now()) {
            self.complete(r, d, c)?;
        } else if let Some(ready) = ready {
            self.due_mut(d).push(Due {
                at: ready,
                kind: Completion::Processed(r),
            });
        }
        Ok(())
    }
    fn complete(&self, r: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut reception = self.reception(d, r).clone();
        let transfer = self.transfer(d, reception.transfer).clone();
        let link = self.effective_link(transfer.direction, reception.segment_receiver)?;
        let device = &self.prepared.ethernet.devices[link.destination];
        reception.row.time_ps = c.now();
        reception.row.ready_ps = Some(c.now());
        if transfer.circuit.is_some() {
            self.replication_complete(&mut reception, &transfer, d, c)?;
            put(&mut d.receptions, r, reception);
            return Ok(());
        }
        if device.kind == "endpoint" {
            reception.row.status = "received".into();
            self.preemption_received(&reception, d, c.now())?;
            self.qcc_delivered(&transfer, d, c)?;
            let frame = self.frame(d, transfer.frame);
            d.points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: c.now(),
                target: device.id.clone(),
                metric: "ethernet.delivery_ps".into(),
                value: c
                    .now()
                    .checked_sub(frame.row.generated_ps)
                    .ok_or_else(|| error("delivery before generation"))?,
                request_id: Some(frame.row.frame_id.clone()),
                receiver: Some(device.id.clone()),
                reason: None,
            });
            self.notify_terminal_delivery(r, &reception, &transfer, None, d, c)?;
        } else {
            if let Some(reason) = reception
                .ingress_no_relay_reason
                .map(IngressNoRelayReason::as_str)
                .or_else(|| {
                    self.prepared
                        .ethernet
                        .port_policies
                        .iter()
                        .find(|p| p.ingress == link.to_port)
                        .and_then(|p| {
                            self.mstp_tree_policy(d, &p.port, reception.row.vlan_id.unwrap())
                        })
                        .map(|tree| {
                            if !tree.learning {
                                Some("stp_discarding")
                            } else if !tree.forwarding {
                                Some("stp_learning")
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| {
                            self.dynamic.as_ref().and_then(|s| {
                                match s.wire_ingress_state_for_vid(
                                    &link.to_port,
                                    reception.row.vlan_id.unwrap(),
                                    d.dynamic.as_ref(),
                                ) {
                                    Some((true, false)) => Some("stp_learning"),
                                    Some((false, false)) => Some("stp_discarding"),
                                    _ => None,
                                }
                            })
                        })
                })
            {
                reception.row.status = "filtered".into();
                reception.row.reason = Some(reason.into());
                put(&mut d.receptions, r, reception);
                return Ok(());
            }
            let provider_port = self.prepared.provider_l2.as_ref().and_then(|topology| {
                self.prepared
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.ingress == link.to_port && topology.ports.contains_key(&p.port))
            });
            if let Some(port) = provider_port {
                self.provider_forward(&mut reception, &transfer, &port.port, d, c)?;
                put(&mut d.receptions, r, reception);
                return Ok(());
            }
            let vid = reception.row.vlan_id.unwrap();
            let frame = self.frame(d, transfer.frame);
            let selected = if let Some(selected) =
                self.qcc_route_at(transfer.frame, transfer.direction, link.destination, d)
            {
                selected
            } else if let Some(selected) = self.wire_multicast(
                &device.id,
                &link.to_port,
                vid,
                frame.ip.as_ref(),
                &transfer.row.wire,
                d,
                c.now(),
            ) {
                selected
            } else if let Some(state) = &self.dynamic {
                d.dynamic.as_ref().map_or_else(
                    || {
                        state.select_egress(
                            &device.id,
                            &link.to_port,
                            vid,
                            &transfer.row.wire,
                            frame.ip.as_ref(),
                        )
                    },
                    |delta| {
                        state.select_egress_after(
                            delta,
                            &device.id,
                            &link.to_port,
                            vid,
                            &transfer.row.wire,
                            frame.ip.as_ref(),
                        )
                    },
                )
            } else {
                {
                    let (directions, reason) = l2::static_egress(
                        &self.prepared.ethernet,
                        transfer.direction,
                        &transfer.row.wire,
                        Some(vid),
                    )?;
                    EgressSelection {
                        ports: directions
                            .into_iter()
                            .map(|i| self.prepared.ethernet.directions[i].from_port.clone())
                            .collect(),
                        reason: reason.map(str::to_string),
                    }
                }
            };
            let dirs: Vec<_> = selected
                .ports
                .iter()
                .filter_map(|p| {
                    self.prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|v| v.from_port == *p)
                })
                .collect();
            let class = l2::Classification {
                vid,
                priority: reception.row.priority,
                dei: transfer.row.wire.tag.map_or(0, |v| v.dei),
            };
            reception.row.egress_transfer_ids = self.offer(
                CopyOrigin {
                    circuit: None,
                    frame: transfer.frame,
                    parent: Some(reception.transfer),
                    visit: reception.visit,
                },
                &transfer.row.wire,
                class,
                &dirs,
                d,
                c,
            )?;
            reception.row.status = if selected.reason.is_some() {
                "filtered"
            } else {
                "forwarded"
            }
            .into();
            reception.row.reason = selected.reason;
        }
        put(&mut d.receptions, r, reception);
        Ok(())
    }
    pub(crate) fn arbitrate(&self, c: &mut Context<'_>) -> Result<NetworkDelta> {
        let mut d = self.delta();
        self.merge_capture_fractional_permissions(&mut d, c.now())?;
        self.phy_synchronize_boundaries(&mut d, c, true)?;
        for direction in 0..self.queues.len() {
            let link = &self.prepared.ethernet.directions[direction];
            if self.segment_busy(direction, &d) {
                continue;
            }
            // Eligibility applies to every queued copy, even when its gate would remain closed.
            for priority in 0..8 {
                let pending: Vec<_> = self.queue(&d, direction).waiting[priority]
                    .iter()
                    .copied()
                    .collect();
                for t in pending {
                    let old = self.transfer(&d, t);
                    if self.gptp_packet(&d, t) {
                        continue;
                    }
                    let reason =
                        self.data_egress_reason(&d, &link.from_port, old.row.vlan_id.unwrap());
                    if let Some(reason) = reason.filter(|reason| {
                        !self.explicit_replication_route(old.circuit.as_ref(), &d)
                            || !matches!(*reason, "stp_discarding" | "stp_learning")
                    }) {
                        let mut row = old.clone();
                        row.row.time_ps = c.now();
                        row.row.status = "dropped".into();
                        row.row.drop_reason = Some(reason.into());
                        let q = self.queue_mut(&mut d, direction);
                        q.waiting[priority].retain(|v| *v != t);
                        q.bytes[priority] = q.bytes[priority]
                            .checked_sub(row.row.wire.mac_bytes)
                            .ok_or_else(|| error("queue byte underflow"))?;
                        put(&mut d.transfers, t, row);
                        self.replication_drop(t, &mut d, c.now(), reason)?;
                    }
                }
            }
            if self.merge_direction(direction) {
                self.merge_arbitrate(direction, &mut d, c)?;
                if let Some(index) = self.phy_attachment_for(direction) {
                    self.phy_replan_raw_demand(index, &mut d, c)?;
                }
                continue;
            }
            if self.preemption_direction(direction) {
                self.preemption_arbitrate(direction, &mut d, c)?;
                continue;
            }
            let input = self.port_input(direction, &d)?;
            let tsn_selected = if let Some(tsn) = &self.tsn {
                let clock = self.clock_for_port(&d, &link.from_port).cloned();
                let next = tsn.plan_port_clock_in(
                    std::mem::take(&mut d.tsn),
                    &link.from_port,
                    c.now(),
                    &input,
                    clock.as_ref(),
                )?;
                let selected = next.selected.get(&link.from_port).copied();
                self.stage_tsn(&mut d, next, &[link.from_port.clone()])?;
                selected
            } else {
                None
            };
            if self.wire_arbitrate(direction, &mut d, c)? {
                continue;
            }
            if self.queue(&d, direction).active.is_some() {
                if self.configured_endpoint(direction).is_some() {
                    self.media_try_start(direction, &mut d, c)?;
                }
                self.phy_set_start_permission(direction, &mut d, c, true)?;
                continue;
            }
            let selected = if self.tsn.is_some() {
                tsn_selected.and_then(|p| {
                    self.queue(&d, direction).waiting[p as usize]
                        .front()
                        .map(|t| (p as usize, *t))
                })
            } else {
                let strict = self
                    .prepared
                    .ethernet
                    .outputs
                    .iter()
                    .find(|o| o.port == link.from_port)
                    .is_some_and(|o| o.scheduler == "strict_priority");
                if strict {
                    (0..8).rev().find_map(|p| {
                        self.queue(&d, direction).waiting[p]
                            .front()
                            .map(|t| (p, *t))
                    })
                } else {
                    (0..8)
                        .filter_map(|p| {
                            self.queue(&d, direction).waiting[p]
                                .front()
                                .map(|t| (p, *t))
                        })
                        .min_by_key(|(_, t)| *t)
                }
            };
            if !self.output_request_allowed(direction, false, &d, c.now()) {
                continue;
            }
            let Some((p, t)) = selected else { continue };
            if self.configured_endpoint(direction).is_some() {
                let bytes = self.transfer(&d, t).row.wire.mac_bytes;
                let q = self.queue_mut(&mut d, direction);
                q.waiting[p].pop_front();
                q.bytes[p] = q.bytes[p]
                    .checked_sub(bytes)
                    .ok_or_else(|| error("queue byte underflow"))?;
                q.active = Some(t);
                self.media_claim(direction, configured_media::Work::Data(t), &mut d, c)?;
                continue;
            }
            if self.phy_attachment_for(direction).is_some() {
                let mut row = self.transfer(&d, t).clone();
                let q = self.queue_mut(&mut d, direction);
                q.waiting[p].pop_front();
                q.bytes[p] = q.bytes[p]
                    .checked_sub(row.row.wire.mac_bytes)
                    .ok_or_else(|| error("queue byte underflow"))?;
                q.active = Some(t);
                row.row.time_ps = c.now();
                row.row.status = "queued".into();
                put(&mut d.transfers, t, row);
                self.phy_admit(direction, t, &mut d, c)?;
                continue;
            }
            let mut row = self.transfer(&d, t).clone();
            let (eof, release, arrival) = l2::timing(
                c.now(),
                row.row.wire.mac_bytes,
                link.bitrate_bps,
                link.delay_ps,
            )?;
            self.segment_reserve(direction, SegmentOwner::Data(t), eof, release, &mut d, c)?;
            let q = self.queue_mut(&mut d, direction);
            q.waiting[p].pop_front();
            q.bytes[p] = q.bytes[p]
                .checked_sub(row.row.wire.mac_bytes)
                .ok_or_else(|| error("queue byte underflow"))?;
            q.active = Some(t);
            row.row.time_ps = c.now();
            row.row.sof_ps = Some(c.now());
            row.row.planned_eof_ps = Some(eof);
            row.row.planned_release_ps = Some(release);
            row.row.planned_arrival_ps = Some(arrival);
            row.row.status = "transmitting".into();
            row.sof_epoch = Some(self.epoch());
            put(&mut d.transfers, t, row);
            if self.preemption.is_some() && !self.gptp_packet(&d, t) {
                self.preemption_sof(t, &mut d, c.now())?;
            }
            self.gptp_tx_start(t, None, self.epoch(), c.now(), &mut d, c)?;
            self.qcc_sof(t, &mut d, c)?;
            self.due_mut(&mut d).extend([
                Due {
                    at: eof,
                    kind: Completion::Eof(t),
                },
                Due {
                    at: release,
                    kind: Completion::Release(t),
                },
            ]);
            self.replication_sof(t, &mut d, c.now())?;
            if self.bridge.is_some() {
                self.notify_bridge_source_sof(t, &mut d, c)?;
                // Retry is a phase1 notification; phase2 naturally sends it to the next delta.
                Self::schedule(
                    c,
                    &mut d,
                    c.now(),
                    Event::Bridge(BridgeTimer::RetryEthernet {
                        source: link.source,
                    }),
                )?;
            }

            if let Some(tsn) = &self.tsn {
                let input = self.port_input(direction, &d)?;
                let mac_bytes = self.transfer(&d, t).row.wire.mac_bytes;
                let clock = self.clock_for_port(&d, &link.from_port).cloned();
                let (next, checked_eof, checked_release) = tsn.plan_start_clock_in(
                    std::mem::take(&mut d.tsn),
                    &link.from_port,
                    p as u8,
                    c.now(),
                    mac_bytes,
                    &input,
                    clock.as_ref(),
                )?;
                if checked_eof != eof || checked_release != release {
                    return Err(error("TSN/shared wire timing mismatch"));
                }
                self.stage_tsn(&mut d, next, &[link.from_port.clone()])?;
            }
        }
        if let Some(state) = &self.bridge {
            for bus in 0..self.prepared.bridge.as_ref().unwrap().can.buses.len() {
                let tx = self.bridge_tx(&d);
                state.plan_can_arbitration_in_delta(
                    d.bridge.as_mut().unwrap(),
                    bus,
                    c.now(),
                    &tx,
                )?;
                self.bridge_actions(&mut d, c, u64::MAX)?;
            }
        }
        self.refresh_tsn(&mut d, c, false)?;
        self.emit_tsn(&mut d, c)?;
        self.sync_pause_links(&mut d, c)?;
        self.sync_preemption_wire_links(&mut d, c)?;
        self.sync_phy_links(&mut d, c)?;
        self.sync_mac_merge_links(&mut d, c)?;
        self.sync_gptp_links(&mut d, c)?;
        self.emit_extensions(&mut d, c)?;
        self.emit_mac_merge(&mut d, c)?;
        self.check_extension_limits(&d)?;
        self.arm_control(&mut d, c)?;
        self.emit_rows(&mut d, c)?;
        self.queue_points(&mut d, c.now());
        Ok(d)
    }
    fn bridge_tx(&self, d: &NetworkDelta) -> BridgeTxView {
        let mut view = BridgeTxView::default();
        for (i, link) in self.prepared.ethernet.directions.iter().enumerate() {
            let q = self.queue(d, i);
            let port_capacity = self.prepared.ethernet.devices[link.source].queue_capacity;
            let total = q.waiting.iter().map(|v| v.len() as u64).sum();
            for p in 0..8 {
                let config = self
                    .prepared
                    .ethernet
                    .outputs
                    .iter()
                    .find(|v| v.port == link.from_port)
                    .and_then(|o| o.queues.iter().find(|q| q.priority == p as u8));
                view.ethernet.insert(
                    (link.source, p as u8),
                    TxQueueView {
                        capacity_frames: config.map_or(port_capacity, |q| q.capacity_frames),
                        capacity_bytes: config.and_then(|q| q.capacity_bytes),
                        queued_frames: q.waiting[p].len() as u64,
                        queued_bytes: q.bytes[p],
                        port_capacity_frames: port_capacity,
                        port_queued_frames: total,
                    },
                );
            }
        }
        view
    }
    fn bridge_event(
        &self,
        event: &BridgeEvent,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        limit: u64,
    ) -> Result<()> {
        let Some(state) = &self.bridge else {
            return Ok(());
        };
        let tx = self.bridge_tx(d);
        state.plan_in_delta(d.bridge.as_mut().unwrap(), event, c.now(), &tx, limit)?;
        self.bridge_actions(d, c, limit)
    }
    fn bridge_actions(&self, d: &mut NetworkDelta, c: &mut Context<'_>, limit: u64) -> Result<()> {
        let mut actions: VecDeque<_> =
            std::mem::take(&mut d.bridge.as_mut().unwrap().actions).into();
        while let Some(action) = actions.pop_front() {
            match action {
                BridgeAction::PoliceEthernet {
                    conversion,
                    branch,
                    conversion_id,
                    branch_id,
                    child_id,
                    source,
                    wire,
                    vid,
                    priority,
                } => {
                    let ingress = format!("{}.rx", self.prepared.ethernet.devices[source].id);
                    let id = replication::identifier(&["synthetic", &branch_id]);
                    if self.receptions.iter().any(|r| r.row.reception_id == id)
                        || d.receptions.iter().any(|(_, r)| r.row.reception_id == id)
                    {
                        return Err(error("synthetic/native reception ID collision"));
                    }
                    let tsn = self
                        .tsn
                        .as_ref()
                        .ok_or_else(|| error("composite policing requires TSN"))?;
                    let ingress_clock = self
                        .clocks_after(d)
                        .and_then(|clocks| clocks.get(source))
                        .cloned();
                    let delta = tsn.plan_arrival_clock_in(
                        std::mem::take(&mut d.tsn),
                        &ingress,
                        &wire.dst_mac,
                        vid,
                        priority,
                        wire.mac_bytes,
                        &id,
                        c.now(),
                        ingress_clock.as_ref(),
                    )?;
                    let policing = delta
                        .psfp
                        .as_ref()
                        .ok_or_else(|| error("missing synthetic policing verdict"))?;
                    let reason = (policing.verdict == "drop").then(|| policing.reason.clone());
                    let data = json!({"ingress_id":id,"conversion_id":conversion_id,"branch_id":branch_id,"ingress":ingress,"child_planned_id":child_id,"stream_id":policing.stream_id,"vid":vid.to_string(),"priority":priority.to_string(),"mac_bytes":wire.mac_bytes.to_string(),"offered_ps":c.now().to_string(),"verdict":policing.verdict,"reason":reason});
                    d.tsn = delta;
                    Self::record(c, d, "dir.composite.ingress", id, data)?;
                    let tx = self.bridge_tx(d);
                    self.bridge.as_ref().unwrap().resolve_ethernet_policing(
                        d.bridge.as_mut().unwrap(),
                        conversion,
                        branch,
                        c.now(),
                        reason,
                        &tx,
                    )?;
                    // Existing waiting-record intents precede the resolved verdict.
                    // Append the continuation to avoid overwriting its final drop/admission.
                    actions.extend(std::mem::take(&mut d.bridge.as_mut().unwrap().actions));
                }
                BridgeAction::Schedule { at, phase, event } => {
                    if phase == 0 {
                        if at == c.now() {
                            self.bridge_event(&BridgeEvent::Timer(event), d, c, limit)?;
                        } else {
                            self.due_mut(d).push(Due {
                                at,
                                kind: Completion::Bridge(event),
                            });
                        }
                    } else {
                        Self::schedule(c, d, at, Event::Bridge(event))?;
                    }
                }
                BridgeAction::DirtyCan { bus } => {
                    if bus >= self.prepared.bridge.as_ref().unwrap().can.buses.len() {
                        return Err(error("unknown CAN arbitration resource"));
                    }
                    c.request_arbitration("network").map_err(model)?;
                }
                BridgeAction::InjectEthernet {
                    conversion_index,
                    branch_index,
                    source,
                    wire,
                    vid,
                    priority,
                    origin_id,
                    parent_id,
                    visited_gateways,
                    branch_lineage,
                    segment_id,
                    generated_ps,
                    branch_id,
                    child_id,
                } => {
                    let gateway_injection = if self.prepared.has_gateway_replication() {
                        let evidence = self.bridge.as_ref().unwrap().gateway_source_evidence(
                            conversion_index,
                            branch_index,
                            d.bridge.as_ref(),
                        )?;
                        self.prepared
                            .extensions
                            .as_ref()
                            .unwrap()
                            .replication
                            .as_ref()
                            .unwrap()
                            .streams
                            .iter()
                            .any(|s| s.source == evidence.selector)
                            .then(|| GatewayInjection {
                                conversion_index,
                                branch_index,
                                selector: evidence.selector,
                                child_id: child_id.clone(),
                            })
                    } else {
                        None
                    };
                    let f = self.frames.len()
                        + d.frames
                            .iter()
                            .filter(|(i, _)| *i >= self.frames.len())
                            .count();
                    let device = &self.prepared.ethernet.devices[source];
                    let row = EthernetFrameRecord {
                        frame_id: child_id.clone(),
                        time_ps: c.now(),
                        source: device.id.clone(),
                        source_vlan_id: Some(vid),
                        wire: wire.clone(),
                        flow_id: Some(child_id.clone()),
                        priority,
                        deadline_ps: None,
                        generated_ps: c.now(),
                        ready_ps: Some(c.now()),
                    };
                    d.frames.push((
                        f,
                        Frame {
                            generator: None,
                            row,
                            ip: None,
                            visits: 0,
                            lineage: Some(BridgeLineage {
                                origin_id,
                                parent_id,
                                visited_gateways,
                                branch_lineage,
                                segment_id,
                                generated_ps,
                            }),
                            branch_id: Some(branch_id),
                            gateway_injection,
                            terminal_indications: Vec::new(),
                        },
                    ));
                    let dirs = self
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| v.source == source)
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>();
                    let prior = d.transfers.len();
                    #[cfg(test)]
                    if d.frames
                        .iter()
                        .filter(|(i, _)| *i >= self.frames.len())
                        .count()
                        == 2
                    {
                        crate::allocation::reservation_checkpoint(
                            "network_composite_second_branch",
                        )?;
                    }
                    crate::allocation::reservation_checkpoint(
                        "network_gateway_before_replication",
                    )?;
                    if !self.replication_offer(
                        f,
                        source,
                        l2::Classification {
                            vid,
                            priority,
                            dei: wire.tag.map_or(0, |t| t.dei),
                        },
                        d,
                        c,
                    )? {
                        self.offer(
                            CopyOrigin {
                                circuit: None,
                                frame: f,
                                parent: None,
                                visit: 0,
                            },
                            &wire,
                            l2::Classification {
                                vid,
                                priority,
                                dei: wire.tag.map_or(0, |t| t.dei),
                            },
                            &dirs,
                            d,
                            c,
                        )?;
                    }
                    if d.transfers[prior..]
                        .iter()
                        .any(|(_, t)| t.row.status != "queued")
                    {
                        return Err(error(
                            "bridge reserved Ethernet admission disagrees with media queue",
                        ));
                    }
                }
                BridgeAction::Record {
                    schema,
                    record_id,
                    time_ps,
                    data,
                } => {
                    if time_ps != c.now() {
                        return Err(error("bridge record time mismatch"));
                    }
                    let data = if schema == "can.request" {
                        data
                    } else {
                        decimal(data)
                    };
                    Self::record(c, d, schema, record_id, data)?;
                }
            }
        }
        d.future_generation |= d.bridge.as_ref().unwrap().future_generation;
        d.bridge.as_mut().unwrap().clear_ethernet_reservations();
        Ok(())
    }
    /// Called only after committing the ordinary physical prefix SOF to this transaction.
    fn notify_bridge_source_sof(
        &self,
        t: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let transfer = self.transfer(d, t);
        if transfer.row.parent_transfer_id.is_some() {
            return Ok(());
        }
        let frame = self.frame(d, transfer.frame);
        let Some(lineage) = frame.lineage.clone() else {
            return Ok(());
        };
        let link = &self.prepared.ethernet.directions[transfer.direction];
        if transfer.row.sof_ps != Some(c.now()) {
            return Err(error("Gateway source notification without actual SOF"));
        }
        let targets = if let Some(stream) = self.replication_source_stream(frame) {
            match &self
                .prepared
                .extensions
                .as_ref()
                .unwrap()
                .replication
                .as_ref()
                .unwrap()
                .streams[stream]
                .placement
            {
                crate::types::network::ReplicationPlacement::BridgePair(pair)
                    if frame.gateway_injection.is_some() =>
                {
                    if pair.prefix.directions.first() != Some(&transfer.direction)
                        || link.source
                            != self
                                .prepared
                                .extensions
                                .as_ref()
                                .unwrap()
                                .replication
                                .as_ref()
                                .unwrap()
                                .streams[stream]
                                .source_device
                    {
                        return Err(error("Gateway source SOF at foreign prefix"));
                    }
                    vec![
                        self.prepared.ethernet.devices[pair.destination_device]
                            .id
                            .clone(),
                    ]
                }
                _ => self.ethernet_targets(
                    link.source,
                    frame.row.source_vlan_id.unwrap(),
                    &transfer.row.wire,
                ),
            }
        } else {
            self.ethernet_targets(
                link.source,
                frame.row.source_vlan_id.unwrap(),
                &transfer.row.wire,
            )
        };
        let event = BridgeEvent::EthernetSof {
            branch_id: frame.branch_id.clone(),
            lineage,
            source_record_id: frame.row.frame_id.clone(),
            targets,
        };
        self.bridge_event(&event, d, c, u64::MAX)
    }
    /// One indication per actual received suffix, including later accepted recovery rounds.
    fn notify_terminal_delivery(
        &self,
        ri: usize,
        reception: &Reception,
        transfer: &Transfer,
        recovery_round: Option<usize>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(lineage) = self.frame(d, transfer.frame).lineage.clone() else {
            return Ok(());
        };
        let endpoint = self
            .effective_link(transfer.direction, reception.segment_receiver)?
            .destination;
        if reception.row.status != "received"
            || self.prepared.ethernet.devices[endpoint].kind != "endpoint"
            || reception.row.ready_ps != Some(c.now())
        {
            return Err(error(
                "Gateway terminal notification without actual endpoint receipt",
            ));
        }
        let terminal_id = self.prepared.ethernet.devices[endpoint].id.clone();
        if self.frame(d, transfer.frame).gateway_injection.is_some() {
            let mut frame = self.frame(d, transfer.frame).clone();
            if frame.terminal_indications.iter().any(|i| i.reception == ri) {
                return Ok(());
            }
            frame.terminal_indications.push(TerminalIndication {
                reception: ri,
                recovery_round,
            });
            let data = decimal(
                json!({"origin_id":lineage.origin_id,"segment_id":lineage.segment_id,
                "branch_lineage":lineage.branch_lineage,"source_frame_id":frame.row.frame_id,
                "terminal_id":terminal_id,"reception_id":reception.row.reception_id,"received_ps":c.now(),
                "recovery_round":recovery_round,"generated_ps":lineage.generated_ps,"end_to_end_ps":c.now().checked_sub(lineage.generated_ps).ok_or_else(||error("negative Gateway delivery"))?}),
            );
            Self::record(
                c,
                d,
                "dir.can_ethernet.terminal_indication",
                reception.row.reception_id.clone(),
                data,
            )?;
            put(&mut d.frames, transfer.frame, frame);
        }
        self.bridge_event(
            &BridgeEvent::TerminalReceived {
                lineage,
                terminal_id,
                reception_id: reception.row.reception_id.clone(),
            },
            d,
            c,
            u64::MAX,
        )
    }
    fn ethernet_targets(&self, source: usize, vid: u16, wire: &EthernetWireFrame) -> Vec<String> {
        let base = &self.prepared.ethernet;
        // SOF freezes intended recipients. Admission and forwarding failures leave
        // those opportunities incomplete rather than removing the denominator.
        let mut targets: Vec<_> = base
            .devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| {
                let member = base
                    .port_policies
                    .iter()
                    .any(|policy| policy.device == index && policy.vlans.contains_key(&vid));
                (index != source
                    && device.kind == "endpoint"
                    && member
                    && (wire.dst_mac == "ff:ff:ff:ff:ff:ff"
                        || device.mac.as_ref() == Some(&wire.dst_mac)
                        || device.subscriptions.contains(&(vid, wire.dst_mac.clone()))))
                .then(|| device.id.clone())
            })
            .collect();
        targets.sort();
        targets
    }
    fn port_input(&self, direction: usize, d: &NetworkDelta) -> Result<PortInput> {
        let q = self.queue(d, direction);
        let link = &self.prepared.ethernet.directions[direction];
        let mut input = PortInput {
            policy_epoch: self.epoch_after(d),
            ..PortInput::default()
        };
        for p in 0..8 {
            input.backlog[p] = !q.waiting[p].is_empty();
            if let Some(&t) = q.waiting[p].front() {
                let row = self.transfer(d, t);
                let release = if self.prepared.has_phy_mac_service()
                    && self.phy_attachment_for(direction).is_some()
                    && !self.merge_raw_transport(direction)
                {
                    self.phy_occupancy_ps(direction, row.row.wire.mac_bytes)?
                } else {
                    l2::timing(0, row.row.wire.mac_bytes, link.bitrate_bps, 0)?.1
                };
                input.heads[p] = Some(Head {
                    occupancy_ps: release,
                });
            }
        }
        if self.preemption_direction(direction) {
            input = self.preemption_input(direction, d)?;
        }
        if let Some(host) = self.wire_host(d) {
            for id in &host.queues[direction] {
                let tx = &host.tx[id];
                if tx.is_mac_control() {
                    continue;
                }
                let p = self.wire_priority(direction, id, d) as usize;
                input.backlog[p] = true;
                let occupancy = if self.phy_attachment_for(direction).is_some()
                    && !self.merge_raw_transport(direction)
                {
                    self.phy_occupancy_ps(direction, tx.intent.bytes.len() as u64)?
                } else {
                    l2::timing(0, tx.intent.bytes.len() as u64, link.bitrate_bps, 0)?.1
                };
                if input.heads[p]
                    .as_ref()
                    .is_none_or(|h| occupancy < h.occupancy_ps)
                {
                    input.heads[p] = Some(Head {
                        occupancy_ps: occupancy,
                    });
                }
            }
        }
        if self.preemption_direction(direction)
            && self
                .wire_host(d)
                .is_none_or(|h| h.active[direction].is_none())
        {
            return Ok(input);
        }
        if self.merge_direction(direction) {
            let state = d.mac_merge.as_ref().or(self.mac_merge.as_ref()).unwrap();
            let port = &state.ports[direction];
            input.merge = port.accounting;
            if port.lldp_pending && state.identity.ports[direction].is_some() {
                let class = self.merge_lldp_priority(direction) as usize;
                let head = self.merge_lldp_head(direction, d)?;
                input.backlog[class] = true;
                if input.heads[class]
                    .as_ref()
                    .is_none_or(|h| head.occupancy_ps < h.occupancy_ps)
                {
                    input.heads[class] = Some(head)
                }
            }
            for t in [port.selected_pmac, port.selected_emac]
                .into_iter()
                .flatten()
            {
                let priority = self.transfer(d, t).row.priority as usize;
                input.backlog[priority] = true;
                let head = self.merge_retained_head(t, d)?;
                if input.heads[priority]
                    .as_ref()
                    .is_none_or(|h| head.occupancy_ps < h.occupancy_ps)
                {
                    input.heads[priority] = Some(head);
                }
            }
            return Ok(input);
        }
        if !self.preemption_direction(direction) && self.configured_endpoint(direction).is_some() {
            input.service = Some(self.media_service_view(direction, d));
            return Ok(input);
        }
        if self.prepared.has_phy_mac_service() && self.phy_attachment_for(direction).is_some() {
            input.service = Some(self.phy_service_view(direction, d));
            return Ok(input);
        }
        if let Some(id) = self.wire_host(d).and_then(|h| h.active[direction].as_ref()) {
            use crate::runtime::ethernet::tsn::merge::{
                MacServiceView, Segment, ServiceAccounting,
            };
            let tx = &self.wire_host(d).unwrap().tx[id];
            if self.preemption_direction(direction) && !tx.is_mac_control() {
                return Ok(input);
            }
            let priority = (!tx.is_mac_control()).then(|| self.wire_priority(direction, id, d));
            let mut retained_backlog = [false; 8];
            if let Some(p) = priority {
                retained_backlog[p as usize] = tx.eof.is_none();
            }
            input.service = Some(MacServiceView {
                active_work: Some(id.clone()),
                retained_backlog,
                transmit_class: if tx.sof.is_some() && tx.eof.is_none() {
                    priority
                } else {
                    None
                },
                busy: true,
                release_ps: tx.planned_release,
                next_transition_ps: tx.planned_eof.filter(|_| tx.eof.is_none()),
                segment: Segment::Control,
                accounting: ServiceAccounting::LegacyCapped,
            });
            return Ok(input);
        }
        if let Some(t) = q.active {
            let row = self.transfer(d, t);
            input.sending = Some(Sending {
                priority: row.row.priority,
                release_ps: row
                    .row
                    .planned_release_ps
                    .or_else(|| self.merge_physical_release(direction, d))
                    .or_else(|| self.phy_attachment_for(direction).map(|_| u64::MAX))
                    .ok_or_else(|| error("missing sending release"))?,
            });
        }
        Ok(input)
    }
    fn stage_tsn(&self, d: &mut NetworkDelta, delta: TsnDelta, ports: &[String]) -> Result<()> {
        for name in ports {
            if let Some(i) = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|p| p.from_port == *name)
                .filter(|i| self.merge_raw_transport(*i))
            {
                if self.tsn.as_ref().is_some_and(|tsn| {
                    tsn.merge_schedule(&d.tsn, name).ok().map(|s| s.0)
                        != tsn.merge_schedule(&delta, name).ok().map(|s| s.0)
                }) {
                    self.merge_permission_changed(i, d)?;
                }
            }
        }
        if let Some(prepared) = &self.prepared.tsn {
            for port in ports {
                if let Some(&i) = prepared.port_index.get(port) {
                    if self
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|d| &d.from_port == port)
                        .is_some_and(|direction| self.preemption_direction(direction))
                    {
                        let wake = delta.wakes.iter().rev().find(|w| &w.port == port).cloned();
                        let previous = d.tsn_wakes.get(&i).unwrap_or(&self.tsn_wakes[i]);
                        if previous != &wake {
                            let direction = self
                                .prepared
                                .ethernet
                                .directions
                                .iter()
                                .position(|d| &d.from_port == port)
                                .ok_or_else(|| error("missing preemption direction"))?;
                            let state = self.preemption_mut(d)?;
                            let p = &mut state.ports[direction];
                            p.wake_epoch = add(p.wake_epoch, 1)?;
                            p.wake_due = wake.as_ref().map(|w| w.time_ps);
                        }
                    }
                    d.tsn_wakes.insert(
                        i,
                        delta.wakes.iter().rev().find(|w| &w.port == port).cloned(),
                    );
                }
            }
        }
        self.merge_journal_stage(d, &delta, ports)?;
        d.tsn = delta;
        Ok(())
    }
    fn refresh_tsn(&self, d: &mut NetworkDelta, c: &mut Context<'_>, control: bool) -> Result<()> {
        let Some(state) = &self.tsn else {
            return Ok(());
        };
        let prepared = self.prepared.tsn.as_ref().unwrap();
        let mut inputs = BTreeMap::new();
        for (i, o) in prepared.outputs.iter().enumerate() {
            let direction = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|v| v.from_port == o.port)
                .ok_or_else(|| error("unknown TSN direction"))?;
            let due = self.tsn_wakes[i].as_ref().is_some_and(|w| {
                w.time_ps == c.now()
                    && state.wake_valid_clock_in(&d.tsn, w, self.clock_for_port(d, &o.port))
            });
            let update = prepared
                .updates
                .iter()
                .any(|u| u.port == o.port && u.effective_at_ps == c.now());
            let policy = d
                .dynamic
                .as_ref()
                .is_some_and(|v| v.dirty_ports.contains(&o.port));
            if d.queues.contains_key(&direction)
                || due
                || update
                || policy
                || (control && c.now() == 0)
                || d.clocks.is_some()
                || d.wire.is_some()
            {
                inputs.insert(o.port.clone(), self.port_input(direction, d)?);
            }
        }
        if inputs.is_empty() {
            return Ok(());
        }
        let ports = inputs.keys().cloned().collect::<Vec<_>>();
        if control {
            let clocks = inputs
                .keys()
                .filter_map(|port| {
                    self.clock_for_port(d, port)
                        .cloned()
                        .map(|v| (port.clone(), v))
                })
                .collect();
            let delta = state.plan_control_clock_in(
                std::mem::take(&mut d.tsn),
                c.now(),
                &inputs,
                &clocks,
            )?;
            if !delta.selected.is_empty() {
                c.request_arbitration("network").map_err(model)?;
            }
            self.stage_tsn(d, delta, &ports)?;
        } else {
            for (port, input) in inputs {
                let clock = self.clock_for_port(d, &port).cloned();
                let delta = state.plan_port_clock_in(
                    std::mem::take(&mut d.tsn),
                    &port,
                    c.now(),
                    &input,
                    clock.as_ref(),
                )?;
                if !delta.selected.is_empty() {
                    c.request_arbitration("network").map_err(model)?;
                }
                self.stage_tsn(d, delta, &[port])?;
            }
        }
        for port in &ports {
            if let Some(direction) = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|l| &l.from_port == port)
            {
                self.phy_set_start_permission(direction, d, c, true)?;
            }
        }
        self.media_check_gate_overruns(d, c)?;
        Ok(())
    }
    fn emit_tsn(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let records = std::mem::take(&mut d.tsn.records);
        for record in records {
            let schema = record
                .schema()
                .strip_prefix("dir.")
                .unwrap_or(record.schema());
            let id = format!("{}:{}", schema, d.record_sequence);
            let value = decimal(record.data(d.record_sequence));
            Self::record(c, d, schema, id, value)?;
        }
        Ok(())
    }
    fn queue_points(&self, d: &mut NetworkDelta, now: u64) {
        if let (Some(state), Some(delta)) = (&self.bridge, &d.bridge) {
            for (target, value) in state.can_queue_occupancy_after(delta) {
                d.points.push(Point {
                    event_seq: None,
                    effect_seq: None,
                    time_ps: now,
                    target,
                    metric: "queue_length".into(),
                    value,
                    request_id: None,
                    receiver: None,
                    reason: None,
                });
            }
        }

        for (&i, q) in &d.queues {
            let port = &self.prepared.ethernet.directions[i].from_port;
            d.points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: now,
                target: format!("{port}.queue"),
                metric: "queue_length".into(),
                value: q.waiting.iter().map(|v| v.len() as u64).sum(),
                request_id: None,
                receiver: None,
                reason: None,
            });
            for p in 0..8 {
                for (metric, value) in [
                    ("queue_length", q.waiting[p].len() as u64),
                    ("queue_bytes", q.bytes[p]),
                ] {
                    d.points.push(Point {
                        event_seq: None,
                        effect_seq: None,
                        time_ps: now,
                        target: format!("{port}.queue.{p}"),
                        metric: metric.into(),
                        value,
                        request_id: None,
                        receiver: None,
                        reason: None,
                    });
                }
            }
        }
    }
    fn emit_rows(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut records = Vec::new();
        for (index, f) in &d.frames {
            if self.frames.get(*index).is_some_and(|old| {
                old.row.time_ps == f.row.time_ps && old.row.ready_ps == f.row.ready_ps
            }) {
                continue;
            }
            let row = &f.row;
            let mut v = wire(&row.wire);
            v["source"] = json!(row.source);
            v["generated_ps"] = json!(row.generated_ps.to_string());
            v["ready_ps"] = opt(row.ready_ps);
            v["flow_id"] = json!(row.flow_id);
            v["priority"] = json!(row.priority.to_string());
            v["deadline_ps"] = opt(row.deadline_ps);
            v["source_vlan_id"] = opt(row.source_vlan_id.map(u64::from));
            v["ip_multicast"] = decimal(json!(f.ip));
            if self.bridge.is_some() && self.dynamic.is_none() {
                v.as_object_mut().unwrap().remove("ip_multicast");
            }
            records.push((
                if self.bridge.is_some() && self.dynamic.is_none() {
                    "ethernet.frame"
                } else {
                    "ethernet.dynamic.frame"
                },
                row.frame_id.clone(),
                v,
            ));
        }
        for (_, t) in &d.transfers {
            let row = &t.row;
            if self.preemption_direction(t.direction) {
                records.push((
                    "dir.preemption.transfer",
                    row.transfer_id.clone(),
                    self.preemption_transfer(t, c.now()),
                ));
                continue;
            }
            let mut v = json!({"frame_id":row.frame_id,"parent_transfer_id":row.parent_transfer_id,"from_port":row.from_port,"to_port":row.to_port,"queued_ps":row.queued_ps.to_string(),"sof_ps":opt(row.sof_ps),"eof_ps":opt(row.eof_ps),"release_ps":opt(row.release_ps),"arrival_ps":opt(row.arrival_ps),"planned_eof_ps":opt(row.planned_eof_ps),"planned_release_ps":opt(row.planned_release_ps),"planned_arrival_ps":opt(row.planned_arrival_ps),"status":row.status,"drop_reason":row.drop_reason,"queue_id":row.queue_id,"priority":row.priority.to_string(),"vlan_id":opt(row.vlan_id.map(u64::from)),"wire":wire(&row.wire),"visit_id":t.visit.to_string(),"copy_id":row.transfer_id,"policy_epoch_offer":t.offer_epoch.to_string(),"policy_epoch_sof":opt(t.sof_epoch)});
            if self.bridge.is_some() && self.dynamic.is_none() {
                for k in [
                    "visit_id",
                    "copy_id",
                    "policy_epoch_offer",
                    "policy_epoch_sof",
                ] {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            if let Some(media) = &row.media {
                v["media"] = decimal(json!(media));
            }
            records.push((
                if self.bridge.is_some() && self.dynamic.is_none() {
                    "ethernet.transfer"
                } else {
                    "ethernet.dynamic.transfer"
                },
                row.transfer_id.clone(),
                v,
            ));
        }
        for (_, r) in &d.receptions {
            let row = &r.row;
            let mut v = json!({"frame_id":row.frame_id,"transfer_id":row.transfer_id,"ingress":row.ingress,"observed_ps":row.observed_ps.to_string(),"ready_ps":opt(row.ready_ps),"planned_ready_ps":opt(row.planned_ready_ps),"status":row.status,"reason":row.reason,"egress_transfer_ids":row.egress_transfer_ids,"vlan_id":opt(row.vlan_id.map(u64::from)),"priority":row.priority.to_string(),"visit_id":r.visit.to_string(),"policy_epoch_ingress":r.epoch.to_string()});
            if self.bridge.is_some() && self.dynamic.is_none() {
                for k in ["visit_id", "policy_epoch_ingress"] {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            records.push((
                if self.bridge.is_some() && self.dynamic.is_none() {
                    "ethernet.reception"
                } else {
                    "ethernet.dynamic.reception"
                },
                row.reception_id.clone(),
                v,
            ));
        }
        for (s, id, v) in records {
            Self::record(c, d, s, id, v)?;
        }
        Ok(())
    }
    pub(crate) fn reserve(&mut self, d: &NetworkDelta) -> Result<()> {
        if d.configured_media.is_some() || d.mac_control.is_some() {
            crate::allocation::reservation_checkpoint("network_media_pause_reserve")?;
        }
        if let (Some(state), Some(delta)) = (&mut self.bridge, &d.bridge) {
            state.reserve(delta)?;
        }
        if self.prepared.has_gateway_replication() {
            crate::allocation::reservation_checkpoint("network_gateway_reserve")?;
        }
        for (result, label) in [
            (
                self.frames.try_reserve(
                    d.frames
                        .iter()
                        .filter(|(i, _)| *i >= self.frames.len())
                        .count(),
                ),
                "frames",
            ),
            (
                self.transfers.try_reserve(
                    d.transfers
                        .iter()
                        .filter(|(i, _)| *i >= self.transfers.len())
                        .count(),
                ),
                "transfers",
            ),
            (
                self.receptions.try_reserve(
                    d.receptions
                        .iter()
                        .filter(|(i, _)| *i >= self.receptions.len())
                        .count(),
                ),
                "receptions",
            ),
        ] {
            result.map_err(|_| error(format!("network allocation failed: {label}")))?;
        }
        Ok(())
    }
    pub(crate) fn apply(&mut self, d: NetworkDelta) {
        if let Some(state) = d.mac_merge {
            self.mac_merge = Some(state);
        }
        if let Some(state) = d.wire {
            self.wire = Some(state);
        }
        if let Some(state) = d.multicast_active {
            self.multicast_active = Some(state);
        }
        if let Some(history) = d.multicast_history {
            self.multicast_history = history;
        }
        if let Some(cursor) = d.multicast_cursor {
            self.multicast_cursor = cursor;
        }
        if let Some(wake) = d.multicast_wake {
            self.multicast_wake = wake;
        }
        if let Some(state) = d.qcc {
            self.qcc = Some(state);
        }
        if let Some(sources) = d.qcc_sources {
            self.qcc_sources = sources;
        }
        if let Some(active) = d.segment_active {
            self.segment_active = active;
        }
        if let Some(arrivals) = d.segment_arrivals {
            self.segment_arrivals = arrivals;
        }
        if let Some(fdb) = d.provider_fdb {
            self.provider_fdb = Some(fdb);
        }
        if let Some(state) = d.replication {
            self.replication = Some(state);
        }
        if let Some(state) = d.preemption {
            self.preemption = Some(state);
        }
        if let Some(clocks) = d.clocks {
            self.clocks = Some(clocks);
        }
        if let Some(gptp) = d.gptp {
            self.gptp = Some(gptp);
        }
        if let Some(cursor) = d.clock_cursor {
            self.clock_cursor = cursor;
        }
        if let Some(phy) = d.phy {
            self.phy = phy;
        }
        if let Some(wakes) = d.phy_wakes {
            self.phy_wakes = wakes;
        }
        if let Some(boundaries) = d.phy_boundaries {
            self.phy_boundaries = boundaries;
        }
        if let Some(arrivals) = d.phy_arrivals {
            self.phy_arrivals = arrivals;
        }
        if let Some(admitted) = d.phy_admitted {
            self.phy_admitted = admitted;
        }
        if let Some(state) = d.configured_media {
            self.configured_media = state;
        }
        if let Some(state) = d.mac_control {
            self.mac_control = state;
        }
        if let Some(services) = d.phy_services {
            self.phy_services = services;
        }
        if let Some(cursors) = d.cursors {
            self.cursors = cursors;
        }
        if let (Some(state), Some(delta)) = (&mut self.bridge, d.bridge) {
            state.apply(delta);
        }
        apply(&mut self.frames, d.frames);
        apply(&mut self.transfers, d.transfers);
        apply(&mut self.receptions, d.receptions);
        if let Some(tsn) = &mut self.tsn {
            tsn.apply(d.tsn);
        }
        for (i, wake) in d.tsn_wakes {
            self.tsn_wakes[i] = wake;
        }
        for (i, q) in d.queues {
            self.queues[i] = q;
        }
        if let Some(delta) = d.dynamic {
            self.dynamic.as_mut().unwrap().apply(delta);
        }
        if let Some(due) = d.due {
            self.due = due;
        }
        if let Some(control) = d.control {
            self.control = control;
        }
        self.control_generation = d.control_generation;
        self.record_sequence = d.record_sequence;
        self.next_visit = d.next_visit;
        self.future_generation = d.future_generation;
        self.pending_direct = d.pending_direct;
    }
    pub(crate) fn snapshot(&self) -> EthernetSnapshot {
        EthernetSnapshot {
            frames: self.frames.iter().map(|f| f.row.clone()).collect(),
            transfers: self.transfers.iter().map(|t| t.row.clone()).collect(),
            receptions: self.receptions.iter().map(|r| r.row.clone()).collect(),
            attempts: vec![],
        }
    }
    pub(crate) fn runtime_snapshot(&self, h: u64) -> Result<Value> {
        let clocks = self.clocks.as_ref().map(|clocks| clocks.iter().enumerate().map(|(i, clock)| {
            let local = clock.local_at(h).map_err(|e| error(e.to_string()))?;
            let (rate_num, rate_den) = clock.rate();
            Ok(json!({"node":self.prepared.ethernet.devices[i].id,"global_ps":h.to_string(),
                "local_numerator":local.numerator().to_string(),"local_denominator":local.denominator().to_string(),
                "rate_num":rate_num.to_string(),"rate_den":rate_den.to_string(),
                "generation":clock.generation().to_string()}))
        }).collect::<Result<Vec<_>>>()).transpose()?;
        let tsn=self.tsn.as_ref().map(|s|s.snapshot(h)).transpose()?.map(|s|json!({"ports":s.ports.into_iter().map(|p|json!({"port":p.port,"schedule_id":p.schedule_id,"schedule_generation":p.schedule_generation.to_string(),"wake_generation":p.wake_generation.to_string(),"next_wake_ps":p.next_wake_ps.map(|v|v.to_string()),"credits":p.credits.into_iter().map(|c|json!({"priority":c.priority.to_string(),"credit":decimal(json!(c.credit)),"last_ps":c.last_ps.to_string(),"mode":c.mode,"backlog":c.backlog,"sending":c.sending})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"meters":s.meters.into_iter().map(|m|json!({"stream_id":m.stream_id,"committed":m.committed.to_string(),"peak":m.peak.to_string(),"last_evaluated_ps":m.last_evaluated_ps.to_string()})).collect::<Vec<_>>(),"update_cursor":s.update_cursor.to_string()}));
        let redundancy = self.replication.as_ref().map(|s| {
            let mut snapshot = s.snapshot(h);
            snapshot["transfer_circuits"] = json!(
                self.transfers
                    .iter()
                    .filter_map(|t| {
                        t.circuit.as_ref().map(|c| {
                            let mut value = json!({
                                "branch_id": c.branch,
                                "transfer_id": t.row.transfer_id,
                                "hop_index": c.hop.to_string(),
                            });
                            if let Some(distributed) = &c.distributed {
                                value["distributed"] = json!(distributed);
                            }
                            value
                        })
                    })
                    .collect::<Vec<_>>()
            );
            snapshot
        });
        let mut value = json!({"dynamic":self.dynamic.as_ref().map(|s|decimal(s.snapshot_tables())),"tsn":tsn,"bridge":self.bridge.as_ref().map(|b|decimal(b.snapshot())),"redundancy":redundancy,"preemption":self.preemption_snapshot(),"wire":self.wire_snapshot(h)?});
        if self.prepared.mac_control.is_some() {
            value["mac_control"] = decimal(json!(self.mac_control));
        }
        if self.prepared.ethernet.media.is_some() {
            value["configured_media"] = decimal(json!(self.configured_media));
        }
        if let Some(merge) = self.mac_merge_snapshot() {
            value["mac_merge"] = merge;
        }
        if let Some(clocks) = clocks {
            value["clocks"] = json!(clocks);
        }
        if let Some(actor) = &self.multicast_active {
            value["multicast_active"] = json!({"actor":actor,"change_cursor":self.multicast_cursor.to_string(),
                "next_wake_ps":self.multicast_wake.map(|v|v.to_string())});
        }
        Ok(value)
    }
    pub(crate) fn can_snapshot(&self) -> Option<crate::snapshot::CanSnapshot> {
        self.bridge.as_ref().map(BridgeState::can_snapshot)
    }
    fn next_generation(&self, cursors: &[u64]) -> Option<u128> {
        self.prepared
            .ethernet
            .generators
            .iter()
            .zip(cursors)
            .filter_map(|(g, &ordinal)| g.time(ordinal))
            .min()
    }
    pub(crate) fn future_generation(&self) -> bool {
        self.future_generation
    }
}
fn put<T>(rows: &mut Vec<(usize, T)>, i: usize, value: T) {
    if let Some((_, v)) = rows.iter_mut().find(|(n, _)| *n == i) {
        *v = value
    } else {
        rows.push((i, value));
    }
}
fn apply<T>(target: &mut Vec<T>, rows: Vec<(usize, T)>) {
    for (i, row) in rows {
        if i == target.len() {
            target.push(row)
        } else {
            target[i] = row
        }
    }
}
fn opt(v: Option<u64>) -> Value {
    v.map_or(Value::Null, |n| json!(n.to_string()))
}
fn wire(w: &EthernetWireFrame) -> Value {
    decimal(json!(w))
}
pub(crate) fn decimal(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::String(n.to_string()),
        Value::Array(a) => Value::Array(a.into_iter().map(decimal).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, decimal(v))).collect()),
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared(example: &str) -> crate::types::PreparedSimulation {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(example),
        )
        .unwrap()
    }
    #[test]
    fn gateway_real_conversion_and_replication_reserve_failures_are_atomic() {
        use crate::{
            CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
        };
        for (cut, fault) in [
            ("injection", "network_gateway_before_replication"),
            ("replication", "network_gateway_reserve"),
        ] {
            let p = prepared("examples/network-composite/gateway-frer/model.ini");
            let (mut session, controller) = RunSession::new(
                p.clone(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            loop {
                assert_eq!(session.advance_one().unwrap(), RunState::Running);
                let view = session.observe().unwrap();
                let last_event_time = view.common.last_event_time_ps.unwrap_or(0);
                let records = &view.registered.as_ref().unwrap().model_records;
                let ready = if cut == "injection" {
                    last_event_time >= 226_000_000
                        && records.values().any(|r| {
                            r.schema.name == "dir.can_ethernet.conversion"
                                && r.data["status"] == "waiting_tx"
                        })
                } else {
                    records.values().any(|r| {
                        r.schema.name == "ethernet.dynamic.reception"
                            && r.data["ingress"] == "Net.r.rx_s"
                            && r.data["status"] == "processing"
                    })
                };
                if ready {
                    break;
                }
            }
            controller.submit(ControlCommand::Pause).unwrap();
            assert_eq!(session.poll_control().unwrap(), RunState::Paused);
            let root = std::env::temp_dir().join(format!(
                "gateway-atomic-{cut}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let ack = session
                .checkpoint(&root, CheckpointLimits::default())
                .unwrap();
            assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
            let saved: Value =
                serde_json::from_slice(&std::fs::read(root.join("state.json")).unwrap()).unwrap();
            let horizon = session.snapshot().common.last_event_time_ps.unwrap();
            let state = NetworkState::restore_checkpoint(
                p.registered
                    .as_ref()
                    .unwrap()
                    .network
                    .as_ref()
                    .unwrap()
                    .clone(),
                &saved["network_backend"]["state"],
                horizon,
            )
            .unwrap();
            let records: Vec<(String, crate::registry::ModelRecord)> =
                checkpoint::decode(saved["network_backend"]["records"].clone()).unwrap();
            // Select the actual saved FES callback, preserving phase separation:
            // CAN release is phase0; BranchOffer is phase1; Bridge-owned receive
            // processing is a Processed completion carried by the phase0 control.
            let saved_event = saved["fes"].as_array().unwrap().iter().find_map(|entry| {
                let envelope = &entry["work"]["envelope"];
                let payload: Vec<u8> = serde_json::from_value(envelope["payload"].clone()).ok()?;
                let event: Event = serde_json::from_slice(&payload).ok()?;
                let at: u64 = envelope["time_ps"].as_str()?.parse().ok()?;
                let selected = if cut == "injection" {
                    matches!(event, Event::Bridge(BridgeTimer::BranchOffer { conversion:0, branch:0 }))
                } else {
                    matches!(event, Event::Control(g) if g == state.control_generation)
                        && state.due.iter().any(|due| due.at == at && matches!(due.kind,
                            Completion::Processed(ri) if state.receptions[ri].row.ingress == "Net.r.rx_s"))
                };
                selected.then_some((envelope, payload))
            }).expect("actual injection/replication FES callback missing");
            let envelope = Envelope {
                event_id: saved_event.0["event_id"].as_str().unwrap().parse().unwrap(),
                source: "@network".into(),
                destination: "@network".into(),
                request_id: None,
                time_ps: saved_event.0["time_ps"].as_str().unwrap().parse().unwrap(),
                input_port: None,
                schema: Schema::new(saved_event.0["schema"]["name"].as_str().unwrap(), 1),
                payload: saved_event.1,
            };
            let before = state.checkpoint();
            crate::allocation::fail_next_reservation(fault);
            let (after, evidence) = super::super::registered::vector_network_failed_callback(
                p,
                state,
                records.into_iter().map(|(_, r)| r).collect(),
                vec![envelope],
            );
            assert!(
                crate::allocation::reservation_checkpoint(fault).is_ok(),
                "fault seam not reached"
            );
            assert_eq!(
                after.checkpoint(),
                before,
                "{cut}: no child/sequence/Bridge/queue mutation may commit"
            );
            assert_eq!(evidence["committed_effects"], 0);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn intended_terminal_survives_ingress_rejection_without_completion() {
        let mut p = prepared("examples/can-ethernet/r01.ini");
        let network = Arc::make_mut(p.registered.as_mut().unwrap().network.as_mut().unwrap());
        let sink = network
            .ethernet
            .devices
            .iter()
            .position(|device| device.id == "R01.sink")
            .unwrap();
        let policy = network
            .ethernet
            .port_policies
            .iter_mut()
            .find(|policy| policy.device == sink)
            .unwrap();
        policy.admit = "tagged_only".into();
        let snapshot = super::super::registered::simulate(&p).unwrap();
        assert_eq!(snapshot.common.termination, "events_exhausted");
        let records = &snapshot.registered.as_ref().unwrap().model_records;
        let segment = records
            .values()
            .find(|row| {
                row.schema.name == "dir.can_ethernet.segment"
                    && row.data["source_record_id"].as_str().is_some_and(|id| {
                        snapshot
                            .ethernet
                            .as_ref()
                            .unwrap()
                            .frames
                            .iter()
                            .any(|frame| frame.frame_id == id)
                    })
            })
            .unwrap();
        assert_eq!(segment.data["target_count"], "1");
        assert_eq!(segment.data["targets"][0]["terminal_id"], "R01.sink");
        assert_eq!(segment.data["targets"][0]["completed_ps"], Value::Null);
        assert_eq!(
            segment.data["targets"][0]["completion_reception_id"],
            Value::Null
        );
        assert!(
            snapshot
                .ethernet
                .as_ref()
                .unwrap()
                .receptions
                .iter()
                .any(|row| row.device == "R01.sink"
                    && row.status == "filtered"
                    && row.reason.as_deref() == Some("ingress_frame_type"))
        );
    }
    #[test]
    fn intended_terminal_set_uses_source_vid_and_ignores_static_misrouting() {
        let p = prepared("examples/ethernet/dynamic/unicast.ini");
        let mut network = (**p.registered.as_ref().unwrap().network.as_ref().unwrap()).clone();
        let source = network.ethernet.generators[0].source;
        let wire = network.ethernet.generators[0].frame.clone();
        let sink = network
            .ethernet
            .devices
            .iter()
            .position(|device| device.mac.as_ref() == Some(&wire.dst_mac))
            .unwrap();
        let expected = network.ethernet.devices[sink].id.clone();
        let incoming = network
            .ethernet
            .directions
            .iter()
            .position(|direction| direction.source == source)
            .unwrap();
        let switch = network.ethernet.directions[incoming].destination;
        let ingress_output = network
            .ethernet
            .port_policies
            .iter()
            .find(|policy| policy.ingress == network.ethernet.directions[incoming].to_port)
            .unwrap()
            .port
            .clone();
        network.ethernet.devices[switch]
            .vlan_fdb
            .insert((10, wire.dst_mac.clone()), ingress_output);
        assert!(
            l2::static_egress(&network.ethernet, incoming, &wire, Some(10))
                .unwrap()
                .0
                .is_empty()
        );
        let state = NetworkState::new(Arc::new(network.clone())).unwrap();
        assert_eq!(
            state.ethernet_targets(source, 10, &wire),
            vec![expected.clone()]
        );
        // Neither tag acceptance nor receiving PVID changes source-VID membership.
        let policy = network
            .ethernet
            .port_policies
            .iter_mut()
            .find(|policy| policy.device == sink)
            .unwrap();
        policy.pvid = 20;
        policy.admit = "untagged_only".into();
        let state = NetworkState::new(Arc::new(network.clone())).unwrap();
        assert_eq!(state.ethernet_targets(source, 10, &wire), vec![expected]);
        assert!(state.ethernet_targets(source, 20, &wire).is_empty());
        let mut broadcast = wire.clone();
        broadcast.dst_mac = "ff:ff:ff:ff:ff:ff".into();
        let targets = state.ethernet_targets(source, 10, &broadcast);
        assert!(!targets.contains(&state.prepared.ethernet.devices[source].id));
        assert_eq!(targets.len(), 2);
    }
}

#[cfg(test)]
pub(crate) mod network_vector_tests;
