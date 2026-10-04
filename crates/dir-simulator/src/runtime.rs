//! Deterministic execution facade. Engine state remains private to the runtime.
pub mod can;
mod engine;
mod gateway;
mod scheduler;

pub mod ethernet;
pub fn simulate(
    prepared: &crate::types::PreparedSimulation,
) -> Result<crate::snapshot::Snapshot, crate::types::Diagnostic> {
    match prepared.common.profile.as_str() {
        "ethernet.l2.store-forward.v1" | "ethernet.l2.qos.v1" | "ethernet.l2.vlan.v1" => {
            ethernet::simulate(prepared)
        }
        "can.cc.ideal.v1" | "can.cc.multibus.v1" => engine::simulate(prepared),
        _ => Err(crate::types::Diagnostic::execution(
            "unsupported runtime profile",
        )),
    }
}

#[cfg(test)]
mod tests;
