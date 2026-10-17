use super::guards;
// ForceProtocolVersion=2 CIST. Wire values retain their 1/256 second units.
use super::codec::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const DA: [u8; 6] = [1, 128, 194, 0, 0, 0];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Config,
    Rst,
    Tcn,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Priority(pub [u64; 7]);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Times {
    pub age: u16,
    pub max_age: u16,
    pub hello: u16,
    pub forward_delay: u16,
}
impl Default for Times {
    fn default() -> Self {
        Self {
            age: 0,
            max_age: 20 * 256,
            hello: 2 * 256,
            forward_delay: 15 * 256,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bpdu {
    pub kind: Kind,
    pub version: u8,
    pub flags: u8,
    pub priority: Priority,
    pub times: Times,
}
pub fn decode(b: &[u8], port_id: u16, max: usize) -> DecodeResult<Bpdu> {
    let payload = client(b, DA, None, max)?;
    if payload.len() < 7 || payload[..3] != [66, 66, 3] {
        return Err(bad("llc"));
    }
    let b = &payload[3..];
    if b[..2] != [0, 0] {
        return Err(bad("protocol_id"));
    }
    let kind = match (b[3], b[2], b.len()) {
        (0, _, n) if n >= 35 => Kind::Config,
        (128, _, _) => Kind::Tcn,
        (2, v, n) if v >= 2 && n >= 36 => Kind::Rst,
        (2, v, _) if v < 2 => return Err(bad("unsupported_version")),
        (2, _, _) => return Err(bad("rst_short")),
        (0, _, _) => return Err(bad("config_short")),
        _ => return Err(bad("unsupported_type")),
    };
    if kind == Kind::Tcn {
        return Ok(Bpdu {
            kind,
            version: b[2],
            flags: 0,
            priority: Priority([0; 7]),
            times: Times::default(),
        });
    }
    let root = u64::from_be_bytes(b[5..13].try_into().unwrap());
    let cost = u32::from_be_bytes(b[13..17].try_into().unwrap()) as u64;
    let sender = u64::from_be_bytes(b[17..25].try_into().unwrap());
    Ok(Bpdu {
        kind,
        version: b[2],
        flags: if kind == Kind::Config {
            b[4] & 0x81 | 12
        } else {
            b[4] & 127
        },
        priority: Priority([
            root,
            cost,
            sender,
            0,
            sender,
            u16be(b, 25)? as u64,
            port_id as u64,
        ]),
        times: Times {
            age: u16be(b, 27)?,
            max_age: u16be(b, 29)?,
            hello: u16be(b, 31)?,
            forward_delay: u16be(b, 33)?,
        },
    })
}
pub fn encode(sa: [u8; 6], m: &Bpdu) -> Vec<u8> {
    let mut p = vec![
        66,
        66,
        3,
        0,
        0,
        if m.kind == Kind::Rst { 2 } else { 0 },
        match m.kind {
            Kind::Config => 0,
            Kind::Tcn => 128,
            Kind::Rst => 2,
        },
    ];
    if m.kind != Kind::Tcn {
        p.push(if m.kind == Kind::Rst {
            m.flags & 127
        } else {
            m.flags & 0x81
        });
        p.extend(m.priority.0[0].to_be_bytes());
        p.extend((m.priority.0[1] as u32).to_be_bytes());
        p.extend(m.priority.0[4].to_be_bytes());
        p.extend((m.priority.0[5] as u16).to_be_bytes());
        for t in [
            m.times.age,
            m.times.max_age,
            m.times.hello,
            m.times.forward_delay,
        ] {
            p.extend(t.to_be_bytes());
        }
        if m.kind == Kind::Rst {
            p.push(0);
        }
    }
    frame(DA, sa, p.len() as u16, &p)
}
macro_rules! states {($name:ident {$($v:ident),*})=>{#[allow(non_camel_case_types,clippy::upper_case_acronyms,clippy::enum_variant_names)]#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]pub enum $name{$($v),*}};}
states!(Pti { ONE_SECOND, TICK });
states!(Prx { DISCARD, RECEIVE });
states!(Ppm {
    CHECKING_RSTP,
    SELECTING_STP,
    SENSING
});
states!(Bdm {
    EDGE,
    NOT_EDGE,
    ISOLATED
});
states!(Ptx {
    TRANSMIT_INIT,
    TRANSMIT_PERIODIC,
    TRANSMIT_CONFIG,
    TRANSMIT_TCN,
    TRANSMIT_RSTP,
    IDLE
});
states!(Pim {
    DISABLED,
    AGED,
    UPDATE,
    CURRENT,
    RECEIVE,
    SUPERIOR_DESIGNATED,
    REPEATED_DESIGNATED,
    INFERIOR_DESIGNATED,
    NOT_DESIGNATED,
    OTHER
});
states!(Prs {
    INIT_TREE,
    ROLE_SELECTION
});
states!(Prt {
    INIT_PORT,
    DISABLE_PORT,
    DISABLED_PORT,
    ROOT_PORT,
    ROOT_DISCARD,
    ROOT_PROPOSED,
    ROOT_AGREED,
    ROOT_SYNCED,
    REROOT,
    REROOTED,
    ROOT_LEARN,
    ROOT_FORWARD,
    DESIGNATED_PORT,
    DESIGNATED_DISCARD,
    DESIGNATED_PROPOSE,
    DESIGNATED_AGREE,
    DESIGNATED_SYNCED,
    DESIGNATED_RETIRED,
    DESIGNATED_LEARN,
    DESIGNATED_FORWARD,
    BLOCK_PORT,
    ALTERNATE_PORT,
    ALTERNATE_PROPOSED,
    ALTERNATE_AGREED,
    BACKUP_PORT
});
states!(Pst {
    DISCARDING,
    LEARNING,
    FORWARDING
});
states!(Tcm {
    INACTIVE,
    LEARNING,
    DETECTED,
    ACTIVE,
    NOTIFIED_TCN,
    NOTIFIED_TC,
    PROPAGATING,
    ACKNOWLEDGED
});
states!(Role {
    Disabled,
    Root,
    Designated,
    Alternate,
    Backup
});
states!(ReceivedInfo {
    SuperiorDesignatedInfo,
    RepeatedDesignatedInfo,
    InferiorDesignatedInfo,
    NotDesignatedInfo,
    OtherInfo
});
states!(Info {
    Disabled,
    Aged,
    Mine,
    Received
});
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortConfig {
    pub id: u16,
    pub cost: u32,
    pub admin_edge: bool,
    pub auto_edge: bool,
    pub auto_isolate: bool,
    pub restricted_role: bool,
    pub restricted_tcn: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub edge: u32,
    pub hello: u32,
    pub migration: u32,
    pub fd: u32,
    pub rr: u32,
    pub rb: u32,
    pub info: u32,
    pub tc: u32,
    pub detected: u32,
    pub tx: u32,
}
impl Counters {
    fn tick(&mut self) {
        for c in [
            &mut self.edge,
            &mut self.hello,
            &mut self.migration,
            &mut self.fd,
            &mut self.rr,
            &mut self.rb,
            &mut self.info,
            &mut self.tc,
            &mut self.detected,
            &mut self.tx,
        ] {
            *c = c.saturating_sub(1);
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub config: PortConfig,
    pub enabled: bool,
    pub role: Role,
    pub selected_role: Role,
    pub info: Info,
    pub priority: Priority,
    pub designated: Priority,
    pub times: Times,
    pub designated_times: Times,
    pub counters: Counters,
    pub prx: Prx,
    pub ppm: Ppm,
    pub bdm: Bdm,
    pub ptx: Ptx,
    pub pim: Pim,
    pub prt: Prt,
    pub pst: Pst,
    pub tcm: Tcm,
    pub selected: bool,
    pub reselect: bool,
    pub update: bool,
    pub send_rstp: bool,
    pub rcvd_stp: bool,
    pub rcvd_rstp: bool,
    pub mcheck: bool,
    pub oper_edge: bool,
    pub isolate: bool,
    pub proposing: bool,
    pub proposed: bool,
    pub agree: bool,
    pub agreed: bool,
    pub disputed: bool,
    pub sync: bool,
    pub synced: bool,
    pub reroot: bool,
    pub learn: bool,
    pub forward: bool,
    pub learning: bool,
    pub forwarding: bool,
    pub new_info: bool,
    pub tc_ack: bool,
    pub rcvd_tc: bool,
    pub rcvd_tcn: bool,
    pub rcvd_tc_ack: bool,
    pub tc_prop: bool,
    pub fdb_flush: bool,
    pub message: Option<Bpdu>,
    pub last_message: Option<Bpdu>,
    pub received_info: Option<ReceivedInfo>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bridge {
    pub id: u64,
    pub times: Times,
    pub root: Priority,
    pub root_times: Times,
    pub root_port: u16,
    pub hold: u32,
    pub pti: Pti,
    pub prs: Prs,
    pub ports: BTreeMap<String, Port>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub port: Option<String>,
    pub machine: String,
    pub from_state: String,
    pub to_state: String,
    pub guard: String,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
    pub tree_before: Option<serde_json::Value>,
    pub tree_after: Option<serde_json::Value>,
}
#[derive(Debug, Clone)]
pub struct Delta {
    pub state: Bridge,
    pub tx: Vec<(String, Bpdu)>,
    pub flush: Vec<String>,
    pub transitions: Vec<Transition>,
}
#[derive(Debug, Clone)]
pub enum Input {
    Initialize,
    Tick,
    Receive(String, Bpdu),
    Link(String, bool),
}
fn received_age_seconds(ticks: u16) -> u32 {
    (ticks as u32 + 384) / 256
}
fn ceil(t: u16) -> u32 {
    (t as u32).div_ceil(256)
}
fn same(a: Priority, b: Priority) -> bool {
    a.0[..6] == b.0[..6]
}
// Ordered CIST classification, IEEE 802.1Q 13.10 and 13.29.12.
fn classify(p: &Port, m: &Bpdu) -> ReceivedInfo {
    if m.kind == Kind::Tcn {
        return ReceivedInfo::OtherInfo;
    }
    let role = (m.flags >> 2) & 3;
    if role == 3 {
        let same_sender = (m.priority.0[4] & 0x0000ffffffffffff)
            == (p.priority.0[4] & 0x0000ffffffffffff)
            && (m.priority.0[5] & 4095) == (p.priority.0[5] & 4095);
        if m.priority.0[..6] < p.priority.0[..6]
            || same_sender
            || same(m.priority, p.priority) && m.times != p.times
        {
            ReceivedInfo::SuperiorDesignatedInfo
        } else if same(m.priority, p.priority) && m.times == p.times && p.info == Info::Received {
            ReceivedInfo::RepeatedDesignatedInfo
        } else {
            ReceivedInfo::InferiorDesignatedInfo
        }
    } else if matches!(role, 1 | 2) && m.priority.0[..6] >= p.priority.0[..6] {
        ReceivedInfo::NotDesignatedInfo
    } else {
        ReceivedInfo::OtherInfo
    }
}
// Shared production entry action; also directly exercised by forced-entry tests.
fn record_designated(p: &mut Port, m: &Bpdu, superior: bool) {
    p.pim = if superior {
        Pim::SUPERIOR_DESIGNATED
    } else {
        Pim::REPEATED_DESIGNATED
    };
    if superior {
        p.agreed = false;
        p.proposing = false;
        p.agree &= p.info == Info::Received && m.priority <= p.priority;
    }
    p.proposed |= m.flags & 2 != 0;
    p.rcvd_tc |= m.flags & 1 != 0;
    p.rcvd_tc_ack |= m.kind == Kind::Config && m.flags & 128 != 0;
    p.agreed = m.flags & 64 != 0 && m.kind == Kind::Rst;
    if p.agreed {
        p.proposing = false;
    }
    if superior {
        p.synced &= p.agreed;
        p.priority = m.priority;
        p.times = m.times;
        p.times.hello = 512;
        p.info = Info::Received;
        p.reselect = true;
        p.selected = false;
    }
    p.counters.info = if received_age_seconds(m.times.age) <= m.times.max_age as u32 / 256 {
        6
    } else {
        0
    };
}
impl Bridge {
    pub fn new(
        id: u64,
        times: Times,
        hold: u32,
        ports: BTreeMap<String, (PortConfig, bool)>,
    ) -> Self {
        let root = Priority([id, 0, id, 0, id, 0, 0]);
        let ports = ports
            .into_iter()
            .map(|(name, (config, enabled))| {
                let priority = Priority([id, 0, id, 0, id, config.id as u64, config.id as u64]);
                let admin = config.admin_edge;
                (
                    name,
                    Port {
                        config,
                        enabled,
                        role: Role::Disabled,
                        selected_role: Role::Disabled,
                        info: Info::Disabled,
                        priority,
                        designated: priority,
                        times,
                        designated_times: times,
                        counters: Counters {
                            edge: 3,
                            hello: 0,
                            migration: 3,
                            fd: ceil(times.max_age),
                            rr: ceil(times.forward_delay),
                            rb: 0,
                            info: 0,
                            tc: 0,
                            detected: 0,
                            tx: 0,
                        },
                        prx: Prx::DISCARD,
                        ppm: Ppm::CHECKING_RSTP,
                        bdm: if admin { Bdm::EDGE } else { Bdm::NOT_EDGE },
                        ptx: Ptx::TRANSMIT_INIT,
                        pim: Pim::DISABLED,
                        prt: Prt::INIT_PORT,
                        pst: Pst::DISCARDING,
                        tcm: Tcm::INACTIVE,
                        selected: false,
                        reselect: true,
                        update: false,
                        send_rstp: true,
                        rcvd_stp: false,
                        rcvd_rstp: false,
                        mcheck: false,
                        oper_edge: admin,
                        isolate: false,
                        proposing: false,
                        proposed: false,
                        agree: false,
                        agreed: false,
                        disputed: false,
                        sync: true,
                        synced: false,
                        reroot: true,
                        learn: false,
                        forward: false,
                        learning: false,
                        forwarding: false,
                        new_info: true,
                        tc_ack: false,
                        rcvd_tc: false,
                        rcvd_tcn: false,
                        rcvd_tc_ack: false,
                        tc_prop: false,
                        fdb_flush: true,
                        message: None,
                        last_message: None,
                        received_info: None,
                    },
                )
            })
            .collect();
        Self {
            id,
            times,
            root,
            root_times: times,
            root_port: 0,
            hold,
            pti: Pti::ONE_SECOND,
            prs: Prs::INIT_TREE,
            ports,
        }
    }
    pub fn plan(&self, input: Input, max_steps: usize) -> Result<Delta, String> {
        let mut d = Delta {
            state: self.clone(),
            tx: vec![],
            flush: vec![],
            transitions: vec![],
        };
        match input {
            Input::Initialize => {
                for (name, p) in &self.ports {
                    for (machine, state) in [
                        ("PRX", format!("{:?}", p.prx)),
                        ("PPM", format!("{:?}", p.ppm)),
                        ("BDM", format!("{:?}", p.bdm)),
                        ("PTX", format!("{:?}", p.ptx)),
                        ("PIM", format!("{:?}", p.pim)),
                        ("PRT", format!("{:?}", p.prt)),
                        ("PST", format!("{:?}", p.pst)),
                        ("TCM", format!("{:?}", p.tcm)),
                    ] {
                        d.journal(name, machine, state.clone(), state, "BEGIN", p.clone());
                    }
                }
                d.transitions.push(Transition {
                    port: None,
                    machine: "PRS".into(),
                    from_state: "INIT_TREE".into(),
                    to_state: "INIT_TREE".into(),
                    guard: "BEGIN".into(),
                    before: serde_json::json!({"root":self.root,"root_times":self.root_times,"root_port":self.root_port}),
                    after: serde_json::json!({"root":d.state.root,"root_times":d.state.root_times,"root_port":d.state.root_port}),
                    tree_before:None,tree_after:None,
                });
            }
            Input::Tick => {
                d.state.pti = Pti::TICK;
                for p in d.state.ports.values_mut() {
                    p.counters.tick();
                }
                d.state.pti = Pti::ONE_SECOND;
                for (name, before) in &self.ports {
                    d.journal(
                        name,
                        "PTI",
                        "ONE_SECOND".into(),
                        "TICK".into(),
                        "PTI_TICK",
                        before.clone(),
                    );
                    d.journal(
                        name,
                        "PTI",
                        "TICK".into(),
                        "ONE_SECOND".into(),
                        "UCT",
                        d.state.ports[name].clone(),
                    );
                }
            }
            Input::Link(n, up) => {
                d.state
                    .ports
                    .get_mut(&n)
                    .ok_or("unknown RSTP port")?
                    .enabled = up;
            }
            Input::Receive(n, m) => {
                let p = d.state.ports.get_mut(&n).ok_or("unknown RSTP port")?;
                if !p.enabled {
                    return Ok(d);
                }
                p.prx = Prx::RECEIVE;
                p.rcvd_stp |= m.kind == Kind::Config || m.kind == Kind::Tcn && m.version <= 1;
                p.rcvd_rstp |= m.kind == Kind::Rst;
                p.oper_edge = false;
                p.isolate = false;
                p.counters.edge = 3;
                p.last_message = Some(m.clone());
                p.message = Some(m);
                d.journal(
                    &n,
                    "PRX",
                    format!("{:?}", self.ports[&n].prx),
                    "RECEIVE".into(),
                    "PRX_RECEIVE",
                    self.ports[&n].clone(),
                );
            }
        }
        for _ in 0..max_steps {
            // PRS per-port rows observe writes by the same bridge entry; they
            // are published but do not execute another state-machine entry.
            if d.transitions
                .iter()
                .filter(|t| !(t.machine == "PRS" && t.port.is_some()))
                .count()
                > max_steps
            {
                return Err("RSTP max_control_steps exceeded".into());
            }
            if d.step()? {
                continue;
            }
            return Ok(d);
        }
        Err("RSTP max_control_steps exceeded".into())
    }
}
impl Delta {
    fn journal(
        &mut self,
        name: &str,
        machine: &str,
        from: String,
        to: String,
        guard: &str,
        before: Port,
    ) {
        let after = self.state.ports[name].clone();
        self.transitions.push(Transition {
            port: Some(name.into()),
            machine: machine.into(),
            from_state: from.clone(),
            to_state: to.clone(),
            guard: guards::rstp(machine, &from, &to, guard, Some(&before)),
            before: serde_json::to_value(before)
                .map_err(|e| e.to_string())
                .unwrap(),
            tree_before: None,
            tree_after: None,
            after: serde_json::to_value(after)
                .map_err(|e| e.to_string())
                .unwrap(),
        });
    }
    fn step(&mut self) -> Result<bool, String> {
        let names: Vec<_> = self.state.ports.keys().cloned().collect();
        // Global reset, receive/migration/edge, then information.
        for name in &names {
            let before = self.state.ports[name].clone();
            let p = self.state.ports.get_mut(name).unwrap();
            if !p.enabled {
                if p.prx != Prx::DISCARD
                    || p.rcvd_stp
                    || p.rcvd_rstp
                    || p.message.is_some()
                    || p.counters.edge != 3
                {
                    p.prx = Prx::DISCARD;
                    p.rcvd_stp = false;
                    p.rcvd_rstp = false;
                    p.message = None;
                    p.counters.edge = 3;
                    self.journal(
                        name,
                        "PRX",
                        format!("{:?}", before.prx),
                        "DISCARD".into(),
                        "PRX_DOWN",
                        before,
                    );
                    return Ok(true);
                }
                if p.ptx != Ptx::TRANSMIT_INIT || !p.new_info || p.counters.tx != 0 {
                    p.ptx = Ptx::TRANSMIT_INIT;
                    p.new_info = true;
                    p.counters.tx = 0;
                    self.journal(
                        name,
                        "PTX",
                        format!("{:?}", before.ptx),
                        "TRANSMIT_INIT".into(),
                        "PTX_DOWN",
                        before,
                    );
                    return Ok(true);
                }
                if p.info != Info::Disabled || p.message.is_some() {
                    p.info = Info::Disabled;
                    p.pim = Pim::DISABLED;
                    p.message = None;
                    p.proposing = false;
                    p.proposed = false;
                    p.agree = false;
                    p.agreed = false;
                    p.counters.info = 0;
                    p.reselect = true;
                    p.selected = false;
                    self.journal(
                        name,
                        "PIM",
                        format!("{:?}", before.pim),
                        "DISABLED".into(),
                        "PIM_GLOBAL_DOWN",
                        before,
                    );
                    return Ok(true);
                }
            }
            let ppm = p.ppm;
            match p.ppm {
                Ppm::CHECKING_RSTP if !p.enabled && p.counters.migration != 3 => {
                    p.mcheck = false;
                    p.send_rstp = true;
                    p.counters.migration = 3;
                }
                Ppm::CHECKING_RSTP if p.counters.migration == 0 => {
                    p.ppm = Ppm::SENSING;
                    p.rcvd_rstp = false;
                    p.rcvd_stp = false;
                }
                Ppm::SENSING if !p.enabled || p.mcheck || !p.send_rstp && p.rcvd_rstp => {
                    p.ppm = Ppm::CHECKING_RSTP;
                    p.mcheck = false;
                    p.send_rstp = true;
                    p.counters.migration = 3;
                }
                Ppm::SENSING if p.send_rstp && p.rcvd_stp => {
                    p.ppm = Ppm::SELECTING_STP;
                    p.send_rstp = false;
                    p.counters.migration = 3;
                }
                Ppm::SELECTING_STP if p.counters.migration == 0 || !p.enabled || p.mcheck => {
                    p.ppm = Ppm::SENSING;
                    p.rcvd_rstp = false;
                    p.rcvd_stp = false;
                }
                _ => {}
            }
            if ppm != p.ppm || before.counters.migration != p.counters.migration {
                let to = format!("{:?}", p.ppm);
                self.journal(name, "PPM", format!("{ppm:?}"), to, "PPM/rule/7", before);
                return Ok(true);
            }
            let old = p.bdm;
            match p.bdm {
                Bdm::NOT_EDGE
                    if !p.enabled && p.config.admin_edge
                        || p.counters.edge == 0
                            && p.config.auto_edge
                            && p.send_rstp
                            && p.proposing =>
                {
                    p.bdm = Bdm::EDGE;
                    p.oper_edge = true;
                    p.isolate = false;
                }
                Bdm::NOT_EDGE
                    if p.counters.edge == 0
                        && !p.config.admin_edge
                        && !p.config.auto_edge
                        && p.send_rstp
                        && p.proposing =>
                {
                    p.bdm = Bdm::ISOLATED;
                    p.oper_edge = false;
                    p.isolate = true;
                }
                Bdm::EDGE
                    if (!p.enabled || !p.config.auto_edge) && !p.config.admin_edge
                        || !p.oper_edge =>
                {
                    p.bdm = Bdm::NOT_EDGE;
                    p.oper_edge = false;
                    p.isolate = false;
                }
                Bdm::ISOLATED if p.config.admin_edge || p.config.auto_edge || !p.isolate => {
                    p.bdm = Bdm::NOT_EDGE;
                    p.oper_edge = false;
                    p.isolate = false;
                }
                _ => {}
            }
            if old != p.bdm {
                let to = format!("{:?}", p.bdm);
                self.journal(name, "BDM", format!("{old:?}"), to, "BDM/rule/7", before);
                return Ok(true);
            }
            if !p.enabled {
                continue;
            }
            let old = p.pim;
            match p.pim {
                Pim::DISABLED => {
                    p.pim = Pim::AGED;
                    p.info = Info::Aged;
                    p.reselect = true;
                    p.selected = false;
                }
                Pim::AGED | Pim::CURRENT if p.selected && p.update => {
                    p.pim = Pim::UPDATE;
                    p.proposing = false;
                    p.proposed = false;
                    p.agreed &= p.info == Info::Mine && p.designated <= p.priority;
                    p.synced &= p.agreed;
                    p.priority = p.designated;
                    p.times = p.designated_times;
                    p.update = false;
                    p.info = Info::Mine;
                    p.new_info = true;
                }
                Pim::UPDATE
                | Pim::SUPERIOR_DESIGNATED
                | Pim::REPEATED_DESIGNATED
                | Pim::INFERIOR_DESIGNATED
                | Pim::NOT_DESIGNATED
                | Pim::OTHER => p.pim = Pim::CURRENT,
                Pim::CURRENT if p.message.is_some() && !p.update => {
                    p.received_info = p.message.as_ref().map(|m| classify(p, m));
                    p.pim = Pim::RECEIVE;
                }
                Pim::CURRENT
                    if p.info == Info::Received
                        && p.counters.info == 0
                        && !p.update
                        && p.message.is_none() =>
                {
                    p.pim = Pim::AGED;
                    p.info = Info::Aged;
                    p.reselect = true;
                    p.selected = false;
                }
                Pim::RECEIVE => {
                    let m = p.message.take().ok_or("PIM RECEIVE without message")?;
                    let received = classify(p, &m);
                    p.received_info = Some(received);
                    if m.kind == Kind::Tcn {
                        p.rcvd_tcn = true;
                        p.pim = Pim::OTHER;
                    } else if matches!(
                        received,
                        ReceivedInfo::SuperiorDesignatedInfo
                            | ReceivedInfo::RepeatedDesignatedInfo
                            | ReceivedInfo::InferiorDesignatedInfo
                    ) {
                        let superior = received == ReceivedInfo::SuperiorDesignatedInfo;
                        if superior || received == ReceivedInfo::RepeatedDesignatedInfo {
                            record_designated(p, &m, superior);
                        } else {
                            p.pim = Pim::INFERIOR_DESIGNATED;
                            if m.flags & 16 != 0 {
                                p.disputed = true;
                                p.agreed = false;
                            }
                        }
                    } else if received == ReceivedInfo::NotDesignatedInfo {
                        p.pim = Pim::NOT_DESIGNATED;
                        p.agreed = m.kind == Kind::Rst && m.flags & 64 != 0;
                        if p.agreed {
                            p.proposing = false;
                        }
                        p.rcvd_tc |= m.flags & 1 != 0;
                    } else {
                        p.pim = Pim::OTHER;
                    }
                }
                _ => {}
            }
            if old != p.pim {
                let to = format!("{:?}", p.pim);
                self.journal(name, "PIM", format!("{old:?}"), to, "PIM/rule/6", before);
                return Ok(true);
            }
        }
        if self.state.prs == Prs::INIT_TREE || self.state.ports.values().any(|p| p.reselect) {
            self.select()?;
            return Ok(true);
        }
        for name in &names {
            if self.role_step(name)? {
                return Ok(true);
            }
        }
        for name in &names {
            let before = self.state.ports[name].clone();
            let p = self.state.ports.get_mut(name).unwrap();
            let old = p.pst;
            match p.pst {
                Pst::DISCARDING if p.learn => {
                    p.pst = Pst::LEARNING;
                    p.learning = true;
                }
                Pst::LEARNING if !p.learn => {
                    p.pst = Pst::DISCARDING;
                    p.learning = false;
                    p.forwarding = false;
                }
                Pst::LEARNING if p.forward => {
                    p.pst = Pst::FORWARDING;
                    p.forwarding = true;
                }
                Pst::FORWARDING if !p.forward => {
                    p.pst = Pst::DISCARDING;
                    p.learning = false;
                    p.forwarding = false;
                }
                _ => {}
            }
            if old != p.pst {
                let to = format!("{:?}", p.pst);
                self.journal(name, "PST", format!("{old:?}"), to, "PST/rule/8", before);
                return Ok(true);
            }
            if p.fdb_flush {
                self.flush.push(name.clone());
                let completed_before = p.clone();
                let state = format!("{:?}", p.tcm);
                p.fdb_flush = false;
                self.journal(
                    name,
                    "TCM",
                    state.clone(),
                    state,
                    "FDB_COMPLETE",
                    completed_before,
                );
                return Ok(true);
            }
        }
        for name in &names {
            if self.tc_step(name) {
                return Ok(true);
            }
        }
        for name in &names {
            if self.tx_step(name) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn select(&mut self) -> Result<(), String> {
        let previous_ports = self.state.ports.clone();
        let previous = serde_json::json!({"root":self.state.root,"root_port":self.state.root_port,"root_times":self.state.root_times});
        let from = format!("{:?}", self.state.prs);
        let b = self.state.id;
        let mut best = Priority([b, 0, b, 0, b, 0, 0]);
        let mut root = 0;
        let mut times = self.state.times;
        for p in self.state.ports.values() {
            if p.info != Info::Received
                || p.config.restricted_role
                || p.priority.0[4] & 0x0000ffffffffffff == b & 0x0000ffffffffffff
            {
                continue;
            }
            let mut v = p.priority;
            v.0[1] = v.0[1]
                .checked_add(p.config.cost as u64)
                .filter(|v| *v <= u32::MAX as u64)
                .ok_or("RSTP path cost overflow")?;
            v.0[2] = b;
            if v < best {
                best = v;
                root = p.config.id;
                times = p.times;
                let age = received_age_seconds(times.age);
                times.age = u16::try_from(age * 256).map_err(|_| "RSTP message age overflow")?;
            }
        }
        self.state.root = best;
        self.state.root_port = root;
        self.state.root_times = times;
        self.state.prs = Prs::ROLE_SELECTION;
        for p in self.state.ports.values_mut() {
            p.reselect = false;
            p.designated = best;
            p.designated.0[2] = b;
            p.designated.0[4] = b;
            p.designated.0[5] = p.config.id as u64;
            p.designated.0[6] = p.config.id as u64;
            p.designated_times = times;
            p.selected_role = match p.info {
                Info::Disabled => Role::Disabled,
                Info::Aged => {
                    p.update = true;
                    Role::Designated
                }
                Info::Mine => {
                    p.update = p.priority != p.designated || p.times != times;
                    Role::Designated
                }
                Info::Received if p.config.id == root => {
                    p.update = false;
                    Role::Root
                }
                Info::Received if p.designated < p.priority => {
                    p.update = true;
                    Role::Designated
                }
                Info::Received => {
                    p.update = false;
                    if p.priority.0[4] & 0x0000ffffffffffff == b & 0x0000ffffffffffff {
                        Role::Backup
                    } else {
                        Role::Alternate
                    }
                }
            };
            p.selected = true;
        }
        self.transitions.push(Transition{port:None,machine:"PRS".into(),from_state:from.clone(),to_state:"ROLE_SELECTION".into(),guard:guards::rstp("PRS",&from,"ROLE_SELECTION","PRS_RESELECT",None),tree_before:None,tree_after:None,before:previous,after:serde_json::json!({"root":self.state.root,"root_port":self.state.root_port,"root_times":self.state.root_times})});
        for (name, before) in previous_ports {
            if before != self.state.ports[&name] {
                self.journal(
                    &name,
                    "PRS",
                    from.clone(),
                    "ROLE_SELECTION".into(),
                    "PRS_RESELECT",
                    before,
                );
            }
        }
        Ok(())
    }
    fn role_step(&mut self, name: &str) -> Result<bool, String> {
        let tree_before = self.state.clone();
        let before = self.state.ports[name].clone();
        let role = before.role;
        let all_ready = self
            .state
            .ports
            .values()
            .all(|q| q.selected && q.role == q.selected_role && !q.update);
        let all_synced = all_ready
            && self.state.ports.iter().all(|(n, q)| {
                if matches!(role, Role::Root | Role::Alternate) {
                    q.role == Role::Root || q.synced
                } else if role == Role::Designated {
                    n == name || q.synced
                } else {
                    false
                }
            });
        let rerooted = self
            .state
            .ports
            .iter()
            .all(|(n, q)| n == name || q.counters.rr == 0);
        let p = self.state.ports.get_mut(name).unwrap();
        let old = p.prt;
        let fd = ceil(p.designated_times.forward_delay);
        let f_d = if p.send_rstp { 2 } else { fd };
        let ma = ceil(p.designated_times.max_age);
        let mut sync_tree = false;
        let mut reroot_tree = false;
        if p.prt == Prt::INIT_PORT {
            p.prt = Prt::DISABLE_PORT;
        } else if p.prt == Prt::DISABLE_PORT && !p.learning && !p.forwarding {
            p.prt = Prt::DISABLED_PORT;
            p.counters.fd = ma;
            p.synced = true;
            p.counters.rr = 0;
            p.sync = false;
            p.reroot = false;
        } else if p.prt == Prt::BLOCK_PORT && !p.learning && !p.forwarding {
            p.prt = Prt::ALTERNATE_PORT;
            p.counters.fd = f_d;
            p.synced = true;
            p.counters.rr = 0;
            p.sync = false;
            p.reroot = false;
        } else if !matches!(
            p.prt,
            Prt::DISABLE_PORT
                | Prt::DISABLED_PORT
                | Prt::ROOT_PORT
                | Prt::DESIGNATED_PORT
                | Prt::BLOCK_PORT
                | Prt::ALTERNATE_PORT
        ) {
            p.prt = match p.role {
                Role::Root => Prt::ROOT_PORT,
                Role::Designated => Prt::DESIGNATED_PORT,
                Role::Alternate | Role::Backup => Prt::ALTERNATE_PORT,
                Role::Disabled => Prt::DISABLED_PORT,
            };
        } else if !p.selected || p.update {
            return Ok(false);
        } else if p.role != p.selected_role {
            p.role = p.selected_role;
            match p.role {
                Role::Disabled => {
                    p.prt = Prt::DISABLE_PORT;
                    p.learn = false;
                    p.forward = false;
                }
                Role::Root => {
                    p.prt = Prt::ROOT_PORT;
                    p.counters.rr = fd;
                }
                Role::Designated => {
                    p.prt = Prt::DESIGNATED_PORT;
                    p.proposing |=
                        !p.config.admin_edge && !p.config.auto_edge && p.config.auto_isolate;
                }
                Role::Alternate | Role::Backup => {
                    p.prt = Prt::BLOCK_PORT;
                    p.learn = false;
                    p.forward = false;
                }
            }
        } else {
            match p.prt {
                Prt::DISABLED_PORT if p.counters.fd != ma || p.sync || p.reroot || !p.synced => {
                    p.counters.fd = ma;
                    p.synced = true;
                    p.counters.rr = 0;
                    p.sync = false;
                    p.reroot = false;
                }
                Prt::ROOT_PORT => {
                    if p.counters.rr != fd {
                        p.counters.rr = fd;
                    } else if p.disputed {
                        p.prt = Prt::ROOT_DISCARD;
                        p.counters.rb = 6;
                        p.learn = false;
                        p.forward = false;
                        p.disputed = false;
                        p.counters.fd = fd;
                    } else if p.proposed && !p.agree {
                        p.prt = Prt::ROOT_PROPOSED;
                        sync_tree = true;
                        p.proposed = false;
                    } else if all_synced && !p.agree || p.proposed && p.agree {
                        p.prt = Prt::ROOT_AGREED;
                        p.proposed = false;
                        p.sync = false;
                        p.agree = true;
                        p.new_info = true;
                    } else if p.agreed && !p.synced || p.sync && p.synced {
                        p.prt = Prt::ROOT_SYNCED;
                        p.synced = true;
                        p.sync = false;
                    } else if !p.forward && p.counters.rb == 0 && !p.reroot {
                        p.prt = Prt::REROOT;
                        reroot_tree = true;
                    } else if p.reroot && p.forward {
                        p.prt = Prt::REROOTED;
                        p.reroot = false;
                    } else if (p.counters.fd == 0 || rerooted && p.counters.rb == 0) && !p.learn {
                        p.prt = Prt::ROOT_LEARN;
                        p.counters.fd = f_d;
                        p.learn = true;
                    } else if (p.counters.fd == 0 || rerooted && p.counters.rb == 0)
                        && p.learn
                        && !p.forward
                    {
                        p.prt = Prt::ROOT_FORWARD;
                        p.counters.fd = 0;
                        p.forward = true;
                    }
                }
                Prt::DESIGNATED_PORT => {
                    if (p.sync && !p.synced
                        || p.reroot && p.counters.rr != 0
                        || p.disputed
                        || p.isolate)
                        && !p.oper_edge
                        && (p.learn || p.forward)
                    {
                        p.prt = Prt::DESIGNATED_DISCARD;
                        p.learn = false;
                        p.forward = false;
                        p.disputed = false;
                        p.counters.fd = f_d;
                    } else if !p.forward && !p.agreed && !p.proposing && !p.oper_edge {
                        p.prt = Prt::DESIGNATED_PROPOSE;
                        p.proposing = true;
                        p.counters.edge = 3;
                        p.new_info = true;
                    } else if all_synced && (p.proposed || !p.agree) {
                        p.prt = Prt::DESIGNATED_AGREE;
                        p.proposed = false;
                        p.sync = false;
                        p.agree = true;
                        p.new_info = true;
                    } else if !p.learning && !p.forwarding && !p.synced
                        || p.agreed && !p.synced
                        || p.oper_edge && !p.synced
                        || p.sync && p.synced
                    {
                        p.prt = Prt::DESIGNATED_SYNCED;
                        p.counters.rr = 0;
                        p.synced = true;
                        p.sync = false;
                    } else if p.reroot && p.counters.rr == 0 {
                        p.prt = Prt::DESIGNATED_RETIRED;
                        p.reroot = false;
                    } else if (p.counters.fd == 0 || p.agreed || p.oper_edge)
                        && (p.counters.rr == 0 || !p.reroot)
                        && !p.sync
                        && !p.isolate
                    {
                        if !p.learn {
                            p.prt = Prt::DESIGNATED_LEARN;
                            p.learn = true;
                            p.counters.fd = f_d;
                        } else if !p.forward {
                            p.prt = Prt::DESIGNATED_FORWARD;
                            p.forward = true;
                            p.counters.fd = 0;
                            p.agreed = p.send_rstp;
                        }
                    }
                }
                Prt::ALTERNATE_PORT => {
                    if p.counters.fd != f_d || p.sync || p.reroot || !p.synced {
                        p.counters.fd = f_d;
                        p.synced = true;
                        p.counters.rr = 0;
                        p.sync = false;
                        p.reroot = false;
                    } else if p.proposed && !p.agree {
                        p.prt = Prt::ALTERNATE_PROPOSED;
                        sync_tree = true;
                        p.proposed = false;
                    } else if all_synced && !p.agree || p.proposed && p.agree {
                        p.prt = Prt::ALTERNATE_AGREED;
                        p.proposed = false;
                        p.agree = true;
                        p.new_info = true;
                    } else if p.role == Role::Backup && p.counters.rb != 4 {
                        p.prt = Prt::BACKUP_PORT;
                        p.counters.rb = 4;
                    }
                }
                _ => {}
            }
        }
        let changed = *p != before;
        let to = format!("{:?}", p.prt);
        if sync_tree {
            for q in self.state.ports.values_mut() {
                q.sync = true;
            }
        }
        if reroot_tree {
            for q in self.state.ports.values_mut() {
                q.reroot = true;
            }
        }
        if changed || sync_tree || reroot_tree {
            self.journal(name, "PRT", format!("{old:?}"), to, "PRT/rule/8", before);
            let tree_after = serde_json::to_value(&self.state).unwrap();
            let t = self.transitions.last_mut().unwrap();
            t.tree_before = Some(serde_json::to_value(tree_before).unwrap());
            t.tree_after = Some(tree_after);
            return Ok(true);
        }
        Ok(false)
    }
    #[allow(clippy::nonminimal_bool)] // Keep the adopted state-table guard readable verbatim.
    fn tc_step(&mut self, name: &str) -> bool {
        let tree_before = self.state.clone();
        let before = self.state.ports[name].clone();
        let p = self.state.ports.get_mut(name).unwrap();
        let old = p.tcm;
        let active = matches!(p.role, Role::Root | Role::Designated);
        let mut propagate = false;
        let mut start = false;
        let clear = |p: &mut Port| {
            p.rcvd_tc = false;
            p.rcvd_tcn = false;
            p.rcvd_tc_ack = false;
            p.tc_prop = false;
        };
        match p.tcm {
            Tcm::INACTIVE if p.learn && !p.fdb_flush => {
                p.tcm = Tcm::LEARNING;
                clear(p);
            }
            Tcm::LEARNING
                if !active
                    && !(p.learn || p.learning)
                    && !(p.rcvd_tc || p.rcvd_tcn || p.rcvd_tc_ack || p.tc_prop) =>
            {
                p.tcm = Tcm::INACTIVE;
                p.fdb_flush = true;
                p.counters.detected = 0;
                p.counters.tc = 0;
                p.tc_ack = false;
            }
            Tcm::LEARNING if p.rcvd_tc || p.rcvd_tcn || p.rcvd_tc_ack || p.tc_prop => clear(p),
            Tcm::LEARNING if active && p.forward && !p.oper_edge => {
                p.tcm = Tcm::DETECTED;
                start = true;
                propagate = true;
                p.new_info = true;
                if p.counters.detected == 0 {
                    p.counters.detected = if p.send_rstp {
                        3
                    } else {
                        (p.designated_times.max_age as u32
                            + p.designated_times.forward_delay as u32)
                            .div_ceil(256)
                    };
                }
            }
            Tcm::DETECTED | Tcm::NOTIFIED_TC | Tcm::PROPAGATING | Tcm::ACKNOWLEDGED => {
                p.tcm = Tcm::ACTIVE
            }
            Tcm::ACTIVE if !active || p.oper_edge => {
                p.tcm = Tcm::LEARNING;
                clear(p);
            }
            Tcm::ACTIVE if p.rcvd_tcn => {
                p.tcm = Tcm::NOTIFIED_TCN;
                start = true;
            }
            Tcm::NOTIFIED_TCN => {
                p.tcm = Tcm::NOTIFIED_TC;
                p.rcvd_tcn = false;
                p.rcvd_tc = false;
                p.tc_ack |= p.role == Role::Designated;
                propagate = true;
            }
            Tcm::ACTIVE if p.rcvd_tc => {
                p.tcm = Tcm::NOTIFIED_TC;
                p.rcvd_tcn = false;
                p.rcvd_tc = false;
                p.tc_ack |= p.role == Role::Designated;
                propagate = true;
            }
            Tcm::ACTIVE if p.tc_prop && !p.oper_edge => {
                p.tcm = Tcm::PROPAGATING;
                start = true;
                p.fdb_flush = true;
                p.tc_prop = false;
            }
            Tcm::ACTIVE if p.rcvd_tc_ack => {
                p.tcm = Tcm::ACKNOWLEDGED;
                p.counters.tc = 0;
                p.rcvd_tc_ack = false;
            }
            _ => {}
        }
        if start && p.counters.tc == 0 {
            p.counters.tc = if p.send_rstp {
                p.new_info = true;
                3
            } else {
                (p.designated_times.max_age as u32 + p.designated_times.forward_delay as u32)
                    .div_ceil(256)
            };
        }
        let changed = *p != before;
        propagate &= !p.config.restricted_tcn;
        let to = format!("{:?}", p.tcm);
        if propagate {
            for (n, p) in &mut self.state.ports {
                if n != name {
                    p.tc_prop = true;
                }
            }
        }
        if changed {
            self.journal(name, "TCM", format!("{old:?}"), to, "TCM/rule/9", before);
            let tree_after = serde_json::to_value(&self.state).unwrap();
            let t = self.transitions.last_mut().unwrap();
            t.tree_before = Some(serde_json::to_value(tree_before).unwrap());
            t.tree_after = Some(tree_after);
        }
        changed
    }
    fn tx_step(&mut self, name: &str) -> bool {
        let before = self.state.ports[name].clone();
        let p = self.state.ports.get_mut(name).unwrap();
        let old = p.ptx;
        if !p.enabled {
            return false;
        }
        if matches!(
            p.ptx,
            Ptx::TRANSMIT_INIT
                | Ptx::TRANSMIT_PERIODIC
                | Ptx::TRANSMIT_CONFIG
                | Ptx::TRANSMIT_TCN
                | Ptx::TRANSMIT_RSTP
        ) {
            p.ptx = Ptx::IDLE;
            p.counters.hello = 2;
        } else if !p.selected || p.update {
            return false;
        } else if p.counters.hello == 0 {
            p.ptx = Ptx::TRANSMIT_PERIODIC;
            p.new_info |= p.role == Role::Designated || p.role == Role::Root && p.counters.tc != 0;
        } else if p.new_info && p.counters.tx < self.state.hold && p.role != Role::Disabled {
            let kind = if p.send_rstp {
                Kind::Rst
            } else if p.role == Role::Root {
                Kind::Tcn
            } else if p.role == Role::Designated {
                Kind::Config
            } else {
                return false;
            };
            p.ptx = match kind {
                Kind::Rst => Ptx::TRANSMIT_RSTP,
                Kind::Config => Ptx::TRANSMIT_CONFIG,
                Kind::Tcn => Ptx::TRANSMIT_TCN,
            };
            let flags = if kind == Kind::Config {
                u8::from(p.counters.tc != 0) | (u8::from(p.tc_ack) * 128)
            } else {
                u8::from(p.counters.tc != 0)
                    | (u8::from(p.proposing) * 2)
                    | match p.role {
                        Role::Root => 8,
                        Role::Designated => 12,
                        Role::Alternate | Role::Backup => 4,
                        _ => 0,
                    }
                    | (u8::from(p.learning) * 16)
                    | (u8::from(p.forwarding) * 32)
                    | (u8::from(p.agree) * 64)
            };
            let mut times = p.designated_times;
            times.hello = p.times.hello;
            self.tx.push((
                name.into(),
                Bpdu {
                    kind,
                    version: if kind == Kind::Rst { 2 } else { 0 },
                    flags,
                    priority: p.designated,
                    times,
                },
            ));
            p.new_info = false;
            p.counters.tx += 1;
            p.tc_ack = false;
        }
        if old != p.ptx {
            let to = format!("{:?}", p.ptx);
            self.journal(name, "PTX", format!("{old:?}"), to, "PTX/rule/7", before);
            true
        } else {
            false
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn bridge(id: u64) -> Bridge {
        Bridge::new(
            id,
            Times::default(),
            6,
            BTreeMap::from([(
                "p".into(),
                (
                    PortConfig {
                        id: 0x8001,
                        cost: 20000,
                        admin_edge: false,
                        auto_edge: true,
                        auto_isolate: false,
                        restricted_role: false,
                        restricted_tcn: false,
                    },
                    true,
                ),
            )]),
        )
    }
    #[test]
    fn codec() {
        let mut b = bridge(0x8000000000000001);
        let d = b.plan(Input::Initialize, 1000).unwrap();
        assert!(!d.tx.is_empty());
        b = d.state;
        let m = &b.ports["p"];
        let packet = Bpdu {
            kind: Kind::Rst,
            version: 2,
            flags: 12,
            priority: m.designated,
            times: m.times,
        };
        let raw = encode([2, 0, 0, 0, 0, 1], &packet);
        assert_eq!(raw.len(), 64);
        let decoded = decode(&raw, 0x8001, 1500).unwrap();
        assert_eq!(decoded, packet);
    }
    #[test]
    fn two_bridge_handshake() {
        let a = bridge(0x8000000000000001)
            .plan(Input::Initialize, 1000)
            .unwrap();
        let b = bridge(0x8000000000000002)
            .plan(Input::Initialize, 1000)
            .unwrap();
        let receive = b
            .state
            .plan(
                Input::Receive("p".into(), a.tx.last().unwrap().1.clone()),
                1000,
            )
            .unwrap();
        assert_eq!(receive.state.ports["p"].role, Role::Root);
        assert_eq!(receive.state.root.0[0], a.state.id);
    }
    #[test]
    fn tick_autoedge() {
        let mut b = bridge(0x8000000000000001)
            .plan(Input::Initialize, 1000)
            .unwrap()
            .state;
        for _ in 0..3 {
            b = b.plan(Input::Tick, 1000).unwrap().state;
        }
        assert!(b.ports["p"].oper_edge);
        assert!(b.ports["p"].forwarding);
    }
}
#[cfg(test)]
mod closure_tests {
    use super::*;
    fn bridge() -> Bridge {
        Bridge::new(
            0x8000020000000002,
            Times::default(),
            6,
            (1..=2)
                .map(|id| {
                    (
                        format!("p{id}"),
                        (
                            PortConfig {
                                id: 0x8000 + id,
                                cost: 20000,
                                admin_edge: false,
                                auto_edge: true,
                                auto_isolate: false,
                                restricted_role: false,
                                restricted_tcn: false,
                            },
                            true,
                        ),
                    )
                })
                .collect(),
        )
    }
    #[test]
    fn supplied_rcv_info_vectors() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/proposals/fixtures/rstp-standard-v1.json"
        )))
        .unwrap();
        let mut count = 0;
        for row in fixture["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["input"]["operation"] == "rcvInfo")
        {
            let input = &row["input"];
            let mut state = bridge();
            let p = state.ports.get_mut("p1").unwrap();
            p.enabled = true;
            p.ppm = Ppm::SENSING;
            p.bdm = Bdm::NOT_EDGE;
            p.proposing = false;
            p.pim = Pim::RECEIVE;
            p.info = if input["infoIs"] == "Received" {
                Info::Received
            } else {
                Info::Disabled
            };
            let mut message = Bpdu {
                kind: Kind::Rst,
                version: 2,
                flags: 12,
                priority: p.priority,
                times: p.times,
            };
            if input["msg_worse_than_port"] == true {
                message.priority.0[0] += 1;
            }
            if input["same_sender"] == false {
                message.priority.0[4] += 1;
            }
            if input["learning_bit"] == true {
                message.flags |= 16;
            }
            p.message = Some(message);
            let mut delta = Delta {
                state,
                tx: vec![],
                flush: vec![],
                transitions: vec![],
            };
            assert!(delta.step().unwrap());
            let p = &delta.state.ports["p1"];
            assert_eq!(
                serde_json::to_value(p.received_info.unwrap()).unwrap(),
                row["expected"]["rcvdInfo"],
                "{}",
                row["id"]
            );
            if row["expected"].get("repeated_branch_reachable").is_some() {
                assert_ne!(p.pim, Pim::REPEATED_DESIGNATED, "{}", row["id"]);
            }
            if let Some(effects) = row["expected"].get("entry_effects") {
                assert_eq!(serde_json::json!(p.disputed), effects["disputed"]);
                assert_eq!(serde_json::json!(p.agreed), effects["agreed"]);
            }
            super::supplied_vectors::receipt_rcv_info(row, p);
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn consumed_message_is_retained_in_snapshot() {
        let state = bridge().plan(Input::Initialize, 10000).unwrap().state;
        let message = Bpdu {
            kind: Kind::Rst,
            version: 2,
            flags: 12,
            priority: Priority([1, 0, 1, 0, 1, 0x8001, 0x8001]),
            times: Times::default(),
        };
        let next = state
            .plan(Input::Receive("p1".into(), message.clone()), 10000)
            .unwrap()
            .state;
        assert_eq!(next.ports["p1"].last_message.as_ref(), Some(&message));
        assert!(next.ports["p1"].message.is_none());
        assert_eq!(
            next.ports["p1"].received_info,
            Some(ReceivedInfo::SuperiorDesignatedInfo)
        );
    }

    #[test]
    fn legacy_migration_and_return_to_rstp() {
        let mut b = bridge().plan(Input::Initialize, 10000).unwrap().state;
        for _ in 0..3 {
            b = b.plan(Input::Tick, 10000).unwrap().state;
        }
        assert_eq!(b.ports["p1"].ppm, Ppm::SENSING);
        let msg = Bpdu {
            kind: Kind::Config,
            version: 0,
            flags: 12,
            priority: Priority([
                0x8000020000000001,
                0,
                0x8000020000000001,
                0,
                0x8000020000000001,
                0x8001,
                0x8001,
            ]),
            times: Times::default(),
        };
        let d = b
            .plan(Input::Receive("p1".into(), msg.clone()), 10000)
            .unwrap();
        b = d.state;
        assert!(!b.ports["p1"].send_rstp);
        assert_eq!(b.ports["p1"].ppm, Ppm::SELECTING_STP);
        for _ in 0..3 {
            b = b.plan(Input::Tick, 10000).unwrap().state;
        }
        let mut rst = msg;
        rst.kind = Kind::Rst;
        rst.version = 2;
        let d = b.plan(Input::Receive("p1".into(), rst), 10000).unwrap();
        assert!(d.state.ports["p1"].send_rstp);
        assert_eq!(d.state.ports["p1"].ppm, Ppm::CHECKING_RSTP);
        assert_eq!(d.state.ports["p1"].counters.migration, 3);
    }
    #[test]
    fn own_other_port_selects_backup() {
        let b = bridge().plan(Input::Initialize, 10000).unwrap().state;
        let m = Bpdu {
            kind: Kind::Rst,
            version: 2,
            flags: 12,
            priority: Priority([b.id, 0, b.id, 0, b.id, 0x8001, 0x8002]),
            times: Times::default(),
        };
        let d = b.plan(Input::Receive("p2".into(), m), 10000).unwrap();
        let p = &d.state.ports["p2"];
        assert_eq!(p.role, Role::Backup);
        assert!(!p.learning && !p.forwarding);
        assert_eq!(p.counters.rb, 4);
        assert!(d.transitions.iter().any(|t| t.to_state == "BACKUP_PORT"));
    }
    #[test]
    fn transmit_hold_releases_one_credit_per_tick() {
        let mut b = bridge().plan(Input::Initialize, 10000).unwrap().state;
        for p in b.ports.values_mut() {
            p.counters.tx = 6;
            p.new_info = true;
            p.counters.hello = 2;
        }
        let mut d = Delta {
            state: b.clone(),
            tx: vec![],
            flush: vec![],
            transitions: vec![],
        };
        assert!(!d.tx_step("p1"));
        assert!(d.tx.is_empty());
        let d = b.plan(Input::Tick, 10000).unwrap();
        assert_eq!(d.tx.iter().filter(|(p, _)| p == "p1").count(), 1);
        assert_eq!(d.state.ports["p1"].counters.tx, 6);
    }
    #[test]
    fn legacy_learning_is_separate_from_forwarding() {
        let mut b = bridge().plan(Input::Initialize, 10000).unwrap().state;
        let p = b.ports.get_mut("p1").unwrap();
        p.prt = Prt::DESIGNATED_PORT;
        p.role = Role::Designated;
        p.selected_role = Role::Designated;
        p.selected = true;
        p.update = false;
        p.send_rstp = false;
        p.oper_edge = false;
        p.isolate = false;
        p.sync = false;
        p.synced = true;
        p.reroot = false;
        p.agree = true;
        p.proposing = true;
        p.counters.fd = 0;
        p.counters.rr = 0;
        p.learn = false;
        p.forward = false;
        p.learning = false;
        p.forwarding = false;
        let mut d = Delta {
            state: b,
            tx: vec![],
            flush: vec![],
            transitions: vec![],
        };
        d.role_step("p1").unwrap();
        assert_eq!(d.state.ports["p1"].prt, Prt::DESIGNATED_LEARN);
        assert!(d.state.ports["p1"].learn);
        assert!(!d.state.ports["p1"].forward);
        assert_eq!(d.state.ports["p1"].counters.fd, 15);
    }
}

#[cfg(test)]
#[path = "rstp_vectors.rs"]
mod supplied_vectors;
