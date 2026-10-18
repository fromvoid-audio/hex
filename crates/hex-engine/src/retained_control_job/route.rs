//! Nonowning correlation minted only with the actual fresh job constructor.
use super::RetainedControlJobBuildError;
use std::{
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

pub(super) static NEXT_ROUTE: AtomicU64 = AtomicU64::new(1);

/// Opaque never-reused routing correlation. Not a lease or provenance witness.
///
/// ```compile_fail
/// let fake = hex_engine::RetainedControlJobRoute::from(7_u64);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedControlJobRoute(NonZeroU64);
impl RetainedControlJobRoute {
    pub(super) fn cancellation_ordinal(self) -> u64 {
        self.0.get()
    }
    pub(super) fn mint(counter: &AtomicU64) -> Result<Self, RetainedControlJobBuildError> {
        let value = counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                if value == 0 {
                    None
                } else {
                    value.checked_add(1)
                }
            })
            .map_err(|_| RetainedControlJobBuildError::IdentityExhausted)?;
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(RetainedControlJobBuildError::IdentityExhausted)
    }
}

/// A bounded lookup hint beside an actual family slot. A match is only a
/// candidate: the actual guarded holder route/incarnation/type must be checked.
/// There is no API to manufacture a route from a hint or caller-authored number.
#[derive(Default)]
pub struct RetainedControlJobRouteHint(AtomicU64);
impl RetainedControlJobRouteHint {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }
    pub fn publish(&self, route: RetainedControlJobRoute) {
        self.0.store(route.0.get(), Ordering::Release);
    }
    pub fn clear(&self) {
        self.0.store(0, Ordering::Release);
    }
    pub fn matches(&self, route: RetainedControlJobRoute) -> bool {
        self.0.load(Ordering::Acquire) == route.0.get()
    }
}
