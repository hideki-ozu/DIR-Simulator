//! Embedded use of the production full-AHB pipeline. The only RAM is lent by its owner.
use super::*;
use crate::runtime::transaction_fabric::service::{AsyncTarget, TargetRequest};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    id: usize,
    fragments: Vec<usize>,
    data: Vec<u8>,
    read: bool,
    reported: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AhbService {
    config: Config,
    workload: Workload,
    state: State,
    batches: Vec<Batch>,
    offsets: Vec<(usize, usize)>,
    asynchronous: BTreeMap<String, AsyncTarget>,
}
#[derive(Clone, Debug)]
pub(crate) struct Completion {
    pub id: usize,
    pub data: Vec<u8>,
    pub error: bool,
}
impl AhbService {
    pub fn new(
        cycle_ps: u64,
        width: usize,
        sources: &[String],
        targets: &[memory_types::Target],
        wait: u64,
    ) -> Self {
        let idle_sources = vec!["soc_idle".to_string()];
        let sources = if sources.is_empty() {
            &idle_sources
        } else {
            sources
        };
        let config = Config {
            schema: "dir.ahb-pipeline.model".into(),
            version: 1,
            cycle_ps,
            data_bytes: width,
            managers: sources
                .iter()
                .enumerate()
                .map(|(i, id)| Manager {
                    id: id.clone(),
                    priority: i as u32,
                })
                .collect(),
            targets: targets
                .iter()
                .map(|t| Target {
                    id: t.id.clone(),
                    base: t.base,
                    size: t.size,
                    wait_cycles: wait,
                    initial: vec![],
                })
                .collect(),
            responses: vec![],
        };
        Self {
            config,
            workload: Workload {
                schema: "dir.ahb-pipeline.workload".into(),
                version: 1,
                requests: vec![],
            },
            state: State {
                transactions: vec![],
                memory: MemoryService::new(&[]),
                address: None,
                data: None,
                lock_owner: None,
                lock_setup_until: None,
                split_masks: BTreeMap::new(),
                last_address: None,
                sequence: 0,
                last_tick: None,
            },
            batches: vec![],
            offsets: vec![],
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
        asynchronous: bool,
        word: bool,
    ) -> ModelResult {
        if self.batches.iter().any(|b| b.id == id)
            || length == 0
            || length > 256
            || word
                && (length != 4
                    || address % 4 != 0
                    || self.config.data_bytes < 4
                    || !enables.is_empty() && enables.iter().any(|b| !*b))
            || !self.config.managers.iter().any(|m| m.id == source)
            || (!read && (data.len() != length || !enables.is_empty() && enables.len() != length))
        {
            return Err("invalid embedded AHB transfer".into());
        }
        let batch = self.batches.len();
        let mut fragments = Vec::new();
        let size = if word { 4 } else { 1 };
        for offset in (0..length).step_by(size) {
            if !deny && !read && !enables.is_empty() && !enables[offset] {
                continue;
            }
            let index = self.workload.requests.len();
            let mut lanes = vec![0; self.config.data_bytes];
            if !read {
                for n in 0..size {
                    lanes[(address as usize + offset + n) % self.config.data_bytes] =
                        data[offset + n];
                }
            }
            let request_id = format!("soc-{id}-byte-{offset}");
            self.workload.requests.push(Request {
                request_id: request_id.clone(),
                source: source.into(),
                at_ps: now,
                operation: if read {
                    Operation::Read
                } else {
                    Operation::Write
                },
                address: address as u64 + offset as u64,
                size_bytes: size,
                beats: 1,
                burst: Burst::Single,
                locked: false,
                write_data: if read { vec![] } else { vec![lanes] },
            });
            if asynchronous && !deny {
                self.asynchronous
                    .insert(request_id.clone(), AsyncTarget::new(size));
            }
            if deny {
                self.config.responses.push(ResponseRule {
                    request_id,
                    beat: 0,
                    attempt: 0,
                    response: Response::Error,
                    wait_cycles: 0,
                    split_release_cycles: None,
                });
            }
            self.state.transactions.push(Transaction {
                next_beat: 0,
                completed: 0,
                attempts: vec![0],
                done: false,
                failed: false,
                fragmented: false,
            });
            self.offsets.push((batch, offset));
            fragments.push(index);
        }
        self.batches.push(Batch {
            id,
            fragments,
            data: if read { vec![0; length] } else { vec![] },
            read,
            reported: false,
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
            let start = c.batch.effects.len();
            let mut next = self.state.clone();
            next.memory = std::mem::replace(memory, MemoryService::new(&[]));
            next.sequence = *sequence;
            let pipeline = Pipeline {
                config: self.config.clone(),
                workload: self.workload.clone(),
                state: self.state.clone(),
                timer: None,
                asynchronous: self.asynchronous.clone(),
            };
            pipeline.tick(&mut next, c)?;
            *sequence = next.sequence;
            *memory = std::mem::replace(&mut next.memory, MemoryService::new(&[]));
            self.state = next;
            // Observe the actual same-callback HREADY/data effects, never planned memory data.
            for effect in &c.batch.effects[start..] {
                if let Effect::Record(r) = effect {
                    if r.schema == record_schema()
                        && r.data["kind"] == "cycle"
                        && r.data["hready"] == true
                    {
                        let p = &r.data["data_phase"];
                        if let Some(id) = p["request_id"].as_str() {
                            let index = self
                                .workload
                                .requests
                                .iter()
                                .position(|r| r.request_id == id)
                                .ok_or("orphan AHB data effect")?;
                            let (batch, offset) = self.offsets[index];
                            if self.batches[batch].read && r.data["hresp"] == "OKAY" {
                                let lane = (self.workload.requests[index].address
                                    % self.config.data_bytes as u64)
                                    as usize;
                                for n in 0..self.workload.requests[index].size_bytes {
                                    self.batches[batch].data[offset + n] = p["read_data"][lane + n]
                                        .as_u64()
                                        .ok_or("missing actual AHB read byte")?
                                        as u8;
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut complete = Vec::new();
        for b in &mut self.batches {
            if !b.reported && b.fragments.iter().all(|i| self.state.transactions[*i].done) {
                b.reported = true;
                complete.push(Completion {
                    id: b.id,
                    data: b.data.clone(),
                    error: b
                        .fragments
                        .iter()
                        .any(|i| self.state.transactions[*i].failed),
                });
            }
        }
        Ok(complete)
    }
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
        }
    }
    pub fn take_target_requests(&mut self) -> Vec<TargetRequest> {
        let Some(d) = &self.state.data else {
            return vec![];
        };
        let r = &self.workload.requests[d.address.request];
        let Some(a) = self.asynchronous.get_mut(&r.request_id) else {
            return vec![];
        };
        if a.error || a.requested[0] {
            return vec![];
        }
        a.requested.fill(true);
        let lane = r.address as usize % self.config.data_bytes;
        vec![TargetRequest {
            key: r.request_id.clone(),
            offset: 0,
            address: r.address,
            read: r.operation == Operation::Read,
            data: if r.operation == Operation::Read {
                vec![0; r.size_bytes]
            } else {
                r.write_data[0][lane..lane + r.size_bytes].to_vec()
            },
            enables: vec![true; r.size_bytes],
        }]
    }
    pub fn target_states(&self) -> Vec<(TargetRequest, Vec<bool>, Vec<u8>, bool)> {
        self.workload
            .requests
            .iter()
            .filter_map(|r| {
                let a = self.asynchronous.get(&r.request_id)?;
                if !a.requested[0] {
                    return None;
                }
                let lane = r.address as usize % self.config.data_bytes;
                Some((
                    TargetRequest {
                        key: r.request_id.clone(),
                        offset: 0,
                        address: r.address,
                        read: r.operation == Operation::Read,
                        data: if r.operation == Operation::Read {
                            vec![0; r.size_bytes]
                        } else {
                            r.write_data[0][lane..lane + r.size_bytes].to_vec()
                        },
                        enables: vec![true; r.size_bytes],
                    },
                    a.completed.clone(),
                    a.data.clone(),
                    a.error,
                ))
            })
            .collect()
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
            .ok_or("orphan AHB physical target")?;
        if offset != 0 || data.len() != a.data.len() || !a.requested[0] || a.completed[0] {
            return Err("invalid/repeated AHB target completion".into());
        }
        a.data.copy_from_slice(data);
        a.completed.fill(true);
        a.error = error;
        Ok(())
    }
    pub fn waiting_only_target(&self) -> bool {
        self.state.data.as_ref().is_some_and(|d| {
            let r = &self.workload.requests[d.address.request];
            self.asynchronous
                .get(&r.request_id)
                .is_some_and(|a| !a.error && !a.completed[0])
        })
    }
    pub fn ids(&self) -> Vec<usize> {
        self.batches.iter().map(|b| b.id).collect()
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
        let b = self
            .batches
            .iter()
            .find(|b| b.id == id)
            .ok_or("missing logical AHB service transfer")?;
        if b.read != read || read && b.data.len() != length {
            return Err("AHB logical/native response shape mismatch".into());
        }
        let size = if word { 4 } else { 1 };
        let denied = b.fragments.iter().any(|i| {
            self.config.responses.iter().any(|rule| {
                rule.request_id == self.workload.requests[*i].request_id
                    && rule.response == Response::Error
            })
        });
        let expected: Vec<_> = (0..length)
            .step_by(size)
            .filter(|i| denied || read || enables.is_empty() || enables[*i])
            .collect();
        if expected.len() != b.fragments.len() {
            return Err("AHB native fragment coverage mismatch".into());
        }
        for (offset, index) in expected.iter().zip(&b.fragments) {
            let r = &self.workload.requests[*index];
            let lane = (address as usize + offset) % self.config.data_bytes;
            if r.source != source
                || r.address != address as u64 + *offset as u64
                || (r.operation == Operation::Read) != read
                || r.size_bytes != size
                || r.beats != 1
                || r.burst != Burst::Single
                || r.locked
                || !read && r.write_data[0][lane..lane + size] != data[*offset..*offset + size]
            {
                return Err("AHB logical/native bytes mismatch".into());
            }
        }
        Ok(b.reported.then(|| Completion {
            id,
            data: b.data.clone(),
            error: b
                .fragments
                .iter()
                .any(|i| self.state.transactions[*i].failed),
        }))
    }
    pub fn idle(&self) -> bool {
        self.batches.iter().all(|b| b.reported)
    }
    pub fn validate(
        &self,
        cycle: u64,
        width: usize,
        sources: &[String],
        targets: &[memory_types::Target],
        wait: u64,
    ) -> ModelResult {
        let expected = Self::new(cycle, width, sources, targets, wait);
        if self.config.schema != expected.config.schema
            || self.config.version != 1
            || self.config.cycle_ps != cycle
            || self.config.data_bytes != width
            || self.config.managers != expected.config.managers
            || self.config.targets != expected.config.targets
            || !self.state.memory.regions.is_empty()
            || self.offsets.len() != self.workload.requests.len()
            || self.state.transactions.len() != self.workload.requests.len()
        {
            return Err("embedded AHB state/config/sole-byte-owner mismatch".into());
        }
        input::validate(&self.config, &self.workload)?;
        for (key, a) in &self.asynchronous {
            let i = self
                .workload
                .requests
                .iter()
                .position(|r| r.request_id == *key)
                .ok_or("orphan AHB physical state")?;
            let size = self.workload.requests[i].size_bytes;
            if a.data.len() != size
                || a.requested.len() != size
                || a.completed.len() != size
                || a.requested.iter().any(|b| *b != a.requested[0])
                || a.completed.iter().any(|b| *b != a.completed[0])
                || a.completed[0] && !a.requested[0]
                || a.requested[0] && self.state.transactions[i].next_beat != 1
                || self.state.transactions[i].done && !a.completed[0] && !a.error
            {
                return Err("invalid AHB physical-target ownership".into());
            }
        }
        for (i, t) in self.state.transactions.iter().enumerate() {
            if t.next_beat > 1
                || t.completed > t.next_beat
                || t.attempts != [0]
                || t.failed && !t.done
                || t.done && !t.failed && t.completed != 1
                || self.offsets[i].0 >= self.batches.len()
            {
                return Err("invalid embedded AHB handshake counters".into());
            }
        }
        if let Some(a) = &self.state.address {
            if a.request >= self.workload.requests.len()
                || a.beat != 0
                || self.state.transactions[a.request].next_beat != 0
            {
                return Err("invalid embedded AHB address owner".into());
            }
        }
        if let Some(d) = &self.state.data {
            if d.address.request >= self.workload.requests.len()
                || d.address.beat != 0
                || self.state.transactions[d.address.request].next_beat != 1
                || self.state.transactions[d.address.request].done
                || d.wait_remaining > wait
                || d.second_response && d.response == Response::Okay
            {
                return Err("invalid embedded AHB data owner/HREADY lifetime".into());
            }
        }
        for (batch, b) in self.batches.iter().enumerate() {
            if b.fragments.iter().any(|i| {
                *i >= self.offsets.len()
                    || self.offsets[*i].0 != batch
                    || self.offsets[*i].1 >= if b.read { b.data.len() } else { 256 }
            }) || b.reported && !b.fragments.iter().all(|i| self.state.transactions[*i].done)
            {
                return Err("invalid embedded AHB completion/capture".into());
            }
        }
        Ok(())
    }
}
