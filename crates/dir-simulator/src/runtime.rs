//! Deterministic execution facade. Engine state remains private to the runtime.
pub mod can;
mod engine;
mod gateway;
mod scheduler;

pub use engine::simulate;

#[cfg(test)]
mod tests;
