use super::*;
impl Soc {
    fn lease_index(&self, s: &State, lease: &LeaseRef) -> Option<usize> {
        self.config
            .slots
            .iter()
            .position(|slot| slot.id == lease.slot)
            .filter(|i| {
                s.slots[*i].generation == lease.generation
                    && s.slots[*i].owner.as_deref() == Some(&lease.owner)
                    && s.slots[*i].phase != SlotPhase::Free
            })
    }
    pub(super) fn transition(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        action: &CommandOp,
    ) -> ModelResult<mailbox::ResultCode> {
        let active = |lease: &LeaseRef| {
            s.transactions
                .iter()
                .any(|t| t.phase != Phase::Done && t.request.lease.as_ref() == Some(lease))
        };
        let code = apply_transition(&self.config, &mut s.slots, &active, action)?;
        mailbox::lifecycle_notification(&self.config, &s.slots, &mut s.mailboxes, action, code);
        if code == mailbox::ResultCode::Okay {
            if let CommandOp::Publish { lease } = action {
                self.emit_publication(s, c, lease)?;
            }
        }
        Ok(code)
    }
    pub(super) fn emit_publication(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        lease: &LeaseRef,
    ) -> ModelResult {
        let i = self
            .config
            .slots
            .iter()
            .position(|slot| slot.id == lease.slot)
            .ok_or("missing published slot")?;
        self.emit(s,c,"slot_published",None,json!({"slot":lease.slot,"generation":lease.generation,"owner":lease.owner,"base":self.config.slots[i].base,"length":s.slots[i].length}))?;
        self.emit(s,c,"slot_notification",None,json!({"slot":lease.slot,"generation":lease.generation,"completed_range":[0,s.slots[i].length]}))
    }
    fn publish(&self, s: &mut State, c: &mut Context<'_>, lease: &LeaseRef) -> ModelResult<bool> {
        Ok(self.transition(
            s,
            c,
            &CommandOp::Publish {
                lease: lease.clone(),
            },
        )? == mailbox::ResultCode::Okay)
    }
    pub(super) fn record_ipc(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        origin: Origin,
    ) -> ModelResult<u64> {
        let sequence = s.sequence;
        s.ipc_events.push(IpcEvent {
            origin: origin.clone(),
            sequence,
            at_ps: c.now(),
        });
        self.emit(s, c, "ipc_transition", None, json!({"origin":origin}))?;
        Ok(sequence)
    }
    pub(super) fn commands(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        while s.commands < self.workload.commands.len()
            && self.workload.commands[s.commands].at_ps <= c.now()
        {
            let cmd = &self.workload.commands[s.commands];
            s.commands += 1;
            let okay = match &cmd.action {
                CommandOp::Lease { .. }
                | CommandOp::Publish { .. }
                | CommandOp::BeginRead { .. }
                | CommandOp::Release { .. } => {
                    self.record_ipc(
                        s,
                        c,
                        Origin::Script {
                            index: s.commands - 1,
                        },
                    )?;
                    self.transition(s, c, &cmd.action)? == mailbox::ResultCode::Okay
                }
                CommandOp::DdrControl { command } => {
                    let command: crate::runtime::ddr4::Command =
                        serde_json::from_value(command.clone())
                            .map_err(|e| ModelError(format!("DDR control: {e}")))?;
                    if let Some(mut d) = s.ddr.take() {
                        match d.service.issue_control(command) {
                            Ok(effects) => {
                                for effect in effects {
                                    self.apply_ddr_effect(s, &mut d, c, effect)?;
                                }
                                s.ddr = Some(d);
                                true
                            }
                            Err(error) => {
                                s.ddr = Some(d);
                                self.emit(s,c,"ddr_control_rejected",None,json!({"command_id":cmd.id,"rule":error.rule,"earliest_ps":error.earliest_ps}))?;
                                false
                            }
                        }
                    } else {
                        false
                    }
                }
                CommandOp::CancelDma { dma } => {
                    if let Some(i) = self.workload.dma.iter().position(|d| &d.id == dma) {
                        if matches!(s.dma[i].phase, DmaPhase::Done | DmaPhase::Failed) {
                            false
                        } else {
                            s.dma[i].cancelled = true;
                            if !matches!(
                                s.dma[i].phase,
                                DmaPhase::Reading { .. } | DmaPhase::Writing { .. }
                            ) {
                                s.dma[i].phase = DmaPhase::Failed;
                                s.dma[i].staging.clear();
                            }
                            true
                        }
                    } else {
                        false
                    }
                }
            };
            self.emit(s,c,if okay{"slot_command_completed"}else{"slot_command_rejected"},None,json!({"command_id":cmd.id,"command":cmd.action,"reason":if okay{"applied"}else{"stale_generation_owner_phase_or_incomplete_bytes"}}))?;
        }
        Ok(())
    }
    pub(super) fn begin_lease_read(&self, s: &mut State, i: usize) -> ModelResult {
        let request = s.transactions[i].request.clone();
        match start_lease(&self.config, &mut s.slots, &request) {
            Ok(active) => s.transactions[i].lease_read_active = active,
            Err(reason) => s.transactions[i].error = Some(reason.into()),
        }
        Ok(())
    }
    pub(super) fn end_lease_read(&self, s: &mut State, i: usize) -> ModelResult {
        let t = s.transactions[i].clone();
        finish_lease(
            &self.config,
            &mut s.slots,
            &t.request,
            t.lease_read_active,
            t.error.is_none(),
        )?;
        s.transactions[i].lease_read_active = false;
        Ok(())
    }
    pub(super) fn setup_dma(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        for i in 0..s.dma.len() {
            let d = &self.workload.dma[i];
            if matches!(s.dma[i].phase, DmaPhase::Pending) && d.at_ps <= c.now() {
                let register_dma = self.mailbox_index(d.source_address, d.length).is_some()
                    || self.mailbox_index(d.destination, d.length).is_some();
                let valid = (!register_dma
                    || d.length == 4
                        && self.config.dma_buffer_bytes >= 4
                        && d.source_address % 4 == 0
                        && d.destination % 4 == 0)
                    && d.lease.as_ref().is_none_or(|l| {
                        self.lease_index(s, l).is_some_and(|slot| {
                            d.destination == self.config.slots[slot].base
                                && d.length == s.slots[slot].length
                                && d.source == l.owner
                                && matches!(
                                    s.slots[slot].phase,
                                    SlotPhase::Leased | SlotPhase::Filling
                                )
                        })
                    });
                if valid && !s.dma[i].cancelled {
                    s.dma[i].phase = DmaPhase::Setup {
                        until: self.later(c.now(), 1)?,
                    };
                    self.emit(s,c,"dma_setup",None,json!({"dma":d.id,"length":d.length,"staging_capacity":self.config.dma_buffer_bytes}))?;
                } else {
                    s.dma[i].phase = DmaPhase::Failed;
                    self.emit(
                        s,
                        c,
                        "dma_failed",
                        None,
                        json!({"dma":d.id,"reason":"stale_or_invalid_lease"}),
                    )?;
                }
            }
            if matches!(s.dma[i].phase,DmaPhase::Setup{until} if until<=c.now()) {
                s.dma[i].phase = DmaPhase::Read;
            }
        }
        Ok(())
    }
    pub(super) fn dma_candidate(
        &self,
        s: &State,
        source: &str,
        now: u64,
    ) -> ModelResult<Option<(Request, Client)>> {
        for (i, runtime) in s.dma.iter().enumerate() {
            let d = &self.workload.dma[i];
            if d.source != source || runtime.cancelled {
                continue;
            }
            let (operation, address, length, data, client, lease) = match runtime.phase {
                DmaPhase::Read => {
                    let address = d
                        .source_address
                        .checked_add(runtime.offset as u32)
                        .ok_or("DMA address overflow")?;
                    let register_dma = self.mailbox_index(d.source_address, d.length).is_some()
                        || self.mailbox_index(d.destination, d.length).is_some();
                    let count = if register_dma {
                        4
                    } else {
                        (d.length - runtime.offset)
                            .min(self.config.dma_buffer_bytes)
                            .min(
                                self.config.cache.line_bytes
                                    - (address as usize % self.config.cache.line_bytes),
                            )
                    };
                    (
                        Operation::Read,
                        address,
                        count,
                        vec![],
                        Client::DmaRead { index: i },
                        None,
                    )
                }
                DmaPhase::Write => (
                    Operation::Write,
                    d.destination
                        .checked_add(runtime.offset as u32)
                        .ok_or("DMA address overflow")?,
                    runtime.staging.len(),
                    runtime.staging.clone(),
                    Client::DmaWrite { index: i },
                    d.lease.clone(),
                ),
                _ => continue,
            };
            return Ok(Some((
                Request {
                    id: format!(
                        "dma:{}:{}:{}",
                        d.id,
                        runtime.offset,
                        if operation == Operation::Read {
                            "read"
                        } else {
                            "write"
                        }
                    ),
                    source: d.source.clone(),
                    at_ps: now,
                    operation,
                    address,
                    length,
                    transport: d.transport,
                    router: d.router,
                    axi_id: 0,
                    qos: d.qos,
                    data,
                    byte_enables: vec![],
                    atomic: None,
                    r_stall_cycles: 0,
                    b_stall_cycles: 0,
                    lease,
                },
                client,
            )));
        }
        Ok(None)
    }
    pub(super) fn dma_completion(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        index: usize,
        txn: usize,
    ) -> ModelResult {
        let t = s.transactions[txn].clone();
        let d = &self.workload.dma[index];
        if t.error.is_some() || s.dma[index].cancelled {
            s.dma[index].phase = DmaPhase::Failed;
            s.dma[index].staging.clear();
            self.emit(s,c,"dma_failed",Some(txn),json!({"dma":d.id,"reason":t.error.as_deref().unwrap_or("cancelled"),"completed_bytes":s.dma[index].offset}))?;
            return Ok(());
        }
        match t.client {
            Client::DmaRead { .. } => {
                s.dma[index].staging = t.result;
                s.dma[index].phase = DmaPhase::Write;
                self.emit(
                    s,
                    c,
                    "dma_bytes_captured",
                    Some(txn),
                    json!({"dma":d.id,"offset":s.dma[index].offset,"data":s.dma[index].staging}),
                )?;
            }
            Client::DmaWrite { .. } => {
                s.dma[index].offset += t.request.length;
                s.dma[index].staging.clear();
                self.emit(s,c,"dma_destination_completed",Some(txn),json!({"dma":d.id,"completed_range":[s.dma[index].offset-t.request.length,s.dma[index].offset]}))?;
                if s.dma[index].offset == d.length {
                    s.dma[index].phase = DmaPhase::Done;
                    if d.publish {
                        self.record_ipc(s, c, Origin::Dma { index })?;
                    }
                    if d.publish
                        && !self.publish(s, c, d.lease.as_ref().ok_or("publish without lease")?)?
                    {
                        s.dma[index].phase = DmaPhase::Failed;
                        self.emit(
                            s,
                            c,
                            "dma_failed",
                            Some(txn),
                            json!({"dma":d.id,"reason":"publish_incomplete_or_stale"}),
                        )?;
                    }
                } else {
                    s.dma[index].phase = DmaPhase::Read;
                }
            }
            _ => return Err("non-DMA completion".into()),
        }
        Ok(())
    }
}

