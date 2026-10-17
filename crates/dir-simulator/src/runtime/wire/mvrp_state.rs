//! MVRP Full Participant: transactional local machines, MAP and timer lifecycle.
use super::mvrp::*;
use crate::types::wire::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribute {
    pub applicant: Applicant,
    pub registrar: Registrar,
    pub registered: bool,
    pub demand: bool,
    pub request_tx: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timer {
    pub generation: u64,
    pub deadline: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub operational: bool,
    pub forwarding: bool,
    pub tc_detected: u32,
    pub leaveall_active: bool,
    pub periodic_active: bool,
    pub random_counter: u64,
    pub last_opportunity: Option<u64>,
    pub attributes: BTreeMap<u16, Attribute>,
    pub timers: BTreeMap<(String, Option<u16>), Timer>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub ports: BTreeMap<String, Port>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub machine: Option<String>,
    pub port: String,
    pub vid: Option<u16>,
    pub event: String,
    pub room: bool,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
}
#[derive(Debug, Clone)]
pub struct Delta {
    pub state: State,
    pub tx: Vec<(String, Vec<u8>)>,
    pub flush: BTreeSet<(String, u16)>,
    pub trace: Vec<Trace>,
    pub failed_registration: BTreeSet<(String, u16)>,
    pub steps: usize,
    #[cfg(test)]
    pub stages: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct TreeChange {
    pub port: String,
    pub operational: bool,
    pub forwarding: bool,
    pub tc_detected: u32,
    pub flush: bool,
    pub redeclare: bool,
}
#[derive(Debug, Clone)]
pub enum Input {
    Initialize,
    Receive(String, DecodedMvrp),
    Timer {
        port: String,
        slot: String,
        vid: Option<u16>,
        generation: u64,
    },
    TreeBatch(Vec<TreeChange>),
    Tree {
        port: String,
        operational: bool,
        forwarding: bool,
        tc_detected: u32,
        flush: bool,
        redeclare: bool,
    },
}
fn control(m: WireRegistrarMode) -> RegistrarControl {
    match m {
        WireRegistrarMode::Normal => RegistrarControl::Normal,
        WireRegistrarMode::FixedIgnoreNew => RegistrarControl::FixedIgnoreNew,
        WireRegistrarMode::FixedPropagateNew => RegistrarControl::FixedPropagateNew,
        WireRegistrarMode::Forbidden => RegistrarControl::Forbidden,
    }
}
fn applicant_mode(m: WireApplicantMode) -> ApplicantControl {
    match m {
        WireApplicantMode::Normal => ApplicantControl::Normal,
        WireApplicantMode::NewOnly => ApplicantControl::NewOnly,
        WireApplicantMode::NonParticipant => ApplicantControl::NonParticipant,
    }
}
fn recv(e: AttributeEvent) -> Event {
    match e {
        AttributeEvent::New => Event::RNew,
        AttributeEvent::JoinIn => Event::RJoinIn,
        AttributeEvent::In => Event::RIn,
        AttributeEvent::JoinMt => Event::RJoinMt,
        AttributeEvent::Mt => Event::RMt,
        AttributeEvent::Lv => Event::RLv,
    }
}
impl State {
    pub fn new(a: &PreparedWireAdapter) -> Self {
        Self {
            ports: a
                .ports
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        Port {
                            operational: true,
                            forwarding: true,
                            tc_detected: 0,
                            leaveall_active: false,
                            periodic_active: false,
                            random_counter: 0,
                            last_opportunity: None,
                            attributes: (1..=4094)
                                .map(|vid| {
                                    (
                                        vid,
                                        Attribute {
                                            applicant: Applicant::VO,
                                            registrar: Registrar::MT,
                                            registered: false,
                                            demand: false,
                                            request_tx: false,
                                        },
                                    )
                                })
                                .collect(),
                            timers: BTreeMap::new(),
                        },
                    )
                })
                .collect(),
        }
    }
    pub fn plan(
        &self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        now: u64,
        event: Input,
    ) -> Result<Delta, String> {
        let WireOptions::Mvrp(c) = &a.options else {
            return Err("MVRP configuration missing".into());
        };
        let mut d = Delta {
            state: self.clone(),
            tx: vec![],
            flush: BTreeSet::new(),
            trace: vec![],
            failed_registration: BTreeSet::new(),
            steps: 0,
            #[cfg(test)]
            stages: vec![],
        };
        match event {
            Input::Initialize => {
                for name in &a.ports {
                    d.initialize_port(p, a, c, name, now)?;
                }
                d.map(p, a, c, now, &BTreeSet::new())?;
            }
            Input::Receive(name, msg) => {
                if !d
                    .state
                    .ports
                    .get(&name)
                    .ok_or("MVRP unknown ingress")?
                    .operational
                {
                    return Ok(d);
                }
                let mut new_sources = BTreeSet::new();
                for v in msg.vectors {
                    if v.leave_all {
                        d.auxiliary(a, &name, "LeaveAll", AuxiliaryEvent::ReceiveLeaveAll)?;
                        d.arm_leaveall(a, c, &name, now)?;
                        for vid in 1..=4094 {
                            d.attribute(p, a, c, &name, vid, Event::RLA, now, &mut new_sources)?;
                        }
                    }
                    for (vid, event) in v.attributes {
                        d.attribute(p, a, c, &name, vid, recv(event), now, &mut new_sources)?;
                    }
                }
                d.map(p, a, c, now, &new_sources)?;
            }
            Input::Timer {
                port: name,
                slot,
                vid,
                generation,
            } => {
                let timer = d
                    .state
                    .ports
                    .get(&name)
                    .and_then(|p| p.timers.get(&(slot.clone(), vid)))
                    .ok_or("MVRP missing timer")?;
                if timer.generation != generation || timer.deadline != Some(now) {
                    return Ok(d);
                }
                d.state
                    .ports
                    .get_mut(&name)
                    .unwrap()
                    .timers
                    .get_mut(&(slot.clone(), vid))
                    .unwrap()
                    .deadline = None;
                let mut new_sources = BTreeSet::new();
                match slot.as_str() {
                    "leave" => {
                        d.attribute(
                            p,
                            a,
                            c,
                            &name,
                            vid.ok_or("MVRP leave timer VID")?,
                            Event::LeaveExpired,
                            now,
                            &mut new_sources,
                        )?;
                        d.map(p, a, c, now, &new_sources)?;
                    }
                    "leaveall" => {
                        d.auxiliary(a, &name, "LeaveAll", AuxiliaryEvent::LeaveAllExpired)?;
                        d.arm_leaveall(a, c, &name, now)?;
                        d.request(c, &name, now)?;
                    }
                    "periodic" => {
                        let transition =
                            d.auxiliary(a, &name, "Periodic", AuxiliaryEvent::PeriodicExpired)?;
                        if transition.periodic {
                            d.arm(&name, "periodic", None, now, 1_000_000_000_000)?;
                            for vid in 1..=4094 {
                                d.attribute(
                                    p,
                                    a,
                                    c,
                                    &name,
                                    vid,
                                    Event::Periodic,
                                    now,
                                    &mut new_sources,
                                )?;
                            }
                        }
                    }
                    "join" => d.transmit(p, a, c, &name, now)?,
                    _ => return Err("MVRP timer slot unknown".into()),
                }
            }
            Input::Tree {
                port,
                operational,
                forwarding,
                tc_detected,
                flush,
                redeclare,
            } => {
                d.tree_batch(
                    p,
                    a,
                    c,
                    now,
                    vec![TreeChange {
                        port,
                        operational,
                        forwarding,
                        tc_detected,
                        flush,
                        redeclare,
                    }],
                )?;
            }
            Input::TreeBatch(changes) => d.tree_batch(p, a, c, now, changes)?,
        }
        if d.steps > a.max_control_steps {
            return Err("MVRP max_control_steps exceeded".into());
        }
        let timers: usize = d
            .state
            .ports
            .values()
            .map(|p| p.timers.values().filter(|t| t.deadline.is_some()).count())
            .sum();
        if timers > a.max_pending_timers || d.tx.len() > a.max_pending_tx {
            return Err("MVRP pending resource capacity".into());
        }
        Ok(d)
    }
}
impl Delta {
    fn stage(&mut self, name: &str) {
        #[cfg(test)]
        self.stages.push(name.into());
        #[cfg(not(test))]
        let _ = name;
    }
    fn tree_batch(
        &mut self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        now: u64,
        mut changes: Vec<TreeChange>,
    ) -> Result<(), String> {
        changes.sort_by(|x, y| x.port.cmp(&y.port));
        let mut new_sources = BTreeSet::new();
        // MAC lifecycle reset precedes the RSTP-to-MRP role primitives.
        for change in &changes {
            let before = self
                .state
                .ports
                .get(&change.port)
                .ok_or("MVRP tree unknown port")?
                .operational;
            if before && !change.operational {
                let keys: Vec<_> = self.state.ports[&change.port]
                    .timers
                    .keys()
                    .cloned()
                    .collect();
                for (slot, vid) in keys {
                    self.cancel(&change.port, &slot, vid)?;
                }
                for vid in 1..=4094 {
                    self.attribute(
                        p,
                        a,
                        c,
                        &change.port,
                        vid,
                        Event::Flush,
                        now,
                        &mut new_sources,
                    )?;
                }
            }
            self.state.ports.get_mut(&change.port).unwrap().operational = change.operational;
            if !before && change.operational {
                self.initialize_port(p, a, c, &change.port, now)?;
            }
        }
        for change in changes.iter().filter(|x| x.flush) {
            self.stage("registrar_flush");
            for vid in 1..=4094 {
                self.attribute(
                    p,
                    a,
                    c,
                    &change.port,
                    vid,
                    Event::Flush,
                    now,
                    &mut new_sources,
                )?;
            }
            self.stage("leaveall_timer");
            self.auxiliary(a, &change.port, "LeaveAll", AuxiliaryEvent::LeaveAllExpired)?;
            self.arm_leaveall(a, c, &change.port, now)?;
            self.request(c, &change.port, now)?;
        }
        for change in changes.iter().filter(|x| x.redeclare) {
            self.stage("redeclare");
            for vid in 1..=4094 {
                self.attribute(
                    p,
                    a,
                    c,
                    &change.port,
                    vid,
                    Event::Redeclare,
                    now,
                    &mut new_sources,
                )?;
            }
        }
        self.stage("forwarding_difference");
        for change in &changes {
            let port = self.state.ports.get_mut(&change.port).unwrap();
            port.forwarding = change.forwarding;
            port.tc_detected = change.tc_detected;
        }
        self.stage("map_recompute");
        self.map(p, a, c, now, &new_sources)?;
        self.stage("mvrp_settle");
        Ok(())
    }
    fn auxiliary(
        &mut self,
        a: &PreparedWireAdapter,
        name: &str,
        machine: &str,
        event: AuxiliaryEvent,
    ) -> Result<AuxiliaryStep, String> {
        let port = &self.state.ports[name];
        let active = if machine == "LeaveAll" {
            port.leaveall_active
        } else {
            port.periodic_active
        };
        let before = if active {
            AuxiliaryState::Active
        } else {
            AuxiliaryState::Passive
        };
        let result = if machine == "LeaveAll" {
            leaveall(before, event)
        } else {
            periodic(before, event)
        };
        if before != result.state
            || result.restart_timer
            || result.send_leaveall
            || result.periodic
            || result.request_tx
        {
            self.step(a)?;
            self.trace.push(Trace { machine: Some(machine.into()), port:name.into(), vid:None, event:format!("{event:?}"),room:true,
                before:serde_json::json!({"state":format!("{before:?}")}),
                after:serde_json::json!({"state":format!("{:?}",result.state),"restart_timer":result.restart_timer,"send_leaveall":result.send_leaveall,"periodic":result.periodic,"request_tx":result.request_tx}) });
        }
        let port = self.state.ports.get_mut(name).unwrap();
        if machine == "LeaveAll" {
            port.leaveall_active = result.state == AuxiliaryState::Active;
        } else {
            port.periodic_active = result.state == AuxiliaryState::Active;
        }
        Ok(result)
    }
    fn step(&mut self, a: &PreparedWireAdapter) -> Result<(), String> {
        self.steps = self.steps.checked_add(1).ok_or("MVRP step overflow")?;
        if self.steps > a.max_control_steps {
            Err("MVRP max_control_steps exceeded".into())
        } else {
            Ok(())
        }
    }
    fn arm(
        &mut self,
        name: &str,
        slot: &str,
        vid: Option<u16>,
        now: u64,
        duration: u64,
    ) -> Result<(), String> {
        if duration == 0 {
            return Err("MVRP zero timer duration".into());
        }
        let deadline = now.checked_add(duration).ok_or("MVRP deadline overflow")?;
        let t = self
            .state
            .ports
            .get_mut(name)
            .ok_or("MVRP unknown timer port")?
            .timers
            .entry((slot.into(), vid))
            .or_insert(Timer {
                generation: 0,
                deadline: None,
            });
        t.generation = t.generation.checked_add(1).ok_or("MVRP token overflow")?;
        t.deadline = Some(deadline);
        Ok(())
    }
    fn cancel(&mut self, name: &str, slot: &str, vid: Option<u16>) -> Result<(), String> {
        if let Some(t) = self
            .state
            .ports
            .get_mut(name)
            .ok_or("MVRP unknown timer port")?
            .timers
            .get_mut(&(slot.into(), vid))
        {
            if t.deadline.take().is_some() {
                t.generation = t.generation.checked_add(1).ok_or("MVRP token overflow")?;
            }
        }
        Ok(())
    }
    fn config<'a>(c: &'a MvrpConfig, name: &str) -> Result<&'a MvrpPortConfig, String> {
        c.ports
            .iter()
            .find(|p| p.port == name)
            .ok_or_else(|| "MVRP unknown port parameters".into())
    }
    fn arm_leaveall(
        &mut self,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        name: &str,
        now: u64,
    ) -> Result<(), String> {
        let cfg = Self::config(c, name)?;
        if cfg.leaveall_time_ps == 0 || cfg.applicant_control == WireApplicantMode::NewOnly {
            return self.cancel(name, "leaveall", None);
        }
        let l = cfg.leaveall_time_ps;
        let n = l.div_ceil(2) - 1;
        let bound = (1u128 << 64) - ((1u128 << 64) % n as u128);
        let sample = loop {
            let port = self.state.ports.get_mut(name).unwrap();
            let text = format!(
                "mvrp-la-v1\n{}\n{}\n{}\n{}\n",
                c.random_seed, a.bridge, name, port.random_counter
            );
            port.random_counter = port
                .random_counter
                .checked_add(1)
                .ok_or("MVRP random counter overflow")?;
            let digest = Sha256::digest(text.as_bytes());
            let x = u64::from_be_bytes(digest[..8].try_into().unwrap());
            if (x as u128) < bound {
                break l + 1 + x % n;
            }
        };
        self.arm(name, "leaveall", None, now, sample)
    }
    fn request(&mut self, c: &MvrpConfig, name: &str, now: u64) -> Result<(), String> {
        let cfg = Self::config(c, name)?;
        let port = &self.state.ports[name];
        if !port.operational || cfg.applicant_control == WireApplicantMode::NonParticipant {
            return Ok(());
        }
        if port
            .timers
            .get(&("join".into(), None))
            .is_some_and(|t| t.deadline.is_some())
        {
            return Ok(());
        }
        let earliest = now.checked_add(1).ok_or("MVRP TX time overflow")?;
        let at = match port.last_opportunity {
            None => earliest,
            Some(last) => earliest.max(
                last.checked_add(cfg.join_time_ps.div_ceil(2))
                    .and_then(|n| n.checked_add(1))
                    .ok_or("MVRP TX pacing overflow")?,
            ),
        };
        self.arm(name, "join", None, now, at - now)
    }
    fn initialize_port(
        &mut self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        name: &str,
        now: u64,
    ) -> Result<(), String> {
        let cfg = Self::config(c, name)?;
        // A Begin/reset invalidates previous opportunities and every old timer.
        // Fresh initialization has no reservations; recovery and management
        // reset must not retain a normal-mode timer in a New-only participant.
        let timer_keys: Vec<_> = self.state.ports[name].timers.keys().cloned().collect();
        for (slot, vid) in timer_keys {
            self.cancel(name, &slot, vid)?;
        }
        self.state.ports.get_mut(name).unwrap().last_opportunity = None;
        if cfg.applicant_control != WireApplicantMode::NewOnly {
            self.auxiliary(a, name, "LeaveAll", AuxiliaryEvent::Begin)?;
            self.auxiliary(a, name, "Periodic", AuxiliaryEvent::Begin)?;
            if !cfg.periodic_enabled {
                self.auxiliary(a, name, "Periodic", AuxiliaryEvent::Disabled)?;
            }
        }
        let port = self.state.ports.get_mut(name).unwrap();
        let mut resets = vec![];
        for (vid, attr) in &mut port.attributes {
            let before = attr.clone();
            attr.applicant = Applicant::VO;
            attr.demand = false;
            attr.request_tx = false;
            attr.registered = false;
            attr.registrar = if matches!(
                p.registrar_modes.get(&(name.into(), *vid)),
                Some(WireRegistrarMode::FixedIgnoreNew | WireRegistrarMode::FixedPropagateNew)
            ) {
                Registrar::IN
            } else {
                Registrar::MT
            };
            if *attr != before {
                resets.push((*vid, before, attr.clone()));
            }
        }
        let operational = port.operational;
        for (vid, before, after) in resets {
            self.step(a)?;
            self.trace.push(Trace {
                machine: None,
                port: name.into(),
                vid: Some(vid),
                event: "Begin".into(),
                room: true,
                before: serde_json::to_value(before).unwrap(),
                after: serde_json::to_value(after).unwrap(),
            });
        }
        if !operational {
            return Ok(());
        }
        // Begin arms the periodic timer even when the following Disabled event
        // makes its first expiry a no-op. New-only does not run this machine.
        let arm_periodic = cfg.applicant_control != WireApplicantMode::NewOnly;
        self.arm_leaveall(a, c, name, now)?;
        if arm_periodic {
            self.arm(name, "periodic", None, now, 1_000_000_000_000)?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Existing staged reducer boundary.
    fn attribute(
        &mut self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        name: &str,
        vid: u16,
        event: Event,
        now: u64,
        new_sources: &mut BTreeSet<(String, u16)>,
    ) -> Result<(), String> {
        self.attribute_from(p, a, c, name, vid, event, now, new_sources, None)
    }
    #[allow(clippy::too_many_arguments)] // Existing staged reducer boundary.
    fn attribute_from(
        &mut self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        name: &str,
        vid: u16,
        event: Event,
        now: u64,
        new_sources: &mut BTreeSet<(String, u16)>,
        journal_before: Option<Attribute>,
    ) -> Result<(), String> {
        let cfg = Self::config(c, name)?;
        let mode = p
            .registrar_modes
            .get(&(name.into(), vid))
            .copied()
            .unwrap_or(WireRegistrarMode::Normal);
        let before =
            journal_before.unwrap_or_else(|| self.state.ports[name].attributes[&vid].clone());
        let r = registrar(before.registrar, event, true, control(mode));
        let app = applicant(
            before.applicant,
            event,
            true,
            r.state,
            applicant_mode(cfg.applicant_control),
        );
        let attr = self
            .state
            .ports
            .get_mut(name)
            .unwrap()
            .attributes
            .get_mut(&vid)
            .unwrap();
        attr.registrar = r.state;
        attr.applicant = app.state;
        attr.request_tx |= app.request_tx;
        if r.leave {
            attr.registered = false;
        }
        if let Some(new) = r.join {
            if mode == WireRegistrarMode::Normal {
                if !cfg.restricted_vlan_registration
                    || p.svre_exists.contains(&(a.bridge.clone(), vid))
                {
                    attr.registered = true;
                } else {
                    self.failed_registration.insert((name.into(), vid));
                }
            }
            if (attr.registered
                || matches!(
                    mode,
                    WireRegistrarMode::FixedIgnoreNew | WireRegistrarMode::FixedPropagateNew
                ))
                && new
            {
                new_sources.insert((name.into(), vid));
                self.flush.insert((name.into(), vid));
            }
        }
        let after = attr.clone();
        if before != after || r.join.is_some() || r.leave {
            self.step(a)?;
            self.trace.push(Trace {
                machine: None,
                port: name.into(),
                vid: Some(vid),
                event: format!("{event:?}"),
                room: true,
                before: serde_json::to_value(before).unwrap(),
                after: serde_json::to_value(after).unwrap(),
            });
        }
        if r.cancel_leave {
            self.cancel(name, "leave", Some(vid))?;
        }
        if r.arm_leave {
            self.arm(name, "leave", Some(vid), now, cfg.leave_time_ps)?;
        }
        if app.request_tx {
            self.request(c, name, now)?;
        }
        Ok(())
    }
    fn map(
        &mut self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        now: u64,
        new_sources: &BTreeSet<(String, u16)>,
    ) -> Result<(), String> {
        // Sources are immutable external local/static/Registrar inputs, never Applicant demand.
        let f: BTreeSet<_> = self
            .state
            .ports
            .iter()
            .filter(|(_, p)| p.operational && p.forwarding)
            .map(|(n, _)| n.clone())
            .collect();
        for vid in 1..=4094 {
            let sources: BTreeSet<_> = a
                .ports
                .iter()
                .filter(|n| {
                    f.contains(*n)
                        && (c.local_declarations.contains(&((*n).clone(), vid))
                            || self.state.ports[*n].attributes[&vid].registered
                            || matches!(
                                p.registrar_modes.get(&((*n).clone(), vid)),
                                Some(
                                    WireRegistrarMode::FixedIgnoreNew
                                        | WireRegistrarMode::FixedPropagateNew
                                )
                            ))
                })
                .cloned()
                .collect();
            for name in &a.ports {
                let wanted = f.contains(name)
                    && (c.local_declarations.contains(&(name.clone(), vid))
                        || sources.iter().any(|s| s != name));
                let prior = self.state.ports[name].attributes[&vid].clone();
                let before = prior.demand;
                let new = f.contains(name)
                    && (new_sources
                        .iter()
                        .any(|(s, v)| *v == vid && s != name && f.contains(s))
                        || !before && c.local_declarations.contains(&(name.clone(), vid))
                        || !before
                            && sources
                                .iter()
                                .any(|s| s != name && self.state.ports[s].tc_detected > 0));
                let event = if wanted && new {
                    Some(Event::New)
                } else if wanted && !before {
                    Some(Event::Join)
                } else if !wanted && before {
                    Some(Event::Lv)
                } else {
                    None
                };
                self.state
                    .ports
                    .get_mut(name)
                    .unwrap()
                    .attributes
                    .get_mut(&vid)
                    .unwrap()
                    .demand = wanted;
                if let Some(event) = event {
                    let mut ignored = BTreeSet::new();
                    self.attribute_from(p, a, c, name, vid, event, now, &mut ignored, Some(prior))?;
                    if event == Event::New {
                        self.flush.insert((name.clone(), vid));
                    }
                }
            }
        }
        Ok(())
    }
    fn transmit(
        &mut self,
        p: &PreparedWire,
        a: &PreparedWireAdapter,
        c: &MvrpConfig,
        name: &str,
        now: u64,
    ) -> Result<(), String> {
        let cfg = Self::config(c, name)?;
        if !self.state.ports[name].operational
            || cfg.applicant_control == WireApplicantMode::NonParticipant
        {
            return Ok(());
        }
        self.state.ports.get_mut(name).unwrap().last_opportunity = Some(now);
        let la = self.state.ports[name].leaveall_active
            && cfg.applicant_control == WireApplicantMode::Normal;
        let mut sources = BTreeSet::new();
        if la {
            self.auxiliary(a, name, "LeaveAll", AuxiliaryEvent::Tx)?;
            for vid in 1..=4094 {
                self.attribute(p, a, c, name, vid, Event::RLA, now, &mut sources)?;
            }
            self.map(p, a, c, now, &sources)?;
        }
        let mut attrs = vec![];
        let mut selected = Vec::new();
        for vid in 1..=4094 {
            let attr = &self.state.ports[name].attributes[&vid];
            if !attr.request_tx && !la {
                continue;
            }
            let r = if la {
                registrar(
                    attr.registrar,
                    Event::TxLA,
                    true,
                    control(
                        p.registrar_modes
                            .get(&(name.into(), vid))
                            .copied()
                            .unwrap_or(WireRegistrarMode::Normal),
                    ),
                )
                .state
            } else {
                attr.registrar
            };
            let result = applicant(
                attr.applicant,
                if la { Event::TxLA } else { Event::Tx },
                true,
                r,
                applicant_mode(cfg.applicant_control),
            );
            if let Some(event) = result.action.filter(|_| !result.optional) {
                attrs.push((vid, event));
            }
            selected.push((vid, result));
        }
        let zero = if cfg.transmit_zero {
            let untagged: Vec<_> = p.ports[name]
                .static_vlans
                .iter()
                .filter(|(_, tagged)| !**tagged)
                .map(|(v, _)| *v)
                .collect();
            if untagged.len() == 1 {
                Some(untagged[0])
            } else {
                None
            }
        } else {
            None
        };
        let mut wire: Vec<_> = attrs
            .iter()
            .map(|(vid, e)| (if Some(*vid) == zero { 0 } else { *vid }, *e))
            .collect();
        wire.sort_by_key(|x| x.0);
        wire.truncate(c.max_attributes_per_pdu);
        let (bytes, used) = if attrs.is_empty() && !la {
            (vec![], 0)
        } else {
            encode(
                p.ports[name].source_mac.ok_or("MVRP missing TX identity")?,
                &wire,
                la,
                a.max_pdu_bytes,
            )
            .map_err(|e| e.0)?
        };
        let included: BTreeSet<_> = wire[..used]
            .iter()
            .map(|(vid, _)| if *vid == 0 { zero.unwrap() } else { *vid })
            .collect();
        for (vid, result) in selected {
            let before = self.state.ports[name].attributes[&vid].clone();
            let required = result.action.is_some() && !result.optional;
            let result = if required && !included.contains(&vid) {
                applicant_tx(
                    before.applicant,
                    if la { Event::TxLA } else { Event::Tx },
                    true,
                    before.registrar,
                    applicant_mode(cfg.applicant_control),
                    false,
                )
            } else {
                result
            };
            let attr = self
                .state
                .ports
                .get_mut(name)
                .unwrap()
                .attributes
                .get_mut(&vid)
                .unwrap();
            attr.applicant = result.state;
            attr.request_tx = result.request_tx;
            let after = attr.clone();
            if before != after {
                self.step(a)?;
                self.trace.push(Trace {
                    machine: None,
                    port: name.into(),
                    vid: Some(vid),
                    event: if la && required && !included.contains(&vid) {
                        "TxLAF"
                    } else if la {
                        "TxLA"
                    } else {
                        "Tx"
                    }
                    .into(),
                    room: !required || included.contains(&vid),
                    before: serde_json::to_value(before).unwrap(),
                    after: serde_json::to_value(after).unwrap(),
                });
            }
        }
        if !bytes.is_empty() {
            self.tx.push((name.into(), bytes));
        }
        if self.state.ports[name]
            .attributes
            .values()
            .any(|a| a.request_tx)
        {
            self.request(c, name, now)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "mvrp_vectors.rs"]
mod supplied_vectors;
