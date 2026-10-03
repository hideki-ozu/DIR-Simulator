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
pub use common::{Diagnostic, InputSnapshot, PreparedCommon, Schedule};
pub use gateway::{Gateway, PreparedGateway, Route};
pub use prepared::PreparedSimulation;
