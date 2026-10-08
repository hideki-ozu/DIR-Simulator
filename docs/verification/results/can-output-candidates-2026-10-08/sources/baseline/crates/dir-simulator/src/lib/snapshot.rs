//! Committed result snapshot, not a resumable checkpoint of the event engine.
#[path = "snapshot/can.rs"]
pub mod can;
#[path = "snapshot/can_ethernet.rs"]
pub mod can_ethernet;
#[path = "snapshot/common.rs"]
pub mod common;
#[path = "snapshot/gateway.rs"]
pub mod gateway;

#[path = "snapshot/ethernet.rs"]
pub mod ethernet;
pub use can::{CanSnapshot, Receiver, Request};
pub use common::{CommonSnapshot, Point};
pub use ethernet::EthernetSnapshot;
pub use gateway::{ForwardRecord, GatewaySnapshot, RequestLineage, RxBufferRecord};
#[path = "snapshot/axi.rs"]
pub mod axi;
#[path = "snapshot/canfd.rs"]
pub mod canfd;
#[path = "snapshot/memory_ipc.rs"]
pub mod memory_ipc;
#[path = "snapshot/soc.rs"]
pub mod soc;

#[derive(Debug)]
pub struct Snapshot {
    pub registered: Option<crate::registry::RegisteredSnapshot>,
    pub network: Option<serde_json::Value>,
    pub common: CommonSnapshot,
    pub can: CanSnapshot,
    pub gateway: GatewaySnapshot,
    pub ethernet: Option<EthernetSnapshot>,
    pub canfd: Option<canfd::CanFdSnapshot>,
    pub axi: Option<axi::AxiSnapshot>,
    pub soc: Option<soc::SocSnapshot>,
    pub memory_ipc: Option<memory_ipc::MemoryIpcSnapshot>,
}

impl Snapshot {
    /// Empty committed journal for a model that owns its own transaction state.
    pub(crate) fn empty(prepared: &crate::types::PreparedSimulation) -> Self {
        Self {
            registered: None,
            network: None,
            common: CommonSnapshot {
                point_spool: None,
                spool_error: None,
                termination: "events_exhausted".into(),
                partial: false,
                end_ps: prepared.common.time_limit_ps,
                last_event_time_ps: None,
                committed_events: 0,
                pending_events: 0,
                points: Vec::new(),
                diagnostics: Vec::new(),
            },
            can: CanSnapshot {
                archive: None,
                bus_state: "idle".into(),
                bus_states: Vec::new(),
                requests: Vec::new(),
                receivers: Vec::new(),
            },
            gateway: GatewaySnapshot {
                forwards: Vec::new(),
                rx_buffers: Vec::new(),
                request_lineage: Default::default(),
            },
            ethernet: None,
            canfd: None,
            axi: None,
            soc: None,
            memory_ipc: None,
        }
    }
}
