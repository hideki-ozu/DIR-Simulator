use super::types::*;
use crate::registry::{ModelError, ModelResult, ProfileInput};
use std::collections::BTreeSet;

pub(super) fn parse(input: &ProfileInput) -> ModelResult<(Config, Workload)> {
    let c: Config = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("AHB pipeline requires model-config")?,
    )
    .map_err(|e| ModelError(format!("AHB model: {e}")))?;
    let w: Workload = serde_json::from_value(
        input
            .workload
            .clone()
            .ok_or("AHB pipeline requires workload")?,
    )
    .map_err(|e| ModelError(format!("AHB workload: {e}")))?;
    validate(&c, &w)?;
    Ok((c, w))
}
pub(super) fn validate_input(input: &ProfileInput) -> ModelResult {
    parse(input).map(|_| ())
}
fn unique<'a>(items: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .all(|id| !id.is_empty() && seen.insert(id))
}
pub(super) fn validate(c: &Config, w: &Workload) -> ModelResult {
    if c.schema != "dir.ahb-pipeline.model"
        || w.schema != "dir.ahb-pipeline.workload"
        || c.version != 1
        || w.version != 1
    {
        return Err("unsupported AHB pipeline schema/version".into());
    }
    if c.cycle_ps == 0
        || !c.data_bytes.is_power_of_two()
        || !(4..=128).contains(&c.data_bytes)
        || c.managers.is_empty()
        || c.managers.len() > 15
        || c.targets.is_empty()
        || !unique(c.managers.iter().map(|m| m.id.as_str()))
        || !unique(c.targets.iter().map(|t| t.id.as_str()))
        || !unique(w.requests.iter().map(|r| r.request_id.as_str()))
        || c.managers.iter().any(|m| !crate::input::identifier(&m.id))
        || c.targets.iter().any(|t| !crate::input::identifier(&t.id))
    {
        return Err("invalid AHB width, period, manager count or identities".into());
    }
    let mut total = 0u64;
    for t in &c.targets {
        let end = t.base.checked_add(t.size).ok_or("AHB target overflow")?;
        total = total
            .checked_add(t.size)
            .ok_or("AHB total memory overflow")?;
        if t.base % 1024 != 0
            || t.size == 0
            || t.size % 1024 != 0
            || t.size > 16_777_216
            || total > 67_108_864
            || end > u32::MAX as u64 + 1
            || t.initial.len() as u64 > t.size
        {
            return Err("AHB target requires aligned 1KiB mapping within 32-bit address space/64MiB storage limit".into());
        }
        if c.targets
            .iter()
            .any(|o| o.id != t.id && t.base < o.base.saturating_add(o.size) && o.base < end)
        {
            return Err("overlapping AHB targets".into());
        }
    }
    for r in &w.requests {
        let fixed = match r.burst {
            Burst::Single => Some(1),
            Burst::Incr => None,
            Burst::Incr4 | Burst::Wrap4 => Some(4),
            Burst::Incr8 | Burst::Wrap8 => Some(8),
            Burst::Incr16 | Burst::Wrap16 => Some(16),
        };
        if !c.managers.iter().any(|m| m.id == r.source)
            || r.at_ps % c.cycle_ps != 0
            || r.size_bytes == 0
            || !r.size_bytes.is_power_of_two()
            || r.size_bytes > c.data_bytes
            || r.address % r.size_bytes as u64 != 0
            || r.beats == 0
            || r.beats > 1024
            || fixed.is_some_and(|b| b != r.beats)
        {
            return Err(
                format!("invalid AHB source/time/alignment/burst: {}", r.request_id).into(),
            );
        }
        r.address
            .checked_add((r.beats * r.size_bytes) as u64)
            .ok_or("AHB burst overflow")?;
        for beat in 0..r.beats {
            let address = beat_address(r, beat);
            let end = address
                .checked_add(r.size_bytes as u64 - 1)
                .ok_or("AHB burst overflow")?;
            if end > u32::MAX as u64
                || address / 1024 != r.address / 1024
                || end / 1024 != r.address / 1024
            {
                return Err("AHB burst crosses 1KiB boundary/32-bit address limit".into());
            }
        }
        if r.operation == Operation::Write
            && (r.write_data.len() != r.beats
                || r.write_data.iter().any(|b| b.len() != c.data_bytes))
            || r.operation == Operation::Read && !r.write_data.is_empty()
        {
            return Err("AHB write beat count/bus lane width mismatch".into());
        }
    }
    let mut rules = BTreeSet::new();
    for rule in &c.responses {
        let r = w
            .requests
            .iter()
            .find(|r| r.request_id == rule.request_id)
            .ok_or("AHB response rule names unknown request")?;
        if rule.beat >= r.beats
            || !rules.insert((&rule.request_id, rule.beat, rule.attempt))
            || (rule.response == Response::Split
                && rule.split_release_cycles.is_none_or(|cycles| cycles == 0))
            || (rule.response != Response::Split && rule.split_release_cycles.is_some())
        {
            return Err("invalid AHB response rule/HSPLIT release".into());
        }
        if rule.response == Response::Retry {
            let address = beat_address(r, rule.beat);
            if let Some(target) = c
                .targets
                .iter()
                .find(|t| address >= t.base && address - t.base < t.size)
            {
                if w.requests.iter().any(|other| {
                    other.source != r.source
                        && (0..other.beats).any(|beat| {
                            let a = beat_address(other, beat);
                            a >= target.base && a - target.base < target.size
                        })
                }) {
                    return Err(
                        "AHB RETRY target requires single-manager access under IHI0011A 3.12.3"
                            .into(),
                    );
                }
            }
        }
    }
    Ok(())
}
