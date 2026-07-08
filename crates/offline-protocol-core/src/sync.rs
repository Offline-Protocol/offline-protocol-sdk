//! Poison-recovering lock extensions.
//!
//! The SDK runs on mobile platforms where a process crash is worse than
//! operating on potentially inconsistent state. A panic on one thread while
//! holding a lock poisons it; with plain `.lock().unwrap()` every later
//! access panics too, permanently wedging the node. Mutex poisoning in Rust
//! is advisory — the protected data is still structurally valid — so these
//! helpers recover the guard via [`std::sync::PoisonError::into_inner`] and
//! log a warning with the caller's location for observability.
//!
//! Use these in code driven by foreign (FFI) threads and in event-emission
//! paths that must keep flowing. Test code should keep `.lock().unwrap()`
//! so poisoning fails loudly.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Extension trait adding poison-recovering locking to [`Mutex`].
pub trait MutexExt<T> {
    /// Locks the mutex, recovering the guard if it is poisoned.
    ///
    /// On poison, logs a warning naming the call site and returns the inner
    /// guard anyway.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    #[track_caller]
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        let location = std::panic::Location::caller();
        self.lock().unwrap_or_else(|e| {
            tracing::warn!(location = %location, "Mutex poisoned — recovering with inner value");
            e.into_inner()
        })
    }
}

/// Extension trait adding poison-recovering locking to [`RwLock`].
pub trait RwLockExt<T> {
    /// Acquires a read guard, recovering it if the lock is poisoned.
    ///
    /// On poison, logs a warning naming the call site and returns the inner
    /// guard anyway.
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T>;

    /// Acquires a write guard, recovering it if the lock is poisoned.
    ///
    /// On poison, logs a warning naming the call site and returns the inner
    /// guard anyway.
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    #[track_caller]
    fn read_or_recover(&self) -> RwLockReadGuard<'_, T> {
        let location = std::panic::Location::caller();
        self.read().unwrap_or_else(|e| {
            tracing::warn!(location = %location, "RwLock poisoned — recovering with inner value");
            e.into_inner()
        })
    }

    #[track_caller]
    fn write_or_recover(&self) -> RwLockWriteGuard<'_, T> {
        let location = std::panic::Location::caller();
        self.write().unwrap_or_else(|e| {
            tracing::warn!(location = %location, "RwLock poisoned — recovering with inner value");
            e.into_inner()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn poison_mutex(lock: &Arc<Mutex<Vec<u32>>>) {
        let cloned = Arc::clone(lock);
        let _ = thread::spawn(move || {
            let _guard = cloned.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
    }

    #[test]
    fn mutex_recovers_after_poison() {
        let lock = Arc::new(Mutex::new(vec![1u32]));
        poison_mutex(&lock);
        assert!(lock.is_poisoned());

        let mut guard = lock.lock_or_recover();
        guard.push(2);
        assert_eq!(*guard, vec![1, 2]);
        drop(guard);

        // Subsequent recoveries keep working too.
        assert_eq!(lock.lock_or_recover().len(), 2);
    }

    #[test]
    fn mutex_unpoisoned_behaves_normally() {
        let lock = Mutex::new(7u32);
        assert_eq!(*lock.lock_or_recover(), 7);
    }

    #[test]
    fn rwlock_recovers_after_poison() {
        let lock = Arc::new(RwLock::new(vec![1u32]));
        let cloned = Arc::clone(&lock);
        let _ = thread::spawn(move || {
            let _guard = cloned.write().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(lock.is_poisoned());

        lock.write_or_recover().push(2);
        assert_eq!(*lock.read_or_recover(), vec![1, 2]);
    }
}
