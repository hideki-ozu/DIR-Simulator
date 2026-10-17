//! IEEE 802.1Q-2022 MVRP wire codec and pure Table 10-3/10-4 reducers.
use super::codec::*;
use serde::{Deserialize, Serialize};
pub const DA: [u8; 6] = [1, 128, 194, 0, 0, 33];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttributeEvent {
    New,
    JoinIn,
    In,
    JoinMt,
    Mt,
    Lv,
}
impl AttributeEvent {
    fn digit(self) -> u8 {
        self as u8
    }
    fn from_digit(d: u8) -> Self {
        [
            Self::New,
            Self::JoinIn,
            Self::In,
            Self::JoinMt,
            Self::Mt,
            Self::Lv,
        ][d as usize]
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vector {
    pub leave_all: bool,
    pub attributes: Vec<(u16, AttributeEvent)>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedMvrp {
    pub version: u8,
    pub vectors: Vec<Vector>,
}
pub fn decode(
    frame: &[u8],
    pvid: u16,
    max_bytes: usize,
    max_attributes: usize,
) -> DecodeResult<DecodedMvrp> {
    decode_payload(
        client(frame, DA, Some(0x88f5), max_bytes)?,
        pvid,
        max_attributes,
    )
}
pub fn decode_payload(b: &[u8], pvid: u16, limit: usize) -> DecodeResult<DecodedMvrp> {
    if !(1..=4094).contains(&pvid) {
        return Err(bad("protocol_malformed"));
    }
    let version = *b.first().ok_or_else(|| bad("message_length"))?;
    let mut p = 1;
    let mut vectors = vec![];
    let mut count = 0;
    let mut seen = false;
    while p < b.len() {
        let typ = *b.get(p).ok_or_else(|| bad("message_length"))?;
        let len = *b.get(p + 1).ok_or_else(|| bad("message_length"))? as usize;
        p += 2;
        if typ == 0 && len == 0 {
            if seen {
                return Ok(DecodedMvrp { version, vectors });
            }
            return Err(bad("empty_messages"));
        }
        let known = typ == 1 && len == 2;
        if len == 0 || (!known && version == 0) {
            return Err(bad(if typ == 1 {
                "attribute_length"
            } else {
                "unknown_attribute_same_version"
            }));
        }
        let mut message_seen = false;
        loop {
            if p == b.len() {
                if message_seen {
                    return Ok(DecodedMvrp { version, vectors });
                }
                return Err(bad("incomplete_vector"));
            }
            let h = u16be(b, p).map_err(|_| bad("partial_endmark"))?;
            p += 2;
            if h == 0 {
                if !message_seen {
                    return Err(bad("empty_attribute_list"));
                }
                break;
            }
            let la = h >> 13;
            let n = (h & 8191) as usize;
            if la > 1 || n == 0 && la != 1 {
                return Err(bad("leaveall_reserved"));
            }
            let first = b.get(p..p + len).ok_or_else(|| bad("incomplete_vector"))?;
            p += len;
            let packed = b
                .get(p..p + n.div_ceil(3))
                .ok_or_else(|| bad("incomplete_vector"))?;
            p += n.div_ceil(3);
            message_seen = true;
            seen = true;
            if !known {
                continue;
            }
            let first = u16::from_be_bytes([first[0], first[1]]) as usize;
            if n > 0 && (first > 4094 || first + n - 1 > 4094) {
                return Err(bad("vid_range"));
            }
            count += n;
            if count > limit {
                return Err(bad("attribute_limit"));
            }
            if packed.iter().any(|x| *x > 215) {
                if version > 0 {
                    continue;
                }
                return Err(bad("event_reserved"));
            }
            let mut attributes = vec![];
            for i in 0..n {
                let digit = (packed[i / 3] / [36, 6, 1][i % 3]) % 6;
                let raw = (first + i) as u16;
                attributes.push((
                    if raw == 0 { pvid } else { raw },
                    AttributeEvent::from_digit(digit),
                ));
            }
            vectors.push(Vector {
                leave_all: la == 1,
                attributes,
            });
        }
    }
    if seen {
        Ok(DecodedMvrp { version, vectors })
    } else {
        Err(bad("message_length"))
    }
}
/// Returns a canonical maximal prefix and the number of attributes consumed.
pub fn encode_payload(
    attrs: &[(u16, AttributeEvent)],
    leave_all: bool,
    capacity: usize,
) -> DecodeResult<(Vec<u8>, usize)> {
    if capacity < 11 {
        return Err(bad("capacity"));
    }
    if attrs.windows(2).any(|w| w[0].0 >= w[1].0) || attrs.iter().any(|a| a.0 > 4094) {
        return Err(bad("attribute_order"));
    }
    let mut out = vec![0, 1, 2];
    let mut used = 0;
    if leave_all {
        out.extend([0x20, 0, 0, 0]);
    }
    while used < attrs.len() {
        let mut n = 1;
        while used + n < attrs.len() && attrs[used + n].0 == attrs[used].0 + n as u16 {
            n += 1;
        }
        while n > 0 && out.len() + 4 + n.div_ceil(3) + 4 > capacity {
            n -= 1;
        }
        if n == 0 {
            break;
        }
        out.extend((n as u16).to_be_bytes());
        out.extend(attrs[used].0.to_be_bytes());
        for chunk in attrs[used..used + n].chunks(3) {
            let mut packed = 0;
            for (i, a) in chunk.iter().enumerate() {
                packed += a.1.digit() * [36, 6, 1][i];
            }
            out.push(packed);
        }
        used += n;
    }
    if used == 0 && !leave_all {
        return Err(bad("capacity"));
    }
    out.extend([0, 0, 0, 0]);
    Ok((out, used))
}
pub fn encode(
    sa: [u8; 6],
    attrs: &[(u16, AttributeEvent)],
    leave_all: bool,
    capacity: usize,
) -> DecodeResult<(Vec<u8>, usize)> {
    let (p, n) = encode_payload(attrs, leave_all, capacity)?;
    Ok((frame(DA, sa, 0x88f5, &p), n))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Applicant {
    VO,
    VP,
    VN,
    AN,
    AA,
    QA,
    LA,
    AO,
    QO,
    AP,
    QP,
    LO,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Registrar {
    IN,
    LV,
    MT,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Begin,
    New,
    Join,
    Lv,
    RNew,
    RJoinIn,
    RIn,
    RJoinMt,
    RMt,
    RLv,
    #[allow(clippy::upper_case_acronyms)] // IEEE receive-LeaveAll event spelling.
    RLA,
    Redeclare,
    Periodic,
    Tx,
    TxLA,
    TxLAF,
    Flush,
    LeaveExpired,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApplicantControl {
    Normal,
    NewOnly,
    NonParticipant,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistrarControl {
    Normal,
    FixedIgnoreNew,
    FixedPropagateNew,
    Forbidden,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicantStep {
    pub state: Applicant,
    pub action: Option<AttributeEvent>,
    pub optional: bool,
    pub request_tx: bool,
}
pub fn applicant(
    s: Applicant,
    e: Event,
    p2p: bool,
    r: Registrar,
    mode: ApplicantControl,
) -> ApplicantStep {
    use Applicant::*;
    use Event::*;
    let mut t = s;
    let mut a = None;
    let mut optional = false;
    if mode == ApplicantControl::NonParticipant
        || mode == ApplicantControl::NewOnly && !matches!(e, Begin | New | Tx)
    {
        return ApplicantStep {
            state: s,
            action: None,
            optional: false,
            request_tx: false,
        };
    }
    let sj = if r == Registrar::IN {
        AttributeEvent::JoinIn
    } else {
        AttributeEvent::JoinMt
    };
    let silence = if r == Registrar::IN {
        AttributeEvent::In
    } else {
        AttributeEvent::Mt
    };
    match e {
        Begin => t = VO,
        New => {
            if !matches!(s, VN | AN) {
                t = VN
            }
        }
        Join => {
            t = match s {
                VO | LO => VP,
                LA => AA,
                AO => AP,
                QO => QP,
                _ => s,
            }
        }
        Lv => {
            t = match s {
                VP => VO,
                VN | AN | AA | QA => LA,
                AP => AO,
                QP => QO,
                _ => s,
            }
        }
        RJoinIn => {
            t = match s {
                AA => QA,
                AO => QO,
                AP => QP,
                VO if !p2p => AO,
                VP if !p2p => AP,
                _ => s,
            }
        }
        RIn if p2p && s == AA => t = QA,
        RJoinMt | RMt => {
            t = match s {
                QA => AA,
                QO => AO,
                QP => AP,
                LO => VO,
                _ => s,
            }
        }
        RLv | RLA | Redeclare => {
            t = match s {
                VO | AO | QO => LO,
                AN => VN,
                AA | QA | AP | QP => VP,
                _ => s,
            }
        }
        Periodic => {
            t = match s {
                QA => AA,
                QP => AP,
                _ => s,
            }
        }
        Tx => {
            let pair = match s {
                VP => (AA, sj),
                VN => (AN, AttributeEvent::New),
                AN => (
                    if r == Registrar::IN || mode == ApplicantControl::NewOnly {
                        QA
                    } else {
                        AA
                    },
                    AttributeEvent::New,
                ),
                AA | AP => (QA, sj),
                LA => (VO, AttributeEvent::Lv),
                LO => (VO, silence),
                QA => {
                    optional = true;
                    (s, sj)
                }
                _ => {
                    optional = true;
                    (s, silence)
                }
            };
            t = pair.0;
            a = Some(pair.1);
        }
        TxLA => {
            let pair = match s {
                VP => (AA, silence),
                VN => (AN, AttributeEvent::New),
                AN => (QA, AttributeEvent::New),
                AA | AP | QP | QA => (QA, sj),
                _ => {
                    optional = true;
                    (LO, silence)
                }
            };
            t = pair.0;
            a = Some(pair.1);
        }
        TxLAF => {
            t = match s {
                VO | LA | AO | QO => LO,
                VP | AA | QA | AP | QP => VP,
                VN | AN => VN,
                LO => LO,
            }
        }
        _ => {}
    }
    if mode == ApplicantControl::NewOnly && a != Some(AttributeEvent::New) {
        a = None;
    }
    ApplicantStep {
        state: t,
        action: a,
        optional,
        request_tx: (s != t && matches!(t, VN | AN | AA | LA | VP | AP | LO)) || e == TxLAF,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrarStep {
    pub state: Registrar,
    pub join: Option<bool>,
    pub leave: bool,
    pub arm_leave: bool,
    pub cancel_leave: bool,
}
pub fn registrar(s: Registrar, e: Event, p2p: bool, mode: RegistrarControl) -> RegistrarStep {
    use Event::*;
    use Registrar::*;
    let mut d = RegistrarStep {
        state: s,
        join: None,
        leave: false,
        arm_leave: false,
        cancel_leave: false,
    };
    match mode {
        RegistrarControl::FixedIgnoreNew | RegistrarControl::FixedPropagateNew => {
            d.state = IN;
            if mode == RegistrarControl::FixedPropagateNew && e == RNew {
                d.join = Some(true);
            }
            return d;
        }
        RegistrarControl::Forbidden => {
            d.state = MT;
            return d;
        }
        _ => {}
    }
    match e {
        Begin => {
            d.state = MT;
            d.cancel_leave = true;
        }
        RNew => {
            d.state = IN;
            d.join = Some(true);
            d.cancel_leave = s == LV;
        }
        RJoinIn | RJoinMt => {
            d.state = IN;
            if s == MT {
                d.join = Some(false);
            }
            d.cancel_leave = s == LV;
        }
        RLv | RLA | TxLA | Redeclare if s == IN => {
            if p2p {
                d.state = MT;
                d.leave = true;
            } else {
                d.state = LV;
                d.arm_leave = true;
            }
        }
        Flush => {
            d.state = MT;
            d.leave = s != MT;
            d.cancel_leave = true;
        }
        LeaveExpired if s == LV => {
            d.state = MT;
            d.leave = true;
        }
        _ => {}
    }
    d
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical() {
        assert_eq!(
            hex(&encode_payload(&[(1, AttributeEvent::New)], false, 46)
                .unwrap()
                .0),
            "000102000100010000000000"
        );
        assert_eq!(
            hex(&encode_payload(&[], true, 46).unwrap().0),
            "0001022000000000000000"
        );
    }
    #[test]
    fn local_leaveall() {
        let a = applicant(
            Applicant::AA,
            Event::RLA,
            true,
            Registrar::IN,
            ApplicantControl::Normal,
        );
        let r = registrar(Registrar::IN, Event::RLA, true, RegistrarControl::Normal);
        assert_eq!((a.state, r.state), (Applicant::VP, Registrar::MT));
        let tx = applicant(
            a.state,
            Event::TxLA,
            true,
            r.state,
            ApplicantControl::Normal,
        );
        assert_eq!(
            (tx.state, tx.action, tx.request_tx),
            (Applicant::AA, Some(AttributeEvent::Mt), true)
        );
    }
    #[test]
    fn malformed_tail() {
        assert!(decode_payload(&[0, 1, 2, 0, 1, 0, 1, 0, 0], 1, 4094).is_err());
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuxiliaryState {
    Active,
    Passive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxiliaryEvent {
    Begin,
    ReceiveLeaveAll,
    LeaveAllExpired,
    PeriodicExpired,
    Tx,
    // Internal management stimulus; the public profile fixes it at prepare.
    #[allow(dead_code)]
    Enabled,
    Disabled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxiliaryStep {
    pub state: AuxiliaryState,
    pub restart_timer: bool,
    pub send_leaveall: bool,
    pub periodic: bool,
    pub request_tx: bool,
}
pub fn leaveall(s: AuxiliaryState, e: AuxiliaryEvent) -> AuxiliaryStep {
    use AuxiliaryEvent::*;
    let mut d = AuxiliaryStep {
        state: s,
        restart_timer: false,
        send_leaveall: false,
        periodic: false,
        request_tx: false,
    };
    match e {
        Begin | ReceiveLeaveAll => {
            d.state = AuxiliaryState::Passive;
            d.restart_timer = true;
        }
        LeaveAllExpired => {
            d.state = AuxiliaryState::Active;
            d.restart_timer = true;
            d.request_tx = true;
        }
        Tx if s == AuxiliaryState::Active => {
            d.state = AuxiliaryState::Passive;
            d.send_leaveall = true;
        }
        _ => {}
    }
    d
}
pub fn periodic(s: AuxiliaryState, e: AuxiliaryEvent) -> AuxiliaryStep {
    use AuxiliaryEvent::*;
    let mut d = AuxiliaryStep {
        state: s,
        restart_timer: false,
        send_leaveall: false,
        periodic: false,
        request_tx: false,
    };
    match e {
        Begin => {
            d.state = AuxiliaryState::Active;
            d.restart_timer = true;
        }
        PeriodicExpired if s == AuxiliaryState::Active => {
            d.restart_timer = true;
            d.periodic = true;
        }
        Enabled if s == AuxiliaryState::Passive => {
            d.state = AuxiliaryState::Active;
            d.restart_timer = true;
        }
        Disabled => d.state = AuxiliaryState::Passive,
        _ => {}
    }
    d
}
/// Capacity is resolved before committing the Applicant transition.
pub fn applicant_tx(
    s: Applicant,
    e: Event,
    p2p: bool,
    r: Registrar,
    mode: ApplicantControl,
    room: bool,
) -> ApplicantStep {
    let result = applicant(s, e, p2p, r, mode);
    if room || result.optional || result.action.is_none() {
        return result;
    }
    if e == Event::TxLA {
        return applicant(s, Event::TxLAF, p2p, r, mode);
    }
    ApplicantStep {
        state: s,
        action: None,
        optional: false,
        request_tx: true,
    }
}
