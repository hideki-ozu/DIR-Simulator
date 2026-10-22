//! Connection between the bounded Qcc service and the owned network FES.
use super::*;
use crate::types::network::configuration::ConfigurationPlan;
use std::collections::BTreeSet;
mod raw;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceBinding {
    pub generator: String,
    pub stream: usize,
    pub version: u64,
    pub interval_origin_ps: u64,
    pub ready_ps: u64,
    pub payload_bytes: usize,
    pub latency_bound_ps: u64,
}

impl NetworkState {
    fn qcc_policy_changed(&self, before: &PolicySnapshot, after: &PolicySnapshot) -> bool {
        self.prepared.qcc.as_ref().is_some_and(|p| {
            p.ports
                .iter()
                .filter(|port| p.depends_on_direction(port.direction))
                .any(|managed| {
                    let port = &managed.port;
                    before.link_up.get(port) != after.link_up.get(port)
                        || before.roles.get(port) != after.roles.get(port)
                        || before.effective_vlans.get(port) != after.effective_vlans.get(port)
                })
        })
    }

    fn qcc_available_in(&self, owner: &DynamicState, snapshot: &PolicySnapshot) -> Vec<bool> {
        let p = self.prepared.qcc.as_ref().unwrap();
        p.ports
            .iter()
            .map(|port| {
                p.streams
                    .iter()
                    .filter(|stream| {
                        stream
                            .paths
                            .iter()
                            .any(|path| path.contains(&port.direction))
                    })
                    .all(|stream| {
                        self.prepared
                            .ethernet
                            .generators
                            .iter()
                            .find(|generator| generator.id == stream.requirement.generator)
                            .and_then(|generator| generator.source_vlan_id)
                            .is_some_and(|vid| {
                                let service_vid = stream
                                    .demands
                                    .iter()
                                    .find(|d| d.direction == port.direction)
                                    .map_or(vid, |d| d.vid);
                                owner
                                    .eligible_egress(&port.port, service_vid, snapshot)
                                    .is_ok()
                            })
                    })
            })
            .collect()
    }

