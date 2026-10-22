use crate::registry::{ModelError, ModelResult, ProfileInput};
use crate::types::soc_execution::*;
use std::collections::BTreeSet;
pub(crate) fn parse(input: &ProfileInput) -> ModelResult<(Config, Workload)> {
    let c: Config = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("SoC execution requires model-config")?,
    )
    .map_err(|e| ModelError(format!("SoC config: {e}")))?;
    let mut w: Workload = serde_json::from_value(
        input
            .workload
            .clone()
            .ok_or("SoC execution requires workload")?,
    )
    .map_err(|e| ModelError(format!("SoC workload: {e}")))?;
    validate(&c, &w)?;
    w.requests
        .sort_by(|a, b| (a.at_ps, &a.id).cmp(&(b.at_ps, &b.id)));
    w.dma
        .sort_by(|a, b| (a.at_ps, &a.id).cmp(&(b.at_ps, &b.id)));
    w.commands
        .sort_by(|a, b| (a.at_ps, &a.id).cmp(&(b.at_ps, &b.id)));
    Ok((c, w))
}
pub(crate) fn validate_input(input: &ProfileInput) -> ModelResult {
    parse(input).map(|_| ())
}
fn unique<'a>(ids: impl Iterator<Item = &'a String>) -> bool {
    let mut seen = BTreeSet::new();
    ids.into_iter().all(|id| !id.is_empty() && seen.insert(id))
}
pub(crate) fn mapped(c: &Config, address: u32, length: usize, write: bool, fetch: bool) -> bool {
    length > 0
        && c.memory.iter().any(|r| {
            address >= r.base
                && (address as u64 + length as u64) <= r.base as u64 + r.size as u64
                && if fetch {
                    r.executable && r.readable
                } else if write {
                    r.writable
                } else {
                    r.readable
                }
        })
}
pub(crate) fn mailbox(c: &Config, address: u32, length: usize) -> Option<usize> {
    c.mailboxes.iter().position(|m| {
        (address as u64) < m.base as u64 + 64 && (m.base as u64) < address as u64 + length as u64
    })
}
fn validate(c: &Config, w: &Workload) -> ModelResult {
    if c.schema != "dir.soc-execution.model"
        || c.version != 1
        || w.schema != "dir.soc-execution.workload"
        || w.version != 1
    {
        return Err("unsupported SoC execution schema/version".into());
    }
    let nodes = c
        .noc
        .topology
        .width
        .checked_mul(c.noc.topology.height)
        .ok_or("SoC topology overflow")?;
    crate::input::noc_flit::parse(&ProfileInput {
        model_config: Some(serde_json::to_value(&c.noc).map_err(|e| ModelError(e.to_string()))?),
        workload: Some(serde_json::json!({"schema":"dir.noc-flit.workload","version":1})),
    })?;
    if c.cycle_ps == 0
        || c.noc.cycle_ps != c.cycle_ps
        || c.memory_router >= nodes
        || c.memory.is_empty()
        || c.memory.len() > 64
        || c.cores.len() > 8
        || !unique(c.memory.iter().map(|r| &r.id))
        || c.memory.iter().any(|r| !crate::input::identifier(&r.id))
        || c.cores.iter().any(|r| !crate::input::identifier(&r.id))
        || !unique(c.cores.iter().map(|r| &r.id))
        || !unique(c.slots.iter().map(|r| &r.id))
        || !c.cache.line_bytes.is_power_of_two()
        || !(4..=256).contains(&c.cache.line_bytes)
        || !c.cache.sets.is_power_of_two()
        || c.cache.sets > 1024
        || c.cache.ways == 0
        || c.cache.ways > 16
        || c.cache.mshrs == 0
        || c.cache.mshrs > 256
        || c.cache.ack_cycles == 0
        || c.fabric.queue_capacity == 0
        || c.fabric.queue_capacity > 4096
        || c.fabric.aging_cycles == 0
        || !c.fabric.data_bytes.is_power_of_two()
        || c.fabric.data_bytes > 16
        || c.dma_buffer_bytes == 0
        || c.dma_buffer_bytes > 4096
        || c.slots.len() > 256
    {
        return Err("invalid SoC topology/cache/finite queue bounds".into());
    }
    if let Some(d) = &c.ddr {
        crate::runtime::ddr4::Service::new(crate::runtime::ddr4::Config {
            mode_id: d.mode_id.clone(),
            source_sha256: d.source_sha256.clone(),
            base: d.base,
            size: d.size,
            queue_capacity: d.queue_capacity,
        })
        .map_err(|e| ModelError(format!("DDR configuration: {e:?}")))?;
        if c.memory
            .iter()
            .filter(|r| r.backend == Backend::DdrStrict)
            .count()
            != 1
            || crate::runtime::ddr4::HALF % c.cycle_ps != 0
        {
            return Err(
                "DDR requires one exact region and a core clock dividing its half cycle".into(),
            );
        }
    }
    let mut total = 0;
    for r in &c.memory {
        total += r.size;
        if r.size == 0
            || r.size > 16_777_216
            || total > 67_108_864
            || r.base as u64 + r.size as u64 > 1u64 << 32
            || r.base as usize % c.cache.line_bytes != 0
            || r.size % c.cache.line_bytes != 0
            || r.initial.len() > r.size
            || r.backend == Backend::DdrStrict
                && (!r.initial.is_empty()
                    || c.ddr
                        .as_ref()
                        .is_none_or(|d| d.base != r.base as u64 || d.size != r.size as u64))
        {
            return Err("invalid SoC memory bounds/backend identity or DDR preload".into());
        }
        if c.memory.iter().any(|o| {
            o.id != r.id
                && (r.base as u64) < o.base as u64 + o.size as u64
                && (o.base as u64) < r.base as u64 + r.size as u64
        }) {
            return Err("overlapping SoC regions".into());
        }
    }
    for core in &c.cores {
        if core.reset_pc % 4 != 0
            || core.instruction_budget == 0
            || core.instruction_budget > 100_000
            || core.cycle_cycles == 0
            || core.execute_cycles == 0
            || core.router >= nodes
            || core.qos > 15
            || core
                .execute_cycles
                .checked_mul(core.cycle_cycles)
                .and_then(|x| x.checked_mul(c.cycle_ps))
                .is_none()
        {
            return Err("invalid RV32I core reset/budget/timing/QoS".into());
        }
    }
    for s in &c.slots {
        if c.slots.iter().any(|other| {
            other.id != s.id
                && (s.base as u64) < other.base as u64 + other.capacity as u64
                && (other.base as u64) < s.base as u64 + s.capacity as u64
        }) {
            return Err("overlapping IPC slot storage".into());
        }
        if s.capacity == 0
            || s.capacity > 1_048_576
            || !mapped(c, s.base, s.capacity, true, false)
            || !mapped(c, s.base, s.capacity, false, false)
        {
            return Err("invalid shared IPC slot range".into());
        }
    }
    if c.mailboxes.len() > 256 || !unique(c.mailboxes.iter().map(|m| &m.id)) {
        return Err("invalid finite mailbox identities".into());
    }
    let mut mapped_slots = BTreeSet::new();
    for m in &c.mailboxes {
        if !crate::input::identifier(&m.id)
            || !crate::input::identifier(&m.producer)
            || !crate::input::identifier(&m.consumer)
            || m.producer == m.consumer
            || m.base % 64 != 0
            || m.base as u64 + 64 > 1u64 << 32
            || m.service_cycles == 0
            || m.service_cycles.checked_mul(c.cycle_ps).is_none()
            || c.fabric.data_bytes < 4
            || !c.slots.iter().any(|s| s.id == m.slot)
            || !mapped_slots.insert(&m.slot)
            || c.memory.iter().any(|r| {
                ((m.base & !1023) as u64) < r.base as u64 + r.size as u64
                    && (r.base as u64) < (m.base & !1023) as u64 + 1024
            })
            || c.mailboxes.iter().any(|o| o.id != m.id && o.base == m.base)
        {
            return Err("invalid mailbox ABI mapping/roles/latency or overlapping aperture".into());
        }
    }
    if !unique(w.requests.iter().map(|r| &r.id))
        || !unique(w.dma.iter().map(|r| &r.id))
        || !unique(w.commands.iter().map(|r| &r.id))
        || w.requests.len() > 10_000
        || w.dma.len() > 10_000
        || w.commands.len() > 10_000
    {
        return Err("invalid duplicate/excessive workload identities".into());
    }
    for r in &w.requests {
        if !crate::input::identifier(&r.source)
            || c.cores.iter().any(|core| core.id == r.source)
            || r.at_ps % c.cycle_ps != 0
            || r.length == 0
            || r.length > 4096
            || r.router >= nodes
            || r.qos > 15
            || r.address as u64 + r.length as u64 > 1u64 << 32
        {
            return Err("invalid shared logical request identity/time/shape".into());
        }
        match r.operation {
            Operation::Read
                if !r.data.is_empty() || !r.byte_enables.is_empty() || r.atomic.is_some() =>
            {
                return Err("read carries write/atomic fields".into());
            }
            Operation::Write
                if r.data.len() != r.length
                    || (!r.byte_enables.is_empty() && r.byte_enables.len() != r.length)
                    || r.atomic.is_some() =>
            {
                return Err("write byte-enable/data shape mismatch".into());
            }
            Operation::Atomic => {
                let a = r.atomic.as_ref().ok_or("atomic missing AXI5 AWATOP")?;
                let supported = (0x10..=0x17).contains(&a.awatop)
                    || (0x20..=0x27).contains(&a.awatop)
                    || matches!(a.awatop, 0x30 | 0x31);
                if !supported
                    || r.transport != Transport::Axi
                    || r.length != 4
                    || r.address % 4 != 0
                    || !r.byte_enables.is_empty()
                    || r.data.len() != if a.awatop == 0x31 { 8 } else { 4 }
                    || a.burst
                        != if a.awatop == 0x31 && r.address % 8 == 4 {
                            AtomicBurst::Wrap
                        } else {
                            AtomicBurst::Incr
                        }
                {
                    return Err(
                        "unsupported AXI5 32-bit atomic AWATOP/alignment/Compare layout".into(),
                    );
                }
            }
            _ => {}
        }
        for cycles in [
            1,
            c.cache.ack_cycles,
            r.r_stall_cycles,
            r.b_stall_cycles,
            c.fabric.axi_address_delay_cycles,
            c.fabric.ahb_wait_cycles,
        ] {
            r.at_ps
                .checked_add(cycles.checked_mul(c.cycle_ps).ok_or("SoC time overflow")?)
                .ok_or("SoC time overflow")?;
        }
    }
    for d in &w.dma {
        if d.id.is_empty()
            || !crate::input::identifier(&d.source)
            || d.at_ps % c.cycle_ps != 0
            || d.length == 0
            || d.length > 1_048_576
            || d.router >= nodes
            || d.qos > 15
            || d.source_address as u64 + d.length as u64 > 1u64 << 32
            || d.destination as u64 + d.length as u64 > 1u64 << 32
            || d.publish && d.lease.is_none()
        {
            return Err("invalid finite coherent DMA descriptor".into());
        }
    }
    for command in &w.commands {
        if let CommandOp::DdrControl { command } = &command.action {
            if c.ddr.is_none() {
                return Err("DDR control without selected DDR target".into());
            }
            serde_json::from_value::<crate::runtime::ddr4::Command>(command.clone())
                .map_err(|e| ModelError(format!("DDR control shape: {e}")))?;
        }
    }
    if w.commands.iter().any(|cmd| cmd.at_ps % c.cycle_ps != 0) {
        return Err("IPC command is not cycle aligned".into());
    }
    Ok(())
}
