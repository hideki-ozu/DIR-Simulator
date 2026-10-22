//! Event-driven digital DDR4 target. The target owns timing, never byte storage.
mod controller;
mod device;
mod types;

pub use controller::Service;
pub use device::{Bank, Device, Phase};
pub use types::{
    CAPACITY, Command, Config, Effect, HALF, MODE, MR, Operation, Request, SOURCE_SHA256, TCK,
    TREFI, Violation, address, offset,
};
