//! Selected ISSI Rev F x8 DDR4-2400R digital command contract.
use serde::{Deserialize, Serialize};

pub const MODE: &str = "issi.is43qr85120b-083rbl.rev-f.ddr4-2400r.x8.bl8.1n.25c.v1";
pub const SOURCE_SHA256: &str = "0606a9501dc13de5934856c9d7fbe0310f6a4edacf26b9cf416f4a5bfc2ca727";
pub const TCK: u64 = 834;
pub const HALF: u64 = 417;
pub const TREFI: u64 = 7_800_000;
pub const CAPACITY: u64 = 536_870_912;
pub const MR: [u16; 7] = [0x0834, 0x0001, 0x0018, 0x0200, 0x0000, 0x0400, 0x0810];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mode_id: String,
    pub source_sha256: String,
    pub base: u64,
    pub size: u64,
    pub queue_capacity: usize,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.mode_id != MODE || self.source_sha256 != SOURCE_SHA256 {
            return Err("unsupported DDR device/mode/source identity".into());
        }
        if self.base % 8 != 0
            || self.size == 0
            || self.size % 8 != 0
            || self.size > CAPACITY
            || self.base.checked_add(self.size).is_none()
            || !(1..=1024).contains(&self.queue_capacity)
        {
            return Err("invalid DDR region or bounded queue".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Read {
        length: usize,
    },
    Write {
        data: Vec<u8>,
        byte_enables: Vec<bool>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub address: u64,
    pub operation: Operation,
}
impl Request {
    pub fn length(&self) -> usize {
        match &self.operation {
            Operation::Read { length } => *length,
            Operation::Write { data, .. } => data.len(),
        }
    }
    pub fn is_write(&self) -> bool {
        matches!(self.operation, Operation::Write { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Reset {
        asserted: bool,
    },
    Cke {
        high: bool,
    },
    Des,
    Act {
        bank: u8,
        row: u16,
    },
    Pre {
        bank: Option<u8>,
    },
    Read {
        bank: u8,
        column: u16,
        auto_precharge: bool,
        burst: String,
        request: String,
        logical_offset: [Option<usize>; 8],
    },
    Write {
        bank: u8,
        column: u16,
        auto_precharge: bool,
        burst: String,
        request: String,
        data: [u8; 8],
        byte_enables: [bool; 8],
        logical_offset: [Option<usize>; 8],
    },
    Refresh,
    Mrs {
        index: u8,
        value: u16,
    },
    Zq {
        long: bool,
    },
    SelfRefresh,
    Dqs {
        high: bool,
    },
}
impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reset { asserted: true } => "RESET_ASSERT",
            Self::Reset { asserted: false } => "RESET_RELEASE",
            Self::Cke { high: true } => "CKE_HIGH",
            Self::Cke { high: false } => "CKE_LOW",
            Self::Des => "DES",
            Self::Act { .. } => "ACT",
            Self::Pre { bank: None } => "PREA",
            Self::Pre { .. } => "PRE",
            Self::Read {
                auto_precharge: false,
                ..
            } => "RD",
            Self::Read { .. } => "RDA",
            Self::Write {
                auto_precharge: false,
                ..
            } => "WR",
            Self::Write { .. } => "WRA",
            Self::Refresh => "REF",
            Self::Mrs { .. } => "MRS",
            Self::Zq { long: true } => "ZQCL",
            Self::Zq { .. } => "ZQCS",
            Self::SelfRefresh => "SRE",
            Self::Dqs { .. } => "DQS_EDGE",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    Command {
        at_ps: u64,
        command: Command,
    },
    State {
        at_ps: u64,
        state: String,
    },
    ResetInvalidation {
        at_ps: u64,
    },
    Ready {
        at_ps: u64,
    },
    ReadByte {
        at_ps: u64,
        request: String,
        burst: String,
        address: u64,
        logical_offset: Option<usize>,
    },
    WriteByte {
        at_ps: u64,
        request: String,
        burst: String,
        address: u64,
        value: u8,
        byte_enable: bool,
        logical_offset: Option<usize>,
    },
    BurstComplete {
        at_ps: u64,
        request: String,
        burst: String,
    },
    RequestComplete {
        at_ps: u64,
        request: String,
    },
    RequestAborted {
        at_ps: u64,
        request: String,
        reason: String,
    },
    RefreshDeadline {
        at_ps: u64,
    },
    LevelFeedback {
        at_ps: u64,
        bits: Option<[bool; 8]>,
    },
}
impl Effect {
    pub fn at_ps(&self) -> u64 {
        match self {
            Self::Command { at_ps, .. }
            | Self::State { at_ps, .. }
            | Self::ResetInvalidation { at_ps }
            | Self::Ready { at_ps }
            | Self::ReadByte { at_ps, .. }
            | Self::WriteByte { at_ps, .. }
            | Self::BurstComplete { at_ps, .. }
            | Self::RequestAborted { at_ps, .. }
            | Self::RequestComplete { at_ps, .. }
            | Self::RefreshDeadline { at_ps }
            | Self::LevelFeedback { at_ps, .. } => *at_ps,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub rule: String,
    /// Some means this minimum-time guard may become eligible later.
    pub earliest_ps: Option<u64>,
}
impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DDR rule {}", self.rule)?;
        if let Some(at) = self.earliest_ps {
            write!(f, " requires time >= {at} ps")?;
        }
        Ok(())
    }
}
impl std::error::Error for Violation {}
pub(crate) fn invalid(rule: &str) -> Violation {
    Violation {
        rule: rule.into(),
        earliest_ps: None,
    }
}
pub(crate) fn wait(now: u64, at: u64, rule: &str) -> Result<(), Violation> {
    if now < at {
        Err(Violation {
            rule: rule.into(),
            earliest_ps: Some(at),
        })
    } else {
        Ok(())
    }
}
pub(crate) fn plus(a: u64, b: u64) -> Result<u64, Violation> {
    a.checked_add(b)
        .ok_or_else(|| invalid("timestamp_overflow"))
}
pub(crate) fn cycle_after(at: u64, cycles: u64) -> Result<u64, Violation> {
    plus(
        at,
        cycles
            .checked_mul(TCK)
            .ok_or_else(|| invalid("timestamp_overflow"))?,
    )
}
pub(crate) fn edge(at: u64) -> Result<u64, Violation> {
    plus(at, (TCK - at % TCK) % TCK)
}
pub fn address(offset: u64) -> (u8, u16, u16) {
    (
        ((offset >> 10) & 15) as u8,
        (offset >> 14) as u16,
        (offset & 1023) as u16,
    )
}
pub fn offset(bank: u8, row: u16, column: u16) -> u64 {
    ((row as u64) << 14) | ((bank as u64) << 10) | column as u64
}
