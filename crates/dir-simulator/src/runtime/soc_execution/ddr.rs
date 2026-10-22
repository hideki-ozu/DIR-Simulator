//! Asynchronous DDR target bridge. The physical controller owns no backing bytes.
use super::*;
use crate::runtime::ddr4 as physical;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Job {
    request: physical::Request,
    epoch: u64,
    native: Option<(Transport, String, usize)>,
    enqueued: bool,
    done: bool,
    data: Vec<u8>,
    seen: Vec<bool>,
    error: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bridge {
    pub service: physical::Service,
    jobs: BTreeMap<String, Job>,
    epoch: u64,
    pub valid: Vec<bool>,
}
impl Bridge {
    pub(super) fn reset_epoch(&self) -> u64 {
        self.epoch
    }
    pub fn new(c: &DdrConfig) -> ModelResult<Self> {
        Ok(Self {
            service: physical::Service::new(physical::Config {
                mode_id: c.mode_id.clone(),
                source_sha256: c.source_sha256.clone(),
                base: c.base,
                size: c.size,
                queue_capacity: c.queue_capacity,
            })
            .map_err(|e| ModelError(format!("DDR: {e:?}")))?,
            jobs: BTreeMap::new(),
            epoch: 0,
            valid: vec![false; c.size as usize],
        })
    }
    fn insert(
        &mut self,
        key: String,
        address: u64,
        read: bool,
        data: Vec<u8>,
        enables: Vec<bool>,
        native: Option<(Transport, String, usize)>,
    ) -> ModelResult {
        let length = data.len();
        let operation = if read {
            physical::Operation::Read { length }
        } else {
            physical::Operation::Write {
                data,
                byte_enables: enables,
            }
        };
        let request = physical::Request {
            id: key.clone(),
            address,
            operation,
        };
        if let Some(saved) = self.jobs.get(&key) {
            if saved.request != request || saved.native != native || saved.epoch != self.epoch {
                return Err("DDR job key reused with different request/owner/epoch".into());
            }
            return Ok(());
        }
        self.jobs.insert(
            key.clone(),
            Job {
                request,
                epoch: self.epoch,
                native,
                enqueued: false,
                done: false,
                data: vec![0; length],
                seen: vec![false; length],
                error: false,
            },
        );
        Ok(())
    }
}
impl Soc {
    fn validate_coherent_ddr_job(&self, s: &State, key: &str, job: &Job) -> ModelResult {
        let fields: Vec<_> = key.split(':').collect();
        let number = |index: usize| -> ModelResult<u64> {
            let text = fields.get(index).ok_or("missing DDR job key field")?;
            let number = text
                .parse::<u64>()
                .map_err(|_| ModelError("invalid DDR job key number".into()))?;
            if number.to_string() != *text {
                return Err("noncanonical DDR job key number".into());
            }
            Ok(number)
        };
        let owner =
            usize::try_from(number(1)?).map_err(|_| ModelError("DDR job owner overflow".into()))?;
        let mut expected_data: Option<Vec<u8>> = None;
        let (address, length, read, enables) = if fields[0] == "eviction" {
            if fields.len() != 4 || owner >= s.cores.len() {
                return Err("invalid DDR eviction key/core".into());
            }
            let line = u32::try_from(number(2)?)
                .map_err(|_| ModelError("DDR eviction line overflow".into()))?;
            let epoch = number(3)?;
            let directory = s
                .directory
                .get(&line)
                .ok_or("DDR eviction lost directory history")?;
            if self.line(line) != line
                || !self.is_ddr(line)
                || epoch == 0
                || epoch > directory.epoch
            {
                return Err("DDR eviction line/epoch mismatch".into());
            }
            if !job.done {
                let eviction = s
                    .evictions
                    .iter()
                    .find(|e| e.core == owner && e.line == line && e.epoch == epoch)
                    .ok_or("DDR eviction lost live owner")?;
                let victim = s
                    .caches
                    .get(owner)
                    .and_then(|c| c.get(eviction.way))
                    .and_then(Option::as_ref)
                    .ok_or("DDR eviction lost cache bytes")?;
                if victim.line != line
                    || victim.state != Mesi::M
                    || directory
                        .transient
                        .as_ref()
                        .is_none_or(|l| l.kind != "evict" || l.epoch != epoch)
                {
                    return Err("DDR eviction cache/directory owner mismatch".into());
                }
                expected_data = Some(victim.data.clone());
            }
            (
                line,
                self.config.cache.line_bytes,
                false,
                vec![true; self.config.cache.line_bytes],
            )
        } else {
            let ack = fields[0] == "ack";
            if fields.len() != if ack { 5 } else { 3 }
                || !matches!(
                    fields[0],
                    "read" | "write" | "atomic" | "intervention" | "ack"
                )
            {
                return Err("unknown DDR coherence job key".into());
            }
            let offset = usize::try_from(number(2)?)
                .map_err(|_| ModelError("DDR chunk offset overflow".into()))?;
            let t = s
                .transactions
                .get(owner)
                .ok_or("orphan DDR coherence job")?;
            let request = &t.request;
            if offset >= request.length
                || offset > t.offset
                || t.client.core().is_some() && offset != 0
            {
                return Err("DDR job offset outside actual logical chunk".into());
            }
            let address = request
                .address
                .checked_add(
                    u32::try_from(offset).map_err(|_| ModelError("DDR offset overflow".into()))?,
                )
                .ok_or("DDR logical address overflow")?;
            let line = self.line(address);
            let count = (self.config.cache.line_bytes
                - address as usize % self.config.cache.line_bytes)
                .min(request.length - offset);
            if !self.is_ddr(address) || offset != 0 && address != line {
                return Err("DDR job is not a mapped logical chunk boundary".into());
            }
            if !job.done
                && (t.phase != Phase::Coherence
                    || t.offset != offset
                    || t.line != Some(line)
                    || s.directory
                        .get(&line)
                        .and_then(|d| d.transient.as_ref())
                        .is_none_or(|l| l.txn != owner || l.kind == "evict"))
            {
                return Err("DDR job lacks live logical directory owner".into());
            }
            match fields[0] {
                "read" => {
                    if t.client.core().is_some() {
                        if request.transport != Transport::Noc || t.cache_hit {
                            return Err("DDR direct cache read bypasses required native adapter or cache hit".into());
                        }
                    } else if !(request.operation == Operation::Atomic
                        && request.transport == Transport::Axi
                        || request.operation == Operation::Read
                            && request.transport == Transport::Noc)
                    {
                        return Err("DDR read has no direct logical transport owner".into());
                    }
                    if t.client.core().is_some() {
                        (line, self.config.cache.line_bytes, true, vec![])
                    } else {
                        (address, count, true, vec![])
                    }
                }
                "write" => {
                    if t.client.core().is_some()
                        || request.operation != Operation::Write
                        || request.transport != Transport::Noc
                    {
                        return Err("DDR direct write lost uncached NoC owner".into());
                    }
                    expected_data = Some(
                        request
                            .data
                            .get(offset..offset + count)
                            .ok_or("short logical DDR write")?
                            .to_vec(),
                    );
                    let enables = if request.byte_enables.is_empty() {
                        vec![true; count]
                    } else {
                        request
                            .byte_enables
                            .get(offset..offset + count)
                            .ok_or("short DDR byte enables")?
                            .to_vec()
                    };
                    (address, count, false, enables)
                }
                "atomic" => {
                    if request.operation != Operation::Atomic || offset != 0 || count != 4 {
                        return Err("DDR atomic request/chunk mismatch".into());
                    }
                    let old = s
                        .ddr
                        .as_ref()
                        .and_then(|d| d.jobs.get(&format!("read:{owner}:0")))
                        .filter(|j| j.done && !j.error)
                        .map(|j| j.data.as_slice())
                        .or_else(|| {
                            t.intervention.as_deref().and_then(|data| {
                                let at = (address - line) as usize;
                                data.get(at..at + 4)
                            })
                        })
                        .or_else(|| {
                            (job.done && t.result.len() == 4).then_some(t.result.as_slice())
                        });
                    if let Some(old) = old {
                        let old = u32::from_le_bytes(old.try_into().map_err(|_| {
                            ModelError("short captured DDR atomic old word".into())
                        })?);
                        expected_data = Some(self.atomic(request, old)?.to_le_bytes().to_vec());
                    } else if !job.done {
                        return Err("DDR atomic write has no captured old word".into());
                    }
                    (address, 4, false, vec![true; 4])
                }
                "intervention" => {
                    if t.client.core().is_some() && request.transport == Transport::Noc {
                        return Err(
                            "NoC cache fill cannot own direct intervention writeback".into()
                        );
                    }
                    if let Some(data) = &t.intervention {
                        if !job.done || t.offset == offset {
                            expected_data = Some(data.clone());
                        }
                    } else if !job.done {
                        return Err("DDR intervention lost dirty byte capture".into());
                    }
                    (
                        line,
                        self.config.cache.line_bytes,
                        false,
                        vec![true; self.config.cache.line_bytes],
                    )
                }
                "ack" => {
                    let core = usize::try_from(number(3)?)
                        .map_err(|_| ModelError("DDR ACK core overflow".into()))?;
                    let epoch = number(4)?;
                    if request.operation != Operation::Read
                        || core >= s.cores.len()
                        || epoch == 0
                        || s.directory.get(&line).is_none_or(|d| epoch > d.epoch)
                    {
                        return Err("DDR ACK core/line/epoch mismatch".into());
                    }
                    if !job.done {
                        let ack = t
                            .acknowledgments
                            .iter()
                            .find(|a| a.core == core && a.epoch == epoch)
                            .ok_or("DDR ACK lost live acknowledgment")?;
                        if ack.invalidate {
                            return Err("DDR downgrade writeback bound to invalidate ACK".into());
                        }
                        let position = self
                            .cache_position(s, core, line)
                            .ok_or("DDR ACK lost cache owner")?;
                        let cache = s.caches[core][position]
                            .as_ref()
                            .ok_or("DDR ACK lost cache line")?;
                        if cache.state != Mesi::M {
                            return Err("DDR ACK has no dirty owner".into());
                        }
                        expected_data = Some(cache.data.clone());
                    }
                    (
                        line,
                        self.config.cache.line_bytes,
                        false,
                        vec![true; self.config.cache.line_bytes],
                    )
                }
                _ => return Err("unknown DDR owner kind".into()),
            }
        };
        if job.request.address != address as u64 || job.request.length() != length {
            return Err("DDR physical request redirected from actual owner range".into());
        }
        match &job.request.operation {
            physical::Operation::Read { .. } if read => {}
            physical::Operation::Write { data, byte_enables } if !read => {
                if *byte_enables != enables
                    || expected_data
                        .as_ref()
                        .is_some_and(|expected| expected != data)
                {
                    return Err("DDR owner write bytes/enables mismatch".into());
                }
            }
            _ => return Err("DDR physical operation differs from actual owner".into()),
        }
        Ok(())
    }
    pub(super) fn validate_ddr(&self, s: &State) -> ModelResult {
        let Some(d) = &s.ddr else {
            if self.config.ddr.is_some() {
                return Err("missing DDR checkpoint owner".into());
            }
            return Ok(());
        };
        let cfg = self
            .config
            .ddr
            .as_ref()
            .ok_or("unexpected DDR checkpoint owner")?;
        if d.service.device.config
            != (physical::Config {
                mode_id: cfg.mode_id.clone(),
                source_sha256: cfg.source_sha256.clone(),
                base: cfg.base,
                size: cfg.size,
                queue_capacity: cfg.queue_capacity,
            })
            || d.valid.len() != cfg.size as usize
            || s.last_tick.is_some_and(|at| d.service.now_ps() != at)
        {
            return Err("DDR checkpoint identity/clock/validity length mismatch".into());
        }
        d.service
            .validate()
            .map_err(|e| ModelError(format!("DDR checkpoint: {e}")))?;
        let pending: BTreeMap<_, _> = d.service.requests().map(|r| (r.id.as_str(), r)).collect();
        let mut expected = vec![false; d.valid.len()];
        for (key, j) in &d.jobs {
            let length = j.request.length();
            let end = j
                .request
                .address
                .checked_add(length as u64)
                .ok_or("DDR job address overflow")?;
            if j.request.id != *key
                || length == 0
                || length > 65536
                || j.data.len() != length
                || j.seen.len() != length
                || j.epoch > d.epoch
                || j.request.address < cfg.base
                || end > cfg.base + cfg.size
                || !j.enqueued && j.seen.iter().any(|x| *x)
                || j.done && !j.error && j.seen.iter().any(|x| !*x)
                || j.enqueued
                    && !j.done
                    && pending.get(key.as_str()).is_none_or(|r| **r != j.request)
                || (!j.enqueued || j.done) && pending.contains_key(key.as_str())
            {
                return Err("DDR bridge byte/request/queue conservation mismatch".into());
            }
            if let physical::Operation::Write { data, byte_enables } = &j.request.operation {
                if data.len() != length
                    || byte_enables.len() != length
                    || j.seen
                        .iter()
                        .enumerate()
                        .any(|(i, seen)| *seen && j.data[i] != data[i])
                {
                    return Err("DDR captured write byte mismatch".into());
                }
                if j.epoch == d.epoch {
                    for i in 0..length {
                        if j.seen[i] && byte_enables[i] {
                            expected[(j.request.address - cfg.base) as usize + i] = true;
                        }
                    }
                }
            }
            if let Some((transport, native, offset)) = &j.native {
                let expected_key = format!("native:{transport:?}:{native}:{offset}");
                if *key != expected_key {
                    return Err("DDR native job identity mismatch".into());
                }
            } else {
                self.validate_coherent_ddr_job(s, key, j)?;
            }
        }
        let mut native_keys = BTreeSet::new();
        for (transport, targets) in [
            (Transport::Axi, s.axi.target_states()),
            (Transport::Ahb, s.ahb.target_states()),
        ] {
            for (target, completed, captured, error) in targets {
                if self
                    .mailbox_index(target.address as u32, target.data.len())
                    .is_some()
                {
                    continue;
                }
                let key = format!("native:{transport:?}:{}:{}", target.key, target.offset);
                let j = d
                    .jobs
                    .get(&key)
                    .ok_or("native physical target lost DDR job")?;
                native_keys.insert(key);
                let operation = if target.read {
                    physical::Operation::Read {
                        length: target.data.len(),
                    }
                } else {
                    physical::Operation::Write {
                        data: target.data,
                        byte_enables: target.enables,
                    }
                };
                if j.request.address != target.address
                    || j.request.operation != operation
                    || j.native.as_ref() != Some(&(transport, target.key, target.offset))
                    || j.done && !j.error && (completed.iter().any(|x| !*x) || captured != j.data)
                    || !j.done && completed.iter().any(|x| *x)
                    || j.done && j.error && !error
                {
                    return Err("DDR/native request/completion byte binding mismatch".into());
                }
            }
        }
        if d.jobs
            .iter()
            .any(|(key, j)| j.native.is_some() && !native_keys.contains(key))
        {
            return Err("orphan DDR native target job".into());
        }
        if pending.keys().any(|key| !d.jobs.contains_key(*key)) || expected != d.valid {
            return Err(
                "DDR bridge pending ownership/initialized-byte conservation mismatch".into(),
            );
        }
        Ok(())
    }
    pub(super) fn backend_wait(&self, s: &State, key: &str) -> bool {
        s.ddr
            .as_ref()
            .is_some_and(|d| d.jobs.get(key).is_some_and(|j| !j.done))
    }
    pub(super) fn transaction_backend_wait(&self, s: &State, i: usize) -> bool {
        s.ddr.as_ref().is_some_and(|d| {
            d.jobs.iter().any(|(key, j)| {
                !j.done
                    && (key.starts_with(&format!("ack:{i}:"))
                        || key.starts_with(&format!("intervention:{i}:"))
                        || key.starts_with(&format!("read:{i}:"))
                        || key.starts_with(&format!("write:{i}:"))
                        || key.starts_with(&format!("atomic:{i}:")))
            })
        })
    }
    pub(super) fn is_ddr(&self, address: u32) -> bool {
        self.config.memory.iter().any(|r| {
            r.backend == Backend::DdrStrict
                && address >= r.base
                && (address as u64) < r.base as u64 + r.size as u64
        })
    }
    pub(super) fn backend_read(
        &self,
        s: &mut State,
        key: String,
        txn: usize,
        address: u32,
        length: usize,
    ) -> ModelResult<Option<Vec<u8>>> {
        if !self.is_ddr(address) {
            return self.read_backing(s, txn, address, length).map(Some);
        }
        let d = s.ddr.as_mut().ok_or("missing DDR owner")?;
        d.insert(
            key.clone(),
            address as u64,
            true,
            vec![0; length],
            vec![],
            None,
        )?;
        let job = &d.jobs[&key];
        if !job.done {
            return Ok(None);
        }
        if job.error {
            s.transactions[txn].error = Some("undefined_or_reset_ddr_bytes".into());
        }
        Ok(Some(job.data.clone()))
    }
    pub(super) fn backend_write(
        &self,
        s: &mut State,
        key: String,
        txn: Option<usize>,
        address: u32,
        data: &[u8],
        enables: &[bool],
    ) -> ModelResult<bool> {
        if !self.is_ddr(address) {
            self.write_backing(s, txn, address, data, enables)?;
            return Ok(true);
        }
        let d = s.ddr.as_mut().ok_or("missing DDR owner")?;
        d.insert(
            key.clone(),
            address as u64,
            false,
            data.to_vec(),
            if enables.is_empty() {
                vec![true; data.len()]
            } else {
                enables.to_vec()
            },
            None,
        )?;
        let job = &d.jobs[&key];
        if job.error {
            return Err("DDR write reset/abort".into());
        }
        Ok(job.done)
    }
    pub(super) fn collect_native_targets(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        for (transport, requests) in [
            (Transport::Axi, s.axi.take_target_requests()),
            (Transport::Ahb, s.ahb.take_target_requests()),
        ] {
            for r in requests {
                if self.mailbox_index(r.address as u32, r.data.len()).is_some() {
                    let id = r
                        .key
                        .strip_prefix("soc-")
                        .and_then(|k| k.split('-').next())
                        .and_then(|k| k.parse::<usize>().ok())
                        .ok_or("invalid mailbox native identity")?;
                    let i = id / 4097;
                    if id % 4097 != 0
                        || r.offset != 0
                        || r.data.len() != 4
                        || s.transactions.get(i).is_none_or(|t| {
                            t.request.address as u64 != r.address
                                || (t.request.operation == Operation::Read) != r.read
                        })
                    {
                        return Err("mailbox native target whole-word binding mismatch".into());
                    }
                    self.accept_mailbox(s, c, i, Some((transport, r.key, r.offset)))?;
                    continue;
                }
                let key = format!("native:{transport:?}:{}:{}", r.key, r.offset);
                s.ddr
                    .as_mut()
                    .ok_or("asynchronous native target without DDR owner")?
                    .insert(
                        key,
                        r.address,
                        r.read,
                        r.data,
                        r.enables,
                        Some((transport, r.key, r.offset)),
                    )?;
            }
        }
        Ok(())
    }
    pub(super) fn advance_ddr(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let Some(mut d) = s.ddr.take() else {
            return Ok(());
        };
        for job in d.jobs.values_mut().filter(|j| !j.enqueued && !j.done) {
            if d.service.len() >= self.config.ddr.as_ref().unwrap().queue_capacity {
                break;
            }
            d.service
                .enqueue(job.request.clone())
                .map_err(|e| ModelError(format!("DDR enqueue: {e:?}")))?;
            job.enqueued = true;
        }
        let effects = d
            .service
            .advance(c.now())
            .map_err(|e| ModelError(format!("DDR advance: {e:?}")))?;
        for effect in effects {
            self.apply_ddr_effect(s, &mut d, c, effect)?;
        }
        s.ddr = Some(d);
        Ok(())
    }
    pub(super) fn apply_ddr_effect(
        &self,
        s: &mut State,
        d: &mut Bridge,
        c: &mut Context<'_>,
        effect: physical::Effect,
    ) -> ModelResult {
        let now = effect.at_ps();
        if now != c.now() {
            return Err("DDR effects must be observed at their exact physical deadline".into());
        }
        match &effect {
            physical::Effect::ResetInvalidation { .. } => {
                self.record_ipc(
                    s,
                    c,
                    ipc::Origin::Reset {
                        command: s
                            .commands
                            .checked_sub(1)
                            .ok_or("reset lacks command owner")?,
                    },
                )?;
                d.epoch = d.epoch.checked_add(1).ok_or("DDR epoch overflow")?;
                d.valid.fill(false);
                let cfg = self.config.ddr.as_ref().unwrap();
                let region = s
                    .memory
                    .regions
                    .iter_mut()
                    .find(|r| r.base == cfg.base)
                    .ok_or("DDR region missing")?;
                region.bytes.fill(0);
                for core in &mut s.caches {
                    for line in core {
                        if line.as_ref().is_some_and(|l| self.is_ddr(l.line)) {
                            *line = None;
                        }
                    }
                }
                for (line, entry) in &mut s.directory {
                    if self.is_ddr(*line) {
                        entry.owner = None;
                        entry.sharers.clear();
                        if entry.transient.as_ref().is_some_and(|l| l.kind == "evict") {
                            entry.transient = None;
                        }
                    }
                }
                s.evictions.retain(|e| !self.is_ddr(e.line));
                for (endpoint, config) in s.mailboxes.iter_mut().zip(&self.config.mailboxes) {
                    if self
                        .config
                        .slots
                        .iter()
                        .any(|slot| slot.id == config.slot && self.is_ddr(slot.base))
                    {
                        endpoint.notification = None;
                    }
                }
                for i in 0..s.slots.len() {
                    if self.is_ddr(self.config.slots[i].base) && s.slots[i].phase != SlotPhase::Free
                    {
                        let generation = s.slots[i].generation;
                        s.slots[i] = Slot {
                            generation,
                            owner: None,
                            consumer: None,
                            phase: SlotPhase::Free,
                            length: 0,
                            completed: vec![],
                            active_reads: 0,
                        };
                        self.emit(s,c,"slot_invalidated",None,json!({"slot":self.config.slots[i].id,"generation":generation,"reason":"ddr_reset_data_loss"}))?;
                    }
                }
                for job in d.jobs.values_mut() {
                    if !job.done {
                        job.done = true;
                        job.error = true;
                    }
                }
                s.axi.abort_targets(cfg.base, cfg.size);
                s.ahb.abort_targets(cfg.base, cfg.size);
                for t in &mut s.transactions {
                    if self.is_ddr(t.request.address) && t.phase != Phase::Done {
                        t.error = Some("ddr_reset_data_loss".into());
                        t.lease_read_active = false;
                        t.acknowledgments.clear();
                        t.intervention = None;
                        if !matches!(
                            t.phase,
                            Phase::Native | Phase::Queued | Phase::Ingress | Phase::OwnerWait
                        ) {
                            t.phase = Phase::Response;
                            t.response_ready = Some(c.now());
                            t.result = if t.request.operation == Operation::Read {
                                vec![0; t.request.length]
                            } else {
                                vec![]
                            };
                            if !t.response_sent {
                                t.wire_data = t.result.clone();
                            }
                        }
                    }
                }
            }
            physical::Effect::ReadByte {
                request,
                address,
                logical_offset: Some(offset),
                ..
            } => {
                let job = d.jobs.get_mut(request).ok_or("orphan physical read byte")?;
                if *offset >= job.data.len() || job.seen[*offset] {
                    return Err("duplicate/out of range DDR read byte".into());
                }
                let valid = d.valid[(*address - self.config.ddr.as_ref().unwrap().base) as usize];
                let bytes = s
                    .memory
                    .read(&MemoryRequest {
                        source: "ddr",
                        request_id: request,
                        id: 0,
                        address: *address,
                        data: &[0],
                        byte_enables: &[],
                    })?
                    .data;
                job.data[*offset] = bytes[0];
                job.error |= !valid;
                job.seen[*offset] = true;
            }
            physical::Effect::WriteByte {
                request,
                address,
                value,
                byte_enable,
                logical_offset: Some(offset),
                ..
            } => {
                let job = d
                    .jobs
                    .get_mut(request)
                    .ok_or("orphan physical write byte")?;
                if *offset >= job.data.len() || job.seen[*offset] {
                    return Err("duplicate/out of range DDR write byte".into());
                }
                if *byte_enable {
                    s.memory.write(&MemoryRequest {
                        source: "ddr",
                        request_id: request,
                        id: 0,
                        address: *address,
                        data: &[*value],
                        byte_enables: &[true],
                    })?;
                    d.valid[(*address - self.config.ddr.as_ref().unwrap().base) as usize] = true;
                }
                job.data[*offset] = *value;
                job.seen[*offset] = true;
            }
            physical::Effect::RequestComplete { request, .. } => {
                let j = d.jobs.get_mut(request).ok_or("orphan DDR completion")?;
                if j.done || j.seen.iter().any(|x| !*x) {
                    return Err(
                        "DDR request completed without all actual logical byte effects".into(),
                    );
                }
                j.done = true;
                if let Some((transport, key, offset)) = &j.native {
                    match transport {
                        Transport::Axi => s.axi.complete_target(key, *offset, &j.data, j.error)?,
                        Transport::Ahb => s.ahb.complete_target(key, *offset, &j.data, j.error)?,
                        _ => return Err("invalid native DDR transport".into()),
                    }
                }
            }
            physical::Effect::RequestAborted { request, .. } => {
                let j = d.jobs.get_mut(request).ok_or("orphan DDR abort")?;
                j.done = true;
                j.error = true;
            }
            _ => {}
        }
        self.emit(s, c, "ddr_effect", None, json!({"effect":effect}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_backend_key_requires_identical_request_owner_and_epoch() {
        let mut bridge = Bridge::new(&DdrConfig {
            mode_id: physical::MODE.into(),
            source_sha256: physical::SOURCE_SHA256.into(),
            base: 4096,
            size: 1024,
            queue_capacity: 4,
        })
        .unwrap();
        let insert = |b: &mut Bridge, address, length, native| {
            b.insert(
                "read:0:0".into(),
                address,
                true,
                vec![0; length],
                vec![],
                native,
            )
        };
        insert(&mut bridge, 4096, 4, None).unwrap();
        insert(&mut bridge, 4096, 4, None).unwrap();
        assert!(insert(&mut bridge, 4128, 4, None).is_err());
        assert!(insert(&mut bridge, 4096, 8, None).is_err());
        assert!(
            insert(
                &mut bridge,
                4096,
                4,
                Some((Transport::Axi, "soc-0".into(), 0))
            )
            .is_err()
        );
        assert!(
            bridge
                .insert(
                    "read:0:0".into(),
                    4096,
                    false,
                    vec![0; 4],
                    vec![true; 4],
                    None
                )
                .is_err()
        );
        bridge.epoch += 1;
        assert!(insert(&mut bridge, 4096, 4, None).is_err());
        assert_eq!(bridge.jobs.len(), 1);
    }
}
