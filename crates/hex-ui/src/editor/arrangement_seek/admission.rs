//! Admission helpers for fresh, history-preserving and quantized Arrangement seeks.
//!
//! Preroll runs on the session service worker. This module only validates the
//! exact source and captures a control-side runtime fence; it never prepares a
//! graph or touches the audio callback.

use hex_audio::prepared::{
    Client, Control, FreshRuntimeCancellationResult, FreshRuntimeFence, SESSION_ROUTE_CAPACITY,
    SessionRouteEntry, SessionRouteOwner,
};
use hex_engine::{PreparedRevision, SessionCapabilityStatus};
use hex_project::Project;
use hex_session::arrangement_seek::{
    ArrangementSeekMode, ArrangementSeekResult, ArrangementSeekSpec,
};
use hex_session::service::DevicePreviewIdentity;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Acquire},
};

// Scalar-only test witness at the real Service handback boundary. This does
// not retain a graph, replace the worker or assert a native destruction time.
#[cfg(test)]
std::thread_local! {
    pub(crate) static LAST_SEEK_RETIREMENT: std::cell::Cell<Option<(u64, usize, bool)>> = const { std::cell::Cell::new(None) };
}

pub(crate) trait SeekAudioControl {
    fn status_view(&self) -> Option<&hex_audio::prepared::Status>;
    fn capture_fence(&self) -> Option<FreshRuntimeFence>;
    fn cancel_publication(&self, revision: u64) -> FreshRuntimeCancellationResult;
}

impl SeekAudioControl for Control {
    fn status_view(&self) -> Option<&hex_audio::prepared::Status> {
        Some(&self.status)
    }
    fn capture_fence(&self) -> Option<FreshRuntimeFence> {
        self.capture_fresh_runtime_fence()
    }
    fn cancel_publication(&self, revision: u64) -> FreshRuntimeCancellationResult {
        self.cancel_fresh_runtime_publication(revision)
    }
}

impl SeekAudioControl for Client {
    fn status_view(&self) -> Option<&hex_audio::prepared::Status> {
        self.status()
    }
    fn capture_fence(&self) -> Option<FreshRuntimeFence> {
        self.capture_fresh_runtime_fence()
    }
    fn cancel_publication(&self, revision: u64) -> FreshRuntimeCancellationResult {
        self.cancel_fresh_runtime_publication(revision)
    }
}

/// One start law retained through intent, deferral and exact acknowledgement.
/// History and section launches cannot be combined.
#[derive(Clone, Copy)]
pub(crate) enum SeekKind {
    Fresh,
    CommittedLocator {
        source: hex_audio::prepared::AppliedAudioSource,
        quantization: Option<hex_engine::LaunchQuantization>,
    },
    History,
    Section(hex_session::arrangement_seek::SectionLaunch),
    Locator(hex_engine::LaunchQuantization),
}

impl SeekKind {
    pub(crate) fn section(self) -> Option<hex_session::arrangement_seek::SectionLaunch> {
        match self {
            Self::Section(section) => Some(section),
            _ => None,
        }
    }
    pub(crate) fn quantization(self) -> Option<hex_engine::LaunchQuantization> {
        match self {
            Self::Section(section) => Some(section.quantization),
            Self::Locator(quantization) => Some(quantization),
            Self::CommittedLocator { quantization, .. } => quantization,
            _ => None,
        }
    }

