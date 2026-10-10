//! Deterministic execution facade. Engine state remains private to the runtime.
pub mod can;
pub mod can_archive;
pub(crate) mod can_ethernet;
mod engine;
mod gateway;
pub(crate) mod network;
pub(crate) mod registered;
mod scheduler;
pub mod spool;
pub(crate) mod timing;

pub mod axi;
pub mod canfd;
pub mod ethernet;
pub mod memory_ipc;
pub mod soc;
pub fn simulate(
    prepared: &crate::types::PreparedSimulation,
) -> Result<crate::snapshot::Snapshot, crate::types::Diagnostic> {
    #[cfg(test)]
    test_probe::facade();
    let mut snapshot = if prepared.registered.is_some() {
        registered::simulate(prepared)?
    } else {
        simulate_builtin(prepared)?
    };
    if snapshot.common.spool_error.is_none() {
        if let Err(d) = snapshot.common.spool_checkpoint(true) {
            snapshot.common.spool_error = Some(d);
        }
    }
    Ok(snapshot)
}

pub(crate) fn simulate_builtin(
    prepared: &crate::types::PreparedSimulation,
) -> Result<crate::snapshot::Snapshot, crate::types::Diagnostic> {
    #[cfg(test)]
    test_probe::builtin();
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

#[cfg(test)]
pub(crate) mod test_probe {
    use std::cell::Cell;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) struct Counts {
        pub facade: u64,
        pub builtin: u64,
        pub can_engine: u64,
        pub can_event_callbacks: u64,
    }
    thread_local! {
        static COUNTS: Cell<Counts> = Cell::new(Counts::default());
    }
    pub(crate) fn reset() {
        COUNTS.with(|counts| counts.set(Counts::default()));
    }
    pub(crate) fn read() -> Counts {
        COUNTS.with(Cell::get)
    }
    fn update(change: impl FnOnce(&mut Counts)) {
        COUNTS.with(|counts| {
            let mut value = counts.get();
            change(&mut value);
            counts.set(value);
        });
    }
    pub(super) fn facade() {
        update(|c| c.facade += 1);
    }
    pub(super) fn builtin() {
        update(|c| c.builtin += 1);
    }
    pub(super) fn can_engine() {
        update(|c| c.can_engine += 1);
    }
    pub(super) fn can_event_callback() {
        update(|c| c.can_event_callbacks += 1);
    }
}
