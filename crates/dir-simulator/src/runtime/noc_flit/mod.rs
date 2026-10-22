//! Finite-buffer credit-controlled flit execution owned by one registered model.
mod checkpoint;
mod router;
pub(crate) mod service;
pub mod topology;
use crate::registry::*;
use crate::types::{Diagnostic, noc_flit::*};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use topology::{Routing, Topology};
pub const PROFILE: &str = "noc.flit.v1";
const IMPLEMENTATION: &str = "dir.noc.Flit";
const ESCAPE: &str = "spanning_tree_up_down_v1";
const FAULT_POLICY: &str = "topology_epoch_abort_v1";
fn tick_schema() -> Schema {
    Schema::new("dir.noc.FlitTick", 1)
}
fn record_schema() -> Schema {
    Schema::new("dir.noc.FlitRecord", 1)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PacketState {
    admitted: bool,
    injected: bool,
    terminal: Option<String>,
    injected_flits: usize,
    consumed_flits: usize,
    payload: Vec<u8>,
    escape: bool,
    stalls: u64,
    local_vc: Option<usize>,
    route: Vec<usize>,
    released: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Flit {
    packet: usize,
    index: usize,
    epoch: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Buffer {
    owner: Option<usize>,
    fifo: VecDeque<Flit>,
    reservation: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transit {
    flit: Flit,
    source_vc: usize,
    destination_vc: usize,
    at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credit {
    buffer: usize,
    at: u64,
    epoch: u64,
    packet: usize,
    flit_index: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ejection {
    flit: Flit,
    at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    packets: Vec<PacketState>,
    healthy: Vec<bool>,
    routing: Routing,
    epoch: u64,
    buffers: Vec<Buffer>,
    credits: Vec<usize>,
    pipeline: Vec<Transit>,
    returns: Vec<Credit>,
    sources: Vec<VecDeque<usize>>,
    ejection: Vec<VecDeque<Ejection>>,
    rr: Vec<usize>,
    output_cursor: usize,
    eject_rr: Vec<usize>,
    fault_cursor: usize,
    sequence: u64,
    last_tick: Option<u64>,
    next_tick: Option<u64>,
}
struct Network {
    config: FlitConfig,
    workload: FlitWorkload,
    topology: Topology,
    state: State,
    timer: Option<TimerToken>,
}
impl Network {
    fn vcs(&self) -> usize {
        self.config.adaptive_vcs + 1
    }
    fn physical_buffers(&self) -> usize {
        self.topology.links.len() * self.vcs()
    }
    fn router(&self, buffer: usize) -> usize {
        if buffer < self.physical_buffers() {
            self.topology.links[buffer / self.vcs()].destination
        } else {
            (buffer - self.physical_buffers()) / self.vcs()
        }
    }
    fn input_port(&self, buffer: usize) -> usize {
        buffer / self.vcs()
    }
    fn later(&self, now: u64, cycles: u64) -> ModelResult<u64> {
        now.checked_add(
            cycles
                .checked_mul(self.config.cycle_ps)
                .ok_or("NoC time overflow")?,
        )
        .ok_or_else(|| "NoC time overflow".into())
    }
    fn fresh_buffers(&self) -> Vec<Buffer> {
        vec![Buffer::default(); (self.topology.links.len() + self.topology.nodes) * self.vcs()]
    }
    fn emit(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        kind: &str,
        flit: Option<&Flit>,
        extra: serde_json::Value,
    ) -> ModelResult {
        let seq = s.sequence;
        s.sequence = seq.checked_add(1).ok_or("NoC record sequence overflow")?;
        let mut data = json!({"kind":kind,"effect_seq":seq,"epoch":s.epoch,"escape_policy":ESCAPE,"fault_policy":FAULT_POLICY,"detail":extra});
        if let Some(f) = flit {
            data["packet_id"] = json!(self.workload.packets[f.packet].id);
            data["flit_index"] = json!(f.index);
        }
        c.upsert_model_record(ModelRecord {
            schema: record_schema(),
            id: format!("event-{seq}"),
            subject: c.subject().into(),
            time_ps: c.now(),
            data,
        })
    }
    fn packet_event(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        kind: &str,
        p: usize,
        extra: serde_json::Value,
    ) -> ModelResult {
        self.emit(
            s,
            c,
            kind,
            Some(&Flit {
                packet: p,
                index: 0,
                epoch: s.epoch,
            }),
            extra,
        )
    }
    fn next_time(&self, s: &State, now: u64) -> ModelResult<Option<u64>> {
        if s.packets.iter().any(|p| p.admitted && p.terminal.is_none())
            || !s.pipeline.is_empty()
            || !s.returns.is_empty()
            || s.ejection.iter().any(|q| !q.is_empty())
        {
            return Ok(Some(self.later(now, 1)?));
        }
        Ok(self
            .workload
            .packets
            .iter()
            .zip(&s.packets)
            .filter(|(_, p)| !p.admitted && p.terminal.is_none())
            .map(|(p, _)| p.at_ps)
            .chain(self.workload.faults.get(s.fault_cursor).map(|f| f.at_ps))
            .min())
    }
}
fn factory(c: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload) = crate::input::noc_flit::parse(&c.profile_input)?;
    Ok(Box::new(new_network(config, workload)))
}
fn new_network(config: FlitConfig, workload: FlitWorkload) -> Network {
    let topology = Topology::new(&config.topology);
    let vcs = config.adaptive_vcs + 1;
    let healthy = vec![true; topology.links.len()];
    let state = State {
        packets: vec![PacketState::default(); workload.packets.len()],
        routing: topology.routing(&healthy),
        healthy,
        epoch: 0,
        buffers: vec![Buffer::default(); (topology.links.len() + topology.nodes) * vcs],
        credits: vec![config.buffer_flits; topology.links.len() * vcs],
        pipeline: vec![],
        returns: vec![],
        sources: vec![VecDeque::new(); topology.nodes],
        ejection: vec![VecDeque::new(); topology.nodes],
        rr: vec![0; topology.links.len()],
        output_cursor: 0,
        eject_rr: vec![0; topology.nodes],
        fault_cursor: 0,
        sequence: 0,
        last_tick: None,
        next_tick: None,
    };
    Network {
        config,
        workload,
        topology,
        state,
        timer: None,
    }
}
impl Model for Network {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        self.state.next_tick = self.next_time(&self.state, c.now())?;
        self.timer = self
            .state
            .next_tick
            .map(|at| c.schedule_at(at, tick_schema(), vec![]))
            .transpose()?;
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        if e.schema != tick_schema()
            || c.current_timer_token() != self.timer.as_ref()
            || self.state.next_tick != Some(c.now())
        {
            return Err("invalid NoC tick owner/time".into());
        }
        let mut next = self.state.clone();
        self.tick(&mut next, c)?;
        next.last_tick = Some(c.now());
        next.next_tick = self.next_time(&next, c.now())?;
        self.validate_state(&next)?;
        let timer = next
            .next_tick
            .map(|at| c.schedule_at(at, tick_schema(), vec![]))
            .transpose()?;
        self.state = next;
        self.timer = timer;
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        self.encode(c)
    }
}
pub fn register_noc_flit(r: &mut Registry) -> Result<(), Diagnostic> {
    r.register_event(EventDescriptor {
        schema: tick_schema(),
        phase: 1,
        validate: |b| {
            if b.is_empty() {
                Ok(())
            } else {
                Err("NoC tick payload must be empty".into())
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
            if v["effect_seq"].is_u64()
                && v["epoch"].is_u64()
                && matches!(
                    v["kind"].as_str(),
                    Some(
                        "packet_admitted"
                            | "injection_rejected"
                            | "vc_allocated"
                            | "vc_released"
                            | "flit_launched"
                            | "flit_arrived"
                            | "flit_injected"
                            | "flit_ejected"
                            | "credit_returned"
                            | "escape_entered"
                            | "topology_changed"
                            | "packet_delivered"
                            | "unreachable"
                            | "topology_epoch_aborted"
                    )
                )
            {
                Ok(())
            } else {
                Err("invalid NoC record".into())
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
        validate: |m, c| {
            if m.len() == 1 && c.is_empty() {
                Ok(())
            } else {
                Err("NoC requires exactly one Flit owner and no external channels".into())
            }
        },
    })?;
    r.register_profile_input_validator(PROFILE, crate::input::noc_flit::validate_input)?;
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
            codec_name: "dir.noc.flit.state".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 268_435_456,
        },
        checkpoint::restore,
    )?;
    Ok(())
}
