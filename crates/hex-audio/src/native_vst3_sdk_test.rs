//! Opt-in native macOS arm64 VST3 SDK integration test target.
//!
//! Relocated out of `src/lib.rs` so default `--lib` and `allocation-probe`
//! builds do not compile or link the ~230 ignored native SDK tests under
//! `tests/native_vst3_sdk/`. Run explicitly with:
//! `cargo test -p hex-audio --features vst3-sdk-tests --test native_vst3_sdk --locked -- --ignored <filter>`
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#![allow(dead_code, unused_imports)]

macro_rules! include_vst3_sdk_tests {
    ($($item:item)*) => {
        $($item)*
    };
}
pub(crate) use include_vst3_sdk_tests;

pub mod audio_diagnostics;
pub mod cpu_meter;
mod device;
pub mod input_capture;
pub mod input_devices;
mod open_outcome;
mod output_route;
pub mod plugin_effects;
pub mod plugin_worker;
mod render_fault;
pub mod sample_preview;
pub mod sample_recording;
pub use device::PluginOutputObservation;
pub use device::{PreparedOutput, WorkerOutputProfile, WorkerPreparedOutput};
pub use lifecycle::{
    AudioLifecycleFailure, RecoveryOpen, RecoveryOpenError, WorkerRecoveryOpen,
    audio_lifecycle_failure, request_initial_open, request_recovery_open, request_worker_open,
    request_worker_open_with_profile, request_worker_open_with_reliable_action_outcomes,
};
pub use open_outcome::{OutputOpenError, RateMismatch};
pub use output_route::{OutputRoute, OutputRouteWatch, ROUTE_SAMPLE_INTERVAL};
pub use render_fault::RenderFaultSnapshot;
mod candidate_catalog;
mod candidate_custody;
#[cfg(all(test, feature = "allocation-probe", feature = "stream-test-support"))]
pub(crate) mod candidate_window_probe {
    use std::cell::Cell;
    thread_local! {
        static WINDOWS: Cell<Option<(usize, usize, usize)>> = const { Cell::new(None) };
        static DOMAINS: Cell<Option<(u8, usize)>> = const { Cell::new(None) };
    }
    pub(crate) fn begin() {
        WINDOWS.with(|value| value.set(Some((0, 0, usize::MAX))));
        DOMAINS.with(|value| value.set(Some((0, usize::MAX))));
    }
    pub(crate) fn attempt(frames: usize) {
        WINDOWS.with(|value| {
            if let Some((failed, next, smallest)) = value.get()
                && failed != 0
            {
                value.set(Some((
                    failed,
                    if next == 0 { frames } else { next },
                    smallest.min(frames),
                )));
            }
        });
    }
    pub(crate) fn accepted(slot: usize) {
        DOMAINS.with(|value| {
            if let Some((mask, usize::MAX)) = value.get() {
                value.set(Some((mask | (1 << slot), usize::MAX)));
            }
        });
    }
    pub(crate) fn capacity(frames: usize, slot: usize) {
        WINDOWS.with(|value| {
            if let Some((0, next, smallest)) = value.get() {
                value.set(Some((frames, next, smallest)));
            }
        });
        DOMAINS.with(|value| {
            if let Some((mask, usize::MAX)) = value.get() {
                value.set(Some((mask, slot)));
            }
        });
    }
    pub(crate) fn domains() -> Option<(u8, usize)> {
        DOMAINS.with(|value| value.get())
    }
    pub(crate) fn finish() -> Option<(usize, usize, usize)> {
        DOMAINS.with(|value| value.set(None));
        WINDOWS.with(|value| value.replace(None))
    }
}
pub const MAX_RETAINED_EDITOR_EDITS: usize = 80;
pub use candidate_custody::{
    NativeSourceDataReservation, NativeSourcePreparationProgress, NativeSourcePreparationReturn,
    RetiringEditLease, RetiringEditProgress, RetiringEditReceipt,
};
pub mod foreign_parameter_delivery;
pub mod gain_recording;
mod lifecycle;
mod live_delivery;
pub mod midi_recording;
mod native_callback_owner;
pub mod native_parameter_input;
pub use candidate_catalog::CandidateDomain;
pub use candidate_custody::{
    AudioCandidateCancellationResult, AudioCandidateCustodyReport,
    AudioCandidateDomainPreparationFailure, AudioCandidateDomainPreparationProgress,
    AudioCandidateDomainRefusal, AudioCandidateDomainReservation, AudioCandidateDomainStartError,
    AudioCandidateDomainToken, AudioCandidateDomainUsage, AudioCandidateEligibleStageRefusal,
    AudioCandidatePreparationProgress, AudioCandidatePreparationRefusal,
    AudioCandidatePreparationReservation, AudioCandidatePreparationToken,
    AudioCandidatePublicationReservation, AudioCandidateRecovery, AudioCandidateReturnKind,
    AudioCandidateStageRefusal, AudioCandidateTicket,
};
pub mod prepared;
pub mod processor_audition;
pub mod spectrum;
pub mod spectrum_target;
pub mod spectrum_worker;
pub mod worker_fixture;
