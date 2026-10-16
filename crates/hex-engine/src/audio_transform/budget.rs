use crate::Invalid;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
pub(super) const RETAINED_LIMIT: usize = 768 * 1024 * 1024;
#[derive(Debug)]
pub(super) struct Budget {
    used: AtomicUsize,
    limit: usize,
}
impl Budget {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            limit,
        }
    }
    pub(super) fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    pub(super) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Reservation, Invalid> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|total| *total <= self.limit)
            })
            .map_err(|_| Invalid("derived audio exceeds the 768 mib retained-buffer budget"))?;
        Ok(Reservation {
            budget: Arc::clone(self),
            bytes,
        })
    }
}
#[derive(Debug)]
pub(crate) struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub(super) fn production_budget() -> Arc<Budget> {
    static BUDGET: OnceLock<Arc<Budget>> = OnceLock::new();
    Arc::clone(BUDGET.get_or_init(|| Arc::new(Budget::new(RETAINED_LIMIT))))
}
