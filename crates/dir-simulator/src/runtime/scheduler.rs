//! Deterministic reservation key and preflight/publication rules.
use super::engine::Engine;
use crate::types::Diagnostic;
use std::cmp::Reverse;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Event {
    Dispatch,
    Generate(usize, u64),
    TxProcessed(usize),
    Ready(usize),
    Arbitrate(usize),
    RoutingInput(usize, usize, usize),
    ForwardDue(usize),
    Forward(usize),
    GwTxSpace(usize),
    Eof(usize),
    Release(usize),
    Observe(usize, usize, usize), // request, controller, receiver row
    RxProcessed(usize, usize, usize),
    Received(usize, usize, usize),
}
impl Event {
    pub(super) fn phase(&self) -> u8 {
        match self {
            Self::TxProcessed(_)
            | Self::Eof(_)
            | Self::Release(_)
            | Self::RxProcessed(..)
            | Self::ForwardDue(_) => 0,
            Self::Arbitrate(_) => 2,
            _ => 1,
        }
    }
}
pub(super) type Key = (u64, u64, u8, u64);

pub(super) fn add(a: u64, b: u64) -> Result<u64, Diagnostic> {
    a.checked_add(b)
        .ok_or_else(|| Diagnostic::execution("time arithmetic overflow"))
}
pub(super) fn limit_error(message: &str) -> Diagnostic {
    let mut d = Diagnostic::execution(message);
    d.code = "E-0004".into();
    d
}

impl Engine<'_> {
    pub(super) fn preflight(&self, events: &[(u64, Event)]) -> Result<(), Diagnostic> {
        self.next_sequence
            .checked_add(events.len() as u64)
            .ok_or_else(|| limit_error("event sequence overflow"))?;
        for (time, event) in events {
            if *time < self.now.0 {
                return Err(Diagnostic::execution("attempt to schedule in the past"));
            }
            if *time == self.now.0 && event.phase() < self.now.2 {
                self.now
                    .1
                    .checked_add(1)
                    .ok_or_else(|| limit_error("delta overflow"))?;
            }
        }
        Ok(())
    }
    pub(super) fn publish(&mut self, events: Vec<(u64, Event)>) {
        for (time, event) in events {
            let phase = event.phase();
            let delta = if time == self.now.0 {
                self.now.1 + u64::from(phase < self.now.2)
            } else {
                0
            };
            self.heap
                .push(Reverse(((time, delta, phase, self.next_sequence), event)));
            self.next_sequence += 1;
        }
    }
    pub(super) fn dirty(&mut self, bus: usize) {
        self.dirty
            .insert((self.now.0, self.now.1 + u64::from(self.now.2 == 2), bus));
    }
}
