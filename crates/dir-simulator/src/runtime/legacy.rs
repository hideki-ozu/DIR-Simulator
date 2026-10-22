//! Internal commit boundary shared by the dedicated family steppers.
//! A successful step is one existing callback transaction, never a replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepOutcome {
    Committed,
    Exhausted,
    Failed,
}
