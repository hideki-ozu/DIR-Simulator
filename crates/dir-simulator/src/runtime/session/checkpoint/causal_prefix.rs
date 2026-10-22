//! Bind private derived state to an exact callback prefix of frozen source input.
//!
//! This deliberately uses execution, not restore, and owns no RunSession/output
//! publisher. Pause, export and checkpoint operations cannot alter this prefix.
use super::*;

fn selected(prepared: &PreparedSimulation) -> bool {
    named_phy_codec(prepared)
        || gptp_codec(prepared)
        || prepared
            .registered
            .as_ref()
            .and_then(|r| r.network.as_ref())
            .is_some_and(|n| {
                (n.wire.is_some()
                    && n.extensions
                        .as_ref()
                        .is_some_and(|e| e.mac_merge.is_some() || e.preemption.is_some()))
                    || n.clocks.is_some()
                    || n.qcc.is_some()
                    || n.mac_control.is_some()
                    || n.ethernet.media.is_some()
                    || n.has_phy_mac_service()
                    || n.extensions
                        .as_ref()
                        .and_then(|e| e.preemption.as_ref())
                        .is_some_and(|p| p.participating.iter().any(|active| !active))
            })
}

fn fes(engine: &Engine) -> Vec<Value> {
    // Canonical checkpoint representation drops cancelled delivery tombstones.
    // Preserve the complete ordering key, including same-time delta/sequence.
    engine.checkpoint_work().iter().filter_map(|(key, work)| {
        if matches!(work, Work::Delivery { token: Some(t), .. } if !engine.live.contains(t)) {
            return None;
        }
        let work = match work {
            Work::Delivery { envelope, token } => json!({"tag":"Delivery","envelope":envelope_value(envelope),"token":token.as_ref().map(token_value)}),
            Work::Arbitration { owner, resource } => json!({"tag":"Arbitration","owner":owner,"resource":resource}),
        };
        Some(json!({"time_ps":key.0.to_string(),"delta":key.1.to_string(),"phase":key.2,"sequence":key.3.to_string(),"work":work}))
    }).collect()
}

pub(super) fn validate(
    saved: &Engine,
    state: &State,
    limits: &CheckpointLimits,
) -> Result<Option<Engine>, ControlError> {
    if !selected(&saved.prepared) {
        return Ok(None);
    }
    let commits = saved.snapshot.common.committed_events;
    // Check before executing anything. A serialized ordinal is only a stop
    // request; the resulting state and last committed key remain derived.
    if commits > saved.prepared.common.max_events {
        return Err(invalid(
            "network causal prefix replay callback limit exceeded",
        ));
    }
    let mut replay = Engine::new(saved.prepared.clone())
        .map_err(|e| invalid(format!("network causal prefix preparation: {}", e.message)))?;
    if state.engine_phase == "initialized" {
        replay
            .initialize()
            .map_err(|e| invalid(format!("network causal prefix initialization: {}", e.0)))?;
    }
    while replay.snapshot.common.committed_events < commits {
        let before = replay.snapshot.common.committed_events;
        if !replay.step()
            || replay.snapshot.common.partial
            || replay.snapshot.common.committed_events != before + 1
        {
            return Err(invalid(
                "network causal prefix is not executable from frozen input",
            ));
        }
        if replay.checkpoint_work().len() as u64 > u64::from(limits.max_fes_entries) {
            return Err(invalid("network causal prefix replay FES limit exceeded"));
        }
    }
    let backend = replay.checkpoint_network();
    if backend.as_ref().is_some_and(|v| {
        serde_json::to_vec(v).map_or(true, |b| b.len() as u64 > limits.max_blob_bytes)
    }) {
        return Err(invalid(
            "network causal prefix replay backend limit exceeded",
        ));
    }
    if replay.last_committed_key != saved.last_committed_key
        || replay.snapshot.common.last_event_time_ps != saved.snapshot.common.last_event_time_ps
        || replay.event_id != saved.event_id
        || replay.sequence != saved.sequence
        || replay.live != saved.live
        || replay.checkpoint_dirty() != saved.checkpoint_dirty()
        || fes(&replay) != state.fes
        || backend != state.network_backend
    {
        return Err(invalid(
            "network causal state differs from exact frozen-input committed prefix",
        ));
    }
    // Views can have been refreshed by owner operations; refresh once at the
    // committed cut before comparing their projection. Session metadata and
    // temporary point-spool layout are deliberately outside physical state.
    replay
        .refresh_view(saved.snapshot.common.last_event_time_ps.unwrap_or(0))
        .map_err(|e| invalid(e.message))?;
    let registered = replay.snapshot.registered.as_ref().unwrap();
    let points = replay
        .snapshot
        .common
        .iter_points()
        .map_err(ControlError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(ControlError::from)?;
    if json!(registered.metrics_v2) != json!(state.metric_accumulators)
        || json!(registered.model_records) != state.journal["model_records"]
        || json!(registered.deliveries) != state.journal["deliveries"]
        || json!(points) != state.journal["points"]
        || json!(replay.snapshot.common.diagnostics) != state.journal["diagnostics"]
    {
        return Err(invalid(
            "network causal journal differs from exact frozen-input committed prefix",
        ));
    }
    Ok(Some(replay))
}
