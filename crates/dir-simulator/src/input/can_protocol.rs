//! Strict input validation shared by preparation and checkpoint restore.
use crate::registry::{ModelError, ModelResult, ProfileInput};
use crate::runtime::can_protocol::{codec, types::*};
use std::collections::BTreeSet;
pub fn parse(input: &ProfileInput) -> ModelResult<(Config, Workload)> {
    let mut config: Config = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("missing CAN protocol model-config")?,
    )
    .map_err(|e| ModelError(e.to_string()))?;
    let mut workload: Workload = serde_json::from_value(
        input
            .workload
            .clone()
            .ok_or("missing CAN protocol workload")?,
    )
    .map_err(|e| ModelError(e.to_string()))?;
    if config.schema_version != 1
        || workload.schema_version != 1
        || !(1..=1_000_000).contains(&config.bitrate)
        || config.controllers.len() < 2
    {
        return Err("invalid CAN protocol schema, bitrate or controller count".into());
    }
    let mut ids = BTreeSet::new();
    for c in &config.controllers {
        if !crate::input::identifier(&c.id) || !ids.insert(&c.id) || c.tec > 256 || c.rec > 255 {
            return Err("invalid CAN protocol controller/counters".into());
        }
        if c.application_rx
            .as_ref()
            .is_some_and(|p| p.capacity > 100_000)
        {
            return Err("application capacity exceeds v1 bound".into());
        }
        let mut responder_ids = BTreeSet::new();
        for f in &c.responders {
            if !responder_ids.insert((f.extended, f.id)) {
                return Err("duplicate remote responder".into());
            }
            codec::encode(f)?;
            if f.remote {
                return Err("remote responder must send a data frame".into());
            }
        }
    }
    let mut generators = BTreeSet::new();
    for g in &workload.generators {
        if !crate::input::identifier(&g.id)
            || !generators.insert(&g.id)
            || !ids.contains(&g.controller)
            || config
                .controllers
                .iter()
                .any(|c| c.id == g.controller && c.listen_only)
        {
            return Err("invalid generator id/controller".into());
        }
        codec::encode(&g.frame)?;
        match &g.schedule {
            Schedule::Explicit { times_ps } => {
                if times_ps.len() > 100_000 || times_ps.windows(2).any(|w| w[0] > w[1]) {
                    return Err("explicit times must be ordered and bounded".into());
                }
            }
            Schedule::Uniform {
                count,
                min_ps,
                max_ps,
                ..
            } => {
                if *count > 100_000 || *min_ps == 0 || min_ps > max_ps || *max_ps == u64::MAX {
                    return Err("invalid uniform schedule".into());
                }
            }
            Schedule::Bernoulli {
                count,
                tick_ps,
                numerator,
                denominator,
                ..
            }
            | Schedule::Geometric {
                count,
                tick_ps,
                numerator,
                denominator,
                ..
            } => {
                if *count > 100_000 || *tick_ps == 0 || *numerator == 0 || numerator > denominator {
                    return Err("invalid discrete probability schedule".into());
                }
            }
        }
    }
    for f in &config.faults {
        if !matches!(f.kind.as_str(), "bit" | "ack" | "stuff" | "form" | "crc")
            || f.attempt == 0
            || (f.kind != "ack" && f.bit.is_none())
            || f.receiver.as_ref().is_some_and(|id| !ids.contains(id))
        {
            return Err("invalid CAN fault selector".into());
        }
    }
    let mut overload_ids = BTreeSet::new();
    for o in &config.overloads {
        if !(1..=2).contains(&o.count) || !overload_ids.insert(&o.request_id) {
            return Err("overload count must be one or two, with unique request selector".into());
        }
    }
    let mut cancellation_ids = BTreeSet::new();
    if config.cancellations.len() > 100_000 {
        return Err("CAN cancellation count exceeds v1 bound".into());
    }
    for cancel in &config.cancellations {
        let (generator, ordinal) = cancel
            .request_id
            .rsplit_once(':')
            .ok_or("cancellation requires generator:ordinal request ID")?;
        let ordinal: u64 = ordinal
            .parse()
            .map_err(|_| ModelError("invalid cancellation ordinal".into()))?;
        let g = workload
            .generators
            .iter()
            .find(|g| g.id == generator)
            .ok_or("unknown cancellation generator")?;
        let bound = match &g.schedule {
            Schedule::Explicit { times_ps } => times_ps.len() as u64,
            Schedule::Uniform { count, .. }
            | Schedule::Bernoulli { count, .. }
            | Schedule::Geometric { count, .. } => *count,
        };
        if cancel.request_id != format!("{generator}:{ordinal}")
            || ordinal >= bound
            || !cancellation_ids.insert(&cancel.request_id)
        {
            return Err("invalid, duplicate or out-of-range cancellation request ID".into());
        }
    }
    config
        .cancellations
        .sort_by(|a, b| (a.at_ps, &a.request_id).cmp(&(b.at_ps, &b.request_id)));
    config.faults.sort_by(|a, b| {
        (&a.request_id, a.attempt, a.bit, &a.receiver, &a.kind).cmp(&(
            &b.request_id,
            b.attempt,
            b.bit,
            &b.receiver,
            &b.kind,
        ))
    });
    workload.generators.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((config, workload))
}
pub fn validate(input: &ProfileInput) -> ModelResult {
    parse(input).map(|_| ())
}

