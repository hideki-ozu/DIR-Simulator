use super::*;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PeerCapabilitiesObserved {
    pub ingress: usize,
    pub link_epoch: u64,
    pub source_mac: [u8; 6],
    pub port_id: Vec<u8>,
    pub received_at: u64,
    pub support: bool,
    pub enabled: bool,
    pub active: bool,
    pub add_frag_size: u8,
    pub lldp_evidence_id: String,
}
impl PeerCapabilitiesObserved {
    /// Bounded Clause 79.3.7 receive adapter: extracts the extension from an
    /// actually validated LLDP MAC. LLDP mandatory TLVs/lifetime are owned by
    /// the caller's LLDP owner; this is not an autonomous IEEE 802.1AB engine.
    pub fn from_received_lldp(
        ingress: usize,
        packet: &ValidatedMacIngress,
        evidence_id: String,
    ) -> MergeResult<Self> {
        let b = &packet.bytes;
        if evidence_id.is_empty()
            || !valid_mac(b)
            || b[..6] != [1, 0x80, 0xc2, 0, 0, 0x0e]
            || b[12..14] != [0x88, 0xcc]
        {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        let mut at = 14;
        let mut port_id = vec![];
        while at + 2 <= b.len() - 4 {
            let header = u16::from_be_bytes([b[at], b[at + 1]]);
            at += 2;
            let kind = header >> 9;
            let n = usize::from(header & 0x1ff);
            if at.checked_add(n).is_none_or(|end| end > b.len() - 4) {
                return Err(MergeError::MissingCapabilityEvidence);
            }
            if kind == 0 {
                break;
            }
            let value = &b[at..at + n];
            if kind == 2 {
                port_id = value.to_vec();
            }
            if kind == 127 && value.len() >= 4 && value[..4] == [0, 0x12, 0x0f, 7] {
                // Omitted extra octets act as zero; additional octets and
                // reserved capability bits are ignored (79.3.7.1).
                let bits =
                    u16::from_be_bytes([*value.get(4).unwrap_or(&0), *value.get(5).unwrap_or(&0)]);
                return Ok(Self {
                    ingress,
                    link_epoch: packet.epoch,
                    source_mac: b[6..12].try_into().unwrap(),
                    port_id,
                    received_at: packet.received_at,
                    support: bits & 1 != 0,
                    enabled: bits & 2 != 0,
                    active: bits & 4 != 0,
                    add_frag_size: ((bits >> 3) & 3) as u8,
                    lldp_evidence_id: evidence_id,
                });
            }
            at += n;
        }
        Err(MergeError::MissingCapabilityEvidence)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum VerificationPhase {
    InitVerification,
    VerificationIdle,
    SendVerify,
    WaitForResponse,
    Verified,
    VerifyFail,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RespondPhase {
    RespondIdle,
    SendRespond,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyTimer {
    pub epoch: u64,
    pub generation: u64,
    pub due_ps: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlTxToken {
    pub epoch: u64,
    pub generation: u64,
    pub kind: PacketKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerificationState {
    pub epoch: u64,
    pub admin_enabled: bool,
    pub link_up: bool,
    pub peer: Option<PeerCapabilitiesObserved>,
    pub verify_time_ms: u8,
    pub phase: VerificationPhase,
    pub attempts: u8,
    pub send_v: bool,
    pub send_r: bool,
    pub rcv_r: bool,
    pub rcv_v: bool,
    pub respond_phase: RespondPhase,
    pub verified: bool,
    pub verify_fail: bool,
    pub last_verify_eof: Option<u64>,
    pub timer: Option<VerifyTimer>,
    pub generation: u64,
    pub control_generation: u64,
}
impl VerificationState {
    pub fn new(epoch: u64, admin_enabled: bool, verify_time_ms: u8) -> MergeResult<Self> {
        if !(1..=128).contains(&verify_time_ms) {
            return Err(MergeError::InvalidConfiguration);
        }
        Ok(Self {
            epoch,
            admin_enabled,
            link_up: true,
            peer: None,
            verify_time_ms,
            phase: VerificationPhase::VerificationIdle,
            attempts: 0,
            send_v: false,
            send_r: false,
            rcv_r: false,
            rcv_v: false,
            respond_phase: RespondPhase::RespondIdle,
            verified: false,
            verify_fail: false,
            last_verify_eof: None,
            timer: None,
            generation: 0,
            control_generation: 0,
        })
    }
    pub fn p_enable(&self) -> bool {
        self.admin_enabled
            && self.link_up
            && self.peer.as_ref().is_some_and(|p| {
                p.link_epoch == self.epoch && p.support && !p.lldp_evidence_id.is_empty()
            })
    }
    pub fn p_active(&self) -> bool {
        self.p_enable() && self.verified
    }
    fn clear_verifier(&mut self) -> MergeResult<()> {
        let next_generation = self.generation.checked_add(1).ok_or(MergeError::Overflow)?;
        let next_control_generation = self
            .control_generation
            .checked_add(1)
            .ok_or(MergeError::Overflow)?;
        self.phase = VerificationPhase::VerificationIdle;
        self.attempts = 0;
        self.send_v = false;
        self.send_r = false;
        self.rcv_r = false;
        self.rcv_v = false;
        self.respond_phase = RespondPhase::RespondIdle;
        self.verified = false;
        self.verify_fail = false;
        self.last_verify_eof = None;
        self.timer = None;
        self.generation = next_generation;
        self.control_generation = next_control_generation;
        Ok(())
    }
    pub fn observe_capabilities(&mut self, peer: PeerCapabilitiesObserved) -> MergeResult<()> {
        if peer.link_epoch != self.epoch {
            return Err(MergeError::WrongEpoch);
        }
        if peer.add_frag_size > 3 || peer.lldp_evidence_id.is_empty() {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        let was_enabled = self.p_enable()
            && self.peer.as_ref().is_some_and(|old| {
                old.source_mac == peer.source_mac && old.port_id == peer.port_id
            });
        self.peer = Some(peer);
        if !self.p_enable() || !was_enabled {
            self.clear_verifier()?;
        }
        self.request_if_enabled();
        Ok(())
    }
    pub fn withdraw_capabilities(&mut self) -> MergeResult<()> {
        self.peer = None;
        self.clear_verifier()
    }
    #[cfg(test)]
    pub fn set_admin_enabled(&mut self, enabled: bool) -> MergeResult<()> {
        if self.admin_enabled != enabled {
            self.admin_enabled = enabled;
            self.clear_verifier()?;
            self.request_if_enabled();
        }
        Ok(())
    }
    pub fn link_reset(&mut self, epoch: u64, link_up: bool) -> MergeResult<()> {
        self.epoch = epoch;
        self.link_up = link_up;
        self.peer = None;
        self.clear_verifier()
    }
    pub fn request_if_enabled(&mut self) {
        if self.p_enable() && self.phase == VerificationPhase::VerificationIdle {
            self.phase = VerificationPhase::SendVerify;
            self.send_v = true;
        }
    }
    /// Capture this token when the common serializer accepts a control packet.
    /// It invalidates same-epoch old completions after management resets and
    /// prevents replaying a previous response EOF against another request.
    pub fn control_tx_token(&self, kind: PacketKind) -> MergeResult<ControlTxToken> {
        if !matches!(kind, PacketKind::Verify | PacketKind::Respond)
            || (kind == PacketKind::Verify && !self.send_v)
            || (kind == PacketKind::Respond && !self.send_r)
        {
            return Err(MergeError::InvalidState);
        }
        Ok(ControlTxToken {
            epoch: self.epoch,
            generation: self.control_generation,
            kind,
        })
    }
    pub fn tx_complete_token(
        &mut self,
        token: ControlTxToken,
        eof_ps: u64,
    ) -> MergeResult<Option<VerifyTimer>> {
        if token.epoch != self.epoch || token.generation != self.control_generation {
            return Err(MergeError::InvalidState);
        }
        self.tx_complete(token.epoch, token.kind, eof_ps)
    }
    /// Called only at the actual control packet EOF, never at enqueue/SOF.
    pub fn tx_complete(
        &mut self,
        epoch: u64,
        kind: PacketKind,
        eof_ps: u64,
    ) -> MergeResult<Option<VerifyTimer>> {
        if epoch != self.epoch {
            return Err(MergeError::WrongEpoch);
        }
        let next_control_generation = self
            .control_generation
            .checked_add(1)
            .ok_or(MergeError::Overflow)?;
        match kind {
            PacketKind::Respond
                if self.send_r && self.respond_phase == RespondPhase::SendRespond =>
            {
                self.send_r = false;
                self.respond_phase = RespondPhase::RespondIdle;
                self.request_response();
                self.control_generation = next_control_generation;
                Ok(None)
            }
            PacketKind::Verify
                if self.send_v
                    && self.phase == VerificationPhase::SendVerify
                    && self.p_enable() =>
            {
                let next_attempts = self.attempts.checked_add(1).ok_or(MergeError::Overflow)?;
                let next_generation = self.generation.checked_add(1).ok_or(MergeError::Overflow)?;
                let token = VerifyTimer {
                    epoch,
                    generation: next_generation,
                    due_ps: checked_time(eof_ps, u64::from(self.verify_time_ms) * 1_000_000_000)?,
                };
                self.attempts = next_attempts;
                self.control_generation = next_control_generation;
                self.generation = next_generation;
                self.send_v = false;
                self.last_verify_eof = Some(eof_ps);
                self.timer = Some(token);
                self.phase = VerificationPhase::WaitForResponse;
                if self.rcv_r {
                    self.accept_response();
                }
                Ok(self.timer)
            }
            _ => Err(MergeError::InvalidState),
        }
    }
    fn request_response(&mut self) {
        if self.respond_phase == RespondPhase::RespondIdle && self.rcv_v {
            self.rcv_v = false;
            self.send_r = true;
            self.respond_phase = RespondPhase::SendRespond;
        }
    }
    fn accept_response(&mut self) {
        self.phase = VerificationPhase::Verified;
        self.verified = true;
        self.timer = None;
    }
    /// The caller obtains `kind` only from successful actual-packet RX. At an
    /// equal timestamp RX runs before timeout, so response wins the !rcv_r guard.
    pub fn receive_control(&mut self, epoch: u64, kind: PacketKind) -> MergeResult<()> {
        if epoch != self.epoch {
            return Err(MergeError::WrongEpoch);
        }
        match kind {
            PacketKind::Verify => {
                self.rcv_v = true;
                self.request_response();
            }
            PacketKind::Respond => {
                self.rcv_r = true;
                if self.phase == VerificationPhase::WaitForResponse {
                    self.accept_response();
                }
            }
            _ => return Err(MergeError::InvalidState),
        }
        Ok(())
    }
    /// Stale epoch/generation tokens are harmless, including a timeout whose
    /// equal-time response was already accepted.
    pub fn timeout(&mut self, token: VerifyTimer, now: u64) -> MergeResult<bool> {
        if self.timer != Some(token) || token.epoch != self.epoch {
            return Ok(false);
        }
        if now != token.due_ps || self.phase != VerificationPhase::WaitForResponse {
            return Err(MergeError::InvalidState);
        }
        self.timer = None;
        if self.rcv_r {
            self.accept_response();
        } else if self.attempts < 3 {
            self.phase = VerificationPhase::SendVerify;
            self.send_v = true;
        } else {
            self.phase = VerificationPhase::VerifyFail;
            self.verify_fail = true;
        }
        Ok(true)
    }
    pub fn validate_checkpoint(&self) -> MergeResult<()> {
        let wait = self.phase == VerificationPhase::WaitForResponse;
        let expected_due = self
            .last_verify_eof
            .and_then(|e| e.checked_add(u64::from(self.verify_time_ms) * 1_000_000_000));
        if !(1..=128).contains(&self.verify_time_ms)
            || self.attempts > 3
            || self.send_r != (self.respond_phase == RespondPhase::SendRespond)
            || (self.rcv_v && self.respond_phase == RespondPhase::RespondIdle)
            || self.peer.as_ref().is_some_and(|p| {
                p.link_epoch != self.epoch || p.add_frag_size > 3 || p.lldp_evidence_id.is_empty()
            })
            || wait != self.timer.is_some()
            || self.send_v != (self.phase == VerificationPhase::SendVerify)
            || self.verified != (self.phase == VerificationPhase::Verified)
            || self.verify_fail != (self.phase == VerificationPhase::VerifyFail)
            || (self.phase == VerificationPhase::Verified
                && (self.attempts == 0 || !self.rcv_r || self.last_verify_eof.is_none()))
            || (self.phase == VerificationPhase::VerifyFail
                && (self.attempts != 3 || self.last_verify_eof.is_none()))
            || (self.phase == VerificationPhase::SendVerify && self.attempts >= 3)
            || (matches!(
                self.phase,
                VerificationPhase::InitVerification | VerificationPhase::VerificationIdle
            ) && (self.attempts != 0 || self.last_verify_eof.is_some()))
            || ((wait || self.verified || self.verify_fail || self.send_v) && !self.p_enable())
            || (wait
                && (!(1..=3).contains(&self.attempts)
                    || self.timer.is_none_or(|t| {
                        t.epoch != self.epoch
                            || t.generation != self.generation
                            || Some(t.due_ps) != expected_due
                    })))
        {
            return Err(MergeError::InvalidCheckpoint);
        }
        Ok(())
    }
}