    pub(crate) fn preparation_status(self) -> &'static str {
        match self {
            Self::Section(_) => "preparing section launch",
            Self::Locator(_)
            | Self::CommittedLocator {
                quantization: Some(_),
                ..
            } => "preparing locator launch",
            _ => "preparing arrangement seek",
        }
    }

    pub(crate) fn mode(self) -> ArrangementSeekMode {
        match self {
            Self::Fresh | Self::CommittedLocator { .. } | Self::Locator(_) => {
                ArrangementSeekMode::Interactive
            }
            Self::History => ArrangementSeekMode::HistoryPreservingInteractive,
            Self::Section(section) => section.mode(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SeekIntent {
    pub note_chase: hex_engine::NoteChase,
    pub kind: SeekKind,
    pub recovery_generation: Option<u64>,
    pub destination_frame: u64,
    pub start_playing: bool,
    pub project_generation: u64,
    pub project_revision: u64,
    pub prepared_revision: Option<Arc<PreparedRevision>>,
    pub device_preview_identity: Option<DevicePreviewIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SeekStage {
    Preparing,
    CandidateReady,
    Published { audio_revision: u64 },
}

pub(crate) struct PendingSeek {
    pub note_chase: hex_engine::NoteChase,
    pub kind: SeekKind,
    pub native_output_identity: Option<u64>,
    pub recovery_generation: Option<u64>,
    pub request_id: u64,
    pub project_generation: u64,
    pub project_revision: u64,
    pub prepared_revision: Option<Arc<PreparedRevision>>,
    pub device_preview_identity: Option<DevicePreviewIdentity>,
    pub destination_frame: u64,
    pub work_budget_frames: u64,
    pub start_playing: bool,
    pub fence: FreshRuntimeFence,
    pub stage: SeekStage,
    pub cancelled: bool,
    pub cancellation_retry: bool,
    pub worker_cancel: Arc<AtomicBool>,
}

impl PendingSeek {
    pub(crate) fn intent(&self) -> SeekIntent {
        SeekIntent {
            note_chase: self.note_chase,
            kind: self.kind,
            recovery_generation: self.recovery_generation,
            destination_frame: self.destination_frame,
            start_playing: self.start_playing,
            project_generation: self.project_generation,
            project_revision: self.project_revision,
            prepared_revision: self.prepared_revision.clone(),
            device_preview_identity: self.device_preview_identity,
        }
    }

    pub fn matches_result(&self, result: &ArrangementSeekResult) -> bool {
        result.request_id == self.request_id
            && result.mode == self.kind.mode()
            && result.generation == self.project_generation
            && result.revision == self.project_revision
            && self
                .prepared_revision
                .as_ref()
                .is_some_and(|prepared| Arc::ptr_eq(&result.prepared_revision, prepared))
            && result.destination_frame == self.destination_frame
            && result.work_budget_frames == self.work_budget_frames
            && result.start_playing == self.start_playing
            && result.note_chase == self.note_chase
    }

    pub fn defers_replacement(&self) -> bool {
        self.prepared_revision.is_none() || self.stage != SeekStage::Preparing
    }
}

pub(crate) fn latch_latest_intent(slot: &mut Option<SeekIntent>, intent: SeekIntent) {
    *slot = Some(intent);
}

pub(crate) fn seek_source_matches(
    candidate: &super::super::PlaybackCandidate,
    prepared: &Option<Arc<PreparedRevision>>,
) -> bool {
    match (candidate.prepared_revision.as_ref(), prepared.as_ref()) {
        (Some(actual), Some(expected)) => Arc::ptr_eq(actual, expected),
        (None, None) => candidate.worker_requires_preroll.is_some(),
        _ => false,
    }
}

impl SeekIntent {
    fn work_budget_frames(&self) -> u64 {
        if matches!(self.kind, SeekKind::History) {
            hex_session::arrangement_seek::MAX_SEEK_WORK_FRAMES
        } else {
            self.destination_frame
        }
    }

    pub fn spec(&self) -> Option<ArrangementSeekSpec> {
        let prepared = self.prepared_revision.as_ref()?;
        // Only explicit history replay spends the ordinary work ceiling.
        // Fresh/section laws retain their existing destination semantics.
        Some(ArrangementSeekSpec {
            note_chase: self.note_chase,
            mode: self.kind.mode(),
            generation: self.project_generation,
            revision: self.project_revision,
            prepared_revision: Arc::clone(prepared),
            destination_frame: self.destination_frame,
            work_budget_frames: self.work_budget_frames(),
            start_playing: self.start_playing,
        })
    }
}

impl PendingSeek {
    pub fn from_intent(request_id: u64, intent: &SeekIntent, fence: FreshRuntimeFence) -> Self {
        Self {
            note_chase: intent.note_chase,
            kind: intent.kind,
            request_id,
            native_output_identity: None,
            recovery_generation: intent.recovery_generation,
            project_generation: intent.project_generation,
            project_revision: intent.project_revision,
            prepared_revision: intent.prepared_revision.clone(),
            device_preview_identity: intent.device_preview_identity,
            destination_frame: intent.destination_frame,
            work_budget_frames: intent.work_budget_frames(),
            start_playing: intent.start_playing,
            fence,
            stage: SeekStage::Preparing,
            cancelled: false,
            cancellation_retry: false,
            worker_cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Capture a fence only when the current route snapshot is the expected audio
/// graph and every project track is currently owned by Arrangement.
///
/// The second coherent read inside `capture_fresh_runtime_fence` must match the
/// first header. A route/command change after this point is rejected by the
/// callback's conditional publication policy.
pub(crate) fn capture_arrangement_fence<C: SeekAudioControl>(
    control: &C,
    project: &Project,
    expected_audio_revision: u64,
) -> Option<FreshRuntimeFence> {
    let status = control.status_view()?;
    if !audio_ready_for_seek(control) {
        return None;
    }
    let mut entries = [SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
    let snapshot = status.session_routes(&mut entries)?;
    if snapshot.audio_revision != expected_audio_revision
        || snapshot.count != project.tracks().len()
        || snapshot.count > entries.len()
        || snapshot.capability != SessionCapabilityStatus::Available
        || !routes_are_current(&entries[..snapshot.count], project)
    {
        return None;
    }

    let fence = control.capture_fence()?;
    (fence.expected_active_revision() == snapshot.audio_revision
        && fence.route_state_generation() == snapshot.route_state_generation)
        .then_some(fence)
}

pub(crate) fn audio_ready_for_seek<C: SeekAudioControl>(control: &C) -> bool {
    control.status_view().is_some_and(|status| {
        !status.device_fault.load(Acquire)
            && !status.render_fault.load(Acquire)
            && !status.shutdown_complete.load(Acquire)
    })
}

pub(crate) fn cancel_published_seek<C: SeekAudioControl>(
    control: Option<&C>,
    pending: &mut PendingSeek,
) -> Option<FreshRuntimeCancellationResult> {
    let SeekStage::Published { audio_revision } = pending.stage else {
        return None;
    };
    pending.cancelled = true;
    control.map(|control| control.cancel_publication(audio_revision))
}

pub(crate) fn routes_are_current(entries: &[SessionRouteEntry], project: &Project) -> bool {
    if entries.len() != project.tracks().len() {
        return false;
    }
    entries.iter().all(|entry| {
        entry.owner == SessionRouteOwner::Arrangement
            && entry.pending.is_none()
            && project
                .tracks()
                .iter()
                .any(|track| track.id().value() == entry.track_id)
    }) && entries.iter().enumerate().all(|(index, entry)| {
        entries[..index]
            .iter()
            .all(|previous| previous.track_id != entry.track_id)
    })
}

/// A prepared revision is required to decide safely between the analytic
/// direct path and history-preserving stateful preroll.
pub(crate) fn requires_preroll(prepared: Option<&Arc<PreparedRevision>>) -> Option<bool> {
    prepared.map(|prepared| prepared.arrangement_seek_requires_preroll())
}

pub(crate) fn seek_requires_preroll_for_mode(
    mode: hex_session::DocumentAudioMode,
    prepared: Option<&Arc<PreparedRevision>>,
    verified_candidate_is_applied: bool,
) -> Option<bool> {
    match mode {
        hex_session::DocumentAudioMode::Resident => requires_preroll(prepared),
        hex_session::DocumentAudioMode::VerifiedStreaming => {
            verified_candidate_is_applied.then_some(false)
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct VerifiedSeekCandidateContext {
    pub(crate) client_mode: hex_session::DocumentAudioMode,
    pub(crate) output_identity: Option<u64>,
    pub(crate) candidate_audio_revision: u64,
    pub(crate) applied_audio_revision: u64,
    pub(crate) candidate_generation: u64,
    pub(crate) current_generation: u64,
    pub(crate) candidate_project_revision: u64,
    pub(crate) current_project_revision: u64,
    pub(crate) applied_project_revision: Option<u64>,
    pub(crate) has_prepared_revision: bool,
    pub(crate) is_canonical_candidate: bool,
}

pub(crate) fn verified_audio_candidate_is_current(context: VerifiedSeekCandidateContext) -> bool {
    context.client_mode == hex_session::DocumentAudioMode::VerifiedStreaming
        && context.output_identity.is_some()
        && !context.has_prepared_revision
        && context.is_canonical_candidate
        && context.candidate_audio_revision == context.applied_audio_revision
        && context.candidate_generation == context.current_generation
        && context.candidate_project_revision == context.current_project_revision
        && context.applied_project_revision == Some(context.current_project_revision)
}

pub(crate) fn resolve_prepared_seek_terminal<E: From<String> + std::ops::Deref<Target = str>>(
    pending_seek: &mut Option<PendingSeek>,
    status: &mut String,
    error: &mut Option<E>,
    applied_high_water: u64,
    rejected_revision: Option<(u64, hex_engine::playback::PlaybackHandoffRejection)>,
) -> bool {
    let Some(pending) = pending_seek.as_ref() else {
        return false;
    };
    let SeekStage::Published { audio_revision } = pending.stage else {
        return false;
    };
    let rejected = rejected_revision.is_some_and(|(revision, _)| revision == audio_revision);
    if !rejected && applied_high_water < audio_revision {
        return false;
    }
    let cancelled = pending.cancelled;
    let kind = pending.kind;
    *pending_seek = None;
    if rejected && !cancelled {
        let reason = rejected_revision
            .map(|(_, reason)| reason.message())
            .unwrap_or("audio rejected the seek");
        *error = Some(
            format!("arrangement seek rejected: {reason}")
                .to_lowercase()
                .into(),
        );
    } else if !cancelled {
        *status = match kind {
            SeekKind::Section(_) => "section launch applied",
            SeekKind::Locator(_)
            | SeekKind::CommittedLocator {
                quantization: Some(_),
                ..
            } => "locator launch applied",
            _ => "cursor ready",
        }
        .into();
        crate::editor::user_errors::clear_stale_playback_unavailable_error(error);
    }
    true
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[path = "../arrangement_seek_tests.rs"]
mod tests;
