//! One shared admission word for the two actors extracted from legacy Control.
//! Callback completion clears only its publication bit. No wait or lock exists.
use std::sync::atomic::{AtomicU8, Ordering};
const PUBLICATION: u8 = 1;
const GAIN: u8 = 2;
#[derive(Default)]
pub(super) struct PublicationAdmission(AtomicU8);
impl PublicationAdmission {
    pub(super) fn load(&self, order: Ordering) -> bool {
        self.0.load(order) & PUBLICATION != 0
    }
    pub(super) fn store(&self, value: bool, order: Ordering) {
        if value {
            self.0.fetch_or(PUBLICATION, order);
        } else {
            self.0.fetch_and(!PUBLICATION, order);
        }
    }
    pub(super) fn reserve_publication(&self) -> bool {
        self.0
            .compare_exchange(0, PUBLICATION, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn reserve_gain(&self) -> bool {
        self.0
            .compare_exchange(0, GAIN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn gain_active(&self) -> bool {
        self.0.load(Ordering::Acquire) & GAIN != 0
    }
    pub(super) fn legacy_gain_started(&self) {
        self.0.fetch_or(GAIN, Ordering::Release);
    }
    pub(super) fn gain_finished(&self) {
        self.0.fetch_and(!GAIN, Ordering::Release);
    }
}

impl PublicationAdmission {
    pub(super) fn audit_scalar_storage(&self) {
        let Self(word) = self;
        let _: &AtomicU8 = word;
    }
}
