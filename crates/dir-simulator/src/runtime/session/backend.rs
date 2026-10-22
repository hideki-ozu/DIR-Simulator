//! Dispatch owned callback continuations without replaying a simulation.
use super::ControlError;
use crate::snapshot::Snapshot;
use crate::{Diagnostic, PreparedSimulation};
use std::sync::Arc;

enum Continuation {
    Registered(super::super::registered::Engine),
    Can(super::super::engine::Stepper),
    CanFd(super::super::canfd::Stepper),
    Axi(super::super::axi::Stepper),
    Ethernet(super::super::ethernet::Stepper),
    Soc(super::super::soc::Stepper),
    MemoryIpc(super::super::memory_ipc::Stepper),
}

pub(super) struct SessionBackend {
    pub(super) prepared: Arc<PreparedSimulation>,
    continuation: Continuation,
}
impl SessionBackend {
    pub(super) fn new(prepared: Arc<PreparedSimulation>) -> Result<Self, Diagnostic> {
        let continuation = if prepared.registered.as_ref().is_some_and(|r| r.is_generic()) {
            Continuation::Registered(super::super::registered::Engine::new(Arc::clone(
                &prepared,
            ))?)
        } else {
            match prepared.common.profile.as_str() {
                "can.cc.ideal.v1" | "can.cc.multibus.v1" => {
                    Continuation::Can(super::super::engine::Stepper::new(Arc::clone(&prepared))?)
                }
                "can.fd.precomputed.v1" | "can.fd.iso.v1" => {
                    Continuation::CanFd(super::super::canfd::Stepper::new(Arc::clone(&prepared))?)
                }
                "ethernet.l2.store-forward.v1"
                | "ethernet.l2.qos.v1"
                | "ethernet.l2.vlan.v1"
                | "ethernet.l2.store-forward.v2"
                | "ethernet.l2.100base-t1.v1" => Continuation::Ethernet(
                    super::super::ethernet::Stepper::new(Arc::clone(&prepared))?,
                ),
                "axi4.transaction.v1" => {
                    Continuation::Axi(super::super::axi::Stepper::new(Arc::clone(&prepared))?)
                }
                "soc.shared.v1" | "ahb.transaction.v1" | "noc.xy.v1" => {
                    Continuation::Soc(super::super::soc::Stepper::new(Arc::clone(&prepared))?)
                }
                "memory.ipc.transaction.v1" => Continuation::MemoryIpc(
                    super::super::memory_ipc::Stepper::new(Arc::clone(&prepared))?,
                ),
                _ => {
                    return Err(Diagnostic::execution(
                        "profile has no cooperative continuation",
                    ));
                }
            }
        };
        Ok(Self {
            prepared,
            continuation,
        })
    }
    pub(super) fn is_legacy(&self) -> bool {
        !matches!(self.continuation, Continuation::Registered(_))
    }
    pub(super) fn registered(&self) -> Result<&super::super::registered::Engine, ControlError> {
        match &self.continuation {
            Continuation::Registered(engine) => Ok(engine),
            _ => Err(ControlError::new(
                "UnsupportedCapability",
                "backend has no checkpoint codec",
            )),
        }
    }
    pub(super) fn registered_mut(
        &mut self,
    ) -> Result<&mut super::super::registered::Engine, ControlError> {
        match &mut self.continuation {
            Continuation::Registered(engine) => Ok(engine),
            _ => Err(ControlError::new(
                "UnsupportedCapability",
                "backend has no checkpoint codec",
            )),
        }
    }
    pub(super) fn snapshot(&self) -> &Snapshot {
        match &self.continuation {
            Continuation::Registered(engine) => &engine.snapshot,
            Continuation::Can(engine) => engine.snapshot(),
            Continuation::CanFd(engine) => engine.snapshot(),
            Continuation::Axi(engine) => engine.snapshot(),
            Continuation::Ethernet(engine) => engine.snapshot(),
            Continuation::Soc(engine) => engine.snapshot(),
            Continuation::MemoryIpc(engine) => engine.snapshot(),
        }
    }
    pub(super) fn snapshot_mut(&mut self) -> &mut Snapshot {
        match &mut self.continuation {
            Continuation::Registered(engine) => &mut engine.snapshot,
            Continuation::Can(engine) => engine.snapshot_mut(),
            Continuation::CanFd(engine) => engine.snapshot_mut(),
            Continuation::Axi(engine) => engine.snapshot_mut(),
            Continuation::Ethernet(engine) => engine.snapshot_mut(),
            Continuation::Soc(engine) => engine.snapshot_mut(),
            Continuation::MemoryIpc(engine) => engine.snapshot_mut(),
        }
    }
    pub(super) fn initialize(&mut self) -> Result<(), Diagnostic> {
        match &mut self.continuation {
            Continuation::Registered(engine) => engine.initialize().map_err(|error| {
                let target = engine.preparation_target.clone();
                engine.fail(error, None, target.as_deref(), true, "model_failed");
                engine
                    .snapshot
                    .common
                    .diagnostics
                    .last()
                    .expect("initialization diagnostic")
                    .clone()
            }),
            Continuation::Can(engine) => engine.initialize(),
            Continuation::CanFd(engine) => engine.initialize(),
            Continuation::Axi(engine) => engine.initialize(),
            Continuation::Ethernet(engine) => engine.initialize(),
            Continuation::Soc(engine) => engine.initialize(),
            Continuation::MemoryIpc(engine) => engine.initialize(),
        }
    }
    pub(super) fn step(&mut self) -> bool {
        use super::super::legacy::StepOutcome;
        let committed = match &mut self.continuation {
            Continuation::Registered(engine) => engine.step(),
            Continuation::Can(engine) => engine.step() == StepOutcome::Committed,
            Continuation::CanFd(engine) => engine.step() == StepOutcome::Committed,
            Continuation::Axi(engine) => engine.step() == StepOutcome::Committed,
            Continuation::Ethernet(engine) => engine.step() == StepOutcome::Committed,
            Continuation::Soc(engine) => engine.step() == StepOutcome::Committed,
            Continuation::MemoryIpc(engine) => engine.step() == StepOutcome::Committed,
        };
        if committed
            && matches!(
                self.continuation,
                Continuation::Can(_)
                    | Continuation::Ethernet(_)
                    | Continuation::Soc(_)
                    | Continuation::MemoryIpc(_)
            )
        {
            self.snapshot_mut().common.checkpoint()
        } else {
            committed
        }
    }
    pub(super) fn refresh_view(&mut self, horizon: u64) -> Result<(), Diagnostic> {
        match &mut self.continuation {
            Continuation::Registered(engine) => {
                engine.refresh_view(horizon)?;
                if engine.registered.network.is_none() {
                    engine.snapshot.common.pending_events =
                        (engine.queue.len() + engine.dirty.len()) as u64;
                }
                Ok(())
            }
            Continuation::Can(engine) => engine.refresh_prefix(horizon),
            Continuation::CanFd(engine) => engine.refresh_prefix(horizon),
            Continuation::Axi(engine) => engine.refresh_prefix(horizon),
            Continuation::Ethernet(engine) => engine.refresh_prefix(horizon),
            Continuation::Soc(engine) => engine.refresh_prefix(horizon),
            Continuation::MemoryIpc(engine) => engine.refresh_prefix(horizon),
        }
    }
    pub(super) fn fail_initialization(&mut self, diagnostic: Diagnostic) {
        if self.snapshot().common.partial {
            return;
        }
        let common = &mut self.snapshot_mut().common;
        common.partial = true;
        common.termination = "model_failed".into();
        common.diagnostics.push(diagnostic);
    }
    pub(super) fn finish(&mut self) {
        match &mut self.continuation {
            Continuation::Registered(engine) => engine.finish(),
            Continuation::Can(engine) => engine.finish_once(),
            Continuation::CanFd(engine) => engine.finish_once(),
            Continuation::Axi(engine) => engine.finish_once(),
            Continuation::Ethernet(engine) => engine.finish_once(),
            Continuation::Soc(engine) => engine.finish_once(),
            Continuation::MemoryIpc(engine) => engine.finish_once(),
        }
    }
}
