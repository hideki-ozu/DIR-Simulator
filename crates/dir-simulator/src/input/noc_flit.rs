//! Strict, bounded and canonical noc.flit.v1 preparation.
use crate::registry::{ModelError, ModelResult, ProfileInput};
use crate::types::noc_flit::*;
use std::collections::BTreeSet;
pub(crate) fn parse(input: &ProfileInput) -> ModelResult<(FlitConfig, FlitWorkload)> {
    let c: FlitConfig = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("NoC requires model-config")?,
    )
    .map_err(|e| ModelError(format!("NoC model-config: {e}")))?;
    let mut w: FlitWorkload =
        serde_json::from_value(input.workload.clone().ok_or("NoC requires workload")?)
            .map_err(|e| ModelError(format!("NoC workload: {e}")))?;
    validate(&c, &w)?;
    w.packets
        .sort_by(|a, b| (a.at_ps, &a.id).cmp(&(b.at_ps, &b.id)));
    w.faults
        .sort_by(|a, b| (a.at_ps, &a.id).cmp(&(b.at_ps, &b.id)));
    Ok((c, w))
}
pub(crate) fn validate_input(input: &ProfileInput) -> ModelResult {
    parse(input).map(|_| ())
}
fn validate(c: &FlitConfig, w: &FlitWorkload) -> ModelResult {
    let n = c
        .topology
        .width
        .checked_mul(c.topology.height)
        .ok_or("NoC topology overflow")?;
    if c.schema != "dir.noc-flit.model"
        || c.version != 1
        || w.schema != "dir.noc-flit.workload"
        || w.version != 1
    {
        return Err("unsupported NoC schema/version".into());
    }
    if n == 0
        || n > 256
        || (c.topology.kind == TopologyKind::Torus
            && (c.topology.width < 2 || c.topology.height < 2))
        || c.flit_bytes == 0
        || c.flit_bytes > 4096
        || c.buffer_flits == 0
        || c.buffer_flits > 256
        || c.adaptive_vcs == 0
        || c.adaptive_vcs > 16
        || c.cycle_ps == 0
        || c.link_latency_cycles == 0
        || c.credit_latency_cycles == 0
        || c.escape_after_stalls == 0
        || c.ejection_capacity_flits == 0
        || c.ejection_capacity_flits > 256
        || c.ejection_service_cycles == 0
        || c.pending_packets > 4096
        || n * 5 * (c.adaptive_vcs + 1) * c.buffer_flits > 1_048_576
        || w.packets.len() > 100_000
        || w.faults.len() > 100_000
    {
        return Err("invalid or excessive NoC topology/resource bounds".into());
    }
    for cycles in [
        c.link_latency_cycles,
        c.credit_latency_cycles,
        c.ejection_service_cycles,
    ] {
        cycles
            .checked_mul(c.cycle_ps)
            .ok_or("NoC latency overflow")?;
    }
    let links = crate::runtime::noc_flit::topology::Topology::new(&c.topology)
        .links
        .len();
    let mut ids = BTreeSet::new();
    let mut bytes = 0usize;
    for p in &w.packets {
        bytes = bytes
            .checked_add(p.payload.len())
            .ok_or("NoC workload size overflow")?;
        if p.id.is_empty()
            || !ids.insert(&p.id)
            || p.at_ps % c.cycle_ps != 0
            || p.source >= n
            || p.destination >= n
            || bytes > 67_108_864
        {
            return Err("invalid NoC packet identity/time/endpoints/payload limit".into());
        }
        for cycles in [
            1,
            c.link_latency_cycles,
            c.credit_latency_cycles,
            c.ejection_service_cycles,
        ] {
            p.at_ps
                .checked_add(
                    cycles
                        .checked_mul(c.cycle_ps)
                        .ok_or("NoC latency overflow")?,
                )
                .ok_or("NoC packet time overflow")?;
        }
    }
    ids.clear();
    let mut updates = BTreeSet::new();
    for f in &w.faults {
        if f.id.is_empty()
            || !ids.insert(&f.id)
            || f.at_ps % c.cycle_ps != 0
            || f.link_id >= links
            || !updates.insert((f.at_ps, f.link_id))
        {
            return Err("invalid/conflicting NoC fault identity/time/link".into());
        }
    }
    Ok(())
}
