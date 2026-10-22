use super::codec::{Message, PortIdentity, Timestamp};
use super::math::Rat;
use crate::types::network::gptp::PreparedGptp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Role {
    Disabled,
    Master,
    Slave,
    Passive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum InformationSource {
    Disabled,
    Aged,
    Mine,
    Received,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capture {
    pub at_ps: u64,
    pub raw: Rat,
    pub packet_id: u64,
    pub link_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampled: Option<SampledCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured: Option<ConfiguredCaptureProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub express: Option<ConfiguredCaptureProof>,
}

/// Serializer-owned identity. Lifecycle epochs are distinct from this token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::runtime::network) enum MacSource {
    OrdinaryAttempt(ConfiguredAttempt),
    MergeExpress {
        transfer: usize,
        direction: usize,
        packet_id: u64,
        epoch: u64,
    },
    AbstractExpress {
        transfer: usize,
        direction: usize,
        epoch: u64,
    },
}
/// Retained by the actual producer, independently of the GP packet copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::runtime::network) struct ExpressSourceEvidence {
    pub source: MacSource,
    pub sof_ps: u64,
    pub eof_planned_ps: u64,
    pub release_planned_ps: u64,
    pub arrival_planned_ps: u64,
    pub eof_actual_ps: Option<u64>,
    pub arrival_actual_ps: Option<u64>,
    pub mac_hex: String,
}

/// Exact retained attempt identity, independent of policy/link generations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::runtime::network) struct ConfiguredAttempt {
    pub index: usize,
    pub transfer: usize,
    pub number: u64,
    pub direction: usize,
    pub physical_link: usize,
    pub side: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::runtime::network) struct ConfiguredCaptureProof {
    pub source: MacSource,
    pub endpoint: Option<(usize, usize)>,
    pub plane: ReferencePlane,
    pub reference_ps: u64,
    pub observed_at_ps: u64,
    pub sof_ps: u64,
    pub mdi_sof_ps: u64,
    pub peer_sof_ps: u64,
    pub eof_planned_ps: u64,
    pub arrival_planned_ps: u64,
    pub bitrate_bps: u64,
    pub propagation_ps: u64,
    pub tx_latency_ps: u64,
    pub peer_rx_latency_ps: u64,
}