    pub(super) fn validate_qcc_topology_history(&self, horizon: u64) -> Result<()> {
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(());
        };
        let qcc = self
            .qcc
            .as_ref()
            .ok_or_else(|| error("Qcc checkpoint state missing"))?;
        let dynamic = self
            .prepared
            .dynamic
            .as_ref()
            .ok_or_else(|| error("Qcc dynamic input missing"))?;
        let mut owner = DynamicState::new(dynamic, &self.prepared.ethernet)?;
        let mut expected = Vec::new();
        while let Some(at) = owner.next_deadline().filter(|at| *at <= horizon) {
            let before = owner.snapshot_policy();
            let delta = owner.plan_controls(at)?;
            let after = owner.snapshot_policy_after(&delta);
            if self.qcc_policy_changed(&before, &after) {
                expected.push((at, self.qcc_available_in(&owner, &after)));
            }
            owner.apply(delta);
        }
        if expected != qcc.topology_history() {
            return Err(error(
                "Qcc topology history differs from physical control replay",
            ));
        }
        if let Some(actual) = &self.dynamic {
            let before = owner.snapshot_policy();
            let after = actual.snapshot_policy();
            if p.ports
                .iter()
                .filter(|port| p.depends_on_direction(port.direction))
                .any(|port| {
                    let port = &port.port;
                    before.link_up.get(port) != after.link_up.get(port)
                        || before.roles.get(port) != after.roles.get(port)
                        || before.effective_vlans.get(port) != after.effective_vlans.get(port)
                })
            {
                return Err(error(
                    "Qcc checkpoint physical policy differs from control replay",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn validate_qcc_sources(&self, horizon: u64) -> Result<()> {
        let p = self
            .prepared
            .qcc
            .as_ref()
            .ok_or_else(|| error("missing Qcc prepared input"))?;
        let state = self
            .qcc
            .as_ref()
            .ok_or_else(|| error("missing Qcc state"))?;
        let installations = state
            .installation_history(p)
            .map_err(|e| error(format!("Qcc source installation history: {e:?}")))?;
        let mut actual = Vec::new();
        for (&index, binding) in &self.qcc_sources {
            let frame = self
                .frames
                .get(index)
                .ok_or_else(|| error("Qcc source frame missing"))?;
            let generator_index = frame
                .generator
                .ok_or_else(|| error("Qcc bound frame is not generated"))?;
            let generator = &self.prepared.ethernet.generators[generator_index];
            let ordinal_text = frame
                .row
                .frame_id
                .strip_prefix(&format!("{}:", generator.id))
                .ok_or_else(|| error("Qcc source frame ID differs from generator"))?;
            let ordinal = ordinal_text
                .parse::<u64>()
                .map_err(|_| error("Qcc source ordinal is invalid"))?;
            let expected_ready = frame
                .row
                .generated_ps
                .checked_add(
                    self.prepared.ethernet.devices[generator.source].tx_processing_delay_ps,
                )
                .ok_or_else(|| error("Qcc source ready overflow"))?;
            if ordinal_text != ordinal.to_string()
                || generator.time(ordinal) != Some(u128::from(frame.row.generated_ps))
                || frame.row.source != self.prepared.ethernet.devices[generator.source].id
                || frame.row.source_vlan_id != generator.source_vlan_id
                || frame.row.flow_id != generator.flow_id
                || frame.row.priority != generator.priority
                || frame.row.deadline_ps != generator.deadline_ps
                || serde_json::to_value(&frame.row.wire).map_err(|e| error(e.to_string()))?
                    != serde_json::to_value(&generator.frame).map_err(|e| error(e.to_string()))?
                || binding.ready_ps != expected_ready
            {
                return Err(error(
                    "Qcc source checkpoint differs from immutable generator",
                ));
            }
            if binding.generator != generator.id
                || p.generator_index.get(&binding.generator) != Some(&binding.stream)
                || frame.row.ready_ps != Some(binding.ready_ps)
                || binding.ready_ps > horizon
                || binding.interval_origin_ps
                    != binding.ready_ps / p.config.cycle_ps.0 * p.config.cycle_ps.0
                || binding.payload_bytes != frame.row.wire.data_hex.len() / 2
                || binding.version > state.version
            {
                return Err(error("Qcc source checkpoint provenance differs"));
            }
            let plan = installations
                .iter()
                .rev()
                .find(|(at, _, _)| *at <= binding.ready_ps)
                .ok_or_else(|| error("Qcc source has no installed plan"))?;
            let stream = &p.streams[binding.stream];
            if plan.1 != binding.version
                || !plan.2.streams.contains(&binding.stream)
                || plan.2.latency_bounds_ps.get(&stream.requirement.id)
                    != Some(&binding.latency_bound_ps)
            {
                return Err(error("Qcc source checkpoint certificate differs"));
            }
            actual.push((
                binding.generator.clone(),
                binding.ready_ps,
                binding.payload_bytes,
            ));
        }
        actual.sort();
        let mut audited = state.source_admissions();
        audited.sort();
        if actual != audited {
            return Err(error("Qcc source checkpoint audit differs"));
        }
        Ok(())
    }

    pub(super) fn validate_qcc_quiescence_history(&self, horizon: u64) -> Result<()> {
        let p = self
            .prepared
            .qcc
            .as_ref()
            .ok_or_else(|| error("missing Qcc prepared input"))?;
        let qcc = self
            .qcc
            .as_ref()
            .ok_or_else(|| error("missing Qcc state"))?;
        for (at, claimed) in qcc.advance_history() {
            if at > horizon {
                return Err(error("Qcc future quiescence audit"));
            }
            // Phase-0 control drains release completions at this time before
            // testing the contract. Direct arrivals and processing completions
            // at this same time run in a later FES phase.
            let mut actual = true;
            for port in &p.ports {
                let direction = port.direction;
                if self.transfers.iter().any(|transfer| {
                    let row = &transfer.row;
                    transfer.direction == direction
                        && row.queued_ps < at
                        && !(row.drop_reason.is_some() && row.time_ps < at)
                        && (row.release_ps.is_none_or(|release| release > at)
                            || row.arrival_ps.is_none_or(|arrival| arrival >= at))
                }) || self.receptions.iter().any(|reception| {
                    self.transfers
                        .get(reception.transfer)
                        .is_some_and(|transfer| {
                            transfer.direction == direction
                                && reception.row.observed_ps < at
                                && reception.row.status != "filtered"
                                && reception.row.ready_ps.is_none_or(|ready| ready >= at)
                        })
                }) {
                    actual = false;
                    break;
                }
            }
            if actual != claimed {
                return Err(error("Qcc quiescence audit differs from physical history"));
            }
        }
        Ok(())
    }

    pub(super) fn qcc_state<'a>(
        &'a self,
        d: &'a NetworkDelta,
    ) -> Option<&'a configuration::QccState> {
        d.qcc.as_ref().or(self.qcc.as_ref())
    }

    pub(super) fn qcc_available(&self, d: &NetworkDelta) -> Result<Vec<bool>> {
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(Vec::new());
        };
        let policy = self.policy(d);
        Ok(self.dynamic.as_ref().zip(policy.as_ref()).map_or_else(
            || vec![true; p.ports.len()],
            |(owner, snapshot)| self.qcc_available_in(owner, snapshot),
        ))
    }

    fn qcc_install(
        &self,
        plan: &ConfigurationPlan,
        version: u64,
        suspend: bool,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let p = self
            .prepared
            .qcc
            .as_ref()
            .ok_or_else(|| error("missing Qcc input"))?;
        if !suspend && !self.qcc_raw_plan_live(plan, d, c.now())? {
            return Err(error(
                "Qcc raw source certificate changed before installation",
            ));
        }
        let tsn = self.tsn.as_ref().ok_or_else(|| error("missing Qcc TAS"))?;
        let mut schedules = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        let mut ports = Vec::new();
        let mut clocks = BTreeMap::new();
        for port in &plan.ports {
            let name = p.ports[port.direction].port.clone();
            schedules.insert(
                name.clone(),
                if suspend && port.slots.is_empty() && port.closed_slots.is_empty() {
                    configuration::schedule(p, port, 0, version)
                } else if let Some(certificate) = plan
                    .schedules
                    .iter()
                    .find(|certificate| certificate.direction == port.direction)
                {
                    let clock_matches = self.clock_for_port(d, &name).map_or_else(
                        || {
                            clock::NodeClock::new(0, 0, 1, 1)
                                .is_ok_and(|identity| identity == certificate.clock)
                        },
                        |clock| clock == &certificate.clock,
                    );
                    if certificate.activation_ps != c.now() || !clock_matches {
                        return Err(error("Qcc schedule dependency changed before installation"));
                    }
                    configuration::compiled_schedule(certificate, version)
                } else {
                    configuration::schedule_clock(
                        p,
                        port,
                        c.now(),
                        version,
                        self.clock_for_port(d, &name),
                    )
                }
                .map_err(|e| error(format!("Qcc schedule: {e:?}")))?,
            );
            if let Some(clock) = self.clock_for_port(d, &name) {
                clocks.insert(name.clone(), clock.clone());
            }
            inputs.insert(name.clone(), self.port_input(port.direction, d)?);
            ports.push(name);
        }
        let planned = if suspend {
            let mut planned = std::mem::take(&mut d.tsn);
            for port in &plan.ports {
                let name = &p.ports[port.direction].port;
                planned = tsn.plan_suspend_clock_in(
                    planned,
                    c.now(),
                    &BTreeMap::from([(name.clone(), schedules[name].clone())]),
                    &BTreeMap::from([(name.clone(), inputs[name].clone())]),
                    p.managed_priority_mask(port.direction),
                    &clocks,
                )?;
            }
            planned
        } else {
            tsn.plan_install_clock_in(
                std::mem::take(&mut d.tsn),
                c.now(),
                &schedules,
                &inputs,
                &clocks,
            )?
        };
        self.stage_tsn(d, planned, &ports)?;
        crate::allocation::reservation_checkpoint("network_qcc_after_domain_install")
            .map_err(|e| error(e.to_string()))?;
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }

    fn qcc_effects(
        &self,
        effects: Vec<configuration::QccEffect>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        for effect in effects {
            match &effect {
                configuration::QccEffect::Installed { version, plan, .. } => {
                    self.qcc_install(plan, *version, plan.streams.is_empty(), d, c)?
                }
                configuration::QccEffect::Suspended { version, .. } => {
                    let p = self.prepared.qcc.as_ref().unwrap();
                    let state = self.qcc_state(d).unwrap();
                    let empty = configuration::solve(
                        p,
                        &Default::default(),
                        state.topology_epoch,
                        state.available(),
                    )
                    .map_err(|e| error(format!("Qcc suspend: {e:?}")))?;
                    self.qcc_install(&empty, *version, true, d, c)?;
                }
                _ => {}
            }
            let id = format!("qcc:{}", d.record_sequence);
            Self::record(
                c,
                d,
                "dir.network.qcc",
                id,
                decimal(serde_json::to_value(&effect).map_err(|e| error(e.to_string()))?),
            )?;
        }
        Ok(())
    }

    pub(super) fn initialize_qcc(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(());
        };
        let mut state = self.qcc.as_ref().unwrap().clone();
        let available = self.qcc_available(d)?;
        if available != state.available() {
            return Err(error("Qcc managed path is initially ineligible"));
        }
        self.qcc_install(&state.active, state.version, true, d, c)?;
        if state.next_deadline(p) == Some(c.now()) {
            let effects = state
                .advance(p, c.now(), true)
                .map_err(|e| error(format!("Qcc initial control: {e:?}")))?;
            d.qcc = Some(state);
            self.qcc_effects(effects, d, c)?;
        }
        Ok(())
    }

