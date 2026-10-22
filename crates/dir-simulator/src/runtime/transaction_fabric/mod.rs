//! Deterministic AXI4 channel transactions over routed, shared byte-memory resources.
//! This is a digital handshake model; electrical signal/clock-domain conformance is outside its contract.
pub mod memory;
pub(crate) mod service;
use crate::registry::*;
use crate::types::{Diagnostic, transaction_fabric::*};
use memory::{MemoryRequest, MemoryService};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

pub const PROFILE: &str = "axi4.concurrent.v1";
const IMPLEMENTATION: &str = "fabric.Concurrent";
fn tick_schema() -> Schema {
    Schema::new("fabric.Tick", 1)
}
fn record_schema() -> Schema {
    Schema::new("fabric.Record", 1)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    issued: bool,
    done: bool,
    address: bool,
    w_accepted: usize,
    w_committed: usize,
    r_completed: usize,
    ready_ps: Option<u64>,
    exclusive_write: Option<bool>,
    valid_since_ps: Option<u64>,
    read_payload: Option<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    transactions: Vec<Transaction>,
    memory: MemoryService,
    sequence: u64,
    last_tick: Option<u64>,
}
struct Fabric {
    config: FabricConfig,
    workload: FabricWorkload,
    state: State,
    timer: Option<TimerToken>,
    denied: BTreeSet<String>,
    qos: std::collections::BTreeMap<String, u8>,
    asynchronous: std::collections::BTreeMap<String, service::AsyncTarget>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    config: FabricConfig,
    workload: FabricWorkload,
    state: State,
    timer: Option<TimerReference>,
}

fn factory(c: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload) = crate::input::transaction_fabric::parse(&c.profile_input)?;
    let state = State {
        transactions: vec![Transaction::default(); workload.requests.len()],
        memory: MemoryService::new(&config.targets),
        sequence: 0,
        last_tick: None,
    };
    Ok(Box::new(Fabric {
        config,
        workload,
        state,
        timer: None,
        denied: BTreeSet::new(),
        qos: Default::default(),
        asynchronous: Default::default(),
    }))
}
fn restore(
    c: &ModelConfig,
    version: u32,
    bytes: &[u8],
    context: &CheckpointDecodeContext<'_>,
) -> ModelResult<Box<dyn Model>> {
    if version != 1 {
        return Err("unsupported fabric checkpoint codec".into());
    }
    let saved: Checkpoint =
        serde_json::from_slice(bytes).map_err(|e| ModelError(format!("fabric checkpoint: {e}")))?;
    let (config, workload) = crate::input::transaction_fabric::parse(&c.profile_input)?;
    if saved.version != 1
        || saved.config != config
        || saved.workload != workload
        || saved.state.transactions.len() != workload.requests.len()
    {
        return Err("fabric checkpoint input/state mismatch".into());
    }
    saved.state.memory.validate(&config.targets)?;
    saved
        .state
        .memory
        .validate_monitors(&workload.requests, |i| saved.state.transactions[i].done)?;
    for (s, r) in saved.state.transactions.iter().zip(&workload.requests) {
        if s.w_committed > s.w_accepted
            || s.w_accepted > r.beats
            || s.r_completed > r.beats
            || (!s.issued && (s.done || s.address || s.w_accepted != 0 || s.r_completed != 0))
            || (r.operation == Operation::Read
                && (s.w_accepted != 0 || s.exclusive_write.is_some()))
            || (r.operation == Operation::Write && s.r_completed != 0)
            || (s.valid_since_ps.is_some() && (s.done || s.ready_ps.is_none() || !s.address))
            || (s.read_payload.is_some()
                != (r.operation == Operation::Read && s.valid_since_ps.is_some()))
            || s.read_payload
                .as_ref()
                .is_some_and(|data| data.len() != config.data_bytes)
            || s.valid_since_ps
                .is_some_and(|t| saved.state.last_tick.is_none_or(|last| t > last))
            || (s.done
                && (!s.address
                    || (r.operation == Operation::Write && s.w_committed != r.beats)
                    || (r.operation == Operation::Read && s.r_completed != r.beats)))
        {
            return Err("invalid fabric checkpoint transaction counters".into());
        }
    }
    let timer = saved
        .timer
        .as_ref()
        .map(|t| context.decode_timer_ref(t))
        .transpose()?;
    let probe = Fabric {
        config: config.clone(),
        workload: workload.clone(),
        state: saved.state.clone(),
        timer: timer.clone(),
        denied: BTreeSet::new(),
        qos: Default::default(),
        asynchronous: Default::default(),
    };
    let mut held = BTreeSet::new();
    for (i, transaction) in saved.state.transactions.iter().enumerate() {
        let r = &workload.requests[i];
        if transaction.valid_since_ps.is_some()
            && !probe.claim(
                r,
                if r.operation == Operation::Read {
                    "R"
                } else {
                    "B"
                },
                &mut held,
            )
        {
            return Err("invalid checkpoint held channel ownership".into());
        }
    }
    for manager in &config.managers {
        if probe.buffered_w(&saved.state, &manager.id) > config.write_buffer_beats {
            return Err("checkpoint write buffer exceeds capacity".into());
        }
    }
    if saved
        .state
        .last_tick
        .is_some_and(|t| t % config.cycle_ps != 0)
        || (saved.state.transactions.iter().any(|t| !t.done) && timer.is_none())
    {
        return Err("invalid fabric checkpoint tick ownership".into());
    }
    Ok(Box::new(Fabric {
        config,
        workload,
        state: saved.state,
        timer,
        denied: BTreeSet::new(),
        qos: Default::default(),
        asynchronous: Default::default(),
    }))
}

