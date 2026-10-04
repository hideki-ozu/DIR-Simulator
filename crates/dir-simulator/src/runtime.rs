//! Deterministic execution facade. Engine state remains private to the runtime.
pub mod can;
mod engine;
mod gateway;
mod scheduler;

pub mod axi;
pub mod canfd;
pub mod ethernet;
pub mod memory_ipc;
pub mod soc;
pub fn simulate(
    prepared: &crate::types::PreparedSimulation,
) -> Result<crate::snapshot::Snapshot, crate::types::Diagnostic> {
    match prepared.common.profile.as_str() {
        "ethernet.l2.store-forward.v1"
        | "ethernet.l2.qos.v1"
        | "ethernet.l2.vlan.v1"
        | "ethernet.l2.store-forward.v2"
        | "ethernet.l2.100base-t1.v1" => ethernet::simulate(prepared),
        "can.fd.precomputed.v1" => canfd::simulate(prepared),
        "axi4.transaction.v1" => axi::simulate(prepared),
        "soc.shared.v1" | "ahb.transaction.v1" | "noc.xy.v1" => soc::simulate(prepared),
        "memory.ipc.transaction.v1" => memory_ipc::simulate(prepared),
        "can.cc.ideal.v1" | "can.cc.multibus.v1" => engine::simulate(prepared),
        _ => Err(crate::types::Diagnostic::execution(
            "unsupported runtime profile",
        )),
    }
}

#[cfg(test)]
mod tests;