pub fn parse_compound(input: &ProfileInput) -> ModelResult<(CompoundConfig, CompoundWorkload)> {
    let mut config: CompoundConfig = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("missing compound CAN model-config")?,
    )
    .map_err(|e| ModelError(e.to_string()))?;
    let mut workload: CompoundWorkload = serde_json::from_value(
        input
            .workload
            .clone()
            .ok_or("missing compound CAN workload")?,
    )
    .map_err(|e| ModelError(e.to_string()))?;
    if config.schema_version != 1
        || workload.schema_version != 1
        || config.execution_contract != "ideal-digital-can.v1"
        || config.standard_edition != "ISO11898-1:2015-wire/Bosch2.0B-cc"
        || config.buses.is_empty()
        || config.buses.len() > 64
        || workload.generators.len() > 100_000
        || config.gateways.len() > 64
        || config.route_updates.len() > 100_000
        || config.cancellations.len() > 100_000
        || config.faults.len() > 100_000
        || config.max_gateway_hops > 1024
    {
        return Err("invalid compound CAN schema/execution_contract/bus/hop bound".into());
    }
    let mut buses = BTreeSet::new();
    for bus in &config.buses {
        if !crate::input::identifier(&bus.id)
            || !buses.insert(bus.id.clone())
            || !(1..=1_000_000).contains(&bus.nominal_bps)
            || !(1..=8_000_000).contains(&bus.data_bps)
            || bus.data_bps < bus.nominal_bps
            || bus.controllers.is_empty()
            || bus.controllers.len() > 64
        {
            return Err(format!("invalid compound CAN bus/rates/controllers: {}", bus.id).into());
        }
        if !bus.pxhd
            || bus
                .hardware_model
                .as_ref()
                .is_some_and(|model| model != "ideal_digital")
        {
            return Err(format!("{}: hardware emulation/PXHD=0 protocol-exception reintegration unsupported; ideal-digital-can.v1 supports PXHD=1-like res form errors",bus.id).into());
        }
        for (name, sample, bps) in [
            ("nominal_sample", &bus.nominal_sample, bus.nominal_bps),
            ("data_sample", &bus.data_sample, bus.data_bps),
        ] {
            if sample.num == 0
                || sample.num >= sample.den
                || sample.den > 1_000_000
                || 1_000_000_000_000u128 * (sample.num.min(sample.den - sample.num) as u128)
                    < (bps as u128 * sample.den as u128)
            {
                return Err(format!("{}.{name}: require 0<num<den and distinct sample/boundary picosecond precision",bus.id).into());
            }
        }
        let mut controllers = BTreeSet::new();
        for controller in &bus.controllers {
            if !crate::input::identifier(&controller.id)
                || !controllers.insert(&controller.id)
                || controller.tec > 256
                || controller.rec > 255
                || controller.pending_capacity > 100_000
            {
                return Err(format!(
                    "invalid compound CAN controller: {}.{}",
                    bus.id, controller.id
                )
                .into());
            }
            if !matches!(
                controller.capability.as_str(),
                "cc_only" | "fd_capable" | "monitor_only"
            ) {
                return Err(format!("{}.{}: unsupported capability {}; choose cc_only, fd_capable or monitor_only; fd_tolerant has no defined reintegration contract",bus.id,controller.id,controller.capability).into());
            }
        }
    }
    let destination = |bus: &str, node: &str| {
        config
            .buses
            .iter()
            .find(|b| b.id == bus)
            .and_then(|b| b.controllers.iter().find(|c| c.id == node))
    };
    let mut generators = BTreeSet::new();
    for generator in &workload.generators {
        let controller = destination(&generator.bus, &generator.controller)
            .ok_or("unknown compound generator bus/controller")?;
        if !crate::input::identifier(&generator.id)
            || !generators.insert(&generator.id)
            || controller.capability == "monitor_only"
            || matches!(generator.frame, ProtocolFrame::IsoFd { .. })
                && controller.capability != "fd_capable"
        {
            return Err("invalid compound generator/capability".into());
        }
        validate_protocol_frame(&generator.frame)?;
        validate_schedule(&generator.schedule)?;
    }
    let mut gateways = BTreeSet::new();
    let mut ports = BTreeSet::new();
    for gateway in &config.gateways {
        if !crate::input::identifier(&gateway.id)
            || !gateways.insert(gateway.id.clone())
            || gateway.ingress_capacity > 100_000
            || gateway.ports.is_empty()
        {
            return Err("invalid protocol Gateway".into());
        }
        for port in &gateway.ports {
            if destination(&port.bus, &port.controller).is_none()
                || !ports.insert((port.bus.clone(), port.controller.clone()))
            {
                return Err("unknown or multiply owned Gateway controller".into());
            }
        }
        validate_routes(&gateway.rules, &gateway.ports, &config)?;
    }
    config
        .route_updates
        .sort_by(|a, b| (a.at_ps, &a.gateway, a.version).cmp(&(b.at_ps, &b.gateway, b.version)));
    let mut versions: std::collections::BTreeMap<_, _> = config
        .gateways
        .iter()
        .map(|g| (g.id.clone(), g.version))
        .collect();
    for update in &config.route_updates {
        let gateway = config
            .gateways
            .iter()
            .find(|g| g.id == update.gateway)
            .ok_or("unknown route update Gateway")?;
        let version = versions.get_mut(&update.gateway).unwrap();
        if update.version <= *version {
            return Err("route versions must increase monotonically".into());
        }
        *version = update.version;
        validate_routes(&update.rules, &gateway.ports, &config)?;
    }
    for fault in &config.faults {
        if fault.attempt == 0
            || fault.bit.is_some_and(|bit| bit > 1_000_000)
            || fault.kind == "flip" && fault.bit.is_none()
            || !matches!(fault.kind.as_str(), "flip" | "ack_suppress")
            || fault.receiver.as_ref().is_some_and(|id| {
                !config
                    .buses
                    .iter()
                    .any(|b| b.controllers.iter().any(|c| c.id == *id))
            })
        {
            return Err("invalid compound fault selector".into());
        }
    }
    let valid_selector = |id: &str| -> bool {
        let Some((name, ordinal)) = id.rsplit_once(':') else {
            return false;
        };
        if let Some(g) = workload.generators.iter().find(|g| g.id == name) {
            let Ok(ordinal) = ordinal.parse::<u64>() else {
                return false;
            };
            let count = match &g.schedule {
                Schedule::Explicit { times_ps } => times_ps.len() as u64,
                Schedule::Uniform { count, .. }
                | Schedule::Bernoulli { count, .. }
                | Schedule::Geometric { count, .. } => *count,
            };
            return id == format!("{name}:{ordinal}") && ordinal < count;
        }
        // Branch IDs are produced only after actual reception; a finite explicit selector may precede that production.
        let parts: Vec<_> = id.split(':').collect();
        parts.len() == 4
            && parts[0] == "branch"
            && config.gateways.iter().any(|g| g.id == parts[1])
            && parts[2]
                .parse::<u64>()
                .is_ok_and(|serial| serial > 0 && serial.to_string() == parts[2])
            && crate::input::identifier(parts[3])
    };
    let mut cancellations = BTreeSet::new();
    for cancel in &config.cancellations {
        if !valid_selector(&cancel.request_id) || !cancellations.insert(&cancel.request_id) {
            return Err("invalid/noncanonical/duplicate compound cancellation".into());
        }
    }
    let mut faults = BTreeSet::new();
    for fault in &config.faults {
        if !valid_selector(&fault.request_id)
            || !faults.insert((
                &fault.request_id,
                fault.attempt,
                &fault.kind,
                fault.bit,
                &fault.receiver,
            ))
        {
            return Err("invalid/noncanonical/duplicate compound fault request selector".into());
        }
    }
    config
        .cancellations
        .sort_by(|a, b| (a.at_ps, &a.request_id).cmp(&(b.at_ps, &b.request_id)));
    workload.generators.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((config, workload))
}
pub(super) fn validate_protocol_frame(frame: &ProtocolFrame) -> ModelResult {
    if let Some(frame) = frame.cc() {
        codec::encode(&frame)?;
    } else if let ProtocolFrame::IsoFd {
        id,
        extended,
        brs,
        data,
    } = frame
    {
        crate::runtime::canfd::codec::encode(&crate::runtime::canfd::codec::Frame {
            id: *id,
            extended: *extended,
            brs: *brs,
            esi: false,
            data: data.clone(),
        })
        .map_err(|e| ModelError(e.to_string()))?;
    }
    Ok(())
}
fn validate_schedule(schedule: &Schedule) -> ModelResult {
    match schedule {
        Schedule::Explicit { times_ps }
            if times_ps.len() <= 100_000 && times_ps.windows(2).all(|w| w[0] <= w[1]) =>
        {
            Ok(())
        }
        Schedule::Uniform {
            count,
            min_ps,
            max_ps,
            ..
        } if *count <= 100_000 && *min_ps > 0 && min_ps <= max_ps && *max_ps < u64::MAX => Ok(()),
        Schedule::Bernoulli {
            count,
            tick_ps,
            numerator,
            denominator,
            ..
        }
        | Schedule::Geometric {
            count,
            tick_ps,
            numerator,
            denominator,
            ..
        } if *count <= 100_000 && *tick_ps > 0 && *numerator > 0 && numerator <= denominator => {
            Ok(())
        }
        _ => Err("invalid compound generator schedule".into()),
    }
}
fn validate_routes(
    rules: &[RouteRule],
    ports: &[GatewayPort],
    config: &CompoundConfig,
) -> ModelResult {
    if rules.len() > 100_000 {
        return Err("Gateway rule count exceeds finite bound".into());
    }
    let mut ids = BTreeSet::new();
    for rule in rules {
        let bus = config
            .buses
            .iter()
            .find(|b| b.id == rule.destination_bus)
            .ok_or("unknown route destination bus")?;
        let controller = bus
            .controllers
            .iter()
            .find(|c| c.id == rule.controller)
            .ok_or("unknown route destination controller")?;
        if !crate::input::identifier(&rule.id)
            || !ids.insert(&rule.id)
            || !ports.iter().any(|p| p.bus == rule.source_bus)
            || !ports
                .iter()
                .any(|p| p.bus == rule.destination_bus && p.controller == rule.controller)
            || !matches!(rule.protocol.as_str(), "cc" | "iso_fd")
            || controller.capability == "monitor_only"
            || rule.protocol == "iso_fd" && controller.capability != "fd_capable"
            || rule.id_value & !rule.id_mask != 0
            || rule.rewrite_id.is_some_and(|id| {
                id > if rule.rewrite_extended == Some(false) {
                    0x7ff
                } else {
                    0x1fffffff
                }
            })
        {
            return Err("invalid Gateway route/rewrite/capability".into());
        }
    }
    Ok(())
}
pub fn validate_compound(input: &ProfileInput) -> ModelResult {
    parse_compound(input).map(|_| ())
}