impl Fabric {
    fn later(&self, now: u64, cycles: u64) -> ModelResult<u64> {
        now.checked_add(
            cycles
                .checked_mul(self.config.cycle_ps)
                .ok_or("time overflow")?,
        )
        .ok_or_else(|| "time overflow".into())
    }
    fn target(&self, r: &Request) -> Option<usize> {
        if self.denied.contains(&r.request_id) {
            return None;
        }
        self.config
            .targets
            .iter()
            .position(|t| r.address >= t.base && r.address - t.base < t.size)
    }
    fn claim(&self, r: &Request, channel: &str, resources: &mut BTreeSet<String>) -> bool {
        let mut keys = vec![format!("{channel}:manager:{}", r.source)];
        if let Some(target) = self.target(r) {
            let manager = self
                .config
                .managers
                .iter()
                .find(|m| m.id == r.source)
                .expect("validated manager");
            keys.extend(
                manager.routes[&self.config.targets[target].id]
                    .iter()
                    .map(|link| format!("{channel}:interconnect:{link}")),
            );
            let path = &manager.routes[&self.config.targets[target].id];
            let nodes: Vec<_> = std::iter::once(format!("manager:{}", r.source))
                .chain(path.iter().map(|p| format!("interconnect:{p}")))
                .chain(std::iter::once(format!("target:{target}")))
                .collect();
            keys.extend(
                nodes
                    .windows(2)
                    .map(|edge| format!("{channel}:link:{}>{}", edge[0], edge[1])),
            );
            keys.push(format!("{channel}:target:{target}"));
        }
        if keys.iter().any(|k| resources.contains(k)) {
            return false;
        }
        resources.extend(keys);
        true
    }
    fn emit(
        &self,
        state: &mut State,
        c: &mut Context<'_>,
        r: &Request,
        channel: &str,
        beat: Option<usize>,
        mut extra: serde_json::Value,
    ) -> ModelResult {
        let sequence = state.sequence;
        state.sequence = sequence.checked_add(1).ok_or("record sequence overflow")?;
        if matches!(channel, "AW" | "AR") {
            extra[if channel == "AW" { "awqos" } else { "arqos" }] =
                json!(self.qos.get(&r.request_id).copied().unwrap_or(0));
        }
        c.upsert_model_record(ModelRecord { schema: record_schema(), id: format!("event-{sequence}"), subject: c.subject().into(), time_ps: c.now(),
            data: json!({"kind":"channel", "effect_seq":sequence.to_string(), "request_id":r.request_id,"source":r.source,"axi_id":r.id,"transport":r.transport,"channel":channel,"beat":beat,"target":self.target(r).map(|i| &self.config.targets[i].id),"detail":extra}) })
    }
    fn older_pending(&self, state: &State, index: usize) -> bool {
        let r = &self.workload.requests[index];
        self.workload.requests.iter().enumerate().any(|(j, other)| {
            (other.at_ps, j) < (r.at_ps, index)
                && other.source == r.source
                && other.id == r.id
                && other.operation == r.operation
                && other.transport == r.transport
                && !state.transactions[j].done
        })
    }
    fn older_w(&self, state: &State, index: usize) -> bool {
        let r = &self.workload.requests[index];
        let mut path = BTreeSet::new();
        self.claim(r, "W", &mut path);
        self.workload.requests.iter().enumerate().any(|(j, other)| {
            (other.at_ps, j) < (r.at_ps, index)
                && other.operation == Operation::Write
                && other.transport == r.transport
                && state.transactions[j].w_accepted != other.beats
                && {
                    let mut other_path = BTreeSet::new();
                    self.claim(other, "W", &mut other_path);
                    !path.is_disjoint(&other_path)
                }
        })
    }
    fn buffered_w(&self, state: &State, source: &str) -> usize {
        self.workload
            .requests
            .iter()
            .zip(&state.transactions)
            .filter(|(r, _)| r.source == source)
            .map(|(_, s)| s.w_accepted - s.w_committed)
            .sum()
    }
    fn older_address(&self, state: &State, index: usize) -> bool {
        let r = &self.workload.requests[index];
        let mut path = BTreeSet::new();
        self.claim(r, "AW", &mut path);
        self.workload.requests.iter().enumerate().any(|(j, other)| {
            (other.at_ps, j) < (r.at_ps, index)
                && other.operation == r.operation
                && other.transport == r.transport
                && (if r.operation == Operation::Write {
                    let mut other_path = BTreeSet::new();
                    self.claim(other, "AW", &mut other_path);
                    !path.is_disjoint(&other_path)
                } else {
                    other.source == r.source && other.id == r.id
                })
                && !state.transactions[j].address
        })
    }
    fn tick(&self, state: &mut State, c: &mut Context<'_>) -> ModelResult {
        let now = c.now();
        let mut resources = BTreeSet::new();
        // An asserted response holds its routed channel and payload until READY.
        for (i, transaction) in state.transactions.iter().enumerate() {
            if transaction.valid_since_ps.is_some() {
                let r = &self.workload.requests[i];
                let channel = if r.operation == Operation::Read {
                    "R"
                } else {
                    "B"
                };
                if !self.claim(r, channel, &mut resources) {
                    return Err("conflicting held response resources".into());
                }
            }
        }
        let mut order: Vec<_> = (0..self.workload.requests.len()).collect();
        order.sort_by_key(|i| (self.workload.requests[*i].at_ps, *i));
        for &i in &order {
            let r = &self.workload.requests[i];
            if r.at_ps <= now {
                state.transactions[i].issued = true;
            }
            if !state.transactions[i].issued || state.transactions[i].done {
                continue;
            }
            let channel = if r.operation == Operation::Write {
                "AW"
            } else {
                "AR"
            };
            if !state.transactions[i].address
                && !self.older_address(state, i)
                && now >= self.later(r.at_ps, r.address_delay_cycles)?
                && self.claim(r, channel, &mut resources)
            {
                state.transactions[i].address = true;
                if r.operation == Operation::Read {
                    let latency = self
                        .target(r)
                        .map_or(1, |t| self.config.targets[t].read_latency_cycles.max(1));
                    state.transactions[i].ready_ps = Some(self.later(now, latency)?);
                    if r.exclusive && self.target(r).is_some() {
                        state.memory.begin_exclusive_read(
                            &r.source,
                            r.id,
                            &r.request_id,
                            r.address,
                            (r.beats * r.size_bytes) as u64,
                        );
                    }
                }
                self.emit(state, c, r, if r.transport == Transport::Dma { "DMA_ACCEPT" } else { channel }, None, json!({"address":r.address.to_string(),"beats":r.beats,"size_bytes":r.size_bytes,"burst":r.burst,"exclusive":r.exclusive}))?;
            }
        }
        for &i in &order {
            let r = &self.workload.requests[i];
            if !state.transactions[i].issued
                || state.transactions[i].done
                || r.operation != Operation::Write
            {
                continue;
            }
            // AXI4 has no WID: one manager's W burst is never interleaved with another burst.
            if state.transactions[i].w_accepted < r.beats
                && self.buffered_w(state, &r.source) < self.config.write_buffer_beats
                && !self.older_w(state, i)
                && now >= self.later(r.at_ps, r.data_delay_cycles)?
                && self.claim(r, "W", &mut resources)
            {
                let beat = state.transactions[i].w_accepted;
                state.transactions[i].w_accepted += 1;
                self.emit(state, c, r, if r.transport == Transport::Dma { "DMA_W" } else { "W" }, Some(beat), json!({"data":r.write_data[beat].data,"strobe":r.write_data[beat].strobe,"last":beat+1==r.beats}))?;
            }
            // Buffered W may precede AW; each target commits at most one beat per cycle.
            if state.transactions[i].address
                && state.transactions[i].w_committed < state.transactions[i].w_accepted
                && self.asynchronous.get(&r.request_id).is_none_or(|a| {
                    a.error || a.completed[state.transactions[i].w_committed * r.size_bytes]
                })
                && self.claim(r, "MEM_WRITE", &mut resources)
            {
                let beat = state.transactions[i].w_committed;
                if r.exclusive && state.transactions[i].exclusive_write.is_none() {
                    state.transactions[i].exclusive_write = Some(state.memory.take_exclusive(
                        &r.source,
                        r.id,
                        r.address,
                        (r.beats * r.size_bytes) as u64,
                    ));
                }
                let enabled = !r.exclusive || state.transactions[i].exclusive_write == Some(true);
                let (address, lanes) = beat_lanes(r, beat, self.config.data_bytes);
                if enabled
                    && self.target(r).is_some()
                    && !self.asynchronous.contains_key(&r.request_id)
                {
                    // Pack only legal bytes into the shared service's contiguous address range.
                    let data: Vec<_> = lanes
                        .iter()
                        .map(|(lane, _)| r.write_data[beat].data[*lane])
                        .collect();
                    let enables: Vec<_> = lanes
                        .iter()
                        .map(|(lane, _)| r.write_data[beat].strobe[*lane])
                        .collect();
                    state.memory.write(&MemoryRequest {
                        source: &r.source,
                        request_id: &r.request_id,
                        id: r.id,
                        address,
                        data: &data,
                        byte_enables: &enables,
                    })?;
                    let t = self.target(r).expect("mapped write");
                    let sequence = state.sequence;
                    state.sequence = sequence
                        .checked_add(1)
                        .ok_or("AXI memory record sequence overflow")?;
                    c.upsert_model_record(ModelRecord { schema: record_schema(), id: format!("memory-{}", self.config.targets[t].id), subject: c.subject().into(), time_ps: now,
                        data: json!({"kind":"memory","effect_seq":sequence,"target":self.config.targets[t].id,"base":self.config.targets[t].base.to_string(),"bytes":state.memory.regions[t].bytes}) })?;
                }
                state.transactions[i].w_committed += 1;
                self.emit(
                    state,
                    c,
                    r,
                    "COMMIT",
                    Some(beat),
                    json!({"address":address.to_string(),"written":enabled && self.target(r).is_some()}),
                )?;
                if state.transactions[i].w_committed == r.beats {
                    let delay = self
                        .target(r)
                        .map_or(1, |t| self.config.targets[t].write_response_cycles.max(1));
                    state.transactions[i].ready_ps = Some(self.later(now, delay)?);
                }
            }
        }
        for &i in &order {
            let r = &self.workload.requests[i];
            if state.transactions[i].done
                || !state.transactions[i].address
                || self.older_pending(state, i)
            {
                continue;
            }
            let Some(ready) = state.transactions[i].ready_ps else {
                continue;
            };
            if self
                .asynchronous
                .get(&r.request_id)
                .is_some_and(|a| !a.error && !a.completed.iter().all(|x| *x))
            {
                continue;
            }
            if now < ready {
                continue;
            }
            let channel = if r.operation == Operation::Read {
                "R"
            } else {
                "B"
            };
            let response = if self.target(r).is_none() {
                "DECERR"
            } else if r.exclusive
                && (r.operation == Operation::Read
                    || state.transactions[i].exclusive_write == Some(true))
            {
                "EXOKAY"
            } else {
                "OKAY"
            };
            let beat = state.transactions[i].r_completed;
            let (address, lanes) = beat_lanes(r, beat, self.config.data_bytes);
            if state.transactions[i].valid_since_ps.is_none() {
                if !self.claim(r, channel, &mut resources) {
                    continue;
                }
                state.transactions[i].valid_since_ps = Some(now);
                if r.operation == Operation::Read {
                    let mut data = vec![0; self.config.data_bytes];
                    if self.target(r).is_some() {
                        let zero = vec![0; lanes.len()];
                        let memory_data = if let Some(a) = self.asynchronous.get(&r.request_id) {
                            a.data[beat * r.size_bytes..beat * r.size_bytes + lanes.len()].to_vec()
                        } else {
                            state
                                .memory
                                .read(&MemoryRequest {
                                    source: &r.source,
                                    request_id: &r.request_id,
                                    id: r.id,
                                    address,
                                    data: &zero,
                                    byte_enables: &[],
                                })?
                                .data
                        };
                        for ((lane, _), byte) in lanes.iter().zip(memory_data) {
                            data[*lane] = byte;
                        }
                    }
                    state.transactions[i].read_payload = Some(data);
                }
            }
            let valid_since = state.transactions[i]
                .valid_since_ps
                .ok_or("missing asserted response")?;
            if now < self.later(valid_since, r.response_stall_cycles)? {
                continue;
            }
            state.transactions[i].valid_since_ps = None;
            if r.operation == Operation::Read {
                let data = state.transactions[i]
                    .read_payload
                    .take()
                    .ok_or("missing stable R payload")?;
                state.transactions[i].r_completed += 1;
                state.transactions[i].done = state.transactions[i].r_completed == r.beats;
                if state.transactions[i].done && r.exclusive {
                    state.memory.complete_exclusive_read(
                        &r.source,
                        r.id,
                        &r.request_id,
                        r.address,
                        (r.beats * r.size_bytes) as u64,
                    );
                }
                state.transactions[i].ready_ps = Some(self.later(now, 1)?);
                self.emit(state, c, r, if r.transport == Transport::Dma { "DMA_R" } else { "R" }, Some(beat), json!({"address":address.to_string(),"data":data,"response":response,"last":beat+1==r.beats,"valid_since_ps":valid_since.to_string()}))?;
            } else {
                state.transactions[i].done = true;
                self.emit(
                    state,
                    c,
                    r,
                    if r.transport == Transport::Dma {
                        "DMA_DONE"
                    } else {
                        "B"
                    },
                    None,
                    json!({"response":response,"valid_since_ps":valid_since.to_string()}),
                )?;
            }
        }
        state.last_tick = Some(now);
        Ok(())
    }
}
impl Model for Fabric {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        if let Some(first) = self.workload.requests.iter().map(|r| r.at_ps).min() {
            self.timer = Some(c.schedule_at(first, tick_schema(), vec![])?)
        }
        Ok(())
    }
    fn on_event(&mut self, event: &Envelope, c: &mut Context<'_>) -> ModelResult {
        if event.schema != tick_schema() || c.current_timer_token() != self.timer.as_ref() {
            return Err("invalid fabric tick owner".into());
        }
        // Keep model bytes and observations on one successful callback boundary.
        let mut next = self.state.clone();
        self.tick(&mut next, c)?;
        let timer = if next.transactions.iter().any(|s| !s.done) {
            let at = if next.transactions.iter().any(|s| s.issued && !s.done) {
                self.later(c.now(), 1)?
            } else {
                self.workload
                    .requests
                    .iter()
                    .zip(&next.transactions)
                    .filter(|(_, s)| !s.done)
                    .map(|(r, _)| r.at_ps)
                    .min()
                    .ok_or("missing pending request")?
            };
            Some(c.schedule_at(at, tick_schema(), vec![])?)
        } else {
            None
        };
        self.state = next;
        self.timer = timer;
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
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
        .map_err(|e| ModelError(format!("fabric checkpoint: {e}")))
    }
}

