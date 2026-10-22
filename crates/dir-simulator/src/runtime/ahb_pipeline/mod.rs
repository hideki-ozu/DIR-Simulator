//! AMBA2 full-AHB digital pipeline with an explicit fixed-priority arbitration policy.
#[path = "../../input/ahb_pipeline.rs"]
mod input;
pub(crate) mod service;
#[path = "../../lib/types/ahb_pipeline.rs"]
pub mod types;
use super::transaction_fabric::memory::{MemoryRequest, MemoryService};
use crate::registry::*;
use crate::types::{Diagnostic, transaction_fabric as memory_types};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use types::*;

pub const PROFILE: &str = "ahb.pipeline.v1";
const IMPLEMENTATION: &str = "ahb.Pipeline";
fn tick_schema() -> Schema {
    Schema::new("ahb.pipeline.Tick", 1)
}
fn record_schema() -> Schema {
    Schema::new("ahb.pipeline.Record", 1)
}
fn memory_targets(c: &Config) -> Vec<memory_types::Target> {
    c.targets
        .iter()
        .map(|t| memory_types::Target {
            id: t.id.clone(),
            base: t.base,
            size: t.size,
            kind: memory_types::MemoryKind::Sram,
            read_latency_cycles: 0,
            write_response_cycles: 0,
            initial: t.initial.clone(),
        })
        .collect()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    next_beat: usize,
    completed: usize,
    attempts: Vec<u32>,
    done: bool,
    failed: bool,
    fragmented: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Address {
    request: usize,
    beat: usize,
    attempt: u32,
    nonseq: bool,
    burst: Burst,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DataPhase {
    address: Address,
    wait_remaining: u64,
    response: Response,
    second_response: bool,
    split_release_cycles: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    transactions: Vec<Transaction>,
    memory: MemoryService,
    address: Option<Address>,
    data: Option<DataPhase>,
    lock_owner: Option<usize>,
    lock_setup_until: Option<u64>,
    split_masks: BTreeMap<usize, u64>,
    last_address: Option<usize>,
    sequence: u64,
    last_tick: Option<u64>,
}
struct Pipeline {
    config: Config,
    workload: Workload,
    state: State,
    timer: Option<TimerToken>,
    asynchronous: BTreeMap<String, super::transaction_fabric::service::AsyncTarget>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    config: Config,
    workload: Workload,
    state: State,
    timer: Option<TimerReference>,
}
fn factory(c: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload) = input::parse(&c.profile_input)?;
    let state = State {
        transactions: workload
            .requests
            .iter()
            .map(|r| Transaction {
                next_beat: 0,
                completed: 0,
                attempts: vec![0; r.beats],
                done: false,
                failed: false,
                fragmented: false,
            })
            .collect(),
        memory: MemoryService::new(&memory_targets(&config)),
        address: None,
        data: None,
        lock_owner: None,
        lock_setup_until: None,
        split_masks: BTreeMap::new(),
        last_address: None,
        sequence: 0,
        last_tick: None,
    };
    Ok(Box::new(Pipeline {
        config,
        workload,
        state,
        timer: None,
        asynchronous: Default::default(),
    }))
}
impl Pipeline {
    fn later(&self, now: u64, cycles: u64) -> ModelResult<u64> {
        now.checked_add(
            cycles
                .checked_mul(self.config.cycle_ps)
                .ok_or("time overflow")?,
        )
        .ok_or_else(|| "time overflow".into())
    }
    fn manager(&self, source: &str) -> usize {
        self.config
            .managers
            .iter()
            .position(|m| m.id == source)
            .expect("validated manager")
    }
    fn target(&self, address: u64) -> Option<usize> {
        self.config
            .targets
            .iter()
            .position(|t| address >= t.base && address - t.base < t.size)
    }
    fn phase(&self, address: Address) -> DataPhase {
        let r = &self.workload.requests[address.request];
        let target = self.target(beat_address(r, address.beat));
        if let Some(rule) = self.config.responses.iter().find(|rule| {
            rule.request_id == r.request_id
                && rule.beat == address.beat
                && rule.attempt == address.attempt
        }) {
            DataPhase {
                address,
                wait_remaining: rule.wait_cycles,
                response: if target.is_none() {
                    Response::Error
                } else {
                    rule.response
                },
                second_response: false,
                split_release_cycles: rule.split_release_cycles,
            }
        } else {
            DataPhase {
                address,
                wait_remaining: target.map_or(0, |t| self.config.targets[t].wait_cycles),
                response: if target.is_some() {
                    Response::Okay
                } else {
                    Response::Error
                },
                second_response: false,
                split_release_cycles: None,
            }
        }
    }
    fn select(&self, s: &mut State, now: u64, released: &[usize]) -> ModelResult<Option<Address>> {
        if s.data
            .as_ref()
            .is_some_and(|d| d.response != Response::Okay && d.second_response)
        {
            return Ok(None);
        }
        let chosen = if let Some(owner) = s.lock_owner {
            let r = &self.workload.requests[owner];
            if s.transactions[owner].done
                || s.transactions[owner].next_beat == r.beats
                || s.split_masks.contains_key(&self.manager(&r.source))
                || released.contains(&self.manager(&r.source))
            {
                return Ok(None);
            }
            if s.lock_setup_until.is_some_and(|until| now < until) {
                return Ok(None);
            }
            s.lock_setup_until = None;
            Some(owner)
        } else {
            let mut candidates = Vec::new();
            for manager in 0..self.config.managers.len() {
                if s.split_masks.contains_key(&manager) || released.contains(&manager) {
                    continue;
                }
                // A SPLIT/RETRY sequence retains the manager's one outstanding workload request.
                if let Some((i, r)) = self
                    .workload
                    .requests
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| {
                        r.source == self.config.managers[manager].id
                            && r.at_ps <= now
                            && !s.transactions[*i].done
                    })
                    .min_by_key(|(i, r)| (r.at_ps, *i))
                {
                    if s.transactions[i].next_beat < r.beats {
                        candidates.push((
                            self.config.managers[manager].priority,
                            manager,
                            r.at_ps,
                            i,
                        ));
                    }
                }
            }
            candidates.into_iter().min().map(|(_, _, _, i)| i)
        };
        let Some(i) = chosen else { return Ok(None) };
        let r = &self.workload.requests[i];
        if r.locked && s.lock_owner.is_none() {
            s.lock_owner = Some(i);
            s.lock_setup_until = Some(self.later(now, 1)?);
            return Ok(None);
        }
        let beat = s.transactions[i].next_beat;
        if beat > 0 && s.last_address != Some(i) {
            s.transactions[i].fragmented = true;
        }
        let fragmented = s.transactions[i].fragmented;
        Ok(Some(Address {
            request: i,
            beat,
            attempt: s.transactions[i].attempts[beat],
            nonseq: beat == 0 || fragmented,
            burst: if fragmented { Burst::Single } else { r.burst },
        }))
    }
    fn address_json(&self, a: &Address) -> Value {
        let r = &self.workload.requests[a.request];
        json!({"request_id":r.request_id,"source":r.source,"master":self.manager(&r.source)+1,"beat":a.beat,"attempt":a.attempt,"address":beat_address(r,a.beat).to_string(),"htrans":if a.nonseq{"NONSEQ"}else{"SEQ"},"hburst":a.burst,"hsize_bytes":r.size_bytes,"hwrite":r.operation==Operation::Write,"hmastlock":r.locked})
    }
    fn record(&self, s: &mut State, c: &mut Context<'_>, kind: &str, data: Value) -> ModelResult {
        let seq = s.sequence;
        s.sequence = seq.checked_add(1).ok_or("AHB record sequence overflow")?;
        let mut data = data;
        data["kind"] = json!(kind);
        data["effect_seq"] = json!(seq.to_string());
        c.upsert_model_record(ModelRecord {
            schema: record_schema(),
            id: format!("event-{seq}"),
            subject: c.subject().into(),
            time_ps: c.now(),
            data,
        })
    }
    fn tick(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let now = c.now();
        let released: Vec<_> = s
            .split_masks
            .iter()
            .filter_map(|(m, t)| (*t <= now).then_some(*m))
            .collect();
        for m in &released {
            s.split_masks.remove(m);
        }
        if s.address.is_none() {
            s.address = self.select(s, now, &released)?;
        }
        let lock_reservation = s
            .lock_owner
            .map(|i| self.workload.requests[i].source.clone());
        let address_owner = s
            .address
            .as_ref()
            .map(|a| self.workload.requests[a.request].source.as_str())
            .or_else(|| {
                s.lock_owner
                    .filter(|i| {
                        !s.split_masks
                            .contains_key(&self.manager(&self.workload.requests[*i].source))
                    })
                    .map(|i| self.workload.requests[i].source.as_str())
            })
            .unwrap_or("@dummy")
            .to_owned();
        let mut hready = true;
        let mut hresp = Response::Okay;
        let mut committed = false;
        let mut read_data = None;
        let mut cancel = false;
        let mut data_json = Value::Null;
        if let Some(mut d) = s.data.take() {
            let a = &d.address;
            let r = &self.workload.requests[a.request];
            let physical = self.asynchronous.get(&r.request_id);
            if physical.is_some_and(|a| a.error) {
                d.response = Response::Error;
            }
            if physical.is_some_and(|a| !a.completed[0] && !a.error) {
                hready = false;
            } else if d.wait_remaining > 0 {
                hready = false;
                d.wait_remaining -= 1;
            } else if d.response != Response::Okay {
                hresp = d.response;
                hready = d.second_response;
                cancel = true;
                d.second_response = true;
            }
            if hready {
                match hresp {
                    Response::Okay => {
                        let address = beat_address(r, a.beat);
                        let first_lane = (address % self.config.data_bytes as u64) as usize;
                        let data = if r.operation == Operation::Write {
                            r.write_data[a.beat][first_lane..first_lane + r.size_bytes].to_vec()
                        } else {
                            vec![0; r.size_bytes]
                        };
                        let enables = vec![true; r.size_bytes];
                        let request = MemoryRequest {
                            source: &r.source,
                            request_id: &r.request_id,
                            id: self.manager(&r.source) as u32,
                            address,
                            data: &data,
                            byte_enables: &enables,
                        };
                        if r.operation == Operation::Write {
                            if physical.is_none() {
                                s.memory.write(&request)?;
                            }
                            committed = true;
                            if physical.is_none() {
                                let target =
                                    self.target(address).ok_or("missing accepted AHB target")?;
                                let sequence = s.sequence;
                                s.sequence = sequence
                                    .checked_add(1)
                                    .ok_or("AHB memory record sequence overflow")?;
                                c.upsert_model_record(ModelRecord{schema:record_schema(),id:format!("memory-{}",self.config.targets[target].id),subject:c.subject().into(),time_ps:now,data:json!({"kind":"memory","effect_seq":sequence,"target":self.config.targets[target].id,"base":self.config.targets[target].base.to_string(),"bytes":s.memory.regions[target].bytes})})?;
                            }
                        } else {
                            let bytes = if let Some(a) = physical {
                                a.data.clone()
                            } else {
                                s.memory.read(&request)?.data
                            };
                            let mut lanes = vec![0; self.config.data_bytes];
                            lanes[first_lane..first_lane + r.size_bytes].copy_from_slice(&bytes);
                            read_data = Some(lanes);
                        }
                        s.transactions[a.request].completed += 1;
                        if s.transactions[a.request].completed == r.beats {
                            s.transactions[a.request].done = true;
                        }
                    }
                    Response::Error => {
                        s.transactions[a.request].done = true;
                        s.transactions[a.request].failed = true;
                    }
                    Response::Retry | Response::Split => {
                        s.transactions[a.request].next_beat = a.beat;
                        s.transactions[a.request].fragmented = true;
                        s.transactions[a.request].attempts[a.beat] =
                            a.attempt.checked_add(1).ok_or("AHB attempt overflow")?;
                        if hresp == Response::Split {
                            let delay = d.split_release_cycles.ok_or("missing HSPLIT release")?;
                            s.split_masks
                                .insert(self.manager(&r.source), self.later(now, delay)?);
                        }
                    }
                }
                if s.transactions[a.request].done && s.lock_owner == Some(a.request) {
                    s.lock_owner = None;
                    s.lock_setup_until = None;
                }
            }
            data_json = self.address_json(a);
            data_json["write_data"] = if r.operation == Operation::Write {
                json!(r.write_data[a.beat])
            } else {
                Value::Null
            };
            data_json["read_data"] = json!(read_data);
            data_json["committed"] = json!(committed);
            if !hready {
                s.data = Some(d);
            }
        }
        let address_json = s.address.as_ref().map(|a| self.address_json(a));
        let address_accepted = hready && !cancel && s.address.is_some();
        self.record(s,c,"cycle",json!({"address_phase":address_json,"address_owner":address_owner,"data_phase":data_json,"hready":hready,"hresp":hresp,"address_accepted":address_accepted,"cancelled_address":cancel && s.address.is_some(),"hsplit":released.iter().map(|m|self.config.managers[*m].id.clone()).collect::<Vec<_>>(),"masked_masters":s.split_masks.keys().map(|m|self.config.managers[*m].id.clone()).collect::<Vec<_>>(),"lock_owner":lock_reservation,"hlock":s.lock_owner.filter(|i|s.transactions[*i].next_beat+1<self.workload.requests[*i].beats || s.lock_setup_until.is_some()).map(|i|&self.workload.requests[i].source)}))?;
        if cancel {
            s.address = None;
        } else if hready {
            if let Some(address) = s.address.take() {
                s.transactions[address.request].next_beat = address.beat + 1;
                s.last_address = Some(address.request);
                s.data = Some(self.phase(address));
            }
        }
        s.last_tick = Some(now);
        Ok(())
    }
}
impl Model for Pipeline {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        if let Some(at) = self.workload.requests.iter().map(|r| r.at_ps).min() {
            self.timer = Some(c.schedule_at(at, tick_schema(), vec![])?)
        }
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        if e.schema != tick_schema() || c.current_timer_token() != self.timer.as_ref() {
            return Err("invalid AHB pipeline tick ownership".into());
        }
        let mut next = self.state.clone();
        self.tick(&mut next, c)?;
        let timer = if next.transactions.iter().any(|t| !t.done) {
            Some(c.schedule_at(self.later(c.now(), 1)?, tick_schema(), vec![])?)
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
        .map_err(|e| ModelError(format!("AHB checkpoint: {e}")))
    }
}
fn restore(
    c: &ModelConfig,
    version: u32,
    bytes: &[u8],
    context: &CheckpointDecodeContext<'_>,
) -> ModelResult<Box<dyn Model>> {
    if version != 1 {
        return Err("unsupported AHB pipeline checkpoint codec".into());
    }
    let saved: Checkpoint =
        serde_json::from_slice(bytes).map_err(|e| ModelError(format!("AHB checkpoint: {e}")))?;
    let (config, workload) = input::parse(&c.profile_input)?;
    if saved.version != 1
        || saved.config != config
        || saved.workload != workload
        || saved.state.transactions.len() != workload.requests.len()
    {
        return Err("AHB checkpoint immutable input mismatch".into());
    }
    saved.state.memory.validate(&memory_targets(&config))?;
    let probe = Pipeline {
        config: config.clone(),
        workload: workload.clone(),
        state: saved.state.clone(),
        timer: None,
        asynchronous: Default::default(),
    };
    for (i, (t, r)) in saved
        .state
        .transactions
        .iter()
        .zip(&workload.requests)
        .enumerate()
    {
        let in_data = saved
            .state
            .data
            .as_ref()
            .is_some_and(|d| d.address.request == i);
        if t.attempts.len() != r.beats
            || t.completed > t.next_beat
            || t.next_beat > r.beats
            || (t.done && !t.failed && t.completed != r.beats)
            || (t.failed && !t.done)
            || (!t.failed && t.next_beat - t.completed != usize::from(in_data))
            || ((t.next_beat > 0 || t.completed > 0 || t.done || t.attempts.iter().any(|a| *a > 0))
                && saved.state.last_tick.is_none_or(|now| r.at_ps > now))
        {
            return Err("invalid AHB checkpoint progress".into());
        }
    }
    for a in saved
        .state
        .address
        .iter()
        .chain(saved.state.data.iter().map(|d| &d.address))
    {
        let Some(r) = workload.requests.get(a.request) else {
            return Err("invalid AHB checkpoint slot request".into());
        };
        if a.beat >= r.beats
            || saved.state.transactions[a.request].done
            || a.attempt != saved.state.transactions[a.request].attempts[a.beat]
            || a.nonseq != (a.beat == 0 || saved.state.transactions[a.request].fragmented)
            || a.burst
                != if saved.state.transactions[a.request].fragmented {
                    Burst::Single
                } else {
                    r.burst
                }
        {
            return Err("invalid AHB checkpoint slot beat/attempt".into());
        }
    }
    if let Some(address) = &saved.state.address {
        if address.beat != saved.state.transactions[address.request].next_beat {
            return Err("AHB checkpoint address slot progression mismatch".into());
        }
    }
    if let Some(data) = &saved.state.data {
        let expected = probe.phase(data.address.clone());
        if saved.state.transactions[data.address.request].next_beat != data.address.beat + 1
            || saved.state.transactions[data.address.request].completed != data.address.beat
            || data.response != expected.response
            || data.split_release_cycles != expected.split_release_cycles
            || data.wait_remaining > expected.wait_remaining
            || data.second_response && (data.response == Response::Okay || data.wait_remaining != 0)
            || data.second_response && saved.state.address.is_some()
        {
            return Err("AHB checkpoint data phase/response mismatch".into());
        }
    }
    if saved.state.lock_owner.is_some_and(|i| {
        workload.requests.get(i).is_none_or(|r| !r.locked) || saved.state.transactions[i].done
    }) || saved.state.lock_setup_until.is_some() && saved.state.lock_owner.is_none()
        || saved
            .state
            .last_address
            .is_some_and(|i| i >= workload.requests.len())
        || saved.state.split_masks.iter().any(|(m, t)| {
            *m >= config.managers.len()
                || *t % config.cycle_ps != 0
                || saved.state.last_tick.is_some_and(|last| *t <= last)
        })
        || saved
            .state
            .last_tick
            .is_some_and(|t| t % config.cycle_ps != 0)
    {
        return Err("invalid AHB checkpoint arbitration ownership".into());
    }
    if let Some(owner) = saved.state.lock_owner {
        if saved
            .state
            .address
            .as_ref()
            .is_some_and(|a| a.request != owner)
        {
            return Err("AHB checkpoint locked address owner mismatch".into());
        }
    }
    for (manager, until) in &saved.state.split_masks {
        let Some((i, r)) = workload
            .requests
            .iter()
            .enumerate()
            .filter(|(i, r)| {
                r.source == config.managers[*manager].id && !saved.state.transactions[*i].done
            })
            .min_by_key(|(i, r)| (r.at_ps, *i))
        else {
            return Err("AHB checkpoint mask has no outstanding request".into());
        };
        let transaction = &saved.state.transactions[i];
        let beat = transaction.next_beat;
        if beat >= r.beats || transaction.attempts[beat] == 0 {
            return Err("AHB checkpoint mask has no split attempt".into());
        }
        let Some(rule) = config.responses.iter().find(|rule| {
            rule.request_id == r.request_id
                && rule.beat == beat
                && rule.attempt == transaction.attempts[beat] - 1
                && rule.response == Response::Split
        }) else {
            return Err("AHB checkpoint mask is not a SPLIT response".into());
        };
        if *until
            > probe.later(
                saved
                    .state
                    .last_tick
                    .ok_or("AHB checkpoint mask precedes initialization")?,
                rule.split_release_cycles.ok_or("missing split release")?,
            )?
            || saved.state.address.as_ref().is_some_and(|a| a.request == i)
            || saved
                .state
                .data
                .as_ref()
                .is_some_and(|d| d.address.request == i)
        {
            return Err("AHB checkpoint split release/slot mismatch".into());
        }
    }
    let timer = saved
        .timer
        .as_ref()
        .map(|t| context.decode_timer_ref(t))
        .transpose()?;
    if saved.state.transactions.iter().any(|t| !t.done) && timer.is_none() {
        return Err("missing AHB checkpoint live tick".into());
    }
    if saved.state.transactions.iter().any(|t| !t.done)
        && saved.timer.as_ref().is_none_or(|t| t.state != "live")
        || saved.state.transactions.iter().all(|t| t.done) && timer.is_some()
    {
        return Err("AHB checkpoint tick liveness mismatch".into());
    }
    Ok(Box::new(Pipeline {
        config,
        workload,
        state: saved.state,
        timer,
        asynchronous: Default::default(),
    }))
}
pub fn register_ahb_pipeline(r: &mut Registry) -> Result<(), Diagnostic> {
    r.register_event(EventDescriptor {
        schema: tick_schema(),
        phase: 1,
        validate: |bytes| {
            if bytes.is_empty() {
                Ok(())
            } else {
                Err("invalid AHB tick payload".into())
            }
        },
    })?;
    r.register_module(
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
    r.register_model_record(ModelRecordDescriptor {
        schema: record_schema(),
        validate: |v| {
            if v.get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| matches!(k, "cycle" | "memory"))
            {
                Ok(())
            } else {
                Err("invalid AHB record".into())
            }
        },
    })?;
    r.register_profile(ProfileDescriptor {
        name: PROFILE.into(),
        implementation_version: "1".into(),
        modules: vec![IMPLEMENTATION.into()],
        channels: vec![],
        events: vec![tick_schema()],
        metrics: vec![],
        model_records: vec![record_schema()],
        output_schema_version: 3,
        validate: |models, channels| {
            if models.len() == 1 && channels.is_empty() {
                Ok(())
            } else {
                Err("AHB pipeline requires one execution owner and no external channels".into())
            }
        },
    })?;
    r.register_profile_input_validator(PROFILE, input::validate_input)?;
    r.register_execution_capabilities(
        PROFILE,
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: false,
            checkpoint: true,
        },
    )?;
    r.register_model_checkpoint_codec(
        CheckpointCodecDescriptor {
            implementation_key: IMPLEMENTATION.into(),
            implementation_version: "1".into(),
            codec_name: "ahb.pipeline.state".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 268_435_456,
        },
        restore,
    )?;
    Ok(())
}
