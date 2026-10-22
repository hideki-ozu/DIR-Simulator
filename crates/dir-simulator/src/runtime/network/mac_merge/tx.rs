use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TxPhase {
    Idle,
    StartPreamble,
    PreemptableData,
    AppendMcrc,
    ResumeWait,
    Ipg,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectedPmac {
    pub mac: Vec<u8>,
    pub offset: usize,
    pub p_allow: bool,
    pub peer_add_frag_size: u8,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MergeTxState {
    pub epoch: u64,
    pub tx_frame: u8,
    pub tx_frag: u8,
    pub hold: bool,
    pub selected: Option<SelectedPmac>,
    pub phase: TxPhase,
    pub ipg_until: u64,
    pub generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TxPacketPlan {
    pub epoch: u64,
    pub generation: u64,
    pub bytes: Vec<u8>,
    pub bitrate: u64,
    pub start_offset: usize,
    pub end_offset: usize,
    pub final_packet: bool,
    pub start_ps: u64,
    pub eof_ps: u64,
    pub release_ps: u64,
}
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleSelection {
    Express,
    Respond,
    Verify,
    Preemptable,
    None,
}
/// Figure 99-5 Idle and ResumeWait have deliberately different priorities.
#[cfg(test)]
pub(crate) fn select_service(
    suspended: bool,
    hold: bool,
    express: bool,
    respond: bool,
    verify: bool,
    pmac: bool,
) -> IdleSelection {
    if express {
        IdleSelection::Express
    } else if suspended {
        if !hold {
            IdleSelection::Preemptable
        } else {
            IdleSelection::None
        }
    } else if respond {
        IdleSelection::Respond
    } else if verify {
        IdleSelection::Verify
    } else if pmac && !hold {
        IdleSelection::Preemptable
    } else {
        IdleSelection::None
    }
}
impl MergeTxState {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            tx_frame: 0,
            tx_frag: 0,
            hold: false,
            selected: None,
            phase: TxPhase::Idle,
            ipg_until: 0,
            generation: 0,
        }
    }
    pub fn reset(&mut self, epoch: u64) -> bool {
        let aborted = self.selected.is_some();
        *self = Self::new(epoch);
        aborted
    }
    pub fn select_pmac(
        &mut self,
        mac: Vec<u8>,
        active: bool,
        peer_add_frag_size: u8,
    ) -> MergeResult<()> {
        if self.selected.is_some() || self.hold || peer_add_frag_size > 3 {
            return Err(MergeError::InvalidState);
        }
        if !valid_mac(&mac) {
            return Err(MergeError::InvalidMacFcs);
        }
        self.selected = Some(SelectedPmac {
            mac,
            offset: 0,
            p_allow: active,
            peer_add_frag_size,
        });
        self.tx_frag = 0;
        self.phase = TxPhase::StartPreamble;
        Ok(())
    }
    /// Latch pActive exactly when the first delimiter is emitted. The initial
    /// select_pmac snapshot can be updated during its preamble, then becomes
    /// immutable for every continuation. A changed delimiter invalidates a
    /// previously reserved EOF plan; the owner must replace that plan atomically.
    pub fn smd_sent(&mut self, active: bool) -> MergeResult<bool> {
        if self.phase != TxPhase::StartPreamble {
            return Err(MergeError::InvalidState);
        }
        let s = self.selected.as_mut().ok_or(MergeError::InvalidState)?;
        if s.offset != 0 {
            return Err(MergeError::InvalidState);
        }
        let changed = s.p_allow != active;
        if changed {
            self.generation = self.generation.checked_add(1).ok_or(MergeError::Overflow)?;
            s.p_allow = active;
        }
        self.phase = TxPhase::PreemptableData;
        Ok(changed)
    }
    /// Plan or replace a not-yet-completed packet when express/HOLD demand occurs.
    /// `demand_ps` is an absolute physical time. The cut is the first legal octet
    /// boundary at/after that time, never a fixed periodic slicing rule.
    pub fn plan_packet(
        &mut self,
        start_ps: u64,
        demand_ps: Option<u64>,
        bitrate: u64,
    ) -> MergeResult<TxPacketPlan> {
        if start_ps < self.ipg_until {
            return Err(MergeError::InvalidState);
        }
        let s = self.selected.as_ref().ok_or(MergeError::InvalidState)?;
        let mut end = s.mac.len() - 4;
        if s.p_allow {
            if let Some(demand) = demand_ps {
                let octet_ps = bit_duration(8, bitrate)?;
                let data_start = checked_time(start_ps, bit_duration(64, bitrate)?)?;
                let demanded_octets =
                    usize::try_from(demand.saturating_sub(data_start).div_ceil(octet_ps))
                        .map_err(|_| MergeError::Overflow)?;
                let cut = s
                    .offset
                    .checked_add(
                        demanded_octets.max(64 * (1 + usize::from(s.peer_add_frag_size)) - 4),
                    )
                    .ok_or(MergeError::Overflow)?;
                if cut <= s.mac.len().saturating_sub(64) {
                    end = cut;
                }
            }
        }
        let bytes = encode_fragment(
            &s.mac,
            s.offset,
            end,
            self.tx_frame,
            if s.offset == 0 {
                None
            } else {
                Some(self.tx_frag)
            },
            s.p_allow,
            s.peer_add_frag_size,
        )?;
        let bits = u64::try_from(bytes.len())
            .ok()
            .and_then(|n| n.checked_mul(8))
            .ok_or(MergeError::Overflow)?;
        let eof_ps = checked_time(start_ps, bit_duration(bits, bitrate)?)?;
        let release_ps = checked_time(eof_ps, bit_duration(96, bitrate)?)?;
        self.generation = self.generation.checked_add(1).ok_or(MergeError::Overflow)?;
        Ok(TxPacketPlan {
            epoch: self.epoch,
            generation: self.generation,
            bytes,
            bitrate,
            start_offset: s.offset,
            end_offset: end,
            final_packet: end == s.mac.len() - 4,
            start_ps,
            eof_ps,
            release_ps,
        })
    }
    /// Only a real physical EOF may commit offsets/counters. IPG release remains
    /// reserved by the enclosing serializer until release_ps.
    pub fn complete_packet(&mut self, plan: &TxPacketPlan, now: u64) -> MergeResult<()> {
        if plan.epoch != self.epoch || plan.generation != self.generation || now != plan.eof_ps {
            return Err(MergeError::InvalidState);
        }
        let bits = u64::try_from(plan.bytes.len())
            .ok()
            .and_then(|n| n.checked_mul(8))
            .ok_or(MergeError::Overflow)?;
        if plan.start_ps < self.ipg_until
            || checked_time(plan.start_ps, bit_duration(bits, plan.bitrate)?)? != plan.eof_ps
            || checked_time(plan.eof_ps, bit_duration(96, plan.bitrate)?)? != plan.release_ps
        {
            return Err(MergeError::InvalidState);
        }
        let s = self.selected.as_mut().ok_or(MergeError::InvalidState)?;
        if s.offset != plan.start_offset {
            return Err(MergeError::InvalidState);
        }
        let expected = encode_fragment(
            &s.mac,
            s.offset,
            plan.end_offset,
            self.tx_frame,
            if s.offset == 0 {
                None
            } else {
                Some(self.tx_frag)
            },
            s.p_allow,
            s.peer_add_frag_size,
        )?;
        if expected != plan.bytes || plan.final_packet != (plan.end_offset == s.mac.len() - 4) {
            return Err(MergeError::InvalidState);
        }
        let next_generation = self.generation.checked_add(1).ok_or(MergeError::Overflow)?;
        if plan.final_packet {
            self.selected = None;
            self.tx_frame = (self.tx_frame + 1) % 4;
            self.phase = TxPhase::Ipg;
        } else {
            if s.offset > 0 {
                self.tx_frag = (self.tx_frag + 1) % 4;
            }
            s.offset = plan.end_offset;
            self.phase = TxPhase::ResumeWait;
        }
        self.ipg_until = plan.release_ps;
        // A completion token is single-use, including after a final packet.
        self.generation = next_generation;
        Ok(())
    }
    pub fn validate_checkpoint(&self) -> MergeResult<()> {
        if self.tx_frame > 3
            || self.tx_frag > 3
            || (self.selected.is_none() && !matches!(self.phase, TxPhase::Idle | TxPhase::Ipg))
            || self.selected.as_ref().is_some_and(|s| {
                !valid_mac(&s.mac)
                    || s.peer_add_frag_size > 3
                    || s.offset > s.mac.len() - 4
                    || (s.offset == 0
                        && !matches!(
                            self.phase,
                            TxPhase::StartPreamble | TxPhase::PreemptableData
                        ))
                    || (s.offset > 0
                        && (self.phase != TxPhase::ResumeWait
                            || !s.p_allow
                            || s.offset < 64 * (1 + usize::from(s.peer_add_frag_size)) - 4
                            || s.mac.len() - s.offset < 64))
            })
        {
            return Err(MergeError::InvalidCheckpoint);
        }
        Ok(())
    }
}
