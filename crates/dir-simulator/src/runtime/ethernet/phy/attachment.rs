//! Byte-derived network seam. Physical TX retirement and actual RX delivery are
//! separate; a planned EOF can never satisfy a received-frame completion here.
use super::{medium::*, *};
use crate::types::ethernet::{EthernetTag, EthernetVlanTag, EthernetWireFrame};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortBinding {
    pub direction_id: String,
    pub from_port: String,
    pub to_port: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalPort {
    /// Electrical-domain identity, equal to a MediumConfig endpoint ID.
    pub endpoint: String,
    /// Network graph port at which this station's MAC submits an egress.
    pub tx_port: String,
    /// Network graph port at which this station receives decoded ingress.
    pub rx_port: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentConfig {
    pub origin_ps: u64,
    pub medium: MediumConfig,
    pub bindings: Vec<PortBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub physical_ports: Vec<PhysicalPort>,
    /// Exact sampled lookahead bound for the named single-FES adapter.
    /// Omission selects 4096; a value changes batching cost, not semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookahead_samples: Option<u32>,
}
impl AttachmentConfig {
    pub fn lookahead_samples(&self) -> u32 {
        self.lookahead_samples.unwrap_or(4096)
    }
    pub fn validate(&self) -> Result<()> {
        self.medium.validate()?;
        if self.medium.physical_mode.is_named() {
            if self
                .lookahead_samples
                .is_some_and(|samples| !(1..=65_536).contains(&samples))
            {
                return Err("named PHY lookahead_samples must be in1..65536".into());
            }
            let endpoint_ids: BTreeSet<_> = if let Some(repeater) = &self.medium.repeater {
                repeater
                    .pairs
                    .iter()
                    .map(|p| p.mac_endpoint.as_str())
                    .collect()
            } else {
                self.medium
                    .endpoints
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect()
            };
            let physical_ids: BTreeSet<_> = self
                .physical_ports
                .iter()
                .map(|p| p.endpoint.as_str())
                .collect();
            let tx_ports: BTreeSet<_> = self
                .physical_ports
                .iter()
                .map(|p| p.tx_port.as_str())
                .collect();
            let rx_ports: BTreeSet<_> = self
                .physical_ports
                .iter()
                .map(|p| p.rx_port.as_str())
                .collect();
            if !self.bindings.is_empty()
                || self.physical_ports.len() != endpoint_ids.len()
                || endpoint_ids != physical_ids
                || tx_ports.len() != self.physical_ports.len()
                || rx_ports.len() != self.physical_ports.len()
                || self.physical_ports.iter().any(|p| {
                    p.tx_port.is_empty()
                        || p.rx_port.is_empty()
                        || p.tx_port.len() > 256
                        || p.rx_port.len() > 256
                })
            {
                return Err("named shared PHY requires one unique electrical/tx/rx physical port per endpoint and no legacy directed bindings".into());
            }
            self.origin_ps
                .checked_add(sample_boundary_ps(
                    self.medium.channel.grid,
                    self.medium.maximum_samples,
                )?)
                .ok_or("PHY attachment global sample time overflow")?;
            return Ok(());
        }
        if !self.physical_ports.is_empty() || self.lookahead_samples.is_some() {
            return Err(
                "legacy PHY attachment cannot declare physical_ports or lookahead_samples".into(),
            );
        }
        let mut ids = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        if self.bindings.is_empty() || self.bindings.len() > 256 {
            return Err("PHY attachment needs1..256 directed bindings".into());
        }
        for binding in &self.bindings {
            let from = self
                .medium
                .endpoints
                .iter()
                .position(|e| e.id == binding.from_port)
                .ok_or("PHY binding source port absent")?;
            let to = self
                .medium
                .endpoints
                .iter()
                .position(|e| e.id == binding.to_port)
                .ok_or("PHY binding sink port absent")?;
            if binding.direction_id.is_empty()
                || binding.direction_id.len() > 256
                || !ids.insert(&binding.direction_id)
                || from == to
                || !pairs.insert((from, to))
                || self.medium.channel.paths[to][from].is_none()
            {
                return Err("duplicate/invalid/noncausal PHY direction binding".into());
            }
        }
        self.origin_ps
            .checked_add(sample_boundary_ps(
                self.medium.channel.grid,
                self.medium.maximum_samples,
            )?)
            .ok_or("PHY attachment global sample time overflow")?;
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rtag {
    pub reserved: u16,
    pub sequence: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodedMac {
    pub wire: EthernetWireFrame,
    pub rtag: Option<Rtag>,
}
pub fn sample_boundary_ps(grid: SampleGrid, index: u64) -> Result<u64> {
    let t = grid.time(index)?;
    u64::try_from(t.numerator_ps.div_ceil(t.denominator))
        .map_err(|_| "PHY boundary time overflow".into())
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(result, "{b:02x}").expect("String write");
    }
    result
}
fn unhex(input: &str) -> Result<Vec<u8>> {
    if input.len() % 2 != 0 || !input.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid canonical MAC hex".into());
    }
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|s| {
            let s = std::str::from_utf8(s).map_err(|_| "invalid hex text")?;
            u8::from_str_radix(s, 16).map_err(|_| "invalid hex digits".into())
        })
        .collect()
}
fn address(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
/// Ethernet-II carries no payload length, so its padding remains part of
/// `data_hex`. IEEE 802.3 length fields expose the LLC payload boundary; keep
/// those padding bytes in `mac_hex` and their count in `pad_bytes`.
pub fn decode_mac(bytes: &[u8], maximum: usize) -> Result<DecodedMac> {
    wire::validate_mac(bytes, maximum)?;
    let end = bytes.len() - 4;
    let mut offset = 12;
    if bytes[..6].iter().all(|&b| b == 0)
        || bytes[6..12].iter().all(|&b| b == 0)
        || bytes[6] & 1 != 0
    {
        return Err("invalid received MAC addresses".into());
    }
    let mut ether_type = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
    offset += 2;
    let mut tags = Vec::new();
    while [EthernetTag::C_TPID, EthernetTag::S_TPID].contains(&ether_type) {
        if tags.len() == 2 || offset + 4 > end {
            return Err("truncated or excessive received VLAN tags".into());
        }
        let tci = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        let parsed = EthernetTag {
            tpid: ether_type,
            tci: EthernetVlanTag {
                vid: tci & 0xfff,
                pcp: (tci >> 13) as u8,
                dei: ((tci >> 12) & 1) as u8,
            },
        };
        parsed.validate().map_err(|e| e.to_owned())?;
        tags.push(parsed);
        ether_type = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]);
        offset += 4;
    }
    if tags.len() == 2
        && (tags[0].tpid != EthernetTag::S_TPID || tags[1].tpid != EthernetTag::C_TPID)
    {
        return Err("invalid received VLAN tag order".into());
    }
    let rtag = if ether_type == 0xf1c1 {
        if offset + 6 > end {
            return Err("truncated received R-TAG".into());
        }
        let reserved = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        let sequence = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]);
        offset += 4;
        ether_type = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        offset += 2;
        Some(Rtag { reserved, sequence })
    } else {
        None
    };
    if (1501..1536).contains(&ether_type) || [0x8100, 0x88a8, 0xf1c1].contains(&ether_type) {
        return Err("invalid received Ethernet length/type or tag nesting".into());
    }
    let (payload_end, pad_bytes) = if ether_type <= 1500 {
        let payload_end = offset
            .checked_add(ether_type as usize)
            .ok_or("received LLC length overflow")?;
        if payload_end > end {
            return Err("received LLC length exceeds MAC payload".into());
        }
        (payload_end, (end - payload_end) as u64)
    } else {
        (end, 0)
    };
    Ok(DecodedMac {
        wire: EthernetWireFrame {
            src_mac: address(&bytes[6..12]),
            dst_mac: address(&bytes[..6]),
            ether_type,
            data_hex: hex(&bytes[offset..payload_end]),
            tag: (tags.len() == 1 && tags[0].tpid == EthernetTag::C_TPID).then(|| tags[0].tci),
            tags: if tags.len() == 1 && tags[0].tpid == EthernetTag::C_TPID {
                vec![]
            } else {
                tags
            },
            pad_bytes,
            mac_bytes: bytes.len() as u64,
            fcs_hex: hex(&bytes[end..]),
            mac_hex: hex(bytes),
        },
        rtag,
    })
}
/// Validate the source's canonical bytes and metadata before ordinary admission.
pub fn canonical_bytes(frame: &EthernetWireFrame, maximum: usize) -> Result<Vec<u8>> {
    let bytes = unhex(&frame.mac_hex)?;
    let parsed = decode_mac(&bytes, maximum)?;
    let payload = unhex(&frame.data_hex)?;
    let actual = unhex(&parsed.wire.data_hex)?;
    let padding = usize::try_from(frame.pad_bytes).map_err(|_| "padding overflow")?;
    let payload_ok = if parsed.wire.ether_type <= 1500 {
        payload == actual && frame.pad_bytes == parsed.wire.pad_bytes
    } else {
        payload.len().checked_add(padding) == Some(actual.len())
            && actual.starts_with(&payload)
            && actual[payload.len()..].iter().all(|&b| b == 0)
    };
    if bytes.len() as u64 != frame.mac_bytes
        || parsed.wire.src_mac != frame.src_mac.to_ascii_lowercase()
        || parsed.wire.dst_mac != frame.dst_mac.to_ascii_lowercase()
        || parsed.wire.ether_type != frame.ether_type
        || parsed.wire.canonical_tags() != frame.canonical_tags()
        || parsed.wire.fcs_hex != frame.fcs_hex.to_ascii_lowercase()
        || !payload_ok
    {
        return Err("canonical MAC byte/metadata mismatch".into());
    }
    Ok(bytes)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionIdentity {
    pub transfer_id: String,
    pub direction_id: String,
    pub mac_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    identity: AdmissionIdentity,
    serialized: bool,
    tx_released: bool,
    tx_failure: Option<String>,
    received: u32,
    failure_emitted: bool,
}
const MAX_PHYSICAL_INGRESS_PROOFS: usize = 65_536;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalIngressProof {
    pub admitted_transfer_id: String,
    pub physical_attempt: Origin,
    pub source_endpoint: String,
    pub receiver_endpoint: String,
    pub receiver_tx_port: String,
    pub receiver_rx_port: String,
    pub reception_index: u32,
    pub mac_hex: String,
    pub rx_sfd_sample: u64,
    pub rx_fcs_sample: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_mapped_sfd_sample: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_mapped_fcs_sample: Option<u64>,
    pub completed_sample: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedAdmissionRecord {
    admitted_at_sample: u64,
    request: MacTx,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum AttachmentEffect {
    Physical {
        effect: PhyEffect,
    },
    TxSerialized {
        transfer_id: String,
        direction_id: String,
        success: bool,
        reason: Option<String>,
    },
    TxReleased {
        transfer_id: String,
        direction_id: String,
        success: bool,
        reason: Option<String>,
    },
    Received {
        transfer_id: String,
        direction_id: String,
        to_port: String,
        reception_index: u32,
        frame: DecodedMac,
        origins: Vec<Origin>,
    },
    /// One decoded observation of a physical attempt on a shared electrical
    /// segment. A source transmission has a single admitted transfer but may
    /// produce this effect once per receiving physical endpoint.
    PhysicalIngress {
        admitted_transfer_id: String,
        physical_attempt: Origin,
        source_endpoint: String,
        receiver_endpoint: String,
        receiver_tx_port: String,
        receiver_rx_port: String,
        reception_index: u32,
        frame: DecodedMac,
        origins: Vec<Origin>,
        rx_sfd_sample: u64,
        rx_fcs_sample: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rx_mapped_sfd_sample: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rx_mapped_fcs_sample: Option<u64>,
    },
    Failed {
        transfer_id: String,
        direction_id: String,
        to_port: String,
        reason: String,
    },
    UnattributedReceive {
        to_port: String,
        frame: DecodedMac,
        origins: Vec<Origin>,
        reason: String,
    },
    DecodeRejected {
        to_port: String,
        mac_bytes: Vec<u8>,
        origins: Vec<Origin>,
        reason: String,
    },
}
#[derive(Clone, Debug)]
pub struct AttachedMedium {
    config: Arc<AttachmentConfig>,
    medium: SampledMedium,
    admissions: BTreeMap<String, Admission>,
    named_admissions: Arc<Vec<NamedAdmissionRecord>>,
    physical_ingress_history: Arc<Vec<PhysicalIngressProof>>,
}
/// A read-only scalar lookahead to the first externally visible attachment
/// effect, or to the supplied horizon when no effect occurs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PredictedEffect {
    pub samples: u64,
    pub end_sample: u64,
    pub effects_present: bool,
}
/// Exact speculative prefix. The caller may install `end_state` only at the
/// recorded cut if no external input has changed the base state meanwhile.
#[derive(Clone, Debug)]
pub struct AttachmentForecast {
    pub samples: u64,
    pub end_sample: u64,
    pub effects: Vec<AttachmentEffect>,
    pub end_state: AttachedMedium,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    config: AttachmentConfig,
    medium: Vec<u8>,
    admissions: BTreeMap<String, Admission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    named_admissions: Vec<NamedAdmissionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    physical_ingress_history: Vec<PhysicalIngressProof>,
}
impl AttachedMedium {
    pub fn new(config: AttachmentConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            medium: SampledMedium::new(config.medium.clone())?,
            config: Arc::new(config),
            admissions: BTreeMap::new(),
            named_admissions: Arc::new(vec![]),
            physical_ingress_history: Arc::new(vec![]),
        })
    }
    pub fn config(&self) -> &AttachmentConfig {
        &self.config
    }
    pub fn next_sample_index(&self) -> u64 {
        self.medium.next_sample_index()
    }
    pub fn next_event_ps(&self) -> Result<u64> {
        self.config
            .origin_ps
            .checked_add(self.medium.next_event_ps()?)
            .ok_or_else(|| "attached PHY global time overflow".into())
    }
    pub fn sample_time_ps(&self, index: u64) -> Result<u64> {
        self.config
            .origin_ps
            .checked_add(sample_boundary_ps(self.config.medium.channel.grid, index)?)
            .ok_or_else(|| "attached PHY global sample time overflow".into())
    }
    pub fn tx_ready(&self, from_port: &str) -> Result<bool> {
        let endpoint = if self.config.medium.physical_mode.is_named() {
            self.config
                .physical_ports
                .iter()
                .find(|p| p.tx_port == from_port)
                .map(|p| p.endpoint.as_str())
                .ok_or("unattached named PHY source tx port")?
        } else {
            from_port
        };
        self.medium.tx_ready(endpoint)
    }
    pub fn gigabit_bilateral_link_ready(&self) -> Result<bool> {
        self.medium.gigabit_bilateral_link_ready()
    }
    pub fn set_mac_start_permission(
        &mut self,
        from_port: &str,
        allowed_until_global_ps: Option<u64>,
    ) -> Result<()> {
        let endpoint = if self.config.medium.physical_mode.is_named() {
            self.config
                .physical_ports
                .iter()
                .find(|p| p.tx_port == from_port)
                .map(|p| p.endpoint.as_str())
                .ok_or("unattached named PHY source tx port")?
        } else {
            from_port
        };
        let local = match allowed_until_global_ps {
            None => None,
            Some(u64::MAX) => Some(u64::MAX),
            Some(deadline) => deadline.checked_sub(self.config.origin_ps),
        };
        self.medium.set_mac_start_permission(endpoint, local)
    }
    pub fn mac_start_permission(&self, from_port: &str) -> Result<Option<u64>> {
        let endpoint = if self.config.medium.physical_mode.is_named() {
            self.config
                .physical_ports
                .iter()
                .find(|p| p.tx_port == from_port)
                .map(|p| p.endpoint.as_str())
                .ok_or("unattached named PHY source tx port")?
        } else {
            from_port
        };
        match self.medium.mac_start_permission(endpoint)? {
            None => Ok(None),
            Some(u64::MAX) => Ok(Some(u64::MAX)),
            Some(local) => self
                .config
                .origin_ps
                .checked_add(local)
                .map(Some)
                .ok_or_else(|| "attached PHY MAC permission global time overflow".into()),
        }
    }
    /// Apply a bilateral named 1000BASE-T1 MDI outage from the next sample.
    /// Network link control must split its FES advance at this exact cut.
    pub fn set_gigabit_mdi_disconnected(&mut self, disconnected: bool) -> Result<()> {
        self.medium.set_gigabit_mdi_disconnected(disconnected)
    }
    fn raw_endpoint(&self, from_port: &str) -> Result<&str> {
        if !self.config.medium.gigabit_raw_merge {
            return Err("attached raw Merge service is disabled".into());
        }
        self.config
            .physical_ports
            .iter()
            .find(|p| p.tx_port == from_port)
            .map(|p| p.endpoint.as_str())
            .ok_or("unattached raw Merge tx port".into())
    }
    pub fn raw_timing_certificate(&self) -> Result<Option<RawTimingCertificate>> {
        let Some(mut certificate) = self.medium.raw_timing_certificate()? else {
            return Ok(None);
        };
        certificate.attachment_config_sha256 = Some(hex(&Sha256::digest(
            serde_json::to_vec(self.config.as_ref())
                .map_err(|_| "raw timing attachment serialization failed")?,
        )));
        certificate.attachment_origin_ps = self.config.origin_ps;
        Ok(Some(certificate))
    }
    pub fn raw_ingress_status(&self, from_port: &str) -> Result<RawIngressStatus> {
        self.medium
            .raw_ingress_status(self.raw_endpoint(from_port)?)
    }
    pub fn admit_raw_mpacket(
        &mut self,
        from_port: &str,
        mut request: RawMPacketTx,
    ) -> Result<AttachmentEffect> {
        let endpoint = self.raw_endpoint(from_port)?.to_string();
        if request.source_endpoint != endpoint {
            return Err("raw Merge source endpoint mismatch".into());
        }
        request.at_ps = request
            .at_ps
            .checked_sub(self.config.origin_ps)
            .ok_or("raw Merge admission before attached origin")?;
        let effect = self.medium.admit_raw_mpacket(request)?;
        Ok(AttachmentEffect::Physical { effect })
    }
    pub fn replace_unconsumed_raw_suffix(
        &mut self,
        from_port: &str,
        packet_id: u64,
        expected_generation: u64,
        from_packet_octet: usize,
        suffix: &[u8],
        new_generation: u64,
    ) -> Result<()> {
        let endpoint = self.raw_endpoint(from_port)?.to_string();
        self.medium.replace_unconsumed_raw_suffix(
            &endpoint,
            packet_id,
            expected_generation,
            from_packet_octet,
            suffix,
            new_generation,
        )
    }
    pub fn admission_identities(&self) -> Vec<AdmissionIdentity> {
        self.admissions
            .values()
            .map(|a| a.identity.clone())
            .collect()
    }
    /// Actual decoded ingress observations accumulated by the shared electrical
    /// medium. These are retained through checkpoints for a causal network audit.
    pub fn physical_ingress_history(&self) -> &[PhysicalIngressProof] {
        self.physical_ingress_history.as_slice()
    }
    pub fn has_pending(&self) -> bool {
        self.medium.has_pending()
    }
    pub fn identity(
        transfer_id: &str,
        direction_id: &str,
        frame: &EthernetWireFrame,
        maximum: usize,
    ) -> Result<AdmissionIdentity> {
        Ok(AdmissionIdentity {
            transfer_id: transfer_id.into(),
            direction_id: direction_id.into(),
            mac_sha256: hex(&Sha256::digest(canonical_bytes(frame, maximum)?)),
        })
    }
    pub fn admit(
        &mut self,
        direction_id: &str,
        transfer_id: &str,
        at_global_ps: u64,
        frame: &EthernetWireFrame,
    ) -> Result<()> {
        if at_global_ps > self.next_event_ps()? {
            return Err("attached PHY sample clock is behind admission; advance its actual idle samples first".into());
        }
        let source_endpoint = if self.config.medium.physical_mode.is_named() {
            self.config
                .physical_ports
                .iter()
                .find(|p| p.tx_port == direction_id)
                .map(|p| p.endpoint.clone())
                .ok_or("unattached named PHY source tx port")?
        } else {
            self.config
                .bindings
                .iter()
                .find(|b| b.direction_id == direction_id)
                .map(|b| b.from_port.clone())
                .ok_or("unattached Ethernet direction")?
        };
        let bytes = canonical_bytes(frame, self.config.medium.maximum_mac_bytes)?;
        let identity = AdmissionIdentity {
            transfer_id: transfer_id.into(),
            direction_id: direction_id.into(),
            mac_sha256: hex(&Sha256::digest(&bytes)),
        };
        let request = MacTx {
            frame_id: transfer_id.into(),
            source_endpoint,
            at_ps: at_global_ps
                .checked_sub(self.config.origin_ps)
                .ok_or("admission before PHY sample origin")?,
            mac_bytes: bytes,
        };
        self.medium.admit(request.clone())?;
        if self.config.medium.physical_mode.is_named() {
            Arc::make_mut(&mut self.named_admissions).push(NamedAdmissionRecord {
                admitted_at_sample: self.medium.next_sample_index(),
                request,
            });
        }
        self.admissions.insert(
            transfer_id.into(),
            Admission {
                identity,
                serialized: false,
                tx_released: false,
                tx_failure: None,
                received: 0,
                failure_emitted: false,
            },
        );
        Ok(())
    }
    /// Compare to the network transfer ledger on checkpoint restore. This is an
    /// identity/hash bijection, never a receiver-payload comparison.
    pub fn validate_external_admissions(&self, external: &[AdmissionIdentity]) -> Result<()> {
        self.validate()?;
        let map: BTreeMap<_, _> = external.iter().map(|x| (&x.transfer_id, x)).collect();
        if map.len() != external.len()
            || map.len() != self.admissions.len()
            || self
                .admissions
                .iter()
                .any(|(id, a)| map.get(id).is_none_or(|x| **x != a.identity))
        {
            return Err("network transfer/PHY admission ledger mismatch".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        self.medium.validate()?;
        if self.medium.admitted_frame_ids()
            != &self.admissions.keys().cloned().collect::<BTreeSet<_>>()
        {
            return Err("PHY medium/admission identities are not bijective".into());
        }
        for (id, a) in &self.admissions {
            if id != &a.identity.transfer_id
                || !(self
                    .config
                    .bindings
                    .iter()
                    .any(|b| b.direction_id == a.identity.direction_id)
                    || self
                        .config
                        .physical_ports
                        .iter()
                        .any(|p| p.tx_port == a.identity.direction_id))
                || a.identity.mac_sha256.len() != 64
                || !a.identity.mac_sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || a.tx_failure.is_some() && !a.serialized
                || a.tx_released && !a.serialized
                || a.failure_emitted && (!a.tx_released || a.received != 0)
            {
                return Err("invalid saved PHY attachment admission".into());
            }
        }
        for pending in self.medium.pending_admissions() {
            let a = self
                .admissions
                .get(&pending.frame_id)
                .ok_or("missing pending PHY admission")?;
            let source = if self.config.medium.physical_mode.is_named() {
                self.config
                    .physical_ports
                    .iter()
                    .find(|p| p.tx_port == a.identity.direction_id)
                    .map(|p| p.endpoint.as_str())
            } else {
                self.config
                    .bindings
                    .iter()
                    .find(|b| b.direction_id == a.identity.direction_id)
                    .map(|b| b.from_port.as_str())
            }
            .ok_or("missing PHY binding")?;
            if source != pending.source_endpoint
                || a.identity.mac_sha256 != hex(&Sha256::digest(&pending.mac_bytes))
                || a.tx_released
            {
                return Err("saved PHY pending source/byte hash mismatch".into());
            }
        }
        let named = self.config.medium.physical_mode.is_named();
        if !named
            && (!self.named_admissions.is_empty() || !self.physical_ingress_history.is_empty())
            || named
                && (self.named_admissions.len() != self.admissions.len()
                    || self.physical_ingress_history.len() > MAX_PHYSICAL_INGRESS_PROOFS)
        {
            return Err("invalid named physical proof ledger size".into());
        }
        let mut previous_admission_sample = 0;
        let mut recorded = BTreeSet::new();
        for record in self.named_admissions.iter() {
            let admission = self
                .admissions
                .get(&record.request.frame_id)
                .ok_or("physical proof admission absent from ledger")?;
            if record.admitted_at_sample < previous_admission_sample
                || record.admitted_at_sample > self.medium.next_sample_index()
                || !recorded.insert(&record.request.frame_id)
                || admission.identity.mac_sha256 != hex(&Sha256::digest(&record.request.mac_bytes))
                || self
                    .config
                    .physical_ports
                    .iter()
                    .find(|p| p.tx_port == admission.identity.direction_id)
                    .is_none_or(|p| p.endpoint != record.request.source_endpoint)
            {
                return Err("invalid named physical proof admission".into());
            }
            previous_admission_sample = record.admitted_at_sample;
        }
        let mut prior_completed = 0;
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for proof in self.physical_ingress_history.iter() {
            let admission = self
                .admissions
                .get(&proof.admitted_transfer_id)
                .ok_or("physical ingress proof lacks admission")?;
            let receiver = self
                .config
                .physical_ports
                .iter()
                .find(|p| p.endpoint == proof.receiver_endpoint)
                .ok_or("physical ingress proof lacks receiver")?;
            let source = self
                .config
                .physical_ports
                .iter()
                .find(|p| p.tx_port == admission.identity.direction_id)
                .ok_or("physical ingress proof lacks source")?;
            let next = counts.entry(&proof.admitted_transfer_id).or_default();
            if proof.physical_attempt.frame_id != proof.admitted_transfer_id
                || proof.physical_attempt.source_endpoint != proof.source_endpoint
                || proof.source_endpoint != source.endpoint
                || receiver.endpoint == source.endpoint
                || receiver.tx_port != proof.receiver_tx_port
                || receiver.rx_port != proof.receiver_rx_port
                || proof.reception_index != *next
                || proof.completed_sample < prior_completed
                || proof.completed_sample > self.medium.next_sample_index()
                || proof.rx_sfd_sample >= proof.rx_fcs_sample
                || proof.rx_fcs_sample > proof.completed_sample
                || proof.rx_mapped_sfd_sample.is_some() != proof.rx_mapped_fcs_sample.is_some()
                || proof
                    .rx_mapped_sfd_sample
                    .zip(proof.rx_mapped_fcs_sample)
                    .is_some_and(|(sfd, fcs)| sfd >= fcs || fcs > proof.completed_sample)
                || proof.rx_mapped_sfd_sample.is_some()
                    && !self.config.medium.exact_mdi_timestamp_mapping_supported()
                || decode_mac(
                    &unhex(&proof.mac_hex)?,
                    self.config.medium.maximum_mac_bytes,
                )
                .is_err()
            {
                return Err("invalid physical ingress proof".into());
            }
            *next += 1;
            prior_completed = proof.completed_sample;
        }
        if named
            && self
                .admissions
                .iter()
                .any(|(id, a)| counts.get(id.as_str()).copied().unwrap_or(0) != a.received)
        {
            return Err("physical ingress proof reception counters differ".into());
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(&Saved {
            version: 1,
            config: (*self.config).clone(),
            medium: self.medium.checkpoint()?,
            admissions: self.admissions.clone(),
            named_admissions: self.named_admissions.as_ref().clone(),
            physical_ingress_history: self.physical_ingress_history.as_ref().clone(),
        })
        .map_err(|e| e.to_string())
    }
    /// Recreate the named-domain electrical prefix from immutable admission
    /// bytes and admission sample cuts. This is intentionally explicit because
    /// it can cost as much as the original sampled run; callers use it when
    /// accepting a restored network checkpoint as causal evidence.
    pub fn verify_physical_ingress_replay(&self) -> Result<()> {
        self.validate()?;
        if !self.config.medium.physical_mode.is_named() {
            return Ok(());
        }
        let mut replay = Self::new((*self.config).clone())?;
        let mut admitted = 0;
        while replay.next_sample_index() < self.next_sample_index() {
            while self
                .named_admissions
                .get(admitted)
                .is_some_and(|r| r.admitted_at_sample == replay.next_sample_index())
            {
                let record = &self.named_admissions[admitted];
                let admission = self
                    .admissions
                    .get(&record.request.frame_id)
                    .ok_or("replayed PHY admission missing from ledger")?;
                let decoded = decode_mac(
                    &record.request.mac_bytes,
                    self.config.medium.maximum_mac_bytes,
                )?;
                replay.admit(
                    &admission.identity.direction_id,
                    &record.request.frame_id,
                    self.config
                        .origin_ps
                        .checked_add(record.request.at_ps)
                        .ok_or("replayed PHY admission time overflow")?,
                    &decoded.wire,
                )?;
                admitted += 1;
            }
            replay.advance_one_sample()?;
        }
        while self
            .named_admissions
            .get(admitted)
            .is_some_and(|r| r.admitted_at_sample == replay.next_sample_index())
        {
            let record = &self.named_admissions[admitted];
            let admission = self
                .admissions
                .get(&record.request.frame_id)
                .ok_or("replayed PHY admission missing from ledger")?;
            let decoded = decode_mac(
                &record.request.mac_bytes,
                self.config.medium.maximum_mac_bytes,
            )?;
            replay.admit(
                &admission.identity.direction_id,
                &record.request.frame_id,
                self.config
                    .origin_ps
                    .checked_add(record.request.at_ps)
                    .ok_or("replayed PHY admission time overflow")?,
                &decoded.wire,
            )?;
            admitted += 1;
        }
        if admitted != self.named_admissions.len() || replay.checkpoint()? != self.checkpoint()? {
            return Err(
                "named physical ingress history differs from deterministic electrical replay"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn restore(config: &AttachmentConfig, bytes: &[u8]) -> Result<Self> {
        let saved: Saved = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if saved.version != 1 || &saved.config != config {
            return Err("saved PHY attachment immutable mismatch".into());
        }
        let result = Self {
            config: Arc::new(config.clone()),
            medium: SampledMedium::restore(&config.medium, &saved.medium)?,
            admissions: saved.admissions,
            named_admissions: Arc::new(saved.named_admissions),
            physical_ingress_history: Arc::new(saved.physical_ingress_history),
        };
        result.validate()?;
        Ok(result)
    }
    pub fn advance_one_sample(&mut self) -> Result<Vec<AttachmentEffect>> {
        self.advance_samples(1)
    }
    /// Executes exact sampled transactions from the current boundary.  This
    /// call is atomic, and the caller must split at every external FES input.
    pub fn advance_samples(&mut self, count: u64) -> Result<Vec<AttachmentEffect>> {
        if count
            > if self.config.medium.trace_samples {
                4096
            } else {
                1_000_000
            }
        {
            return Err("attached-medium batch exceeds bounded effect capacity".into());
        }
        let mut candidate = self.clone();
        let mut out = vec![];
        for _ in 0..count {
            out.extend(candidate.advance_inner()?);
        }
        *self = candidate;
        Ok(out)
    }
    /// Looks ahead on a clone until the first observable effect, without
    /// committing future PHY state at the current FES time.  A false
    /// `effects_present` means the exact scalar prefix contains no effect;
    /// it does not mean the medium is
    /// quiescent.  A scheduled external input must cap this horizon.
    pub fn predict_until_first_effect(&self, max_samples: u64) -> Result<PredictedEffect> {
        let forecast = self.forecast_until_first_effect(max_samples)?;
        Ok(PredictedEffect {
            samples: forecast.samples,
            end_sample: forecast.end_sample,
            effects_present: !forecast.effects.is_empty(),
        })
    }
    /// Retains the exact future state and first effect from the same scalar
    /// lookahead. Nothing is committed to `self` before the caller's cut.
    pub fn forecast_until_first_effect(&self, max_samples: u64) -> Result<AttachmentForecast> {
        if max_samples
            > if self.config.medium.trace_samples {
                4096
            } else {
                1_000_000
            }
        {
            return Err("attached-medium prediction exceeds bounded effect capacity".into());
        }
        let mut candidate = self.clone();
        for samples in 1..=max_samples {
            let effects = candidate.advance_inner()?;
            if !effects.is_empty() {
                return Ok(AttachmentForecast {
                    samples,
                    end_sample: candidate.next_sample_index(),
                    effects,
                    end_state: candidate,
                });
            }
        }
        Ok(AttachmentForecast {
            samples: max_samples,
            end_sample: candidate.next_sample_index(),
            effects: vec![],
            end_state: candidate,
        })
    }
    fn advance_inner(&mut self) -> Result<Vec<AttachmentEffect>> {
        let physical = self.medium.advance_one_sample_in_transaction()?;
        let mut out = vec![];
        for effect in physical {
            match &effect {
                PhyEffect::TxFinished { origin, .. } | PhyEffect::RetryExhausted { origin, .. } => {
                    let a = self
                        .admissions
                        .get_mut(&origin.frame_id)
                        .ok_or("retired PHY attempt absent from network ledger")?;
                    if a.serialized {
                        return Err("duplicate PHY transmission retirement".into());
                    }
                    let success = matches!(effect, PhyEffect::TxFinished { .. });
                    a.serialized = true;
                    a.tx_failure = (!success).then(|| "retry_exhausted".into());
                    out.push(AttachmentEffect::TxSerialized {
                        transfer_id: origin.frame_id.clone(),
                        direction_id: a.identity.direction_id.clone(),
                        success,
                        reason: a.tx_failure.clone(),
                    });
                    // A terminal failure has no future readiness obligation.
                    // Retire only this admission, never all claims on the port.
                    if !success {
                        a.tx_released = true;
                        out.push(AttachmentEffect::TxReleased {
                            transfer_id: origin.frame_id.clone(),
                            direction_id: a.identity.direction_id.clone(),
                            success: false,
                            reason: a.tx_failure.clone(),
                        });
                    }
                }
                PhyEffect::TxReady {
                    endpoint,
                    ready: true,
                    ..
                } => {
                    for a in self.admissions.values_mut() {
                        let source = if self.config.medium.physical_mode.is_named() {
                            self.config
                                .physical_ports
                                .iter()
                                .find(|p| p.tx_port == a.identity.direction_id)
                                .map(|p| p.endpoint.as_str())
                        } else {
                            self.config
                                .bindings
                                .iter()
                                .find(|b| b.direction_id == a.identity.direction_id)
                                .map(|b| b.from_port.as_str())
                        }
                        .ok_or("missing PHY binding")?;
                        if source == endpoint && a.serialized && !a.tx_released {
                            a.tx_released = true;
                            out.push(AttachmentEffect::TxReleased {
                                transfer_id: a.identity.transfer_id.clone(),
                                direction_id: a.identity.direction_id.clone(),
                                success: a.tx_failure.is_none(),
                                reason: a.tx_failure.clone(),
                            });
                        }
                    }
                }
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    origins,
                    rx_sfd_sample,
                    rx_fcs_sample,
                    rx_mapped_sfd_sample,
                    rx_mapped_fcs_sample,
                    sample_index,
                } => {
                    if self.config.medium.physical_mode.is_named() {
                        let frame =
                            match decode_mac(mac_bytes, self.config.medium.maximum_mac_bytes) {
                                Ok(frame) => frame,
                                Err(reason) => {
                                    out.push(AttachmentEffect::DecodeRejected {
                                        to_port: receiver_endpoint.clone(),
                                        mac_bytes: mac_bytes.clone(),
                                        origins: origins.clone(),
                                        reason,
                                    });
                                    out.push(AttachmentEffect::Physical { effect });
                                    continue;
                                }
                            };
                        if origins.len() == 1 {
                            let origin = &origins[0];
                            let station = self
                                .config
                                .physical_ports
                                .iter()
                                .find(|p| p.endpoint == *receiver_endpoint)
                                .ok_or("unknown named PHY receiver endpoint")?;
                            if let Some(admission) = self.admissions.get_mut(&origin.frame_id) {
                                if self.config.physical_ports.iter().any(|p| {
                                    p.endpoint == origin.source_endpoint
                                        && p.tx_port == admission.identity.direction_id
                                }) {
                                    let index = admission.received;
                                    admission.received = admission
                                        .received
                                        .checked_add(1)
                                        .ok_or("PHY reception counter overflow")?;
                                    if self.physical_ingress_history.len()
                                        >= MAX_PHYSICAL_INGRESS_PROOFS
                                    {
                                        return Err("named physical ingress proof history capacity exceeded".into());
                                    }
                                    Arc::make_mut(&mut self.physical_ingress_history).push(
                                        PhysicalIngressProof {
                                            admitted_transfer_id: origin.frame_id.clone(),
                                            physical_attempt: origin.clone(),
                                            source_endpoint: origin.source_endpoint.clone(),
                                            receiver_endpoint: receiver_endpoint.clone(),
                                            receiver_tx_port: station.tx_port.clone(),
                                            receiver_rx_port: station.rx_port.clone(),
                                            reception_index: index,
                                            mac_hex: frame.wire.mac_hex.clone(),
                                            rx_sfd_sample: *rx_sfd_sample,
                                            rx_fcs_sample: *rx_fcs_sample,
                                            rx_mapped_sfd_sample: *rx_mapped_sfd_sample,
                                            rx_mapped_fcs_sample: *rx_mapped_fcs_sample,
                                            completed_sample: *sample_index,
                                        },
                                    );
                                    out.push(AttachmentEffect::PhysicalIngress {
                                        admitted_transfer_id: origin.frame_id.clone(),
                                        physical_attempt: origin.clone(),
                                        source_endpoint: origin.source_endpoint.clone(),
                                        receiver_endpoint: receiver_endpoint.clone(),
                                        receiver_tx_port: station.tx_port.clone(),
                                        receiver_rx_port: station.rx_port.clone(),
                                        reception_index: index,
                                        frame,
                                        origins: origins.clone(),
                                        rx_sfd_sample: *rx_sfd_sample,
                                        rx_fcs_sample: *rx_fcs_sample,
                                        rx_mapped_sfd_sample: *rx_mapped_sfd_sample,
                                        rx_mapped_fcs_sample: *rx_mapped_fcs_sample,
                                    });
                                    out.push(AttachmentEffect::Physical { effect });
                                    continue;
                                }
                            }
                        }
                        out.push(AttachmentEffect::UnattributedReceive {
                            to_port: receiver_endpoint.clone(),
                            frame,
                            origins: origins.clone(),
                            reason: "ambiguous_or_unbound_physical_attempt".into(),
                        });
                        out.push(AttachmentEffect::Physical { effect });
                        continue;
                    }
                    let candidates: BTreeSet<_> = origins
                        .iter()
                        .filter_map(|origin| {
                            self.admissions
                                .get(&origin.frame_id)
                                .and_then(|a| {
                                    self.config.bindings.iter().find(|b| {
                                        b.direction_id == a.identity.direction_id
                                            && b.from_port == origin.source_endpoint
                                            && b.to_port == *receiver_endpoint
                                    })
                                })
                                .map(|_| origin.frame_id.clone())
                        })
                        .collect();
                    let frame = match decode_mac(mac_bytes, self.config.medium.maximum_mac_bytes) {
                        Ok(frame) => frame,
                        Err(reason) => {
                            out.push(AttachmentEffect::DecodeRejected {
                                to_port: receiver_endpoint.clone(),
                                mac_bytes: mac_bytes.clone(),
                                origins: origins.clone(),
                                reason,
                            });
                            out.push(AttachmentEffect::Physical { effect });
                            continue;
                        }
                    };
                    if candidates.len() == 1 {
                        let id = candidates.into_iter().next().unwrap();
                        let a = self.admissions.get_mut(&id).unwrap();
                        let index = a.received;
                        a.received = a
                            .received
                            .checked_add(1)
                            .ok_or("PHY reception counter overflow")?;
                        out.push(AttachmentEffect::Received {
                            transfer_id: id,
                            direction_id: a.identity.direction_id.clone(),
                            to_port: receiver_endpoint.clone(),
                            reception_index: index,
                            frame,
                            origins: origins.clone(),
                        });
                    } else {
                        out.push(AttachmentEffect::UnattributedReceive {
                            to_port: receiver_endpoint.clone(),
                            frame,
                            origins: origins.clone(),
                            reason: "ambiguous_or_unbound_physical_provenance".into(),
                        });
                    }
                }
                _ => {}
            }
            out.push(AttachmentEffect::Physical { effect });
        }
        if !self.medium.has_pending() {
            for a in self.admissions.values_mut() {
                if a.received == 0 && !a.failure_emitted {
                    if !a.tx_released {
                        return Err("quiescent PHY has unretired transmission".into());
                    }
                    let to_port = if self.config.medium.physical_mode.is_named() {
                        self.config
                            .physical_ports
                            .iter()
                            .find(|p| p.tx_port == a.identity.direction_id)
                            .map(|p| p.rx_port.as_str())
                    } else {
                        self.config
                            .bindings
                            .iter()
                            .find(|b| b.direction_id == a.identity.direction_id)
                            .map(|b| b.to_port.as_str())
                    }
                    .ok_or("missing PHY binding")?;
                    a.failure_emitted = true;
                    out.push(AttachmentEffect::Failed {
                        transfer_id: a.identity.transfer_id.clone(),
                        direction_id: a.identity.direction_id.clone(),
                        to_port: to_port.into(),
                        reason: a
                            .tx_failure
                            .clone()
                            .unwrap_or_else(|| "no_valid_received_frame".into()),
                    });
                }
            }
        }
        Ok(out)
    }
}
