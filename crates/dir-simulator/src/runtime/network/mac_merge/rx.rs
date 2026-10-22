use super::*;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RxProvenance {
    LocalTransfer(usize),
    ExternalInjection { evidence_id: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhysicalPacket {
    pub bytes: Vec<u8>,
    pub provenance: RxProvenance,
    /// False means truncated/dropped physical reception, never a successful EOF.
    pub complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidatedMacIngress {
    pub epoch: u64,
    pub received_at: u64,
    pub bytes: Vec<u8>,
    pub provenance: RxProvenance,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcceptedFragment {
    pub received_at: u64,
    pub packet: PhysicalPacket,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RxAssembly {
    pub epoch: u64,
    pub frame_count: u8,
    pub next_frag_count: u8,
    pub bytes: Vec<u8>,
    pub crc_state: u32,
    pub fragments: Vec<AcceptedFragment>,
    pub provenance: RxProvenance,
    pub started_at: u64,
    pub last_fragment_at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MergeRxState {
    pub epoch: u64,
    pub assembly: Option<RxAssembly>,
    pub max_mac_octets: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RxOutcome {
    pub ingress: Option<ValidatedMacIngress>,
    pub control: Option<PacketKind>,
    pub errors: Vec<MergeError>,
    pub suspended: bool,
}
impl MergeRxState {
    pub fn new(epoch: u64, max_mac_octets: usize) -> MergeResult<Self> {
        if max_mac_octets < 64 {
            return Err(MergeError::InvalidConfiguration);
        }
        Ok(Self {
            epoch,
            assembly: None,
            max_mac_octets,
        })
    }
    pub fn reset(&mut self, epoch: u64) -> bool {
        self.epoch = epoch;
        self.assembly.take().is_some()
    }
    pub fn validate_checkpoint(&self) -> MergeResult<()> {
        if self.max_mac_octets < 64
            || self.assembly.as_ref().is_some_and(|a| {
                a.epoch != self.epoch
                    || a.frame_count > 3
                    || a.next_frag_count > 3
                    || a.bytes.len() < 60
                    || a.bytes.len() > self.max_mac_octets.saturating_sub(4)
                    || a.crc_state != crate::runtime::ethernet::crc32(&a.bytes)
                    || a.started_at > a.last_fragment_at
            })
        {
            return Err(MergeError::InvalidCheckpoint);
        }
        if let Some(a) = &self.assembly {
            if a.fragments.is_empty() || a.fragments.len() > self.max_mac_octets / 60 {
                return Err(MergeError::InvalidCheckpoint);
            }
            let mut replay = Self::new(self.epoch, self.max_mac_octets)?;
            let mut previous = None;
            for f in &a.fragments {
                if previous.is_some_and(|at| at > f.received_at) {
                    return Err(MergeError::InvalidCheckpoint);
                }
                previous = Some(f.received_at);
                let out = replay.receive(self.epoch, f.received_at, f.packet.clone());
                if !out.errors.is_empty()
                    || out.ingress.is_some()
                    || out.control.is_some()
                    || !out.suspended
                {
                    return Err(MergeError::InvalidCheckpoint);
                }
            }
            if replay.assembly.as_ref() != Some(a) {
                return Err(MergeError::InvalidCheckpoint);
            }
        }
        Ok(())
    }
    pub fn receive(&mut self, epoch: u64, now: u64, packet: PhysicalPacket) -> RxOutcome {
        let mut out = RxOutcome {
            ingress: None,
            control: None,
            errors: vec![],
            suspended: false,
        };
        if epoch != self.epoch {
            out.errors.push(MergeError::WrongEpoch);
            return out;
        }
        if !packet.complete {
            if affects_assembly(&packet.bytes) {
                self.assembly = None;
            }
            out.errors.push(MergeError::FrameCheckError);
            return out;
        }
        let decoded = match decode_packet(&packet.bytes) {
            Ok(d) => d,
            Err(e) => {
                // Figure 99-6 CHECK_FOR_RESUME loops E/ERR back to wait:
                // malformed unrelated packets do not destroy retained pMAC data.
                // A recognizable S/C packet with bad framing/count does.
                if affects_assembly(&packet.bytes) {
                    self.assembly = None;
                }
                out.errors.push(e);
                return out;
            }
        };
        match decoded.kind {
            PacketKind::Verify | PacketKind::Respond => {
                out.control = Some(decoded.kind);
            }
            PacketKind::Express => {
                let mac = packet.bytes[8..].to_vec();
                if mac.len() > self.max_mac_octets {
                    out.errors.push(MergeError::ReceiveLimit);
                } else if !valid_mac(&mac) {
                    out.errors.push(MergeError::InvalidMacFcs);
                } else {
                    out.ingress = Some(ValidatedMacIngress {
                        epoch,
                        received_at: now,
                        bytes: mac,
                        provenance: packet.provenance,
                    });
                }
            }
            PacketKind::Start { frame_count } => {
                if self.assembly.take().is_some() {
                    out.errors.push(MergeError::FrameCheckError);
                }
                self.assembly = Some(RxAssembly {
                    epoch,
                    frame_count,
                    next_frag_count: 0,
                    bytes: vec![],
                    crc_state: 0,
                    provenance: packet.provenance.clone(),
                    fragments: vec![],
                    started_at: now,
                    last_fragment_at: now,
                });
                self.append(decoded, now, packet.clone(), &mut out);
            }
            PacketKind::Continuation {
                frame_count,
                frag_count,
            } => match self.assembly.as_ref() {
                None => out.errors.push(MergeError::NoAssembly),
                Some(a) if a.frame_count != frame_count || a.next_frag_count != frag_count => {
                    self.assembly = None;
                    out.errors.push(MergeError::CounterMismatch);
                }
                Some(_) => {
                    self.assembly.as_mut().unwrap().next_frag_count = (frag_count + 1) % 4;
                    self.append(decoded, now, packet.clone(), &mut out);
                }
            },
        }
        out.suspended = self.assembly.is_some();
        out
    }
    fn append(
        &mut self,
        d: DecodedPacket<'_>,
        now: u64,
        packet: PhysicalPacket,
        out: &mut RxOutcome,
    ) {
        let a = self.assembly.as_mut().unwrap();
        if a.bytes
            .len()
            .checked_add(d.data.len())
            .and_then(|n| n.checked_add(4))
            .is_none_or(|n| n > self.max_mac_octets)
        {
            self.assembly = None;
            out.errors.push(MergeError::ReceiveLimit);
            return;
        }
        a.bytes.extend_from_slice(d.data);
        a.last_fragment_at = now;
        a.crc_state = crate::runtime::ethernet::crc32(&a.bytes);
        if d.crc == mcrc(&a.bytes) {
            a.fragments.push(AcceptedFragment {
                received_at: now,
                packet,
            });
            return;
        }
        let mut a = self.assembly.take().unwrap();
        a.bytes.extend_from_slice(&d.crc);
        if !valid_mac(&a.bytes) {
            out.errors.push(MergeError::InvalidMacFcs);
            return;
        }
        out.ingress = Some(ValidatedMacIngress {
            epoch: a.epoch,
            received_at: now,
            bytes: a.bytes,
            provenance: a.provenance,
        });
    }
}

fn affects_assembly(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && bytes[..6] == [0x55; 6]
        && (SMD_C.contains(&bytes[6]) || (bytes[6] == 0x55 && SMD_S.contains(&bytes[7])))
}
