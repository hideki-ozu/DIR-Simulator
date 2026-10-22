//! Bounded metadata audit, using the live lease and register transition functions.
//! This does not replay CPU instructions, native buses, DDR commands or payload RAM.
use super::*;
impl Soc {
    pub(super) fn validate_ipc_history(&self, s: &State) -> ModelResult {
        enum Event<'a> {
            Admit(usize),
            Finish(usize),
            Ipc(&'a ipc::IpcEvent),
        }
        let mut events = Vec::new();
        let mut sequence = BTreeSet::new();
        for (i, t) in s.transactions.iter().enumerate() {
            if t.admitted_sequence >= s.sequence
                || !sequence.insert(t.admitted_sequence)
                || (t.phase == Phase::Done)
                    != (t.completed_sequence.is_some() && t.completed_at.is_some())
                || t.completed_sequence.is_some() != t.completed_at.is_some()
            {
                return Err("IPC transaction lifetime/sequence mismatch".into());
            }
            events.push((t.admitted_sequence, t.admitted, Event::Admit(i)));
            if let (Some(seq), Some(at)) = (t.completed_sequence, t.completed_at) {
                if seq <= t.admitted_sequence
                    || seq >= s.sequence
                    || at < t.admitted
                    || s.last_tick.is_none_or(|last| at > last)
                    || !sequence.insert(seq)
                {
                    return Err("IPC transaction completion sequence/time mismatch".into());
                }
                events.push((seq, at, Event::Finish(i)));
            }
        }
        let mut origins = BTreeSet::new();
        for e in &s.ipc_events {
            if e.sequence >= s.sequence
                || e.at_ps % self.config.cycle_ps != 0
                || s.last_tick.is_none_or(|last| e.at_ps > last)
                || !sequence.insert(e.sequence)
                || !origins.insert(
                    serde_json::to_string(&e.origin).map_err(|e| ModelError(e.to_string()))?,
                )
            {
                return Err("duplicate/invalid IPC input origin or sequence".into());
            }
            events.push((e.sequence, e.at_ps, Event::Ipc(e)));
        }
        events.sort_by_key(|(seq, _, _)| *seq);
        let mut slots: Vec<Slot> = self
            .config
            .slots
            .iter()
            .map(|_| Slot {
                generation: 0,
                owner: None,
                consumer: None,
                phase: SlotPhase::Free,
                length: 0,
                completed: vec![],
                active_reads: 0,
            })
            .collect();
        let mut endpoints = vec![mailbox::Endpoint::default(); self.config.mailboxes.len()];
        let mut active = BTreeSet::new();
        let mut reads = vec![false; s.transactions.len()];
        let mut mapped = BTreeSet::new();
        let mut scripted = BTreeSet::new();
        let mut published_dma = BTreeSet::new();
        let mut resets = BTreeSet::new();
        let mut previous_time = 0;
        for (seq, at, event) in events {
            if at < previous_time {
                return Err("IPC causal sequence rewinds physical time".into());
            }
            previous_time = at;
            match event {
                Event::Admit(i) => {
                    active.insert(i);
                    match ipc::start_lease(&self.config, &mut slots, &s.transactions[i].request) {
                        Ok(read) => reads[i] = read,
                        Err(_) => {
                            if s.transactions[i].error.is_none() {
                                return Err(
                                    "successful transaction admitted with invalid lease".into()
                                );
                            }
                        }
                    }
                }
                Event::Finish(i) => {
                    if !active.remove(&i) {
                        return Err("IPC transaction completion has no admission".into());
                    }
                    ipc::finish_lease(
                        &self.config,
                        &mut slots,
                        &s.transactions[i].request,
                        reads[i],
                        s.transactions[i].error.is_none(),
                    )?;
                    reads[i] = false;
                }
                Event::Ipc(e) => {
                    let ongoing = |lease: &LeaseRef| {
                        active
                            .iter()
                            .any(|i| s.transactions[*i].request.lease.as_ref() == Some(lease))
                    };
                    match e.origin {
                        ipc::Origin::Script { index } => {
                            let command = self
                                .workload
                                .commands
                                .get(index)
                                .filter(|_| index < s.commands)
                                .ok_or("IPC script origin is not processed")?;
                            if command.at_ps != at || !scripted.insert(index) {
                                return Err("IPC script time/duplicate mismatch".into());
                            }
                            let code = ipc::apply_transition(
                                &self.config,
                                &mut slots,
                                &ongoing,
                                &command.action,
                            )?;
                            mailbox::lifecycle_notification(
                                &self.config,
                                &slots,
                                &mut endpoints,
                                &command.action,
                                code,
                            );
                        }
                        ipc::Origin::Mapped { transaction: i } => {
                            let a = s
                                .mailbox_accesses
                                .get(&i)
                                .ok_or("IPC mapped effect has no accepted target")?;
                            let t = s
                                .transactions
                                .get(i)
                                .ok_or("IPC mapped effect lost logical transaction")?;
                            if !a.applied
                                || a.applied_sequence != Some(seq)
                                || a.ready != at
                                || !mapped.insert(i)
                            {
                                return Err("IPC mapped apply marker/deadline mismatch".into());
                            }
                            if a.endpoint >= endpoints.len() || !self.mailbox_access_valid(t) {
                                return Err("IPC mapped owner/shape mismatch".into());
                            }
                            let cfg = &self.config.mailboxes[a.endpoint];
                            let role = usize::from(t.request.source != cfg.producer);
                            if endpoints[a.endpoint].stages[role] != a.stage {
                                return Err(
                                    "IPC mapped snapshot disagrees with staged words".into()
                                );
                            }
                            let (data, result, _) = mailbox::apply_access(
                                &self.config,
                                a.endpoint,
                                &mut endpoints[a.endpoint],
                                &mut slots,
                                &t.request,
                                &a.stage,
                                &ongoing,
                            )?;
                            let slot = self
                                .config
                                .slots
                                .iter()
                                .position(|s| s.id == cfg.slot)
                                .ok_or("IPC mapped slot absent")?;
                            if data != a.data
                                || result != a.result
                                || slots[slot].generation != a.generation
                            {
                                return Err(
                                    "IPC mapped derived register/result/generation mismatch".into(),
                                );
                            }
                        }
                        ipc::Origin::Dma { index } => {
                            let d = self
                                .workload
                                .dma
                                .get(index)
                                .ok_or("IPC DMA publication lacks descriptor")?;
                            if !d.publish || !published_dma.insert(index) {
                                return Err("IPC DMA publication duplicate/unrequested".into());
                            }
                            let mut count = 0;
                            for t in &s.transactions {
                                if t.client == (Client::DmaWrite { index })
                                    && t.completed_sequence.is_some_and(|end| end < seq)
                                    && t.error.is_none()
                                {
                                    count += t.request.length;
                                }
                            }
                            if count != d.length {
                                return Err(
                                    "IPC DMA publication precedes actual destination completion"
                                        .into(),
                                );
                            }
                            let action = CommandOp::Publish {
                                lease: d.lease.clone().ok_or("DMA publication without lease")?,
                            };
                            let result =
                                ipc::apply_transition(&self.config, &mut slots, &ongoing, &action)?;
                            mailbox::lifecycle_notification(
                                &self.config,
                                &slots,
                                &mut endpoints,
                                &action,
                                result,
                            );
                            if (result == mailbox::ResultCode::Okay)
                                != matches!(s.dma[index].phase, DmaPhase::Done)
                            {
                                return Err("IPC DMA publication result/lifecycle mismatch".into());
                            }
                        }
                        ipc::Origin::Reset { command } => {
                            let cmd = self
                                .workload
                                .commands
                                .get(command)
                                .filter(|_| command < s.commands)
                                .ok_or("IPC reset lacks processed command")?;
                            if cmd.at_ps != at
                                || !resets.insert(command)
                                || !matches!(&cmd.action,CommandOp::DdrControl{command} if command["kind"]=="reset"&&command["asserted"]==true)
                            {
                                return Err("IPC reset origin mismatch".into());
                            }
                            for (i, slot) in slots.iter_mut().enumerate() {
                                if self.is_ddr(self.config.slots[i].base) {
                                    *slot = Slot {
                                        generation: slot.generation,
                                        owner: None,
                                        consumer: None,
                                        phase: SlotPhase::Free,
                                        length: 0,
                                        completed: vec![],
                                        active_reads: 0,
                                    };
                                    for (endpoint, cfg) in
                                        endpoints.iter_mut().zip(&self.config.mailboxes)
                                    {
                                        if cfg.slot == self.config.slots[i].id {
                                            endpoint.notification = None;
                                        }
                                    }
                                }
                            }
                            for (i, t) in s.transactions.iter().enumerate() {
                                if self.is_ddr(t.request.address) {
                                    reads[i] = false;
                                }
                            }
                        }
                    }
                }
            }
        }
        if slots != s.slots {
            return Err("IPC slot lifecycle differs from typed transition history".into());
        }
        for (expected, actual) in endpoints.iter().zip(&s.mailboxes) {
            if expected.stages != actual.stages
                || expected.result != actual.result
                || expected.notification != actual.notification
            {
                return Err(
                    "IPC mailbox register/notification differs from transition history".into(),
                );
            }
        }
        for (i, a) in &s.mailbox_accesses {
            if a.applied != mapped.contains(i) || a.applied != a.applied_sequence.is_some() {
                return Err("IPC accepted target lost or duplicated applied effect".into());
            }
        }
        for (index, cmd) in self.workload.commands.iter().take(s.commands).enumerate() {
            if matches!(
                cmd.action,
                CommandOp::Lease { .. }
                    | CommandOp::Publish { .. }
                    | CommandOp::BeginRead { .. }
                    | CommandOp::Release { .. }
            ) && !scripted.contains(&index)
            {
                return Err("processed IPC command lost transition history".into());
            }
        }
        for (index, d) in self.workload.dma.iter().enumerate() {
            if d.publish
                && matches!(s.dma[index].phase, DmaPhase::Done)
                && !published_dma.contains(&index)
            {
                return Err("completed DMA publication lost transition history".into());
            }
        }
        if s.ddr
            .as_ref()
            .is_some_and(|d| d.reset_epoch() != resets.len() as u64)
        {
            return Err("DDR reset epoch/IPC invalidation history mismatch".into());
        }
        for (i, t) in s.transactions.iter().enumerate() {
            if reads[i] != t.lease_read_active {
                return Err("IPC active lease read differs from transaction lifetime".into());
            }
        }
        Ok(())
    }
}
