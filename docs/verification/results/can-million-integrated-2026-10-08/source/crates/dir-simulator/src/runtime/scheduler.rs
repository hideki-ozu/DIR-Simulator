//! Deterministic reservation key and preflight/publication rules.
use super::engine::Engine;
use crate::allocation::{allocation_diagnostic, reservation_checkpoint, reserve_vec};
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
type DirtyKey = (u64, u64, usize);
type SealedDirty = (Vec<DirtyKey>, Vec<(u64, Event)>);

pub(super) fn add(a: u64, b: u64) -> Result<u64, Diagnostic> {
    a.checked_add(b)
        .ok_or_else(|| limit_error("time arithmetic overflow"))
}
pub(super) fn limit_error(message: &str) -> Diagnostic {
    let mut d = Diagnostic::execution(message);
    d.code = "E-0004".into();
    d.reason = if message.contains("max-events") {
        "event_limit"
    } else if message.contains("max-delta") {
        "delta_cycle_limit"
    } else {
        "arithmetic_overflow"
    }
    .into();
    d
}

impl Engine<'_> {
    pub(super) fn preflight(&mut self, events: &[(u64, Event)]) -> Result<(), Diagnostic> {
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
        // BinaryHeap::push cannot allocate once the complete batch fits.
        reservation_checkpoint("schedule_events")?;
        self.heap
            .try_reserve(events.len())
            .map_err(|_| allocation_diagnostic("schedule_events"))?;
        Ok(())
    }
    pub(super) fn seal_dirty_preflight(
        &mut self,
        time: u64,
        delta: u64,
    ) -> Result<SealedDirty, Diagnostic> {
        let count = self
            .dirty
            .range((time, delta, 0)..=(time, delta, usize::MAX))
            .count();
        let mut dirty = Vec::new();
        reserve_vec(&mut dirty, count, "seal_arbitration")?;
        let mut events = Vec::new();
        reserve_vec(&mut events, count, "seal_arbitration")?;
        for &key in self
            .dirty
            .range((time, delta, 0)..=(time, delta, usize::MAX))
        {
            dirty.push(key);
            events.push((time, Event::Arbitrate(key.2)));
        }
        self.preflight(&events).map_err(|mut diagnostic| {
            if diagnostic.reason == "allocation_failed" {
                diagnostic = diagnostic.with_detail("operation", "seal_arbitration");
            }
            diagnostic
        })?;
        Ok((dirty, events))
    }
    pub(super) fn publish(&mut self, events: Vec<(u64, Event)>) {
        for (time, event) in events {
            self.retain_event(&event);
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

#[cfg(test)]
mod tests {
    #[test]
    fn initial_schedule_allocation_failure_is_diagnostic() {
        let config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gateway/fanout.ini");
        let prepared = crate::prepare(&config).unwrap();
        crate::allocation::fail_next_reservation("schedule_events");
        let diagnostic = super::super::engine::simulate(&prepared).unwrap_err();
        assert_eq!(diagnostic.code, "E-0002");
        assert_eq!(diagnostic.reason, "allocation_failed");
        assert_eq!(diagnostic.details.unwrap()["operation"], "schedule_events");
    }
}