/// Immutable input reference for every applied slot or mapped-register operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Origin {
    Script { index: usize },
    Mapped { transaction: usize },
    Dma { index: usize },
    Reset { command: usize },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IpcEvent {
    pub origin: Origin,
    pub sequence: u64,
    pub at_ps: u64,
}
fn checked_lease(
    c: &Config,
    slots: &[Slot],
    lease: &LeaseRef,
) -> Result<usize, mailbox::ResultCode> {
    use mailbox::ResultCode as R;
    let i = c
        .slots
        .iter()
        .position(|s| s.id == lease.slot)
        .ok_or(R::InvalidPhase)?;
    if slots[i].generation != lease.generation {
        return Err(R::StaleGeneration);
    }
    if slots[i].owner.as_deref() != Some(&lease.owner) {
        return Err(R::ForeignSource);
    }
    Ok(i)
}
pub(super) fn apply_transition(
    c: &Config,
    slots: &mut [Slot],
    active: &dyn Fn(&LeaseRef) -> bool,
    action: &CommandOp,
) -> ModelResult<mailbox::ResultCode> {
    use mailbox::ResultCode as R;
    if let CommandOp::Lease {
        slot,
        owner,
        length,
    } = action
    {
        let Some(i) = c.slots.iter().position(|s| s.id == *slot) else {
            return Ok(R::InvalidPhase);
        };
        if slots[i].phase != SlotPhase::Free {
            return Ok(R::InvalidPhase);
        }
        if *length == 0 || *length > c.slots[i].capacity || owner.is_empty() {
            return Ok(R::InvalidLength);
        }
        let generation = slots[i]
            .generation
            .checked_add(1)
            .ok_or("slot generation overflow")?;
        slots[i] = Slot {
            generation,
            owner: Some(owner.clone()),
            consumer: None,
            phase: SlotPhase::Leased,
            length: *length,
            completed: vec![false; *length],
            active_reads: 0,
        };
        return Ok(R::Okay);
    }
    let lease = match action {
        CommandOp::Publish { lease }
        | CommandOp::BeginRead { lease, .. }
        | CommandOp::Release { lease, .. } => lease,
        _ => return Err("non-IPC lifecycle action".into()),
    };
    let i = match checked_lease(c, slots, lease) {
        Ok(i) => i,
        Err(code) => return Ok(code),
    };
    Ok(match action {
        CommandOp::Publish { .. } => {
            if !matches!(slots[i].phase, SlotPhase::Leased | SlotPhase::Filling) {
                R::InvalidPhase
            } else if slots[i].completed.len() != slots[i].length
                || slots[i].completed.iter().any(|b| !*b)
            {
                R::IncompletePayload
            } else if active(lease) {
                R::Outstanding
            } else {
                slots[i].phase = SlotPhase::Ready;
                R::Okay
            }
        }
        CommandOp::BeginRead { consumer, .. } => {
            if slots[i].phase != SlotPhase::Ready || consumer.is_empty() {
                R::InvalidPhase
            } else {
                slots[i].phase = SlotPhase::Reading;
                slots[i].consumer = Some(consumer.clone());
                R::Okay
            }
        }
        CommandOp::Release { consumer, .. } => {
            if slots[i].phase != SlotPhase::Reading {
                R::InvalidPhase
            } else if slots[i].consumer.as_deref() != Some(consumer) {
                R::ForeignSource
            } else if slots[i].active_reads != 0 || active(lease) {
                R::Outstanding
            } else {
                slots[i].phase = SlotPhase::Free;
                slots[i].owner = None;
                slots[i].consumer = None;
                slots[i].length = 0;
                slots[i].completed.clear();
                R::Okay
            }
        }
        _ => unreachable!(),
    })
}
pub(super) fn start_lease(
    c: &Config,
    slots: &mut [Slot],
    r: &Request,
) -> Result<bool, &'static str> {
    let Some(lease) = &r.lease else {
        return Ok(false);
    };
    let i = checked_lease(c, slots, lease).map_err(|_| "stale_or_foreign_lease")?;
    if slots[i].phase == SlotPhase::Free {
        return Err("stale_or_foreign_lease");
    }
    let within = r.address >= c.slots[i].base
        && r.address as u64 + r.length as u64 <= c.slots[i].base as u64 + slots[i].length as u64;
    if r.operation == Operation::Read {
        if within
            && slots[i].phase == SlotPhase::Reading
            && slots[i].consumer.as_deref() == Some(&r.source)
        {
            slots[i].active_reads += 1;
            Ok(true)
        } else {
            Err("slot_not_reading_or_foreign_consumer")
        }
    } else if within
        && r.source == lease.owner
        && matches!(slots[i].phase, SlotPhase::Leased | SlotPhase::Filling)
    {
        slots[i].phase = SlotPhase::Filling;
        Ok(false)
    } else {
        Err("slot_not_filling_or_foreign_owner")
    }
}
pub(super) fn finish_lease(
    c: &Config,
    slots: &mut [Slot],
    r: &Request,
    read_active: bool,
    success: bool,
) -> ModelResult {
    let Some(lease) = &r.lease else {
        return Ok(());
    };
    if !success && !read_active {
        return Ok(());
    }
    let i = checked_lease(c, slots, lease)
        .map_err(|_| ModelError("lease generation changed while operation outstanding".into()))?;
    if read_active {
        slots[i].active_reads = slots[i]
            .active_reads
            .checked_sub(1)
            .ok_or("slot active read underflow")?;
    }
    if success && r.operation == Operation::Write {
        let start = r
            .address
            .checked_sub(c.slots[i].base)
            .ok_or("lease write below slot")? as usize;
        let end = start.checked_add(r.length).ok_or("lease range overflow")?;
        let destination = slots[i]
            .completed
            .get_mut(start..end)
            .ok_or("lease completed write exceeds slot")?;
        for (n, b) in destination.iter_mut().enumerate() {
            if r.byte_enables.is_empty() || r.byte_enables[n] {
                *b = true;
            }
        }
    }
    Ok(())
}
