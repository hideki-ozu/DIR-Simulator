use super::*;
use std::collections::BTreeSet;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    escape_policy: String,
    fault_policy: String,
    config: FlitConfig,
    workload: FlitWorkload,
    state: State,
    timer: Option<TimerReference>,
}
impl Network {
    pub(super) fn encode(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        self.validate_state(&self.state)?;
        serde_json::to_vec(&Checkpoint {
            version: 1,
            escape_policy: ESCAPE.into(),
            fault_policy: FAULT_POLICY.into(),
            config: self.config.clone(),
            workload: self.workload.clone(),
            state: self.state.clone(),
            timer: self
                .timer
                .as_ref()
                .map(|t| c.encode_timer_ref(t))
                .transpose()?,
        })
        .map_err(|e| ModelError(format!("NoC checkpoint: {e}")))
    }
    pub(super) fn validate_state(&self, s: &State) -> ModelResult {
        let physical = self.physical_buffers();
        let n = (self.topology.links.len() + self.topology.nodes) * self.vcs();
        if s.packets.len() != self.workload.packets.len()
            || s.healthy.len() != self.topology.links.len()
            || s.buffers.len() != n
            || s.credits.len() != physical
            || s.sources.len() != self.topology.nodes
            || s.ejection.len() != self.topology.nodes
            || s.rr.len() != self.topology.links.len()
            || s.eject_rr.len() != self.topology.nodes
            || (s.output_cursor >= self.topology.links.len().max(1))
            || s.rr.iter().chain(&s.eject_rr).any(|r| *r >= n)
            || s.fault_cursor > self.workload.faults.len()
            || s.routing != self.topology.routing(&s.healthy)
        {
            return Err("NoC checkpoint topology/routing/resource identity mismatch".into());
        }
        let mut healthy = vec![true; self.topology.links.len()];
        let mut epoch = 0;
        let mut cursor = 0;
        while cursor < s.fault_cursor {
            let at = self.workload.faults[cursor].at_ps;
            let mut changed = false;
            if s.last_tick.is_none_or(|last| at > last) {
                return Err("NoC checkpoint future fault cursor".into());
            }
            while cursor < s.fault_cursor && self.workload.faults[cursor].at_ps == at {
                let f = &self.workload.faults[cursor];
                changed |= healthy[f.link_id] != f.up;
                healthy[f.link_id] = f.up;
                cursor += 1;
            }
            if changed {
                epoch += 1;
            }
        }
        if healthy != s.healthy
            || epoch != s.epoch
            || self
                .workload
                .faults
                .get(s.fault_cursor)
                .is_some_and(|f| s.last_tick.is_some_and(|t| f.at_ps <= t))
        {
            return Err("NoC checkpoint stale topology epoch".into());
        }
        let mut seen = BTreeSet::new();
        let mut reserved = vec![0usize; physical];
        let mut returns = vec![0usize; physical];
        let mut check_flit = |f: &Flit| -> ModelResult {
            let p = s
                .packets
                .get(f.packet)
                .ok_or("NoC checkpoint unknown flit packet")?;
            if f.epoch != s.epoch
                || p.terminal.is_some()
                || !p.injected
                || f.index < p.consumed_flits
                || f.index >= p.injected_flits
                || !seen.insert((f.packet, f.index))
            {
                return Err("NoC checkpoint duplicate/stale/invalid flit placement".into());
            }
            Ok(())
        };
        for (b, buffer) in s.buffers.iter().enumerate() {
            if buffer.fifo.len() > self.config.buffer_flits
                || (buffer.owner.is_none()
                    && (!buffer.fifo.is_empty() || buffer.reservation.is_some()))
            {
                return Err("NoC checkpoint FIFO/owner mismatch".into());
            }
            if let Some(p) = buffer.owner {
                let packet = s.packets.get(p).ok_or("NoC checkpoint orphan VC owner")?;
                if !packet.injected
                    || packet.terminal.is_some()
                    || !packet
                        .route
                        .get(packet.released..)
                        .is_some_and(|route| route.contains(&b))
                {
                    return Err("NoC checkpoint orphan VC owner".into());
                }
            }
            for f in &buffer.fifo {
                check_flit(f)?;
                if buffer.owner != Some(f.packet) {
                    return Err("NoC checkpoint wrong FIFO owner".into());
                }
            }
            if buffer
                .fifo
                .iter()
                .zip(buffer.fifo.iter().skip(1))
                .any(|(a, b)| a.index + 1 != b.index)
            {
                return Err("NoC checkpoint unordered FIFO".into());
            }
            if let Some(target) = buffer.reservation {
                if target >= physical
                    || self.topology.links[target / self.vcs()].source != self.router(b)
                    || buffer.owner.is_none()
                {
                    return Err("NoC checkpoint invalid downstream reservation".into());
                }
                let p = buffer.owner.unwrap();
                let route = &s.packets[p].route;
                if !route.windows(2).any(|pair| pair == [b, target])
                    || s.buffers[target].owner != Some(p)
                {
                    return Err("NoC checkpoint reservation ownership mismatch".into());
                }
            }
        }
        let mut physical_launches = BTreeSet::new();
        let mut input_reads = BTreeSet::new();
        for a in &s.pipeline {
            check_flit(&a.flit)?;
            if a.destination_vc >= physical
                || a.source_vc >= n
                || s.buffers[a.destination_vc].owner != Some(a.flit.packet)
                || self.topology.links[a.destination_vc / self.vcs()].source
                    != self.router(a.source_vc)
                || s.last_tick.is_none_or(|last| {
                    a.at <= last
                        || a.at - last > self.config.link_latency_cycles * self.config.cycle_ps
                })
                || a.at % self.config.cycle_ps != 0
                || !physical_launches.insert((a.destination_vc / self.vcs(), a.at))
                || !input_reads.insert((a.source_vc / self.vcs(), a.at))
            {
                return Err("NoC checkpoint invalid pipeline reservation/time".into());
            }
            if !s.packets[a.flit.packet]
                .route
                .windows(2)
                .any(|pair| pair == [a.source_vc, a.destination_vc])
            {
                return Err("NoC checkpoint pipeline outside packet route".into());
            }
            reserved[a.destination_vc] += 1;
        }
        let mut credit_pops = BTreeSet::new();
        for t in &s.returns {
            if t.buffer >= physical
                || t.epoch != s.epoch
                || s.packets.get(t.packet).is_none_or(|p| {
                    t.flit_index >= p.injected_flits
                        || !p.route.contains(&t.buffer)
                        || matches!(
                            p.terminal.as_deref(),
                            Some("topology_epoch_aborted" | "unreachable" | "injection_rejected")
                        )
                })
                || t.at % self.config.cycle_ps != 0
                || s.last_tick.is_none_or(|last| {
                    t.at <= last
                        || t.at - last > self.config.credit_latency_cycles * self.config.cycle_ps
                })
                || !credit_pops.insert((t.buffer / self.vcs(), t.at))
            {
                return Err("NoC checkpoint stale/invalid credit token".into());
            }
            returns[t.buffer] += 1;
        }
        for b in 0..physical {
            if s.credits[b]
                .checked_add(reserved[b])
                .and_then(|v| v.checked_add(s.buffers[b].fifo.len()))
                .and_then(|v| v.checked_add(returns[b]))
                != Some(self.config.buffer_flits)
            {
                return Err("NoC checkpoint credit conservation failure".into());
            }
        }
        for (r, q) in s.ejection.iter().enumerate() {
            if q.len() > self.config.ejection_capacity_flits {
                return Err("NoC checkpoint excessive ejection occupancy".into());
            }
            let mut previous: Option<u64> = None;
            for e in q {
                check_flit(&e.flit)?;
                if self.workload.packets[e.flit.packet].destination != r
                    || e.at % self.config.cycle_ps != 0
                    || s.last_tick.is_none_or(|t| e.at <= t)
                    || (previous.is_none()
                        && s.last_tick.is_some_and(|t| {
                            e.at - t > self.config.ejection_service_cycles * self.config.cycle_ps
                        }))
                    || previous.is_some_and(|p| {
                        p.checked_add(self.config.ejection_service_cycles * self.config.cycle_ps)
                            != Some(e.at)
                    })
                {
                    return Err("NoC checkpoint invalid ejection queue/time".into());
                }
                previous = Some(e.at);
            }
        }
        let mut queued = BTreeSet::new();
        for (r, q) in s.sources.iter().enumerate() {
            if q.len() > self.config.pending_packets + 1 {
                return Err("NoC checkpoint source overflow".into());
            }
            let mut previous = None;
            for (position, p) in q.iter().enumerate() {
                let state = s
                    .packets
                    .get(*p)
                    .ok_or("NoC checkpoint unknown source packet")?;
                let req = &self.workload.packets[*p];
                if !queued.insert(*p)
                    || req.source != r
                    || !state.admitted
                    || state.terminal.is_some()
                    || state.injected_flits >= req.flits(self.config.flit_bytes)
                    || (position > 0 && state.injected)
                    || previous.is_some_and(|before| before >= *p)
                {
                    return Err("NoC checkpoint invalid source queue".into());
                }
                previous = Some(*p);
            }
        }
        for (i, p) in s.packets.iter().enumerate() {
            let req = &self.workload.packets[i];
            let count = req.flits(self.config.flit_bytes);
            if p.injected_flits > count
                || p.consumed_flits > p.injected_flits
                || p.injected != (p.injected_flits > 0)
                || p.released > p.route.len()
                || p.route.len() > self.topology.nodes * 2 + 1
                || (!p.injected
                    && (p.local_vc.is_some()
                        || !p.route.is_empty()
                        || p.released != 0
                        || p.consumed_flits != 0
                        || p.escape
                        || p.stalls != 0))
                || (p.admitted && s.last_tick.is_none_or(|last| req.at_ps > last))
                || (!p.admitted && (p.injected || p.terminal.is_some()))
                || p.local_vc.is_some_and(|b| {
                    b < physical
                        || b >= n
                        || self.router(b) != req.source
                        || p.route.first() != Some(&b)
                })
                || (p.injected && p.local_vc.is_none())
                || (p.escape && p.stalls < self.config.escape_after_stalls)
                || (!p.escape && p.route.iter().any(|b| *b < physical && b % self.vcs() == 0))
                || p.route.iter().any(|b| *b >= n)
            {
                return Err("NoC checkpoint invalid packet counters/mode/route".into());
            }
            let expected_bytes = req
                .payload
                .len()
                .min(p.consumed_flits * self.config.flit_bytes);
            if p.terminal.as_deref() != Some("topology_epoch_aborted")
                && p.payload != req.payload[..expected_bytes]
            {
                return Err("NoC checkpoint assembled payload mismatch".into());
            }
            if let Some(kind) = p.terminal.as_deref() {
                if !matches!(
                    kind,
                    "packet_delivered"
                        | "topology_epoch_aborted"
                        | "unreachable"
                        | "injection_rejected"
                ) || (kind == "packet_delivered"
                    && (p.consumed_flits != count || p.released != p.route.len()))
                    || (kind == "topology_epoch_aborted" && (!p.injected || !p.payload.is_empty()))
                    || (matches!(kind, "unreachable" | "injection_rejected") && p.injected)
                {
                    return Err("NoC checkpoint invalid terminal accounting".into());
                }
            } else {
                if p.admitted && p.injected_flits < count && !queued.contains(&i) {
                    return Err("NoC checkpoint missing pending injection".into());
                }
                for index in p.consumed_flits..p.injected_flits {
                    if !seen.contains(&(i, index)) {
                        return Err("NoC checkpoint missing live flit".into());
                    }
                }
                for &b in &p.route[p.released..] {
                    if s.buffers[b].owner != Some(i) {
                        return Err("NoC checkpoint lost VC owner".into());
                    }
                }
                let mut escaped = false;
                let mut visited = BTreeSet::new();
                for route in p.route.windows(2) {
                    let l = route[1] / self.vcs();
                    if l >= self.topology.links.len()
                        || self.topology.links[l].source != self.router(route[0])
                        || !visited.insert(route[1])
                    {
                        return Err("NoC checkpoint disconnected/cyclic route".into());
                    }
                    if route[1] % self.vcs() == 0 {
                        escaped = true;
                        if self.topology.escape_next(
                            &s.routing,
                            self.router(route[0]),
                            req.destination,
                        ) != Some(l)
                        {
                            return Err("NoC checkpoint invalid escape path".into());
                        }
                    } else if escaped
                        || s.routing.distances[req.destination][self.router(route[1])]
                            .zip(s.routing.distances[req.destination][self.router(route[0])])
                            .is_none_or(|(a, b)| a + 1 != b)
                    {
                        return Err("NoC checkpoint nonminimal adaptive/escape reversal".into());
                    }
                }
            }
        }
        if s.last_tick.is_some_and(|t| t % self.config.cycle_ps != 0)
            || s.next_tick.is_some_and(|t| {
                t % self.config.cycle_ps != 0 || s.last_tick.is_some_and(|last| t <= last)
            })
            || s.next_tick != self.next_time(s, s.last_tick.unwrap_or(0))?
        {
            return Err("NoC checkpoint invalid next tick".into());
        }
        Ok(())
    }
}
pub(super) fn restore(
    c: &ModelConfig,
    version: u32,
    bytes: &[u8],
    context: &CheckpointDecodeContext<'_>,
) -> ModelResult<Box<dyn Model>> {
    if version != 1 {
        return Err("unsupported NoC checkpoint codec".into());
    }
    let saved: Checkpoint =
        serde_json::from_slice(bytes).map_err(|e| ModelError(format!("NoC checkpoint: {e}")))?;
    let (config, workload) = crate::input::noc_flit::parse(&c.profile_input)?;
    if saved.version != 1
        || saved.escape_policy != ESCAPE
        || saved.fault_policy != FAULT_POLICY
        || saved.config != config
        || saved.workload != workload
    {
        return Err("NoC checkpoint immutable identity mismatch".into());
    }
    let timer = saved
        .timer
        .as_ref()
        .map(|t| context.decode_timer_ref(t))
        .transpose()?;
    if let Some(token) = &timer {
        let envelope = context.timer_envelope(token)?;
        if Some(envelope.time_ps) != saved.state.next_tick
            || envelope.schema != tick_schema()
            || !envelope.payload.is_empty()
            || envelope.source != c.id
            || envelope.destination != c.id
        {
            return Err("NoC checkpoint tick envelope mismatch".into());
        }
    }
    if timer.is_some() != saved.state.next_tick.is_some()
        || saved.timer.as_ref().is_some_and(|t| t.state != "live")
        || context
            .live_timers
            .iter()
            .filter(|t| t.owner == context.owner)
            .count()
            != usize::from(timer.is_some())
    {
        return Err("NoC checkpoint tick ownership mismatch".into());
    }
    let network = Network {
        topology: Topology::new(&config.topology),
        config,
        workload,
        state: saved.state,
        timer,
    };
    network.validate_state(&network.state)?;
    Ok(Box::new(network))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn malformed_checkpoint_rejects_duplicate_credit_epoch_owner_and_timer() {
        let config:FlitConfig=serde_json::from_value(json!({"schema":"dir.noc-flit.model","version":1,"topology":{"kind":"mesh","width":2,"height":1},"flit_bytes":2,"buffer_flits":1,"adaptive_vcs":1,"cycle_ps":1,"link_latency_cycles":2,"credit_latency_cycles":3,"escape_after_stalls":2,"ejection_capacity_flits":1,"ejection_service_cycles":2,"pending_packets":4})).unwrap();
        let workload = FlitWorkload {
            schema: "dir.noc-flit.workload".into(),
            version: 1,
            packets: vec![Packet {
                id: "p".into(),
                at_ps: 0,
                source: 0,
                destination: 1,
                payload: vec![1, 2, 3, 4],
            }],
            faults: vec![],
        };
        let topology = Topology::new(&config.topology);
        let healthy = vec![true; 2];
        let mut buffers = vec![Buffer::default(); 8];
        buffers[5].owner = Some(0);
        buffers[5].reservation = Some(1);
        buffers[1].owner = Some(0);
        let state = State {
            packets: vec![PacketState {
                admitted: true,
                injected: true,
                injected_flits: 1,
                local_vc: Some(5),
                route: vec![5, 1],
                ..Default::default()
            }],
            healthy: healthy.clone(),
            routing: topology.routing(&healthy),
            epoch: 0,
            buffers,
            credits: vec![1, 0, 1, 1],
            pipeline: vec![Transit {
                flit: Flit {
                    packet: 0,
                    index: 0,
                    epoch: 0,
                },
                source_vc: 5,
                destination_vc: 1,
                at: 2,
            }],
            returns: vec![],
            sources: vec![VecDeque::from([0]), VecDeque::new()],
            ejection: vec![VecDeque::new(); 2],
            rr: vec![0; 2],
            output_cursor: 0,
            eject_rr: vec![0; 2],
            fault_cursor: 0,
            sequence: 3,
            last_tick: Some(0),
            next_tick: Some(1),
        };
        let token = TimerToken {
            owner: "noc".into(),
            id: 7,
        };
        let live = BTreeSet::from([token]);
        let pending_timers = std::collections::BTreeMap::from([(
            live.first().unwrap().clone(),
            Envelope {
                event_id: 7,
                source: "noc".into(),
                destination: "noc".into(),
                request_id: None,
                time_ps: 1,
                input_port: None,
                schema: tick_schema(),
                payload: vec![],
            },
        )]);
        let context = CheckpointDecodeContext {
            owner: "noc",
            next_event_id: 8,
            live_timers: &live,
            reservations: &live,
            pending_timers: &pending_timers,
        };
        let model = ModelConfig {
            profile_input: Arc::new(ProfileInput {
                model_config: Some(serde_json::to_value(&config).unwrap()),
                workload: Some(serde_json::to_value(&workload).unwrap()),
            }),
            id: "noc".into(),
            subject: "noc".into(),
            implementation_key: IMPLEMENTATION.into(),
            parameters: Default::default(),
            connections: vec![],
            output_sources: Default::default(),
        };
        let saved = Checkpoint {
            version: 1,
            escape_policy: ESCAPE.into(),
            fault_policy: FAULT_POLICY.into(),
            config,
            workload,
            state,
            timer: Some(TimerReference {
                version: 1,
                owner: "noc".into(),
                id: "7".into(),
                state: "live".into(),
            }),
        };
        let good = serde_json::to_value(saved).unwrap();
        assert!(restore(&model, 1, &serde_json::to_vec(&good).unwrap(), &context).is_ok());
        let mut cases = Vec::new();
        let mut v = good.clone();
        let duplicate = v["state"]["pipeline"][0].clone();
        v["state"]["pipeline"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        cases.push(("duplicate flit", v));
        let mut v = good.clone();
        v["state"]["credits"][1] = json!(1);
        cases.push(("credit leak", v));
        let mut v = good.clone();
        v["state"]["pipeline"][0]["flit"]["epoch"] = json!(1);
        cases.push(("stale epoch", v));
        let mut v = good.clone();
        v["state"]["buffers"][0]["owner"] = json!(0);
        cases.push(("orphan owner", v));
        let mut v = good.clone();
        v["timer"]["owner"] = json!("other");
        cases.push(("wrong timer", v));
        let mut v = good.clone();
        v["state"]["pipeline"][0]["at"] = json!(0);
        cases.push(("past arrival", v));
        let mut v = good.clone();
        v["state"]["routing"]["tree_links"][0] = json!(false);
        cases.push(("wrong tree", v));
        for (name, v) in cases {
            assert!(
                restore(&model, 1, &serde_json::to_vec(&v).unwrap(), &context).is_err(),
                "accepted {name}"
            );
        }
    }
}
