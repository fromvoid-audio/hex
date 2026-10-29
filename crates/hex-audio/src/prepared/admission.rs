//! One admission word for publication, native epoch, recording and native input actors.
//! Callback completion clears only its publication bit. No wait or lock exists.
use std::sync::atomic::{AtomicU8, Ordering};
const PUBLICATION: u8 = 1;
const GAIN: u8 = 2;
const MIDI: u8 = 4;
const NATIVE_EPOCH: u8 = 8;
const PARAMETER_INPUT: u8 = 16;
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
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn reserve_native_epoch(&self) -> bool {
        self.0
            .compare_exchange(0, NATIVE_EPOCH, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn release_native_epoch(&self) {
        self.0.fetch_and(!NATIVE_EPOCH, Ordering::Release);
    }
    pub(super) fn reserve_native_epoch_publication(&self) -> bool {
        self.0
            .compare_exchange(
                NATIVE_EPOCH,
                NATIVE_EPOCH | PUBLICATION,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
    pub(super) fn reserve_gain(&self) -> bool {
        self.0
            .compare_exchange(0, GAIN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn reserve_legacy_gain(&self) -> bool {
        let current = self.0.load(Ordering::Acquire);
        current & (GAIN | MIDI | NATIVE_EPOCH | PARAMETER_INPUT) == 0
            && self
                .0
                .compare_exchange(current, current | GAIN, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    pub(super) fn recording_active(&self) -> bool {
        self.0.load(Ordering::Acquire) & (GAIN | MIDI | PARAMETER_INPUT) != 0
    }
    pub(super) fn midi_active(&self) -> bool {
        self.0.load(Ordering::Acquire) & MIDI != 0
    }
    pub(super) fn reserve_parameter_input(&self) -> bool {
        self.0
            .compare_exchange(0, PARAMETER_INPUT, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn parameter_input_finished(&self) {
        self.0.fetch_and(!PARAMETER_INPUT, Ordering::Release);
    }
    pub(super) fn reserve_midi(&self) -> bool {
        self.0
            .compare_exchange(0, MIDI, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub(super) fn midi_finished(&self) {
        self.0.fetch_and(!MIDI, Ordering::Release);
    }
    pub(super) fn legacy_gain_started(&self) {
        self.0.fetch_or(GAIN, Ordering::Release);
    }
    pub(super) fn gain_finished(&self) {
        self.0.fetch_and(!GAIN, Ordering::Release);
    }
}
