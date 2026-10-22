use super::*;
use std::collections::BTreeSet;
impl Network {
    pub(super) fn tick(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let now = c.now();
        // All physical updates at one timestamp share one epoch disposition.
        let start = s.fault_cursor;
        while s.fault_cursor < self.workload.faults.len()
            && self.workload.faults[s.fault_cursor].at_ps == now
        {
            s.fault_cursor += 1;
        }
        let mut changed = false;
        for f in &self.workload.faults[start..s.fault_cursor] {
            changed |= s.healthy[f.link_id] != f.up;
            s.healthy[f.link_id] = f.up;
        }
        if changed {
            let previous = s.epoch;
            s.epoch = s.epoch.checked_add(1).ok_or("NoC epoch overflow")?;
            let fault_ids: Vec<_> = self.workload.faults[start..s.fault_cursor]
                .iter()
                .map(|f| &f.id)
                .collect();
            for p in 0..s.packets.len() {
                if s.packets[p].injected && s.packets[p].terminal.is_none() {
                    s.packets[p].terminal = Some("topology_epoch_aborted".into());
                    s.packets[p].payload.clear();
                    self.packet_event(
                        s,
                        c,
                        "topology_epoch_aborted",
                        p,
                        json!({"previous_epoch":previous,"fault_ids":fault_ids}),
                    )?;
                }
            }
            s.buffers = self.fresh_buffers();
            s.credits.fill(self.config.buffer_flits);
            s.pipeline.clear();
            s.returns.clear();
            for q in &mut s.ejection {
                q.clear();
            }
            for q in &mut s.sources {
                q.retain(|p| s.packets[*p].terminal.is_none());
            }
            s.routing = self.topology.routing(&s.healthy);
            self.emit(
                s,
                c,
                "topology_changed",
                None,
                json!({"previous_epoch":previous,"fault_ids":fault_ids,"healthy_links":s.healthy}),
            )?;
        }
        // Canonical request order; bounded source FIFO includes one active stream.
        for p in 0..self.workload.packets.len() {
            let r = &self.workload.packets[p];
            if r.at_ps > now || s.packets[p].admitted || s.packets[p].terminal.is_some() {
                continue;
            }
            s.packets[p].admitted = true;
            if s.routing.distances[r.destination][r.source].is_none() {
                s.packets[p].terminal = Some("unreachable".into());
                self.packet_event(s, c, "unreachable", p, json!({}))?;
            } else if s.sources[r.source].len() > self.config.pending_packets {
                s.packets[p].terminal = Some("injection_rejected".into());
                self.packet_event(s, c, "injection_rejected", p, json!({"source":r.source}))?;
            } else {
                s.sources[r.source].push_back(p);
                self.packet_event(s,c,"packet_admitted",p,json!({"source":r.source,"destination":r.destination,"flits":r.flits(self.config.flit_bytes)}))?;
            }
        }
        for source in 0..s.sources.len() {
            let queued: Vec<_> = s.sources[source].iter().copied().collect();
            for p in queued {
                let r = &self.workload.packets[p];
                if !s.packets[p].injected && s.routing.distances[r.destination][r.source].is_none()
                {
                    s.packets[p].terminal = Some("unreachable".into());
                    s.sources[source].retain(|i| *i != p);
                    self.packet_event(s, c, "unreachable", p, json!({}))?;
                }
            }
        }
        let arrivals = std::mem::take(&mut s.pipeline);
        for a in arrivals {
            if a.at <= now {
                s.buffers[a.destination_vc].fifo.push_back(a.flit.clone());
                self.emit(s,c,"flit_arrived",Some(&a.flit),json!({"link_id":a.destination_vc/self.vcs(),"source_vc":a.source_vc,"destination_vc":a.destination_vc,"valid_bytes":self.workload.packets[a.flit.packet].bytes(a.flit.index,self.config.flit_bytes).len()}))?;
            } else {
                s.pipeline.push(a);
            }
        }
        let returns = std::mem::take(&mut s.returns);
        for t in returns {
            if t.at <= now {
                s.credits[t.buffer] += 1;
                self.emit(s,c,"credit_returned",Some(&Flit{packet:t.packet,index:t.flit_index,epoch:t.epoch}),json!({"link_id":t.buffer/self.vcs(),"destination_vc":t.buffer,"available_credits":s.credits[t.buffer]}))?;
            } else {
                s.returns.push(t);
            }
        }
        let used_inputs = self.eject(s, c)?;
        // One new source flit per source per cycle; local FIFOs are finite too.
        for source in 0..s.sources.len() {
            let Some(&p) = s.sources[source].front() else {
                continue;
            };
            if s.packets[p].local_vc.is_none() {
                let base = self.physical_buffers() + source * self.vcs();
                if let Some(b) =
                    (base + 1..base + self.vcs()).find(|b| s.buffers[*b].owner.is_none())
                {
                    s.buffers[b].owner = Some(p);
                    s.packets[p].local_vc = Some(b);
                    s.packets[p].route.push(b);
                    self.packet_event(
                        s,
                        c,
                        "vc_allocated",
                        p,
                        json!({"destination_vc":b,"local":true}),
                    )?;
                }
            }
            let Some(b) = s.packets[p].local_vc else {
                continue;
            };
            if s.buffers[b].fifo.len() == self.config.buffer_flits {
                continue;
            }
            let f = Flit {
                packet: p,
                index: s.packets[p].injected_flits,
                epoch: s.epoch,
            };
            s.packets[p].injected = true;
            s.packets[p].injected_flits += 1;
            s.buffers[b].fifo.push_back(f.clone());
            self.emit(s,c,"flit_injected",Some(&f),json!({"destination_vc":b,"valid_bytes":self.workload.packets[p].bytes(f.index,self.config.flit_bytes).len()}))?;
            if s.packets[p].injected_flits == self.workload.packets[p].flits(self.config.flit_bytes)
            {
                s.sources[source].pop_front();
            }
        }
        self.launch(s, c, used_inputs)?;
        Ok(())
    }
    fn pop(&self, s: &mut State, c: &mut Context<'_>, b: usize) -> ModelResult<Flit> {
        let f = s.buffers[b]
            .fifo
            .pop_front()
            .ok_or("NoC empty buffer pop")?;
        if b < self.physical_buffers() {
            s.returns.push(Credit {
                buffer: b,
                at: self.later(c.now(), self.config.credit_latency_cycles)?,
                epoch: s.epoch,
                packet: f.packet,
                flit_index: f.index,
            });
        }
        if f.index + 1 == self.workload.packets[f.packet].flits(self.config.flit_bytes) {
            s.buffers[b].owner = None;
            s.buffers[b].reservation = None;
            s.packets[f.packet].released += 1;
            self.emit(s, c, "vc_released", Some(&f), json!({"source_vc":b}))?;
        }
        Ok(f)
    }
    fn eject(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult<BTreeSet<usize>> {
        let mut inputs = BTreeSet::new();
        for router in 0..self.topology.nodes {
            // A single serial service port; queue entries have ordered service completion times.
            if s.ejection[router].front().is_some_and(|e| e.at <= c.now()) {
                let e = s.ejection[router].pop_front().unwrap();
                let p = e.flit.packet;
                if e.flit.index != s.packets[p].consumed_flits {
                    return Err("NoC out-of-order packet ejection".into());
                }
                s.packets[p].consumed_flits += 1;
                s.packets[p].payload.extend_from_slice(
                    self.workload.packets[p].bytes(e.flit.index, self.config.flit_bytes),
                );
                self.emit(s,c,"flit_ejected",Some(&e.flit),json!({"router":router,"valid_bytes":self.workload.packets[p].bytes(e.flit.index,self.config.flit_bytes).len()}))?;
                if s.packets[p].consumed_flits
                    == self.workload.packets[p].flits(self.config.flit_bytes)
                {
                    s.packets[p].terminal = Some("packet_delivered".into());
                    self.packet_event(
                        s,
                        c,
                        "packet_delivered",
                        p,
                        json!({"payload":s.packets[p].payload,"router":router}),
                    )?;
                }
            }
            if s.ejection[router].len() >= self.config.ejection_capacity_flits {
                continue;
            }
            let len = s.buffers.len();
            let cursor = s.eject_rr[router];
            if let Some(b) = (0..len).map(|i| (i + cursor) % len).find(|b| {
                self.router(*b) == router
                    && s.buffers[*b]
                        .fifo
                        .front()
                        .is_some_and(|f| self.workload.packets[f.packet].destination == router)
            }) {
                inputs.insert(self.input_port(b));
                let f = self.pop(s, c, b)?;
                let base = s.ejection[router]
                    .back()
                    .map_or(c.now(), |e| e.at.max(c.now()));
                s.ejection[router].push_back(Ejection {
                    flit: f,
                    at: self.later(base, self.config.ejection_service_cycles)?,
                });
                s.eject_rr[router] = (b + 1) % len;
            }
        }
        Ok(inputs)
    }
    fn launch(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        mut inputs: BTreeSet<usize>,
    ) -> ModelResult {
        let n = s.buffers.len();
        let mut proposals = vec![Vec::new(); self.topology.links.len()];
        let mut waiting = Vec::new();
        for b in 0..n {
            let Some(f) = s.buffers[b].fifo.front() else {
                continue;
            };
            let p = f.packet;
            let here = self.router(b);
            let dst = self.workload.packets[p].destination;
            if here == dst {
                continue;
            }
            if let Some(target) = s.buffers[b].reservation {
                if s.credits[target] > 0 {
                    proposals[target / self.vcs()].push((b, target, false));
                }
                continue;
            }
            if f.index != 0 {
                return Err("NoC body without head reservation".into());
            }
            waiting.push((b, p));
            let mut candidates = Vec::new();
            if s.packets[p].escape {
                if let Some(l) = self.topology.escape_next(&s.routing, here, dst) {
                    let target = l * self.vcs();
                    if s.buffers[target].owner.is_none() && s.credits[target] > 0 {
                        candidates.push(target);
                    }
                }
            } else {
                for &l in &self.topology.outgoing[here] {
                    let next = self.topology.links[l].destination;
                    if s.healthy[l]
                        && s.healthy[self.topology.links[l].reverse]
                        && s.routing.distances[dst][next]
                            .zip(s.routing.distances[dst][here])
                            .is_some_and(|(a, b)| a + 1 == b)
                    {
                        for vc in 1..self.vcs() {
                            let target = l * self.vcs() + vc;
                            if s.buffers[target].owner.is_none() && s.credits[target] > 0 {
                                candidates.push(target);
                            }
                        }
                    }
                }
            }
            candidates.sort_by_key(|t| (std::cmp::Reverse(s.credits[*t]), *t));
            if let Some(&target) = candidates.first() {
                proposals[target / self.vcs()].push((b, target, true));
            }
        }
        let mut granted = BTreeSet::new();
        let link_count = proposals.len();
        for offset in 0..link_count {
            let link = (s.output_cursor + offset) % link_count;
            let contenders = &mut proposals[link];
            contenders.sort_by_key(|(b, t, _)| ((b + n - s.rr[link]) % n, *t));
            let Some(&(b, target, allocate)) = contenders
                .iter()
                .find(|(b, _, _)| !inputs.contains(&self.input_port(*b)))
            else {
                continue;
            };
            inputs.insert(self.input_port(b));
            let f = s.buffers[b].fifo.front().unwrap().clone();
            granted.insert(b);
            if allocate {
                s.buffers[target].owner = Some(f.packet);
                s.buffers[b].reservation = Some(target);
                if !s.packets[f.packet].escape {
                    s.packets[f.packet].stalls = 0;
                }
                s.packets[f.packet].route.push(target);
                self.emit(s,c,"vc_allocated",Some(&f),json!({"link_id":link,"source_vc":b,"destination_vc":target,"escape":target%self.vcs()==0}))?;
            }
            s.credits[target] -= 1;
            let f = self.pop(s, c, b)?;
            s.pipeline.push(Transit {
                flit: f.clone(),
                source_vc: b,
                destination_vc: target,
                at: self.later(c.now(), self.config.link_latency_cycles)?,
            });
            s.rr[link] = (b + 1) % n;
            self.emit(s,c,"flit_launched",Some(&f),json!({"link_id":link,"source_router":self.topology.links[link].source,"destination_router":self.topology.links[link].destination,"source_vc":b,"destination_vc":target,"escape":target%self.vcs()==0,"available_credits":s.credits[target]}))?;
        }
        if link_count > 0 {
            s.output_cursor = (s.output_cursor + 1) % link_count;
        }
        for (b, p) in waiting {
            if !granted.contains(&b) && !s.packets[p].escape {
                s.packets[p].stalls = s.packets[p]
                    .stalls
                    .checked_add(1)
                    .ok_or("NoC stall counter overflow")?;
                if s.packets[p].stalls >= self.config.escape_after_stalls {
                    s.packets[p].escape = true;
                    self.packet_event(s, c, "escape_entered", p, json!({"source_vc":b}))?;
                }
            }
        }
        Ok(())
    }
}
