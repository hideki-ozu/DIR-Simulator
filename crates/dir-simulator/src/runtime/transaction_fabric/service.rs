//! Embedded production AXI4 channel engine sharing its caller's only backing store.
use super::*;
/// Completion state supplied by an asynchronous target; it never owns memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AsyncTarget {
    pub requested: Vec<bool>,
    pub completed: Vec<bool>,
    pub data: Vec<u8>,
    pub error: bool,
}
impl AsyncTarget {
    pub fn new(length: usize) -> Self {
        Self {
            requested: vec![false; length],
            completed: vec![false; length],
            data: vec![0; length],
            error: false,
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct TargetRequest {
    pub key: String,
    pub offset: usize,
    pub address: u64,
    pub read: bool,
    pub data: Vec<u8>,
    pub enables: Vec<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    id: usize,
    native: usize,
    data: Vec<u8>,
    reported: bool,
    qos: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AxiService {
    config: FabricConfig,
    workload: FabricWorkload,
    state: State,
    transfers: Vec<Transfer>,
    denied: BTreeSet<String>,
    asynchronous: std::collections::BTreeMap<String, AsyncTarget>,
}
#[derive(Clone, Debug)]
pub(crate) struct Completion {
    pub id: usize,
    pub data: Vec<u8>,
    pub error: bool,
}
impl AxiService {
    pub fn new(cycle: u64, width: usize, sources: &[String], targets: &[Target]) -> Self {
        let idle_sources = vec!["soc_idle".to_string()];
        let sources = if sources.is_empty() {
            &idle_sources
        } else {
            sources
        };
        let config = FabricConfig {
            schema: "dir.transaction-fabric.model".into(),
            version: 1,
            cycle_ps: cycle,
            data_bytes: width,
            write_buffer_beats: 4,
            managers: sources
                .iter()
                .map(|id| Manager {
                    id: id.clone(),
                    routes: targets
                        .iter()
                        .map(|t| (t.id.clone(), vec!["shared".into()]))
                        .collect(),
                })
                .collect(),
            interconnects: vec!["shared".into()],
            targets: targets.to_vec(),
        };
        Self {
            config,
            workload: FabricWorkload {
                schema: "dir.transaction-fabric.workload".into(),
                version: 1,
                requests: vec![],
            },
            state: State {
                transactions: vec![],
                memory: MemoryService::new(&[]),
                sequence: 0,
                last_tick: None,
            },
            transfers: vec![],
            denied: BTreeSet::new(),
            asynchronous: Default::default(),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn submit(
        &mut self,
        id: usize,
        source: &str,
        now: u64,
        address: u32,
        length: usize,
        data: &[u8],
        enables: &[bool],
        read: bool,
        deny: bool,
        axi_id: u32,
        address_delay: u64,
        r_stall: u64,
        b_stall: u64,
        qos: u8,
        asynchronous: bool,
        word: bool,
    ) -> ModelResult {
        if self.transfers.iter().any(|t| t.id == id)
            || length == 0
            || length > 256
            || word && (length != 4 || address % 4 != 0 || self.config.data_bytes < 4)
            || !self.config.managers.iter().any(|m| m.id == source)
            || (!read && (data.len() != length || !enables.is_empty() && enables.len() != length))
        {
            return Err("invalid embedded AXI transfer".into());
        }
        let native = self.workload.requests.len();
        let request_id = format!("soc-{id}");
        let size = if word { 4 } else { 1 };
        let beats = length / size;
        let write_data = if read {
            vec![]
        } else {
            (0..beats)
                .map(|beat| {
                    let mut lanes = vec![0; self.config.data_bytes];
                    let mut strobe = vec![false; self.config.data_bytes];
                    for n in 0..size {
                        let offset = beat * size + n;
                        let lane = (address as usize + offset) % self.config.data_bytes;
                        lanes[lane] = data[offset];
                        strobe[lane] = enables.is_empty() || enables[offset];
                    }
                    WriteBeat {
                        data: lanes,
                        strobe,
                    }
                })
                .collect()
        };
        self.workload.requests.push(Request {
            request_id: request_id.clone(),
            source: source.into(),
            transport: Transport::Axi,
            id: axi_id,
            at_ps: now,
            operation: if read {
                Operation::Read
            } else {
                Operation::Write
            },
            address: address as u64,
            size_bytes: size,
            beats,
            burst: Burst::Incr,
            exclusive: false,
            address_delay_cycles: address_delay,
            data_delay_cycles: 0,
            response_stall_cycles: if read { r_stall } else { b_stall },
            write_data,
        });
        if asynchronous && !deny {
            self.asynchronous
                .insert(request_id.clone(), AsyncTarget::new(length));
        }
        if deny {
            self.denied.insert(request_id);
        }
        self.state.transactions.push(Transaction::default());
        self.transfers.push(Transfer {
            id,
            native,
            data: vec![],
            reported: false,
            qos,
        });
        Ok(())
    }
    pub fn advance(
        &mut self,
        memory: &mut MemoryService,
        c: &mut Context<'_>,
        sequence: &mut u64,
    ) -> ModelResult<Vec<Completion>> {
        if self.state.transactions.iter().any(|t| !t.done) {
            let begin = c.batch.effects.len();
            let mut next = self.state.clone();
            next.memory = std::mem::replace(memory, MemoryService::new(&[]));
            next.sequence = *sequence;
            let fabric = Fabric {
                config: self.config.clone(),
                workload: self.workload.clone(),
                state: self.state.clone(),
                timer: None,
                denied: self.denied.clone(),
                asynchronous: self.asynchronous.clone(),
                qos: self
                    .transfers
                    .iter()
                    .map(|t| (self.workload.requests[t.native].request_id.clone(), t.qos))
                    .collect(),
            };
            fabric.tick(&mut next, c)?;
            *sequence = next.sequence;
            *memory = std::mem::replace(&mut next.memory, MemoryService::new(&[]));
            self.state = next;
            for effect in &c.batch.effects[begin..] {
                if let Effect::Record(r) = effect {
                    if r.schema == record_schema() && r.data["channel"] == "R" {
                        let id = r.data["request_id"]
                            .as_str()
                            .ok_or("missing native AXI response identity")?;
                        let transfer = self
                            .transfers
                            .iter_mut()
                            .find(|t| self.workload.requests[t.native].request_id == id)
                            .ok_or("orphan native AXI R response")?;
                        let req = &self.workload.requests[transfer.native];
                        let beat = r.data["beat"].as_u64().ok_or("missing AXI R beat")? as usize;
                        let (_, lanes) = beat_lanes(req, beat, self.config.data_bytes);
                        for (lane, _) in lanes {
                            transfer.data.push(
                                r.data["detail"]["data"][lane]
                                    .as_u64()
                                    .ok_or("missing actual AXI R lane")?
                                    as u8,
                            );
                        }
                    }
                }
            }
        }
        let mut done = Vec::new();
        for t in &mut self.transfers {
            if !t.reported && self.state.transactions[t.native].done {
                t.reported = true;
                done.push(Completion {
                    id: t.id,
                    data: t.data.clone(),
                    error: self
                        .denied
                        .contains(&self.workload.requests[t.native].request_id),
                });
            }
        }
        Ok(done)
    }
    /// Only handshaken address/data may enter the physical backend.
    pub fn abort_targets(&mut self, base: u64, size: u64) {
        for (key, a) in &mut self.asynchronous {
            if !self
                .workload
                .requests
                .iter()
                .any(|r| r.request_id == *key && r.address >= base && r.address - base < size)
            {
                continue;
            }
            a.error = true;
            self.denied.insert(key.clone());
        }
    }
    pub fn take_target_requests(&mut self) -> Vec<TargetRequest> {
        let mut requests = vec![];
        for (index, r) in self.workload.requests.iter().enumerate() {
            let Some(a) = self.asynchronous.get_mut(&r.request_id) else {
                continue;
            };
            let t = &self.state.transactions[index];
            if a.error {
                continue;
            }
            if !t.address {
                continue;
            }
            if r.operation == Operation::Read {
                if !a.requested[0] {
                    a.requested.fill(true);
                    requests.push(TargetRequest {
                        key: r.request_id.clone(),
                        offset: 0,
                        address: r.address,
                        read: true,
                        data: vec![0; r.beats * r.size_bytes],
                        enables: vec![],
                    });
                }
            } else {
                for beat in 0..t.w_accepted {
                    if a.requested[beat * r.size_bytes] {
                        continue;
                    }
                    a.requested[beat * r.size_bytes..(beat + 1) * r.size_bytes].fill(true);
                    let (address, lanes) = beat_lanes(r, beat, self.config.data_bytes);
                    requests.push(TargetRequest {
                        key: r.request_id.clone(),
                        offset: beat * r.size_bytes,
                        address,
                        read: false,
                        data: lanes
                            .iter()
                            .map(|(lane, _)| r.write_data[beat].data[*lane])
                            .collect(),
                        enables: lanes
                            .iter()
                            .map(|(lane, _)| r.write_data[beat].strobe[*lane])
                            .collect(),
                    });
                }
            }
        }
        requests
    }
    pub fn target_states(&self) -> Vec<(TargetRequest, Vec<bool>, Vec<u8>, bool)> {
        let mut result = vec![];
        for r in &self.workload.requests {
            let Some(a) = self.asynchronous.get(&r.request_id) else {
                continue;
            };
            if r.operation == Operation::Read {
                if a.requested[0] {
                    result.push((
                        TargetRequest {
                            key: r.request_id.clone(),
                            offset: 0,
                            address: r.address,
                            read: true,
                            data: vec![0; r.beats * r.size_bytes],
                            enables: vec![],
                        },
                        a.completed.clone(),
                        a.data.clone(),
                        a.error,
                    ));
                }
            } else {
                for beat in 0..r.beats {
                    if !a.requested[beat * r.size_bytes] {
                        continue;
                    }
                    let (address, lanes) = beat_lanes(r, beat, self.config.data_bytes);
                    result.push((
                        TargetRequest {
                            key: r.request_id.clone(),
                            offset: beat * r.size_bytes,
                            address,
                            read: false,
                            data: lanes
                                .iter()
                                .map(|(lane, _)| r.write_data[beat].data[*lane])
                                .collect(),
                            enables: lanes
                                .iter()
                                .map(|(lane, _)| r.write_data[beat].strobe[*lane])
                                .collect(),
                        },
                        a.completed[beat * r.size_bytes..(beat + 1) * r.size_bytes].to_vec(),
                        a.data[beat * r.size_bytes..(beat + 1) * r.size_bytes].to_vec(),
                        a.error,
                    ));
                }
            }
        }
        result
    }
    pub fn complete_target(
        &mut self,
        key: &str,
        offset: usize,
        data: &[u8],
        error: bool,
    ) -> ModelResult {
        let a = self
            .asynchronous
            .get_mut(key)
            .ok_or("orphan asynchronous AXI target")?;
        let end = offset
            .checked_add(data.len())
            .ok_or("target completion overflow")?;
        if data.is_empty()
            || end > a.data.len()
            || a.requested[offset..end].iter().any(|x| !*x)
            || a.completed[offset..end].iter().any(|x| *x)
        {
            return Err("invalid/repeated AXI target completion".into());
        }
        a.data[offset..end].copy_from_slice(data);
        a.completed[offset..end].fill(true);
        a.error |= error;
        if error {
            self.denied.insert(key.into());
        }
        Ok(())
    }
    pub fn waiting_only_target(&self) -> bool {
        !self.idle()
            && self
                .state
                .transactions
                .iter()
                .enumerate()
                .filter(|(_, t)| !t.done)
                .all(|(i, t)| {
                    let r = &self.workload.requests[i];
                    self.asynchronous.get(&r.request_id).is_some_and(|a| {
                        t.address
                            && !a.error
                            && if r.operation == Operation::Read {
                                !a.completed.iter().all(|x| *x)
                            } else {
                                t.w_committed < r.beats
                                    && (t.w_accepted == r.beats
                                        || self.buffered_for(&r.source)
                                            >= self.config.write_buffer_beats)
                                    && (t.w_committed == t.w_accepted
                                        || !a.completed[t.w_committed * r.size_bytes])
                            }
                    })
                })
    }
    fn buffered_for(&self, source: &str) -> usize {
        self.workload
            .requests
            .iter()
            .zip(&self.state.transactions)
            .filter(|(r, _)| r.source == source)
            .map(|(_, t)| t.w_accepted - t.w_committed)
            .sum()
    }
    pub fn ids(&self) -> Vec<usize> {
        self.transfers.iter().map(|t| t.id).collect()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn validate_binding(
        &self,
        id: usize,
        source: &str,
        address: u32,
        length: usize,
        data: &[u8],
        enables: &[bool],
        read: bool,
        word: bool,
    ) -> ModelResult<Option<Completion>> {
        let transfer = self
            .transfers
            .iter()
            .find(|t| t.id == id)
            .ok_or("missing logical AXI service transfer")?;
        let r = &self.workload.requests[transfer.native];
        if r.source != source
            || r.address != address as u64
            || r.beats * r.size_bytes != length
            || r.size_bytes != if word { 4 } else { 1 }
            || word && r.beats != 1
            || (r.operation == Operation::Read) != read
        {
            return Err("AXI logical/native request mismatch".into());
        }
        if !read {
            for offset in 0..length {
                let lane = (address as usize + offset) % self.config.data_bytes;
                if r.write_data[offset / r.size_bytes].data[lane] != data[offset]
                    || r.write_data[offset / r.size_bytes].strobe[lane]
                        != (enables.is_empty() || enables[offset])
                {
                    return Err("AXI native write bytes/strobes mismatch".into());
                }
            }
        }
        Ok(transfer.reported.then(|| Completion {
            id,
            data: transfer.data.clone(),
            error: self.denied.contains(&r.request_id),
        }))
    }
    pub fn idle(&self) -> bool {
        self.transfers.iter().all(|t| t.reported)
    }
    pub fn validate(
        &self,
        cycle: u64,
        width: usize,
        sources: &[String],
        targets: &[Target],
    ) -> ModelResult {
        if self.config != Self::new(cycle, width, sources, targets).config
            || !self.state.memory.regions.is_empty()
            || self.state.transactions.len() != self.workload.requests.len()
            || self.transfers.len() != self.workload.requests.len()
        {
            return Err("embedded AXI config/state/sole-byte-owner mismatch".into());
        }
        crate::input::transaction_fabric::validate(&self.config, &self.workload)?;
        for (key, a) in &self.asynchronous {
            let i = self
                .workload
                .requests
                .iter()
                .position(|r| r.request_id == *key)
                .ok_or("orphan AXI backend state")?;
            let r = &self.workload.requests[i];
            let t = &self.state.transactions[i];
            if a.data.len() != r.beats * r.size_bytes
                || a.completed.len() != r.beats * r.size_bytes
                || a.requested.len() != r.beats * r.size_bytes
                || a.completed.iter().zip(&a.requested).any(|(c, q)| *c && !*q)
                || a.requested
                    .chunks(r.size_bytes)
                    .any(|v| v.iter().any(|b| *b != v[0]))
                || a.completed
                    .chunks(r.size_bytes)
                    .any(|v| v.iter().any(|b| *b != v[0]))
                || a.requested.iter().any(|x| *x) && !t.address
                || r.operation == Operation::Write
                    && (a
                        .requested
                        .iter()
                        .skip(t.w_accepted * r.size_bytes)
                        .any(|x| *x)
                        || !a.error
                            && a.completed
                                .iter()
                                .take(t.w_committed * r.size_bytes)
                                .any(|x| !*x))
            {
                return Err("invalid AXI physical-target ownership/counters".into());
            }
        }
        let mut ids = BTreeSet::new();
        for t in &self.transfers {
            if !ids.insert(t.id)
                || t.native >= self.state.transactions.len()
                || t.reported && !self.state.transactions[t.native].done
            {
                return Err("invalid embedded AXI completion reference".into());
            }
            let r = &self.workload.requests[t.native];
            let state = &self.state.transactions[t.native];
            if state.w_committed > state.w_accepted
                || state.done
                    && (!state.address
                        || r.operation == Operation::Read && state.r_completed != r.beats
                        || r.operation == Operation::Write && state.w_committed != r.beats)
                || state.w_accepted > r.beats
                || state.r_completed > r.beats
                || t.data.len() != state.r_completed * r.size_bytes
                || r.operation == Operation::Read
                    && (state.w_accepted != 0 || state.w_committed != 0)
                || r.operation == Operation::Write && state.r_completed != 0
                || state
                    .read_payload
                    .as_ref()
                    .is_some_and(|v| v.len() != width)
            {
                return Err("invalid embedded AXI AW/W/R/B counters/captured bytes".into());
            }
        }
        if self
            .denied
            .iter()
            .any(|id| !self.workload.requests.iter().any(|r| r.request_id == *id))
        {
            return Err("orphan AXI denied target".into());
        }
        Ok(())
    }
}
