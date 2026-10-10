//! Sparse deterministic completion/generation/arbitration phases for three SoC profiles.
use crate::snapshot::Snapshot;
use crate::snapshot::soc::{ActivePlan, SocSnapshot, Transaction, Transfer};
use crate::types::soc::PreparedSoc;
use crate::types::{Diagnostic, PreparedSimulation};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub mod protocol;
type Result<T> = std::result::Result<T, Diagnostic>;
fn overflow() -> Diagnostic {
    Diagnostic {
        schema_version: 1,
        code: "E-0004".into(),
        stage: "run".into(),
        message: "SoC time or event limit overflow".into(),
        details: None,
        ..Diagnostic::execution("").with_reason("arithmetic_overflow")
    }
}
fn checked(n: u128) -> Result<u64> {
    u64::try_from(n).map_err(|_| overflow())
}
fn edge(t: u64, p: u64) -> Result<u64> {
    checked((t as u128).div_ceil(p as u128) * p as u128)
}
struct State {
    journal: SocSnapshot,
    sources: Vec<VecDeque<usize>>,
    inputs: BTreeMap<String, VecDeque<usize>>,
    reserved: BTreeMap<String, u64>,
    active: BTreeMap<String, usize>,
    cursor: BTreeMap<String, usize>,
    completions: BTreeMap<u64, Vec<usize>>,
    wakes: BTreeSet<u64>,
    used: Vec<u64>,
    unfinished: usize,
    undo_limit: usize,
    undo_rows: BTreeMap<usize, Transaction>,
}
/// Only live queues/reservations and journal append boundaries are checkpointed.
/// Completed history is never copied when another callback starts.
struct Checkpoint {
    sources: Vec<VecDeque<usize>>,
    inputs: BTreeMap<String, VecDeque<usize>>,
    reserved: BTreeMap<String, u64>,
    active: BTreeMap<String, usize>,
    cursor: BTreeMap<String, usize>,
    completions: BTreeMap<u64, Vec<usize>>,
    wakes: BTreeSet<u64>,
    used: Vec<u64>,
    unfinished: usize,
    transactions: usize,
    transfers: usize,
    gauges: BTreeMap<String, (usize, u64)>,
    busy: BTreeMap<String, usize>,
}
impl State {
    fn new(sources: usize) -> Self {
        Self {
            journal: SocSnapshot::default(),
            sources: vec![VecDeque::new(); sources],
            inputs: BTreeMap::new(),
            reserved: BTreeMap::new(),
            active: BTreeMap::new(),
            cursor: BTreeMap::new(),
            completions: BTreeMap::new(),
            wakes: BTreeSet::new(),
            used: vec![0; sources],
            unfinished: 0,
            undo_limit: 0,
            undo_rows: BTreeMap::new(),
        }
    }
    fn checkpoint(&mut self) -> Checkpoint {
        self.undo_limit = self.journal.transactions.len();
        self.undo_rows.clear();
        Checkpoint {
            sources: self.sources.clone(),
            inputs: self.inputs.clone(),
            reserved: self.reserved.clone(),
            active: self.active.clone(),
            cursor: self.cursor.clone(),
            completions: self.completions.clone(),
            wakes: self.wakes.clone(),
            used: self.used.clone(),
            unfinished: self.unfinished,
            transactions: self.journal.transactions.len(),
            transfers: self.journal.transfers.len(),
            gauges: self
                .journal
                .queues
                .iter()
                .map(|(id, g)| (id.clone(), (g.changes.len(), g.maximum)))
                .collect(),
            busy: self
                .journal
                .busy
                .iter()
                .map(|(id, intervals)| (id.clone(), intervals.len()))
                .collect(),
        }
    }
    fn transaction_mut(&mut self, index: usize) -> &mut Transaction {
        if index < self.undo_limit && !self.undo_rows.contains_key(&index) {
            self.undo_rows
                .insert(index, self.journal.transactions[index].clone());
        }
        &mut self.journal.transactions[index]
    }
    fn commit(&mut self) {
        self.undo_rows.clear();
    }
    fn rollback(&mut self, before: Checkpoint) {
        for (index, row) in std::mem::take(&mut self.undo_rows) {
            self.journal.transactions[index] = row;
        }
        self.journal.transactions.truncate(before.transactions);
        self.journal.transfers.truncate(before.transfers);
        self.journal.queues.retain(|id, g| {
            if let Some(&(len, maximum)) = before.gauges.get(id) {
                g.changes.truncate(len);
                g.maximum = maximum;
                true
            } else {
                false
            }
        });
        self.journal.busy.retain(|id, intervals| {
            if let Some(&len) = before.busy.get(id) {
                intervals.truncate(len);
                true
            } else {
                false
            }
        });
        self.sources = before.sources;
        self.inputs = before.inputs;
        self.reserved = before.reserved;
        self.active = before.active;
        self.cursor = before.cursor;
        self.completions = before.completions;
        self.wakes = before.wakes;
        self.used = before.used;
        self.unfinished = before.unfinished;
    }
    fn queue(&mut self, id: &str, t: u64, n: usize) {
        self.journal
            .queues
            .entry(id.into())
            .or_default()
            .set(t, n as u64);
    }
}
fn source_queue(m: &PreparedSoc, s: usize) -> String {
    if m.prefix() == "noc" {
        format!("{}:source", m.sources[s].node)
    } else {
        m.sources[s].node.clone()
    }
}
fn complete(m: &PreparedSoc, s: &mut State, now: u64) -> Result<()> {
    let mut cohort = s.completions.remove(&now).unwrap_or_default();
    if let Some(&r) = cohort.first() {
        let row = &s.journal.transactions[r];
        let plan = row
            .active_plan
            .as_ref()
            .ok_or_else(|| Diagnostic::execution("missing completion active plan"))?;
        let event = protocol::Event::Complete {
            resource: plan.resource.clone(),
            request_id: row.id.clone(),
            hop: plan.hop,
        };
        if protocol::Event::decode(
            &m.profile,
            &format!("{}.Complete", m.profile),
            1,
            0,
            &event.encode(),
        )? != event
        {
            return Err(Diagnostic::execution(
                "completion dispatcher codec mismatch",
            ));
        }
    }
    cohort.sort_by(|&a, &b| {
        let a = &s.journal.transactions[a];
        let b = &s.journal.transactions[b];
        (a.active_plan.as_ref().map(|p| &p.resource), &a.id)
            .cmp(&(b.active_plan.as_ref().map(|p| &p.resource), &b.id))
    });
    for index in cohort {
        let plan = s.journal.transactions[index]
            .active_plan
            .clone()
            .ok_or_else(|| Diagnostic::execution("SoC completion without owner"))?;
        if plan.planned_end_ps != now || s.active.remove(&plan.resource) != Some(index) {
            return Err(Diagnostic::execution("SoC completion ownership mismatch"));
        }
        s.journal.transfers.push(Transfer {
            request: index,
            plan: plan.clone(),
            end_ps: now,
        });
        if plan.downstream.is_none() {
            let source = m.generators[s.journal.transactions[index].generator].source;
            s.used[source] = s.used[source]
                .checked_sub(1)
                .ok_or_else(|| Diagnostic::execution("SoC source usage underflow"))?;
            s.unfinished = s
                .unfinished
                .checked_sub(1)
                .ok_or_else(|| Diagnostic::execution("SoC unfinished count underflow"))?;
        }
        let row = s.transaction_mut(index);
        row.time_ps = now;
        row.active_plan = None;
        row.hop += 1;
        if let Some(downstream) = &plan.downstream {
            let reserved = s
                .reserved
                .get_mut(downstream)
                .ok_or_else(|| Diagnostic::execution("NoC missing reservation"))?;
            *reserved = reserved
                .checked_sub(1)
                .ok_or_else(|| Diagnostic::execution("NoC reservation underflow"))?;
            let q = s.inputs.get_mut(downstream).unwrap();
            q.push_back(index);
            let len = q.len();
            s.queue(downstream, now, len);
        } else {
            row.status = "completed".into();
            row.completed_ps = Some(now);
            row.response = plan.response.clone();
        }
    }
    s.wakes.insert(now);
    Ok(())
}
fn grant(m: &PreparedSoc, s: &mut State, index: usize, plan: ActivePlan, now: u64) -> Result<()> {
    s.completions
        .entry(plan.planned_end_ps)
        .or_default()
        .push(index);
    s.active.insert(plan.resource.clone(), index);
    s.journal
        .busy
        .entry(plan.resource.clone())
        .or_default()
        .push((now, plan.planned_end_ps));
    let row = s.transaction_mut(index);
    row.time_ps = now;
    row.status = "active".into();
    row.start_ps.get_or_insert(now);
    if m.prefix() != "noc" {
        row.target = plan.to.clone();
    }
    row.active_plan = Some(plan);
    Ok(())
}
fn bus_arbitrate(m: &PreparedSoc, s: &mut State, now: u64) -> Result<()> {
    if !s.active.is_empty() {
        return Ok(());
    }
    let cursor = *s.cursor.get(&m.bus).unwrap_or(&0);
    let candidates: Vec<_> = (0..m.sources.len())
        .filter(|&i| {
            s.sources[i]
                .front()
                .is_some_and(|&r| s.journal.transactions[r].generated_ps <= now)
        })
        .collect();
    let winner = if m.arbitration == "fixed_priority" {
        candidates
            .into_iter()
            .min_by_key(|&i| (m.sources[i].priority, &m.sources[i].node))
    } else {
        (0..m.sources.len())
            .map(|offset| (cursor + offset) % m.sources.len())
            .find(|i| candidates.contains(i))
    };
    if let Some(source) = winner {
        let index = *s.sources[source].front().unwrap();
        let row = &s.journal.transactions[index];
        let tx = &m.generators[row.generator].transaction;
        let address = tx.address.unwrap();
        let target = m
            .targets
            .iter()
            .find(|t| address >= t.base && address + tx.bytes <= t.base + t.size);
        let error = target.is_none_or(|t| {
            t.errors
                .iter()
                .any(|&(a, b)| address < b && a < address + tx.bytes)
        });
        let cycles = if m.prefix() == "ahb" {
            2 + target.map_or(0, |t| t.cycles) + u64::from(error)
        } else {
            tx.bytes.div_ceil(m.bytes_per_cycle) + target.map_or(0, |t| t.cycles)
        };
        let end = checked(now as u128 + cycles as u128 * m.clock_period as u128)?;
        let plan = ActivePlan {
            resource: m.bus.clone(),
            hop: 0,
            start_ps: now,
            planned_end_ps: end,
            from: m.sources[source].node.clone(),
            to: target.map(|t| t.node.clone()),
            downstream: None,
            response: Some(if error { "ERROR" } else { "OKAY" }.into()),
            address_end_ps: (m.prefix() == "ahb").then(|| now + m.clock_period),
            nominal_data_end_ps: (m.prefix() == "ahb").then(|| now + 2 * m.clock_period),
            data_end_ps: (m.prefix() == "ahb").then(|| end - u64::from(error) * m.clock_period),
        };
        s.sources[source].pop_front();
        s.queue(&source_queue(m, source), now, s.sources[source].len());
        s.cursor
            .insert(m.bus.clone(), (source + 1) % m.sources.len());
        grant(m, s, index, plan, now)?;
    }
    Ok(())
}
fn generate(m: &PreparedSoc, s: &mut State, now: u64, g: usize, ordinal: usize) -> Result<()> {
    let generator = &m.generators[g];
    if generator.times.get(ordinal) != Some(&now) {
        return Err(Diagnostic::execution("SoC generator cursor invariant"));
    }
    let event = protocol::Event::Generate {
        generator_id: generator.id.clone(),
        ordinal: checked(ordinal as u128)?,
    };
    protocol::Event::decode(
        &m.profile,
        &format!("{}.Generate", m.profile),
        1,
        1,
        &event.encode(),
    )?;
    ordinal.checked_add(1).ok_or_else(overflow)?;
    let source = generator.source;
    let full = if m.prefix() == "noc" {
        s.sources[source].len() as u64 >= m.sources[source].capacity
    } else {
        s.used[source] >= m.sources[source].capacity
    };
    let wake = if full {
        None
    } else {
        Some(edge(now, m.clock_period)?)
    };
    let index = s.journal.transactions.len();
    let target = generator
        .transaction
        .destination
        .map(|d| m.sources[d].node.clone());
    s.journal.transactions.push(Transaction {
        id: format!("{}:{ordinal}", generator.id),
        generator: g,
        time_ps: now,
        generated_ps: now,
        status: if full { "dropped" } else { "pending" }.into(),
        target,
        start_ps: None,
        completed_ps: None,
        response: None,
        drop_reason: full.then(|| "source_full".into()),
        active_plan: None,
        hop: 0,
    });
    if let Some(wake) = wake {
        s.used[source] = s.used[source].checked_add(1).ok_or_else(overflow)?;
        s.unfinished = s.unfinished.checked_add(1).ok_or_else(overflow)?;
        s.sources[source].push_back(index);
        s.queue(&source_queue(m, source), now, s.sources[source].len());
        s.wakes.insert(wake);
    }
    Ok(())
}
fn next_output(m: &PreparedSoc, row: &Transaction, router: usize) -> String {
    let tx = &m.generators[row.generator].transaction;
    let destination = m.sources[tx.destination.unwrap()].router.unwrap();
    let r = &m.routers[router];
    let d = &m.routers[destination];
    let port = if r.x < d.x {
        "out_east"
    } else if r.x > d.x {
        "out_west"
    } else if r.y < d.y {
        "out_north"
    } else if r.y > d.y {
        "out_south"
    } else {
        "local_out"
    };
    format!("{}:{port}", r.node)
}
fn noc_arbitrate(m: &PreparedSoc, s: &mut State, now: u64) -> Result<()> {
    let outputs: Vec<String> = s.journal.busy.keys().cloned().collect();
    // Every successful grant makes one output busy; source-only passes require one final recheck.
    for pass in 0..=outputs.len() + 1 {
        let mut progress = false;
        for source in 0..m.sources.len() {
            let local = format!(
                "{}:local_in",
                m.routers[m.sources[source].router.unwrap()].node
            );
            loop {
                let Some(&index) = s.sources[source].front() else {
                    break;
                };
                if s.journal.transactions[index].generated_ps > now
                    || s.inputs[&local].len() as u64 + s.reserved[&local] >= m.input_capacity
                {
                    break;
                }
                s.sources[source].pop_front();
                s.inputs.get_mut(&local).unwrap().push_back(index);
                s.queue(&source_queue(m, source), now, s.sources[source].len());
                s.queue(&local, now, s.inputs[&local].len());
                progress = true;
            }
        }
        for output in &outputs {
            if s.active.contains_key(output) {
                continue;
            }
            let (router_path, port) = output.rsplit_once(':').unwrap();
            let router = m
                .routers
                .iter()
                .position(|r| r.node == router_path)
                .unwrap();
            let input_prefix = format!("{router_path}:");
            let inputs: Vec<_> = s
                .inputs
                .keys()
                .filter(|id| id.starts_with(&input_prefix))
                .cloned()
                .collect();
            let cursor = *s.cursor.get(output).unwrap_or(&0);
            let downstream = if port == "local_out" {
                None
            } else {
                let to = &m.edges[&format!("{router_path}.{port}")];
                let (owner, gate) = to.rsplit_once('.').unwrap();
                Some(format!("{owner}:{gate}"))
            };
            if downstream
                .as_ref()
                .is_some_and(|d| s.inputs[d].len() as u64 + s.reserved[d] >= m.input_capacity)
            {
                continue;
            }
            let winner = (0..inputs.len())
                .map(|offset| (cursor + offset) % inputs.len())
                .find(|&i| {
                    s.inputs[&inputs[i]].front().is_some_and(|&r| {
                        next_output(m, &s.journal.transactions[r], router) == *output
                            && s.journal.transactions[r].generated_ps <= now
                    })
                });
            if let Some(input) = winner {
                let index = *s.inputs[&inputs[input]].front().unwrap();
                let row = &s.journal.transactions[index];
                let tx = &m.generators[row.generator].transaction;
                let cycles = tx.bytes.div_ceil(m.bytes_per_cycle) + m.link_cycles;
                let end = checked(now as u128 + cycles as u128 * m.clock_period as u128)?;
                let to = if let Some(d) = &downstream {
                    Some(d.rsplit_once(':').unwrap().0.into())
                } else {
                    Some(m.sources[tx.destination.unwrap()].node.clone())
                };
                let plan = ActivePlan {
                    resource: output.clone(),
                    hop: row.hop,
                    start_ps: now,
                    planned_end_ps: end,
                    from: router_path.into(),
                    to,
                    downstream: downstream.clone(),
                    response: downstream.is_none().then(|| "OKAY".into()),
                    address_end_ps: None,
                    nominal_data_end_ps: None,
                    data_end_ps: None,
                };
                s.inputs.get_mut(&inputs[input]).unwrap().pop_front();
                s.queue(&inputs[input], now, s.inputs[&inputs[input]].len());
                if let Some(d) = downstream {
                    *s.reserved.get_mut(&d).unwrap() += 1;
                }
                s.cursor.insert(output.clone(), (input + 1) % inputs.len());
                grant(m, s, index, plan, now)?;
                progress = true;
            }
        }
        if !progress {
            return Ok(());
        }
        if pass == outputs.len() + 1 {
            return Err(Diagnostic::execution("NoC fixed point iteration invariant"));
        }
    }
    Ok(())
}
/// Defensive progress classifier, independent of public topology acceptance.
fn progress(uncompleted: usize, active: usize, future: bool) -> Result<()> {
    if uncompleted > 0 && active == 0 && !future {
        let mut d = Diagnostic::execution("NoC deadlock");
        d.details = Some(serde_json::json!({"reason":"model_failed","operation":"deadlock"}));
        Err(d)
    } else {
        Ok(())
    }
}
pub(super) fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot> {
    let m = prepared
        .soc
        .as_ref()
        .ok_or_else(|| Diagnostic::execution("missing SoC prepared model"))?;
    let mut snapshot = Snapshot::empty(prepared);
    let mut s = State::new(m.sources.len());
    for i in 0..m.sources.len() {
        s.journal.queues.entry(source_queue(m, i)).or_default();
    }
    if m.prefix() == "noc" {
        for (start, end) in &m.edges {
            let (owner, gate) = start.rsplit_once('.').unwrap();
            if m.routers.iter().any(|r| r.node == owner) {
                s.journal.busy.entry(format!("{owner}:{gate}")).or_default();
            }
            let (owner, gate) = end.rsplit_once('.').unwrap();
            if m.routers.iter().any(|r| r.node == owner) {
                let id = format!("{owner}:{gate}");
                s.inputs.entry(id.clone()).or_default();
                s.reserved.insert(id.clone(), 0);
                s.journal.queues.entry(id).or_default();
            }
        }
    } else {
        s.journal.busy.insert(m.bus.clone(), Vec::new());
    }
    // One cursor per generator and one logical dispatcher reservation at the
    // smallest next time; future workload ordinals stay in immutable input.
    let mut generations: BTreeSet<(u64, usize, usize)> = m
        .generators
        .iter()
        .enumerate()
        .filter_map(|(g, generator)| generator.times.first().map(|&time| (time, g, 0)))
        .collect();
    let event_loop = super::timing::event_loop();
    loop {
        let next = [
            generations.first().map(|g| g.0),
            s.completions.keys().next().copied(),
            s.wakes.first().copied(),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(now) = next else {
            break;
        };
        if now >= prepared.common.time_limit_ps {
            break;
        }
        // Completion cohorts commit together before each generation; the
        // dispatcher cursor advances only after that generation commits.
        let mut phase = if s.completions.contains_key(&now) {
            0
        } else {
            1
        };
        let mut failed = false;
        loop {
            let generation = generations.first().copied().filter(|g| g.0 == now);
            if phase == 1 && generation.is_none() {
                phase = 2;
            }
            if phase == 2 && !s.wakes.contains(&now) {
                break;
            }
            let before = s.checkpoint();
            let result = if snapshot.common.committed_events >= prepared.common.max_events {
                Err(overflow().with_reason("event_limit"))
            } else {
                match phase {
                    0 => complete(m, &mut s, now),
                    1 => {
                        let (_, g, ordinal) = generation.unwrap();
                        generate(m, &mut s, now, g, ordinal)
                    }
                    _ => {
                        s.wakes.remove(&now);
                        if m.prefix() == "noc" {
                            noc_arbitrate(m, &mut s, now)
                        } else {
                            bus_arbitrate(m, &mut s, now)
                        }
                    }
                }
            };
            if let Err(d) = result {
                s.rollback(before);
                snapshot.common.partial = true;
                snapshot.common.termination = "execution_failed".into();
                snapshot.common.end_ps = now;
                snapshot
                    .common
                    .diagnostics
                    .push(d.with_runtime("run", Some(now), None, None));
                failed = true;
                break;
            }
            s.commit();
            if phase == 1 {
                let entry = generation.unwrap();
                generations.remove(&entry);
                let ordinal = entry.2 + 1;
                if let Some(&time) = m.generators[entry.1].times.get(ordinal) {
                    generations.insert((time, entry.1, ordinal));
                }
            }
            snapshot.common.committed_events += 1;
            snapshot.common.last_event_time_ps = Some(now);
            if !snapshot.common.checkpoint() {
                failed = true;
                break;
            }
            if phase == 2 {
                break;
            }
            phase = 1;
        }
        if failed {
            break;
        }
        if m.prefix() == "noc" {
            if let Err(d) = progress(
                s.unfinished,
                s.active.len(),
                !generations.is_empty() || !s.wakes.is_empty(),
            ) {
                snapshot.common.partial = true;
                snapshot.common.termination = "execution_failed".into();
                snapshot.common.end_ps = now;
                snapshot
                    .common
                    .diagnostics
                    .push(d.with_runtime("run", Some(now), None, None));
                break;
            }
        }
    }
    drop(event_loop);
    snapshot.common.pending_events =
        (usize::from(!generations.is_empty()) + s.completions.len() + s.wakes.len()) as u64;
    if !snapshot.common.partial {
        snapshot.common.termination =
            if snapshot.common.pending_events > 0 || prepared.common.time_limit_ps == 0 {
                "time_limit"
            } else {
                "events_exhausted"
            }
            .into();
    }
    snapshot.soc = Some(s.journal);
    Ok(snapshot)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_copies_only_touched_rows_and_restores_journal_append_boundaries() {
        let row = Transaction {
            id: "a:0".into(),
            generator: 0,
            time_ps: 0,
            generated_ps: 0,
            status: "pending".into(),
            target: None,
            start_ps: None,
            completed_ps: None,
            response: None,
            drop_reason: None,
            active_plan: None,
            hop: 0,
        };
        let mut state = State::new(1);
        state.journal.transactions = vec![row.clone(); 10_000];
        state.sources[0].push_back(9_999);
        state.used[0] = 1;
        state.unfinished = 1;
        state.queue("source", 0, 1);
        state.journal.busy.insert("bus".into(), Vec::new());
        let before = state.checkpoint();
        assert!(state.undo_rows.is_empty());
        state.transaction_mut(9_999).status = "active".into();
        state.transaction_mut(9_999).time_ps = 10;
        assert_eq!(state.undo_rows.len(), 1);
        state.journal.transactions.push(row);
        state.queue("source", 10, 0);
        state.queue("new", 10, 2);
        state.journal.busy.get_mut("bus").unwrap().push((10, 20));
        state.sources[0].pop_front();
        state.used[0] = 0;
        state.unfinished = 0;
        state.rollback(before);
        assert_eq!(state.journal.transactions.len(), 10_000);
        assert_eq!(state.journal.transactions[9_999].status, "pending");
        assert_eq!(state.journal.transactions[9_999].time_ps, 0);
        assert_eq!(state.journal.queues["source"].changes, vec![(0, 1)]);
        assert_eq!(state.journal.queues["source"].maximum, 1);
        assert!(!state.journal.queues.contains_key("new"));
        assert!(state.journal.busy["bus"].is_empty());
        assert_eq!(state.sources[0].front(), Some(&9_999));
        assert_eq!(state.used[0], 1);
        assert_eq!(state.unfinished, 1);
    }
    #[test]
    fn failed_completion_cohort_restores_rows_transfers_counters_and_reservation() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/verification/fixtures/soc/noc-backpressure.ini");
        let prepared = crate::prepare(&path).unwrap();
        let m = prepared.soc.as_ref().unwrap();
        let mut state = State::new(m.sources.len());
        for (index, resource) in ["Main.r00:local_out", "Main.r10:local_out"]
            .into_iter()
            .enumerate()
        {
            let plan = ActivePlan {
                resource: resource.into(),
                hop: 0,
                start_ps: 0,
                planned_end_ps: 10,
                from: resource.split(':').next().unwrap().into(),
                to: Some("Main.e00".into()),
                downstream: None,
                response: Some("OKAY".into()),
                address_end_ps: None,
                nominal_data_end_ps: None,
                data_end_ps: None,
            };
            state.journal.transactions.push(Transaction {
                id: format!("a:{index}"),
                generator: 0,
                time_ps: 0,
                generated_ps: 0,
                status: "active".into(),
                target: Some("Main.e00".into()),
                start_ps: Some(0),
                completed_ps: None,
                response: None,
                drop_reason: None,
                active_plan: Some(plan),
                hop: 0,
            });
            // The second completion's token deliberately names the wrong owner.
            state.active.insert(resource.into(), 0);
        }
        let source = m.generators[0].source;
        state.used[source] = 2;
        state.unfinished = 2;
        state.completions.insert(10, vec![0, 1]);
        let before = state.checkpoint();
        assert!(complete(m, &mut state, 10).is_err());
        assert_eq!(state.journal.transfers.len(), 1);
        assert_eq!(state.journal.transactions[0].status, "completed");
        state.rollback(before);
        assert!(state.journal.transfers.is_empty());
        assert!(
            state
                .journal
                .transactions
                .iter()
                .all(|r| r.status == "active"
                    && r.completed_ps.is_none()
                    && r.active_plan.is_some())
        );
        assert_eq!(state.used[source], 2);
        assert_eq!(state.unfinished, 2);
        assert_eq!(state.active.len(), 2);
        assert_eq!(state.completions[&10], vec![0, 1]);
        assert!(state.wakes.is_empty());
    }
    #[test]
    fn defensive_deadlock() {
        let d = super::progress(1, 0, false).unwrap_err();
        assert_eq!(d.code, "E-0002");
        assert_eq!(d.details.unwrap()["operation"], "deadlock");
        assert!(super::progress(1, 1, false).is_ok());
        assert!(super::progress(1, 0, true).is_ok());
    }
}
