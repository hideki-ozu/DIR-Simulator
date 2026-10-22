//! Causal sampled Ethernet media with explicit project and named IEEE PHY modes.
use super::{
    channel::*,
    ieee::{mau, plca, repeater, t1_gigabit, t1_link, t1_raw, t1s},
    receiver::*,
    wire, *,
};
use serde::{Deserializer, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
fn is_false(value: &bool) -> bool {
    !*value
}
fn default_mac_start_permission() -> Option<u64> {
    Some(u64::MAX)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    pub id: String,
    pub transmitter: TransmitterConfig,
    pub receiver: ReceiverConfig,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediumConfig {
    pub execution_contract: String,
    pub standard_edition: String,
    #[serde(default, skip_serializing_if = "PhysicalMode::is_project")]
    pub physical_mode: PhysicalMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plca: Option<plca::DomainConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeater: Option<repeater::Config>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gigabit_t1: Option<t1_link::Config>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub gigabit_raw_merge: bool,
    pub channel: ChannelConfig,
    pub endpoints: Vec<EndpointConfig>,
    pub half_duplex: bool,
    pub carrier_threshold_uv: i64,
    pub carrier_hold_samples: u32,
    pub collision_threshold_uv: i64,
    pub ifg_bits: u32,
    pub jam_bits: u32,
    pub slot_bits: u32,
    pub retry_limit: u32,
    pub seed: u64,
    pub maximum_mac_bytes: usize,
    pub maximum_samples: u64,
    pub trace_samples: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalMode {
    #[default]
    ProjectNumerical,
    Ieee10BaseT1sMultidrop2022,
    Ieee10BaseT2022,
    Ieee1000BaseT1PointToPoint2022,
}
impl PhysicalMode {
    fn is_project(&self) -> bool {
        matches!(self, Self::ProjectNumerical)
    }
    pub fn is_named(self) -> bool {
        self != Self::ProjectNumerical
    }
}
impl MediumConfig {
    /// Exact completed-renderer marker propagation is proved only for this
    /// static, noiseless unity path. RX decode cadence remains a separate plane.
    pub fn exact_mdi_timestamp_mapping_supported(&self) -> bool {
        self.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022
            && self.endpoints.len() == 2
            && self.channel.paths.len() == 2
            && self.channel.noise_rms_uv == [0, 0]
            && self
                .endpoints
                .iter()
                .enumerate()
                .all(|(receiver, endpoint)| {
                    let source = 1 - receiver;
                    let peer = &self.endpoints[source];
                    self.channel.paths[receiver].len() == 2
                        && self.channel.paths[receiver][receiver].is_none()
                        && self.channel.paths[receiver][source]
                            .as_ref()
                            .is_some_and(|path| path.taps_q30 == [Q30])
                        && endpoint.transmitter.clock_ppm == 0
                        && endpoint.transmitter.transition_ui_ppm == 0
                        && endpoint.transmitter.filter_taps_q30 == [Q30]
                        && endpoint.receiver.clock_ppm == 0
                        && endpoint.receiver.max_tracking_ppm == 0
                        && endpoint.receiver.frontend_taps_q30 == [Q30]
                        && endpoint.receiver.ffe_taps == 1
                        && endpoint.receiver.echo_taps == 0
                        && endpoint.receiver.adaptation_q30 == 0
                        && peer.transmitter.peak_uv <= self.channel.clip_uv
                        && peer.transmitter.peak_uv <= endpoint.receiver.adc_full_scale_uv
                })
    }
    /// Conservative pre-startup envelope for any subsequently granted exact
    /// raw timing certificate. Both end epochs subtract the same2700 UIs;
    /// fixed one-decision/UI cadence leaves less than one UI of phase rounding.
    pub fn raw_timing_envelope(&self) -> Option<RawTimingEnvelope> {
        if !self.gigabit_raw_merge || !self.exact_mdi_timestamp_mapping_supported() {
            return None;
        }
        let ui = self.channel.grid.samples_per_ui as u64;
        if ui == 0 {
            return None;
        }
        let mut directions = Vec::new();
        for source in 0..2 {
            let receiver = 1 - source;
            let delay =
                u64::try_from(self.channel.paths[receiver][source].as_ref()?.delay_samples).ok()?;
            directions.push(RawDirectionEnvelope {
                source_endpoint: self.endpoints[source].id.clone(),
                receiver_endpoint: self.endpoints[receiver].id.clone(),
                channel_delay_samples: delay,
                pmax_samples_upper_bound: delay.checked_add(ui - 1)?,
                callback_rounding_samples: 0,
            });
        }
        Some(RawTimingEnvelope {
            grid: self.channel.grid,
            directions,
        })
    }
    pub fn validate(&self) -> Result<()> {
        self.channel.validate()?;
        match self.physical_mode {
            PhysicalMode::ProjectNumerical => {
                if self.plca.is_some()
                    || self.repeater.is_some()
                    || self.gigabit_t1.is_some()
                    || self.gigabit_raw_merge
                {
                    return Err("PLCA requires the 10BASE-T1S multidrop physical mode".into());
                }
                if self.execution_contract != wire::CONTRACT
                    || self.standard_edition != "project.numerical.v1"
                {
                    return Err("project numerical PHY contract/edition mismatch".into());
                }
            }
            PhysicalMode::Ieee10BaseT1sMultidrop2022 => {
                if self.execution_contract != t1s::CONTRACT
                    || self.standard_edition != "IEEE Std 802.3-2022"
                    || !self.half_duplex
                    || self.channel.endpoints.len() < 3
                    || self.channel.grid.symbol_rate_hz != 25_000_000
                    || ![16, 32].contains(&self.channel.grid.samples_per_ui)
                    || self.jam_bits != 32
                    || self.slot_bits != 512
                    || self.retry_limit != 16
                    || self.ifg_bits < 96
                    || self.repeater.is_some()
                    || self.gigabit_t1.is_some()
                    || self.gigabit_raw_merge
                {
                    return Err("10BASE-T1S multidrop requires Clause 147 chip/sample grid, at least three half-duplex endpoints, and Clause 4 MAC timings".into());
                }
                if let Some(plca) = &self.plca {
                    plca.validate(&self.channel.endpoints)?;
                }
            }
            PhysicalMode::Ieee10BaseT2022 => {
                if !["ieee8023.10base_t.half.2022", "ieee8023.10base_t.2022"]
                    .contains(&self.execution_contract.as_str())
                    || self.standard_edition != "IEEE Std 802.3-2022"
                    || self.plca.is_some()
                    || self.gigabit_t1.is_some()
                    || self.gigabit_raw_merge
                    || !self.half_duplex
                    || self.channel.endpoints.len() != if self.repeater.is_some() { 6 } else { 2 }
                    || self.channel.grid.symbol_rate_hz != 20_000_000
                    || ![16, 32].contains(&self.channel.grid.samples_per_ui)
                    || self.jam_bits != 32
                    || self.slot_bits != 512
                    || self.retry_limit != 16
                    || self.ifg_bits < 96
                {
                    return Err("10BASE-T point-to-point requires Clause 14 Manchester chip grid and Clause 4 MAC timings".into());
                }
                if let Some(repeater) = &self.repeater {
                    repeater.validate()?;
                    let endpoints: BTreeSet<_> = self.channel.endpoints.iter().collect();
                    let declared: BTreeSet<_> = repeater
                        .pairs
                        .iter()
                        .flat_map(|p| [&p.mac_endpoint, &p.mau_endpoint])
                        .collect();
                    if declared != endpoints {
                        return Err(
                            "10BASE-T repeater pairs do not cover exactly six endpoints".into()
                        );
                    }
                    let paired = |a: &str, b: &str| {
                        repeater.pairs.iter().any(|p| {
                            (p.mac_endpoint == a && p.mau_endpoint == b)
                                || (p.mac_endpoint == b && p.mau_endpoint == a)
                        })
                    };
                    for (i, row) in self.channel.paths.iter().enumerate() {
                        for (j, path) in row.iter().enumerate() {
                            if path.is_some()
                                && !paired(&self.channel.endpoints[i], &self.channel.endpoints[j])
                            {
                                return Err("10BASE-T repeater requires three disjoint MAC/MAU links without bypass or echo".into());
                            }
                        }
                    }
                    let link_roundtrips = repeater
                        .pairs
                        .iter()
                        .map(|pair| {
                            let mac = self
                                .channel
                                .endpoints
                                .iter()
                                .position(|id| id == &pair.mac_endpoint)
                                .unwrap();
                            let mau = self
                                .channel
                                .endpoints
                                .iter()
                                .position(|id| id == &pair.mau_endpoint)
                                .unwrap();
                            [&self.channel.paths[mac][mau], &self.channel.paths[mau][mac]]
                                .into_iter()
                                .flatten()
                                .map(|path| {
                                    path.delay_samples + path.taps_q30.len().saturating_sub(1)
                                })
                                .sum::<usize>()
                        })
                        .collect::<Vec<_>>();
                    let mut worst = link_roundtrips;
                    worst.sort_unstable_by(|a, b| b.cmp(a));
                    let repeater_roundtrip =
                        2 * (repeater.retime_chips + 1) * self.channel.grid.samples_per_ui as usize;
                    if worst[0] + worst[1] + repeater_roundtrip
                        >= self.slot_bits as usize * self.mac_bit_samples() as usize
                    {
                        return Err("10BASE-T repeater star exceeds selected Clause 4 slot-time path budget".into());
                    }
                }
            }
            PhysicalMode::Ieee1000BaseT1PointToPoint2022 => {
                if self.execution_contract != t1_link::CONTRACT
                    || self.standard_edition != "IEEE Std 802.3-2022"
                    || self.plca.is_some()
                    || self.repeater.is_some()
                    || self.half_duplex
                    || self.channel.endpoints.len() != 2
                    || self.channel.grid.symbol_rate_hz != 750_000_000
                    || ![16, 32].contains(&self.channel.grid.samples_per_ui)
                    || self.ifg_bits < 96
                {
                    return Err("1000BASE-T1 requires two full-duplex endpoints on a 750 MBd Clause 97 sample grid".into());
                }
                self.gigabit_t1
                    .as_ref()
                    .ok_or("1000BASE-T1 requires station role/AN configuration")?
                    .validate(&self.channel.endpoints)?;
                if self.channel.paths[0][1].is_none()
                    || self.channel.paths[1][0].is_none()
                    || self
                        .channel
                        .paths
                        .iter()
                        .enumerate()
                        .any(|(i, row)| row[i].is_some())
                {
                    return Err("bounded 1000BASE-T1 ideal echo reference requires both partner paths and no diagonal echo path".into());
                }
            }
        }
        if self.endpoints.len() != self.channel.endpoints.len()
            || self
                .endpoints
                .iter()
                .zip(&self.channel.endpoints)
                .any(|(e, id)| &e.id != id || e.id.len() > 128)
            || self.carrier_threshold_uv <= 0
            || self.collision_threshold_uv <= 0
            || self.carrier_threshold_uv > self.channel.clip_uv
            || self.collision_threshold_uv > self.channel.clip_uv
            || self.carrier_hold_samples < self.channel.grid.samples_per_ui * 2
            || self.carrier_hold_samples > self.channel.grid.samples_per_ui * 64
            || self.ifg_bits > 4096
            || self.jam_bits == 0
            || self.jam_bits > 256
            || self.jam_bits % 8 != 0
            || self.slot_bits == 0
            || self.slot_bits > 65536
            || self.retry_limit > 32
            || !(64..=16384).contains(&self.maximum_mac_bytes)
            || self.maximum_samples == 0
            || self.maximum_samples
                > if self.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
                    3_000_000_000
                } else {
                    100_000_000
                }
        {
            return Err(
                "invalid sampled-medium limits, identities, thresholds or MAC policy".into(),
            );
        }
        for endpoint in &self.endpoints {
            Transmitter::new(endpoint.transmitter.clone())?;
            endpoint.receiver.validate()?;
            if endpoint.transmitter.grid != self.channel.grid
                || endpoint.receiver.grid != self.channel.grid
                || endpoint.receiver.minimum_training_symbols
                    > match self.physical_mode {
                        PhysicalMode::ProjectNumerical => (wire::PILOT_BYTES * 16) as u32,
                        PhysicalMode::Ieee10BaseT1sMultidrop2022 => 40,
                        PhysicalMode::Ieee10BaseT2022 => 64,
                        PhysicalMode::Ieee1000BaseT1PointToPoint2022 => 64,
                    }
                || self.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022
                    && (endpoint.transmitter.transition_ui_ppm != 0
                        || endpoint.transmitter.filter_taps_q30 != [Q30]
                        || endpoint.receiver.echo_taps != 0)
            {
                return Err("PHY endpoint grids/pilot training bounds differ".into());
            }
        }
        if self
            .channel
            .paths
            .iter()
            .flatten()
            .flatten()
            .any(|p| p.delay_samples + p.taps_q30.len() > 4096)
        {
            return Err("production sampled-medium propagation history exceeds4096 samples".into());
        }
        if !self.half_duplex
            && self
                .channel
                .paths
                .iter()
                .enumerate()
                .any(|(i, row)| row[i].is_some())
        {
            return Err(
                "independent-direction project PHY requires explicit zero diagonal coupling".into(),
            );
        }
        Ok(())
    }
    fn ifg_samples(&self) -> u64 {
        self.ifg_bits as u64 * self.mac_bit_samples()
    }
    fn mac_bit_samples(&self) -> u64 {
        let ui = self.channel.grid.samples_per_ui as u64;
        match self.physical_mode {
            // 10BASE-T1S has an 80 ns PCS bit but a 100 ns MAC bit after
            // 4B/5B expansion. Clause 4 IPG, slot and BEB use MAC bit time.
            PhysicalMode::Ieee10BaseT1sMultidrop2022 => 5 * ui / 2,
            PhysicalMode::ProjectNumerical | PhysicalMode::Ieee10BaseT2022 => 2 * ui,
            PhysicalMode::Ieee1000BaseT1PointToPoint2022 => 3 * ui / 4,
        }
    }
    fn history_len(&self, source: usize) -> usize {
        self.channel
            .paths
            .iter()
            .filter_map(|row| row[source].as_ref())
            .map(|p| p.delay_samples + p.taps_q30.len())
            .max()
            .unwrap_or(1)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacTx {
    pub frame_id: String,
    pub source_endpoint: String,
    pub at_ps: u64,
    /// Canonical MAC bytes include FCS, exclude preamble/SFD/IFG.
    pub mac_bytes: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMPacketTx {
    /// Attachment-local unrendered sample; None retains exact integer admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_sample: Option<u64>,
    pub packet_id: u64,
    pub generation: u64,
    pub source_endpoint: String,
    pub at_ps: u64,
    /// Existing Merge xMII bytes, including its own preamble, SMD and CRC.
    pub xmii_bytes: Vec<u8>,
    pub origin_frame_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDirectionEnvelope {
    pub source_endpoint: String,
    pub receiver_endpoint: String,
    pub channel_delay_samples: u64,
    pub pmax_samples_upper_bound: u64,
    pub callback_rounding_samples: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTimingEnvelope {
    pub grid: SampleGrid,
    pub directions: Vec<RawDirectionEnvelope>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDirectionTiming {
    pub source_endpoint: String,
    pub receiver_endpoint: String,
    pub source_startup_sample: u64,
    pub receiver_startup_sample: u64,
    pub source_data_epoch_sample: u64,
    pub reconstructed_rx_epoch_sample: u64,
    pub source_initial_idle_end_sample: u64,
    pub received_initial_idle_end_sample: u64,
    pub source_switch_pfc24: u32,
    pub pmax_samples: u64,
    pub callback_rounding_samples: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTimingCertificate {
    pub medium_config_sha256: String,
    pub attachment_config_sha256: Option<String>,
    pub attachment_origin_ps: u64,
    pub endpoints: Vec<String>,
    pub physical_epoch_start_samples: Vec<u64>,
    pub grid: SampleGrid,
    pub bilateral_ready: bool,
    pub initial_idle_verified: bool,
    pub directions: Vec<RawDirectionTiming>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawIngressStatus {
    pub ready: bool,
    pub next_cut_sample: u64,
    pub consumed_gmii_slots: u64,
    pub active_packet_id: Option<u64>,
    pub generation: Option<u64>,
}
#[derive(Clone, Debug)]
struct SharedTx(Arc<MacTx>);
impl Serialize for SharedTx {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}
impl<'de> Deserialize<'de> for SharedTx {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self(Arc::new(MacTx::deserialize(d)?)))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub frame_id: String,
    pub source_endpoint: String,
    pub attempt: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum PhyEffect {
    XmiStarted {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        sample_index: u64,
    },
    XmiDelimiter {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        octet_index: usize,
        value: u8,
        sample_index: u64,
    },
    /// Actual delimiter byte accepted by xMII after any same-cut suffix edit.
    XmiDelimiterConsumed {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        octet_index: usize,
        value: u8,
        consumed_cut_sample: u64,
        sample_index: u64,
    },
    XmiEnd {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        sample_index: u64,
    },
    XmiReleased {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        sample_index: u64,
    },
    XmiAborted {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        reason: String,
        sample_index: u64,
    },
    /// Actual mapped delimiter end, observed only after rendering its final UI.
    MdiDelimiterEnd {
        source_endpoint: String,
        packet_id: u64,
        generation: u64,
        delimiter_slot: u64,
        sample_index: u64,
    },
    MdiCodedEnd {
        endpoint: String,
        packet_id: u64,
        generation: u64,
        sample_index: u64,
    },
    RawMPacketIngress {
        receiver_endpoint: String,
        packet_id: u64,
        /// Generation of the append-only prefix at delimiter consumption.
        generation: u64,
        final_generation: u64,
        /// Exact mapped reference; absent outside the proved fixed cadence.
        delimiter_sample: Option<u64>,
        /// Reconstructed recovered cadence, distinct from exact channel mapping.
        reconstructed_delimiter_sample: Option<u64>,
        xmii_bytes: Vec<u8>,
        origins: Vec<Origin>,
        receive_cut_sample: u64,
        decoded_at_sample: u64,
        sample_index: u64,
    },
    RawPacketRejected {
        receiver_endpoint: String,
        packet_ids: Vec<u64>,
        reason: String,
        sample_index: u64,
    },
    TxStarted {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    TxMacPreamble {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    TxMacSfd {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    TxMacFcsEnd {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    TxCodedEnd {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    RxMacSfd {
        endpoint: String,
        sample_index: u64,
    },
    PcsControl {
        endpoint: String,
        command: String,
        sample_index: u64,
    },
    Repeater {
        event: repeater::Event,
        sample_index: u64,
    },
    TxReady {
        endpoint: String,
        ready: bool,
        sample_index: u64,
    },
    TxFinished {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    Retry {
        endpoint: String,
        origin: Origin,
        slots: u64,
        sample_index: u64,
    },
    RetryExhausted {
        endpoint: String,
        origin: Origin,
        sample_index: u64,
    },
    CarrierChanged {
        endpoint: String,
        present: bool,
        sample_index: u64,
    },
    Collision {
        endpoint: String,
        origin: Origin,
        late: bool,
        residual_uv: i64,
        sample_index: u64,
    },
    Receiver {
        endpoint: String,
        event: ReceiverEvent,
    },
    RxStarted {
        endpoint: String,
        token: FrameToken,
        sample_index: u64,
    },
    RxFrame {
        receiver_endpoint: String,
        mac_bytes: Vec<u8>,
        origins: Vec<Origin>,
        rx_sfd_sample: u64,
        rx_fcs_sample: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rx_mapped_sfd_sample: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rx_mapped_fcs_sample: Option<u64>,
        sample_index: u64,
    },
    RxAbort {
        endpoint: String,
        reason: String,
        sample_index: u64,
    },
    Samples {
        sample_index: u64,
        transmitted_uv: Vec<i64>,
        received_uv: Vec<i64>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    request: SharedTx,
    origin: Origin,
    next_chip: usize,
    jam: bool,
    started: u64,
    #[serde(default)]
    sfd_reported: bool,
    #[serde(default)]
    fcs_reported: bool,
    #[serde(default)]
    t1s_group: usize,
    #[serde(default)]
    t1s_chips: VecDeque<i8>,
    #[serde(default)]
    burst_keepalive: bool,
    #[serde(default)]
    late_collision: bool,
    /// Actual MAC input stream endpoint, independently of the delayed line tail.
    #[serde(default)]
    mac_input_end_sample: Option<u64>,
    /// Actual MAC JAM suppressed by Clause148 COLLIDE, counted in samples.
    /// None retains the ordinary physical Program behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mac_jam_samples_left: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GigabitProgram {
    request: SharedTx,
    origin: Origin,
    range: t1_link::SourceRange,
    start_sample: u64,
    sfd_reported: bool,
    fcs_reported: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalMdiSpan {
    origin: Origin,
    sfd_symbol: u64,
    fcs_symbol: u64,
    sfd_sample: u64,
    fcs_sample: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawActive {
    request: RawMPacketTx,
    origin: Origin,
    consumed_octets: usize,
    start_slot: Option<u64>,
    start_sample: u64,
    end_slot: Option<u64>,
    end_at_sample: Option<u64>,
    release_at_sample: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpan {
    packet_id: u64,
    generation: u64,
    origin: Origin,
    start_slot: u64,
    end_slot: u64,
    coded_end_symbol: u64,
    coded_reported: bool,
    /// (generation, consumed prefix length) at each permitted suffix change.
    generation_lineage: Vec<(u64, usize)>,
    delimiter_slot: Option<u64>,
    delimiter_generation: Option<u64>,
    delimiter_end_symbol: Option<u64>,
    delimiter_sample: Option<u64>,
}
impl Program {
    fn len(&self, c: &MediumConfig) -> usize {
        if c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022 {
            // Each Clause 147 5B group contributes ten DME half-bit chips,
            // followed by the mandatory encoded zero and high impedance.
            return (if self.jam {
                8 + 2
            } else {
                18 + self.request.0.mac_bytes.len() * 2
            }) * 10
                + if self.burst_keepalive && !self.jam {
                    0
                } else {
                    3
                };
        }
        if self.jam {
            c.jam_bits as usize * 2
        } else {
            ((if c.physical_mode == PhysicalMode::Ieee10BaseT2022 {
                0
            } else {
                wire::PILOT_BYTES
            }) + 8
                + self.request.0.mac_bytes.len())
                * 16
        }
    }
    fn chip(&self, index: usize, c: &MediumConfig) -> i8 {
        let pilot = if c.physical_mode == PhysicalMode::Ieee10BaseT2022 {
            0
        } else {
            wire::PILOT_BYTES
        };
        let byte = if self.jam {
            0xaa
        } else if index / 16 < pilot {
            0x55
        } else if index / 16 < pilot + 8 {
            if index / 16 == pilot + 7 { 0xd5 } else { 0x55 }
        } else {
            self.request.0.mac_bytes[index / 16 - pilot - 8]
        };
        let first = if byte & (1 << ((index % 16) / 2)) != 0 {
            1
        } else {
            -1
        };
        if index % 2 == 0 { first } else { -first }
    }
    fn next_t1s_chip(
        &mut self,
        scrambler: &mut t1s::Scrambler,
        dme: &mut t1s::DmeTransmitter,
    ) -> Result<i8> {
        if self.t1s_chips.is_empty() {
            let count = if self.jam {
                10
            } else {
                18 + self.request.0.mac_bytes.len() * 2
            };
            let group = if self.t1s_group == count {
                t1s::SILENCE
            } else if self.jam {
                match self.t1s_group.cmp(&8) {
                    std::cmp::Ordering::Less => {
                        t1s::DATA_GROUPS[scrambler.scramble_nibble(0xa)? as usize]
                    }
                    std::cmp::Ordering::Equal => t1s::ESD_HEARTBEAT,
                    std::cmp::Ordering::Greater => t1s::ESD_ERROR,
                }
            } else if self.t1s_group < 2 {
                t1s::SYNC_COMMIT
            } else if self.t1s_group < 4 {
                t1s::SSD
            } else if self.t1s_group < 16 {
                let nibble = self.t1s_group;
                let byte = if nibble / 2 == 7 { 0xd5 } else { 0x55 };
                t1s::DATA_GROUPS
                    [scrambler.scramble_nibble((byte >> (4 * (nibble % 2))) & 15)? as usize]
            } else if self.t1s_group < count - 2 {
                let nibble = self.t1s_group - 16;
                let byte = self.request.0.mac_bytes[nibble / 2];
                t1s::DATA_GROUPS
                    [scrambler.scramble_nibble((byte >> (4 * (nibble % 2))) & 15)? as usize]
            } else if self.t1s_group == count - 2 {
                t1s::ESD_HEARTBEAT
            } else {
                t1s::ESD_OK_BURST
            };
            self.t1s_group += 1;
            self.t1s_chips.extend(dme.group(group)?);
        }
        self.next_chip += 1;
        self.t1s_chips
            .pop_front()
            .ok_or("empty DME group chip buffer".into())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Port {
    tx: Transmitter,
    rx: Receiver,
    queue: VecDeque<SharedTx>,
    #[serde(default = "default_mac_start_permission")]
    mac_start_permission: Option<u64>,
    program: Option<Program>,
    attempt: u32,
    carrier: bool,
    below: u32,
    idle_samples: u64,
    backoff_samples: u64,
    decoder: wire::Decoder,
    token: Option<FrameToken>,
    origins: BTreeSet<Origin>,
    #[serde(default)]
    t1s_scrambler: Option<t1s::Scrambler>,
    #[serde(default)]
    t1s_dme_tx: Option<t1s::DmeTransmitter>,
    #[serde(default)]
    t1s_dme_rx: Option<t1s::DmeReceiver>,
    #[serde(default)]
    t1s_pcs_rx: Option<t1s::PcsReceiver>,
    #[serde(default)]
    t1s_last_chip: Option<i8>,
    #[serde(default)]
    rx_sfd_sample: Option<u64>,
    #[serde(default)]
    rx_fcs_sample: Option<u64>,
    #[serde(default)]
    plca_node: Option<plca::Node>,
    #[serde(default)]
    plca_observation: plca::Observation,
    #[serde(default)]
    plca_control_chips: VecDeque<i8>,
    #[serde(default)]
    plca_beacon_in_flight: bool,
    #[serde(default)]
    plca_beacon_waveform_done: bool,
    #[serde(default)]
    plca_received_beacon: bool,
    #[serde(default)]
    plca_commit_active: bool,
    #[serde(default)]
    repeater_chips: VecDeque<repeater::OutputChip>,
    #[serde(default)]
    repeater_current_origins: BTreeSet<Origin>,
    #[serde(default)]
    repeater_recovered_chip: Option<i8>,
    #[serde(default)]
    repeater_recovered_origins: BTreeSet<Origin>,
    #[serde(default)]
    mau: Option<mau::Mau>,
    #[serde(default)]
    mau_raw_active: bool,
    #[serde(default)]
    gigabit_link: Option<t1_link::Port>,
    #[serde(default)]
    gigabit_program: Option<GigabitProgram>,
    #[serde(default)]
    gigabit_rx_data_start: Option<u64>,
    #[serde(default)]
    /// Actual channel-observed sample interval for each physical source attempt.
    #[serde(
        serialize_with = "serialize_observed_origins",
        deserialize_with = "deserialize_observed_origins"
    )]
    gigabit_rx_origins: BTreeMap<Origin, (u64, u64)>,
    gigabit_mdi_spans: VecDeque<CanonicalMdiSpan>,
    gigabit_raw_tx_initial_end_sample: Option<u64>,
    gigabit_raw_rx_initial_end_sample: Option<u64>,
    #[serde(default)]
    raw_active: Option<RawActive>,
    #[serde(default)]
    raw_spans: VecDeque<RawSpan>,
}
#[derive(Clone, Debug)]
pub struct SampledMedium {
    config: Arc<MediumConfig>,
    state: State,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    index: u64,
    channel: MatrixChannel,
    ports: Vec<Port>,
    seen: BTreeSet<String>,
    #[serde(default)]
    raw_seen: BTreeSet<(String, u64)>,
    terminal: BTreeSet<String>,
    raw_history: Vec<VecDeque<i64>>,
    #[serde(deserialize_with = "deserialize_origin_history")]
    origin_history: Vec<VecDeque<BTreeSet<Origin>>>,
    #[serde(default)]
    repeater: Option<repeater::Repeater>,
    #[serde(default)]
    gigabit_mdi_disconnected: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    config: MediumConfig,
    state: State,
}
fn serialize_observed_origins<S: Serializer>(
    origins: &BTreeMap<Origin, (u64, u64)>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    origins.iter().collect::<Vec<_>>().serialize(serializer)
}
fn deserialize_observed_origins<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<Origin, (u64, u64)>, D::Error> {
    let entries = Vec::<(Origin, (u64, u64))>::deserialize(deserializer)?;
    if entries.len() > 4096 || entries.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(serde::de::Error::custom(
            "unordered, duplicate or unbounded observed origin spans",
        ));
    }
    Ok(entries.into_iter().collect())
}
fn deserialize_origin_history<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<VecDeque<BTreeSet<Origin>>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Set(BTreeSet<Origin>),
        Legacy(Option<Origin>),
    }
    let old_or_new = Vec::<VecDeque<Entry>>::deserialize(deserializer)?;
    Ok(old_or_new
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|entry| match entry {
                    Entry::Set(set) => set,
                    Entry::Legacy(Some(origin)) => BTreeSet::from([origin]),
                    Entry::Legacy(None) => BTreeSet::new(),
                })
                .collect()
        })
        .collect())
}
impl SampledMedium {
    fn mac_permission_at(config: &MediumConfig, port: &Port, sample: u64) -> bool {
        match port.mac_start_permission {
            None => false,
            Some(u64::MAX) => true,
            Some(until_ps) => config
                .channel
                .grid
                .time(sample)
                .is_ok_and(|at| at.numerator_ps <= until_ps as u128 * at.denominator),
        }
    }
    pub fn set_mac_start_permission(
        &mut self,
        endpoint: &str,
        allowed_until_ps: Option<u64>,
    ) -> Result<()> {
        let index = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == endpoint)
            .ok_or("unknown PHY MAC permission endpoint")?;
        self.state.ports[index].mac_start_permission = allowed_until_ps;
        Ok(())
    }
    /// Clause148 management enable at the next physical sample. This changes
    /// neither the immutable station identity nor other nodes' local status.
    pub fn set_plca_enabled(&mut self, endpoint: &str, enabled: bool) -> Result<()> {
        let index = self
            .config
            .endpoints
            .iter()
            .position(|port| port.id == endpoint)
            .ok_or("unknown PLCA endpoint")?;
        let port = &mut self.state.ports[index];
        let node = port
            .plca_node
            .as_mut()
            .ok_or("PLCA is not configured on endpoint")?;
        let restart_guard = enabled && node.control == plca::ControlState::Disable;
        node.set_enabled(enabled);
        if restart_guard {
            node.timer_samples = plca::MiiClock::new(
                port.tx.next_index(),
                self.config.mac_bit_samples(),
                self.config.endpoints[index].transmitter.clock_ppm,
            )
            .bit_samples(node.node_count as u64 * node.to_timer_bits as u64 + plca::BEACON_BITS);
        }
        if !enabled {
            port.plca_commit_active = false;
            port.plca_control_chips.clear();
            if port.plca_beacon_in_flight {
                port.plca_beacon_in_flight = false;
                port.plca_beacon_waveform_done = false;
                port.tx.replace_pending_symbols(&[])?;
            }
        }
        Ok(())
    }
    pub fn mac_start_permission(&self, endpoint: &str) -> Result<Option<u64>> {
        let index = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == endpoint)
            .ok_or("unknown PHY MAC permission endpoint")?;
        Ok(self.state.ports[index].mac_start_permission)
    }
    /// Bilateral 1000BASE-T1 MDI outage at the next sample boundary. This
    /// bounded fault input cuts the received waveform, while the transmitters
    /// and channel continue to advance for deterministic recovery evidence.
    pub fn set_gigabit_mdi_disconnected(&mut self, disconnected: bool) -> Result<()> {
        if self.config.physical_mode != PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
            return Err("MDI disconnect fault requires named 1000BASE-T1".into());
        }
        self.state.gigabit_mdi_disconnected = disconnected;
        Ok(())
    }
    pub fn new(config: MediumConfig) -> Result<Self> {
        config.validate()?;
        let ports = config
            .endpoints
            .iter()
            .enumerate()
            .map(|(index, e)| {
                let t1s = config.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022;
                let seed = ((config.seed.rotate_left(index as u32) as u32) & 0x1ffff) | 1;
                let plca_node = config
                    .plca
                    .as_ref()
                    .map(|domain| {
                        let mut node = plca::Node::new(
                            domain.stations[index].clone(),
                            domain,
                            config.mac_bit_samples(),
                        )?;
                        node.timer_samples = plca::MiiClock::new(
                            0,
                            config.mac_bit_samples(),
                            e.transmitter.clock_ppm,
                        )
                        .bit_samples(
                            domain.node_count as u64 * domain.to_timer_bits as u64
                                + plca::BEACON_BITS,
                        );
                        Ok::<_, String>(node)
                    })
                    .transpose()?;
                Ok(Port {
                    tx: Transmitter::new(e.transmitter.clone())?,
                    rx: Receiver::new(e.receiver.clone())?,
                    queue: VecDeque::new(),
                    mac_start_permission: Some(u64::MAX),
                    program: None,
                    attempt: 0,
                    carrier: false,
                    below: 0,
                    idle_samples: config.ifg_samples(),
                    backoff_samples: 0,
                    decoder: wire::Decoder::new(config.maximum_mac_bytes),
                    token: None,
                    origins: BTreeSet::new(),
                    t1s_scrambler: t1s
                        .then(|| t1s::Scrambler::new(seed).expect("nonzero 17-bit seed")),
                    t1s_dme_tx: t1s.then(t1s::DmeTransmitter::default),
                    t1s_dme_rx: t1s.then(t1s::DmeReceiver::default),
                    t1s_pcs_rx: t1s.then(t1s::PcsReceiver::new),
                    t1s_last_chip: None,
                    rx_sfd_sample: None,
                    rx_fcs_sample: None,
                    plca_node,
                    plca_observation: plca::Observation::default(),
                    plca_control_chips: VecDeque::new(),
                    plca_beacon_in_flight: false,
                    plca_beacon_waveform_done: false,
                    plca_received_beacon: false,
                    plca_commit_active: false,
                    repeater_chips: VecDeque::new(),
                    repeater_current_origins: BTreeSet::new(),
                    repeater_recovered_chip: None,
                    repeater_recovered_origins: BTreeSet::new(),
                    mau: (config.physical_mode == PhysicalMode::Ieee10BaseT2022)
                        .then(|| mau::Mau::new(config.channel.grid.samples_per_ui)),
                    mau_raw_active: false,
                    gigabit_link: config
                        .gigabit_t1
                        .as_ref()
                        .map(|gigabit| {
                            let mut link = t1_link::Port::new(
                                gigabit.stations[index].clone(),
                                config.channel.grid.samples_per_ui,
                                config.maximum_mac_bytes,
                            )?;
                            if config.gigabit_raw_merge {
                                link.enable_raw_merge()?;
                            }
                            Ok::<t1_link::Port, String>(link)
                        })
                        .transpose()?,
                    gigabit_program: None,
                    gigabit_rx_data_start: None,
                    gigabit_rx_origins: BTreeMap::new(),
                    gigabit_mdi_spans: VecDeque::new(),
                    gigabit_raw_tx_initial_end_sample: None,
                    gigabit_raw_rx_initial_end_sample: None,
                    raw_active: None,
                    raw_spans: VecDeque::new(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let n = ports.len();
        let state = State {
            index: 0,
            channel: MatrixChannel::new(config.channel.clone())?,
            ports,
            seen: BTreeSet::new(),
            raw_seen: BTreeSet::new(),
            terminal: BTreeSet::new(),
            raw_history: vec![VecDeque::new(); n],
            origin_history: vec![VecDeque::new(); n],
            repeater: config
                .repeater
                .clone()
                .map(repeater::Repeater::new)
                .transpose()?,
            gigabit_mdi_disconnected: false,
        };
        Ok(Self {
            config: Arc::new(config),
            state,
        })
    }
    pub fn config(&self) -> &MediumConfig {
        &self.config
    }
    pub fn admitted_frame_ids(&self) -> &BTreeSet<String> {
        &self.state.seen
    }
    pub fn pending_admissions(&self) -> impl Iterator<Item = &MacTx> {
        self.state
            .ports
            .iter()
            .flat_map(|p| p.queue.iter().map(|r| r.0.as_ref()))
    }
    pub fn tx_ready(&self, endpoint: &str) -> Result<bool> {
        let i = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == endpoint)
            .ok_or("unknown PHY ready port")?;
        if self
            .config
            .repeater
            .as_ref()
            .is_some_and(|r| r.pairs.iter().any(|p| p.mau_endpoint == endpoint))
        {
            return Ok(false);
        }
        Ok(self.port_ready(&self.state.ports[i]))
    }
    /// Physical link state, independently of a retained MAC transmission.
    pub fn gigabit_bilateral_link_ready(&self) -> Result<bool> {
        if self.config.physical_mode != PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
            return Err("bilateral gigabit readiness requires 1000BASE-T1".into());
        }
        Ok(!self.state.gigabit_mdi_disconnected
            && self.state.ports.len() == 2
            && self
                .state
                .ports
                .iter()
                .all(|port| port.gigabit_link.as_ref().is_some_and(t1_link::Port::ready)))
    }
    fn port_ready(&self, p: &Port) -> bool {
        if self.config.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
            return p.gigabit_link.as_ref().is_some_and(t1_link::Port::ready)
                && p.gigabit_program.is_none();
        }
        p.program.is_none()
            && p.mau.as_ref().is_none_or(|m| m.link_ok() && !m.jabber())
            && !p.plca_beacon_in_flight
            && p.plca_control_chips.is_empty()
            && p.plca_node
                .as_ref()
                .is_none_or(|node| !node.mac_carrier_held())
            && (!self.config.half_duplex || !p.carrier || p.plca_commit_active)
            && p.idle_samples >= self.config.ifg_samples()
            && p.backoff_samples == 0
    }
    pub fn next_sample_index(&self) -> u64 {
        self.state.index
    }
    pub fn next_event_ps(&self) -> Result<u64> {
        let time = self.config.channel.grid.time(
            self.state
                .index
                .checked_add(1)
                .ok_or("sample index overflow")?,
        )?;
        u64::try_from(time.numerator_ps.div_ceil(time.denominator))
            .map_err(|_| "sample time overflow".into())
    }
    pub fn admit(&mut self, request: MacTx) -> Result<()> {
        if self.config.gigabit_raw_merge {
            return Err("raw Merge PHY requires RawMPacketTx, not canonical MacTx".into());
        }
        wire::validate_mac(&request.mac_bytes, self.config.maximum_mac_bytes)?;
        let endpoint = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == request.source_endpoint)
            .ok_or("unknown PHY source endpoint")?;
        if self.state.ports[endpoint]
            .gigabit_link
            .as_ref()
            .is_some_and(|link| link.phase() == t1_link::Phase::Fault)
        {
            return Err("terminal 1000BASE-T1 negotiation rejection; no MAC admission".into());
        }
        if self.config.repeater.as_ref().is_some_and(|r| {
            r.pairs
                .iter()
                .any(|p| p.mau_endpoint == request.source_endpoint)
        }) {
            return Err("repeater MAU has no MAC admission service".into());
        }
        let now = self.config.channel.grid.time(self.state.index)?;
        if request.frame_id.is_empty()
            || request.frame_id.len() > 128
            || self.state.seen.len() >= 4096
            || self.state.seen.contains(&request.frame_id)
            || (request.at_ps as u128) * now.denominator < now.numerator_ps
            || self.state.ports[endpoint]
                .queue
                .back()
                .is_some_and(|r| r.0.at_ps > request.at_ps)
        {
            return Err("invalid/duplicate/bounded/past PHY admission".into());
        }
        self.state.seen.insert(request.frame_id.clone());
        self.state.ports[endpoint]
            .queue
            .push_back(SharedTx(Arc::new(request)));
        Ok(())
    }
    /// Source-certified fixed raw coding/decoding cadence. This is withdrawn
    /// immediately on physical disconnection or either endpoint leaving Data.
    /// The first verified RX frame is tied to the paired sender's training
    /// switch/seed and completed initial-idle renderer boundary. Later equal
    /// idle payloads cannot create this evidence.
    pub fn raw_timing_certificate(&self) -> Result<Option<RawTimingCertificate>> {
        let c = &self.config;
        let s = &self.state;
        if !c.gigabit_raw_merge
            || !c.exact_mdi_timestamp_mapping_supported()
            || s.gigabit_mdi_disconnected
            || s.ports
                .iter()
                .any(|port| !port.gigabit_link.as_ref().is_some_and(t1_link::Port::ready))
        {
            return Ok(None);
        }
        let ui = c.channel.grid.samples_per_ui as u64;
        let mut directions = Vec::new();
        for source in 0..2 {
            let receiver = 1 - source;
            let tx = s.ports[source].gigabit_link.as_ref().unwrap();
            let rx = s.ports[receiver].gigabit_link.as_ref().unwrap();
            if !rx.raw_initial_idle_verified() {
                return Ok(None);
            }
            let Some(switch) = tx.raw_source_switch_pfc24() else {
                return Ok(None);
            };
            if rx.raw_received_switch_pfc24() != Some(switch)
                || rx.raw_received_data_seed() != Some(tx.raw_source_data_seed())
            {
                return Ok(None);
            }
            let Some(source_end) = s.ports[source].gigabit_raw_tx_initial_end_sample else {
                return Ok(None);
            };
            let Some(received_end) = s.ports[receiver].gigabit_raw_rx_initial_end_sample else {
                return Ok(None);
            };
            let Some(source_epoch) = source_end.checked_sub(2700 * ui) else {
                return Ok(None);
            };
            let Some(receive_epoch) = s.ports[receiver].gigabit_rx_data_start else {
                return Ok(None);
            };
            if receive_epoch.checked_add(2700 * ui) != Some(received_end) {
                return Ok(None);
            }
            let symbols = rx.raw_rx_symbols().unwrap_or(0);
            let expected_slots = (symbols / 2700)
                .saturating_sub(1)
                .checked_mul(450)
                .ok_or("raw timing slot count overflow")?;
            if rx.raw_rx_slots() != Some(expected_slots) {
                return Ok(None);
            }
            // A fixed zero-offset receiver releases one decision per UI. The
            // initial source frame must lie in that first physical UI window;
            // an omitted or later idle codeword is2700 UI away and fails.
            let delay = c.channel.paths[receiver][source]
                .as_ref()
                .unwrap()
                .delay_samples as u64;
            let expected_end = source_end
                .checked_add(delay)
                .ok_or("raw timing source alignment overflow")?;
            if received_end.abs_diff(expected_end) >= ui {
                return Ok(None);
            }
            let declared = c
                .raw_timing_envelope()
                .ok_or("raw timing certificate lacks static envelope")?;
            if receive_epoch.saturating_sub(source_epoch)
                > declared.directions[source].pmax_samples_upper_bound
            {
                return Ok(None);
            }
            directions.push(RawDirectionTiming {
                source_endpoint: c.endpoints[source].id.clone(),
                receiver_endpoint: c.endpoints[receiver].id.clone(),
                source_startup_sample: tx.startup_sample(),
                receiver_startup_sample: rx.startup_sample(),
                source_data_epoch_sample: source_epoch,
                reconstructed_rx_epoch_sample: receive_epoch,
                source_initial_idle_end_sample: source_end,
                received_initial_idle_end_sample: received_end,
                source_switch_pfc24: switch,
                pmax_samples: receive_epoch.saturating_sub(source_epoch),
                callback_rounding_samples: 0,
            });
        }
        let bytes =
            serde_json::to_vec(c.as_ref()).map_err(|_| "raw timing config serialization failed")?;
        Ok(Some(RawTimingCertificate {
            medium_config_sha256: format!("{:x}", Sha256::digest(bytes)),
            attachment_config_sha256: None,
            attachment_origin_ps: 0,
            endpoints: c
                .endpoints
                .iter()
                .map(|endpoint| endpoint.id.clone())
                .collect(),
            physical_epoch_start_samples: s
                .ports
                .iter()
                .map(|port| port.gigabit_link.as_ref().unwrap().startup_sample())
                .collect(),
            grid: c.channel.grid,
            bilateral_ready: true,
            initial_idle_verified: true,
            directions,
        }))
    }
    pub fn raw_ingress_status(&self, endpoint: &str) -> Result<RawIngressStatus> {
        if !self.config.gigabit_raw_merge {
            return Err("raw xMII service is disabled".into());
        }
        let index = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == endpoint)
            .ok_or("unknown raw xMII endpoint")?;
        let port = &self.state.ports[index];
        let active = port.raw_active.as_ref();
        let ui = self.config.channel.grid.samples_per_ui as u64;
        let (next_cut_sample, consumed, due) =
            if let Some(tx) = port.gigabit_link.as_ref().and_then(t1_link::Port::raw_tx) {
                let to_boundary = (ui - self.state.index % ui) % ui;
                (
                    self.state.index + to_boundary + tx.symbols_to_next_ingress() * ui,
                    tx.consumed_slots(),
                    tx.ingress_due() && to_boundary == 0,
                )
            } else {
                (u64::MAX, 0, false)
            };
        Ok(RawIngressStatus {
            ready: due
                && port.gigabit_link.as_ref().is_some_and(t1_link::Port::ready)
                && active.is_none(),
            next_cut_sample,
            consumed_gmii_slots: consumed,
            active_packet_id: active.map(|p| p.request.packet_id),
            generation: active.map(|p| p.request.generation),
        })
    }
    pub fn admit_raw_mpacket(&mut self, request: RawMPacketTx) -> Result<PhyEffect> {
        let status = self.raw_ingress_status(&request.source_endpoint)?;
        let now = self.config.channel.grid.time(self.state.index)?;
        let index = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == request.source_endpoint)
            .ok_or("unknown raw xMII endpoint")?;
        if !status.ready
            || status.next_cut_sample != self.state.index
            || match request.reference_sample {
                None => u128::from(request.at_ps) * now.denominator != now.numerator_ps,
                Some(sample) => {
                    sample != self.state.index
                        || u128::from(request.at_ps) != now.numerator_ps.div_ceil(now.denominator)
                }
            }
            || request.origin_frame_id.is_empty()
            || request.origin_frame_id.len() > 128
            || !(72..=self.config.maximum_mac_bytes + 8).contains(&request.xmii_bytes.len())
            || request.xmii_bytes[..6] != [0x55; 6]
            || self.state.raw_seen.len() >= 4096
            || self.state.ports[index].raw_spans.len() >= 128
            || !self
                .state
                .raw_seen
                .insert((request.source_endpoint.clone(), request.packet_id))
        {
            return Err("raw mPacket admission is not at a free exact xMII cut".into());
        }
        let origin = Origin {
            frame_id: request.origin_frame_id.clone(),
            source_endpoint: request.source_endpoint.clone(),
            attempt: 0,
        };
        let first_slot = self.state.ports[index]
            .gigabit_link
            .as_ref()
            .and_then(t1_link::Port::raw_tx)
            .ok_or("raw xMII transmitter not started")?
            .consumed_slots();
        let effect = PhyEffect::XmiStarted {
            endpoint: request.source_endpoint.clone(),
            packet_id: request.packet_id,
            generation: request.generation,
            sample_index: self.state.index,
        };
        self.state.ports[index].raw_spans.push_back(RawSpan {
            packet_id: request.packet_id,
            generation: request.generation,
            origin: origin.clone(),
            start_slot: first_slot,
            end_slot: first_slot,
            coded_end_symbol: 0,
            coded_reported: false,
            generation_lineage: vec![(request.generation, 0)],
            delimiter_slot: None,
            delimiter_generation: None,
            delimiter_end_symbol: None,
            delimiter_sample: None,
        });
        self.state.ports[index].raw_active = Some(RawActive {
            request,
            origin,
            consumed_octets: 0,
            start_slot: Some(first_slot),
            start_sample: self.state.index,
            end_slot: None,
            end_at_sample: None,
            release_at_sample: None,
        });
        Ok(effect)
    }
    pub fn replace_unconsumed_raw_suffix(
        &mut self,
        endpoint: &str,
        packet_id: u64,
        expected_generation: u64,
        from_packet_octet: usize,
        suffix: &[u8],
        new_generation: u64,
    ) -> Result<()> {
        if new_generation <= expected_generation {
            return Err("raw suffix generation did not advance".into());
        }
        let index = self
            .config
            .endpoints
            .iter()
            .position(|e| e.id == endpoint)
            .ok_or("unknown raw xMII endpoint")?;
        let port = &mut self.state.ports[index];
        if port
            .raw_spans
            .back()
            .is_none_or(|span| span.packet_id != packet_id)
        {
            return Err("active raw mPacket lineage mismatch".into());
        }
        if port.raw_spans.back().unwrap().generation_lineage.len() >= 4096 {
            return Err("raw consumed-prefix generation lineage bound exceeded".into());
        }
        let active = port.raw_active.as_mut().ok_or("no active raw mPacket")?;
        if active.request.packet_id != packet_id
            || active.request.generation != expected_generation
            || from_packet_octet < active.consumed_octets
            || from_packet_octet > active.request.xmii_bytes.len()
            || active.end_slot.is_some()
        {
            return Err("raw suffix rewrite crosses consumed xMII prefix".into());
        }
        let next_len = from_packet_octet
            .checked_add(suffix.len())
            .ok_or("raw suffix length overflow")?;
        if next_len <= active.consumed_octets
            || !(72..=self.config.maximum_mac_bytes + 8).contains(&next_len)
        {
            return Err("raw suffix length outside mPacket bound".into());
        }
        active.request.xmii_bytes.truncate(from_packet_octet);
        active.request.xmii_bytes.extend_from_slice(suffix);
        active.request.generation = new_generation;
        let span = port.raw_spans.back_mut().unwrap();
        span.generation = new_generation;
        span.generation_lineage
            .push((new_generation, active.consumed_octets));
        Ok(())
    }
    pub fn has_pending(&self) -> bool {
        if self.config.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
            return self.state.ports.iter().any(|p| {
                !p.queue.is_empty()
                    || p.raw_active.is_some()
                    || p.raw_spans.iter().any(|span| !span.coded_reported)
                    || p.gigabit_program.is_some()
                    || !p.gigabit_rx_origins.is_empty()
                    || p.gigabit_link.as_ref().is_some_and(|link| {
                        !matches!(link.phase(), t1_link::Phase::Data | t1_link::Phase::Fault)
                    })
            });
        }
        self.state.ports.iter().any(|p| {
            !p.queue.is_empty()
                || p.program.is_some()
                || p.plca_beacon_in_flight
                || p.plca_commit_active
                || !p.plca_control_chips.is_empty()
                || p.carrier
                || p.token.is_some()
                || !p.repeater_chips.is_empty()
                || p.idle_samples < self.config.ifg_samples()
        }) || self
            .state
            .raw_history
            .iter()
            .any(|h| h.iter().any(|&v| v != 0))
            || self.state.repeater.as_ref().is_some_and(|r| !r.is_idle())
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(&Saved {
            version: 1,
            config: (*self.config).clone(),
            state: self.state.clone(),
        })
        .map_err(|e| e.to_string())
    }
    pub fn validate_admissions(&self, requests: &[MacTx]) -> Result<()> {
        let expected: std::collections::BTreeMap<_, _> =
            requests.iter().map(|r| (r.frame_id.as_str(), r)).collect();
        if expected.len() != requests.len()
            || self
                .state
                .seen
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != expected.keys().copied().collect()
        {
            return Err("sampled-medium saved admissions differ from workload".into());
        }
        for port in &self.state.ports {
            for request in &port.queue {
                if expected
                    .get(request.0.frame_id.as_str())
                    .is_none_or(|r| **r != *request.0)
                {
                    return Err("sampled-medium saved pending frame differs from workload".into());
                }
            }
        }
        Ok(())
    }
    pub fn restore(config: &MediumConfig, bytes: &[u8]) -> Result<Self> {
        let saved: Saved = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if saved.version != 1 || &saved.config != config {
            return Err("sampled-medium checkpoint immutable mismatch".into());
        }
        let result = Self {
            config: Arc::new(config.clone()),
            state: saved.state,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<()> {
        let c = &self.config;
        let s = &self.state;
        c.validate()?;
        if s.index > c.maximum_samples
            || s.ports.len() != c.endpoints.len()
            || s.raw_history.len() != s.ports.len()
            || s.origin_history.len() != s.ports.len()
            || s.seen.len() > 4096
            || s.raw_seen.len() > 4096
            || s.channel.next_index() != s.index
            || c.physical_mode != PhysicalMode::Ieee1000BaseT1PointToPoint2022
                && s.gigabit_mdi_disconnected
        {
            return Err("sampled-medium checkpoint dimensions/index/bounds".into());
        }
        let pending: Vec<_> = s
            .ports
            .iter()
            .flat_map(|p| p.queue.iter().map(|r| r.0.frame_id.clone()))
            .collect();
        let pending_set: BTreeSet<_> = pending.iter().cloned().collect();
        if pending_set.len() != pending.len()
            || !pending_set.is_disjoint(&s.terminal)
            || pending_set
                .union(&s.terminal)
                .cloned()
                .collect::<BTreeSet<_>>()
                != s.seen
        {
            return Err(
                "saved PHY admissions do not partition into pending and terminal frames".into(),
            );
        }
        MatrixChannel::restore(&c.channel, &s.channel.checkpoint()?)?;
        match (&c.repeater, &s.repeater) {
            (Some(config), Some(state)) => {
                repeater::Repeater::restore(config, &state.checkpoint()?)?;
            }
            (None, None) => {}
            _ => return Err("repeater checkpoint presence mismatch".into()),
        }
        for (i, p) in s.ports.iter().enumerate() {
            if let Some(program) = &p.program {
                if let Some(left) = program.mac_jam_samples_left {
                    let duration = c.jam_bits as u64 * c.mac_bit_samples();
                    if c.physical_mode != PhysicalMode::Ieee10BaseT1sMultidrop2022
                        || p.plca_node.is_none()
                        || !program.jam
                        || left == 0
                        || left > duration
                        || program.next_chip != 0
                        || program.t1s_group != 0
                        || !program.t1s_chips.is_empty()
                        || program.sfd_reported
                        || program.fcs_reported
                        || program.mac_input_end_sample != program.started.checked_add(duration)
                        || program
                            .mac_input_end_sample
                            .and_then(|end| end.checked_sub(s.index))
                            != Some(left)
                    {
                        return Err("invalid saved PLCA MAC-only JAM/source clock".into());
                    }
                }
            }
            let is_mau = c.repeater.as_ref().is_some_and(|r| {
                r.pairs
                    .iter()
                    .any(|pair| pair.mau_endpoint == c.endpoints[i].id)
            });
            if p.tx.next_index() != s.index
                || p.rx.next_index() != s.index
                || p.attempt > c.retry_limit
                || p.queue.is_empty() && p.attempt != 0
                || p.below > c.carrier_hold_samples
                || p.idle_samples > c.ifg_samples()
                || s.raw_history[i].len() != s.origin_history[i].len()
                || s.raw_history[i].len() != c.history_len(i).min(s.index as usize)
                || p.token.as_ref() != p.rx.active_frame_token()
                || p.backoff_samples > 1023 * c.slot_bits as u64 * c.mac_bit_samples()
                || s.raw_history[i]
                    .iter()
                    .any(|v| v.unsigned_abs() > 1_000_000_000)
                || p.queue.iter().any(|r| {
                    r.0.source_endpoint != c.endpoints[i].id || !s.seen.contains(&r.0.frame_id)
                })
                || (is_mau && (!p.queue.is_empty() || p.program.is_some()))
                || p.repeater_chips.len() > 1
                || p.repeater_chips
                    .iter()
                    .any(|chip| ![-1, 1].contains(&chip.level))
                || p.repeater_recovered_chip
                    .is_some_and(|chip| !(-1..=1).contains(&chip))
                || (!is_mau
                    && (!p.repeater_chips.is_empty()
                        || !p.repeater_current_origins.is_empty()
                        || p.repeater_recovered_chip.is_some()
                        || !p.repeater_recovered_origins.is_empty()))
                || match (&p.mau, c.physical_mode) {
                    (Some(mau), PhysicalMode::Ieee10BaseT2022) => {
                        !mau.validate(c.channel.grid.samples_per_ui) || mau.next_sample() != s.index
                    }
                    (None, PhysicalMode::Ieee10BaseT2022) => true,
                    (Some(_), _) => true,
                    (None, _) => false,
                }
                || p.mau.is_none() && p.mau_raw_active
            {
                return Err("invalid sampled-medium endpoint state".into());
            }
            Transmitter::restore(&c.endpoints[i].transmitter, &p.tx.checkpoint()?)?;
            Receiver::restore(&c.endpoints[i].receiver, &p.rx.checkpoint()?)?;
            if let Some(domain) = &c.plca {
                let node = p.plca_node.as_ref().ok_or("missing saved PLCA node")?;
                if node.config != domain.stations[i]
                    || node.samples_per_mac_bit != c.mac_bit_samples()
                    || node.node_count != domain.node_count
                    || node.to_timer_bits != domain.to_timer_bits
                    || node.burst_timer_bits != domain.burst_timer_bits
                    || node.max_burst_count != domain.max_burst_count
                    || node.config.node_id == 0 && node.cur_id > domain.node_count
                    || p.plca_control_chips.len() > 53
                    || (p.plca_beacon_in_flight || p.plca_commit_active)
                        && p.program
                            .as_ref()
                            .is_some_and(|program| program.mac_jam_samples_left.is_none())
                    || p.plca_commit_active && p.plca_beacon_in_flight
                    || p.plca_beacon_waveform_done && !p.plca_beacon_in_flight
                    || p.plca_control_chips
                        .iter()
                        .any(|chip| !(-1..=1).contains(chip))
                    || !p.plca_beacon_in_flight
                        && !p.plca_commit_active
                        && !p.plca_control_chips.is_empty()
                        && (p.program.as_ref().is_some_and(|program| {
                            program.mac_jam_samples_left.is_none()
                        }) || p.plca_control_chips.iter().any(|chip| *chip == 0)
                            || s.index == 0
                            // Only the unstarted suffix of this MCD's ten-chip
                            // COMMIT group may outlive command removal. Derive
                            // its size from the existing TX oscillator cut.
                            || p.plca_control_chips.len()
                                != 9 - (((s.index - 1) as u128
                                    * (1_000_000i64
                                        + c.endpoints[i].transmitter.clock_ppm as i64)
                                        as u128
                                    / (c.channel.grid.samples_per_ui as u128 * 1_000_000))
                                    % 10) as usize)
                    || node.data_timer_samples
                        > plca::MiiClock::new(
                            p.tx.next_index(),
                            c.mac_bit_samples(),
                            c.endpoints[i].transmitter.clock_ppm,
                        )
                        .bit_samples(plca::PENDING_BITS)
                    || node.hold_samples > s.index
                    || node.hold_samples
                        > (plca::DELAY_LINE_BITS + plca::COMMIT_BITS) * node.samples_per_mac_bit
                    || node.invalid_beacon_samples
                        >= plca::INVALID_BEACON_BITS * node.samples_per_mac_bit
                {
                    return Err("invalid saved PLCA physical node state".into());
                }
            } else if p.plca_node.is_some()
                || p.plca_beacon_in_flight
                || p.plca_beacon_waveform_done
                || p.plca_received_beacon
                || p.plca_commit_active
                || !p.plca_control_chips.is_empty()
            {
                return Err("unexpected PLCA state without configuration".into());
            }
            p.decoder.validate(c.maximum_mac_bytes)?;
            for r in &p.queue {
                wire::validate_mac(&r.0.mac_bytes, c.maximum_mac_bytes)?;
            }
            if let Some(program) = &p.program {
                if program.next_chip > program.len(c)
                    || program.started > s.index
                    || p.queue.front().is_none_or(|r| r.0 != program.request.0)
                    || program.origin.frame_id != program.request.0.frame_id
                    || program.origin.source_endpoint != c.endpoints[i].id
                    || program.origin.attempt != p.attempt
                {
                    return Err("invalid sampled-medium active attempt".into());
                }
            }
            match (&c.gigabit_t1, &p.gigabit_link) {
                (Some(config), Some(link)) => {
                    if link.endpoint() != config.stations[i].endpoint
                        || link.raw_enabled() != c.gigabit_raw_merge
                        || !link.raw_state_valid()
                        || p.raw_spans.len() > 128
                        || p.gigabit_raw_tx_initial_end_sample
                            .is_some_and(|sample| sample > s.index)
                        || p.gigabit_raw_rx_initial_end_sample
                            .is_some_and(|sample| sample > s.index)
                        || p.gigabit_raw_rx_initial_end_sample.is_some()
                            != link.raw_initial_idle_verified()
                        || p.gigabit_mdi_spans.len() > 4096
                        || p.gigabit_mdi_spans.iter().any(|span| {
                            span.sfd_sample > s.index
                                || span.fcs_sample.is_some_and(|sample| {
                                    sample > s.index || sample <= span.sfd_sample
                                })
                                || span.sfd_symbol >= span.fcs_symbol
                        })
                        || p.gigabit_rx_origins.len() > 4096
                        || p.gigabit_rx_origins
                            .values()
                            .any(|&(first, last)| first > last || last > s.index)
                        || !c.gigabit_raw_merge
                            && (p.raw_active.is_some() || !p.raw_spans.is_empty())
                        || p.raw_active.as_ref().is_some_and(|active| {
                            active.request.source_endpoint != c.endpoints[i].id
                                || active.origin.frame_id != active.request.origin_frame_id
                                || active.consumed_octets > active.request.xmii_bytes.len()
                                || active.request.reference_sample.is_some_and(|sample| {
                                    sample != active.start_sample
                                        || c.channel.grid.time(sample).map_or(true, |time| {
                                            u128::from(active.request.at_ps)
                                                != time.numerator_ps.div_ceil(time.denominator)
                                        })
                                })
                                || active.start_sample > s.index
                                || active.start_slot.is_none()
                                || !s.raw_seen.contains(&(
                                    active.request.source_endpoint.clone(),
                                    active.request.packet_id,
                                ))
                        })
                        || p.raw_spans.iter().any(|span| {
                            span.end_slot < span.start_slot
                                || span.coded_end_symbol != 0 && span.coded_end_symbol % 2700 != 0
                                || span.generation_lineage.is_empty()
                                || span.generation_lineage.len() > 4096
                                || span
                                    .generation_lineage
                                    .last()
                                    .is_none_or(|&(generation, _)| generation != span.generation)
                                || span
                                    .generation_lineage
                                    .windows(2)
                                    .any(|pair| pair[0].0 >= pair[1].0 || pair[0].1 > pair[1].1)
                                || span.delimiter_slot.is_some()
                                    != span.delimiter_generation.is_some()
                                || span.delimiter_generation.is_some_and(|generation| {
                                    !span.generation_lineage.iter().any(|&(g, prefix)| {
                                        g == generation
                                            && span.delimiter_slot.is_some_and(|slot| {
                                                span.start_slot + prefix as u64 <= slot
                                            })
                                    })
                                })
                                || span.delimiter_sample.is_some_and(|sample| sample > s.index)
                                || span.delimiter_sample.is_some()
                                    != span.delimiter_end_symbol.is_some()
                        })
                        || p.gigabit_program.as_ref().is_some_and(|program| {
                            p.queue.front().is_none_or(|r| r.0 != program.request.0)
                                || program.origin.frame_id != program.request.0.frame_id
                                || program.origin.source_endpoint != c.endpoints[i].id
                                || program.start_sample > s.index
                        })
                    {
                        return Err("invalid 1000BASE-T1 sampled link state".into());
                    }
                }
                (None, None)
                    if p.gigabit_program.is_none()
                        && p.gigabit_rx_data_start.is_none()
                        && p.gigabit_rx_origins.is_empty() => {}
                _ => return Err("1000BASE-T1 checkpoint presence mismatch".into()),
            }
        }
        Ok(())
    }
    /// Exactly one simultaneous physical sample transaction. All endpoint drive
    /// decisions precede channel resolution; local observations follow it.
    pub fn advance_one_sample(&mut self) -> Result<Vec<PhyEffect>> {
        self.advance_samples(1)
    }
    /// Used only inside an outer transactional attachment clone.  Keeping
    /// that single transaction owner avoids cloning two complete PHY states
    /// for every sample of a millisecond-scale training sequence.
    pub(crate) fn advance_one_sample_in_transaction(&mut self) -> Result<Vec<PhyEffect>> {
        self.advance_inner()
    }
    /// Exact scalar execution with one transactional state clone for a bounded
    /// run of samples. The caller must stop at every external FES/control
    /// boundary; this method does not extrapolate receiver or timer state.
    pub fn advance_samples(&mut self, count: u64) -> Result<Vec<PhyEffect>> {
        if count
            > if self.config.trace_samples {
                4096
            } else {
                1_000_000
            }
        {
            return Err("sampled-medium batch exceeds bounded effect capacity".into());
        }
        let mut next = self.clone();
        let mut effects = vec![];
        for _ in 0..count {
            effects.extend(next.advance_inner()?);
        }
        *self = next;
        Ok(effects)
    }
    fn advance_gigabit_inner(&mut self) -> Result<Vec<PhyEffect>> {
        let c = &self.config;
        let s = &mut self.state;
        if s.index >= c.maximum_samples {
            return Err("1000BASE-T1 sampled-medium numerical budget exhausted".into());
        }
        let ui = c.channel.grid.samples_per_ui as u64;
        let boundary = s.index.checked_add(1).ok_or("sample index overflow")?;
        let exact = c.channel.grid.time(s.index)?;
        let mut effects = Vec::new();
        let mut drives = Vec::with_capacity(2);
        let mut tx_origins = vec![BTreeSet::new(); 2];
        for (i, p) in s.ports.iter_mut().enumerate() {
            let endpoint = &c.endpoints[i].id;
            let mac_start_allowed = Self::mac_permission_at(c, p, s.index);
            let link = p
                .gigabit_link
                .as_mut()
                .ok_or("missing 1000BASE-T1 link port")?;
            if link.ready()
                && mac_start_allowed
                && p.gigabit_program.is_none()
                && p.tx.at_symbol_boundary()
                && p.tx.pending_symbols() == 0
                && link.data_tx().is_some_and(|tx| {
                    tx.pending_gmii() == 0
                        && tx.pending_pam3() == 0
                        && tx.symbols_emitted() % 2700 == 0
                })
                && p.queue.front().is_some_and(|request| {
                    request.0.at_ps as u128 * exact.denominator <= exact.numerator_ps
                })
            {
                let request = p.queue.front().ok_or("missing gigabit queue head")?.clone();
                let origin = Origin {
                    frame_id: request.0.frame_id.clone(),
                    source_endpoint: endpoint.clone(),
                    attempt: 0,
                };
                let range = link.enqueue_mac(&request.0.mac_bytes)?;
                p.gigabit_program = Some(GigabitProgram {
                    request,
                    origin: origin.clone(),
                    range,
                    start_sample: s.index,
                    sfd_reported: false,
                    fcs_reported: false,
                });
                effects.push(PhyEffect::TxStarted {
                    endpoint: endpoint.clone(),
                    origin: origin.clone(),
                    sample_index: s.index,
                });
                effects.push(PhyEffect::TxMacPreamble {
                    endpoint: endpoint.clone(),
                    origin,
                    sample_index: s.index,
                });
            }
            if c.gigabit_raw_merge
                && p.tx.at_symbol_boundary()
                && link.raw_tx().is_some_and(|tx| tx.ingress_due())
            {
                let mut octet = t1_gigabit::Gmii::Control(t1_gigabit::Control::InterFrameReady);
                let mut delimiter_consumed = None;
                if let Some(active) = &mut p.raw_active {
                    if active.consumed_octets < active.request.xmii_bytes.len() {
                        let octet_index = active.consumed_octets;
                        let value = active.request.xmii_bytes[octet_index];
                        let delimiter_index = if active.request.xmii_bytes[6] == 0x55 {
                            7
                        } else {
                            6
                        };
                        if octet_index == delimiter_index {
                            delimiter_consumed = Some((
                                active.request.packet_id,
                                active.request.generation,
                                octet_index,
                                value,
                            ));
                        }
                        octet = t1_gigabit::Gmii::Data(value);
                        active.consumed_octets += 1;
                    }
                }
                let tx = link
                    .raw_tx_mut()
                    .ok_or("raw xMII transmitter missing at ingress cut")?;
                tx.push_gmii_marked(octet, delimiter_consumed.is_some())?;
                if let Some((packet_id, generation, octet_index, value)) = delimiter_consumed {
                    let span = p
                        .raw_spans
                        .back_mut()
                        .ok_or("raw delimiter source span absent")?;
                    if span.packet_id != packet_id || span.delimiter_slot.is_some() {
                        return Err("raw delimiter source identity changed".into());
                    }
                    span.delimiter_slot = Some(tx.consumed_slots() - 1);
                    span.delimiter_generation = Some(generation);
                    effects.push(PhyEffect::XmiDelimiterConsumed {
                        endpoint: endpoint.clone(),
                        packet_id,
                        generation,
                        octet_index,
                        value,
                        consumed_cut_sample: s.index,
                        sample_index: boundary,
                    });
                }
                if let Some(active) = &mut p.raw_active {
                    if active.consumed_octets == active.request.xmii_bytes.len()
                        && active.end_slot.is_none()
                    {
                        let end_slot = tx.consumed_slots();
                        active.end_slot = Some(end_slot);
                        active.end_at_sample = Some(s.index + 6 * ui);
                        active.release_at_sample = Some(s.index + 6 * ui * 13);
                        let span = p.raw_spans.back_mut().ok_or("raw source span absent")?;
                        if span.packet_id != active.request.packet_id {
                            return Err("raw source span identity changed".into());
                        }
                        span.end_slot = end_slot;
                        span.generation = active.request.generation;
                        span.coded_end_symbol = ((end_slot - 1) / 450 + 2) * 2700;
                    }
                }
            }
            let was_an = link.phase() == t1_link::Phase::AutoNegotiation;
            let (symbol, events) = link.drive(s.index, c.channel.grid.samples_per_ui)?;
            for event in events {
                match event {
                    t1_link::Event::Restarting(reason) => {
                        p.gigabit_rx_data_start = None;
                        p.gigabit_rx_origins.clear();
                        effects.push(PhyEffect::TxReady {
                            endpoint: endpoint.clone(),
                            ready: false,
                            sample_index: boundary,
                        });
                        effects.push(PhyEffect::PcsControl {
                            endpoint: endpoint.clone(),
                            command: format!("startup_restarting:{reason}"),
                            sample_index: boundary,
                        });
                    }
                    t1_link::Event::Fault(reason) => {
                        p.raw_active = None;
                        for span in p.raw_spans.drain(..) {
                            effects.push(PhyEffect::XmiAborted {
                                endpoint: endpoint.clone(),
                                packet_id: span.packet_id,
                                generation: span.generation,
                                reason: reason.clone(),
                                sample_index: boundary,
                            });
                        }
                        effects.push(PhyEffect::PcsControl {
                            endpoint: endpoint.clone(),
                            command: format!("link_fault:{reason}"),
                            sample_index: boundary,
                        });
                        while let Some(request) = p.queue.pop_front() {
                            s.terminal.insert(request.0.frame_id.clone());
                            effects.push(PhyEffect::RetryExhausted {
                                endpoint: endpoint.clone(),
                                origin: Origin {
                                    frame_id: request.0.frame_id.clone(),
                                    source_endpoint: endpoint.clone(),
                                    attempt: 0,
                                },
                                sample_index: boundary,
                            });
                        }
                        p.gigabit_program = None;
                    }
                    t1_link::Event::DataReady => effects.push(PhyEffect::TxReady {
                        endpoint: endpoint.clone(),
                        ready: true,
                        sample_index: boundary,
                    }),
                    _ => {}
                }
            }
            if p.tx.at_symbol_boundary() && p.tx.pending_symbols() == 0 {
                p.tx.push_symbols(&[if was_an { 0 } else { symbol.unwrap_or(0) }])?;
            }
            let voltage = p.tx.render_one_in_transaction()?;
            drives.push(if was_an {
                symbol.unwrap_or(0) as i64 * c.endpoints[i].transmitter.peak_uv
            } else {
                voltage
            });
            if let Some(program) = &p.gigabit_program {
                tx_origins[i].insert(program.origin.clone());
            }
            s.raw_history[i].push_front(drives[i]);
            s.raw_history[i].truncate(c.history_len(i));
            s.origin_history[i].push_front(tx_origins[i].clone());
            s.origin_history[i].truncate(c.history_len(i));
            if p.tx.at_symbol_boundary() && p.tx.pending_symbols() == 0 {
                if let Some(program) = &mut p.gigabit_program {
                    let emitted = link.data_tx().map_or(0, |tx| tx.symbols_emitted());
                    if !program.sfd_reported && emitted >= program.range.sfd_end_symbol() {
                        program.sfd_reported = true;
                        if p.gigabit_mdi_spans.len() >= 4096 {
                            return Err("canonical MDI source span bound exceeded".into());
                        }
                        p.gigabit_mdi_spans.push_back(CanonicalMdiSpan {
                            origin: program.origin.clone(),
                            sfd_symbol: program.range.sfd_end_symbol(),
                            fcs_symbol: program.range.fcs_end_symbol(),
                            sfd_sample: boundary,
                            fcs_sample: None,
                        });
                        effects.push(PhyEffect::TxMacSfd {
                            endpoint: endpoint.clone(),
                            origin: program.origin.clone(),
                            sample_index: boundary,
                        });
                    }
                    if !program.fcs_reported && emitted >= program.range.fcs_end_symbol() {
                        program.fcs_reported = true;
                        let span = p
                            .gigabit_mdi_spans
                            .back_mut()
                            .ok_or("canonical FCS lacks actual SFD span")?;
                        if span.origin != program.origin {
                            return Err("canonical FCS source span identity changed".into());
                        }
                        span.fcs_sample = Some(boundary);
                        effects.push(PhyEffect::TxMacFcsEnd {
                            endpoint: endpoint.clone(),
                            origin: program.origin.clone(),
                            sample_index: boundary,
                        });
                    }
                    if emitted >= program.range.coded_end_symbol {
                        let program = p.gigabit_program.take().unwrap();
                        effects.push(PhyEffect::TxCodedEnd {
                            endpoint: endpoint.clone(),
                            origin: program.origin.clone(),
                            sample_index: boundary,
                        });
                        effects.push(PhyEffect::TxFinished {
                            endpoint: endpoint.clone(),
                            origin: program.origin,
                            sample_index: boundary,
                        });
                        let popped = p
                            .queue
                            .pop_front()
                            .ok_or("gigabit coded end lacks queue head")?;
                        if popped.0.frame_id != program.request.0.frame_id {
                            return Err("gigabit coded end queue identity changed".into());
                        }
                        s.terminal.insert(popped.0.frame_id.clone());
                        effects.push(PhyEffect::TxReady {
                            endpoint: endpoint.clone(),
                            ready: true,
                            sample_index: boundary,
                        });
                    }
                }
            }
            if c.gigabit_raw_merge {
                if let Some(active) = &p.raw_active {
                    let delimiter_index = if active.request.xmii_bytes[6] == 0x55 {
                        7
                    } else {
                        6
                    };
                    if boundary == active.start_sample + delimiter_index as u64 * 6 * ui {
                        effects.push(PhyEffect::XmiDelimiter {
                            endpoint: endpoint.clone(),
                            packet_id: active.request.packet_id,
                            generation: active.request.generation,
                            octet_index: delimiter_index,
                            value: active.request.xmii_bytes[delimiter_index],
                            sample_index: boundary,
                        });
                    }
                    if active.end_at_sample == Some(boundary) {
                        effects.push(PhyEffect::XmiEnd {
                            endpoint: endpoint.clone(),
                            packet_id: active.request.packet_id,
                            generation: active.request.generation,
                            sample_index: boundary,
                        });
                    }
                    if active.release_at_sample == Some(boundary) {
                        effects.push(PhyEffect::XmiReleased {
                            endpoint: endpoint.clone(),
                            packet_id: active.request.packet_id,
                            generation: active.request.generation,
                            sample_index: boundary,
                        });
                        p.raw_active = None;
                    }
                }
                if p.tx.at_symbol_boundary()
                    && p.tx.pending_symbols() == 0
                    && link.raw_tx().is_some()
                {
                    let tx = link
                        .raw_tx_mut()
                        .ok_or("raw transmitter missing at marker cut")?;
                    let emitted = tx.emitted_symbols();
                    if emitted == 2700 {
                        p.gigabit_raw_tx_initial_end_sample = Some(boundary);
                    }
                    while let Some(marker) = tx.take_completed_delimiter() {
                        let span = p
                            .raw_spans
                            .iter_mut()
                            .find(|span| span.delimiter_slot == Some(marker.slot))
                            .ok_or("raw mapped delimiter lacks consumed-prefix lineage")?;
                        if span.delimiter_sample.is_some() || marker.end_symbol != emitted {
                            return Err(
                                "raw mapped delimiter missed its actual renderer cut".into()
                            );
                        }
                        span.delimiter_end_symbol = Some(marker.end_symbol);
                        span.delimiter_sample = Some(boundary);
                        effects.push(PhyEffect::MdiDelimiterEnd {
                            source_endpoint: endpoint.clone(),
                            packet_id: span.packet_id,
                            generation: span
                                .delimiter_generation
                                .ok_or("raw delimiter generation absent")?,
                            delimiter_slot: marker.slot,
                            sample_index: boundary,
                        });
                    }
                    for span in &mut p.raw_spans {
                        if span.coded_end_symbol != 0
                            && !span.coded_reported
                            && emitted >= span.coded_end_symbol
                        {
                            span.coded_reported = true;
                            effects.push(PhyEffect::MdiCodedEnd {
                                endpoint: endpoint.clone(),
                                packet_id: span.packet_id,
                                generation: span.generation,
                                sample_index: boundary,
                            });
                        }
                    }
                }
            }
        }
        let mut received = s.channel.process_one_in_transaction(&drives)?;
        if s.gigabit_mdi_disconnected {
            received.fill(0);
        }
        let mut canonical_received = Vec::new();
        let mut raw_received: Vec<(usize, t1_raw::ReceivedRaw)> = vec![];
        let mut raw_rejected: Vec<(usize, String, u64)> = vec![];
        for (i, p) in s.ports.iter_mut().enumerate() {
            let endpoint = &c.endpoints[i].id;
            let decisions = p.rx.process_one_in_transaction(s.index, received[i], 0)?;
            if !s.gigabit_mdi_disconnected {
                for (j, path) in c.channel.paths[i].iter().enumerate() {
                    if let Some(path) = path {
                        for (tap, &gain) in path.taps_q30.iter().enumerate() {
                            let delay = path.delay_samples + tap;
                            if gain != 0 && s.raw_history[j].get(delay).is_some_and(|v| *v != 0) {
                                if let Some(origins) = s.origin_history[j].get(delay) {
                                    for origin in origins {
                                        p.gigabit_rx_origins
                                            .entry(origin.clone())
                                            .and_modify(|range| range.1 = boundary)
                                            .or_insert((boundary, boundary));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let link = p
                .gigabit_link
                .as_mut()
                .ok_or("missing 1000BASE-T1 link port")?;
            let recovered = decisions.last().map(|d| d.symbol);
            let events = link.observe(
                s.index,
                received[i],
                c.carrier_threshold_uv,
                recovered,
                c.channel.grid.samples_per_ui,
            )?;
            let mut link_lost = false;
            for event in events {
                match event {
                    t1_link::Event::LinkLost => link_lost = true,
                    t1_link::Event::Restarting(reason) => {
                        p.gigabit_rx_data_start = None;
                        p.gigabit_rx_origins.clear();
                        effects.push(PhyEffect::TxReady {
                            endpoint: endpoint.clone(),
                            ready: false,
                            sample_index: boundary,
                        });
                        effects.push(PhyEffect::PcsControl {
                            endpoint: endpoint.clone(),
                            command: format!("startup_restarting:{reason}"),
                            sample_index: boundary,
                        });
                    }
                    t1_link::Event::An(t1_link::AnEvent::Completed(_)) => {
                        effects.push(PhyEffect::PcsControl {
                            endpoint: endpoint.clone(),
                            command: "autoneg_complete".into(),
                            sample_index: boundary,
                        })
                    }
                    t1_link::Event::An(t1_link::AnEvent::PageRejected) => {
                        effects.push(PhyEffect::PcsControl {
                            endpoint: endpoint.clone(),
                            command: "autoneg_page_rejected".into(),
                            sample_index: boundary,
                        })
                    }
                    t1_link::Event::TrainingInfo(_) => {}
                    t1_link::Event::DataReady => effects.push(PhyEffect::TxReady {
                        endpoint: endpoint.clone(),
                        ready: true,
                        sample_index: boundary,
                    }),
                    t1_link::Event::ReceiveMac(frame) => {
                        let data_start = p
                            .gigabit_rx_data_start
                            .ok_or("gigabit MAC lacks sampled data onset")?;
                        let sfd = data_start + frame.sfd_end_physical_symbol * ui;
                        let fcs = data_start + frame.fcs_end_physical_symbol * ui;
                        effects.push(PhyEffect::RxMacSfd {
                            endpoint: endpoint.clone(),
                            sample_index: boundary,
                        });
                        canonical_received.push((
                            i,
                            frame.sfd_end_physical_symbol,
                            frame.fcs_end_physical_symbol,
                            sfd,
                            fcs,
                        ));
                        effects.push(PhyEffect::RxFrame {
                            receiver_endpoint: endpoint.clone(),
                            mac_bytes: frame.bytes,
                            origins: p
                                .gigabit_rx_origins
                                .iter()
                                .filter(|(_, (first, last))| *first <= fcs && *last > sfd)
                                .map(|(origin, _)| origin.clone())
                                .collect(),
                            rx_sfd_sample: sfd,
                            rx_fcs_sample: fcs,
                            rx_mapped_sfd_sample: None,
                            rx_mapped_fcs_sample: None,
                            sample_index: boundary,
                        });
                        // Retain origins which can overlap a following frame in this
                        // same codeword. A later tail of this frame has its own
                        // observed interval and cannot overlap that frame's SFD.
                        p.gigabit_rx_origins.retain(|_, (_, last)| *last > fcs);
                    }
                    t1_link::Event::ReceiveRaw(frame) => raw_received.push((i, frame)),
                    t1_link::Event::FrameRejected(reason) => {
                        p.gigabit_rx_origins.clear();
                        if c.gigabit_raw_merge {
                            raw_rejected.push((i, reason, link.raw_rx_slots().unwrap_or(0)));
                        } else {
                            effects.push(PhyEffect::RxAbort {
                                endpoint: endpoint.clone(),
                                reason,
                                sample_index: boundary,
                            });
                        }
                    }
                    t1_link::Event::Fault(reason)
                    | t1_link::Event::An(t1_link::AnEvent::Fault(reason)) => {
                        p.raw_active = None;
                        for span in p.raw_spans.drain(..) {
                            effects.push(PhyEffect::XmiAborted {
                                endpoint: endpoint.clone(),
                                packet_id: span.packet_id,
                                generation: span.generation,
                                reason: reason.clone(),
                                sample_index: boundary,
                            });
                        }
                        effects.push(PhyEffect::PcsControl {
                            endpoint: endpoint.clone(),
                            command: format!("link_fault:{reason}"),
                            sample_index: boundary,
                        });
                        while let Some(request) = p.queue.pop_front() {
                            s.terminal.insert(request.0.frame_id.clone());
                            effects.push(PhyEffect::RetryExhausted {
                                endpoint: endpoint.clone(),
                                origin: Origin {
                                    frame_id: request.0.frame_id.clone(),
                                    source_endpoint: endpoint.clone(),
                                    attempt: 0,
                                },
                                sample_index: boundary,
                            });
                        }
                        p.gigabit_program = None;
                    }
                    _ => {}
                }
            }
            if p.gigabit_rx_data_start.is_none() && link.data_rx_started() {
                p.gigabit_rx_data_start = Some(boundary);
            }
            if p.gigabit_raw_rx_initial_end_sample.is_none() && link.raw_initial_idle_verified() {
                if link.raw_rx_symbols() != Some(2700) {
                    return Err(
                        "raw initial-idle decode callback missed its exact counter cut".into(),
                    );
                }
                p.gigabit_raw_rx_initial_end_sample = Some(boundary);
            }
            let receiver_events = p.rx.take_events();
            let lost_lock = receiver_events
                .iter()
                .any(|event| matches!(event, ReceiverEvent::LockLost { .. }));
            for event in receiver_events {
                effects.push(PhyEffect::Receiver {
                    endpoint: endpoint.clone(),
                    event,
                });
            }
            if link_lost
                || lost_lock
                    && matches!(
                        link.phase(),
                        t1_link::Phase::Data | t1_link::Phase::DataIdle
                    )
            {
                let reason = "1000BASE-T1 receive lock lost; restarting link".to_string();
                raw_received.retain(|(receiver, _)| *receiver != i);
                effects.retain(|effect| {
                    !matches!(effect, PhyEffect::RxFrame { receiver_endpoint, .. } if receiver_endpoint == endpoint)
                });
                if !link_lost {
                    link.restart(boundary, c.channel.grid.samples_per_ui)?;
                }
                p.gigabit_rx_data_start = None;
                p.gigabit_mdi_spans.clear();
                p.gigabit_raw_tx_initial_end_sample = None;
                p.gigabit_raw_rx_initial_end_sample = None;
                p.gigabit_rx_origins.clear();
                p.raw_active = None;
                for span in p.raw_spans.drain(..) {
                    effects.push(PhyEffect::XmiAborted {
                        endpoint: endpoint.clone(),
                        packet_id: span.packet_id,
                        generation: span.generation,
                        reason: reason.clone(),
                        sample_index: boundary,
                    });
                }
                if let Some(program) = p.gigabit_program.take() {
                    let request = p.queue.pop_front().ok_or("lost PHY attempt queue head")?;
                    if request.0.frame_id != program.request.0.frame_id {
                        return Err("lost PHY attempt queue identity changed".into());
                    }
                    s.terminal.insert(request.0.frame_id.clone());
                    effects.push(PhyEffect::RetryExhausted {
                        endpoint: endpoint.clone(),
                        origin: program.origin,
                        sample_index: boundary,
                    });
                }
                effects.push(PhyEffect::RxAbort {
                    endpoint: endpoint.clone(),
                    reason: reason.clone(),
                    sample_index: boundary,
                });
                effects.push(PhyEffect::TxReady {
                    endpoint: endpoint.clone(),
                    ready: false,
                    sample_index: boundary,
                });
                effects.push(PhyEffect::PcsControl {
                    endpoint: endpoint.clone(),
                    command: "link_retraining".into(),
                    sample_index: boundary,
                });
            }
        }
        if !canonical_received.is_empty() && c.exact_mdi_timestamp_mapping_supported() {
            for (receiver, sfd_symbol, fcs_symbol, cadence_sfd, cadence_fcs) in canonical_received {
                let source = 1 - receiver;
                let spans = s.ports[source]
                    .gigabit_mdi_spans
                    .iter()
                    .filter(|span| span.sfd_symbol == sfd_symbol && span.fcs_symbol == fcs_symbol)
                    .collect::<Vec<_>>();
                if spans.len() != 1 {
                    continue;
                }
                let span = spans[0];
                let Some(tx_fcs) = span.fcs_sample else {
                    continue;
                };
                let delay = c.channel.paths[receiver][source]
                    .as_ref()
                    .unwrap()
                    .delay_samples as u64;
                let mapped_sfd = span
                    .sfd_sample
                    .checked_add(delay)
                    .ok_or("mapped SFD sample overflow")?;
                let mapped_fcs = tx_fcs
                    .checked_add(delay)
                    .ok_or("mapped FCS sample overflow")?;
                if mapped_fcs > boundary {
                    return Err("mapped FCS lies after actual receive decode".into());
                }
                if let Some(PhyEffect::RxFrame { origins, rx_mapped_sfd_sample, rx_mapped_fcs_sample, .. }) = effects.iter_mut().find(|effect| matches!(effect, PhyEffect::RxFrame { receiver_endpoint, rx_sfd_sample, rx_fcs_sample, .. } if receiver_endpoint == &c.endpoints[receiver].id && *rx_sfd_sample == cadence_sfd && *rx_fcs_sample == cadence_fcs)) {
                    // A real decoded source span and the actual channel-observed
                    // attempt must agree before publishing the exact plane.
                    if origins.as_slice() == [span.origin.clone()] {
                        *rx_mapped_sfd_sample = Some(mapped_sfd);
                        *rx_mapped_fcs_sample = Some(mapped_fcs);
                    }
                }
            }
        }
        for (receiver, frame) in raw_received {
            let source = 1 - receiver;
            let matches = s.ports[source]
                .raw_spans
                .iter()
                .filter(|span| {
                    span.start_slot == frame.start_slot && span.end_slot == frame.terminator_slot
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                effects.push(PhyEffect::RxAbort {
                    endpoint: c.endpoints[receiver].id.clone(),
                    reason: "raw mPacket lineage absent or ambiguous at decoded GMII cut".into(),
                    sample_index: boundary,
                });
                continue;
            }
            let span = matches[0];
            if span.delimiter_end_symbol != Some(frame.delimiter_end_symbol) {
                return Err("raw decoded delimiter disagrees with sealed source layout".into());
            }
            let fixed_cadence = c.endpoints.iter().all(|endpoint| {
                endpoint.transmitter.clock_ppm == 0
                    && endpoint.receiver.clock_ppm == 0
                    && endpoint.receiver.max_tracking_ppm == 0
            });
            let reconstructed_delimiter_sample = if fixed_cadence {
                Some(
                    s.ports[receiver]
                        .gigabit_rx_data_start
                        .ok_or("raw packet lacks actual received data onset")?
                        .checked_add(
                            frame
                                .delimiter_end_symbol
                                .checked_mul(ui)
                                .ok_or("raw mapped delimiter sample overflow")?,
                        )
                        .ok_or("raw mapped delimiter sample overflow")?,
                )
            } else {
                None
            };
            let delimiter_sample = if c.exact_mdi_timestamp_mapping_supported() {
                let delay = c.channel.paths[receiver][source]
                    .as_ref()
                    .unwrap()
                    .delay_samples as u64;
                Some(
                    span.delimiter_sample
                        .ok_or("raw RX lacks completed renderer source marker")?
                        .checked_add(delay)
                        .ok_or("raw mapped RX sample overflow")?,
                )
            } else {
                None
            };
            if delimiter_sample.is_some_and(|sample| sample > boundary) {
                return Err("raw mapped receive delimiter lies after decode observation".into());
            }
            effects.push(PhyEffect::RawMPacketIngress {
                receiver_endpoint: c.endpoints[receiver].id.clone(),
                packet_id: span.packet_id,
                generation: span
                    .delimiter_generation
                    .ok_or("raw ingress delimiter generation absent")?,
                final_generation: span.generation,
                delimiter_sample,
                reconstructed_delimiter_sample,
                xmii_bytes: frame.bytes,
                origins: vec![span.origin.clone()],
                receive_cut_sample: boundary,
                decoded_at_sample: boundary,
                sample_index: boundary,
            });
        }
        for (receiver, reason, decoded_slots) in raw_rejected {
            let source = 1 - receiver;
            let first = decoded_slots.saturating_sub(450);
            let mut packet_ids = s.ports[source]
                .raw_spans
                .iter()
                .filter(|span| {
                    span.start_slot < decoded_slots
                        && (span.end_slot > first || span.end_slot == span.start_slot)
                })
                .map(|span| span.packet_id)
                .collect::<Vec<_>>();
            packet_ids.sort_unstable();
            packet_ids.dedup();
            effects.push(PhyEffect::RawPacketRejected {
                receiver_endpoint: c.endpoints[receiver].id.clone(),
                packet_ids,
                reason,
                sample_index: boundary,
            });
        }
        if c.gigabit_raw_merge {
            for source in 0..2 {
                let receiver = 1 - source;
                let decoded = s.ports[receiver]
                    .gigabit_link
                    .as_ref()
                    .and_then(t1_link::Port::raw_rx_slots)
                    .unwrap_or(0);
                while s.ports[source]
                    .raw_spans
                    .front()
                    .is_some_and(|span| span.coded_reported && span.end_slot <= decoded)
                {
                    s.ports[source].raw_spans.pop_front();
                }
            }
        }
        if c.trace_samples {
            effects.push(PhyEffect::Samples {
                sample_index: boundary,
                transmitted_uv: drives,
                received_uv: received,
            });
        }
        s.index = boundary;
        Ok(effects)
    }
    fn advance_inner(&mut self) -> Result<Vec<PhyEffect>> {
        if self.config.physical_mode == PhysicalMode::Ieee1000BaseT1PointToPoint2022 {
            return self.advance_gigabit_inner();
        }
        let prior_ready: Vec<_> = self
            .state
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                !self.config.repeater.as_ref().is_some_and(|r| {
                    r.pairs
                        .iter()
                        .any(|pair| pair.mau_endpoint == self.config.endpoints[i].id)
                }) && self.port_ready(p)
            })
            .collect();
        let c = &self.config;
        let s = &mut self.state;
        if s.index >= c.maximum_samples {
            return Err(
                "sampled-medium numerical budget exhausted with actual pending state".into(),
            );
        }
        let boundary = s.index.checked_add(1).ok_or("sample index overflow")?;
        let exact = c.channel.grid.time(s.index)?;
        let mut effects = vec![];
        let mut observed_origins = vec![BTreeSet::new(); s.ports.len()];
        let mut plca_permits = vec![true; s.ports.len()];
        let mut plca_carrier_holds = vec![false; s.ports.len()];
        for (i, p) in s.ports.iter_mut().enumerate() {
            let mac_start_allowed = Self::mac_permission_at(c, p, s.index);
            if let Some(node) = &mut p.plca_node {
                let clock = plca::MiiClock::new(
                    p.tx.next_index(),
                    c.mac_bit_samples(),
                    c.endpoints[i].transmitter.clock_ppm,
                );
                let mut observed = std::mem::take(&mut p.plca_observation);
                observed.clock = Some(clock);
                observed.carrier = p.carrier;
                observed.beacon |= p.carrier
                    && p.t1s_pcs_rx
                        .as_ref()
                        .is_some_and(t1s::PcsReceiver::beacon_active);
                observed.commit |= p.carrier
                    && p.t1s_pcs_rx
                        .as_ref()
                        .is_some_and(t1s::PcsReceiver::commit_active);
                observed.receiving_data |= p.carrier
                    && p.t1s_pcs_rx
                        .as_ref()
                        .is_some_and(t1s::PcsReceiver::active_frame);
                observed.queued_packet = p.queue.front().is_some_and(|request| {
                    request.0.at_ps as u128 * exact.denominator <= exact.numerator_ps
                }) && mac_start_allowed;
                if matches!(
                    node.data,
                    plca::DataState::Idle
                        | plca::DataState::Receive
                        | plca::DataState::WaitMac
                        | plca::DataState::WaitIdle
                ) {
                    // Source MAC nibble acceptance, before asserting TX_EN.
                    observed.queued_packet &= clock.mcd();
                }
                observed.transmitting = p
                    .program
                    .as_ref()
                    .is_some_and(|program| program.mac_jam_samples_left.is_none())
                    && node.data != plca::DataState::WaitIdle;
                observed.mac_input_ended = p.program.as_ref().is_some_and(|program| {
                    program
                        .mac_input_end_sample
                        .is_some_and(|end| s.index >= end)
                });
                observed.mac_output_ended = p.program.as_ref().is_some_and(|program| {
                    if program.jam {
                        observed.mac_input_ended
                    } else {
                        s.index
                            >= clock.after_nibbles(
                                program.started,
                                (8 + program.request.0.mac_bytes.len() as u64) * 2,
                            )
                    }
                });
                observed.jamming = p.program.as_ref().is_some_and(|program| program.jam);
                observed.between_mac_clocks = !clock.mcd();
                observed.between_prescient_mac_clocks = !clock.pmcd();
                observed.forwarding_blocked = !p.tx.at_symbol_boundary()
                    || p.tx.pending_symbols() != 0
                    || !p.plca_control_chips.is_empty()
                    || p.plca_beacon_in_flight;
                observed.mac_deferred = !mac_start_allowed
                    || p.carrier && !p.plca_commit_active
                    || p.backoff_samples > 0
                    || p.idle_samples < c.ifg_samples();
                observed.beacon_driving = p.plca_beacon_in_flight;
                let previous_data = node.data;
                let previous_signal_error = node.signal_error;
                let previous_control = node.control;
                let decision = node.step(observed);
                if node.data != previous_data {
                    effects.push(PhyEffect::PcsControl {
                        endpoint: c.endpoints[i].id.clone(),
                        command: format!("plca_data:{:?}", node.data),
                        sample_index: boundary,
                    });
                }
                if node.control != previous_control {
                    effects.push(PhyEffect::PcsControl {
                        endpoint: c.endpoints[i].id.clone(),
                        command: format!("plca_control:{:?}", node.control),
                        sample_index: boundary,
                    });
                }
                if node.signal_error != previous_signal_error {
                    effects.push(PhyEffect::PcsControl {
                        endpoint: c.endpoints[i].id.clone(),
                        command: if node.signal_error {
                            "plca_signal_error_on"
                        } else {
                            "plca_signal_error_off"
                        }
                        .into(),
                        sample_index: boundary,
                    });
                }
                plca_permits[i] = decision.permit_packet;
                plca_carrier_holds[i] = decision.carrier_hold;
                let commit_active = p.program.is_none()
                    && matches!(
                        node.control,
                        plca::ControlState::Burst | plca::ControlState::Commit
                    )
                    && node.committed;
                // Removal stops future groups; tx_cmd_sync retains the group
                // already sampled at MCD in the same chip queue/driver. A
                // genuine physical collision still interrupts it below.
                p.plca_commit_active = commit_active;
                if decision.force_collision {
                    if p.program.is_none() && observed.queued_packet {
                        let request = p.queue.front().unwrap().clone();
                        let origin = Origin {
                            frame_id: request.0.frame_id.clone(),
                            source_endpoint: c.endpoints[i].id.clone(),
                            attempt: p.attempt,
                        };
                        p.program = Some(Program {
                            request,
                            origin: origin.clone(),
                            next_chip: 0,
                            jam: false,
                            started: s.index,
                            sfd_reported: false,
                            fcs_reported: false,
                            t1s_group: 0,
                            t1s_chips: VecDeque::new(),
                            burst_keepalive: false,
                            late_collision: false,
                            mac_input_end_sample: None,
                            mac_jam_samples_left: Some(c.jam_bits as u64 * c.mac_bit_samples()),
                        });
                        effects.push(PhyEffect::PcsControl {
                            endpoint: c.endpoints[i].id.clone(),
                            command: "plca_mac_jam_start".into(),
                            sample_index: s.index,
                        });
                    }
                    if let Some(program) = p.program.as_mut() {
                        if !program.jam {
                            effects.push(PhyEffect::Collision {
                                endpoint: c.endpoints[i].id.clone(),
                                origin: program.origin.clone(),
                                late: false,
                                residual_uv: 0,
                                sample_index: boundary,
                            });
                            program.jam = true;
                            program.mac_input_end_sample =
                                Some(s.index + c.jam_bits as u64 * c.mac_bit_samples());
                            program.next_chip = 0;
                            program.t1s_group = 0;
                            program.t1s_chips.clear();
                            if program.mac_jam_samples_left.is_none() {
                                p.plca_commit_active = false;
                                p.plca_control_chips.clear();
                                if let Some(last_chip) = p.t1s_last_chip {
                                    p.t1s_dme_tx
                                        .as_mut()
                                        .ok_or("missing T1S DME transmitter")?
                                        .resume_after_interruption(last_chip)?;
                                }
                                p.tx.replace_pending_symbols(&[])?;
                            }
                        }
                    }
                }
                if decision.send_beacon {
                    if p.program
                        .as_ref()
                        .is_some_and(|program| program.mac_jam_samples_left.is_none())
                        || p.plca_beacon_in_flight
                    {
                        return Err("PLCA BEACON overlaps a local physical transmission".into());
                    }
                    let tx = p
                        .t1s_dme_tx
                        .as_mut()
                        .ok_or("PLCA BEACON needs a T1S DME transmitter")?;
                    for _ in 0..5 {
                        p.plca_control_chips.extend(tx.group(t1s::BEACON)?);
                    }
                    p.plca_control_chips.extend(tx.group(t1s::SILENCE)?);
                    p.plca_beacon_in_flight = true;
                    p.plca_beacon_waveform_done = false;
                    effects.push(PhyEffect::PcsControl {
                        endpoint: c.endpoints[i].id.clone(),
                        command: "beacon_tx".into(),
                        sample_index: boundary,
                    });
                }
            }
        }
        for (i, p) in s.ports.iter_mut().enumerate() {
            let ready = p.program.is_none()
                && Self::mac_permission_at(c, p, s.index)
                && p.mau.as_ref().is_none_or(|m| m.link_ok() && !m.jabber())
                && !p.plca_beacon_in_flight
                && (!c.half_duplex || !p.carrier || p.plca_commit_active)
                && p.idle_samples >= c.ifg_samples()
                && p.backoff_samples == 0
                && plca_permits[i]
                && p.plca_node.as_ref().is_none_or(|node| {
                    node.data == plca::DataState::Normal || plca::MiiClock::new(p.tx.next_index(), c.mac_bit_samples(), c.endpoints[i].transmitter.clock_ppm).mcd()
                })
                // A MAC start event is the first PHY chip, not an arbitrary
                // sample at which the MAC request became eligible. The idle
                // chip already driving the MDI cannot be replaced mid-UI.
                && (c.physical_mode == PhysicalMode::ProjectNumerical
                    || p.tx.at_symbol_boundary() && p.tx.pending_symbols() == 0)
                && p.plca_control_chips.is_empty()
                && p.queue
                    .front()
                    .is_some_and(|r| r.0.at_ps as u128 * exact.denominator <= exact.numerator_ps);
            if ready {
                p.plca_commit_active = false;
                let request = p.queue.front().unwrap().clone();
                let origin = Origin {
                    frame_id: request.0.frame_id.clone(),
                    source_endpoint: c.endpoints[i].id.clone(),
                    attempt: p.attempt,
                };
                p.program = Some(Program {
                    request,
                    origin: origin.clone(),
                    next_chip: 0,
                    jam: false,
                    started: s.index,
                    sfd_reported: false,
                    fcs_reported: false,
                    t1s_group: 0,
                    t1s_chips: VecDeque::new(),
                    burst_keepalive: p.plca_node.as_ref().is_some_and(|node| {
                        node.enabled
                            && node.config.node_id != 255
                            && node.committed
                            && matches!(
                                node.control,
                                plca::ControlState::Commit | plca::ControlState::Burst
                            )
                            && node.max_burst_count > node.burst_count
                    }),
                    late_collision: false,
                    mac_input_end_sample: (c.physical_mode
                        == PhysicalMode::Ieee10BaseT1sMultidrop2022)
                        .then(|| {
                            let bytes = p.queue.front().unwrap().0.mac_bytes.len() as u64;
                            if let Some(node) = &p.plca_node {
                                if node.data != plca::DataState::Normal {
                                    return plca::MiiClock::new(
                                        p.tx.next_index(),
                                        c.mac_bit_samples(),
                                        c.endpoints[i].transmitter.clock_ppm,
                                    )
                                    .after_nibbles(s.index - node.hold_samples, (8 + bytes) * 2);
                                }
                            }
                            s.index + (8 + bytes) * 8 * c.mac_bit_samples()
                        }),
                    mac_jam_samples_left: None,
                });
                p.idle_samples = 0;
                effects.push(PhyEffect::TxStarted {
                    endpoint: c.endpoints[i].id.clone(),
                    origin: origin.clone(),
                    sample_index: if c.physical_mode.is_named() {
                        s.index
                    } else {
                        boundary
                    },
                });
                if c.physical_mode != PhysicalMode::ProjectNumerical {
                    effects.push(PhyEffect::TxMacPreamble {
                        endpoint: c.endpoints[i].id.clone(),
                        origin,
                        sample_index: s.index,
                    });
                }
            }
        }
        let mut drives = vec![];
        for (i, p) in s.ports.iter_mut().enumerate() {
            let is_repeater_mau = c.repeater.as_ref().is_some_and(|r| {
                r.pairs
                    .iter()
                    .any(|pair| pair.mau_endpoint == c.endpoints[i].id)
            });
            if let Some(left) = p
                .program
                .as_mut()
                .and_then(|program| program.mac_jam_samples_left.as_mut())
            {
                *left = left.saturating_sub(1);
            }
            if p.tx.pending_symbols() == 0 {
                if p.mau.as_ref().is_some_and(mau::Mau::jabber)
                    || is_repeater_mau && p.mau.as_ref().is_some_and(|m| !m.link_ok())
                {
                    // Clause 14 jabber self-interrupt and link-test failure
                    // gate repeater TD data. An active MAC attempt is retired
                    // when jabber trips in the MAU observation pass.
                    p.repeater_chips.clear();
                    p.tx.push_symbols(&[0])?;
                } else if let Some(program) = p
                    .program
                    .as_mut()
                    .filter(|program| program.mac_jam_samples_left.is_none())
                {
                    if program.next_chip < program.len(c) {
                        let chip = if c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022 {
                            program.next_t1s_chip(
                                p.t1s_scrambler.as_mut().ok_or("missing T1S TX scrambler")?,
                                p.t1s_dme_tx.as_mut().ok_or("missing T1S DME transmitter")?,
                            )?
                        } else {
                            let chip = program.chip(program.next_chip, c);
                            program.next_chip += 1;
                            chip
                        };
                        if chip != 0 {
                            p.t1s_last_chip = Some(chip);
                        }
                        p.tx.push_symbols(&[chip])?;
                    }
                } else if is_repeater_mau
                    && !p.repeater_chips.is_empty()
                    && p.tx.at_symbol_boundary()
                {
                    let output = p.repeater_chips.pop_front().unwrap();
                    p.repeater_current_origins = output.origins;
                    p.tx.push_symbols(&[output.level])?;
                } else if p.mau.is_some() && p.tx.at_symbol_boundary() {
                    p.repeater_current_origins.clear();
                    let pulse_or_idle = p.mau.as_mut().unwrap().idle_chip();
                    p.tx.push_symbols(&[pulse_or_idle])?;
                } else if !p.plca_control_chips.is_empty()
                    && p.tx.at_symbol_boundary()
                    && (!p.plca_beacon_in_flight
                        || p.plca_control_chips.len() != 53
                        || plca::MiiClock::new(
                            p.tx.next_index(),
                            c.mac_bit_samples(),
                            c.endpoints[i].transmitter.clock_ppm,
                        )
                        .mcd())
                {
                    let chip = p.plca_control_chips.pop_front().unwrap();
                    if chip != 0 {
                        p.t1s_last_chip = Some(chip);
                    }
                    p.tx.push_symbols(&[chip])?;
                } else if p.plca_commit_active
                    && p.tx.at_symbol_boundary()
                    && plca::MiiClock::new(
                        p.tx.next_index(),
                        c.mac_bit_samples(),
                        c.endpoints[i].transmitter.clock_ppm,
                    )
                    .mcd()
                {
                    let tx = p
                        .t1s_dme_tx
                        .as_mut()
                        .ok_or("PLCA COMMIT needs a T1S DME transmitter")?;
                    p.plca_control_chips.extend(tx.group(t1s::SYNC_COMMIT)?);
                    p.tx.push_symbols(&[p
                        .plca_control_chips
                        .pop_front()
                        .ok_or("missing PLCA COMMIT chip")?])?;
                } else if (!p.plca_beacon_in_flight
                    || p.plca_beacon_waveform_done
                    || p.plca_control_chips.len() == 53)
                    && (c.physical_mode == PhysicalMode::ProjectNumerical
                        || p.tx.at_symbol_boundary())
                {
                    p.repeater_current_origins.clear();
                    p.tx.push_symbols(&[0])?;
                }
            }
            let voltage = p.tx.render_one_in_transaction()?;
            if p.plca_beacon_in_flight && p.plca_control_chips.is_empty() && p.tx.is_idle() {
                p.plca_beacon_waveform_done = true;
            }
            if c.physical_mode != PhysicalMode::ProjectNumerical && p.tx.at_symbol_boundary() {
                if let Some(program) = &mut p.program {
                    let sfd_chips = if c.physical_mode == PhysicalMode::Ieee10BaseT2022 {
                        8 * 16
                    } else {
                        160
                    };
                    let fcs_chips = if c.physical_mode == PhysicalMode::Ieee10BaseT2022 {
                        (8 + program.request.0.mac_bytes.len()) * 16
                    } else {
                        (16 + program.request.0.mac_bytes.len() * 2) * 10
                    };
                    if !program.jam && !program.sfd_reported && program.next_chip == sfd_chips + 1 {
                        program.sfd_reported = true;
                        effects.push(PhyEffect::TxMacSfd {
                            endpoint: c.endpoints[i].id.clone(),
                            origin: program.origin.clone(),
                            sample_index: boundary,
                        });
                    }
                    if !program.jam
                        && !program.fcs_reported
                        // Program may have queued its final chip one UI ahead
                        // of the driver. Only the empty TX queue proves that
                        // the final FCS chip has actually left the MDI.
                        && (c.physical_mode != PhysicalMode::Ieee10BaseT2022
                            || p.tx.pending_symbols() == 0)
                        && program.next_chip
                            == fcs_chips
                                + usize::from(
                                    c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022,
                                )
                    {
                        program.fcs_reported = true;
                        effects.push(PhyEffect::TxMacFcsEnd {
                            endpoint: c.endpoints[i].id.clone(),
                            origin: program.origin.clone(),
                            sample_index: boundary,
                        });
                    }
                }
            }
            drives.push(voltage);
            s.raw_history[i].push_front(voltage);
            s.raw_history[i].truncate(c.history_len(i));
            s.origin_history[i].push_front(
                p.program
                    .as_ref()
                    .filter(|program| program.mac_jam_samples_left.is_none())
                    .map(|v| BTreeSet::from([v.origin.clone()]))
                    .unwrap_or_else(|| p.repeater_current_origins.clone()),
            );
            s.origin_history[i].truncate(c.history_len(i));
        }
        let received = s.channel.process_one_in_transaction(&drives)?;
        for (i, p) in s.ports.iter_mut().enumerate() {
            let endpoint = c.endpoints[i].id.clone();
            let is_repeater_mau = c
                .repeater
                .as_ref()
                .is_some_and(|r| r.pairs.iter().any(|pair| pair.mau_endpoint == endpoint));
            let raw_signal = received[i].unsigned_abs() >= c.carrier_threshold_uv as u64;
            let raw_onset = p.mau.is_some() && raw_signal && !p.mau_raw_active;
            p.mau_raw_active = p.mau.is_some() && raw_signal;
            let signal = if let Some(mau) = &mut p.mau {
                let own_data = p.program.is_some()
                    || is_repeater_mau && !p.repeater_current_origins.is_empty();
                let jabber_before = mau.jabber();
                let observation = mau.observe(own_data, received[i], c.carrier_threshold_uv);
                if !jabber_before && observation.jabber {
                    p.tx.replace_pending_symbols(&[])?;
                    p.repeater_chips.clear();
                    p.repeater_current_origins.clear();
                    if let Some(program) = p.program.take() {
                        effects.push(PhyEffect::RetryExhausted {
                            endpoint: endpoint.clone(),
                            origin: program.origin,
                            sample_index: boundary,
                        });
                        s.terminal.insert(
                            p.queue
                                .pop_front()
                                .ok_or("jabber interrupt without MAC queue head")?
                                .0
                                .frame_id
                                .clone(),
                        );
                        p.attempt = 0;
                        p.backoff_samples = 0;
                        p.idle_samples = 0;
                    }
                }
                observation.packet_carrier
            } else {
                raw_signal
            };
            if signal {
                p.below = 0;
            } else {
                p.below = (p.below + 1).min(c.carrier_hold_samples);
            }
            let present = signal || p.carrier && p.below < c.carrier_hold_samples;
            let onset = present && !p.carrier;
            let ended = !present && p.carrier;
            if c.physical_mode == PhysicalMode::Ieee10BaseT2022 && raw_onset
                || c.physical_mode != PhysicalMode::Ieee10BaseT2022 && onset
            {
                if let Some(token) = p.token.take() {
                    p.rx.abort_frame(&token)?;
                }
                p.decoder = wire::Decoder::new(c.maximum_mac_bytes);
                p.origins.clear();
                p.rx_sfd_sample = None;
                p.rx_fcs_sample = None;
            }
            if present != p.carrier {
                effects.push(PhyEffect::CarrierChanged {
                    endpoint: endpoint.clone(),
                    present,
                    sample_index: boundary,
                });
            }
            p.carrier = present;
            if p.plca_beacon_in_flight && p.plca_beacon_waveform_done && !present {
                p.plca_beacon_in_flight = false;
                p.plca_beacon_waveform_done = false;
            }
            if ended {
                p.plca_received_beacon = false;
            }
            if present {
                for (j, path) in c.channel.paths[i].iter().enumerate() {
                    if let Some(path) = path {
                        for (tap, &gain) in path.taps_q30.iter().enumerate() {
                            let delay = path.delay_samples + tap;
                            if gain != 0 && s.raw_history[j].get(delay).is_some_and(|v| *v != 0) {
                                if let Some(origins) = s.origin_history[j].get(delay) {
                                    observed_origins[i].extend(origins.iter().cloned());
                                    p.origins.extend(origins.iter().cloned());
                                }
                            }
                        }
                    }
                }
            }
            if onset && c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022 {
                if let Some(token) = p.token.take() {
                    p.rx.abort_frame(&token)?;
                }
                p.t1s_dme_rx
                    .as_mut()
                    .ok_or("missing T1S DME receiver")?
                    .carrier_lost();
                p.t1s_pcs_rx = Some(t1s::PcsReceiver::new());
                p.rx.begin_control_acquisition()?;
            }
            let was_locked = p.rx.acquisition() == Acquisition::Locked;
            let decisions =
                p.rx.process_one_in_transaction(s.index, received[i], drives[i])?;
            if c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022
                && was_locked
                && p.rx.acquisition() != Acquisition::Locked
            {
                // Receiver already invalidated its token at the actual loss
                // cut. Drop the matching PCS assembly before decoding more.
                p.token = None;
                p.t1s_dme_rx
                    .as_mut()
                    .ok_or("missing T1S DME receiver")?
                    .carrier_lost();
                p.t1s_pcs_rx = Some(t1s::PcsReceiver::new());
                p.rx_sfd_sample = None;
                p.rx_fcs_sample = None;
                effects.push(PhyEffect::RxAbort {
                    endpoint: endpoint.clone(),
                    reason: "10base_t1s_receiver_lock_lost".into(),
                    sample_index: boundary,
                });
            }
            if is_repeater_mau {
                if let Some(decision) = decisions.last() {
                    p.repeater_recovered_chip = Some(decision.symbol);
                    p.repeater_recovered_origins = if observed_origins[i].is_empty() {
                        p.origins.clone()
                    } else {
                        observed_origins[i].clone()
                    };
                }
            }
            if c.physical_mode == PhysicalMode::Ieee10BaseT2022 && raw_onset {
                p.rx.start_training(&wire::encode_bytes(&[0x55; 8]))?;
            } else if onset && c.physical_mode == PhysicalMode::ProjectNumerical {
                p.rx.start_training(&wire::pilot())?;
            }
            for decision in decisions.into_iter().filter(|_| {
                !onset
                    || c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022
                    || c.physical_mode == PhysicalMode::Ieee10BaseT2022
            }) {
                if (p.carrier || c.physical_mode == PhysicalMode::Ieee10BaseT2022 && raw_signal)
                    && !is_repeater_mau
                    && (!c.half_duplex
                        || p.program
                            .as_ref()
                            .is_none_or(|program| program.mac_jam_samples_left.is_some()))
                {
                    if c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022 {
                        let prefix = p.rx.take_control_prefix();
                        let decoded = if !prefix.is_empty() {
                            let dme = p.t1s_dme_rx.as_mut().ok_or("missing T1S DME receiver")?;
                            dme.carrier_lost();
                            let mut groups = vec![];
                            for chip in prefix {
                                groups.extend(dme.push_chip(chip)?);
                            }
                            Ok(groups)
                        } else if p.rx.trained_symbols() >= 20
                            && !matches!(p.rx.acquisition(), Acquisition::Lost(_))
                        {
                            p.t1s_dme_rx
                                .as_mut()
                                .ok_or("missing T1S DME receiver")?
                                .push_chip(decision.symbol)
                        } else {
                            Ok(vec![])
                        };
                        let groups = match decoded {
                            Ok(groups) => groups,
                            Err(reason) => {
                                if let Some(token) = p.token.take() {
                                    p.rx.abort_frame(&token)?;
                                }
                                p.t1s_dme_rx.as_mut().unwrap().carrier_lost();
                                p.t1s_pcs_rx = Some(t1s::PcsReceiver::new());
                                effects.push(PhyEffect::RxAbort {
                                    endpoint: endpoint.clone(),
                                    reason,
                                    sample_index: boundary,
                                });
                                continue;
                            }
                        };
                        for group in groups {
                            if [t1s::BEACON, t1s::SYNC_COMMIT].contains(&group)
                                && !p.rx.control_group_valid(group)
                            {
                                if let Some(token) = p.token.take() {
                                    if p.rx.active_frame_token() == Some(&token) {
                                        p.rx.abort_frame(&token)?;
                                    }
                                }
                                p.t1s_dme_rx.as_mut().unwrap().carrier_lost();
                                p.t1s_pcs_rx = Some(t1s::PcsReceiver::new());
                                effects.push(PhyEffect::RxAbort {
                                    endpoint: endpoint.clone(),
                                    reason: "10base_t1s_control_analog_rejected".into(),
                                    sample_index: boundary,
                                });
                                break;
                            }
                            let in_data = p
                                .t1s_pcs_rx
                                .as_ref()
                                .ok_or("missing T1S PCS receiver")?
                                .data_state();
                            let effect = p
                                .t1s_pcs_rx
                                .as_mut()
                                .ok_or("missing T1S PCS receiver")?
                                .push(group)?;
                            if in_data && matches!(t1s::decode_group(group)?, t1s::Group::Data(_)) {
                                p.rx_fcs_sample = Some(boundary);
                            }
                            match effect {
                                Some(t1s::PcsRxEffect::Beacon) => {
                                    p.plca_observation.beacon = true;
                                    if p.plca_node.is_some() {
                                        p.plca_received_beacon = true;
                                        p.idle_samples = c.ifg_samples();
                                    }
                                    effects.push(PhyEffect::PcsControl {
                                        endpoint: endpoint.clone(),
                                        command: "beacon".into(),
                                        sample_index: boundary,
                                    })
                                }
                                Some(t1s::PcsRxEffect::Commit) => {
                                    p.plca_observation.commit = true;
                                    p.plca_received_beacon = false;
                                    effects.push(PhyEffect::PcsControl {
                                        endpoint: endpoint.clone(),
                                        command: "commit".into(),
                                        sample_index: boundary,
                                    })
                                }
                                Some(t1s::PcsRxEffect::Start) => {
                                    p.plca_observation.receiving_data = true;
                                    if p.rx.acquisition() == Acquisition::Locked {
                                        let token = p
                                            .rx
                                            .begin_frame(&format!("rx:{endpoint}:{}", boundary))?;
                                        p.token = Some(token.clone());
                                        p.rx_sfd_sample = Some(boundary);
                                        effects.push(PhyEffect::RxMacSfd {
                                            endpoint: endpoint.clone(),
                                            sample_index: boundary,
                                        });
                                        effects.push(PhyEffect::RxStarted {
                                            endpoint: endpoint.clone(),
                                            token,
                                            sample_index: boundary,
                                        });
                                    } else {
                                        effects.push(PhyEffect::RxAbort {
                                            endpoint: endpoint.clone(),
                                            reason: "10base_t1s_unlocked_sfd".into(),
                                            sample_index: boundary,
                                        });
                                    }
                                }
                                Some(t1s::PcsRxEffect::Frame(mac_bytes)) => {
                                    if let Some(token) = p.token.take() {
                                        p.rx.finish_frame(&token)?;
                                        effects.push(PhyEffect::RxFrame {
                                            receiver_endpoint: endpoint.clone(),
                                            mac_bytes,
                                            origins: p.origins.iter().cloned().collect(),
                                            rx_sfd_sample: p
                                                .rx_sfd_sample
                                                .ok_or("T1S frame lacks received SFD sample")?,
                                            rx_fcs_sample: p
                                                .rx_fcs_sample
                                                .ok_or("T1S frame lacks received FCS sample")?,
                                            rx_mapped_sfd_sample: None,
                                            rx_mapped_fcs_sample: None,
                                            sample_index: boundary,
                                        });
                                    } else {
                                        effects.push(PhyEffect::RxAbort {
                                            endpoint: endpoint.clone(),
                                            reason: "10base_t1s_frame_without_locked_sfd".into(),
                                            sample_index: boundary,
                                        });
                                    }
                                }
                                Some(t1s::PcsRxEffect::Abort(reason)) => {
                                    if let Some(token) = p.token.take() {
                                        p.rx.abort_frame(&token)?;
                                    }
                                    effects.push(PhyEffect::RxAbort {
                                        endpoint: endpoint.clone(),
                                        reason,
                                        sample_index: boundary,
                                    });
                                }
                                None => {}
                            }
                        }
                        continue;
                    }
                    match p
                        .decoder
                        .push(decision.symbol, decision.acquisition == Acquisition::Locked)?
                    {
                        Some(wire::DecodeEffect::Start) => {
                            let token = p.rx.begin_frame(&format!("rx:{endpoint}:{}", boundary))?;
                            p.token = Some(token.clone());
                            p.rx_sfd_sample = Some(boundary);
                            effects.push(PhyEffect::RxStarted {
                                endpoint: endpoint.clone(),
                                token,
                                sample_index: boundary,
                            });
                        }
                        Some(wire::DecodeEffect::Abort(reason)) => {
                            if let Some(token) = p.token.take() {
                                p.rx.abort_frame(&token)?;
                            }
                            effects.push(PhyEffect::RxAbort {
                                endpoint: endpoint.clone(),
                                reason: reason.into(),
                                sample_index: boundary,
                            });
                        }
                        None => {}
                    }
                    if p.decoder.collecting() && decision.symbol != 0 {
                        p.rx_fcs_sample = Some(boundary);
                    }
                }
            }
            let events = p.rx.take_events();
            for event in events {
                if is_repeater_mau {
                    continue;
                }
                if matches!(
                    event,
                    ReceiverEvent::FrameAborted { .. } | ReceiverEvent::LockLost { .. }
                ) {
                    p.token = None;
                }
                effects.push(PhyEffect::Receiver {
                    endpoint: endpoint.clone(),
                    event,
                });
            }
            if ended && !is_repeater_mau && (!c.half_duplex || p.program.is_none()) {
                if c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022 {
                    if let Some(t1s::PcsRxEffect::Abort(reason)) = p
                        .t1s_pcs_rx
                        .as_mut()
                        .ok_or("missing T1S PCS receiver")?
                        .carrier_lost()
                    {
                        if let Some(token) = p.token.take() {
                            p.rx.abort_frame(&token)?;
                        }
                        effects.push(PhyEffect::RxAbort {
                            endpoint: endpoint.clone(),
                            reason,
                            sample_index: boundary,
                        });
                    }
                    p.t1s_dme_rx
                        .as_mut()
                        .ok_or("missing T1S DME receiver")?
                        .carrier_lost();
                }
                if c.physical_mode != PhysicalMode::Ieee10BaseT1sMultidrop2022 {
                    if let Some(token) = p.token.take() {
                        match p.decoder.finish().and_then(|bytes| {
                            p.rx.finish_frame(&token)?;
                            Ok(bytes)
                        }) {
                            Ok(mac_bytes) => effects.push(PhyEffect::RxFrame {
                                receiver_endpoint: endpoint.clone(),
                                mac_bytes,
                                origins: p.origins.iter().cloned().collect(),
                                rx_sfd_sample: p
                                    .rx_sfd_sample
                                    .ok_or("sampled MAC frame lacks SFD sample")?,
                                rx_fcs_sample: p
                                    .rx_fcs_sample
                                    .ok_or("sampled MAC frame lacks FCS sample")?,
                                rx_mapped_sfd_sample: None,
                                rx_mapped_fcs_sample: None,
                                sample_index: boundary,
                            }),
                            Err(reason) => {
                                let _ = p.rx.abort_frame(&token);
                                effects.push(PhyEffect::RxAbort {
                                    endpoint: endpoint.clone(),
                                    reason,
                                    sample_index: boundary,
                                });
                            }
                        }
                    }
                }
                for event in p.rx.take_events() {
                    effects.push(PhyEffect::Receiver {
                        endpoint: endpoint.clone(),
                        event,
                    });
                }
            }
            if c.half_duplex {
                let own = if let Some(path) = &c.channel.paths[i][i] {
                    let sum =
                        path.taps_q30
                            .iter()
                            .enumerate()
                            .try_fold(0i128, |sum, (tap, &gain)| {
                                sum.checked_add(
                                    s.raw_history[i]
                                        .get(path.delay_samples + tap)
                                        .copied()
                                        .unwrap_or(0) as i128
                                        * gain as i128,
                                )
                                .ok_or("collision own-path sum overflow")
                            })?;
                    rounded_q30(sum)?.clamp(-c.channel.clip_uv, c.channel.clip_uv)
                } else {
                    0
                };
                let residual = received[i]
                    .checked_sub(own)
                    .ok_or("collision residual overflow")?;
                let physical_col = residual.unsigned_abs() >= c.collision_threshold_uv as u64;
                p.plca_observation.collision = physical_col
                    && p.program
                        .as_ref()
                        .is_some_and(|program| program.mac_jam_samples_left.is_none());
                if physical_col {
                    if let Some(program) = &mut p.program {
                        if !program.jam {
                            let late = boundary - program.started
                                > c.slot_bits as u64 * c.mac_bit_samples();
                            program.late_collision = late;
                            program.mac_input_end_sample =
                                Some(boundary + c.jam_bits as u64 * c.mac_bit_samples());
                            effects.push(PhyEffect::Collision {
                                endpoint: endpoint.clone(),
                                origin: program.origin.clone(),
                                late,
                                residual_uv: residual,
                                sample_index: boundary,
                            });
                            program.jam = true;
                            program.next_chip = 0;
                            program.t1s_group = 0;
                            program.t1s_chips.clear();
                            if c.physical_mode == PhysicalMode::Ieee10BaseT1sMultidrop2022 {
                                if let Some(last_chip) = p.t1s_last_chip {
                                    p.t1s_dme_tx
                                        .as_mut()
                                        .ok_or("missing T1S DME transmitter")?
                                        .resume_after_interruption(last_chip)?;
                                }
                            }
                            p.tx.replace_pending_symbols(&[])?;
                        }
                    }
                }
            }
            let finished =
                p.program
                    .as_ref()
                    .is_some_and(|program| match program.mac_jam_samples_left {
                        Some(left) => left == 0,
                        None => program.next_chip == program.len(c) && p.tx.is_idle(),
                    });
            if finished {
                let program = p.program.take().unwrap();
                if program.mac_jam_samples_left.is_some() {
                    effects.push(PhyEffect::PcsControl {
                        endpoint: endpoint.clone(),
                        command: "plca_mac_jam_end".into(),
                        sample_index: boundary,
                    });
                } else if c.physical_mode != PhysicalMode::ProjectNumerical {
                    effects.push(PhyEffect::TxCodedEnd {
                        endpoint: endpoint.clone(),
                        origin: program.origin.clone(),
                        sample_index: boundary,
                    });
                }
                p.idle_samples = 0;
                if program.jam {
                    let late = program.late_collision;
                    if p.attempt >= c.retry_limit
                        || c.physical_mode != PhysicalMode::ProjectNumerical
                            && p.attempt + 1 >= c.retry_limit
                        || c.physical_mode != PhysicalMode::ProjectNumerical && late
                    {
                        effects.push(PhyEffect::RetryExhausted {
                            endpoint: endpoint.clone(),
                            origin: program.origin,
                            sample_index: boundary,
                        });
                        s.terminal.insert(
                            p.queue
                                .pop_front()
                                .ok_or("missing failed PHY queue head")?
                                .0
                                .frame_id
                                .clone(),
                        );
                        p.attempt = 0;
                        p.backoff_samples = 0;
                    } else {
                        p.attempt += 1;
                        let slots = backoff(c.seed, &program.origin, p.attempt);
                        p.backoff_samples = slots * c.slot_bits as u64 * c.mac_bit_samples();
                        effects.push(PhyEffect::Retry {
                            endpoint: endpoint.clone(),
                            origin: program.origin,
                            slots,
                            sample_index: boundary,
                        });
                    }
                } else {
                    effects.push(PhyEffect::TxFinished {
                        endpoint: endpoint.clone(),
                        origin: program.origin,
                        sample_index: boundary,
                    });
                    s.terminal.insert(
                        p.queue
                            .pop_front()
                            .ok_or("missing completed PHY queue head")?
                            .0
                            .frame_id
                            .clone(),
                    );
                    p.attempt = 0;
                }
            }
            if p.program.is_none() {
                if c.physical_mode != PhysicalMode::ProjectNumerical {
                    // Clause 4 BEB counts wall-clock bit times after jam,
                    // including intervals when another MAC owns carrier.
                    p.backoff_samples = p.backoff_samples.saturating_sub(1);
                }
                let pending_carrier = p.plca_node.as_ref().is_some_and(|node| {
                    matches!(
                        node.data,
                        plca::DataState::Collide
                            | plca::DataState::DelayPending
                            | plca::DataState::Pending
                    )
                });
                if pending_carrier {
                    p.idle_samples = 0;
                } else if !c.half_duplex || !p.carrier || p.plca_commit_active {
                    if (finished || c.half_duplex && ended) && c.ifg_samples() > 0 {
                        // The just-observed interval contained the final TX
                        // chip/carrier. IFG begins at its consumed boundary.
                    } else if p.idle_samples < c.ifg_samples() {
                        p.idle_samples += 1;
                    } else if c.physical_mode == PhysicalMode::ProjectNumerical {
                        p.backoff_samples = p.backoff_samples.saturating_sub(1);
                    }
                } else if !p.plca_beacon_in_flight && !p.plca_received_beacon {
                    p.idle_samples = 0;
                }
            }
        }
        for (i, p) in s.ports.iter().enumerate() {
            if c.repeater.as_ref().is_some_and(|r| {
                r.pairs
                    .iter()
                    .any(|pair| pair.mau_endpoint == c.endpoints[i].id)
            }) {
                continue;
            }
            let ready = p.program.is_none()
                && !plca_carrier_holds[i]
                && p.mau.as_ref().is_none_or(|m| m.link_ok() && !m.jabber())
                && !p.plca_beacon_in_flight
                && p.plca_control_chips.is_empty()
                && (!c.half_duplex || !p.carrier || p.plca_commit_active)
                && p.idle_samples >= c.ifg_samples()
                && p.backoff_samples == 0;
            if ready != prior_ready[i] {
                effects.push(PhyEffect::TxReady {
                    endpoint: c.endpoints[i].id.clone(),
                    ready,
                    sample_index: boundary,
                });
            }
        }
        if let Some(config) = &c.repeater {
            if s.index % c.channel.grid.samples_per_ui as u64
                == c.channel.grid.samples_per_ui as u64 - 1
            {
                let mut observations = Vec::with_capacity(config.pairs.len());
                let mut mau_indices = Vec::with_capacity(config.pairs.len());
                for pair in &config.pairs {
                    let index = c
                        .endpoints
                        .iter()
                        .position(|e| e.id == pair.mau_endpoint)
                        .ok_or("repeater MAU endpoint absent")?;
                    mau_indices.push(index);
                    let active = s.ports[index].carrier;
                    let chip = s.ports[index]
                        .repeater_recovered_chip
                        .take()
                        .filter(|chip| *chip != 0);
                    let origins = std::mem::take(&mut s.ports[index].repeater_recovered_origins);
                    observations.push(repeater::PortObservation {
                        chip,
                        data_in: active,
                        collision: active
                            && drives[index].unsigned_abs() >= c.carrier_threshold_uv as u64,
                        link_ok: s.ports[index].mau.as_ref().is_none_or(mau::Mau::link_ok),
                        origins,
                    });
                }
                let step = s
                    .repeater
                    .as_mut()
                    .ok_or("missing repeater state")?
                    .step(&observations)?;
                for event in step.events {
                    effects.push(PhyEffect::Repeater {
                        event,
                        sample_index: boundary,
                    });
                }
                for (port, output) in step.outputs.into_iter().enumerate() {
                    if let Some(chip) = output {
                        let mau_port = &mut s.ports[mau_indices[port]];
                        if mau_port
                            .mau
                            .as_ref()
                            .is_some_and(|m| m.link_ok() && !m.jabber())
                        {
                            mau_port.repeater_chips.push_back(chip);
                        }
                    }
                }
            }
        }
        if c.trace_samples {
            effects.push(PhyEffect::Samples {
                sample_index: s.index,
                transmitted_uv: drives,
                received_uv: received,
            });
        }
        s.index = boundary;
        Ok(effects)
    }
}
fn backoff(seed: u64, origin: &Origin, attempt: u32) -> u64 {
    let mut value = seed ^ 0xcbf29ce484222325;
    for field in [
        origin.source_endpoint.as_bytes(),
        origin.frame_id.as_bytes(),
    ] {
        for byte in (field.len() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(field.iter().copied())
        {
            value = (value ^ byte as u64).wrapping_mul(0x100000001b3);
        }
    }
    for byte in attempt.to_le_bytes() {
        value = (value ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    value ^= value >> 31;
    value & ((1u64 << attempt.min(10)) - 1)
}

#[cfg(test)]
mod observed_origin_tests {
    use super::*;
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct SavedOrigins {
        #[serde(
            serialize_with = "serialize_observed_origins",
            deserialize_with = "deserialize_observed_origins"
        )]
        spans: BTreeMap<Origin, (u64, u64)>,
    }
    #[test]
    fn nonempty_observed_origin_intervals_checkpoint_roundtrip_rejects_duplicate_keys() {
        let origin = Origin {
            frame_id: "actual-physical-attempt".into(),
            source_endpoint: "p0".into(),
            attempt: 0,
        };
        let saved = SavedOrigins {
            spans: BTreeMap::from([(origin, (14, 312))]),
        };
        let json = serde_json::to_vec(&saved).unwrap();
        assert_eq!(
            serde_json::from_slice::<SavedOrigins>(&json).unwrap(),
            saved
        );
        let mut duplicate: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let entries = duplicate["spans"].as_array_mut().unwrap();
        entries.push(entries[0].clone());
        assert!(serde_json::from_value::<SavedOrigins>(duplicate).is_err());
    }
}