    /// Call after staging a clock correction, before renewed eligibility claims.
    /// TSN settlement under the old anchor has already happened in `before`.
    pub(super) fn qcc_clock_changed(
        &self,
        node: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(());
        };
        if !p
            .ports
            .iter()
            .any(|port| port.source == node && p.depends_on_direction(port.direction))
        {
            return Ok(());
        }
        let Some(clock) = self.clocks_after(d).and_then(|v| v.get(node)).cloned() else {
            return Ok(());
        };
        let mut state = self
            .qcc_state(d)
            .ok_or_else(|| error("Qcc state missing at clock change"))?
            .clone();
        let effects = state
            .clock_changed(p, c.now(), node, clock)
            .map_err(|e| error(format!("Qcc clock dependency: {e:?}")))?;
        d.qcc = Some(state);
        self.qcc_effects(effects, d, c)
    }

    /// Link/admin reset or capability expiry/relearn changes the stable Merge
    /// epoch. Ordinary capability refresh is already charged by its envelope.
    pub(super) fn qcc_protocol_changed(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(());
        };
        if !p.depends_on_direction(direction) {
            return Ok(());
        }
        let mut state = self
            .qcc_state(d)
            .ok_or_else(|| error("Qcc state missing at protocol reset"))?
            .clone();
        let effects = state
            .protocol_changed(p, c.now(), direction)
            .map_err(|e| error(format!("Qcc protocol dependency: {e:?}")))?;
        d.qcc = Some(state);
        self.qcc_effects(effects, d, c)
    }

    pub(super) fn qcc_control(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        self.qcc_refresh_raw_sources(d, c)?;
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(());
        };
        let mut state = self.qcc_state(d).unwrap().clone();
        let available = self.qcc_available(d)?;
        let changed_policy =
            self.dynamic
                .as_ref()
                .zip(self.policy(d))
                .is_some_and(|(owner, after)| {
                    self.qcc_policy_changed(&owner.snapshot_policy(), &after)
                });
        if available != state.available() || changed_policy {
            let effects = state
                .topology_changed(p, c.now(), available)
                .map_err(|e| error(format!("Qcc topology: {e:?}")))?;
            d.qcc = Some(state.clone());
            self.qcc_effects(effects, d, c)?;
        }
        if state.next_deadline(p) == Some(c.now()) {
            let quiescent = self.qcc_quiescent(d);
            let effects = state
                .advance(p, c.now(), quiescent)
                .map_err(|e| error(format!("Qcc control: {e:?}")))?;
            d.qcc = Some(state);
            self.qcc_effects(effects, d, c)?;
        }
        Ok(())
    }

    /// None means an ordinary source. An empty route means a rejected managed attempt.
    pub(super) fn qcc_source(
        &self,
        f: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<Option<Vec<usize>>> {
        self.qcc_refresh_raw_sources(d, c)?;
        let Some(p) = self.prepared.qcc.as_ref() else {
            return Ok(None);
        };
        let frame = self.frame(d, f);
        let Some(generator_index) = frame.generator else {
            return Ok(None);
        };
        let generator = &self.prepared.ethernet.generators[generator_index].id;
        if !p.generator_index.contains_key(generator) {
            return Ok(None);
        }
        let payload_bytes = frame.row.wire.data_hex.len() / 2;
        let frame_id = frame.row.frame_id.clone();
        let mut state = self.qcc_state(d).unwrap().clone();
        match state.admit_source(p, generator, c.now(), payload_bytes) {
            Ok(admission) => {
                let binding = SourceBinding {
                    generator: generator.clone(),
                    stream: admission.stream,
                    version: admission.version,
                    interval_origin_ps: admission.interval_origin_ps,
                    ready_ps: c.now(),
                    payload_bytes,
                    latency_bound_ps: state.active.latency_bounds_ps
                        [&p.streams[admission.stream].requirement.id],
                };
                let mut sources = d
                    .qcc_sources
                    .clone()
                    .unwrap_or_else(|| self.qcc_sources.clone());
                sources.insert(f, binding);
                d.qcc_sources = Some(sources);
                d.qcc = Some(state);
                Self::record(
                    c,
                    d,
                    "dir.network.qcc",
                    format!("qcc:admit:{frame_id}"),
                    json!({"kind":"source_admitted","frame_id":frame_id,
                        "generator":generator,"stream":admission.stream.to_string(),
                        "version":admission.version.to_string(),
                        "interval_origin_ps":admission.interval_origin_ps.to_string(),
                        "ready_ps":c.now().to_string(),"payload_bytes":payload_bytes.to_string()}),
                )?;
                Ok(Some(
                    p.streams[admission.stream]
                        .paths
                        .iter()
                        .filter_map(|path| path.first().copied())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                ))
            }
            Err(reason) => {
                Self::record(
                    c,
                    d,
                    "dir.network.qcc",
                    format!("qcc:reject:{frame_id}"),
                    json!({"kind":"source_rejected","frame_id":frame_id,
                        "generator":generator,"reason":reason}),
                )?;
                Ok(Some(Vec::new()))
            }
        }
    }

    pub(super) fn qcc_route_at(
        &self,
        f: usize,
        incoming: usize,
        node: usize,
        d: &NetworkDelta,
    ) -> Option<EgressSelection> {
        let sources = d.qcc_sources.as_ref().unwrap_or(&self.qcc_sources);
        let binding = sources.get(&f)?;
        let p = self.prepared.qcc.as_ref().unwrap();
        let state = self.qcc_state(d).unwrap();
        if !state.active_valid || binding.version != state.version {
            return Some(EgressSelection {
                ports: Vec::new(),
                reason: Some("qcc_suspended".into()),
            });
        }
        let mut next = BTreeSet::new();
        for path in &p.streams[binding.stream].paths {
            if let Some(index) = path.iter().position(|direction| *direction == incoming) {
                if let Some(direction) = path
                    .get(index + 1)
                    .copied()
                    .filter(|direction| p.ports[*direction].source == node)
                {
                    next.insert(direction);
                }
            }
        }
        Some(EgressSelection {
            ports: next
                .into_iter()
                .map(|direction| {
                    self.prepared.ethernet.directions[direction]
                        .from_port
                        .clone()
                })
                .collect(),
            reason: None,
        })
    }

    pub(super) fn qcc_sof(
        &self,
        transfer: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.qcc_refresh_raw_sources(d, c)?;
        let row = self.transfer(d, transfer);
        if row.row.parent_transfer_id.is_some() {
            return Ok(());
        }
        let sources = d.qcc_sources.as_ref().unwrap_or(&self.qcc_sources);
        let Some(binding) = sources.get(&row.frame) else {
            return Ok(());
        };
        let p = self.prepared.qcc.as_ref().unwrap();
        let stream = &p.streams[binding.stream];
        let state = self.qcc_state(d).unwrap();
        let assigned = state
            .active
            .talker_offsets_ps
            .get(&stream.requirement.id)
            .copied();
        let actual = c
            .now()
            .checked_sub(binding.interval_origin_ps)
            .ok_or_else(|| error("Qcc SOF precedes interval origin"))?;
        let first = !sources.iter().any(|(&frame, other)| {
            frame < row.frame
                && other.stream == binding.stream
                && other.interval_origin_ps == binding.interval_origin_ps
        });
        let slot = state
            .active
            .ports
            .iter()
            .find(|port| port.direction == row.direction)
            .and_then(|port| {
                port.slots
                    .iter()
                    .find(|slot| slot.priority == stream.priority)
            });
        let occupied =
            configuration::occupied_ps(row.row.wire.mac_bytes, p.ports[row.direction].bitrate_bps)
                .map_err(|e| error(format!("Qcc SOF occupancy: {e:?}")))?;
        let quantum =
            self.clock_for_port(d, &p.ports[row.direction].port)
                .map_or(0, |clock| {
                    let (num, den) = clock.rate();
                    if num == den
                        && clock
                            .local_at(c.now())
                            .is_ok_and(|time| time.denominator() == 1)
                    {
                        0
                    } else {
                        den.div_ceil(num)
                    }
                })
                .saturating_add(
                    self.prepared
                        .phy_attachments
                        .iter()
                        .find(|a| a.directions.contains(&row.direction))
                        .map_or(0, |a| {
                            let grid = a.config.medium.channel.grid;
                            u64::try_from(2_000_000_000_000u128.div_ceil(
                                grid.symbol_rate_hz as u128 * grid.samples_per_ui as u128,
                            ))
                            .unwrap_or(u64::MAX)
                        }),
                );
        let valid = assigned.is_some_and(|offset| {
            if first {
                actual >= offset.saturating_sub(quantum)
                    && actual
                        <= state
                            .active
                            .talker_latest_offsets_ps
                            .get(&stream.requirement.id)
                            .copied()
                            .unwrap_or_else(|| offset.saturating_add(quantum))
            } else {
                actual >= offset
            }
        }) && slot.is_some_and(|slot| {
            actual >= slot.start_ps.saturating_sub(quantum)
                && actual
                    .checked_add(occupied)
                    .is_some_and(|end| end <= slot.end_ps)
        }) && state.version == binding.version
            && state.active_valid;
        let frame_id = self.frame(d, row.frame).row.frame_id.clone();
        Self::record(
            c,
            d,
            "dir.network.qcc",
            format!("qcc:sof:{frame_id}"),
            json!({"kind":"source_sof","frame_id":frame_id,"stream":stream.requirement.id,
                "version":binding.version.to_string(),"actual_offset_ps":actual.to_string(),
                "assigned_offset_ps":assigned.map(|v|v.to_string()),"first_in_interval":first,
                "within_certificate":valid}),
        )
    }

    pub(super) fn qcc_delivered(
        &self,
        transfer: &Transfer,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.qcc_refresh_raw_sources(d, c)?;
        let sources = d.qcc_sources.as_ref().unwrap_or(&self.qcc_sources);
        let Some(binding) = sources.get(&transfer.frame) else {
            return Ok(());
        };
        let actual = c
            .now()
            .checked_sub(binding.interval_origin_ps)
            .ok_or_else(|| error("Qcc delivery precedes interval origin"))?;
        let valid = actual <= binding.latency_bound_ps
            && self.qcc_state(d).unwrap().version == binding.version
            && self.qcc_state(d).unwrap().active_valid;
        let frame_id = self.frame(d, transfer.frame).row.frame_id.clone();
        Self::record(
            c,
            d,
            "dir.network.qcc",
            format!("qcc:delivery:{frame_id}"),
            json!({"kind":"delivery","frame_id":frame_id,"stream":binding.stream.to_string(),
                "version":binding.version.to_string(),"interval_origin_ps":binding.interval_origin_ps.to_string(),
                "actual_latency_ps":actual.to_string(),"certified_latency_bound_ps":binding.latency_bound_ps.to_string(),
                "within_certificate":valid}),
        )
    }

    fn qcc_quiescent(&self, d: &NetworkDelta) -> bool {
        let Some(p) = self.prepared.qcc.as_ref() else {
            return true;
        };
        p.ports.iter().all(|port| {
            let direction = port.direction;
            let q = self.queue(d, direction);
            q.active.is_none()
                && self.port_input(direction, d).is_ok_and(|input| {
                    input.sending.is_none()
                        && input.service.as_ref().is_none_or(|service| !service.busy)
                        && input
                            .merge
                            .as_ref()
                            .is_none_or(|merge| merge.medium_busy_until.is_none())
                })
                && q.waiting.iter().all(VecDeque::is_empty)
                && self.transfers.iter().enumerate().all(|(i, _)| {
                    let t = self.transfer(d, i);
                    t.direction != direction
                        || t.row.arrival_ps.is_some()
                        || t.row.status == "dropped"
                })
                && self.receptions.iter().enumerate().all(|(i, _)| {
                    let r = self.reception(d, i);
                    self.transfer(d, r.transfer).direction != direction || r.row.ready_ps.is_some()
                })
        })
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    #[test]
    fn qcc_actual_multi_port_install_reservation_failure_rolls_back_all_owners() {
        use crate::{
            CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
        };
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/qcc/model.ini"),
        )
        .unwrap();
        let (mut session, controller) = RunSession::new(
            p.clone(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
            if session.snapshot().common.last_event_time_ps.unwrap_or(0) >= 11_000_000 {
                break;
            }
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let directory =
            std::env::temp_dir().join(format!("qcc-install-atomic-{}", std::process::id()));
        let ack = session
            .checkpoint(&directory, CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let saved: Value =
            serde_json::from_slice(&std::fs::read(directory.join("state.json")).unwrap()).unwrap();
        let horizon = session.snapshot().common.last_event_time_ps.unwrap();
        let state = NetworkState::restore_checkpoint(
            p.registered
                .as_ref()
                .unwrap()
                .network
                .as_ref()
                .unwrap()
                .clone(),
            &saved["network_backend"]["state"],
            horizon,
        )
        .unwrap();
        let records: Vec<(String, crate::registry::ModelRecord)> =
            checkpoint::decode(saved["network_backend"]["records"].clone()).unwrap();
        let envelope = saved["fes"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|entry| {
                let raw = &entry["work"]["envelope"];
                let payload: Vec<u8> = serde_json::from_value(raw["payload"].clone()).ok()?;
                let event: Event = serde_json::from_slice(&payload).ok()?;
                let time_ps: u64 = raw["time_ps"].as_str()?.parse().ok()?;
                (matches!(event, Event::Control(_)) && time_ps == 100_000_000).then(|| Envelope {
                    event_id: raw["event_id"].as_str().unwrap().parse().unwrap(),
                    source: "@network".into(),
                    destination: "@network".into(),
                    request_id: None,
                    time_ps,
                    input_port: None,
                    schema: Schema::new(raw["schema"]["name"].as_str().unwrap(), 1),
                    payload,
                })
            })
            .expect("actual pending activation callback");
        let before = state.checkpoint();
        crate::allocation::fail_next_reservation("network_qcc_after_domain_install");
        let (after, effects) = super::super::super::registered::vector_network_failed_callback(
            p,
            state,
            records.into_iter().map(|(_, record)| record).collect(),
            vec![envelope],
        );
        assert!(
            crate::allocation::reservation_checkpoint("network_qcc_after_domain_install").is_ok()
        );
        assert_eq!(
            after.checkpoint(),
            before,
            "Qcc/TAS/queues/wakes and provenance commit together"
        );
        assert_eq!(effects["committed_effects"], 0);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
