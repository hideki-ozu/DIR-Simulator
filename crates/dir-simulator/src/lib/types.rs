//! Shared input contracts. No runtime or exporter dependencies.
#[path = "types/can.rs"]
pub mod can;
#[path = "types/common.rs"]
pub mod common;
#[path = "types/gateway.rs"]
pub mod gateway;
#[path = "types/prepared.rs"]
pub mod prepared;

pub use can::{Bus, Controller, Frame, Generator, PreparedCan};
pub use common::{Diagnostic, InputSnapshot, PreparedCommon, Schedule, SourceSpan};
pub use gateway::{Gateway, PreparedGateway, Route};
pub use prepared::PreparedSimulation;
#[path = "types/can_ethernet.rs"]
pub mod can_ethernet;
#[path = "types/ethernet.rs"]
pub mod ethernet;
#[path = "types/network.rs"]
pub mod network;
pub use ethernet::PreparedEthernet;
#[path = "types/canfd.rs"]
pub mod canfd;
pub use canfd::PreparedCanFd;
#[path = "types/axi.rs"]
pub mod axi;
#[path = "types/memory_ipc.rs"]
pub mod memory_ipc;
#[path = "types/noc_flit.rs"]
pub mod noc_flit;
#[path = "types/soc.rs"]
pub mod soc;
#[path = "types/soc_execution.rs"]
pub mod soc_execution;
#[path = "types/transaction_fabric.rs"]
pub mod transaction_fabric;
#[path = "types/wire.rs"]
pub mod wire;
