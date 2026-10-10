//! Wall time spent dispatching events. Kept outside snapshots and published results.
use std::cell::RefCell;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Measurement {
    elapsed: Duration,
    scope_depth: usize,
    started: Option<Instant>,
}

thread_local! {
    static MEASUREMENTS: RefCell<Vec<Measurement>> = const { RefCell::new(Vec::new()) };
}

struct MeasurementGuard {
    active: bool,
}

impl MeasurementGuard {
    fn finish(mut self) -> Duration {
        let elapsed = MEASUREMENTS.with(|stack| {
            let mut stack = stack.borrow_mut();
            let measurement = stack.pop().expect("active wall-time measurement");
            debug_assert_eq!(measurement.scope_depth, 0);
            if let Some(parent) = stack.last_mut() {
                if parent.scope_depth > 0 {
                    parent.started = Some(Instant::now());
                }
            }
            measurement.elapsed
        });
        self.active = false;
        elapsed
    }
}

impl Drop for MeasurementGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        MEASUREMENTS.with(|stack| {
            let mut stack = stack.borrow_mut();
            stack.pop();
            if let Some(parent) = stack.last_mut() {
                if parent.scope_depth > 0 {
                    parent.started = Some(Instant::now());
                }
            }
        });
    }
}

/// Measure only the event-loop scopes reached by `operation`.
/// Nested runs pause and restore the caller's active scope.
pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, Duration) {
    MEASUREMENTS.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(parent) = stack.last_mut() {
            if let Some(started) = parent.started.take() {
                parent.elapsed += started.elapsed();
            }
        }
        stack.push(Measurement::default());
    });
    let guard = MeasurementGuard { active: true };
    let result = operation();
    (result, guard.finish())
}

pub(crate) struct EventLoopGuard {
    measured: bool,
}

/// Mark the actual dispatch loop; preparation and result publication stay outside it.
pub(crate) fn event_loop() -> EventLoopGuard {
    let measured = MEASUREMENTS.with(|stack| {
        let mut stack = stack.borrow_mut();
        let Some(current) = stack.last_mut() else {
            return false;
        };
        if current.scope_depth == 0 {
            current.started = Some(Instant::now());
        }
        current.scope_depth += 1;
        true
    });
    EventLoopGuard { measured }
}

impl Drop for EventLoopGuard {
    fn drop(&mut self) {
        if !self.measured {
            return;
        }
        MEASUREMENTS.with(|stack| {
            let mut stack = stack.borrow_mut();
            let current = stack.last_mut().expect("active wall-time measurement");
            current.scope_depth -= 1;
            if current.scope_depth == 0 {
                current.elapsed += current.started.take().expect("event-loop start").elapsed();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_scoped_work_and_restores_nested_measurements() {
        let (_, unscoped) = measure(|| std::thread::sleep(Duration::from_millis(1)));
        assert_eq!(unscoped, Duration::ZERO);
        let (nested, outer) = measure(|| {
            let _outer = event_loop();
            let (_, nested) = measure(|| {
                let _inner = event_loop();
                std::thread::sleep(Duration::from_millis(1));
            });
            nested
        });
        assert!(nested > Duration::ZERO);
        assert!(outer > Duration::ZERO);
        let (_, after) = measure(|| {});
        assert_eq!(after, Duration::ZERO);
    }
}
