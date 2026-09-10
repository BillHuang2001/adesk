//! Shared concurrency helpers.
//!
//! The manager's registry and the mock backend's interior state are both behind
//! a [`Mutex`]. A poisoned lock can only appear when a thread panicked while
//! holding it, and none of the guarded critical sections can leave their data
//! inconsistent when they unwind, so the crate recovers the data instead of
//! propagating the panic — request and command paths must never panic.

use std::sync::{Mutex, MutexGuard};

/// Locks `mutex`, recovering the data from a poisoned lock.
pub(crate) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