/// Physical reference and causal observation are deliberately separate. The
/// reference is exact on the attachment's rational sample grid; observation is
/// the event-grid cut at which the producer or validated decoder reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::runtime::network) struct SampledCapture {
    pub attachment: usize,
    pub reference_sample: u64,
    pub reference: Rat,
    pub observed_at_ps: u64,
    pub physical_attempt: u32,
    pub packet_generation: Option<u64>,
    pub plane: ReferencePlane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::runtime::network) enum ReferencePlane {
    MdiMacSfd,
    IdealMacSfd,
    MdiMergeDelimiter,
    XmiiDelimiter,
    RawDecodeAvailability,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Packet {
    pub id: u64,
    pub transfer: usize,
    pub frame: usize,
    pub direction: usize,
    pub link_epoch: u64,
    pub mac_packet: Option<u64>,
    pub physical_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac_source: Option<MacSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_completed_at_ps: Option<u64>,
    pub admitted: bool,
    /// Locally constructed transmit intent, independently bound to transfer MAC bytes.
    pub message: Message,
    /// Actual ingress MAC bytes (including FCS) and decoded body after physical validation.
    /// These may differ from transmit intent when a physical adapter changes bytes and FCS.
    pub received_mac_hex: Option<String>,
    pub received_message: Option<Message>,
    pub relay: Option<Relay>,
    pub tx: Option<Capture>,
    pub rx: Option<Capture>,
    pub rx_validated: bool,
    pub tx_eof: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PdelayPending {
    pub generation: u64,
    pub sequence: u16,
    pub request_transfer: usize,
    pub t1: Option<Rat>,
    pub t2: Option<Rat>,
    pub t3: Option<Rat>,
    pub t4: Option<Rat>,
    pub peer: Option<PortIdentity>,
    pub response_transfer: Option<usize>,
    pub followup_transfer: Option<usize>,
    pub deadline_raw: Rat,
    pub deadline_ps: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Candidate {
    pub source: PortIdentity,
    pub grandmaster: [u8; 8],
    pub identity: [u8; 14],
    pub steps_removed: u16,
    pub path: Vec<[u8; 8]>,
    pub sequence: u16,
    pub transfer: usize,
    pub receipt_ps: u64,
    pub deadline_raw: Rat,
    pub deadline_ps: u64,
    pub generation: u64,
    pub time_source: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Port {
    pub role: Role,
    pub information_source: InformationSource,
    pub link_up: bool,
    pub link_epoch: u64,
    pub sync_seq: u16,
    pub announce_seq: u16,
    pub pdelay_seq: u16,
    pub pdelay_generation: u64,
    pub pdelay_due_raw: Option<Rat>,
    pub sync_generation: u64,
    pub sync_due_raw: Option<Rat>,
    pub announce_generation: u64,
    pub announce_due_raw: Option<Rat>,
    pub pending: Option<PdelayPending>,
    pub last_sample: Option<(Rat, Rat)>,
    pub last_sample_peer: Option<PortIdentity>,
    pub last_sample_index: Option<usize>,
    pub rate: Option<Rat>,
    pub delay: Option<Rat>,
    pub as_capable: bool,
    pub lost_responses: u32,
    pub candidate: Option<Candidate>,
    pub peer: Option<PortIdentity>,
}
impl Port {
    /// Figure 11-9 RESET tests the old counter, then increments or clears
    /// qualification. The loss that starts at `allowed + 1` still qualifies.
    pub(super) fn reset_after_lost_response(&mut self, allowed: u32) {
        self.pending = None;
        if self.lost_responses <= allowed {
            self.lost_responses = self.lost_responses.saturating_add(1);
        } else {
            self.as_capable = false;
            self.rate = None;
            self.delay = None;
            self.last_sample = None;
            self.last_sample_peer = None;
            self.last_sample_index = None;
            self.candidate = None;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PdelaySample {
    pub direction: usize,
    pub link_epoch: u64,
    pub previous: Option<usize>,
    pub request_transfer: usize,
    pub response_transfer: usize,
    pub followup_transfer: usize,
    pub t1: Rat,
    pub t2: Rat,
    pub t3: Rat,
    pub t4: Rat,
    pub rate: Option<Rat>,
    pub delay: Option<Rat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<super::math::PdelayQuantization>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SyncPair {
    pub parent: usize,
    pub source: PortIdentity,
    pub sequence: u16,
    pub transfer: usize,
    pub rx: Rat,
    pub epoch: u64,
    pub deadline_raw: Rat,
    pub deadline_ps: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Relay {
    pub origin: Timestamp,
    pub correction: Rat,
    pub upstream_raw: Rat,
    pub gm_local_ratio: Rat,
    pub epoch: u64,
    pub gm: [u8; 8],
    pub sync_transfer: usize,
    pub followup_transfer: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Node {
    pub parent: Option<usize>,
    pub grandmaster: [u8; 8],
    pub epoch: u64,
    pub synchronized: bool,
    pub sync_pair: Option<SyncPair>,
    pub relay: Option<Relay>,
    pub last_sync_ps: Option<u64>,
    pub sync_deadline_raw: Option<Rat>,
    pub sync_deadline_ps: Option<u64>,
    pub sync_timeout_generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Correction {
    pub node: usize,
    pub at_ps: u64,
    pub target: Rat,
    pub rate: Rat,
    pub sync_transfer: usize,
    pub followup_transfer: usize,
    pub parent: usize,
    pub epoch: u64,
    pub captured_rx: Rat,
    pub delay_neighbor: Rat,
    pub neighbor_rate: Rat,
    pub pdelay_sample: usize,
    pub origin: Timestamp,
    pub correction: Rat,
    pub cumulative_scaled_rate_offset: i32,
    pub clock_generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    pub node: usize,
    pub at_ps: u64,
    pub epoch: u64,
    pub parent: Option<usize>,
    pub grandmaster: [u8; 8],
    pub candidate_transfer: Option<usize>,
    pub candidate_link_epoch: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cause {
    pub at_ps: u64,
    /// The vector index is the stable callback and intra-callback order.
    pub kind: CauseKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum CauseKind {
    Timer(Timer),
    Sent {
        transfer: usize,
    },
    Sof {
        transfer: usize,
    },
    Capture {
        transfer: usize,
        kind: CaptureKind,
    },
    SampledCapture {
        transfer: usize,
        kind: CaptureKind,
        provenance: SampledCapture,
    },
    Eof {
        transfer: usize,
    },
    TxComplete {
        transfer: usize,
    },
    Receive {
        transfer: usize,
    },
    Link {
        direction: usize,
        up: bool,
    },
    PhysicalReadiness {
        attachment: usize,
        ready: bool,
        epoch: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PhysicalReadiness {
    pub ready: bool,
    pub epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::runtime::network) struct State {
    pub(super) ports: Vec<Option<Port>>,
    pub(super) nodes: Vec<Node>,
    pub(super) packets: BTreeMap<usize, Packet>,
    pub(super) next_packet: u64,
    pub(super) journal: Vec<Correction>,
    pub(super) selections: Vec<Selection>,
    pub(super) samples: Vec<PdelaySample>,
    pub(super) causes: Vec<Cause>,
    /// Absent until the first actual bilateral training completion. Initial
    /// queued protocol admissions retain the normal sampled startup behavior.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) physical_links: BTreeMap<usize, PhysicalReadiness>,
}
impl State {
    pub(super) fn pending_exchange_count(&self) -> usize {
        self.ports
            .iter()
            .filter(|p| p.as_ref().is_some_and(|p| p.pending.is_some()))
            .count()
            + self.nodes.iter().filter(|n| n.sync_pair.is_some()).count()
    }
    pub fn new(p: &PreparedGptp) -> Self {
        Self {
            ports: p
                .ports
                .iter()
                .map(|v| {
                    v.as_ref().map(|_| Port {
                        role: Role::Disabled,
                        information_source: InformationSource::Aged,
                        link_up: true,
                        link_epoch: 0,
                        sync_seq: 0,
                        announce_seq: 0,
                        pdelay_seq: 0,
                        pdelay_generation: 0,
                        pdelay_due_raw: None,
                        sync_generation: 0,
                        sync_due_raw: None,
                        announce_generation: 0,
                        announce_due_raw: None,
                        pending: None,
                        last_sample: None,
                        last_sample_peer: None,
                        last_sample_index: None,
                        rate: None,
                        delay: None,
                        as_capable: false,
                        lost_responses: 0,
                        candidate: None,
                        peer: None,
                    })
                })
                .collect(),
            nodes: p
                .nodes
                .iter()
                .map(|n| Node {
                    parent: None,
                    grandmaster: n.clock_identity,
                    epoch: 0,
                    synchronized: n.priority1 != 255,
                    sync_pair: None,
                    relay: None,
                    last_sync_ps: None,
                    sync_deadline_raw: None,
                    sync_deadline_ps: None,
                    sync_timeout_generation: 0,
                })
                .collect(),
            packets: BTreeMap::new(),
            next_packet: 0,
            journal: vec![],
            selections: vec![],
            samples: vec![],
            causes: vec![],
            physical_links: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::runtime::network) enum CaptureKind {
    Tx,
    Rx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(in crate::runtime::network) enum Timer {
    Pdelay { direction: usize, generation: u64 },
    PdelayTimeout { direction: usize, generation: u64 },
    Announce { direction: usize, generation: u64 },
    AnnounceTimeout { direction: usize, generation: u64 },
    Sync { direction: usize, generation: u64 },
    SyncTimeout { node: usize, generation: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::runtime::network) enum Event {
    Timer(Timer),
    Capture {
        transfer: usize,
        packet_id: u64,
        link_epoch: u64,
        kind: CaptureKind,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn figure_11_9_reset_uses_old_counter_and_preserves_ratio_until_third_loss() {
        let prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gptp/two-node.ini"),
        )
        .unwrap();
        let cfg = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .gptp
            .as_ref()
            .unwrap();
        let mut state = State::new(cfg);
        let port = state.ports.iter_mut().flatten().next().unwrap();
        port.as_capable = true;
        port.rate = Some(Rat::ONE);
        port.delay = Some(Rat::new(100, 1).unwrap());
        port.last_sample = Some((Rat::ONE, Rat::ONE));
        for old in [0, 1] {
            assert_eq!(port.lost_responses, old);
            port.reset_after_lost_response(1);
            assert!(port.as_capable);
            assert_eq!(port.rate, Some(Rat::ONE));
            assert_eq!(port.lost_responses, old + 1);
        }
        port.reset_after_lost_response(1);
        assert!(!port.as_capable);
        assert_eq!(port.rate, None);
        assert_eq!(port.delay, None);
        assert_eq!(port.last_sample, None);
        assert_eq!(port.lost_responses, 2);
    }
}
