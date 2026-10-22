//! Prefix receiver for the ideal digital CC/ISO-FD contract.
//! All lengths, stuffing, count codes, CRCs and ACK decisions come from samples.
use super::codec::DATA_LENGTHS;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "snake_case", deny_unknown_fields)]
pub enum DecodedFrame {
    Cc {
        id: u32,
        extended: bool,
        remote: bool,
        dlc: u8,
        data: Vec<u8>,
    },
    IsoFd {
        id: u32,
        extended: bool,
        brs: bool,
        esi: bool,
        data: Vec<u8>,
    },
}
#[derive(Clone, Debug)]
pub struct Sample {
    pub level: u8,
    pub local_index: usize,
    pub exact_num: u128,
    pub exact_den: u128,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RateAction {
    None,
    EnterData,
    ReturnNominal,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Detection {
    pub kind: String,
    pub field: String,
    pub local_index: usize,
    pub causes: Vec<String>,
    pub delayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub field: String,
    pub arbitration_position: bool,
    /// True after the valid local CRC delimiter, for the following ACK slot.
    pub ack_intent: bool,
    pub completed_frame: Option<DecodedFrame>,
    pub detection: Option<Detection>,
    pub pending_causes: Vec<String>,
    pub rate_action: RateAction,
    pub data_phase_active: bool,
    pub frame_end: bool,
    pub overload: bool,
    pub sample_index: usize,
    pub exact_num: String,
    pub exact_den: String,
}
impl Observation {
    fn pending(field: &str, index: usize) -> Self {
        Self {
            field: field.into(),
            arbitration_position: false,
            ack_intent: false,
            completed_frame: None,
            detection: None,
            pending_causes: vec![],
            rate_action: RateAction::None,
            data_phase_active: false,
            frame_end: false,
            overload: false,
            sample_index: index,
            exact_num: "0".into(),
            exact_den: "1".into(),
        }
    }
    fn error(mut self, kind: &str, delayed: bool) -> Self {
        let mut causes = self.pending_causes.clone();
        if !causes.iter().any(|cause| cause == kind) {
            causes.push(kind.into());
        }
        self.detection = Some(Detection {
            kind: kind.into(),
            field: self.field.clone(),
            local_index: self.sample_index,
            causes,
            delayed,
        });
        self
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receiver {
    fd_capable: bool,
    samples: Vec<u8>,
    delivered: bool,
    failed: bool,
    ended: bool,
}
impl Receiver {
    pub fn new(fd_capable: bool) -> Self {
        Self {
            fd_capable,
            samples: vec![],
            delivered: false,
            failed: false,
            ended: false,
        }
    }
    pub fn fd_capable(&self) -> bool {
        self.fd_capable
    }
    pub fn len(&self) -> usize {
        self.samples.len()
    }
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
    pub fn samples(&self) -> &[u8] {
        &self.samples
    }
    pub fn data_phase_active(&self) -> bool {
        inspect(&self.samples, self.fd_capable).data_phase_active
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.samples.len() > 1024 || self.samples.iter().any(|b| *b > 1) {
            return Err("invalid receiver sample arena".into());
        }
        let result = inspect(&self.samples, self.fd_capable);
        if self.failed != result.detection.is_some()
            || self.ended != result.frame_end
            || self.delivered != (result.completed_frame.is_some() || result.frame_end)
        {
            return Err("unrecorded receiver detection".into());
        }
        if self.delivered && !result.frame_end && result.completed_frame.is_none() && !self.failed {
            return Err("receiver delivery without valid EOF".into());
        }
        Ok(())
    }
    pub fn sample(&mut self, sample: Sample) -> Result<Observation, String> {
        if self.failed
            || self.ended
            || sample.local_index != self.samples.len()
            || sample.level > 1
            || sample.exact_den == 0
            || self.samples.len() >= 1024
        {
            return Err("receiver sample sequence, level, timing or terminal state invalid".into());
        }
        self.samples.push(sample.level);
        let mut result = inspect(&self.samples, self.fd_capable);
        if self.delivered {
            result.completed_frame = None;
        }
        if result.completed_frame.is_some() {
            self.delivered = true;
        }
        if result.detection.is_some() {
            self.failed = true;
        }
        self.ended = result.frame_end;
        result.exact_num = sample.exact_num.to_string();
        result.exact_den = sample.exact_den.to_string();
        Ok(result)
    }
}
fn value(bits: &[u8]) -> u32 {
    bits.iter().fold(0, |n, b| (n << 1) | *b as u32)
}
fn crc(input: impl IntoIterator<Item = u8>, width: u8, polynomial: u32, initial: u32) -> u32 {
    let mut result = initial;
    for bit in input {
        let feedback = (result >> (width - 1)) ^ bit as u32;
        result = (result << 1) & ((1 << width) - 1);
        if feedback != 0 {
            result ^= polynomial;
        }
    }
    result
}
fn raw_field(index: usize, extended: bool, fd: Option<bool>) -> (&'static str, bool) {
    if index == 0 {
        return ("sof", false);
    }
    if index <= 11 {
        return ("base_id", true);
    }
    if index == 12 {
        return (if extended { "srr" } else { "rtr_rrs" }, true);
    }
    if index == 13 {
        return ("ide", true);
    }
    if extended && index <= 32 {
        return (
            if index == 32 {
                "rtr_rrs"
            } else {
                "extended_id"
            },
            true,
        );
    }
    let fdf = if extended { 33 } else { 14 };
    if index == fdf {
        return ("fdf_reserved", false);
    }
    if fd == Some(true) {
        if index == fdf + 1 {
            return ("res", false);
        }
        if index == fdf + 2 {
            return ("brs", false);
        }
        if index == fdf + 3 {
            return ("esi", false);
        }
        if index < fdf + 8 {
            return ("dlc", false);
        }
    } else if index < if extended { 39 } else { 19 } {
        return ("control", false);
    }
    ("data", false)
}
fn inspect(samples: &[u8], fd_capable: bool) -> Observation {
    let index = samples.len().saturating_sub(1);
    let mut result = Observation::pending("sof", index);
    let mut raw = Vec::new();
    let mut physical = 0usize;
    let mut last = 2u8;
    let mut run = 0usize;
    let mut stuff = 0usize;
    let mut extended = false;
    let mut fd = None;
    let mut control = 0usize;
    let mut data_len = 0usize;
    let mut required = usize::MAX;
    let mut brs_position = None;
    loop {
        if run == 5 {
            let Some(&bit) = samples.get(physical) else {
                return result;
            };
            result.field = "dynamic_stuff".into();
            result.arbitration_position = false;
            if bit == last {
                return result.error("stuff", false);
            }
            run = 1;
            stuff += 1;
            physical += 1;
            if physical == samples.len() {
                return result;
            }
        }
        let Some(&bit) = samples.get(physical) else {
            return result;
        };
        let raw_index = raw.len();
        raw.push(bit);
        if raw_index == 13 {
            extended = bit != 0;
        }
        let fdf = if extended { 33 } else { 14 };
        if raw_index == fdf {
            fd = Some(fd_capable && bit != 0);
        }
        let (field, arbitration) = raw_field(raw_index, extended, fd);
        result.field = field.into();
        result.arbitration_position = arbitration;
        if raw_index == 13 && extended && raw[12] != 1 {
            return result.error("form", false);
        }
        if raw_index == 0 && bit != 0 {
            return result.error("form", false);
        }
        if fd == Some(true) {
            if raw_index == fdf && raw[if extended { 32 } else { 12 }] != 0 {
                return result.error("form", false);
            }
            if extended && raw.len() > 14 && raw[12] != 1 {
                return result.error("form", false);
            }
            if raw_index == fdf + 1 && bit != 0 {
                return result.error("form", false);
            }
            if raw_index == fdf + 2 {
                brs_position = Some(physical);
                result.data_phase_active = bit != 0;
                if bit != 0 && physical == index {
                    result.rate_action = RateAction::EnterData;
                }
            }
        }
        last = bit;
        run = if raw.len() > 1 && samples.get(physical.wrapping_sub(1)) == Some(&bit) {
            run + 1
        } else {
            1
        };
        physical += 1;
        if let Some(is_fd) = fd {
            control = if is_fd {
                fdf + 8
            } else if extended {
                39
            } else {
                19
            };
            if raw.len() == control {
                let dlc = value(&raw[control - 4..control]) as usize;
                if !is_fd && dlc > 8 {
                    return result.error("form", false);
                }
                data_len = if is_fd {
                    DATA_LENGTHS[dlc]
                } else if raw[if extended { 32 } else { 12 }] == 1 {
                    0
                } else {
                    dlc
                };
                required = control + data_len * 8 + if is_fd { 0 } else { 15 };
            }
        }
        if raw.len() == required {
            break;
        }
        if physical == samples.len() {
            return result;
        }
    }
    let is_fd = fd.unwrap_or(false);
    let fdf = if extended { 33 } else { 14 };
    if extended && raw[12] != 1 {
        return result.error("form", false);
    }
    let brs = is_fd && raw[fdf + 2] == 1;
    let mut causes = Vec::new();
    if is_fd {
        let dynamic_end = physical;
        let width = if data_len <= 16 { 17 } else { 21 };
        let mut field = Vec::new();
        for i in 0..4 + width as usize {
            if i % 4 == 0 {
                let Some(&bit) = samples.get(physical) else {
                    result.pending_causes = causes;
                    return result;
                };
                result.field = "fixed_stuff".into();
                result.arbitration_position = false;
                if bit == samples[physical - 1] {
                    result.pending_causes = causes;
                    return result.error("form", false);
                }
                physical += 1;
                if physical == samples.len() {
                    result.pending_causes = causes;
                    return result;
                }
            }
            let Some(&bit) = samples.get(physical) else {
                result.pending_causes = causes;
                return result;
            };
            result.field = if i < 4 { "stuff_count" } else { "crc" }.into();
            result.arbitration_position = false;
            field.push(bit);
            physical += 1;
            if field.len() == 4 {
                let expected = (stuff % 8) as u32;
                let gray = expected ^ (expected >> 1);
                if value(&field) != (gray << 1) | (gray.count_ones() & 1) {
                    causes.push("stuff_count".into());
                }
            }
            if physical == samples.len() && i + 1 < 4 + width as usize {
                result.pending_causes = causes;
                return result;
            }
        }
        let expected = crc(
            samples[..dynamic_end]
                .iter()
                .chain(field[..4].iter())
                .copied(),
            width,
            if width == 17 { 0x1685b } else { 0x102899 },
            1 << (width - 1),
        );
        if expected != value(&field[4..]) {
            causes.push("crc".into());
        }
    } else {
        // Classical CAN also inserts a dynamic stuff bit after a final CRC run of five.
        if run == 5 {
            let Some(&bit) = samples.get(physical) else {
                return result;
            };
            result.field = "dynamic_stuff".into();
            result.arbitration_position = false;
            if bit == last {
                return result.error("stuff", false);
            }
            physical += 1;
        }
        if crc(raw[..raw.len() - 15].iter().copied(), 15, 0x4599, 0)
            != value(&raw[raw.len() - 15..])
        {
            causes.push("crc".into());
        }
    }
    result.pending_causes = causes;
    let delimiter = physical;
    for offset in 0..10 {
        let Some(&bit) = samples.get(delimiter + offset) else {
            return result;
        };
        result.field = match offset {
            0 => "crc_delimiter",
            1 => "ack",
            2 => "ack_delimiter",
            _ => "eof",
        }
        .into();
        result.arbitration_position = false;
        if offset == 0 {
            result.data_phase_active = false;
        }
        if offset == 0 && brs && delimiter == index {
            result.rate_action = RateAction::ReturnNominal;
        }
        if offset != 1 && bit != 1 {
            if offset == 9 {
                result.frame_end = true;
                result.overload = true;
                return result;
            }
            return result.error("form", false);
        }
        if offset == 0 && index == delimiter && result.pending_causes.is_empty() {
            result.ack_intent = true;
        }
        if offset == 2 && !result.pending_causes.is_empty() {
            let kind = result.pending_causes[0].clone();
            return result.error(&kind, true);
        }
        if offset == 8 && result.pending_causes.is_empty() && index == delimiter + offset {
            let id = if extended {
                (value(&raw[1..12]) << 18) | value(&raw[14..32])
            } else {
                value(&raw[1..12])
            };
            let data = raw[control..control + data_len * 8]
                .chunks_exact(8)
                .map(|bits| value(bits) as u8)
                .collect();
            result.completed_frame = Some(if is_fd {
                DecodedFrame::IsoFd {
                    id,
                    extended,
                    brs,
                    esi: raw[fdf + 3] != 0,
                    data,
                }
            } else {
                DecodedFrame::Cc {
                    id,
                    extended,
                    remote: raw[if extended { 32 } else { 12 }] != 0,
                    dlc: value(&raw[control - 4..control]) as u8,
                    data,
                }
            });
        }
        if offset == 9 {
            result.frame_end = true;
        }
        if delimiter + offset == index {
            return result;
        }
    }
    let _ = brs_position;
    result
}
