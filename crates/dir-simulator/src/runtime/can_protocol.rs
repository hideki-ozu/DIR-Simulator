//! Cooperative Classical CAN 2.0 B digital protocol execution (explicit profile).
pub mod codec;
pub mod confinement;
#[path = "../input/can_protocol.rs"]
pub mod input;
mod mixed;
#[path = "../output/can_protocol.rs"]
pub mod output;
pub mod random;
mod time;
#[path = "../lib/types/can_protocol.rs"]
pub mod types;
use crate::registry::*;
use crate::types::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use types::*;
const KEY: &str = "dir.can.ProtocolV1";
const PROFILE: &str = "can.cc.protocol.v1";
fn schema() -> Schema {
    Schema::new("dir.can.ProtocolEvent", 1)
}
fn record_schema() -> Schema {
    Schema::new("dir.can.ProtocolRecord", 1)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
enum Event {
    Generate {
        generator: usize,
        time: u64,
    },
    Bit,
    Recover(usize),
    Cancel(usize),
    ApplicationComplete {
        controller: usize,
        request_id: String,
        attempt: u64,
    },
}
fn event_validate(bytes: &[u8]) -> ModelResult {
    serde_json::from_slice::<Event>(bytes)
        .map(|_| ())
        .map_err(|e| ModelError(e.to_string()))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    controller: usize,
    frame: Frame,
    generated: u64,
    attempt: u64,
    parent: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", deny_unknown_fields)]
enum LocalPhase {
    Idle,
    Frame,
    EofLast,
    Intermission { count: u8 },
    Suspend { remaining: u8 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    counters: confinement::Counters,
    recessive: u8,
    idle_sequences: u16,
    recovery_enabled: bool,
    suspend_until: u64,
    phase: LocalPhase,
    flag: Option<Flag>,
    frame_origin: usize,
    received: bool,
    needs_suspend: bool,
    last_sample: u8,
    application_queue: std::collections::VecDeque<ApplicationItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Flag {
    start: usize,
    active: bool,
    transmitter: bool,
    kind: String,
    dominant_run: u16,
    ack_deferred: bool,
    end: Option<usize>,
    equal_run: u8,
    last: u8,
    delimiter: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Peer {
    request: Request,
    wire: codec::Wire,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Signal {
    controller: usize,
    kind: String,
    start: usize,
    active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    request: Request,
    peers: Vec<Peer>,
    orphaned: bool,
    wire: codec::Wire,
    samples: Vec<Vec<u8>>,
    position: usize,
    observed: Vec<u8>,
    stage: String,
    error_start: Option<u64>,
    overloads: u8,
    sof: u64,
    intermission: u8,
    frame_done: bool,
    failed: bool,
    settled: bool,
    closed: bool,
    history: Vec<Signal>,
    overload_group: u64,
    overload_start: Option<usize>,
    external_origin: Option<usize>,
    rx_ack: Vec<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scheduled {
    token: TimerToken,
    at: u64,
    event: Event,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    initialized: bool,
    cancelled: std::collections::BTreeSet<String>,
    terminal: std::collections::BTreeMap<String, String>,
    scheduled: Vec<Scheduled>,
    nodes: Vec<Node>,
    cursors: Vec<random::Cursor>,
    pending: Vec<Request>,
    active: Option<Attempt>,
    idle_timer: bool,
    idle_origin: Option<u64>,
    idle_bits: u64,
    serial: u64,
    bit_tick: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationItem {
    request_id: String,
    attempt: u64,
    frame: Frame,
}
struct Protocol {
    config: Config,
    workload: Workload,
    state: State,
}
fn bytes<T: Serialize>(v: &T) -> ModelResult<Vec<u8>> {
    serde_json::to_vec(v).map_err(|e| ModelError(e.to_string()))
}
fn factory(c: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload) = input::parse(&c.profile_input)?;
    let nodes = config
        .controllers
        .iter()
        .map(|c| Node {
            counters: confinement::Counters {
                tec: c.tec,
                rec: c.rec,
            },
            recessive: 0,
            idle_sequences: 0,
            recovery_enabled: c.automatic_recovery,
            suspend_until: 0,
            phase: LocalPhase::Idle,
            flag: None,
            frame_origin: 0,
            received: false,
            needs_suspend: false,
            last_sample: 1,
            application_queue: Default::default(),
        })
        .collect();
    let cursors = workload
        .generators
        .iter()
        .map(|g| random::Cursor::new(&g.schedule))
        .collect();
    Ok(Box::new(Protocol {
        config,
        workload,
        state: State {
            initialized: false,
            cancelled: Default::default(),
            terminal: Default::default(),
            scheduled: vec![],
            nodes,
            cursors,
            pending: vec![],
            active: None,
            idle_timer: false,
            idle_origin: None,
            idle_bits: 0,
            serial: 0,
            bit_tick: 0,
        },
    }))
}
impl Protocol {
    fn after_bits(&self, now: u64, n: u64) -> ModelResult<u64> {
        let d = (n as u128 * 1_000_000_000_000).div_ceil(self.config.bitrate as u128);
        u64::try_from(now as u128 + d).map_err(|_| ModelError("CAN timestamp overflow".into()))
    }
    fn schedule(&mut self, c: &mut Context<'_>, at: u64, e: Event) -> ModelResult {
        let token = c.schedule_at(at, schema(), bytes(&e)?)?;
        self.state.scheduled.push(Scheduled {
            token,
            at,
            event: e,
        });
        Ok(())
    }
    fn record(&self, c: &mut Context<'_>, id: String, data: Value) -> ModelResult {
        c.upsert_model_record(ModelRecord {
            schema: record_schema(),
            id,
            subject: c.subject().into(),
            time_ps: c.now(),
            data,
        })
    }
    fn node_record(&self, c: &mut Context<'_>, n: usize) -> ModelResult {
        let node = &self.state.nodes[n];
        self.record(c,format!("controller:{}",self.config.controllers[n].id),json!({"kind":"controller","controller":self.config.controllers[n].id,"tec":node.counters.tec,"rec":node.counters.rec,"state":node.counters.state(),"recovery_idle_sequences":node.idle_sequences}))
    }
    fn next_generation(&mut self, c: &mut Context<'_>, g: usize) -> ModelResult {
        if let Some(time) = self.state.cursors[g].next(&self.workload.generators[g].schedule)? {
            self.schedule(c, time, Event::Generate { generator: g, time })?;
            self.record(c,format!("generator:{}",self.workload.generators[g].id),json!({"kind":"generator","generator":self.workload.generators[g].id,"algorithm":"splitmix64.v1","rng_state":self.state.cursors[g].rng.to_string(),"position":self.state.cursors[g].position,"next_ps":time.to_string()}))?;
        }
        Ok(())
    }
    fn ready(&mut self, c: &mut Context<'_>) -> ModelResult {
        if self.state.active.is_none() {
            c.request_arbitration("bus")?;
        }
        Ok(())
    }
    fn bit_timer(&mut self, c: &mut Context<'_>) -> ModelResult {
        let at = if let Some(a) = &self.state.active {
            self.after_bits(a.sof, a.observed.len() as u64 + 1)?
        } else if let Some(origin) = self.state.idle_origin {
            self.after_bits(origin, self.state.idle_bits + 1)?
        } else {
            self.after_bits(c.now(), 1)?
        };
        self.schedule(c, at, Event::Bit)
    }
    fn observe_samples(&mut self, c: &mut Context<'_>, samples: &[u8]) -> ModelResult {
        for (n, sample) in samples.iter().enumerate() {
            let node = &mut self.state.nodes[n];
            if node.counters.state() != "bus_off" || !node.recovery_enabled {
                continue;
            }
            if *sample == 0 {
                node.recessive = 0;
            } else {
                node.recessive += 1;
                if node.recessive == 11 {
                    node.recessive = 0;
                    node.idle_sequences += 1;
                    if node.idle_sequences == 128 {
                        node.counters.tec = 0;
                        node.counters.rec = 0;
                        node.idle_sequences = 0;
                        node.phase = LocalPhase::Idle;
                        node.flag = None;
                        node.needs_suspend = false;
                        node.suspend_until = 0;
                    }
                }
            }
            self.node_record(c, n)?;
        }
        Ok(())
    }
    fn senders(a: &Attempt) -> Vec<(&Request, &codec::Wire)> {
        std::iter::once((&a.request, &a.wire))
            .filter(|_| !a.orphaned)
            .chain(a.peers.iter().map(|p| (&p.request, &p.wire)))
            .collect()
    }
    fn sender(a: &Attempt, n: usize) -> Option<(&Request, &codec::Wire)> {
        Self::senders(a)
            .into_iter()
            .find(|(r, _)| r.controller == n)
    }
    fn eligible(&self, now: u64) -> ModelResult<Vec<usize>> {
        // One pending message per controller participates. Equal arbitration fields
        // on different controllers remain concurrent transmitters after arbitration.
        let mut candidates = std::collections::BTreeMap::new();
        for (i, r) in self.state.pending.iter().enumerate() {
            if self.state.nodes[r.controller].counters.state() == "bus_off"
                || self.state.nodes[r.controller].suspend_until > now
                || self.state.nodes[r.controller].flag.is_some()
                || self.state.nodes[r.controller].needs_suspend
                || !matches!(
                    self.state.nodes[r.controller].phase,
                    LocalPhase::Idle | LocalPhase::Intermission { count: 2 }
                )
            {
                continue;
            }
            let key = (
                codec::encode(&r.frame)?.arbitration,
                r.generated,
                r.id.clone(),
            );
            let entry = candidates.entry(r.controller).or_insert((i, key.clone()));
            if key < entry.1 {
                *entry = (i, key);
            }
        }
        Ok(candidates.values().map(|(i, _)| *i).collect())
    }

    fn start(
        &mut self,
        c: &mut Context<'_>,
        mut indices: Vec<usize>,
        sof: Option<u64>,
    ) -> ModelResult {
        self.state.idle_origin = None;
        self.state.idle_bits = 0;
        indices.sort_unstable();
        let mut senders = vec![];
        for index in indices.into_iter().rev() {
            let mut request = self.state.pending.remove(index);
            request.attempt = request
                .attempt
                .checked_add(1)
                .ok_or("CAN attempt overflow")?;
            let wire = codec::encode(&request.frame)?;
            self.state.nodes[request.controller].phase = LocalPhase::Frame;
            self.state.nodes[request.controller].frame_origin = 0;
            self.state.nodes[request.controller].received = false;
            let start = sof.unwrap_or(c.now());
            self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"parent_request_id":request.parent,"frame":request.frame,"generated_ps":request.generated.to_string(),"state":"transmitting","attempt":request.attempt}))?;
            c.observe("can.protocol.attempts", c.subject().to_owned().as_str(), 1)?;
            self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"sof_ps":start.to_string(),"state":"transmitting","wire_bits":wire.bits,"crc15":wire.crc}))?;
            senders.push(Peer { request, wire });
        }
        senders.sort_by(|a, b| a.request.id.cmp(&b.request.id));
        let primary = senders.remove(0);
        let count = self.state.nodes.len();
        let already_sof = usize::from(sof.is_some());
        if sof.is_some() {
            for node in &mut self.state.nodes {
                if matches!(node.phase, LocalPhase::Frame) && node.flag.is_none() {
                    node.frame_origin = 0;
                }
            }
        }
        self.state.active = Some(Attempt {
            request: primary.request,
            wire: primary.wire,
            peers: senders,
            orphaned: false,
            samples: vec![vec![0; already_sof]; count],
            position: already_sof,
            observed: vec![0; already_sof],
            stage: "frame".into(),
            error_start: None,
            overloads: 0,
            sof: sof.unwrap_or(c.now()),
            intermission: 0,
            frame_done: false,
            failed: false,
            settled: false,
            closed: false,
            history: vec![],
            overload_group: 0,
            overload_start: None,
            external_origin: None,
            rx_ack: vec![false; count],
        });
        self.bit_timer(c)
    }
    fn participating(&self, n: usize) -> bool {
        self.state.nodes[n].counters.state() != "bus_off" && !self.config.controllers[n].listen_only
    }
    fn signal(
        &mut self,
        c: &mut Context<'_>,
        a: &mut Attempt,
        n: usize,
        kind: String,
        at: usize,
        counted_flag_error: bool,
    ) -> ModelResult {
        let tx = self.state.nodes[n].flag.as_ref().map_or_else(
            || Self::sender(a, n).is_some() && a.external_origin.is_none(),
            |flag| flag.transmitter,
        );
        let active = self.state.nodes[n].counters.state() == "error_active";
        let ack_deferred = tx && !active && kind == "ack";
        if !counted_flag_error {
            if tx && !ack_deferred {
                let (_, wire) = Self::sender(a, n).unwrap();
                let arbitration_end = wire.arbitration.len() + 1;
                let stuff_exception = kind == "stuff"
                    && wire.stuff_positions.contains(&at)
                    && wire.bits.get(at) == Some(&1)
                    && a.samples[n][at] == 0
                    && at - wire.stuff_positions.iter().filter(|&&p| p < at).count()
                        < arbitration_end;
                self.state.nodes[n]
                    .counters
                    .tx_error(false, stuff_exception);
            } else if !tx {
                self.state.nodes[n].counters.rx_error(false);
            }
        }
        let start = if kind == "crc" {
            // Detection occurs on the locally parsed CRC delimiter; ACK and
            // ACK delimiter follow it, independently of the original frame DLC.
            at + 3
        } else {
            at + 1
        };
        self.state.nodes[n].flag = Some(Flag {
            start: self.state.bit_tick + start - at,
            active,
            transmitter: tx,
            kind: kind.clone(),
            dominant_run: 0,
            ack_deferred,
            end: active.then_some(self.state.bit_tick + start - at + 6),
            equal_run: 0,
            last: 2,
            delimiter: 0,
        });
        if !a.frame_done && (tx || active) {
            a.failed = true;
            a.stage = "recovery".into();
        }
        if a.frame_done {
            a.stage = "recovery".into();
        }
        if a.error_start.is_none() {
            a.error_start = Some(c.now());
        }
        a.history.push(Signal {
            controller: n,
            kind: kind.clone(),
            start,
            active,
        });
        self.node_record(c, n)?;
        self.record(c,format!("error:{}:{}:{}:{}",a.request.id,a.request.attempt,self.config.controllers[n].id,a.history.len()),json!({"kind":"error","request_id":a.request.id,"attempt":a.request.attempt,"error":kind,"detector":self.config.controllers[n].id,"detected_ps":c.now().to_string(),"detected_bit":at,"flag_start_bit":start,"flag":if active{"active"}else{"passive"},"flag_bits":6,"state":"signalled"}))
    }
    fn overload(&mut self, a: &mut Attempt, n: usize, start: usize) {
        if a.overload_start.is_none() {
            a.overload_start = Some(start);
            a.overload_group += 1;
        }
        let start_tick = self.state.bit_tick + start - a.position;
        self.state.nodes[n].flag = Some(Flag {
            start: start_tick,
            active: true,
            transmitter: Self::sender(a, n).is_some(),
            kind: "overload".into(),
            dominant_run: 0,
            ack_deferred: false,
            end: Some(start_tick + 6),
            equal_run: 0,
            last: 2,
            delimiter: 0,
        });
        a.stage = "recovery".into();
    }
    fn internal_overload(&mut self, a: &mut Attempt) {
        if a.overloads > 0 {
            if let Some(n) = (0..self.state.nodes.len())
                .find(|&n| Self::sender(a, n).is_none() && self.participating(n))
            {
                a.overloads -= 1;
                self.overload(a, n, a.position);
            } else {
                a.overloads = 0;
            }
        }
    }
    fn lose_arbitration(
        &mut self,
        c: &mut Context<'_>,
        a: &mut Attempt,
        n: usize,
        at: usize,
    ) -> ModelResult {
        let request = if a.request.controller == n {
            let lost = a.request.clone();
            if a.peers.is_empty() {
                a.orphaned = true;
                a.failed = true;
                a.stage = "external".into();
                a.external_origin = Some(0);
            } else {
                let next = a.peers.remove(0);
                a.request = next.request;
                a.wire = next.wire;
            }
            lost
        } else {
            let index = a
                .peers
                .iter()
                .position(|p| p.request.controller == n)
                .ok_or("missing arbitration contender")?;
            a.peers.remove(index).request
        };
        self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"sof_ps":a.sof.to_string(),"state":"arbitration_lost","lost_bit":at,"lost_ps":c.now().to_string(),"wire_bits":a.observed}))?;
        let cancelled = self.state.cancelled.contains(&request.id);
        self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"state":if cancelled {"cancelled"}else{"queued"},"attempt":request.attempt,"cancel_requested":cancelled,"physical_outcome":"arbitration_lost"}))?;
        if cancelled {
            self.state
                .terminal
                .insert(request.id.clone(), "cancelled".into());
            self.cancellation_record(c, &request.id, "acknowledged", "arbitration_lost")?;
        } else {
            self.state.pending.push(request);
        }
        Ok(())
    }
    fn cancellation_record(
        &self,
        c: &mut Context<'_>,
        id: &str,
        state: &str,
        outcome: &str,
    ) -> ModelResult {
        let requested = self
            .config
            .cancellations
            .iter()
            .find(|cancel| cancel.request_id == id)
            .ok_or("missing cancellation selector")?
            .at_ps;
        self.record(c,format!("cancellation:{id}"),json!({"kind":"cancellation","request_id":id,"state":state,"requested_ps":requested.to_string(),"observed_ps":c.now().to_string(),"physical_outcome":outcome,"retry_suppressed":state!="too_late"}))
    }
    fn cancel(&mut self, c: &mut Context<'_>, index: usize) -> ModelResult {
        let cancel = self
            .config
            .cancellations
            .get(index)
            .ok_or("cancellation index out of bounds")?;
        if cancel.at_ps != c.now() {
            return Err("cancellation time mismatch".into());
        }
        let id = cancel.request_id.clone();
        if !self.state.cancelled.insert(id.clone()) {
            return Err("duplicate cancellation event".into());
        }
        if self.state.terminal.contains_key(&id) {
            return self.cancellation_record(c, &id, "too_late", "completed");
        }
        if let Some((request, failed)) = self.state.active.as_ref().and_then(|a| {
            Self::senders(a)
                .into_iter()
                .find(|(r, _)| r.id == id)
                .map(|(r, _)| (r.clone(), a.failed && a.settled))
        }) {
            self.state.pending.retain(|r| r.id != id);
            if failed {
                self.state.terminal.insert(id.clone(), "cancelled".into());
            }
            self.record(c,format!("request:{id}"),json!({"kind":"request","request_id":id,"frame":request.frame,"generated_ps":request.generated.to_string(),"attempt":request.attempt,"state":if failed {"cancelled"} else {"cancel_requested"},"cancel_requested":true,"physical_outcome":if failed {"failed"} else {"pending"}}))?;
            return self.cancellation_record(
                c,
                &id,
                "acknowledged",
                if failed { "failed" } else { "pending" },
            );
        }
        if let Some(index) = self.state.pending.iter().position(|r| r.id == id) {
            let r = self.state.pending.remove(index);
            self.state.terminal.insert(id.clone(), "cancelled".into());
            self.record(c,format!("request:{id}"),json!({"kind":"request","request_id":id,"frame":r.frame,"generated_ps":r.generated.to_string(),"attempt":r.attempt,"state":"cancelled","cancel_requested":true,"physical_outcome":"not_started"}))?;
            self.cancellation_record(c, &id, "acknowledged", "not_started")
        } else if let Some(a) = &self.state.active {
            if let Some((r, _)) = Self::senders(a).into_iter().find(|(r, _)| r.id == id) {
                self.record(c,format!("request:{id}"),json!({"kind":"request","request_id":id,"frame":r.frame,"generated_ps":r.generated.to_string(),"attempt":r.attempt,"state":"cancel_requested","cancel_requested":true,"physical_outcome":"pending"}))?;
                self.cancellation_record(c, &id, "acknowledged", "pending")
            } else {
                self.cancellation_record(c, &id, "waiting_for_request", "not_generated")
            }
        } else {
            self.cancellation_record(c, &id, "waiting_for_request", "not_generated")
        }
    }
    fn settle_outcome(&mut self, c: &mut Context<'_>, a: &mut Attempt) -> ModelResult {
        if a.settled {
            return Ok(());
        }
        a.settled = true;
        if a.failed && !a.frame_done {
            for (request, _) in Self::senders(a) {
                c.observe("can.protocol.failures", c.subject().to_owned().as_str(), 1)?;
                self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"sof_ps":a.sof.to_string(),"error_ps":a.error_start.map(|x|x.to_string()),"release_ps":Value::Null,"state":"failed","errors":a.history.iter().map(|s|json!({"controller":self.config.controllers[s.controller].id,"kind":s.kind,"flag_start_bit":s.start,"active":s.active})).collect::<Vec<_>>(),"wire_bits":a.observed}))?;
                let cancelled = self.state.cancelled.contains(&request.id);
                self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"state":if cancelled {"cancelled"}else{"retry_pending"},"attempt":request.attempt,"cancel_requested":cancelled,"physical_outcome":"failed"}))?;
                if cancelled {
                    self.state
                        .terminal
                        .insert(request.id.clone(), "cancelled".into());
                    self.cancellation_record(c, &request.id, "acknowledged", "failed")?;
                } else {
                    self.state.pending.push(request.clone());
                }
            }
        }
        Ok(())
    }
    fn enter_intermission(&mut self, a: &Attempt, n: usize) {
        self.state.nodes[n].phase = LocalPhase::Intermission { count: 0 };
        self.state.nodes[n].needs_suspend =
            Self::sender(a, n).is_some() && self.state.nodes[n].counters.state() == "error_passive";
    }
    fn close_occupation(
        &mut self,
        c: &mut Context<'_>,
        a: &mut Attempt,
        release: u64,
        exclusive: usize,
    ) -> ModelResult {
        if a.closed {
            return Ok(());
        }
        self.settle_outcome(c, a)?;
        a.closed = true;
        if a.failed && !a.frame_done {
            for (request, _) in Self::senders(a) {
                self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"sof_ps":a.sof.to_string(),"error_ps":a.error_start.map(|x|x.to_string()),"release_ps":release.to_string(),"state":"failed","errors":a.history.iter().map(|s|json!({"controller":self.config.controllers[s.controller].id,"kind":s.kind,"flag_start_bit":s.start,"active":s.active})).collect::<Vec<_>>(),"wire_bits":&a.observed[..exclusive]}))?;
            }
        }
        Ok(())
    }
    fn frame_bit(&mut self, c: &mut Context<'_>, a: &mut Attempt) -> ModelResult<bool> {
        let at = a.position;
        let tick = self.state.bit_tick;
        let mut adopted = Vec::new();
        let faults: Vec<_> = self
            .config
            .faults
            .iter()
            .filter(|f| {
                Self::senders(a).iter().any(|(r, w)| {
                    f.request_id == r.id
                        && f.attempt == r.attempt
                        && if f.kind == "ack" {
                            at == w.ack
                        } else {
                            f.bit == Some(at)
                        }
                })
            })
            .cloned()
            .collect();
        let mut bus = 1;
        let mut ack_driven = vec![false; self.state.nodes.len()];
        if !a.frame_done && a.external_origin.is_none() {
            for (request, wire) in Self::senders(a) {
                let n = request.controller;
                if self.participating(n)
                    && self.state.nodes[n]
                        .flag
                        .as_ref()
                        .is_none_or(|f| tick < f.start)
                    && at != wire.ack
                {
                    bus &= wire.bits.get(at).copied().unwrap_or(1);
                }
            }
        }
        for (n, driven) in ack_driven.iter_mut().enumerate() {
            if !self.participating(n) {
                continue;
            }
            if self.state.nodes[n]
                .flag
                .as_ref()
                .is_some_and(|f| f.active && (f.start..f.start + 6).contains(&tick))
            {
                bus = 0;
            }
            if !a.frame_done
                && a.external_origin.is_none()
                && Self::sender(a, n).is_none()
                && self.state.nodes[n].flag.is_none()
                && self.state.nodes[n].frame_origin <= a.samples[n].len()
                && codec::acknowledgement_ready(&a.samples[n][self.state.nodes[n].frame_origin..])
                && !faults
                    .iter()
                    .any(|f| f.kind == "ack" && f.receiver.is_none())
                && self.state.nodes[n].flag.is_none()
            {
                bus = 0;
                a.rx_ack[n] = true;
                *driven = true;
            }
        }
        for f in &faults {
            if f.receiver.is_none() {
                if f.kind == "ack" {
                    bus = 1;
                } else {
                    bus ^= 1;
                }
            }
        }
        a.observed.push(bus);
        for n in 0..self.state.nodes.len() {
            let mut sample = bus;
            for f in &faults {
                if f.receiver.as_deref() == Some(self.config.controllers[n].id.as_str()) {
                    if f.kind == "ack" {
                        sample = 1;
                    } else {
                        sample ^= 1;
                    }
                }
            }
            a.samples[n].push(sample);
        }
        // Recovery sees each controller's actual sample, including local faults.
        self.observe_samples(c, &a.samples.iter().map(|s| s[at]).collect::<Vec<_>>())?;
        for (n, driven) in ack_driven.iter().enumerate() {
            let sample = a.samples[n][at];
            let tx = Self::sender(a, n).is_some() && a.external_origin.is_none();
            let participating = self.participating(n);
            if let Some(mut flag) = self.state.nodes[n].flag.take() {
                let tx = flag.transmitter;
                let mut restart = None;
                if tick >= flag.start {
                    let inside = flag.end.is_none_or(|end| tick < end);
                    if participating && flag.active && tick < flag.start + 6 && sample != 0 {
                        self.state.nodes[n].counters.flag_bit_error(tx);
                        self.node_record(c, n)?;
                        restart = Some(("bit", true));
                    }
                    if participating
                        && flag.kind != "overload"
                        && flag.end == Some(tick)
                        && sample == 0
                        && !tx
                    {
                        self.state.nodes[n].counters.dominant_after_flag();
                        self.node_record(c, n)?;
                    }
                    // Active/overload threshold includes the six driven flag bits;
                    // passive threshold starts AFTER its six equal observed bits.
                    if flag.active || !inside {
                        flag.dominant_run = if sample == 0 {
                            flag.dominant_run.saturating_add(1)
                        } else {
                            0
                        };
                        let threshold = if flag.active { 14 } else { 8 };
                        if participating
                            && flag.dominant_run >= threshold
                            && (flag.dominant_run - threshold) % 8 == 0
                        {
                            self.state.nodes[n].counters.dominant_extension(tx);
                            self.node_record(c, n)?;
                        }
                    }
                    if !flag.active && flag.end.is_none() {
                        flag.equal_run = if flag.last == sample {
                            flag.equal_run + 1
                        } else {
                            1
                        };
                        flag.last = sample;
                        if flag.equal_run == 6 {
                            flag.end = Some(tick + 1);
                        }
                    }
                    if participating && flag.ack_deferred && sample == 0 && inside {
                        flag.ack_deferred = false;
                        self.state.nodes[n].counters.tx_error(false, false);
                        self.node_record(c, n)?;
                    }
                    if flag.end.is_some_and(|end| tick >= end) {
                        if flag.delimiter == 0 {
                            if sample == 1 {
                                flag.delimiter = 1;
                            }
                        } else if flag.delimiter < 8 {
                            if sample == 1 {
                                flag.delimiter += 1;
                            } else if participating {
                                if flag.delimiter == 7 {
                                    restart = Some(("overload", false));
                                } else {
                                    restart = Some(("form", false));
                                }
                            }
                        }
                    }
                }
                let done = flag.delimiter == 8;
                self.state.nodes[n].flag = (!done).then_some(flag);
                if done {
                    self.enter_intermission(a, n);
                }
                if let Some((kind, counted)) = restart {
                    if kind == "overload" {
                        // The last delimiter bit terminates the preceding overload
                        // even when dominant; the next flag is a distinct frame.
                        if let Some(start) = a.overload_start.filter(|start| *start <= at) {
                            self.record(c,format!("overload:{}:{}:{}",a.request.id,a.request.attempt,a.overload_group),json!({"kind":"overload","request_id":a.request.id,"attempt":a.request.attempt,"state":"completed","start_bit":start,"end_bit":at+1,"wire_bits":&a.observed[start..],"delimiter_bits":8,"end_ps":c.now().to_string()}))?;
                            a.overload_start = None;
                        }
                        self.overload(a, n, at + 1);
                    } else {
                        self.signal(c, a, n, kind.into(), at, counted)?;
                    }
                }
                continue;
            }
            if !participating {
                continue;
            }
            match self.state.nodes[n].phase.clone() {
                LocalPhase::EofLast => {
                    if sample == 0 {
                        self.overload(a, n, at + 1);
                    } else {
                        self.enter_intermission(a, n);
                    }
                    continue;
                }
                LocalPhase::Intermission { count } => {
                    if sample == 0 && count < 2 {
                        self.overload(a, n, at + 1);
                    } else if sample == 0 {
                        self.state.nodes[n].phase = LocalPhase::Frame;
                        self.state.nodes[n].frame_origin = at;
                        self.state.nodes[n].received = false;
                        // Eligibility is evaluated before clearing the previous passive sender obligation.
                        adopted.push((n, !self.state.nodes[n].needs_suspend));
                    } else if count == 2 {
                        if self.state.nodes[n].needs_suspend {
                            self.state.nodes[n].phase = LocalPhase::Suspend { remaining: 8 };
                            self.state.nodes[n].suspend_until = self.after_bits(c.now(), 8)?;
                        } else {
                            self.state.nodes[n].phase = LocalPhase::Idle;
                        }
                    } else {
                        self.state.nodes[n].phase = LocalPhase::Intermission { count: count + 1 };
                    }
                    continue;
                }
                LocalPhase::Suspend { remaining } => {
                    if sample == 0 {
                        self.state.nodes[n].phase = LocalPhase::Frame;
                        self.state.nodes[n].frame_origin = at;
                        self.state.nodes[n].received = false;
                        self.state.nodes[n].needs_suspend = false;
                        self.state.nodes[n].suspend_until = 0;
                    } else if remaining == 1 {
                        self.state.nodes[n].phase = LocalPhase::Idle;
                        self.state.nodes[n].needs_suspend = false;
                        self.state.nodes[n].suspend_until = 0;
                    } else {
                        self.state.nodes[n].phase = LocalPhase::Suspend {
                            remaining: remaining - 1,
                        };
                    }
                    continue;
                }
                LocalPhase::Idle => {
                    if sample == 0 {
                        self.state.nodes[n].phase = LocalPhase::Frame;
                        self.state.nodes[n].frame_origin = at;
                        self.state.nodes[n].received = false;
                    } else {
                        continue;
                    }
                }
                LocalPhase::Frame => {}
            }
            if a.external_origin.is_none() && tx {
                let (_, wire) = Self::sender(a, n).unwrap();
                let wire = wire.clone();
                if at == wire.ack && sample == 1 {
                    self.signal(c, a, n, "ack".into(), at, false)?;
                } else if at < wire.bits.len() {
                    let expected = if at == wire.ack { 1 } else { wire.bits[at] };
                    let arbitration = at < wire.crc_delimiter
                        && at - wire.stuff_positions.iter().filter(|&&p| p < at).count()
                            < wire.arbitration.len() + 1;
                    if expected == 1
                        && sample == 0
                        && arbitration
                        && !wire.stuff_positions.contains(&at)
                    {
                        self.lose_arbitration(c, a, n, at)?;
                    } else if sample != expected
                        && !(expected == 1 && sample == 0 && (arbitration || at == wire.ack))
                    {
                        self.signal(c, a, n, "bit".into(), at, false)?;
                    } else if let Err(d) = codec::inspect(&a.samples[n]) {
                        if d.kind == "stuff" {
                            self.signal(c, a, n, d.kind, at, false)?;
                        }
                    }
                }
            } else {
                let origin = self.state.nodes[n].frame_origin;
                if let Err(d) = codec::inspect(&a.samples[n][origin..]) {
                    self.signal(c, a, n, d.kind, at, false)?;
                } else if let Some(frame) = codec::receiver_valid(&a.samples[n][origin..])? {
                    self.receive_valid(c, a, n, &frame)?;
                    self.state.nodes[n].phase = LocalPhase::EofLast;
                } else if *driven {
                    // ACK receivers monitor their driven dominant bit too. An ACK
                    // suppression fault inhibits driving; a bit corruption does not.
                    if sample == 0 {
                        self.state.nodes[n].counters.rx_success();
                        self.node_record(c, n)?;
                    } else {
                        self.signal(c, a, n, "bit".into(), at, false)?;
                    }
                }
            }
        }
        a.position += 1;
        self.state.bit_tick = self
            .state
            .bit_tick
            .checked_add(1)
            .ok_or("CAN bit cursor overflow")?;
        if a.failed {
            self.settle_outcome(c, a)?;
        }
        if !a.failed && !a.frame_done && a.position == a.wire.bits.len() {
            self.complete(c, a)?;
            return Ok(false);
        }
        if let Some(start) = a.overload_start {
            if !self
                .state
                .nodes
                .iter()
                .any(|n| n.flag.as_ref().is_some_and(|f| f.kind == "overload"))
            {
                self.record(c,format!("overload:{}:{}:{}",a.request.id,a.request.attempt,a.overload_group),json!({"kind":"overload","request_id":a.request.id,"attempt":a.request.attempt,"state":"completed","start_bit":start,"end_bit":a.position,"wire_bits":&a.observed[start..],"delimiter_bits":8,"end_ps":c.now().to_string()}))?;
                a.overload_start = None;
                self.internal_overload(a);
            }
        }
        if !adopted.is_empty() {
            let mut eligible = self.eligible_local_sof(&adopted)?;
            let sof = self.after_bits(a.sof, at as u64)?;
            for (n, _) in &adopted {
                self.record(c, format!("local-sof:{}:{}:{}:{}", a.request.id, a.request.attempt, self.config.controllers[*n].id, at), json!({"kind":"local_sof","controller":self.config.controllers[*n].id,"request_id":a.request.id,"attempt":a.request.attempt,"sof_ps":sof.to_string(),"observed_ps":c.now().to_string(),"physical_bit":at,"resolved_level":bus,"sample":a.samples[*n][at]}))?;
            }
            self.close_occupation(c, a, sof, at)?;
            if !eligible.is_empty() {
                let samples: Vec<_> = a.samples.iter().map(|s| s[at]).collect();
                self.start(c, std::mem::take(&mut eligible), Some(sof))?;
                let next = self.state.active.as_mut().unwrap();
                next.observed[0] = bus;
                for (n, sample) in samples.into_iter().enumerate() {
                    next.samples[n][0] = sample;
                }
                return Ok(true);
            }
            for (n, _) in adopted {
                self.state.nodes[n].needs_suspend = false;
                self.state.nodes[n].suspend_until = 0;
            }
            a.external_origin = Some(at);
            a.stage = "external".into();
        }
        if self.state.nodes.iter().enumerate().all(|(index, n)| {
            n.flag.is_none() && matches!(n.phase, LocalPhase::Idle | LocalPhase::Suspend { .. })
                || n.counters.state() == "bus_off"
                || self.config.controllers[index].listen_only
        }) {
            self.close_occupation(c, a, c.now(), a.position)?;
            return Ok(true);
        }
        if a.failed || a.frame_done {
            a.stage = if self.state.nodes.iter().any(|n| n.flag.is_some()) {
                "recovery"
            } else {
                "intermission"
            }
            .into();
        }
        Ok(false)
    }
    fn eligible_local_sof(&self, adopted: &[(usize, bool)]) -> ModelResult<Vec<usize>> {
        let mut candidates = std::collections::BTreeMap::new();
        for (i, r) in self.state.pending.iter().enumerate() {
            if !adopted.contains(&(r.controller, true))
                || self.state.nodes[r.controller].counters.state() == "bus_off"
            {
                continue;
            }
            let key = (
                codec::encode(&r.frame)?.arbitration,
                r.generated,
                r.id.clone(),
            );
            let entry = candidates.entry(r.controller).or_insert((i, key.clone()));
            if key < entry.1 {
                *entry = (i, key);
            }
        }
        Ok(candidates.values().map(|(i, _)| *i).collect())
    }
    fn complete(&mut self, c: &mut Context<'_>, a: &mut Attempt) -> ModelResult {
        let request_ids: Vec<_> = Self::senders(a).iter().map(|(r, _)| r.id.clone()).collect();
        for (request, wire) in Self::senders(a) {
            self.state
                .terminal
                .insert(request.id.clone(), "completed".into());
            let cancelled = self.state.cancelled.contains(&request.id);
            if cancelled {
                self.cancellation_record(c, &request.id, "acknowledged", "completed")?;
            }
            self.state.nodes[request.controller].counters.tx_success();
            self.node_record(c, request.controller)?;
            self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"parent_request_id":request.parent,"frame":request.frame,"generated_ps":request.generated.to_string(),"state":"completed","attempt":request.attempt,"eof_ps":c.now().to_string(),"cancel_requested":cancelled,"physical_outcome":"completed"}))?;
            self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"sof_ps":a.sof.to_string(),"eof_ps":c.now().to_string(),"state":"completed","wire_bits":a.observed,"crc15":wire.crc,"co_transmitted_requests":request_ids}))?;
        }
        a.frame_done = true;
        a.stage = "intermission".into();
        a.intermission = 0;
        for n in 0..self.state.nodes.len() {
            if self.state.nodes[n].flag.is_none()
                && matches!(self.state.nodes[n].phase, LocalPhase::Frame)
            {
                self.enter_intermission(a, n);
            }
        }
        a.overloads = self
            .config
            .overloads
            .iter()
            .filter(|o| request_ids.contains(&o.request_id))
            .map(|o| o.count)
            .max()
            .unwrap_or(0);
        self.internal_overload(a);
        Ok(())
    }
    fn receive_valid(
        &mut self,
        c: &mut Context<'_>,
        a: &Attempt,
        n: usize,
        frame: &Frame,
    ) -> ModelResult {
        if self.state.nodes[n].received {
            return Ok(());
        }
        self.state.nodes[n].received = true;
        let request_ids: Vec<_> = Self::senders(a).iter().map(|(r, _)| r.id.clone()).collect();
        c.observe(
            "can.protocol.receptions",
            c.subject().to_owned().as_str(),
            1,
        )?;
        self.record(c,format!("reception:{}:{}:{}",a.request.id,a.request.attempt,self.config.controllers[n].id),json!({"kind":"reception","request_id":a.request.id,"request_ids":request_ids,"attempt":a.request.attempt,"receiver":self.config.controllers[n].id,"frame":frame,"state":"received"}))?;
        self.application_admit(c, n, &a.request.id, a.request.attempt, frame)?;
        if frame.remote {
            for response in self.config.controllers[n]
                .responders
                .clone()
                .into_iter()
                .filter(|f| f.id == frame.id && f.extended == frame.extended && f.dlc == frame.dlc)
            {
                self.state.serial = self
                    .state
                    .serial
                    .checked_add(1)
                    .ok_or("CAN response sequence overflow")?;
                self.state.pending.push(Request {
                    id: format!("response:{}:{}", a.request.id, self.state.serial),
                    controller: n,
                    frame: response,
                    generated: c.now(),
                    attempt: 0,
                    parent: Some(a.request.id.clone()),
                });
            }
        }
        Ok(())
    }
    fn application_admit(
        &mut self,
        c: &mut Context<'_>,
        n: usize,
        request: &str,
        attempt: u64,
        frame: &Frame,
    ) -> ModelResult {
        let Some(policy) = self.config.controllers[n].application_rx.clone() else {
            return Ok(());
        };
        let admitted = self.state.nodes[n].application_queue.len() < policy.capacity;
        self.record(c,format!("application:{request}:{attempt}:{}",self.config.controllers[n].id),json!({"kind":"application","request_id":request,"attempt":attempt,"receiver":self.config.controllers[n].id,"state":if admitted{"queued"}else{"dropped"},"reason":if admitted{Value::Null}else{json!("rx_fifo_full")},"admitted_ps":c.now().to_string()}))?;
        if admitted {
            let was_empty = self.state.nodes[n].application_queue.is_empty();
            self.state.nodes[n]
                .application_queue
                .push_back(ApplicationItem {
                    request_id: request.into(),
                    attempt,
                    frame: frame.clone(),
                });
            if was_empty {
                let at = c
                    .now()
                    .checked_add(policy.service_ps)
                    .ok_or("application service time overflow")?;
                self.schedule(
                    c,
                    at,
                    Event::ApplicationComplete {
                        controller: n,
                        request_id: request.into(),
                        attempt,
                    },
                )?;
            }
        }
        Ok(())
    }
    fn application_complete(
        &mut self,
        c: &mut Context<'_>,
        n: usize,
        id: &str,
        attempt: u64,
    ) -> ModelResult {
        let node = self
            .state
            .nodes
            .get_mut(n)
            .ok_or("application controller out of bounds")?;
        let item = node
            .application_queue
            .pop_front()
            .ok_or("empty application service queue")?;
        if item.request_id != id || item.attempt != attempt {
            return Err("application service lineage mismatch".into());
        }
        self.record(c,format!("application:{id}:{attempt}:{}",self.config.controllers[n].id),json!({"kind":"application","request_id":id,"attempt":attempt,"receiver":self.config.controllers[n].id,"state":"completed","frame":item.frame,"completed_ps":c.now().to_string()}))?;
        if let Some(next) = self.state.nodes[n].application_queue.front() {
            let policy = self.config.controllers[n]
                .application_rx
                .as_ref()
                .ok_or("application policy missing")?;
            let at = c
                .now()
                .checked_add(policy.service_ps)
                .ok_or("application service time overflow")?;
            self.schedule(
                c,
                at,
                Event::ApplicationComplete {
                    controller: n,
                    request_id: next.request_id.clone(),
                    attempt: next.attempt,
                },
            )?;
        }
        Ok(())
    }
    fn bit(&mut self, c: &mut Context<'_>) -> ModelResult {
        if let Some(mut a) = self.state.active.take() {
            if self.frame_bit(c, &mut a)? {
                if self.state.active.is_none() {
                    self.ready(c)?;
                }
            } else {
                self.state.active = Some(a);
                self.bit_timer(c)?;
            }
        } else {
            self.state.idle_timer = false;
            self.state.idle_bits = self
                .state
                .idle_bits
                .checked_add(1)
                .ok_or("idle bit counter overflow")?;
            self.observe_samples(c, &vec![1; self.state.nodes.len()])?;
            self.state.bit_tick = self
                .state
                .bit_tick
                .checked_add(1)
                .ok_or("CAN bit cursor overflow")?;
            for node in &mut self.state.nodes {
                if let LocalPhase::Suspend { remaining } = node.phase {
                    node.phase = if remaining <= 1 {
                        node.needs_suspend = false;
                        node.suspend_until = 0;
                        LocalPhase::Idle
                    } else {
                        LocalPhase::Suspend {
                            remaining: remaining - 1,
                        }
                    };
                }
            }
            self.ready(c)?;
        }
        Ok(())
    }
}
impl Model for Protocol {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        self.state.initialized = true;
        for n in 0..self.state.nodes.len() {
            self.node_record(c, n)?;
            if let Some(at) = self.config.controllers[n].recover_at_ps {
                self.schedule(c, at, Event::Recover(n))?;
            }
        }
        for index in 0..self.config.cancellations.len() {
            self.schedule(
                c,
                self.config.cancellations[index].at_ps,
                Event::Cancel(index),
            )?;
        }
        for g in 0..self.workload.generators.len() {
            self.next_generation(c, g)?;
        }
        self.ready(c)
    }
    fn on_event(&mut self, event: &Envelope, c: &mut Context<'_>) -> ModelResult {
        let checkpoint = self.state.clone();
        let result = (|| {
            let decoded: Event =
                serde_json::from_slice(&event.payload).map_err(|e| ModelError(e.to_string()))?;
            let index = self
                .state
                .scheduled
                .iter()
                .position(|t| t.token.id == event.event_id)
                .ok_or("CAN event has no live owned timer")?;
            let scheduled = self.state.scheduled.remove(index);
            if scheduled.at != c.now() || scheduled.event != decoded {
                return Err("CAN timer payload or deadline mismatch".into());
            }
            match decoded {
                Event::Generate { generator: g, time } => {
                    let generator = self
                        .workload
                        .generators
                        .get(g)
                        .ok_or("generator out of bounds")?;
                    if time != c.now() {
                        return Err("generator time mismatch".into());
                    }
                    let cursor = &mut self.state.cursors[g];
                    let ordinal = cursor.ordinal;
                    cursor.ordinal += 1;
                    let controller = self
                        .config
                        .controllers
                        .iter()
                        .position(|n| n.id == generator.controller)
                        .unwrap();
                    let request = Request {
                        id: format!("{}:{ordinal}", generator.id),
                        controller,
                        frame: generator.frame.clone(),
                        generated: time,
                        attempt: 0,
                        parent: None,
                    };
                    self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"generated_ps":time.to_string(),"state":"queued","rng_algorithm":"splitmix64.v1","rng_state":self.state.cursors[g].rng.to_string()}))?;
                    if self.state.cancelled.contains(&request.id) {
                        self.state
                            .terminal
                            .insert(request.id.clone(), "cancelled".into());
                        self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"generated_ps":time.to_string(),"attempt":0,"state":"cancelled","cancel_requested":true,"physical_outcome":"not_started"}))?;
                        self.cancellation_record(c, &request.id, "acknowledged", "not_started")?;
                    } else {
                        self.state.pending.push(request);
                    }
                    self.next_generation(c, g)?;
                    self.ready(c)
                }
                Event::Bit => self.bit(c),
                Event::Cancel(index) => self.cancel(c, index),
                Event::ApplicationComplete {
                    controller,
                    request_id,
                    attempt,
                } => self.application_complete(c, controller, &request_id, attempt),
                Event::Recover(n) => {
                    if n >= self.state.nodes.len() {
                        return Err("recovery controller out of bounds".into());
                    }
                    self.state.nodes[n].recovery_enabled = true;
                    self.state.nodes[n].recessive = 0;
                    self.state.nodes[n].idle_sequences = 0;
                    self.ready(c)
                }
            }
        })();
        if result.is_err() {
            self.state = checkpoint;
        }
        result
    }
    fn on_arbitration(&mut self, _: &str, c: &mut Context<'_>) -> ModelResult {
        let checkpoint = self.state.clone();
        let result = (|| {
            if self.state.active.is_some() || self.state.idle_timer {
                return Ok(());
            }
            let eligible = self.eligible(c.now())?;
            if !eligible.is_empty() {
                self.start(c, eligible, None)
            } else if !self.state.idle_timer
                && self.state.nodes.iter().any(|n| {
                    n.recovery_enabled && n.counters.state() == "bus_off"
                        || n.suspend_until > c.now()
                })
            {
                self.state.idle_timer = true;
                if self.state.idle_origin.is_none() {
                    self.state.idle_origin = Some(c.now());
                    self.state.idle_bits = 0;
                }
                self.bit_timer(c)
            } else {
                Ok(())
            }
        })();
        if result.is_err() {
            self.state = checkpoint;
        }
        result
    }
    fn checkpoint(&self, context: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        validate_state(&self.state, &self.config, &self.workload)?;
        for timer in &self.state.scheduled {
            if context.encode_timer_ref(&timer.token)?.state != "live" {
                return Err("CAN scheduled timer is retired".into());
            }
        }
        if context.live_timers.len() != self.state.scheduled.len() {
            return Err("CAN timer ownership is incomplete".into());
        }
        bytes(&self.state)
    }
}
fn validate_state(state: &State, config: &Config, workload: &Workload) -> ModelResult {
    let bad = || ModelError("CAN checkpoint state invariant".into());
    if state.nodes.len() != config.controllers.len()
        || state.cursors.len() != workload.generators.len()
        || state.serial == u64::MAX
        || state.idle_bits == u64::MAX
        || state.idle_timer && state.idle_origin.is_none()
        || state.idle_timer && state.active.is_some()
    {
        return Err(bad());
    }
    for (n, node) in state.nodes.iter().enumerate() {
        if node.counters.tec > 256
            || node.counters.rec > 255
            || node.recessive >= 11
            || node.idle_sequences >= 128
            || matches!(node.phase, LocalPhase::Intermission { count } if count > 2)
            || matches!(node.phase, LocalPhase::Suspend { remaining } if remaining == 0 || remaining > 8)
            || node.last_sample > 1
            || node.application_queue.len()
                > config.controllers[n]
                    .application_rx
                    .as_ref()
                    .map_or(0, |p| p.capacity)
        {
            return Err(bad());
        }
        for item in &node.application_queue {
            codec::encode(&item.frame)?;
            if item.attempt == 0 || item.request_id.is_empty() {
                return Err(bad());
            }
        }
    }
    for (cursor, generator) in state.cursors.iter().zip(&workload.generators) {
        let maximum = match &generator.schedule {
            Schedule::Explicit { times_ps } => times_ps.len() as u64,
            Schedule::Uniform { count, .. }
            | Schedule::Bernoulli { count, .. }
            | Schedule::Geometric { count, .. } => *count,
        };
        if cursor.position > maximum || cursor.ordinal > cursor.position {
            return Err(bad());
        }
    }
    let mut timer_ids = std::collections::BTreeSet::new();
    let mut bits = 0;
    let mut generators = std::collections::BTreeSet::new();
    let mut services = std::collections::BTreeSet::new();
    let mut recoveries = std::collections::BTreeSet::new();
    let mut cancellations = std::collections::BTreeSet::new();
    for timer in &state.scheduled {
        if !timer_ids.insert(&timer.token) {
            return Err(bad());
        }
        match &timer.event {
            Event::Bit => {
                bits += 1;
                let (origin, count) = if let Some(a) = &state.active {
                    (a.sof, a.observed.len() as u64 + 1)
                } else {
                    (state.idle_origin.ok_or_else(bad)?, state.idle_bits + 1)
                };
                let expected = origin as u128
                    + (count as u128 * 1_000_000_000_000).div_ceil(config.bitrate as u128);
                if timer.at as u128 != expected {
                    return Err(bad());
                }
            }
            Event::Generate { generator, time } => {
                if *generator >= workload.generators.len()
                    || !generators.insert(*generator)
                    || timer.at != *time
                {
                    return Err(bad());
                }
                let cursor = &state.cursors[*generator];
                let expected = match &workload.generators[*generator].schedule {
                    Schedule::Explicit { times_ps } => {
                        times_ps.get(cursor.ordinal as usize).copied()
                    }
                    Schedule::Uniform { .. } | Schedule::Geometric { .. } => Some(cursor.time),
                    Schedule::Bernoulli { tick_ps, .. } => cursor.time.checked_sub(*tick_ps),
                };
                if expected != Some(*time) || cursor.ordinal >= cursor.position {
                    return Err(bad());
                }
            }
            Event::ApplicationComplete {
                controller,
                request_id,
                attempt,
            } => {
                if *controller >= state.nodes.len() || !services.insert(*controller) {
                    return Err(bad());
                }
                let front = state.nodes[*controller]
                    .application_queue
                    .front()
                    .ok_or_else(bad)?;
                if front.request_id != *request_id || front.attempt != *attempt {
                    return Err(bad());
                }
            }
            Event::Cancel(index) => {
                let cancel = config.cancellations.get(*index).ok_or_else(bad)?;
                if !cancellations.insert(cancel.request_id.clone())
                    || state.cancelled.contains(&cancel.request_id)
                    || cancel.at_ps != timer.at
                {
                    return Err(bad());
                }
            }
            Event::Recover(n) => {
                if *n >= state.nodes.len()
                    || !recoveries.insert(*n)
                    || config.controllers[*n].recover_at_ps != Some(timer.at)
                {
                    return Err(bad());
                }
            }
        }
    }
    if bits != usize::from(state.active.is_some() || state.idle_timer)
        || !state.initialized
            && (!state.scheduled.is_empty() || !state.pending.is_empty() || state.active.is_some())
        || state
            .nodes
            .iter()
            .enumerate()
            .any(|(n, node)| node.application_queue.is_empty() == services.contains(&n))
    {
        return Err(bad());
    }
    for (g, (cursor, generator)) in state.cursors.iter().zip(&workload.generators).enumerate() {
        match &generator.schedule {
            Schedule::Explicit { .. } | Schedule::Uniform { .. } | Schedule::Geometric { .. } => {
                if cursor.position != cursor.ordinal + u64::from(generators.contains(&g)) {
                    return Err(bad());
                }
            }
            Schedule::Bernoulli { count, .. } => {
                if !generators.contains(&g) && state.initialized && cursor.position != *count {
                    return Err(bad());
                }
            }
        }
    }
    if state
        .cancelled
        .iter()
        .any(|id| !config.cancellations.iter().any(|c| c.request_id == *id))
        || state.initialized
            && config.cancellations.iter().any(|c| {
                !state.cancelled.contains(&c.request_id) && !cancellations.contains(&c.request_id)
            })
        || state
            .pending
            .iter()
            .any(|r| state.cancelled.contains(&r.id) || state.terminal.contains_key(&r.id))
        || state
            .terminal
            .values()
            .any(|outcome| !matches!(outcome.as_str(), "completed" | "cancelled"))
    {
        return Err(bad());
    }
    let check_request = |r: &Request| -> ModelResult {
        if r.controller >= state.nodes.len() || r.id.is_empty() || r.attempt == u64::MAX {
            return Err(bad());
        }
        if let Some(parent) = &r.parent {
            let (prefix, serial) = r.id.rsplit_once(':').ok_or_else(bad)?;
            let serial: u64 = serial.parse().map_err(|_| bad())?;
            if prefix != format!("response:{parent}")
                || serial == 0
                || serial > state.serial
                || !config.controllers[r.controller]
                    .responders
                    .contains(&r.frame)
            {
                return Err(bad());
            }
        } else {
            let (generator, ordinal) = r.id.rsplit_once(':').ok_or_else(bad)?;
            let ordinal: u64 = ordinal.parse().map_err(|_| bad())?;
            let (g, definition) = workload
                .generators
                .iter()
                .enumerate()
                .find(|(_, g)| g.id == generator)
                .ok_or_else(bad)?;
            if r.id != format!("{generator}:{ordinal}")
                || ordinal >= state.cursors[g].ordinal
                || definition.controller != config.controllers[r.controller].id
                || definition.frame != r.frame
            {
                return Err(bad());
            }
            if let Schedule::Explicit { times_ps } = &definition.schedule {
                if times_ps.get(ordinal as usize) != Some(&r.generated) {
                    return Err(bad());
                }
            }
        }
        codec::encode(&r.frame).map(|_| ())
    };
    let mut ids = std::collections::BTreeSet::new();
    for request in &state.pending {
        check_request(request)?;
        if !ids.insert(&request.id) {
            return Err(bad());
        }
    }
    if let Some(a) = &state.active {
        check_request(&a.request)?;
        let wire = codec::encode(&a.request.frame)?;
        let mut controllers = std::collections::BTreeSet::new();
        for (request, encoded) in Protocol::senders(a) {
            check_request(request)?;
            if !ids.insert(&request.id) && !a.settled
                || !controllers.insert(request.controller)
                || request.attempt == 0
                || *encoded != codec::encode(&request.frame)?
            {
                return Err(bad());
            }
        }
        if a.wire != wire
            || a.samples.len() != state.nodes.len()
            || a.rx_ack.len() != state.nodes.len()
            || a.position != a.observed.len()
            || a.observed.len() > 1_000_000
            || a.observed.iter().any(|b| *b > 1)
            || a.samples
                .iter()
                .any(|sample| sample.len() != a.position || sample.iter().any(|b| *b > 1))
            || !matches!(
                a.stage.as_str(),
                "frame" | "recovery" | "intermission" | "external"
            )
            || a.intermission > 2
            || a.overloads > 2
            || a.frame_done && a.failed
            || a.stage == "frame" && (a.frame_done || a.failed || a.position >= wire.bits.len())
            || a.stage == "intermission" && state.nodes.iter().any(|n| n.flag.is_some())
            || a.stage == "recovery" && !state.nodes.iter().any(|n| n.flag.is_some())
            || a.overload_start.is_some_and(|start| start > a.position + 1)
            || a.external_origin.is_some_and(|start| start >= a.position)
            || a.failed && !a.orphaned && a.error_start.is_none()
            || a.orphaned && (!a.peers.is_empty() || !a.failed || a.external_origin != Some(0))
        {
            return Err(bad());
        }
        for flag in state.nodes.iter().filter_map(|n| n.flag.as_ref()) {
            if !matches!(
                flag.kind.as_str(),
                "bit" | "ack" | "stuff" | "form" | "crc" | "overload"
            ) || flag.equal_run > 6
                || flag.last > 2
                || flag.delimiter > 8
                || flag.start > state.bit_tick + 2
                || flag.active && flag.end != Some(flag.start + 6)
                || !flag.active
                    && flag
                        .end
                        .is_some_and(|end| end > state.bit_tick || flag.equal_run != 6)
                || flag.end.is_some_and(|end| end < flag.start)
                || flag.delimiter > 0 && flag.end.is_none_or(|end| state.bit_tick <= end)
                || flag.kind == "overload" && (!flag.active || flag.ack_deferred)
                || flag.ack_deferred && (flag.kind != "ack" || flag.active)
            {
                return Err(bad());
            }
        }
        for signal in &a.history {
            if signal.controller >= state.nodes.len()
                || signal.start > a.position + 2
                || !matches!(
                    signal.kind.as_str(),
                    "bit" | "ack" | "stuff" | "form" | "crc"
                )
            {
                return Err(bad());
            }
        }
    }

    Ok(())
}
/// Registers the executable profile, strict schemas and complete explicit state codec.
pub fn register_can_protocol(r: &mut Registry) -> Result<(), Diagnostic> {
    r.register_event(EventDescriptor {
        schema: schema(),
        phase: 1,
        validate: event_validate,
    })?;
    r.register_module(
        KEY,
        factory,
        ModuleDescriptor {
            implementation_key: KEY.into(),
            implementation_version: "2".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![schema()],
            resources: vec!["bus".into()],
        },
    )?;
    r.register_model_record(ModelRecordDescriptor {
        schema: record_schema(),
        validate: output::validate_record,
    })?;
    for name in [
        "can.protocol.attempts",
        "can.protocol.failures",
        "can.protocol.receptions",
    ] {
        r.register_metric(MetricDescriptor {
            name: name.into(),
            unit: "count".into(),
        })?;
    }
    r.register_profile(ProfileDescriptor {
        name: PROFILE.into(),
        implementation_version: "2".into(),
        modules: vec![KEY.into()],
        channels: vec![],
        events: vec![schema()],
        metrics: vec![
            "can.protocol.attempts".into(),
            "can.protocol.failures".into(),
            "can.protocol.receptions".into(),
        ],
        model_records: vec![record_schema()],
        output_schema_version: 3,
        validate: |models, channels| {
            if models.len() == 1 && channels.is_empty() {
                Ok(())
            } else {
                Err("CAN protocol profile requires one coordinator and no channels".into())
            }
        },
    })?;
    r.register_profile_input_validator(PROFILE, input::validate)?;
    r.register_execution_capabilities(
        PROFILE,
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: false,
            checkpoint: true,
        },
    )?;
    r.register_model_checkpoint_codec(
        CheckpointCodecDescriptor {
            implementation_key: KEY.into(),
            implementation_version: "2".into(),
            codec_name: "dir.can.protocol.state".into(),
            codec_version: 2,
            external_state: "none".into(),
            max_bytes: 64 * 1024 * 1024,
        },
        |c, version, raw, context| {
            if version != 2 {
                return Err("unsupported CAN protocol checkpoint version".into());
            }
            let (config, workload) = input::parse(&c.profile_input)?;
            let state: State =
                serde_json::from_slice(raw).map_err(|e| ModelError(e.to_string()))?;
            validate_state(&state, &config, &workload)?;
            for timer in &state.scheduled {
                context.decode_timer_ref(&TimerReference {
                    version: 1,
                    owner: timer.token.owner.clone(),
                    id: timer.token.id.to_string(),
                    state: "live".into(),
                })?;
                let envelope = context.timer_envelope(&timer.token)?;
                if envelope.time_ps != timer.at
                    || envelope.schema != schema()
                    || envelope.payload != bytes(&timer.event)?
                    || envelope.destination != timer.token.owner
                {
                    return Err(
                        "CAN timer deadline/schema/payload/owner differs from saved FES".into(),
                    );
                }
            }
            if context.live_timers.len() != state.scheduled.len() {
                return Err("CAN timer ownership is incomplete".into());
            }
            Ok(Box::new(Protocol {
                config,
                workload,
                state,
            }))
        },
    )?;
    mixed::register(r)?;
    Ok(())
}

