//! Fallible reservations for allocations that precede committed publication.
use crate::types::Diagnostic;

#[cfg(test)]
thread_local! {
    static FAIL_NEXT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn fail_next_reservation(operation: &'static str) {
    FAIL_NEXT.with(|slot| slot.set(Some(operation)));
}

pub(crate) fn reservation_checkpoint(operation: &str) -> Result<(), Diagnostic> {
    #[cfg(test)]
    if FAIL_NEXT.with(|slot| {
        if slot.get() == Some(operation) {
            slot.set(None);
            true
        } else {
            false
        }
    }) {
        return Err(allocation_diagnostic(operation));
    }
    #[cfg(not(test))]
    let _ = operation;
    Ok(())
}

pub(crate) fn allocation_diagnostic(operation: &str) -> Diagnostic {
    Diagnostic::execution("memory allocation failed")
        .with_reason("allocation_failed")
        .with_detail("operation", operation)
}

pub(crate) fn reserve_vec<T>(
    vec: &mut Vec<T>,
    additional: usize,
    operation: &str,
) -> Result<(), Diagnostic> {
    reservation_checkpoint(operation)?;
    vec.try_reserve(additional)
        .map_err(|_| allocation_diagnostic(operation))
}

pub(crate) fn copy_string(source: &str, operation: &str) -> Result<String, Diagnostic> {
    reservation_checkpoint(operation)?;
    let mut owned = String::new();
    owned
        .try_reserve(source.len())
        .map_err(|_| allocation_diagnostic(operation))?;
    owned.push_str(source);
    Ok(owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_overflow_is_structured_and_keeps_vector_intact() {
        let mut committed = vec![7u64];
        let result = reserve_vec(&mut committed, usize::MAX, "test_reservation").unwrap_err();
        assert_eq!(result.code, "E-0002");
        assert_eq!(result.reason, "allocation_failed");
        assert_eq!(result.details.unwrap()["operation"], "test_reservation");
        assert_eq!(committed, [7]);
    }
}
