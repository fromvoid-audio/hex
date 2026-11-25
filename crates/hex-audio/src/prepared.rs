//! Bounded prepared-playback ownership handoff for native output.
//!
//! Control constructs and destroys graphs. Each callback accepts at most one graph and the bounded
//! command prefix; retirement capacity is checked before ownership moves. A full queue retains
//! owners in the existing held slots, preserving current audio and priority Stop. Stop's
//! independent ticket cutoff discards older commands while retaining later admitted FIFO commands.
//! InheritRuntime preserves compatible transport/route/history. Fresh policies use caller-prepared
//! runtime state; a conditional fresh fence additionally matches the actual predecessor, route
//! generation and admitted-command prefix at the final swap. Canonical editor revision and applied
//! audio revision remain distinct until that acceptance. Queue acceptance, source readiness and
//! physical disposal are separate facts; callback code must never finally destroy graphs, PCM or
//! endpoints.
//! Resident Clients stamp ordinary queued actions with the bound project generation. Compare it
//! with the actual active graph before an action changes that graph. Same-document revisions
//! share the binding. The existing output-wide priority Stop retains its independent cutoff.
//!
//! Ordinary computer keyboard inputs retain only this output's scalar activation clock. Control
//! Stop revokes its reserved prefix before publishing the priority marker; callback consumption
//! preserves only an input bound to that exact clock, allowing newer physical epochs without a
//! graph replacement. Queued explicit Stop, foreign/unbound inputs and lifecycle shutdown remain
//! terminal. Epoch transitions and note snapshots perform no callback allocation or reclamation.
use crate::{
    audio_diagnostics::{
        self, AudioDiagnosticEvent, AudioDiagnosticKind, AudioDiagnosticsFlags,
        PublicationWaitReason,
    },
    processor_audition, spectrum_target,
    spectrum_worker::{SpectrumCapture, SpectrumStamp},
};
use hex_engine::{
    Frame, Invalid, MAX_SESSION_ROUTE_TRACKS, PlaybackAction, PlaybackActionRejection,
    PlaybackCommand, PlaybackHandoffRejection, PostNodeIdentity, SessionCapabilityStatus,
    playback::Playback,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU32, AtomicU64,
        Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst},
    },
};
use std::time::Instant;

// Queue capacity and per-block consumption budget must stay equal: every
// pre-cut prefix fits in one callback, so no persistent Stop-discard watermark
// is needed when a later post-cut ticket remains queued.
const COMMANDS: usize = 32;

mod action_outcomes;
mod audible_clock;
pub use action_outcomes::{
    ACTION_OUTCOME_CAPACITY, Failure as ActionOutcomeFailure, Outcome as ActionOutcome,
    Reader as ActionOutcomeReader, Receipt as ActionOutcomeReceipt,
};
mod admission;
mod device_insert_cut;
mod eligible_publication;
#[cfg(target_os = "macos")]
mod output_presentation;
#[cfg(target_os = "macos")]
pub(crate) use output_presentation::PluginOutputFence;
mod endpoints;
pub(crate) mod retired_scalar_receipts;
pub use retired_scalar_receipts::{
    RetiredReceiptFailure, RetiredScalarReceiptBatch, RetiredScalarReceipts, ScalarReceipt,
    ScalarReceiptTarget, ScalarReceiptTerminal,
};
mod native_effects;
pub(crate) use native_effects::NativeEffectWindow;
mod midi_recording;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native_sources;
pub use hex_lattice::ProcessorInstanceIdentity as NativeProcessorIdentity;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use native_sources::{NativeParameterContinuationCapture, NativeParameterContinuationRequest};
mod device_insert_consent;
mod eligible_device_insert;
mod publication_receipt;
pub use device_insert_consent::DeviceInsertConsent;
use device_insert_consent::{DeviceInsertAdmission, DeviceInsertBasis};
pub(crate) use publication_receipt::PublicationReceiptOwner;
pub use publication_receipt::{
    CancellablePublicationReceipt, FreshRuntimeCancellationResult, PublicationReceiptState,
};
mod resident_forwarding;
pub use endpoints::{Client, WorkerControl};
pub(crate) use endpoints::{ClientEndpoints, EndpointTerminals};
pub use resident_forwarding::{ResidentPublicationResponse, ResidentPublicationToken};
#[cfg(all(test, feature = "allocation-probe"))]
mod alias_tests;
mod applied_source;
#[cfg(all(test, feature = "allocation-probe"))]
mod delay_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod device_resources_tests;
pub mod input_monitor;
#[cfg(test)]
mod input_monitor_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod keyboard_activation_tests;
#[cfg(test)]
mod latency_edit_timing_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod loop_audio_tests;
#[cfg(test)]
mod project_spectrum_tests;
mod runtime_view;
mod sampler_edit;
mod sampler_keyboard;
mod section_launch;
mod tempo_cut;
pub use tempo_cut::FencedTempoCut;
use tempo_cut::{FencedAdmission, TEMPO_STREAM_WINDOW_FRAMES};
#[cfg(test)]
mod spectrum_multi_tests;
#[cfg(test)]
mod spectrum_profile;
mod spectrum_stages;
#[cfg(all(test, feature = "allocation-probe"))]
mod tempo_tests;
pub use applied_source::AppliedAudioSource;
pub use sampler_edit::{StoppedEditFence, StoppedEditFenceState};
mod ordinary_streaming;
#[cfg(test)]
use spectrum_stages::SpectrumStage;
use spectrum_stages::SpectrumStages;
mod streaming;
mod transport_fade;
mod transport_observation;
pub use transport_observation::TransportObservation;
pub(crate) const RETIRED: usize = 2;
#[cfg(test)]
pub(crate) mod candidate_tests;
#[cfg(all(test, feature = "allocation-probe"))]
mod final_active_tests;

mod callback;
mod control;
mod legacy_advance;
mod policy;
mod route_snapshot;
mod status;

#[cfg(test)]
pub(crate) use callback::channel_with_interface;
pub use callback::{Callback, channel, channel_with_spectrum, channel_with_worker_pool};
pub(crate) use callback::{channel_with_interface_and_buffer, native_endpoint_bank_storage_quote};
pub use control::{
    AdmittedPublishError, Control, PublicationControl, PublishError, PublishErrorKind,
};
pub(crate) use control::{
    OutputTransportDelivery, worker_boundary_storage_quote, worker_publication_queue_storage_quote,
};
pub use policy::{FreshRuntimeFence, PublicationPolicy};
pub(crate) use policy::{Publication, ReturnedPublication};
pub use route_snapshot::{
    SESSION_ROUTE_CAPACITY, SessionPendingRoute, SessionRouteEntry, SessionRouteOwner,
    SessionRouteSnapshot, SessionRouteTarget,
};
pub use status::Status;

use control::capture_runtime_fence;
use policy::{
    ActivePublication, CommandCut, QueuedCommand, QueuedPublication, RenderOutcome,
    action_invalidates_processor_audition, command_cut, enqueue_legacy, fault_cursor_intent,
    is_pause_action, native_epoch_accepts_successor, recovery_cursor_action, stale_output_start,
    starts_playback,
};
use route_snapshot::{AtomicRouteSlot, MAX_TRACKS, ROUTE_READER_ATTEMPTS};

#[cfg(all(test, feature = "allocation-probe"))]
#[path = "prepared/diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
mod tests;
