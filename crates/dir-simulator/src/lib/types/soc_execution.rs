//! Inputs for one coherent executable SoC and its shared memory/transport owner.
use super::noc_flit::FlitConfig;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: String,
    pub version: u32,
    pub cycle_ps: u64,
    pub memory: Vec<Region>,
    pub cores: Vec<CoreConfig>,
    pub cache: CacheConfig,
    pub fabric: FabricConfig,
    pub noc: FlitConfig,
    pub memory_router: usize,
    pub dma_buffer_bytes: usize,
    #[serde(default)]
    pub slots: Vec<SlotConfig>,
    #[serde(default)]
    pub mailboxes: Vec<MailboxConfig>,
    #[serde(default)]
    pub ddr: Option<DdrConfig>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DdrConfig {
    pub mode_id: String,
    pub source_sha256: String,
    pub base: u64,
    pub size: u64,
    pub queue_capacity: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub id: String,
    pub base: u32,
    pub size: usize,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    #[serde(default)]
    pub initial: Vec<u8>,
    #[serde(default)]
    pub backend: Backend,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Sram,
    DdrStrict,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreConfig {
    pub id: String,
    pub reset_pc: u32,
    pub initial_sp: u32,
    pub instruction_budget: u64,
    pub cycle_cycles: u64,
    pub execute_cycles: u64,
    pub transport: Transport,
    pub router: usize,
    #[serde(default)]
    pub qos: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    pub line_bytes: usize,
    pub sets: usize,
    pub ways: usize,
    pub mshrs: usize,
    pub ack_cycles: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FabricConfig {
    pub queue_capacity: usize,
    pub aging_cycles: u64,
    pub data_bytes: usize,
    pub axi_address_delay_cycles: u64,
    pub ahb_wait_cycles: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotConfig {
    pub id: String,
    pub base: u32,
    pub capacity: usize,
}
/// Project mailbox ABI v1: one uncached 64-byte aperture for one shared slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxConfig {
    pub id: String,
    pub base: u32,
    pub slot: String,
    pub producer: String,
    pub consumer: String,
    pub service_cycles: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Axi,
    Ahb,
    Noc,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
    Atomic,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomicBurst {
    Incr,
    Wrap,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atomic {
    pub awatop: u8,
    pub burst: AtomicBurst,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRef {
    pub slot: String,
    pub generation: u64,
    pub owner: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub source: String,
    pub at_ps: u64,
    pub operation: Operation,
    pub address: u32,
    pub length: usize,
    pub transport: Transport,
    pub router: usize,
    #[serde(default)]
    pub axi_id: u32,
    #[serde(default)]
    pub qos: u8,
    #[serde(default)]
    pub data: Vec<u8>,
    #[serde(default)]
    pub byte_enables: Vec<bool>,
    #[serde(default)]
    pub atomic: Option<Atomic>,
    #[serde(default)]
    pub r_stall_cycles: u64,
    #[serde(default)]
    pub b_stall_cycles: u64,
    #[serde(default)]
    pub lease: Option<LeaseRef>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dma {
    pub id: String,
    pub source: String,
    pub at_ps: u64,
    pub source_address: u32,
    pub destination: u32,
    pub length: usize,
    pub transport: Transport,
    pub router: usize,
    #[serde(default)]
    pub qos: u8,
    #[serde(default)]
    pub lease: Option<LeaseRef>,
    #[serde(default)]
    pub publish: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandOp {
    Lease {
        slot: String,
        owner: String,
        length: usize,
    },
    Publish {
        lease: LeaseRef,
    },
    BeginRead {
        lease: LeaseRef,
        consumer: String,
    },
    Release {
        lease: LeaseRef,
        consumer: String,
    },
    DdrControl {
        command: serde_json::Value,
    },
    CancelDma {
        dma: String,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Command {
    pub id: String,
    pub at_ps: u64,
    #[serde(flatten)]
    pub action: CommandOp,
}
impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut fields =
            std::collections::BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        let id = serde_json::from_value(
            fields
                .remove("id")
                .ok_or_else(|| D::Error::custom("missing command id"))?,
        )
        .map_err(D::Error::custom)?;
        let at_ps = serde_json::from_value(
            fields
                .remove("at_ps")
                .ok_or_else(|| D::Error::custom("missing command at_ps"))?,
        )
        .map_err(D::Error::custom)?;
        let action =
            serde_json::from_value(serde_json::Value::Object(fields.into_iter().collect()))
                .map_err(D::Error::custom)?;
        Ok(Self { id, at_ps, action })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub schema: String,
    pub version: u32,
    #[serde(default)]
    pub requests: Vec<Request>,
    #[serde(default)]
    pub dma: Vec<Dma>,
    #[serde(default)]
    pub commands: Vec<Command>,
}
