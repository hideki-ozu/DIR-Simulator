//! Committed result snapshot, not a resumable checkpoint of the event engine.
#[path = "snapshot/can.rs"]
pub mod can;
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

#[derive(Debug)]
pub struct Snapshot {
    pub common: CommonSnapshot,
    pub can: CanSnapshot,
    pub gateway: GatewaySnapshot,
    pub ethernet: Option<EthernetSnapshot>,
}
