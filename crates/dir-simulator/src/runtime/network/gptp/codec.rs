//! Selected IEEE 802.1AS-2020 domain-0 instance-specific P2P wire profile.
//! The caller supplies only a validated Ethernet payload; padding is allowed
//! after messageLength and cannot become protocol data.
use serde::{Deserialize, Serialize};

pub const ETHERTYPE: u16 = 0x88f7;
pub const MULTICAST: [u8; 6] = [0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e];
const HEADER: usize = 34;
const UNREPRESENTABLE: i64 = i64::MAX;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortIdentity {
    pub clock: [u8; 8],
    pub port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    pub seconds: u64,
    pub nanoseconds: u32,
}
impl Timestamp {
    pub fn new(seconds: u64, nanoseconds: u32) -> Result<Self, &'static str> {
        if seconds >= 1u64 << 48 || nanoseconds >= 1_000_000_000 {
            return Err("invalid timestamp range");
        }
        Ok(Self {
            seconds,
            nanoseconds,
        })
    }
    pub fn floor_ps(self) -> Result<i128, &'static str> {
        i128::from(self.seconds)
            .checked_mul(1_000_000_000_000)
            .and_then(|v| v.checked_add(i128::from(self.nanoseconds) * 1000))
            .ok_or("timestamp overflow")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpInfo {
    pub cumulative_scaled_rate_offset: i32,
    pub gm_time_base_indicator: u16,
    pub last_gm_phase_change: [u8; 12],
    pub scaled_last_gm_freq_change: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announce {
    pub current_utc_offset: i16,
    pub priority1: u8,
    pub clock_class: u8,
    pub clock_accuracy: u8,
    pub offset_scaled_log_variance: u16,
    pub priority2: u8,
    pub grandmaster_identity: [u8; 8],
    pub steps_removed: u16,
    pub time_source: u8,
    pub path_trace: Vec<[u8; 8]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    Sync,
    FollowUp {
        precise_origin: Timestamp,
        info: FollowUpInfo,
    },
    PdelayReq,
    PdelayResp {
        request_receipt: Timestamp,
        requester: PortIdentity,
    },
    PdelayRespFollowUp {
        response_origin: Timestamp,
        requester: PortIdentity,
    },
    Announce(Announce),
}
impl Body {
    pub fn message_type(&self) -> u8 {
        match self {
            Self::Sync => 0,
            Self::PdelayReq => 2,
            Self::PdelayResp { .. } => 3,
            Self::FollowUp { .. } => 8,
            Self::PdelayRespFollowUp { .. } => 10,
            Self::Announce(_) => 11,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub source: PortIdentity,
    pub sequence: u16,
    /// Signed units of 2^-16 nanoseconds. i64::MAX is an invalid sentinel.
    pub correction: i64,
    pub log_interval: i8,
    pub body: Body,
}
impl Message {
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        if self.source.port == 0 || self.correction == UNREPRESENTABLE {
            return Err("invalid header field");
        }
        let kind = self.body.message_type();
        let size = match &self.body {
            Body::Sync => 44,
            Body::PdelayReq | Body::PdelayResp { .. } | Body::PdelayRespFollowUp { .. } => 54,
            Body::FollowUp { .. } => 76,
            Body::Announce(a) => {
                if a.path_trace.len() != usize::from(a.steps_removed) + 1 || a.path_trace.is_empty()
                {
                    return Err("path trace length differs from stepsRemoved");
                }
                68usize
                    .checked_add(
                        8usize
                            .checked_mul(a.path_trace.len())
                            .ok_or("message too long")?,
                    )
                    .ok_or("message too long")?
            }
        };
        let length = u16::try_from(size).map_err(|_| "message too long")?;
        let mut b = vec![0u8; size];
        b[0] = 0x10 | kind;
        b[1] = 0x12;
        b[2..4].copy_from_slice(&length.to_be_bytes());
        if matches!(self.body, Body::Sync | Body::PdelayResp { .. }) {
            b[6] = 0x02;
        }
        b[8..16].copy_from_slice(&self.correction.to_be_bytes());
        b[20..28].copy_from_slice(&self.source.clock);
        b[28..30].copy_from_slice(&self.source.port.to_be_bytes());
        b[30..32].copy_from_slice(&self.sequence.to_be_bytes());
        b[33] = self.log_interval as u8;
        match &self.body {
            Body::Sync | Body::PdelayReq => {}
            Body::FollowUp {
                precise_origin,
                info,
            } => {
                write_timestamp(&mut b[34..44], *precise_origin)?;
                b[44..46].copy_from_slice(&3u16.to_be_bytes());
                b[46..48].copy_from_slice(&28u16.to_be_bytes());
                b[48..51].copy_from_slice(&[0, 0x80, 0xc2]);
                b[51..54].copy_from_slice(&[0, 0, 1]);
                b[54..58].copy_from_slice(&info.cumulative_scaled_rate_offset.to_be_bytes());
                b[58..60].copy_from_slice(&info.gm_time_base_indicator.to_be_bytes());
                b[60..72].copy_from_slice(&info.last_gm_phase_change);
                b[72..76].copy_from_slice(&info.scaled_last_gm_freq_change.to_be_bytes());
            }
            Body::PdelayResp {
                request_receipt,
                requester,
            } => {
                write_timestamp(&mut b[34..44], *request_receipt)?;
                write_port(&mut b[44..54], requester);
            }
            Body::PdelayRespFollowUp {
                response_origin,
                requester,
            } => {
                write_timestamp(&mut b[34..44], *response_origin)?;
                write_port(&mut b[44..54], requester);
            }
            Body::Announce(a) => {
                b[44..46].copy_from_slice(&a.current_utc_offset.to_be_bytes());
                b[47] = a.priority1;
                b[48] = a.clock_class;
                b[49] = a.clock_accuracy;
                b[50..52].copy_from_slice(&a.offset_scaled_log_variance.to_be_bytes());
                b[52] = a.priority2;
                b[53..61].copy_from_slice(&a.grandmaster_identity);
                b[61..63].copy_from_slice(&a.steps_removed.to_be_bytes());
                b[63] = a.time_source;
                b[64..66].copy_from_slice(&8u16.to_be_bytes());
                b[66..68].copy_from_slice(
                    &(u16::try_from(a.path_trace.len() * 8).map_err(|_| "path trace too long")?)
                        .to_be_bytes(),
                );
                for (i, clock) in a.path_trace.iter().enumerate() {
                    b[68 + i * 8..76 + i * 8].copy_from_slice(clock);
                }
            }
        }
        Ok(b)
    }
    pub fn decode(padded: &[u8]) -> Result<Self, &'static str> {
        if padded.len() < HEADER {
            return Err("truncated PTP header");
        }
        if padded[0] >> 4 != 1 || padded[1] & 0x0f != 2 || padded[4] != 0 || padded[5] != 0 {
            return Err("unsupported SDO, PTP version or domain");
        }
        let kind = padded[0] & 0x0f;
        let length = usize::from(u16::from_be_bytes([padded[2], padded[3]]));
        if length > padded.len() || length < HEADER {
            return Err("truncated or invalid messageLength");
        }
        let b = &padded[..length];
        let correction = i64::from_be_bytes(b[8..16].try_into().unwrap());
        if correction == UNREPRESENTABLE {
            return Err("unrepresentable correctionField");
        }
        let source = port(&b[20..30]);
        if source.port == 0 {
            return Err("zero source port number");
        }
        let two_step = b[6] & 0x02 != 0;
        let body = match kind {
            0 if length == 44 && two_step => Body::Sync,
            0 if !two_step => return Err("one-step Sync outside selected profile"),
            2 if length == 54 => Body::PdelayReq,
            3 if length == 54 && two_step => Body::PdelayResp {
                request_receipt: timestamp(&b[34..44])?,
                requester: port(&b[44..54]),
            },
            3 if !two_step => return Err("one-step Pdelay response outside selected profile"),
            8 if length == 76 => {
                if b[44..46] != 3u16.to_be_bytes()
                    || b[46..48] != 28u16.to_be_bytes()
                    || b[48..51] != [0, 0x80, 0xc2]
                    || b[51..54] != [0, 0, 1]
                {
                    return Err("invalid Follow_Up information TLV");
                }
                Body::FollowUp {
                    precise_origin: timestamp(&b[34..44])?,
                    info: FollowUpInfo {
                        cumulative_scaled_rate_offset: i32::from_be_bytes(
                            b[54..58].try_into().unwrap(),
                        ),
                        gm_time_base_indicator: u16::from_be_bytes(b[58..60].try_into().unwrap()),
                        last_gm_phase_change: b[60..72].try_into().unwrap(),
                        scaled_last_gm_freq_change: i32::from_be_bytes(
                            b[72..76].try_into().unwrap(),
                        ),
                    },
                }
            }
            10 if length == 54 => Body::PdelayRespFollowUp {
                response_origin: timestamp(&b[34..44])?,
                requester: port(&b[44..54]),
            },
            11 if length >= 76 && b[64..66] == 8u16.to_be_bytes() => {
                let steps = u16::from_be_bytes(b[61..63].try_into().unwrap());
                let path_length = usize::from(u16::from_be_bytes(b[66..68].try_into().unwrap()));
                if path_length == 0
                    || path_length % 8 != 0
                    || path_length / 8 != usize::from(steps) + 1
                    || length != 68 + path_length
                {
                    return Err("invalid Announce path trace");
                }
                Body::Announce(Announce {
                    current_utc_offset: i16::from_be_bytes(b[44..46].try_into().unwrap()),
                    priority1: b[47],
                    clock_class: b[48],
                    clock_accuracy: b[49],
                    offset_scaled_log_variance: u16::from_be_bytes(b[50..52].try_into().unwrap()),
                    priority2: b[52],
                    grandmaster_identity: b[53..61].try_into().unwrap(),
                    steps_removed: steps,
                    time_source: b[63],
                    path_trace: b[68..]
                        .chunks_exact(8)
                        .map(|v| v.try_into().unwrap())
                        .collect(),
                })
            }
            11 => return Err("unsupported Announce TLV or length"),
            0 | 2 | 3 | 8 | 10 => return Err("invalid PTP message length or flags"),
            _ => return Err("unsupported PTP message type"),
        };
        Ok(Self {
            source,
            sequence: u16::from_be_bytes(b[30..32].try_into().unwrap()),
            correction,
            log_interval: b[33] as i8,
            body,
        })
    }
}

fn write_port(out: &mut [u8], p: &PortIdentity) {
    out[..8].copy_from_slice(&p.clock);
    out[8..10].copy_from_slice(&p.port.to_be_bytes());
}
fn port(b: &[u8]) -> PortIdentity {
    PortIdentity {
        clock: b[..8].try_into().unwrap(),
        port: u16::from_be_bytes(b[8..10].try_into().unwrap()),
    }
}
fn write_timestamp(out: &mut [u8], t: Timestamp) -> Result<(), &'static str> {
    Timestamp::new(t.seconds, t.nanoseconds)?;
    out[..6].copy_from_slice(&t.seconds.to_be_bytes()[2..]);
    out[6..10].copy_from_slice(&t.nanoseconds.to_be_bytes());
    Ok(())
}
fn timestamp(b: &[u8]) -> Result<Timestamp, &'static str> {
    let mut sec = [0; 8];
    sec[2..].copy_from_slice(&b[..6]);
    Timestamp::new(
        u64::from_be_bytes(sec),
        u32::from_be_bytes(b[6..10].try_into().unwrap()),
    )
}

