use super::*;
impl Soc {
    pub(super) fn line(&self, address: u32) -> u32 {
        address & !((self.config.cache.line_bytes as u32) - 1)
    }
    pub(super) fn cache_position(&self, s: &State, core: usize, line: u32) -> Option<usize> {
        s.caches[core]
            .iter()
            .position(|l| l.as_ref().is_some_and(|l| l.line == line))
    }
    pub(super) fn cache_hit(&self, s: &State, i: usize) -> bool {
        let t = &s.transactions[i];
        if self
            .mailbox_index(t.request.address, t.request.length)
            .is_some()
        {
            return false;
        }
        let Some(core) = t.client.core() else {
            return false;
        };
        let line = self.line(t.request.address);
        self.cache_position(s, core, line).is_some_and(|p| {
            t.request.operation == Operation::Read
                || s.caches[core][p]
                    .as_ref()
                    .is_some_and(|l| matches!(l.state, Mesi::E | Mesi::M))
        })
    }
    fn victim(&self, s: &State, core: usize, line: u32) -> usize {
        let set = (line as usize / self.config.cache.line_bytes) % self.config.cache.sets;
        let start = set * self.config.cache.ways;
        (start..start + self.config.cache.ways)
            .min_by_key(|p| {
                s.caches[core][*p]
                    .as_ref()
                    .map_or((0, 0, *p), |l| (1, l.age, *p))
            })
            .expect("nonempty cache set")
    }
    fn active_mshrs(&self, s: &State) -> usize {
        s.directory
            .values()
            .filter(|d| d.transient.is_some())
            .count()
    }
    fn start_eviction(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        core: usize,
        way: usize,
        txn: Option<usize>,
    ) -> ModelResult<bool> {
        let victim = s.caches[core][way]
            .as_ref()
            .ok_or("missing cache victim")?
            .clone();
        if s.directory
            .get(&victim.line)
            .is_some_and(|d| d.transient.is_some())
            || self.active_mshrs(s) >= self.config.cache.mshrs
        {
            return Ok(false);
        }
        let d = s.directory.entry(victim.line).or_default();
        d.epoch = d.epoch.checked_add(1).ok_or("directory epoch overflow")?;
        let epoch = d.epoch;
        d.transient = Some(Lock {
            txn: txn.unwrap_or(usize::MAX),
            epoch,
            kind: "evict".into(),
        });
        s.evictions.push(Eviction {
            core,
            way,
            line: victim.line,
            epoch,
            at: self.later(c.now(), self.config.cache.ack_cycles)?,
            txn,
        });
        self.emit(s,c,"cache_eviction_started",txn,json!({"core":self.config.cores[core].id,"line":victim.line,"state":victim.state,"epoch":epoch}))?;
        Ok(true)
    }
    pub(super) fn complete_evictions(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let events = std::mem::take(&mut s.evictions);
        for e in events {
            if e.at > c.now() {
                s.evictions.push(e);
                continue;
            }
            let victim = s.caches[e.core][e.way]
                .as_ref()
                .ok_or("lost cache eviction owner")?
                .clone();
            let d = s.directory.get(&e.line).ok_or("lost eviction directory")?;
            if victim.line != e.line
                || d.epoch != e.epoch
                || d.transient
                    .as_ref()
                    .is_none_or(|l| l.kind != "evict" || l.epoch != e.epoch)
            {
                return Err("stale/duplicate eviction acknowledgment".into());
            }
            if victim.state == Mesi::M {
                if !self.backend_write(
                    s,
                    format!("eviction:{}:{}:{}", e.core, e.line, e.epoch),
                    e.txn,
                    e.line,
                    &victim.data,
                    &vec![true; victim.data.len()],
                )? {
                    s.evictions.push(e);
                    continue;
                }
                self.emit(s,c,"cache_writeback",e.txn,json!({"line":e.line,"core":self.config.cores[e.core].id,"data":victim.data,"epoch":e.epoch}))?;
            }
            s.caches[e.core][e.way] = None;
            let d = s.directory.get_mut(&e.line).unwrap();
            d.sharers.remove(&e.core);
            if d.owner == Some(e.core) {
                d.owner = None;
            }
            d.transient = None;
            self.emit(
                s,
                c,
                "cache_eviction_completed",
                e.txn,
                json!({"line":e.line,"core":self.config.cores[e.core].id,"epoch":e.epoch}),
            )?;
        }
        Ok(())
    }
    pub(super) fn arbitrate_owner(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let mut candidates: Vec<_> = s
            .transactions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.phase == Phase::OwnerWait)
            .map(|(i, t)| {
                let wait = (c.now() - t.admitted) / self.config.cycle_ps;
                let effective =
                    (t.request.qos as u64 + wait / self.config.fabric.aging_cycles).min(15);
                (
                    i,
                    effective,
                    wait,
                    t.admitted,
                    t.request.source.clone(),
                    t.request.axi_id,
                )
            })
            .collect();
        candidates.sort_by_key(|(i, p, _, at, src, id)| {
            (std::cmp::Reverse(*p), *at, src.clone(), *id, *i)
        });
        for (i, effective, wait, _, _, _) in candidates {
            if self.active_mshrs(s) >= self.config.cache.mshrs {
                break;
            }
            let actual_hit = self.cache_hit(s, i);
            if s.transactions[i].cache_hit && !actual_hit {
                s.transactions[i].cache_hit = false;
                if s.transactions[i].request.transport == Transport::Noc
                    && !s.transactions[i].ingress_delivered
                {
                    s.transactions[i].phase = Phase::Ingress;
                    continue;
                }
            }
            let t = s.transactions[i].clone();
            if t.request.transport == Transport::Axi
                && t.request.operation != Operation::Atomic
                && s.transactions.iter().any(Self::atomic_reserved)
            {
                continue;
            }
            if (0..i).any(|j| {
                s.transactions[j].request.source == t.request.source
                    && s.transactions[j].request.axi_id == t.request.axi_id
                    && s.transactions[j].phase != Phase::Done
            }) {
                continue;
            }
            if let Some(e) = self.mailbox_index(t.request.address, t.request.length) {
                let valid = t.error.is_none() && self.mailbox_access_valid(&t);
                if valid && !self.mailbox_available(s, e) {
                    continue;
                }
                if t.request.transport != Transport::Noc && t.request.operation != Operation::Atomic
                {
                    if valid {
                        s.mailboxes[e].pending = Some(i);
                    }
                    self.submit_native(s, c, i, !valid)?;
                } else if valid {
                    s.transactions[i].phase = Phase::Peripheral;
                    self.accept_mailbox(s, c, i, None)?;
                } else {
                    s.transactions[i].error = Some("mailbox_access_error".into());
                    s.transactions[i].phase = Phase::Response;
                    s.transactions[i].response_ready = Some(c.now());
                    if t.request.operation != Operation::Write {
                        s.transactions[i].result = vec![0; t.request.length];
                        s.transactions[i].wire_data = s.transactions[i].result.clone();
                    }
                }
                continue;
            }
            let write = t.request.operation != Operation::Read;
            let fetch = matches!(t.client, Client::Fetch { .. });
            if t.error.is_some()
                || !crate::input::soc_execution::mapped(
                    &self.config,
                    t.request.address,
                    t.request.length,
                    write,
                    fetch,
                )
                || t.request.operation == Operation::Atomic
                    && !crate::input::soc_execution::mapped(
                        &self.config,
                        t.request.address,
                        4,
                        false,
                        false,
                    )
            {
                s.transactions[i]
                    .error
                    .get_or_insert_with(|| "decode_or_permission_error".into());
                if t.request.transport != Transport::Noc && t.request.operation != Operation::Atomic
                {
                    self.submit_native(s, c, i, true)?;
                    continue;
                }
                s.transactions[i].phase = Phase::Response;
                s.transactions[i].response_ready = Some(c.now());
                if t.request.operation == Operation::Read
                    || t.request.operation == Operation::Atomic
                {
                    s.transactions[i].result = vec![0; t.request.length];
                    s.transactions[i].wire_data = s.transactions[i].result.clone();
                }
                continue;
            }
            let address = t
                .request
                .address
                .checked_add(t.offset as u32)
                .ok_or("request address overflow")?;
            let line = self.line(address);
            if s.directory
                .get(&line)
                .is_some_and(|d| d.transient.is_some())
            {
                continue;
            }
            if let Some(core) = t.client.core() {
                if self.cache_position(s, core, line).is_none() {
                    let way = self.victim(s, core, line);
                    if s.caches[core][way].is_some() {
                        self.start_eviction(s, c, core, way, Some(i))?;
                        continue;
                    }
                }
            }
            let core = t.client.core();
            let d = s.directory.entry(line).or_default();
            d.epoch = d.epoch.checked_add(1).ok_or("directory epoch overflow")?;
            let epoch = d.epoch;
            d.transient = Some(Lock {
                txn: i,
                epoch,
                kind: if write { "get_m" } else { "get_s" }.into(),
            });
            let mut peers: BTreeSet<usize> = d.sharers.clone();
            if let Some(owner) = d.owner {
                peers.insert(owner);
            }
            if let Some(core) = core {
                peers.remove(&core);
            }
            s.transactions[i].phase = Phase::Coherence;
            s.transactions[i].grant_order = Some(s.sequence);
            s.transactions[i].line = Some(line);
            s.transactions[i].owner_ready = self.later(c.now(), 1)?;
            for peer in peers {
                s.transactions[i].acknowledgments.push(Ack {
                    core: peer,
                    epoch,
                    at: self.later(c.now(), self.config.cache.ack_cycles)?,
                    invalidate: write,
                });
                self.emit(
                    s,
                    c,
                    if write {
                        "invalidate_sent"
                    } else {
                        "intervention_sent"
                    },
                    Some(i),
                    json!({"core":self.config.cores[peer].id,"line":line,"epoch":epoch}),
                )?;
            }
            self.emit(s,c,"owner_grant",Some(i),json!({"base_qos":t.request.qos,"effective_qos":effective,"wait_cycles":wait,"starvation_counter":wait/self.config.fabric.aging_cycles,"rationale":"highest_effective_then_oldest_source_id_sequence","line":line,"epoch":epoch,"cache_hit":t.cache_hit}))?;
        }
        Ok(())
    }
    pub(super) fn complete_coherence(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let mut order: Vec<_> = (0..s.transactions.len()).collect();
        order.sort_by_key(|i| (s.transactions[*i].grant_order.unwrap_or(u64::MAX), *i));
        for i in order {
            if s.transactions[i].phase != Phase::Coherence {
                continue;
            }
            let line = s.transactions[i].line.ok_or("coherence without line")?;
            let acks = std::mem::take(&mut s.transactions[i].acknowledgments);
            for ack in acks {
                if ack.at > c.now() {
                    s.transactions[i].acknowledgments.push(ack);
                    continue;
                }
                if s.directory.get(&line).is_none_or(|d| {
                    d.epoch != ack.epoch
                        || d.transient
                            .as_ref()
                            .is_none_or(|l| l.txn != i || l.epoch != ack.epoch)
                }) {
                    return Err("stale/duplicate coherence acknowledgment".into());
                }
                let p = self
                    .cache_position(s, ack.core, line)
                    .ok_or("coherence acknowledgment lost cache owner")?;
                let old = s.caches[ack.core][p].as_ref().unwrap().clone();
                if old.state == Mesi::M {
                    s.transactions[i].intervention = Some(old.data.clone());
                    if !ack.invalidate {
                        if !self.backend_write(
                            s,
                            format!(
                                "ack:{i}:{}:{}:{}",
                                s.transactions[i].offset, ack.core, ack.epoch
                            ),
                            Some(i),
                            line,
                            &old.data,
                            &vec![true; old.data.len()],
                        )? {
                            s.transactions[i].acknowledgments.push(ack);
                            continue;
                        }
                        self.emit(s,c,"cache_writeback",Some(i),json!({"line":line,"core":self.config.cores[ack.core].id,"data":old.data,"reason":"owner_intervention","epoch":ack.epoch}))?;
                    }
                }
                if ack.invalidate {
                    s.caches[ack.core][p] = None;
                } else {
                    s.caches[ack.core][p].as_mut().unwrap().state = Mesi::S;
                }
                let d = s.directory.get_mut(&line).unwrap();
                if d.owner == Some(ack.core) {
                    d.owner = None;
                }
                if ack.invalidate {
                    d.sharers.remove(&ack.core);
                } else {
                    d.sharers.insert(ack.core);
                }
                self.emit(s,c,"coherence_ack",Some(i),json!({"core":self.config.cores[ack.core].id,"line":line,"epoch":ack.epoch,"invalidated":ack.invalidate,"dirty_data":if old.state==Mesi::M{Some(old.data)}else{None}}))?;
            }
            if !s.transactions[i].acknowledgments.is_empty()
                || s.transactions[i].owner_ready > c.now()
            {
                continue;
            }
            if s.transactions[i].request.transport != Transport::Noc
                && !s.transactions[i].cache_hit
                && s.transactions[i].request.operation != Operation::Atomic
            {
                self.submit_native(s, c, i, false)?;
                continue;
            }
            let t = s.transactions[i].clone();
            let req = &t.request;
            let address = req
                .address
                .checked_add(t.offset as u32)
                .ok_or("request address overflow")?;
            let at = (address - line) as usize;
            let count = (self.config.cache.line_bytes - at).min(req.length - t.offset);
            let data = if let Some(core) = t.client.core() {
                if let Some(p) = self.cache_position(s, core, line) {
                    s.caches[core][p].as_ref().unwrap().data.clone()
                } else if let Some(data) = &t.intervention {
                    data.clone()
                } else {
                    match self.backend_read(
                        s,
                        format!("read:{i}:{}", t.offset),
                        i,
                        line,
                        self.config.cache.line_bytes,
                    )? {
                        Some(data) => data,
                        None => continue,
                    }
                }
            } else if let Some(data) = &t.intervention {
                data.clone()
            } else if req.operation == Operation::Write {
                vec![0; self.config.cache.line_bytes]
            } else {
                match self.backend_read(s, format!("read:{i}:{}", t.offset), i, address, count)? {
                    Some(bytes) => {
                        let mut data = vec![0; self.config.cache.line_bytes];
                        data[at..at + count].copy_from_slice(&bytes);
                        data
                    }
                    None => continue,
                }
            };
            if s.transactions[i].error.is_some() {
                s.transactions[i].phase = Phase::Response;
                s.transactions[i].response_ready = Some(c.now());
                s.transactions[i].result = vec![0; t.request.length];
                s.transactions[i].wire_data = s.transactions[i].result.clone();
                continue;
            }
            if t.client.core().is_some() {
                if t.request.transport == Transport::Noc && !t.cache_hit {
                    s.transactions[i].wire_data = data;
                    s.transactions[i].offset = req.length;
                    s.transactions[i].intervention = None;
                    s.transactions[i].phase = Phase::Response;
                    s.transactions[i].response_ready = Some(c.now());
                    continue;
                }
                self.install_core_data(s, c, i, line, data.clone())?;
            } else {
                // Dirty owner bytes become backing bytes before an external partial write.
                if t.intervention.is_some()
                    && !self.backend_write(
                        s,
                        format!("intervention:{i}:{}", t.offset),
                        Some(i),
                        line,
                        &data,
                        &vec![true; data.len()],
                    )?
                {
                    continue;
                }
                match req.operation {
                    Operation::Read => s.transactions[i]
                        .result
                        .extend_from_slice(&data[at..at + count]),
                    Operation::Write => {
                        let enables = if req.byte_enables.is_empty() {
                            vec![true; count]
                        } else {
                            req.byte_enables[t.offset..t.offset + count].to_vec()
                        };
                        if !self.backend_write(
                            s,
                            format!("write:{i}:{}", t.offset),
                            Some(i),
                            address,
                            &req.data[t.offset..t.offset + count],
                            &enables,
                        )? {
                            continue;
                        }
                    }
                    Operation::Atomic => {
                        let old = u32::from_le_bytes(
                            data[at..at + 4]
                                .try_into()
                                .map_err(|_| ModelError("atomic spans line".into()))?,
                        );
                        let new = self.atomic(req, old)?;
                        if !self.backend_write(
                            s,
                            format!("atomic:{i}:{}", t.offset),
                            Some(i),
                            address,
                            &new.to_le_bytes(),
                            &[true; 4],
                        )? {
                            continue;
                        }
                        if req.atomic.as_ref().unwrap().awatop & 0x20 != 0 {
                            s.transactions[i].result = old.to_le_bytes().to_vec();
                        }
                        self.emit(s,c,"atomic_linearized",Some(i),json!({"awatop":req.atomic.as_ref().unwrap().awatop,"old":old,"new":new,"address":address}))?;
                    }
                }
                self.emit(s,c,"memory_visibility",Some(i),json!({"operation":req.operation,"address":address,"length":count,"data":if req.operation==Operation::Read{data[at..at+count].to_vec()}else{req.data.clone()}}))?;
            }
            s.transactions[i].offset += count;
            s.transactions[i].intervention = None;
            if s.transactions[i].offset == req.length {
                s.transactions[i].phase = Phase::Response;
                s.transactions[i].response_ready = Some(c.now());
                if s.transactions[i].wire_data.is_empty() {
                    s.transactions[i].wire_data = s.transactions[i].result.clone();
                }
            } else {
                self.release_line(s, i)?;
                s.transactions[i].phase = Phase::OwnerWait;
            }
        }
        Ok(())
    }
    pub(super) fn install_core_data(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        i: usize,
        line: u32,
        mut data: Vec<u8>,
    ) -> ModelResult {
        let t = s.transactions[i].clone();
        let req = &t.request;
        let core = t.client.core().ok_or("cache fill without core")?;
        let at = (req.address - line) as usize;
        let count = req.length;
        if data.len() != self.config.cache.line_bytes || at + count > data.len() {
            return Err("short native cache-line response".into());
        }
        if s.transactions[i].wire_data.is_empty() && !t.cache_hit {
            s.transactions[i].wire_data = data.clone();
        }
        if req.operation == Operation::Write {
            data[at..at + count].copy_from_slice(&req.data[..count]);
        } else {
            s.transactions[i]
                .result
                .extend_from_slice(&data[at..at + count]);
        }
        s.age = s.age.checked_add(1).ok_or("cache age overflow")?;
        let pos = self
            .cache_position(s, core, line)
            .unwrap_or_else(|| self.victim(s, core, line));
        let d = s.directory.get_mut(&line).unwrap();
        let state = if req.operation == Operation::Write {
            d.sharers.clear();
            d.owner = Some(core);
            Mesi::M
        } else if d.sharers.is_empty() && d.owner.is_none_or(|o| o == core) {
            d.owner = Some(core);
            if s.caches[core][pos]
                .as_ref()
                .is_some_and(|l| l.state == Mesi::M)
            {
                Mesi::M
            } else {
                Mesi::E
            }
        } else {
            d.owner = None;
            d.sharers.insert(core);
            Mesi::S
        };
        s.caches[core][pos] = Some(CacheLine {
            line,
            state,
            data: data.clone(),
            age: s.age,
        });
        self.emit(
            s,
            c,
            if t.cache_hit {
                "cache_hit"
            } else {
                "cache_fill"
            },
            Some(i),
            json!({"core":self.config.cores[core].id,"line":line,"state":state,"data":data}),
        )?;
        Ok(())
    }
    pub(super) fn release_line(&self, s: &mut State, i: usize) -> ModelResult {
        if let Some(line) = s.transactions[i].line.take() {
            let d = s
                .directory
                .get_mut(&line)
                .ok_or("missing release directory")?;
            if d.transient.as_ref().is_none_or(|l| l.txn != i) {
                return Err("directory release owner mismatch".into());
            }
            d.transient = None;
        }
        Ok(())
    }
    pub(super) fn flush_final(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        if !self.terminal_cores(s)
            || s.external.iter().any(Option::is_none)
            || s.transactions.iter().any(|t| t.phase != Phase::Done)
            || s.dma
                .iter()
                .any(|d| !matches!(d.phase, DmaPhase::Done | DmaPhase::Failed))
            || s.commands < self.workload.commands.len()
        {
            return Ok(());
        }
        for core in 0..s.caches.len() {
            for p in 0..s.caches[core].len() {
                if s.caches[core][p]
                    .as_ref()
                    .is_some_and(|l| l.state == Mesi::M)
                {
                    self.start_eviction(s, c, core, p, None)?;
                }
            }
        }
        Ok(())
    }
}
