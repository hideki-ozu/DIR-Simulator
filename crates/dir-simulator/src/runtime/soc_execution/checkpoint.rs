use super::*;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    config: Config,
    workload: Workload,
    state: State,
    timer: Option<TimerReference>,
}
impl Soc {
    fn validate_cpu_request(&self, s: &State, i: usize, core: usize) -> ModelResult {
        let t = &s.transactions[i];
        let r = &t.request;
        let cfg = self
            .config
            .cores
            .get(core)
            .ok_or("unknown CPU request core")?;
        let shape = match t.client {
            Client::Fetch { .. } => {
                r.operation == Operation::Read
                    && r.length == 4
                    && r.data.is_empty()
                    && (t.phase != Phase::Done || t.error.is_some() || t.result.len() == 4)
            }
            Client::Load { .. } => {
                r.operation == Operation::Read && matches!(r.length, 1 | 2 | 4) && r.data.is_empty()
            }
            Client::Store { .. } => {
                r.operation == Operation::Write
                    && matches!(r.length, 1 | 2 | 4)
                    && r.data.len() == r.length
            }
            _ => false,
        };
        if t.error.is_none()
            && (t.phase == Phase::Done
                || t.phase == Phase::Response
                    && (t.cache_hit || r.transport != Transport::Noc || t.response_delivered))
            && r.operation == Operation::Read
            && (t.result.len() != r.length || t.offset != r.length)
        {
            return Err("short completed CPU response".into());
        }
        if !shape
            || r.source != cfg.id
            || r.transport != cfg.transport
            || r.router != cfg.router
            || r.qos != cfg.qos
            || r.atomic.is_some()
            || r.lease.is_some()
            || !r.byte_enables.is_empty()
        {
            return Err("tampered CPU request protocol/width/operation".into());
        }
        if t.phase != Phase::Done {
            let correct = match s.cores[core].phase {
                CorePhase::Fetching { txn } => {
                    txn == i
                        && t.client == Client::Fetch { core }
                        && r.address == s.cores[core].architectural.pc
                }
                CorePhase::Memory { txn, .. } => {
                    txn == i
                        && matches!(t.client,Client::Load{core:c}|Client::Store{core:c} if c==core)
                }
                _ => false,
            };
            if !correct {
                return Err("CPU phase/client/fetch-PC mismatch".into());
            }
        }
        Ok(())
    }
    fn validate_dma_request(&self, s: &State, i: usize, index: usize) -> ModelResult {
        let t = &s.transactions[i];
        let r = &t.request;
        let d = self
            .workload
            .dma
            .get(index)
            .ok_or("unknown DMA descriptor")?;
        let read = matches!(t.client, Client::DmaRead { .. });
        let prefix = format!("dma:{}:", d.id);
        let suffix = if read { ":read" } else { ":write" };
        let offset =
            r.id.strip_prefix(&prefix)
                .and_then(|x| x.strip_suffix(suffix))
                .and_then(|n| n.parse::<usize>().ok())
                .ok_or("invalid DMA burst identity")?;
        if offset >= d.length {
            return Err("DMA burst offset outside descriptor".into());
        }
        let address = if read {
            d.source_address
        } else {
            d.destination
        }
        .checked_add(offset as u32)
        .ok_or("DMA burst address overflow")?;
        let source = d
            .source_address
            .checked_add(offset as u32)
            .ok_or("DMA source overflow")?;
        let register_dma = self.mailbox_index(d.source_address, d.length).is_some()
            || self.mailbox_index(d.destination, d.length).is_some();
        let count = if register_dma {
            4
        } else {
            (d.length - offset)
                .min(self.config.dma_buffer_bytes)
                .min(self.config.cache.line_bytes - source as usize % self.config.cache.line_bytes)
        };
        if r.source != d.source
            || r.address != address
            || r.length != count
            || r.transport != d.transport
            || r.router != d.router
            || r.qos != d.qos
            || r.axi_id != 0
            || r.atomic.is_some()
            || !r.byte_enables.is_empty()
            || r.at_ps < d.at_ps
            || r.operation
                != if read {
                    Operation::Read
                } else {
                    Operation::Write
                }
            || r.lease != if read { None } else { d.lease.clone() }
            || read && !r.data.is_empty()
        {
            return Err("DMA request disagrees with descriptor/chunk".into());
        }
        if !read {
            let pair = format!("dma:{}:{offset}:read", d.id);
            if !s.transactions.iter().any(|x| {
                x.client == Client::DmaRead { index }
                    && x.request.id == pair
                    && x.phase == Phase::Done
                    && x.error.is_none()
                    && x.result == r.data
            }) {
                return Err("DMA write lacks actual completed source capture".into());
            }
        }
        if t.phase != Phase::Done {
            let runtime = s.dma.get(index).ok_or("missing DMA runtime")?;
            let correct = match runtime.phase {
                DmaPhase::Reading { txn } => read && txn == i && offset == runtime.offset,
                DmaPhase::Writing { txn } => {
                    !read && txn == i && offset == runtime.offset && r.data == runtime.staging
                }
                _ => false,
            };
            if !correct {
                return Err("DMA stage/client/payload mismatch".into());
            }
        }
        Ok(())
    }
    pub(super) fn encode(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        self.validate_state(&self.state)?;
        serde_json::to_vec(&Checkpoint {
            version: 1,
            config: self.config.clone(),
            workload: self.workload.clone(),
            state: self.state.clone(),
            timer: self
                .timer
                .as_ref()
                .map(|t| c.encode_timer_ref(t))
                .transpose()?,
        })
        .map_err(|e| ModelError(format!("SoC checkpoint: {e}")))
    }
    fn validate_transport_bindings(&self, s: &State) -> ModelResult {
        let mut active = BTreeSet::new();
        let mut deliveries = BTreeSet::new();
        for (transport, id, data, error) in &s.native_deliveries {
            if !deliveries.insert((*transport as u8, *id)) {
                return Err("duplicate native queued completion".into());
            }
            let i = *id / 4097;
            let offset = *id % 4097;
            let t = s.transactions.get(i).ok_or("orphan native delivery")?;
            if t.phase != Phase::Native || t.offset != offset || t.request.transport != *transport {
                return Err("native queued completion lost logical owner".into());
            }
            let expected = self.native_binding(s, i, offset, *transport)?;
            if expected
                .as_ref()
                .is_none_or(|(bytes, status)| bytes != data || status != error)
            {
                return Err("tampered native completion bytes/status".into());
            }
        }
        for (transport, ids) in [(Transport::Axi, s.axi.ids()), (Transport::Ahb, s.ahb.ids())] {
            for id in ids {
                let i = id / 4097;
                let offset = id % 4097;
                let t = s.transactions.get(i).ok_or("orphan native job")?;
                if t.request.transport != transport
                    || offset >= t.request.length
                    || offset > t.offset
                {
                    return Err("native job logical identity/offset mismatch".into());
                }
                let completion = self.native_binding(s, i, offset, transport)?;
                if completion.is_none() {
                    if t.phase != Phase::Native || t.offset != offset {
                        return Err("pending native job has no logical owner".into());
                    }
                    active.insert((transport as u8, id));
                } else if t.phase == Phase::Native
                    && t.offset == offset
                    && !deliveries.contains(&(transport as u8, id))
                {
                    return Err("lost queued native completion".into());
                }
            }
        }
        for (i, t) in s
            .transactions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.phase == Phase::Native)
        {
            let id = i
                .checked_mul(4097)
                .and_then(|n| n.checked_add(t.offset))
                .ok_or("native id overflow")?;
            if !active.contains(&(t.request.transport as u8, id))
                && !deliveries.contains(&(t.request.transport as u8, id))
            {
                return Err("logical native request stranded without service owner".into());
            }
        }
        let mut seen = BTreeSet::new();
        let mut queued = BTreeSet::new();
        for (id, bytes, status) in &s.noc_deliveries {
            if !queued.insert(id.clone()) {
                return Err("duplicate queued NoC delivery".into());
            }
            let (_, terminal, payload) = s
                .noc
                .packets()
                .into_iter()
                .find(|(p, _, _)| p.id == *id)
                .ok_or("orphan queued NoC delivery")?;
            if terminal != Some(status.as_str()) || payload != bytes {
                return Err("tampered queued NoC delivery".into());
            }
        }
        for (p, status, payload) in s.noc.packets() {
            let (response, text) = if let Some(x) = p.id.strip_prefix("request-") {
                (false, x)
            } else if let Some(x) = p.id.strip_prefix("response-") {
                (true, x)
            } else {
                return Err("unknown composed NoC packet".into());
            };
            let i: usize = text
                .parse()
                .map_err(|_| ModelError("invalid NoC logical identity".into()))?;
            let t = s.transactions.get(i).ok_or("orphan NoC packet")?;
            if !seen.insert((i, response))
                || t.request.transport != Transport::Noc
                || p.source
                    != if response {
                        self.config.memory_router
                    } else {
                        t.request.router
                    }
                || p.destination
                    != if response {
                        t.request.router
                    } else {
                        self.config.memory_router
                    }
                || p.payload
                    != if response {
                        t.wire_data.clone()
                    } else {
                        self.ingress_payload(t, i)
                    }
                || status.is_some_and(|x| x != "packet_delivered")
                || status.is_some() && payload != p.payload
            {
                return Err("NoC logical packet/byte binding mismatch".into());
            }
            if response {
                if !t.response_sent
                    || status.is_none() && t.phase != Phase::Response
                    || status.is_some() && !t.response_delivered && !queued.contains(&p.id)
                {
                    return Err("NoC response lifecycle mismatch".into());
                }
            } else if !t.ingress_sent
                || status.is_none() && t.phase != Phase::Ingress
                || status.is_some() && !t.ingress_delivered && !queued.contains(&p.id)
            {
                return Err("NoC request lifecycle mismatch".into());
            }
        }
        for (i, t) in s.transactions.iter().enumerate() {
            if t.error.is_none() && t.phase == Phase::Done && !t.cache_hit {
                if t.request.transport == Transport::Noc {
                    if !t.ingress_delivered || !t.response_delivered {
                        return Err("successful NoC request lacks actual delivery".into());
                    }
                } else if t.request.operation != Operation::Atomic {
                    let mut offset = 0;
                    while offset < t.request.length {
                        if self
                            .native_binding(s, i, offset, t.request.transport)?
                            .is_none()
                        {
                            return Err("successful request lacks actual native completion".into());
                        }
                        if t.client.core().is_some() {
                            break;
                        }
                        let address = t
                            .request
                            .address
                            .checked_add(offset as u32)
                            .ok_or("native history address overflow")?;
                        offset += (self.config.cache.line_bytes
                            - address as usize % self.config.cache.line_bytes)
                            .min(t.request.length - offset);
                    }
                }
            }
            if t.ingress_sent && !seen.contains(&(i, false))
                || t.response_sent && !seen.contains(&(i, true))
            {
                return Err("logical NoC packet lost physical owner".into());
            }
        }
        Ok(())
    }
    fn native_binding(
        &self,
        s: &State,
        i: usize,
        offset: usize,
        transport: Transport,
    ) -> ModelResult<Option<(Vec<u8>, bool)>> {
        let t = &s.transactions[i];
        let mmio = self
            .mailbox_index(t.request.address, t.request.length)
            .is_some();
        let word = mmio && self.mailbox_access_valid(t);
        let core = !mmio
            && t.client.core().is_some()
            && crate::input::soc_execution::mapped(
                &self.config,
                t.request.address,
                t.request.length,
                t.request.operation != Operation::Read,
                matches!(t.client, Client::Fetch { .. }),
            );
        let address = if core {
            self.line(t.request.address)
        } else {
            t.request
                .address
                .checked_add(offset as u32)
                .ok_or("native logical address overflow")?
        };
        let length = if word {
            4
        } else if core {
            self.config.cache.line_bytes
        } else {
            (self.config.cache.line_bytes - address as usize % self.config.cache.line_bytes).min(
                t.request
                    .length
                    .checked_sub(offset)
                    .ok_or("native offset exceeds request")?,
            )
        };
        let read = core || t.request.operation == Operation::Read;
        let data = if read {
            &[][..]
        } else {
            t.request
                .data
                .get(offset..offset + length)
                .ok_or("short logical native write")?
        };
        let enables = if read || t.request.byte_enables.is_empty() {
            &[][..]
        } else {
            t.request
                .byte_enables
                .get(offset..offset + length)
                .ok_or("short logical native strobes")?
        };
        let id = i * 4097 + offset;
        match transport {
            Transport::Axi => Ok(s
                .axi
                .validate_binding(
                    id,
                    &t.request.source,
                    address,
                    length,
                    data,
                    enables,
                    read,
                    word,
                )?
                .map(|c| (c.data, c.error))),
            Transport::Ahb => Ok(s
                .ahb
                .validate_binding(
                    id,
                    &t.request.source,
                    address,
                    length,
                    data,
                    enables,
                    read,
                    word,
                )?
                .map(|c| (c.data, c.error))),
            _ => Err("NoC native binding invalid".into()),
        }
    }
    pub(super) fn validate_state(&self, s: &State) -> ModelResult {
        s.memory.validate(&memory::targets(&self.config))?;
        s.memory.validate_monitors(&[], |_| false)?;
        s.noc.validate(&self.config.noc)?;
        s.axi.validate(
            self.config.cycle_ps,
            self.config.fabric.data_bytes,
            &self.sources,
            &memory::native_targets(&self.config),
        )?;
        s.ahb.validate(
            self.config.cycle_ps,
            self.config.fabric.data_bytes,
            &self.sources,
            &memory::native_targets(&self.config),
            self.config.fabric.ahb_wait_cycles,
        )?;
        if s.cores.len() != self.config.cores.len()
            || s.caches.len() != s.cores.len()
            || s.external.len() != self.workload.requests.len()
            || s.dma.len() != self.workload.dma.len()
            || s.slots.len() != self.config.slots.len()
            || s.mailboxes.len() != self.config.mailboxes.len()
            || s.commands > self.workload.commands.len()
            || s.admission_cursor >= self.sources.len().max(1)
            || s.transactions
                .iter()
                .filter(|t| t.phase != Phase::Done)
                .count()
                > self.config.fabric.queue_capacity
            || s.directory
                .values()
                .filter(|d| d.transient.is_some())
                .count()
                > self.config.cache.mshrs
        {
            return Err("SoC checkpoint resource/input identity mismatch".into());
        }
        let last = s.last_tick;
        if let Some(i) = s.axi_write_owner {
            let t = s.transactions.get(i).ok_or("orphan AXI W owner")?;
            if t.phase != Phase::Ingress
                || t.request.transport != Transport::Axi
                || t.client.core().is_some()
                || t.request.operation == Operation::Read
                || (t.address_accepted && t.w_accepted == t.request.data.len())
            {
                return Err("invalid persistent AXI AW/W owner".into());
            }
        }
        for (i, core) in s.cores.iter().enumerate() {
            core.architectural.validate().map_err(ModelError)?;
            if core.architectural.retired > self.config.cores[i].instruction_budget
                || core.ready % self.config.cycle_ps != 0
            {
                return Err("invalid core budget/ready time".into());
            }
            match &core.phase {
                CorePhase::Fetching { txn } | CorePhase::Memory { txn, .. } => {
                    let t = s.transactions.get(*txn).ok_or("orphan CPU request")?;
                    if t.client.core() != Some(i) || t.phase == Phase::Done {
                        return Err("CPU outstanding transaction identity mismatch".into());
                    }
                }
                CorePhase::Execute { ready, .. } => {
                    if ready % self.config.cycle_ps != 0 {
                        return Err("unaligned CPU execution timer".into());
                    }
                }
                CorePhase::Halted if core.architectural.trap.is_none() => {
                    return Err("halt without contained EEI trap".into());
                }
                CorePhase::BudgetPaused
                    if core.architectural.retired != self.config.cores[i].instruction_budget
                        || core.architectural.trap.is_some() =>
                {
                    return Err("invalid core budget pause".into());
                }
                _ => {}
            }
            if let CorePhase::Memory {
                instruction,
                action,
                txn,
            } = &core.phase
            {
                if cpu::evaluate(&core.architectural, *instruction).as_ref() != Ok(action) {
                    return Err("tampered deferred CPU instruction/action".into());
                }
                let t = &s.transactions[*txn];
                let shape = match action.effect {
                    cpu::Effect::Load { address, bytes, .. } => {
                        t.request.operation == Operation::Read
                            && t.request.address == address
                            && t.request.length == bytes
                    }
                    cpu::Effect::Store {
                        address,
                        bytes,
                        value,
                        ..
                    } => {
                        t.request.operation == Operation::Write
                            && t.request.address == address
                            && t.request.length == bytes
                            && t.request.data == value.to_le_bytes()[..bytes]
                    }
                    _ => false,
                };
                if !shape {
                    return Err("CPU memory request disagrees with instruction".into());
                }
            }
            if s.caches[i].len() != self.config.cache.sets * self.config.cache.ways {
                return Err("wrong cache capacity".into());
            }
            let mut seen = BTreeSet::new();
            for (way, l) in s.caches[i].iter().enumerate() {
                if let Some(l) = l {
                    if l.line != self.line(l.line)
                        || !seen.insert(l.line)
                        || l.data.len() != self.config.cache.line_bytes
                        || l.age > s.age
                        || way / self.config.cache.ways
                            != (l.line as usize / self.config.cache.line_bytes)
                                % self.config.cache.sets
                    {
                        return Err("invalid cache tag/set/data/LRU".into());
                    }
                    let d = s.directory.get(&l.line).ok_or("orphan cache line")?;
                    if match l.state {
                        Mesi::M | Mesi::E => d.owner != Some(i) || !d.sharers.is_empty(),
                        Mesi::S => !d.sharers.contains(&i) || d.owner.is_some(),
                    } {
                        return Err("cache/directory stable owner mismatch".into());
                    }
                    if l.state != Mesi::M
                        && l.data != self.read_backing(s, usize::MAX, l.line, l.data.len())?
                    {
                        return Err(
                            "clean shared/exclusive cache bytes disagree with backing".into()
                        );
                    }
                }
            }
        }
        let mut locked = BTreeSet::new();
        for (line, d) in &s.directory {
            if *line != self.line(*line)
                || d.owner.is_some_and(|o| o >= s.cores.len())
                || d.sharers.iter().any(|p| *p >= s.cores.len())
                || d.owner.is_some() && !d.sharers.is_empty()
            {
                return Err("invalid directory stable owner/sharers".into());
            }
            if let Some(owner) = d.owner {
                let p = self
                    .cache_position(s, owner, *line)
                    .ok_or("directory owner has no cache line")?;
                if !matches!(
                    s.caches[owner][p].as_ref().unwrap().state,
                    Mesi::E | Mesi::M
                ) {
                    return Err("directory owner cache state mismatch".into());
                }
            }
            for peer in &d.sharers {
                let p = self
                    .cache_position(s, *peer, *line)
                    .ok_or("directory sharer has no cache line")?;
                if s.caches[*peer][p].as_ref().unwrap().state != Mesi::S {
                    return Err("directory sharer cache state mismatch".into());
                }
            }
            if let Some(lock) = &d.transient {
                if lock.epoch != d.epoch || !locked.insert(*line) {
                    return Err("stale directory transient epoch".into());
                }
                if lock.kind == "evict" {
                    if !s.evictions.iter().any(|e| {
                        e.line == *line
                            && e.epoch == lock.epoch
                            && e.txn.unwrap_or(usize::MAX) == lock.txn
                    }) {
                        return Err("orphan directory eviction transient".into());
                    }
                } else {
                    let t = s
                        .transactions
                        .get(lock.txn)
                        .ok_or("orphan directory request transient")?;
                    if !matches!(t.phase, Phase::Coherence | Phase::Native | Phase::Response)
                        || t.line != Some(*line)
                        || !matches!(lock.kind.as_str(), "get_s" | "get_m")
                    {
                        return Err("invalid directory transient transaction".into());
                    }
                }
            }
        }
        let mut evict_lines = BTreeSet::new();
        for e in &s.evictions {
            if e.core >= s.cores.len()
                || e.way >= s.caches[e.core].len()
                || !evict_lines.insert(e.line)
                || s.caches[e.core][e.way]
                    .as_ref()
                    .is_none_or(|l| l.line != e.line)
                || last.is_none_or(|t| e.at <= t)
                    && !self.backend_wait(s, &format!("eviction:{}:{}:{}", e.core, e.line, e.epoch))
                || e.at % self.config.cycle_ps != 0
                || s.directory.get(&e.line).is_none_or(|d| {
                    d.epoch != e.epoch || d.transient.as_ref().is_none_or(|l| l.kind != "evict")
                })
            {
                return Err("invalid/stale cache eviction ownership/timer".into());
            }
        }
        let mut active_client = BTreeSet::new();
        for (i, t) in s.transactions.iter().enumerate() {
            let r = &t.request;
            if r.length == 0
                || r.length > 4096
                || r.qos > 15
                || r.at_ps > t.admitted
                || last.is_none_or(|last| t.admitted > last)
                || t.admitted % self.config.cycle_ps != 0
                || t.offset > r.length
                || t.result.len() > r.length
                || t.w_accepted > r.data.len()
                || t.r_sent > t.wire_data.len().max(r.length).max(1)
                || t.wire_data.len() > self.config.cache.line_bytes.max(r.length)
                || t.request.router
                    >= self.config.noc.topology.width * self.config.noc.topology.height
            {
                return Err("invalid SoC transaction counters/time/shape".into());
            }
            if r.operation == Operation::Atomic && t.error.is_none() && t.phase == Phase::Done {
                let returning =
                    r.atomic.as_ref().ok_or("missing atomic opcode")?.awatop & 0x20 != 0;
                let bytes = if returning { 4 } else { 0 };
                if !t.address_accepted
                    || t.w_accepted != r.data.len()
                    || !t.b_sent
                    || t.r_sent != bytes
                    || t.result.len() != bytes
                    || t.wire_data != t.result
                {
                    return Err("atomic completed without actual AW/W/R/B shape".into());
                }
            }
            if t.phase == Phase::Done {
                if t.line.is_some() || !t.acknowledgments.is_empty() || t.lease_read_active {
                    return Err("completed transaction retains live resources".into());
                }
            } else if !active_client
                .insert(serde_json::to_string(&t.client).map_err(|e| ModelError(e.to_string()))?)
            {
                return Err("duplicate outstanding source client".into());
            }
            if let Some(line) = t.line {
                if s.directory
                    .get(&line)
                    .and_then(|d| d.transient.as_ref())
                    .is_none_or(|l| l.txn != i || l.kind == "evict")
                {
                    return Err("transaction missing directory reservation".into());
                }
            }
            let mut peers = BTreeSet::new();
            for ack in &t.acknowledgments {
                if t.phase != Phase::Coherence
                    || ack.core >= s.cores.len()
                    || !peers.insert(ack.core)
                    || last.is_none_or(|last| ack.at <= last)
                        && !self.backend_wait(
                            s,
                            &format!(
                                "ack:{i}:{}:{}:{}",
                                s.transactions[i].offset, ack.core, ack.epoch
                            ),
                        )
                    || ack.at % self.config.cycle_ps != 0
                    || t.line
                        .and_then(|line| s.directory.get(&line))
                        .is_none_or(|d| d.epoch != ack.epoch)
                    || t.line
                        .is_none_or(|line| self.cache_position(s, ack.core, line).is_none())
                {
                    return Err("invalid/stale/duplicate coherence acknowledgment".into());
                }
            }
            if t.intervention
                .as_ref()
                .is_some_and(|data| data.len() != self.config.cache.line_bytes)
                || t.response_ready
                    .is_some_and(|ready| last.is_none_or(|last| ready > last))
                || t.phase == Phase::Response && t.response_ready.is_none()
                || t.phase == Phase::Coherence && t.line.is_none()
            {
                return Err("invalid transient bytes/response lifetime".into());
            }
            match t.client {
                Client::External { index } => {
                    if self.workload.requests.get(index) != Some(r)
                        || s.external.get(index) != Some(&Some(i))
                    {
                        return Err("external immutable transaction identity mismatch".into());
                    }
                }
                Client::Fetch { core } | Client::Load { core } | Client::Store { core } => {
                    if core >= s.cores.len() || r.source != self.config.cores[core].id {
                        return Err("unknown CPU request source".into());
                    }
                    self.validate_cpu_request(s, i, core)?;
                }
                Client::DmaRead { index } | Client::DmaWrite { index } => {
                    if index >= s.dma.len() || r.source != self.workload.dma[index].source {
                        return Err("unknown DMA request source".into());
                    }
                    self.validate_dma_request(s, i, index)?;
                }
            }
        }
        for (i, external) in s.external.iter().enumerate() {
            if external.is_some_and(|txn| {
                s.transactions
                    .get(txn)
                    .is_none_or(|t| t.client != Client::External { index: i })
            }) {
                return Err("external request reference mismatch".into());
            }
        }
        for (i, d) in s.dma.iter().enumerate() {
            if d.offset > self.workload.dma[i].length
                || d.staging.len() > self.config.dma_buffer_bytes
                || d.offset + d.staging.len() > self.workload.dma[i].length
                || !matches!(d.phase, DmaPhase::Write | DmaPhase::Writing { .. })
                    && !d.staging.is_empty()
                || matches!(d.phase, DmaPhase::Done) && d.offset != self.workload.dma[i].length
            {
                return Err("invalid DMA captured/completed byte accounting".into());
            }
            let mut completed = vec![false; d.offset];
            for t in &s.transactions {
                if t.client == (Client::DmaWrite { index: i })
                    && t.phase == Phase::Done
                    && t.error.is_none()
                {
                    let start = t
                        .request
                        .address
                        .checked_sub(self.workload.dma[i].destination)
                        .ok_or("DMA completed destination precedes descriptor")?
                        as usize;
                    let end = start
                        .checked_add(t.request.length)
                        .ok_or("DMA completed range overflow")?;
                    let range = completed
                        .get_mut(start..end)
                        .ok_or("DMA completion exceeds actual destination offset")?;
                    if range.iter().any(|x| *x) {
                        return Err("duplicate DMA completed destination range".into());
                    }
                    range.fill(true);
                }
            }
            if completed.iter().any(|x| !*x) {
                return Err("DMA destination offset lacks actual completed writes".into());
            }
            if matches!(d.phase, DmaPhase::Write | DmaPhase::Writing { .. }) {
                let id = format!("dma:{}:{}:read", self.workload.dma[i].id, d.offset);
                if !s.transactions.iter().any(|t| {
                    t.client == (Client::DmaRead { index: i })
                        && t.request.id == id
                        && t.phase == Phase::Done
                        && t.error.is_none()
                        && t.result == d.staging
                }) {
                    return Err("DMA staging lacks actual captured source bytes".into());
                }
            }
            match d.phase{DmaPhase::Reading{txn}|DmaPhase::Writing{txn}=>if s.transactions.get(txn).is_none_or(|t|t.phase==Phase::Done||!matches!(t.client,Client::DmaRead{index}|Client::DmaWrite{index} if index==i)){return Err("orphan DMA burst".into());},_=>{}}
        }
        self.validate_mailboxes(s)?;
        self.validate_ipc_history(s)?;
        for (i, slot) in s.slots.iter().enumerate() {
            let reads = s
                .transactions
                .iter()
                .filter(|t| {
                    t.lease_read_active
                        && t.request.lease.as_ref().is_some_and(|l| {
                            l.slot == self.config.slots[i].id && l.generation == slot.generation
                        })
                })
                .count();
            let mut expected = vec![false; slot.length];
            let lease_commands=self.workload.commands.iter().take(s.commands).filter(|c|matches!(&c.action,CommandOp::Lease{slot:id,..} if id==&self.config.slots[i].id)).count() as u64;
            let mailbox_leases = s
                .mailbox_accesses
                .iter()
                .filter(|(txn, a)| {
                    a.applied
                        && a.result == mailbox::ResultCode::Okay
                        && self.config.mailboxes[a.endpoint].slot == self.config.slots[i].id
                        && s.transactions[**txn].request.operation == Operation::Write
                        && s.transactions[**txn].request.address
                            == self.config.mailboxes[a.endpoint].base + 24
                        && s.transactions[**txn].request.data == 1u32.to_le_bytes()
                })
                .count() as u64;
            if slot.generation > lease_commands + mailbox_leases {
                return Err("lease generation exceeds actual lease commands".into());
            }
            if slot.phase != SlotPhase::Free {
                for t in &s.transactions {
                    if t.phase == Phase::Done
                        && t.error.is_none()
                        && t.request.operation == Operation::Write
                        && t.request.lease.as_ref().is_some_and(|l| {
                            l.slot == self.config.slots[i].id
                                && l.generation == slot.generation
                                && Some(l.owner.as_str()) == slot.owner.as_deref()
                        })
                    {
                        let start = t
                            .request
                            .address
                            .checked_sub(self.config.slots[i].base)
                            .ok_or("lease write precedes slot")?
                            as usize;
                        let end = start
                            .checked_add(t.request.length)
                            .ok_or("lease range overflow")?;
                        let destination = expected
                            .get_mut(start..end)
                            .ok_or("lease completed write exceeds slot")?;
                        for (n, byte) in destination.iter_mut().enumerate() {
                            if t.request.byte_enables.is_empty() || t.request.byte_enables[n] {
                                *byte = true;
                            }
                        }
                    }
                }
                if expected != slot.completed {
                    return Err("lease byte completion lacks actual successful writes".into());
                }
            }
            if slot.length > self.config.slots[i].capacity
                || slot.completed.len() != slot.length
                || slot.active_reads != reads
                || slot.phase == SlotPhase::Free
                    && (slot.owner.is_some() || slot.consumer.is_some() || slot.length != 0)
                || slot.phase != SlotPhase::Free
                    && (slot.owner.is_none() || slot.generation == 0 || slot.length == 0)
                || matches!(slot.phase, SlotPhase::Ready | SlotPhase::Reading)
                    && slot.completed.iter().any(|b| !*b)
                || slot.phase == SlotPhase::Reading && slot.consumer.is_none()
            {
                return Err("invalid IPC lease phase/generation/completion accounting".into());
            }
        }
        self.validate_transport_bindings(s)?;
        self.validate_ddr(s)?;
        if s.last_tick.is_some_and(|t| t % self.config.cycle_ps != 0)
            || s.next_tick.is_some_and(|t| {
                t % self.config.cycle_ps != 0 || s.last_tick.is_some_and(|last| t <= last)
            })
            || s.last_tick.is_some() && s.next_tick != self.next_time(s, s.last_tick.unwrap())?
        {
            return Err("invalid SoC checkpoint timer deadline".into());
        }
        Ok(())
    }
}
pub(super) fn restore(
    c: &ModelConfig,
    version: u32,
    bytes: &[u8],
    context: &CheckpointDecodeContext<'_>,
) -> ModelResult<Box<dyn Model>> {
    if version != 1 {
        return Err("unsupported SoC execution checkpoint codec".into());
    }
    let saved: Checkpoint =
        serde_json::from_slice(bytes).map_err(|e| ModelError(format!("SoC checkpoint: {e}")))?;
    let (config, workload) = crate::input::soc_execution::parse(&c.profile_input)?;
    if saved.version != 1 || saved.config != config || saved.workload != workload {
        return Err("SoC checkpoint immutable input identity mismatch".into());
    }
    let timer = saved
        .timer
        .as_ref()
        .map(|t| context.decode_timer_ref(t))
        .transpose()?;
    if timer.is_some() != saved.state.next_tick.is_some()
        || saved.timer.as_ref().is_some_and(|t| t.state != "live")
    {
        return Err("SoC checkpoint timer ownership mismatch".into());
    }
    if let Some(timer) = &timer {
        let e = context.timer_envelope(timer)?;
        if Some(e.time_ps) != saved.state.next_tick
            || e.schema != tick_schema()
            || !e.payload.is_empty()
            || e.source != c.id
            || e.destination != c.id
        {
            return Err("SoC checkpoint timer envelope mismatch".into());
        }
    }
    let soc = Soc {
        sources: source_ids(&config, &workload),
        config,
        workload,
        state: saved.state,
        timer,
    };
    soc.validate_state(&soc.state)?;
    Ok(Box::new(soc))
}