/// Euclidean wire quantization. The correction retains the positive fraction
/// beneath the integral nanosecond timestamp, in 2^-16 ns units.
#[cfg(test)]
pub fn split_ps(ps: i128) -> Result<(Timestamp, i64), &'static str> {
    if ps < 0 {
        return Err("negative PTP timestamp");
    }
    let ns = ps.div_euclid(1000);
    let correction = ps
        .rem_euclid(1000)
        .checked_mul(65536)
        .ok_or("correction overflow")?
        .div_euclid(1000);
    let seconds = u64::try_from(ns.div_euclid(1_000_000_000)).map_err(|_| "timestamp overflow")?;
    let nanos = u32::try_from(ns.rem_euclid(1_000_000_000)).map_err(|_| "timestamp overflow")?;
    Ok((
        Timestamp::new(seconds, nanos)?,
        i64::try_from(correction).map_err(|_| "correction overflow")?,
    ))
}

pub fn rate_to_scaled(numerator: i128, denominator: i128) -> Result<i32, &'static str> {
    if numerator <= 0 || denominator <= 0 {
        return Err("nonpositive rate ratio");
    }
    let delta = numerator.checked_sub(denominator).ok_or("rate overflow")?;
    let scaled = delta
        .checked_mul(1i128 << 41)
        .ok_or("rate overflow")?
        .div_euclid(denominator);
    i32::try_from(scaled).map_err(|_| "rate field overflow")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> PortIdentity {
        PortIdentity {
            clock: [1, 2, 3, 4, 5, 6, 7, 8],
            port: 0x1234,
        }
    }
    #[test]
    fn literal_header_and_padding() {
        let m = Message {
            source: identity(),
            sequence: 65535,
            correction: 2 * 65536 + 32768,
            log_interval: -3,
            body: Body::Sync,
        };
        let b = m.encode().unwrap();
        assert_eq!(&b[..4], &[0x10, 0x12, 0, 44]);
        assert_eq!(&b[6..8], &[2, 0]);
        assert_eq!(&b[8..16], &[0, 0, 0, 0, 0, 2, 0x80, 0]);
        assert_eq!(
            &b[20..34],
            &[1, 2, 3, 4, 5, 6, 7, 8, 0x12, 0x34, 0xff, 0xff, 0, 0xfd]
        );
        let mut padded = b;
        padded.resize(46, 0x5a);
        assert_eq!(Message::decode(&padded).unwrap(), m);
        assert_eq!(m.sequence.wrapping_add(1), 0);
        padded[3] = 47;
        assert!(Message::decode(&padded).is_err());
    }
    #[test]
    fn independent_followup_pdelay_and_announce_layout() {
        let t = Timestamp::new(0x010203040506, 987654321).unwrap();
        let info = FollowUpInfo {
            cumulative_scaled_rate_offset: -1,
            gm_time_base_indicator: 0x3456,
            last_gm_phase_change: [0; 12],
            scaled_last_gm_freq_change: -2,
        };
        let f = Message {
            source: identity(),
            sequence: 9,
            correction: 0,
            log_interval: -3,
            body: Body::FollowUp {
                precise_origin: t,
                info,
            },
        }
        .encode()
        .unwrap();
        assert_eq!(&f[..4], &[0x18, 0x12, 0, 76]);
        assert_eq!(&f[34..44], &[1, 2, 3, 4, 5, 6, 0x3a, 0xde, 0x68, 0xb1]);
        assert_eq!(
            &f[44..60],
            &[
                0, 3, 0, 28, 0, 0x80, 0xc2, 0, 0, 1, 0xff, 0xff, 0xff, 0xff, 0x34, 0x56
            ]
        );
        assert_eq!(Message::decode(&f).unwrap().encode().unwrap(), f);
        let r = Message {
            source: identity(),
            sequence: 7,
            correction: 0x3333,
            log_interval: 127,
            body: Body::PdelayResp {
                request_receipt: t,
                requester: identity(),
            },
        }
        .encode()
        .unwrap();
        assert_eq!(&r[..4], &[0x13, 0x12, 0, 54]);
        assert_eq!(&r[44..54], &[1, 2, 3, 4, 5, 6, 7, 8, 0x12, 0x34]);
        assert_eq!(Message::decode(&r).unwrap().encode().unwrap(), r);
        let a = Announce {
            current_utc_offset: 37,
            priority1: 128,
            clock_class: 248,
            clock_accuracy: 0xfe,
            offset_scaled_log_variance: 0x1234,
            priority2: 128,
            grandmaster_identity: identity().clock,
            steps_removed: 0,
            time_source: 0xa0,
            path_trace: vec![identity().clock],
        };
        let a = Message {
            source: identity(),
            sequence: 0x4567,
            correction: 0,
            log_interval: 0,
            body: Body::Announce(a),
        }
        .encode()
        .unwrap();
        assert_eq!(&a[..4], &[0x1b, 0x12, 0, 76]);
        assert_eq!(&a[44..54], &[0, 37, 0, 128, 248, 0xfe, 0x12, 0x34, 128, 1]);
        assert_eq!(&a[64..76], &[0, 8, 0, 8, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(Message::decode(&a).unwrap().encode().unwrap(), a);
    }
    #[test]
    fn independent_pdelay_request_and_response_followup_layout() {
        let req = Message {
            source: identity(),
            sequence: 0x1234,
            correction: 0,
            log_interval: 127,
            body: Body::PdelayReq,
        }
        .encode()
        .unwrap();
        assert_eq!(&req[..4], &[0x12, 0x12, 0, 54]);
        assert_eq!(&req[6..8], &[0, 0]);
        assert_eq!(&req[30..34], &[0x12, 0x34, 0, 127]);
        assert_eq!(&req[34..54], &[0; 20]);
        assert!(matches!(
            Message::decode(&req).unwrap().body,
            Body::PdelayReq
        ));
        let follow = Message {
            source: identity(),
            sequence: 0x1234,
            correction: -65536,
            log_interval: 127,
            body: Body::PdelayRespFollowUp {
                response_origin: Timestamp::new(0x010203040506, 987654321).unwrap(),
                requester: identity(),
            },
        }
        .encode()
        .unwrap();
        assert_eq!(&follow[..4], &[0x1a, 0x12, 0, 54]);
        assert_eq!(&follow[8..16], &(-65536i64).to_be_bytes());
        assert_eq!(&follow[34..44], &[1, 2, 3, 4, 5, 6, 0x3a, 0xde, 0x68, 0xb1]);
        assert_eq!(&follow[44..54], &[1, 2, 3, 4, 5, 6, 7, 8, 0x12, 0x34]);
        assert_eq!(Message::decode(&follow).unwrap().encode().unwrap(), follow);
    }
    #[test]
    fn rejects_sentinel_tlv_invalid_nanoseconds_and_one_step() {
        let mut b = Message {
            source: identity(),
            sequence: 0,
            correction: 0,
            log_interval: 0,
            body: Body::FollowUp {
                precise_origin: Timestamp::new(0, 0).unwrap(),
                info: FollowUpInfo {
                    cumulative_scaled_rate_offset: 0,
                    gm_time_base_indicator: 0,
                    last_gm_phase_change: [0; 12],
                    scaled_last_gm_freq_change: 0,
                },
            },
        }
        .encode()
        .unwrap();
        b[48] = 1;
        assert_eq!(
            Message::decode(&b),
            Err("invalid Follow_Up information TLV")
        );
        b[48] = 0;
        b[51] = 1;
        assert_eq!(
            Message::decode(&b),
            Err("invalid Follow_Up information TLV")
        );
        b[51] = 0;
        b[40..44].copy_from_slice(&1_000_000_000u32.to_be_bytes());
        assert_eq!(Message::decode(&b), Err("invalid timestamp range"));
        b[40..44].copy_from_slice(&0u32.to_be_bytes());
        b[8..16].copy_from_slice(&i64::MAX.to_be_bytes());
        assert_eq!(Message::decode(&b), Err("unrepresentable correctionField"));
        let mut sync = Message {
            source: identity(),
            sequence: 0,
            correction: 0,
            log_interval: 0,
            body: Body::Sync,
        }
        .encode()
        .unwrap();
        sync[6] = 0;
        assert_eq!(
            Message::decode(&sync),
            Err("one-step Sync outside selected profile")
        );
        assert_eq!(rate_to_scaled((1i128 << 41) - 1, 1i128 << 41), Ok(-1));
        assert_eq!(
            split_ps(2500),
            Ok((
                Timestamp {
                    seconds: 0,
                    nanoseconds: 2
                },
                32768
            ))
        );
    }
}