/// Lossless offline migration of v1 states which predate independent post-frame progress.
/// Recovery/intermission states are rejected because v1 omitted the per-controller phase.
pub fn migrate_v1_state(raw: &[u8]) -> ModelResult<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(raw).map_err(|e| ModelError(e.to_string()))?;
    let object = value
        .as_object_mut()
        .ok_or("CAN v1 state must be an object")?;
    let allowed = [
        "initialized",
        "cancelled",
        "terminal",
        "scheduled",
        "nodes",
        "cursors",
        "pending",
        "active",
        "idle_timer",
        "idle_origin",
        "idle_bits",
        "serial",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str())) || object.len() != allowed.len() {
        return Err("CAN v1 state has unknown or missing top-level fields".into());
    }
    let frame = !object["active"].is_null();
    let tick = if frame {
        let active = object
            .get_mut("active")
            .and_then(Value::as_object_mut)
            .ok_or("invalid CAN v1 active state")?;
        if active.get("stage").and_then(Value::as_str) != Some("frame")
            || active.get("failed") != Some(&Value::Bool(false))
            || active.get("frame_done") != Some(&Value::Bool(false))
            || active.get("orphaned") != Some(&Value::Bool(false))
            || active.get("external_origin") != Some(&Value::Null)
            || active.get("error_start") != Some(&Value::Null)
            || active.get("overload_start") != Some(&Value::Null)
            || active
                .get("flags")
                .and_then(Value::as_array)
                .is_none_or(|flags| flags.iter().any(|flag| !flag.is_null()))
        {
            return Err("CAN v1 aggregate recovery/intermission state has no lossless local-phase migration".into());
        }
        let tick = active
            .get("position")
            .and_then(Value::as_u64)
            .ok_or("invalid CAN v1 position")?;
        active.remove("flags");
        active.insert("settled".into(), Value::Bool(false));
        active.insert("closed".into(), Value::Bool(false));
        tick
    } else {
        object["idle_bits"]
            .as_u64()
            .ok_or("invalid CAN v1 idle position")?
    };
    object.insert("bit_tick".into(), json!(tick));
    let nodes = object
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .ok_or("invalid CAN v1 controller arena")?;
    for node in nodes {
        let node = node.as_object_mut().ok_or("invalid CAN v1 controller")?;
        if !frame && node.get("suspend_until").and_then(Value::as_u64) != Some(0) {
            return Err("CAN v1 suspended idle state has no lossless local-phase migration".into());
        }
        let phase = if frame { "Frame" } else { "Idle" };
        for (key, data) in [
            ("phase", json!({"phase":phase})),
            ("flag", Value::Null),
            ("frame_origin", json!(0)),
            ("received", json!(false)),
            ("needs_suspend", json!(false)),
            ("last_sample", json!(1)),
        ] {
            node.insert(key.into(), data);
        }
    }
    let state: State = serde_json::from_value(value).map_err(|e| ModelError(e.to_string()))?;
    bytes(&state)
}