pub fn register_transaction_fabric(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register_event(EventDescriptor {
        schema: tick_schema(),
        phase: 1,
        validate: |bytes| {
            if bytes.is_empty() {
                Ok(())
            } else {
                Err("invalid fabric tick payload".into())
            }
        },
    })?;
    registry.register_module(
        IMPLEMENTATION,
        factory,
        ModuleDescriptor {
            implementation_key: IMPLEMENTATION.into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![tick_schema()],
            resources: vec![],
        },
    )?;
    registry.register_model_record(ModelRecordDescriptor {
        schema: record_schema(),
        validate: |v| {
            if v.get("kind")
                .and_then(|v| v.as_str())
                .is_some_and(|k| matches!(k, "channel" | "memory"))
            {
                Ok(())
            } else {
                Err("invalid fabric record kind".into())
            }
        },
    })?;
    registry.register_profile(ProfileDescriptor { name: PROFILE.into(), implementation_version: "1".into(), modules: vec![IMPLEMENTATION.into()], channels: vec![], events: vec![tick_schema()], metrics: vec![], model_records: vec![record_schema()], output_schema_version: 3,
        validate: |models, channels| if models.len() == 1 && channels.is_empty() { Ok(()) } else { Err("fabric profile requires exactly one Concurrent execution owner and no external channels".into()) } })?;
    registry.register_profile_input_validator(
        PROFILE,
        crate::input::transaction_fabric::validate_input,
    )?;
    registry.register_execution_capabilities(
        PROFILE,
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: false,
            checkpoint: true,
        },
    )?;
    registry.register_model_checkpoint_codec(
        CheckpointCodecDescriptor {
            implementation_key: IMPLEMENTATION.into(),
            implementation_version: "1".into(),
            codec_name: "fabric.state".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 268_435_456,
        },
        restore,
    )?;
    Ok(())
}
