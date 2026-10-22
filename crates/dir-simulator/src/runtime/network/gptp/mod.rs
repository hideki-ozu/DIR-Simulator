//! Packet-derived gPTP clock discipline on the shared Ethernet scheduler.
mod checkpoint;
pub(super) mod codec;
pub(super) mod integration;
pub(super) mod math;
mod receive;
mod replay;
mod selection;
pub(in crate::runtime::network) mod state;
