use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mesi {
    S,
    E,
    M,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CacheLine {
    pub line: u32,
    pub state: Mesi,
    pub data: Vec<u8>,
    pub age: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Directory {
    pub owner: Option<usize>,
    pub sharers: BTreeSet<usize>,
    pub epoch: u64,
    pub transient: Option<Lock>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lock {
    pub txn: usize,
    pub epoch: u64,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ack {
    pub core: usize,
    pub epoch: u64,
    pub at: u64,
    pub invalidate: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Eviction {
    pub core: usize,
    pub way: usize,
    pub line: u32,
    pub epoch: u64,
    pub at: u64,
    pub txn: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Client {
    Fetch { core: usize },
    Load { core: usize },
    Store { core: usize },
    External { index: usize },
    DmaRead { index: usize },
    DmaWrite { index: usize },
}
impl Client {
    pub fn core(&self) -> Option<usize> {
        match self {
            Self::Fetch { core } | Self::Load { core } | Self::Store { core } => Some(*core),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Queued,
    Ingress,
    OwnerWait,
    Coherence,
    Peripheral,
    Native,
    Response,
    Done,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Transaction {
    pub request: Request,
    pub client: Client,
    pub phase: Phase,
    pub admitted: u64,
    pub admitted_sequence: u64,
    pub completed_sequence: Option<u64>,
    pub completed_at: Option<u64>,
    pub offset: usize,
    pub result: Vec<u8>,
    pub wire_data: Vec<u8>,
    pub error: Option<String>,
    pub lease_read_active: bool,
    pub address_accepted: bool,
    pub w_accepted: usize,
    pub ingress_sent: bool,
    pub ingress_delivered: bool,
    pub cache_hit: bool,
    pub line: Option<u32>,
    pub acknowledgments: Vec<Ack>,
    pub intervention: Option<Vec<u8>>,
    pub owner_ready: u64,
    pub grant_order: Option<u64>,
    pub response_ready: Option<u64>,
    pub r_sent: usize,
    pub b_sent: bool,
    pub response_sent: bool,
    pub response_delivered: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CorePhase {
    Fetch,
    Fetching {
        txn: usize,
    },
    Execute {
        instruction: u32,
        ready: u64,
    },
    Memory {
        instruction: u32,
        action: cpu::Action,
        txn: usize,
    },
    Fence {
        instruction: u32,
        next_pc: u32,
    },
    Halted,
    BudgetPaused,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CoreRuntime {
    pub architectural: cpu::Core,
    pub phase: CorePhase,
    pub ready: u64,
    pub fetch_transaction: Option<usize>,
    pub instruction_started: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SlotPhase {
    Free,
    Leased,
    Filling,
    Ready,
    Reading,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Slot {
    pub generation: u64,
    pub owner: Option<String>,
    pub consumer: Option<String>,
    pub phase: SlotPhase,
    pub length: usize,
    pub completed: Vec<bool>,
    pub active_reads: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DmaPhase {
    Pending,
    Setup { until: u64 },
    Read,
    Reading { txn: usize },
    Write,
    Writing { txn: usize },
    Done,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DmaRuntime {
    pub phase: DmaPhase,
    pub offset: usize,
    pub staging: Vec<u8>,
    pub cancelled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub memory: MemoryService,
    pub ddr: Option<super::ddr::Bridge>,
    pub cores: Vec<CoreRuntime>,
    pub caches: Vec<Vec<Option<CacheLine>>>,
    pub directory: BTreeMap<u32, Directory>,
    pub transactions: Vec<Transaction>,
    pub external: Vec<Option<usize>>,
    pub dma: Vec<DmaRuntime>,
    pub commands: usize,
    pub slots: Vec<Slot>,
    pub mailboxes: Vec<super::mailbox::Endpoint>,
    pub ipc_events: Vec<super::ipc::IpcEvent>,
    pub mailbox_accesses: BTreeMap<usize, super::mailbox::Access>,
    pub evictions: Vec<Eviction>,
    pub noc: FlitService,
    pub axi: super::super::transaction_fabric::service::AxiService,
    pub ahb: super::super::ahb_pipeline::service::AhbService,
    pub native_deliveries: Vec<(Transport, usize, Vec<u8>, bool)>,
    pub noc_deliveries: Vec<(String, Vec<u8>, String)>,
    pub sequence: u64,
    pub age: u64,
    pub admission_cursor: usize,
    pub axi_write_owner: Option<usize>,
    pub last_tick: Option<u64>,
    pub next_tick: Option<u64>,
    pub exported: bool,
}
