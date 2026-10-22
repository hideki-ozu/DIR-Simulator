//! Strict preparation for the independent concurrent transaction fabric profile.
use crate::registry::{ModelError, ModelResult, ProfileInput};
use crate::types::transaction_fabric::*;
use std::collections::BTreeSet;

pub(crate) fn parse(input: &ProfileInput) -> ModelResult<(FabricConfig, FabricWorkload)> {
    let config: FabricConfig = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("fabric requires model-config")?,
    )
    .map_err(|e| ModelError(format!("fabric model-config: {e}")))?;
    let workload: FabricWorkload =
        serde_json::from_value(input.workload.clone().ok_or("fabric requires workload")?)
            .map_err(|e| ModelError(format!("fabric workload: {e}")))?;
    validate(&config, &workload)?;
    Ok((config, workload))
}
pub(crate) fn validate_input(input: &ProfileInput) -> ModelResult {
    parse(input).map(|_| ())
}
fn unique<'a>(values: impl Iterator<Item = &'a String>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|s| !s.is_empty() && seen.insert(s))
}
pub(crate) fn validate(c: &FabricConfig, w: &FabricWorkload) -> ModelResult {
    if c.schema != "dir.transaction-fabric.model"
        || c.version != 1
        || w.schema != "dir.transaction-fabric.workload"
        || w.version != 1
    {
        return Err("unsupported transaction fabric schema/version".into());
    }
    if c.cycle_ps == 0
        || !c.data_bytes.is_power_of_two()
        || c.data_bytes > 128
        || c.write_buffer_beats == 0
        || c.write_buffer_beats > 256
        || c.managers.is_empty()
        || c.targets.is_empty()
        || !unique(c.managers.iter().map(|m| &m.id))
        || !unique(c.targets.iter().map(|t| &t.id))
        || !unique(c.interconnects.iter())
        || c.managers.iter().any(|m| !crate::input::identifier(&m.id))
        || c.targets.iter().any(|t| !crate::input::identifier(&t.id))
        || c.interconnects
            .iter()
            .any(|id| !crate::input::identifier(id))
        || !unique(w.requests.iter().map(|r| &r.request_id))
    {
        return Err("invalid fabric width, period, empty or duplicate identities".into());
    }
    let mut total = 0u64;
    for t in &c.targets {
        let end = t
            .base
            .checked_add(t.size)
            .ok_or("target address overflow")?;
        if t.size == 0 || t.size > 16_777_216 || t.initial.len() as u64 > t.size {
            return Err(
                "invalid memory target size/initial bytes (maximum 16 MiB per region)".into(),
            );
        }
        total = total
            .checked_add(t.size)
            .ok_or("memory allocation overflow")?;
        if total > 67_108_864 {
            return Err("fabric memory exceeds 64 MiB".into());
        }
        if c.targets
            .iter()
            .any(|o| o.id != t.id && t.base < o.base.saturating_add(o.size) && o.base < end)
        {
            return Err("overlapping memory targets".into());
        }
    }
    for m in &c.managers {
        if m.routes.len() != c.targets.len() {
            return Err("manager must route every target".into());
        }
        for (target, path) in &m.routes {
            if !c.targets.iter().any(|t| &t.id == target)
                || path.is_empty()
                || !unique(path.iter())
                || path.iter().any(|p| !c.interconnects.contains(p))
            {
                return Err("invalid, cyclic or unknown interconnect route".into());
            }
        }
    }
    for r in &w.requests {
        if !c.managers.iter().any(|m| m.id == r.source)
            || r.at_ps % c.cycle_ps != 0
            || r.size_bytes == 0
            || !r.size_bytes.is_power_of_two()
            || r.size_bytes > c.data_bytes
            || r.beats == 0
            || r.beats > 256
            || (r.burst == Burst::Fixed && r.beats > 16)
            || (r.burst == Burst::Wrap
                && (!matches!(r.beats, 2 | 4 | 8 | 16) || r.address % r.size_bytes as u64 != 0))
        {
            return Err(format!("invalid AXI request shape/source/time: {}", r.request_id).into());
        }
        r.address
            .checked_add((r.beats as u64 + 1) * r.size_bytes as u64)
            .ok_or("burst address overflow")?;
        let target = c
            .targets
            .iter()
            .position(|t| r.address >= t.base && r.address < t.base + t.size);
        for beat in 0..r.beats {
            let (_, lanes) = beat_lanes(r, beat, c.data_bytes);
            for (_, address) in &lanes {
                if *address / 4096 != r.address / 4096 {
                    return Err("AXI burst crosses 4 KiB boundary".into());
                }
                let found = c
                    .targets
                    .iter()
                    .position(|t| *address >= t.base && *address < t.base + t.size);
                if target != found {
                    return Err("AXI burst crosses address-map target boundary".into());
                }
            }
            if r.operation == Operation::Write {
                let b = r.write_data.get(beat).ok_or("missing write beat")?;
                if b.data.len() != c.data_bytes
                    || b.strobe.len() != c.data_bytes
                    || b.strobe
                        .iter()
                        .enumerate()
                        .any(|(lane, active)| *active && !lanes.iter().any(|(l, _)| *l == lane))
                {
                    return Err("write data width or illegal WSTRB lane".into());
                }
            }
        }
        if (r.operation == Operation::Read && !r.write_data.is_empty())
            || (r.operation == Operation::Write && r.write_data.len() != r.beats)
        {
            return Err("write beat count does not match operation/AxLEN".into());
        }
        let bytes = r.beats * r.size_bytes;
        if r.exclusive
            && (r.transport != Transport::Axi
                || r.burst != Burst::Incr
                || r.beats > 16
                || bytes > 128
                || !bytes.is_power_of_two()
                || r.address % bytes as u64 != 0)
        {
            return Err("exclusive access requires aligned power-of-two INCR range of at most 128 bytes/16 beats".into());
        }
        for delay in [
            r.address_delay_cycles,
            r.data_delay_cycles,
            r.response_stall_cycles,
        ] {
            r.at_ps
                .checked_add(delay.checked_mul(c.cycle_ps).ok_or("time overflow")?)
                .ok_or("time overflow")?;
        }
    }
    Ok(())
}
