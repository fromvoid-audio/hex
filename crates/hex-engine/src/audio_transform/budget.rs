use crate::Invalid;
#[cfg(test)]
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
pub(super) const RETAINED_LIMIT: usize = 768 * 1024 * 1024;
#[cfg(test)]
thread_local! {
    pub(super) static RELEASED_PRODUCTION_RESERVATIONS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
#[derive(Debug)]
pub(super) struct Budget {
    used: AtomicUsize,
    limit: usize,
}
impl Budget {
    pub(super) const fn new(limit: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            limit,
        }
    }
    pub(super) fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
}
// The existing production counter has process lifetime. Reservations borrow it;
// they cannot hide a shared owning allocation or perform its final Arc drop.
static PRODUCTION_BUDGET: Budget = Budget::new(RETAINED_LIMIT);

#[derive(Clone, Debug)]
pub(super) enum BudgetOwner {
    Production(&'static Budget),
    #[cfg(test)]
    Local(Arc<Budget>),
}
impl BudgetOwner {
    #[cfg(test)]
    pub(super) fn local(limit: usize) -> Self {
        Self::Local(Arc::new(Budget::new(limit)))
    }
    fn counter(&self) -> &Budget {
        match self {
            Self::Production(budget) => budget,
            #[cfg(test)]
            Self::Local(budget) => budget,
        }
    }
    pub(super) fn used(&self) -> usize {
        self.counter().used()
    }
    pub(super) fn reserve(&self, bytes: usize) -> Result<Reservation, Invalid> {
        let counter = self.counter();
        counter
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|total| *total <= counter.limit)
            })
            .map_err(|_| Invalid("derived audio exceeds the 768 mib retained-buffer budget"))?;
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}
#[derive(Debug)]
pub(crate) struct Reservation {
    budget: BudgetOwner,
    bytes: usize,
}
impl Reservation {
    pub(crate) fn covers_builtin_prepared_pcm(&self, bytes: usize) -> bool {
        matches!(&self.budget, BudgetOwner::Production(budget) if std::ptr::eq(*budget, &PRODUCTION_BUDGET))
            && bytes <= self.bytes
    }
    pub(crate) fn reserved_bytes(&self) -> usize {
        self.bytes
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        #[cfg(test)]
        if self.covers_builtin_prepared_pcm(0) {
            RELEASED_PRODUCTION_RESERVATIONS.with(|released| {
                let (count, bytes) = released.get();
                released.set((count + 1, bytes + self.bytes));
            });
        }
        self.budget
            .counter()
            .used
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub(super) fn production_budget() -> BudgetOwner {
    BudgetOwner::Production(&PRODUCTION_BUDGET)
}
