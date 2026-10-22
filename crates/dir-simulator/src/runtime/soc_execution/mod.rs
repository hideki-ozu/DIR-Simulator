//! One instruction/coherence/transport/memory timeline.
mod checkpoint;
mod coherence;
pub mod cpu;
mod ddr;
mod ipc;
mod ipc_checkpoint;
mod mailbox;
mod memory;
mod state;
mod transport;
use super::noc_flit::service::FlitService;
use super::transaction_fabric::memory::{MemoryRequest, MemoryService};
use crate::registry::*;
use crate::types::{Diagnostic, soc_execution::*};
use serde::{Deserialize, Serialize};
use serde_json::json;
use state::*;
use std::collections::{BTreeMap, BTreeSet};
pub const PROFILE: &str = "soc.execution.v1";
const IMPLEMENTATION: &str = "dir.soc.Execution";
fn tick_schema() -> Schema {
    Schema::new("dir.soc.ExecutionTick", 1)
}
fn record_schema() -> Schema {
    Schema::new("dir.soc.ExecutionRecord", 1)
}
struct Soc {
    config: Config,
    workload: Workload,
    state: State,
    sources: Vec<String>,
    timer: Option<TimerToken>,
}
fn source_ids(c: &Config, w: &Workload) -> Vec<String> {
    let mut s: BTreeSet<String> = c.cores.iter().map(|c| c.id.clone()).collect();
    s.extend(w.requests.iter().map(|r| r.source.clone()));
    s.extend(w.dma.iter().map(|d| d.source.clone()));
    s.extend(
        c.mailboxes
            .iter()
            .flat_map(|m| [m.producer.clone(), m.consumer.clone()]),
    );
    s.into_iter().collect()
}
fn factory(model: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload) = crate::input::soc_execution::parse(&model.profile_input)?;
    let sources = source_ids(&config, &workload);
    let state = State {
        memory: MemoryService::new(&memory::targets(&config)),
        ddr: config.ddr.as_ref().map(ddr::Bridge::new).transpose()?,
        cores: config
            .cores
            .iter()
            .map(|c| CoreRuntime {
                architectural: cpu::Core::new(c.reset_pc, c.initial_sp),
                phase: CorePhase::Fetch,
                ready: 0,
                fetch_transaction: None,
                instruction_started: 0,
            })
            .collect(),
        caches: vec![vec![None; config.cache.sets * config.cache.ways]; config.cores.len()],
        directory: BTreeMap::new(),
        transactions: vec![],
        external: vec![None; workload.requests.len()],
        dma: workload
            .dma
            .iter()
            .map(|_| DmaRuntime {
                phase: DmaPhase::Pending,
                offset: 0,
                staging: vec![],
                cancelled: false,
            })
            .collect(),
        commands: 0,
        slots: config
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
            .collect(),
        mailboxes: config
            .mailboxes
            .iter()
            .map(|_| mailbox::Endpoint::default())
            .collect(),
        mailbox_accesses: BTreeMap::new(),
        ipc_events: vec![],
        evictions: vec![],
        noc: FlitService::new(config.noc.clone()),
        axi: super::transaction_fabric::service::AxiService::new(
            config.cycle_ps,
            config.fabric.data_bytes,
            &sources,
            &memory::native_targets(&config),
        ),
        ahb: super::ahb_pipeline::service::AhbService::new(
            config.cycle_ps,
            config.fabric.data_bytes,
            &sources,
            &memory::native_targets(&config),
            config.fabric.ahb_wait_cycles,
        ),
        native_deliveries: vec![],
        noc_deliveries: vec![],
        sequence: 0,
        age: 0,
        admission_cursor: 0,
        axi_write_owner: None,
        last_tick: None,
        next_tick: None,
        exported: false,
    };
    Ok(Box::new(Soc {
        config,
        workload,
        state,
        sources,
        timer: None,
    }))
}
impl Soc {
    pub(super) fn atomic_reserved(t: &Transaction) -> bool {
        t.request.operation == Operation::Atomic
            && t.phase != Phase::Done
            && (!matches!(t.phase, Phase::Queued | Phase::Ingress)
                || t.address_accepted
                || t.w_accepted > 0)
    }
    fn later(&self, now: u64, cycles: u64) -> ModelResult<u64> {
        now.checked_add(
            cycles
                .checked_mul(self.config.cycle_ps)
                .ok_or("SoC time overflow")?,
        )
        .ok_or_else(|| "SoC time overflow".into())
    }
    fn emit(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        kind: &str,
        txn: Option<usize>,
        detail: serde_json::Value,
    ) -> ModelResult {
        let seq = s.sequence;
        s.sequence = seq.checked_add(1).ok_or("SoC sequence overflow")?;
        let mut data = json!({"kind":kind,"effect_seq":seq,"phase":1,"detail":detail});
        if let Some(i) = txn {
            let t = &s.transactions[i];
            data["request_id"] = json!(t.request.id);
            data["source"] = json!(t.request.source);
            data["transaction"] = json!(i);
        }
        c.upsert_model_record(ModelRecord {
            schema: record_schema(),
            id: format!("event-{seq}"),
            subject: c.subject().into(),
            time_ps: c.now(),
            data,
        })
    }
    fn terminal_cores(&self, s: &State) -> bool {
        s.cores
            .iter()
            .all(|c| matches!(c.phase, CorePhase::Halted | CorePhase::BudgetPaused))
    }
    fn pending(&self, s: &State) -> bool {
        !self.terminal_cores(s)
            || s.external.iter().any(Option::is_none)
            || s.transactions.iter().any(|t| t.phase != Phase::Done)
            || s.dma
                .iter()
                .any(|d| !matches!(d.phase, DmaPhase::Done | DmaPhase::Failed))
            || s.commands < self.workload.commands.len()
            || !s.evictions.is_empty()
            || !s.noc.idle()
            || !s.noc_deliveries.is_empty()
            || !s.exported
    }
    fn next_time(&self, s: &State, now: u64) -> ModelResult<Option<u64>> {
        if !self.pending(s) && s.ddr.is_none() {
            return Ok(None);
        }
        let next = self.later(now, 1)?;
        let mut candidates = vec![];
        let mut local = !s.native_deliveries.is_empty()
            || !s.noc_deliveries.is_empty()
            || !s.noc.idle()
            || !s.axi.idle() && !s.axi.waiting_only_target()
            || !s.ahb.idle() && !s.ahb.waiting_only_target();
        let capacity = s
            .transactions
            .iter()
            .filter(|t| t.phase != Phase::Done)
            .count()
            < self.config.fabric.queue_capacity;
        let mshrs = s
            .directory
            .values()
            .filter(|d| d.transient.is_some())
            .count()
            < self.config.cache.mshrs;
        for (i, t) in s.transactions.iter().enumerate() {
            match t.phase {
                Phase::Queued | Phase::Ingress | Phase::Response => local = true,
                Phase::OwnerWait if mshrs => {
                    let address = t
                        .request
                        .address
                        .checked_add(t.offset as u32)
                        .ok_or("scheduler address overflow")?;
                    if s.directory
                        .get(&self.line(address))
                        .is_none_or(|d| d.transient.is_none())
                    {
                        local = true;
                    }
                }
                Phase::Coherence => {
                    if t.owner_ready > now {
                        candidates.push(t.owner_ready);
                    }
                    for ack in &t.acknowledgments {
                        if ack.at > now {
                            candidates.push(ack.at);
                        }
                    }
                    if !self.transaction_backend_wait(s, i)
                        && t.owner_ready <= now
                        && t.acknowledgments.iter().all(|a| a.at <= now)
                    {
                        local = true;
                    }
                }
                _ => {}
            }
        }
        for access in s.mailbox_accesses.values().filter(|a| !a.applied) {
            candidates.push(access.ready.max(next));
        }
        for e in &s.evictions {
            if e.at > now {
                candidates.push(e.at);
            } else if !self.backend_wait(s, &format!("eviction:{}:{}:{}", e.core, e.line, e.epoch))
            {
                local = true;
            }
        }
        for core in &s.cores {
            match core.phase {
                CorePhase::Fetch if capacity => {
                    if core.ready > now {
                        candidates.push(core.ready);
                    } else {
                        local = true;
                    }
                }
                CorePhase::Execute { ready, .. } => {
                    if ready > now {
                        candidates.push(ready);
                    } else if capacity {
                        local = true;
                    }
                }
                CorePhase::Fence { .. }
                    if s.transactions.iter().all(|t| t.phase == Phase::Done)
                        && s.evictions.is_empty() =>
                {
                    local = true
                }
                _ => {}
            }
        }
        for (d, cfg) in s.dma.iter().zip(&self.workload.dma) {
            match d.phase {
                DmaPhase::Pending => {
                    if cfg.at_ps > now {
                        candidates.push(cfg.at_ps);
                    } else {
                        local = true;
                    }
                }
                DmaPhase::Setup { until } => {
                    if until > now {
                        candidates.push(until);
                    } else {
                        local = true;
                    }
                }
                DmaPhase::Read | DmaPhase::Write if capacity => local = true,
                _ => {}
            }
        }
        if capacity {
            for (r, t) in self.workload.requests.iter().zip(&s.external) {
                if t.is_none() {
                    if r.at_ps > now {
                        candidates.push(r.at_ps);
                    } else {
                        local = true;
                    }
                }
            }
        }
        if let Some(cmd) = self.workload.commands.get(s.commands) {
            if cmd.at_ps > now {
                candidates.push(cmd.at_ps);
            } else {
                local = true;
            }
        }
        if let Some(d) = &s.ddr {
            if let Some(at) = d.service.next_due_ps() {
                candidates.push(at.max(next));
            }
        }
        if local
            || !s.exported
                && self.terminal_cores(s)
                && s.external.iter().all(Option::is_some)
                && s.commands == self.workload.commands.len()
                && s.dma
                    .iter()
                    .all(|d| matches!(d.phase, DmaPhase::Done | DmaPhase::Failed))
                && s.transactions.iter().all(|t| t.phase == Phase::Done)
                && s.evictions.is_empty()
        {
            candidates.push(next);
        }
        Ok(candidates.into_iter().min())
    }
    #[allow(clippy::too_many_arguments)]
    fn cpu_request(
        &self,
        s: &State,
        core: usize,
        operation: Operation,
        address: u32,
        length: usize,
        data: Vec<u8>,
        label: &str,
        now: u64,
    ) -> Request {
        let cfg = &self.config.cores[core];
        Request {
            id: format!("{}:{}:{label}", cfg.id, s.cores[core].architectural.retired),
            source: cfg.id.clone(),
            at_ps: now,
            operation,
            address,
            length,
            transport: cfg.transport,
            router: cfg.router,
            axi_id: 0,
            qos: cfg.qos,
            data,
            byte_enables: vec![],
            atomic: None,
            r_stall_cycles: 0,
            b_stall_cycles: 0,
            lease: None,
        }
    }
    fn trap(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        core: usize,
        trap: cpu::Trap,
    ) -> ModelResult {
        s.cores[core].architectural.trap = Some(trap.clone());
        s.cores[core].phase = CorePhase::Halted;
        self.emit(
            s,
            c,
            "cpu_trap",
            None,
            json!({"core":self.config.cores[core].id,"trap":trap}),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn retire(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        core: usize,
        instruction: u32,
        opcode: &str,
        rd: Option<(u8, u32)>,
        next_pc: u32,
        txn: Option<usize>,
    ) -> ModelResult {
        let pre = s.cores[core].architectural.pc;
        let old = rd.map(|(r, _)| s.cores[core].architectural.regs[r as usize]);
        s.cores[core]
            .architectural
            .retire(rd, next_pc)
            .map_err(ModelError)?;
        s.cores[core].phase = CorePhase::Fetch;
        s.cores[core].ready = self.later(c.now(), self.config.cores[core].cycle_cycles)?;
        self.emit(s,c,"cpu_retired",txn,json!({"core":self.config.cores[core].id,"instruction":instruction,"opcode":opcode,"pre_pc":pre,"post_pc":next_pc,"register":rd.filter(|(r,_)|*r!=0),"old_register":old,"retired":s.cores[core].architectural.retired,"fetch_transaction":s.cores[core].fetch_transaction,"memory_transaction":txn,"elapsed_cycles":(c.now()-s.cores[core].instruction_started)/self.config.cycle_ps}))
    }
    fn execute_cores(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        for core in 0..s.cores.len() {
            let phase = s.cores[core].phase.clone();
            match phase {
                CorePhase::Fetch
                    if s.cores[core].architectural.retired
                        >= self.config.cores[core].instruction_budget =>
                {
                    s.cores[core].phase = CorePhase::BudgetPaused;
                    self.emit(s,c,"cpu_budget_paused",None,json!({"core":self.config.cores[core].id,"pc":s.cores[core].architectural.pc,"retired":s.cores[core].architectural.retired}))?;
                }
                CorePhase::Execute { instruction, ready } if ready <= c.now() => {
                    match cpu::evaluate(&s.cores[core].architectural, instruction) {
                        Err(t) => self.trap(s, c, core, t)?,
                        Ok(a) => match a.effect {
                            cpu::Effect::Retire { rd, next_pc } => {
                                self.retire(s, c, core, instruction, &a.opcode, rd, next_pc, None)?
                            }
                            cpu::Effect::Fence { next_pc } => {
                                s.cores[core].phase = CorePhase::Fence {
                                    instruction,
                                    next_pc,
                                };
                                self.emit(s,c,"cpu_stall",None,json!({"core":self.config.cores[core].id,"reason":"fence_drain"}))?;
                            }
                            _ => {}
                        },
                    }
                }
                CorePhase::Fence {
                    instruction,
                    next_pc,
                } if s.transactions.iter().all(|t| t.phase == Phase::Done)
                    && s.evictions.is_empty() =>
                {
                    self.retire(s, c, core, instruction, "FENCE", None, next_pc, None)?
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn admit(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        if self.sources.is_empty() {
            return Ok(());
        }
        let count = self.sources.len();
        let mut last = None;
        for offset in 0..count {
            if s.transactions
                .iter()
                .filter(|t| t.phase != Phase::Done)
                .count()
                >= self.config.fabric.queue_capacity
            {
                break;
            }
            let src = (s.admission_cursor + offset) % count;
            let source = &self.sources[src];
            let mut candidate = None;
            for core in 0..s.cores.len() {
                if self.config.cores[core].id != *source || s.cores[core].ready > c.now() {
                    continue;
                }
                match s.cores[core].phase.clone() {
                    CorePhase::Fetch => {
                        candidate = Some((
                            self.cpu_request(
                                s,
                                core,
                                Operation::Read,
                                s.cores[core].architectural.pc,
                                4,
                                vec![],
                                "fetch",
                                c.now(),
                            ),
                            Client::Fetch { core },
                            None,
                        ));
                    }
                    CorePhase::Execute { instruction, ready } if ready <= c.now() => {
                        if let Ok(action) = cpu::evaluate(&s.cores[core].architectural, instruction)
                        {
                            match action.effect {
                                cpu::Effect::Load { address, bytes, .. } => {
                                    candidate = Some((
                                        self.cpu_request(
                                            s,
                                            core,
                                            Operation::Read,
                                            address,
                                            bytes,
                                            vec![],
                                            "load",
                                            c.now(),
                                        ),
                                        Client::Load { core },
                                        Some((instruction, action)),
                                    ))
                                }
                                cpu::Effect::Store {
                                    address,
                                    bytes,
                                    value,
                                    ..
                                } => {
                                    candidate = Some((
                                        self.cpu_request(
                                            s,
                                            core,
                                            Operation::Write,
                                            address,
                                            bytes,
                                            value.to_le_bytes()[..bytes].to_vec(),
                                            "store",
                                            c.now(),
                                        ),
                                        Client::Store { core },
                                        Some((instruction, action)),
                                    ))
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            if candidate.is_none() {
                if let Some((i, r)) = self.workload.requests.iter().enumerate().find(|(i, r)| {
                    r.source == *source && r.at_ps <= c.now() && s.external[*i].is_none()
                }) {
                    candidate = Some((r.clone(), Client::External { index: i }, None));
                }
            }
            if candidate.is_none() {
                candidate = self
                    .dma_candidate(s, source, c.now())?
                    .map(|(r, cl)| (r, cl, None));
            }
            if let Some((request, client, action)) = candidate {
                let i = s.transactions.len();
                s.transactions.push(Transaction {
                    request,
                    client: client.clone(),
                    phase: Phase::Queued,
                    admitted: c.now(),
                    admitted_sequence: s.sequence,
                    completed_sequence: None,
                    completed_at: None,
                    offset: 0,
                    result: vec![],
                    wire_data: vec![],
                    error: None,
                    lease_read_active: false,
                    address_accepted: false,
                    w_accepted: 0,
                    ingress_sent: false,
                    ingress_delivered: false,
                    cache_hit: false,
                    line: None,
                    acknowledgments: vec![],
                    intervention: None,
                    owner_ready: 0,
                    grant_order: None,
                    response_ready: None,
                    r_sent: 0,
                    b_sent: false,
                    response_sent: false,
                    response_delivered: false,
                });
                match client {
                    Client::Fetch { core } => {
                        s.cores[core].phase = CorePhase::Fetching { txn: i };
                        s.cores[core].fetch_transaction = Some(i);
                        s.cores[core].instruction_started = c.now();
                    }
                    Client::Load { core } | Client::Store { core } => {
                        let (instruction, action) = action.ok_or("missing CPU data action")?;
                        s.cores[core].phase = CorePhase::Memory {
                            instruction,
                            action,
                            txn: i,
                        };
                    }
                    Client::External { index } => s.external[index] = Some(i),
                    Client::DmaRead { index } => s.dma[index].phase = DmaPhase::Reading { txn: i },
                    Client::DmaWrite { index } => s.dma[index].phase = DmaPhase::Writing { txn: i },
                }
                self.begin_lease_read(s, i)?;
                self.emit(s,c,"request_admitted",Some(i),json!({"base_qos":s.transactions[i].request.qos,"queue_capacity":self.config.fabric.queue_capacity,"admission_policy":"rotating_source_v1"}))?;
                last = Some(src);
            }
        }
        if let Some(src) = last {
            s.admission_cursor = (src + 1) % count;
        }
        Ok(())
    }
    fn finish_transaction(&self, s: &mut State, c: &mut Context<'_>, i: usize) -> ModelResult {
        self.release_line(s, i)?;
        if let Some(e) = self.mailbox_index(
            s.transactions[i].request.address,
            s.transactions[i].request.length,
        ) {
            if s.mailboxes[e].pending == Some(i) {
                s.mailboxes[e].pending = None;
            }
        }
        s.transactions[i].phase = Phase::Done;
        self.end_lease_read(s, i)?;
        s.transactions[i].completed_sequence = Some(s.sequence);
        s.transactions[i].completed_at = Some(c.now());
        let t = s.transactions[i].clone();
        self.emit(s,c,"request_completed",Some(i),json!({"status":t.error.as_deref().unwrap_or("okay"),"data":t.result,"completed_bytes":if t.error.is_none(){t.request.length}else{t.offset}}))?;
        match t.client {
            Client::Fetch { core } => {
                if t.error.is_some() {
                    self.trap(
                        s,
                        c,
                        core,
                        cpu::Trap {
                            cause: cpu::Cause::InstructionAccessFault,
                            pc: s.cores[core].architectural.pc,
                            instruction: None,
                            tval: t.request.address,
                        },
                    )?;
                } else {
                    s.cores[core].phase = CorePhase::Execute {
                        instruction: u32::from_le_bytes(
                            t.result
                                .get(..4)
                                .ok_or("short instruction response")?
                                .try_into()
                                .map_err(|_| ModelError("invalid instruction response".into()))?,
                        ),
                        ready: self.later(
                            c.now(),
                            self.config.cores[core]
                                .execute_cycles
                                .checked_mul(self.config.cores[core].cycle_cycles)
                                .ok_or("core execute time overflow")?,
                        )?,
                    };
                }
            }
            Client::Load { core } | Client::Store { core } => {
                let CorePhase::Memory {
                    instruction,
                    action,
                    ..
                } = s.cores[core].phase.clone()
                else {
                    return Err("CPU completion without pending instruction".into());
                };
                if t.error.is_some() {
                    self.trap(
                        s,
                        c,
                        core,
                        cpu::Trap {
                            cause: if t.request.operation == Operation::Read {
                                cpu::Cause::LoadAccessFault
                            } else {
                                cpu::Cause::StoreAccessFault
                            },
                            pc: s.cores[core].architectural.pc,
                            instruction: Some(instruction),
                            tval: t.request.address,
                        },
                    )?;
                } else {
                    let (rd, next) = match action.effect {
                        cpu::Effect::Load {
                            rd,
                            signed,
                            next_pc,
                            ..
                        } => {
                            if t.result.len() != t.request.length {
                                return Err("short CPU load response".into());
                            }
                            (Some((rd, cpu::load_value(&t.result, signed))), next_pc)
                        }
                        cpu::Effect::Store { next_pc, .. } => (None, next_pc),
                        _ => return Err("wrong deferred CPU action".into()),
                    };
                    self.retire(s, c, core, instruction, &action.opcode, rd, next, Some(i))?;
                }
            }
            Client::External { .. } => {}
            Client::DmaRead { index } | Client::DmaWrite { index } => {
                self.dma_completion(s, c, index, i)?
            }
        }
        Ok(())
    }
    fn tick(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        self.advance_ddr(s, c)?;
        self.advance_mailboxes(s, c)?;
        self.transport_completions(s, c)?;
        self.complete_evictions(s, c)?;
        self.complete_coherence(s, c)?;
        self.commands(s, c)?;
        self.execute_cores(s, c)?;
        self.setup_dma(s, c)?;
        self.admit(s, c)?;
        self.transport_ingress(s, c)?;
        self.arbitrate_owner(s, c)?;
        self.flush_final(s, c)?;
        let atomic_reserved = s.transactions.iter().any(Self::atomic_reserved);
        let axi = if atomic_reserved {
            vec![]
        } else {
            s.axi.advance(&mut s.memory, c, &mut s.sequence)?
        };
        let ahb = s.ahb.advance(&mut s.memory, c, &mut s.sequence)?;
        self.collect_native_targets(s, c)?;
        self.advance_ddr(s, c)?;
        s.native_deliveries = axi
            .into_iter()
            .map(|d| (Transport::Axi, d.id, d.data, d.error))
            .chain(
                ahb.into_iter()
                    .map(|d| (Transport::Ahb, d.id, d.data, d.error)),
            )
            .collect();
        s.noc_deliveries = s
            .noc
            .advance(c, &mut s.sequence)?
            .into_iter()
            .map(|d| (d.id, d.payload, d.status))
            .collect();
        let done = self.terminal_cores(s)
            && s.external.iter().all(Option::is_some)
            && s.transactions.iter().all(|t| t.phase == Phase::Done)
            && s.dma
                .iter()
                .all(|d| matches!(d.phase, DmaPhase::Done | DmaPhase::Failed))
            && s.commands == self.workload.commands.len()
            && s.evictions.is_empty()
            && s.caches
                .iter()
                .flatten()
                .flatten()
                .all(|l| l.state != Mesi::M)
            && s.noc.idle()
            && s.noc_deliveries.is_empty();
        let done = done && s.axi.idle() && s.ahb.idle() && s.native_deliveries.is_empty();
        if done && !s.exported {
            self.emit(
                s,
                c,
                "memory_final",
                None,
                json!({"regions":s.memory.regions,"cores":s.cores,"slots":s.slots}),
            )?;
            s.exported = true;
        }
        Ok(())
    }
}
impl Model for Soc {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        self.state.next_tick = Some(c.now());
        self.timer = Some(c.schedule_at(c.now(), tick_schema(), vec![])?);
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        if e.schema != tick_schema()
            || c.current_timer_token() != self.timer.as_ref()
            || self.state.next_tick != Some(c.now())
        {
            return Err("invalid SoC tick owner/time".into());
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
pub fn register_soc_execution(r: &mut Registry) -> Result<(), Diagnostic> {
    r.register_event(EventDescriptor {
        schema: tick_schema(),
        phase: 1,
        validate: |b| {
            if b.is_empty() {
                Ok(())
            } else {
                Err("SoC tick requires empty payload".into())
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
            if v["kind"].is_string() && v["effect_seq"].is_u64() {
                Ok(())
            } else {
                Err("invalid SoC execution record".into())
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
        model_records: vec![
            record_schema(),
            Schema::new("dir.noc.FlitRecord", 1),
            Schema::new("fabric.Record", 1),
            Schema::new("ahb.pipeline.Record", 1),
        ],
        output_schema_version: 3,
        validate: |m, c| {
            if m.len() == 1 && c.is_empty() {
                Ok(())
            } else {
                Err("SoC execution requires one owner and no external channels".into())
            }
        },
    })?;
    r.register_profile_input_validator(PROFILE, crate::input::soc_execution::validate_input)?;
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
            codec_name: "dir.soc.execution.state".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 268_435_456,
        },
        checkpoint::restore,
    )?;
    Ok(())
}
