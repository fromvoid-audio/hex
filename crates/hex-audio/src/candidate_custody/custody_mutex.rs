//! Exactly the five native-custody exclusions. No callback waiting policy.
//! The pinned dependency keeps lock state inline; payload descendants still
//! require their actual visitors. Control contention yields without fairness or
//! a bounded-wait guarantee. Try paths perform one attempt and never yield.
use spin::mutex::{SpinMutex, SpinMutexGuard};
use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) struct CustodyMutex<T> {
    value: SpinMutex<T, spin::Yield>,
    poisoned: AtomicBool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CustodyTryLockError {
    Contended,
    Poisoned,
}
pub(super) struct CustodyMutexGuard<'a, T> {
    value: SpinMutexGuard<'a, T>,
    poisoned: &'a AtomicBool,
    panic_at_acquire: bool,
    // Dependency guards are Send. Preserve the old std guard's non-Send law.
    _not_send: PhantomData<Rc<()>>,
}
impl<T> CustodyMutex<T> {
    pub(super) const fn new(value: T) -> Self {
        Self {
            value: SpinMutex::new(value),
            poisoned: AtomicBool::new(false),
        }
    }
    pub(super) fn try_lock(&self) -> Result<CustodyMutexGuard<'_, T>, CustodyTryLockError> {
        let value = self
            .value
            .try_lock()
            .ok_or(CustodyTryLockError::Contended)?;
        if self.poisoned.load(Ordering::Relaxed) {
            drop(value);
            return Err(CustodyTryLockError::Poisoned);
        }
        Ok(CustodyMutexGuard {
            value,
            poisoned: &self.poisoned,
            panic_at_acquire: std::thread::panicking(),
            _not_send: PhantomData,
        })
    }
    /// Existing control-only recovery. Never clears sticky poison; never call
    /// this from a callback or substitute it for an existing try path.
    pub(super) fn lock_recover_on_control(&self) -> CustodyMutexGuard<'_, T> {
        CustodyMutexGuard {
            value: self.value.lock(),
            poisoned: &self.poisoned,
            panic_at_acquire: std::thread::panicking(),
            _not_send: PhantomData,
        }
    }
}
impl<T: Default> Default for CustodyMutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
impl<T> Deref for CustodyMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}
impl<T> DerefMut for CustodyMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}
impl<T> Drop for CustodyMutexGuard<'_, T> {
    fn drop(&mut self) {
        if !self.panic_at_acquire && std::thread::panicking() {
            // This runs before Rust drops value and releases its exclusion.
            self.poisoned.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
