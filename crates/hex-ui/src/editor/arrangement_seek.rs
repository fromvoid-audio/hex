//! Admission helpers for history-preserving Arrangement seeks.
//!
//! Preroll runs on the session service worker. This module only validates the
//! exact source and captures a control-side runtime fence; it never prepares a
//! graph or touches the audio callback.

use hex_audio::prepared::{
    Control, FreshRuntimeFence, SESSION_ROUTE_CAPACITY, SessionRouteEntry, SessionRouteOwner,
};
use hex_engine::{PreparedRevision, SessionCapabilityStatus};
use hex_project::Project;
use hex_session::arrangement_seek::{
    ArrangementSeekMode, ArrangementSeekResult, ArrangementSeekSpec, ArrangementSeekSubmitError,
    MAX_SEEK_WORK_FRAMES,
};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct SeekIntent {
    pub destination_frame: u64,
    pub start_playing: bool,
    pub project_generation: u64,
    pub project_revision: u64,
    pub prepared_revision: Arc<PreparedRevision>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SeekStage {
    Preparing,
    CandidateReady,
    Published { audio_revision: u64 },
}

pub(super) struct PendingSeek {
    pub request_id: u64,
    pub mode: ArrangementSeekMode,
    pub project_generation: u64,
    pub project_revision: u64,
    pub prepared_revision: Arc<PreparedRevision>,
    pub destination_frame: u64,
    pub work_budget_frames: u64,
    pub start_playing: bool,
    pub fence: FreshRuntimeFence,
    pub stage: SeekStage,
    pub cancelled: bool,
}

impl PendingSeek {
    pub fn matches_result(&self, result: &ArrangementSeekResult) -> bool {
        result.request_id == self.request_id
            && result.mode == self.mode
            && result.generation == self.project_generation
            && result.revision == self.project_revision
            && Arc::ptr_eq(&result.prepared_revision, &self.prepared_revision)
            && result.destination_frame == self.destination_frame
            && result.work_budget_frames == self.work_budget_frames
            && result.start_playing == self.start_playing
            && self.mode == ArrangementSeekMode::Interactive
    }

    pub fn defers_replacement(&self) -> bool {
        self.stage != SeekStage::Preparing
    }
}

fn latch_latest_intent(slot: &mut Option<SeekIntent>, intent: SeekIntent) {
    *slot = Some(intent);
}

impl SeekIntent {
    pub fn spec(&self) -> Option<ArrangementSeekSpec> {
        (self.destination_frame <= MAX_SEEK_WORK_FRAMES).then(|| ArrangementSeekSpec {
            mode: ArrangementSeekMode::Interactive,
            generation: self.project_generation,
            revision: self.project_revision,
            prepared_revision: Arc::clone(&self.prepared_revision),
            destination_frame: self.destination_frame,
            work_budget_frames: self.destination_frame,
            start_playing: self.start_playing,
        })
    }
}

impl PendingSeek {
    pub fn from_intent(request_id: u64, intent: &SeekIntent, fence: FreshRuntimeFence) -> Self {
        Self {
            request_id,
            mode: ArrangementSeekMode::Interactive,
            project_generation: intent.project_generation,
            project_revision: intent.project_revision,
            prepared_revision: Arc::clone(&intent.prepared_revision),
            destination_frame: intent.destination_frame,
            work_budget_frames: intent.destination_frame,
            start_playing: intent.start_playing,
            fence,
            stage: SeekStage::Preparing,
            cancelled: false,
        }
    }
}

/// Capture a fence only when the current route snapshot is the expected audio
/// graph and every project track is currently owned by Arrangement.
///
/// The second coherent read inside `capture_fresh_runtime_fence` must match the
/// first header. A route/command change after this point is rejected by the
/// callback's conditional publication policy.
pub(super) fn capture_arrangement_fence(
    control: &Control,
    project: &Project,
    expected_audio_revision: u64,
) -> Option<FreshRuntimeFence> {
    if !audio_ready_for_seek(control) {
        return None;
    }
    let mut entries = [SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
    let snapshot = control.status.session_routes(&mut entries)?;
    if snapshot.audio_revision != expected_audio_revision
        || snapshot.count != project.tracks().len()
        || snapshot.count > entries.len()
        || snapshot.capability != SessionCapabilityStatus::Available
        || !routes_are_current(&entries[..snapshot.count], project)
    {
        return None;
    }

    let fence = control.capture_fresh_runtime_fence()?;
    (fence.expected_active_revision() == snapshot.audio_revision
        && fence.route_state_generation() == snapshot.route_state_generation)
        .then_some(fence)
}

pub(super) fn audio_ready_for_seek(control: &Control) -> bool {
    !control.status.device_fault.load(Acquire)
        && !control.status.render_fault.load(Acquire)
        && !control.status.shutdown_complete.load(Acquire)
}

pub(super) fn routes_are_current(entries: &[SessionRouteEntry], project: &Project) -> bool {
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
pub(super) fn requires_preroll(prepared: Option<&Arc<PreparedRevision>>) -> Option<bool> {
    prepared.map(|prepared| prepared.arrangement_seek_requires_preroll())
}

#[cfg(test)]
#[path = "arrangement_seek_tests.rs"]
mod tests;

use super::{EditorView, PendingPlayback};
use hex_engine::playback::PlaybackHandoffRejection;
use std::sync::atomic::Ordering::Acquire;

impl EditorView {
    pub(super) fn current_seek_requires_preroll(&self) -> Option<bool> {
        self.audible_playback
            .as_ref()?
            .prepared_revision
            .as_ref()
            .map(|prepared| prepared.arrangement_seek_requires_preroll())
    }

    pub(super) fn current_preroll_source(
        &self,
    ) -> Option<(Arc<hex_engine::PreparedRevision>, u64, u64)> {
        let state = self.state.as_ref()?;
        let candidate = self.audible_playback.as_ref()?;
        let prepared = candidate.prepared_revision.as_ref()?;
        requires_preroll(Some(prepared))?.then(|| {
            (
                Arc::clone(prepared),
                candidate.audio_revision,
                state.revision,
            )
        })
    }

    pub(super) fn request_prepared_seek(&mut self, destination_frame: u64, start_playing: bool) {
        let Some((prepared_revision, audio_revision, project_revision)) =
            self.current_preroll_source()
        else {
            self.error = Some("stateful arrangement seek source is unavailable".into());
            return;
        };
        let intent = super::arrangement_seek::SeekIntent {
            destination_frame,
            start_playing,
            project_generation: self.project_generation,
            project_revision,
            prepared_revision,
        };
        if intent.spec().is_none() {
            self.error = Some("seek exceeds the bounded arrangement preroll range".into());
            return;
        }
        if self.retired_seek_result.is_some() {
            latch_latest_intent(&mut self.latched_seek_intent, intent);
            self.status = "seek queued · reclaiming the previous candidate".into();
            return;
        }
        if let Some(pending) = self.prepared_seek.as_ref()
            && pending.defers_replacement()
        {
            latch_latest_intent(&mut self.latched_seek_intent, intent);
            self.status = "seek queued · waiting for the current graph".into();
            return;
        }
        self.admit_prepared_seek(intent, audio_revision);
    }

    pub(super) fn desired_playing(&self, actual_playing: bool) -> bool {
        desired_playing_with_pending(
            self.play_intent.desired_playing(actual_playing),
            self.prepared_seek
                .as_ref()
                .filter(|seek| !seek.cancelled)
                .map(|seek| seek.start_playing),
            self.latched_seek_intent
                .as_ref()
                .map(|intent| intent.start_playing),
        )
    }

    pub(super) fn play_toggle_action(
        &self,
        actual_playing: bool,
        start_frame: u64,
    ) -> ToggleAction {
        toggle_action(
            self.play_intent.desired_playing(actual_playing),
            self.latched_seek_intent
                .as_ref()
                .map(|intent| intent.start_playing),
            self.prepared_seek
                .as_ref()
                .filter(|seek| !seek.cancelled)
                .map(|seek| seek.start_playing),
            start_frame,
            self.current_seek_requires_preroll()
                .or_else(|| self.audio.is_none().then_some(false)),
        )
    }

    fn admit_prepared_seek(
        &mut self,
        intent: super::arrangement_seek::SeekIntent,
        audio_revision: u64,
    ) {
        let Some(fence) = self.capture_seek_admission_fence(&intent, audio_revision) else {
            self.error =
                Some("arrangement seek requires the current applied arrangement graph".into());
            return;
        };
        let Some(spec) = intent.spec() else {
            self.error = Some("seek exceeds the bounded arrangement preroll range".into());
            return;
        };
        match self.service.request_arrangement_seek(spec) {
            Ok(request_id) => {
                self.prepared_seek = Some(super::arrangement_seek::PendingSeek::from_intent(
                    request_id, &intent, fence,
                ));
                self.status = "preparing arrangement seek".into();
                self.error = None;
            }
            Err(ArrangementSeekSubmitError::RetirementPending) => {
                self.service.invalidate_arrangement_seek();
                self.prepared_seek = None;
                self.latched_seek_intent = Some(intent);
                self.status = "seek queued · reclaiming the previous candidate".into();
            }
            Err(error) => {
                self.error =
                    Some(format!("could not prepare arrangement seek: {error:?}").to_lowercase());
            }
        }
    }

    fn capture_seek_admission_fence(
        &self,
        intent: &super::arrangement_seek::SeekIntent,
        audio_revision: u64,
    ) -> Option<hex_audio::prepared::FreshRuntimeFence> {
        let state = self.state.as_ref()?;
        let audio = self.audio.as_ref()?;
        let audible = self.audible_playback.as_ref()?;
        let applied = audio.control.status.applied_revision.load(Acquire);
        if self.mode != super::EditorMode::Arrangement
            || state.revision != intent.project_revision
            || self.project_generation != intent.project_generation
            || self.applied_project_revision != Some(intent.project_revision)
            || audible.audio_revision != audio_revision
            || audible.audio_revision != applied
            || audible.project_generation != intent.project_generation
            || audible.state.revision != intent.project_revision
            || audible
                .prepared_revision
                .as_ref()
                .is_none_or(|prepared| !Arc::ptr_eq(prepared, &intent.prepared_revision))
            || self.edit_pending()
            || self.opening_project
            || self.close_prompt_open
            || self.recovery.generation().is_some()
            || self.device_gesture_pending()
            || self.device_gesture.snapshot().is_some()
            || !self.pending_audio_commands.is_empty()
            || self.pending_playback.is_some()
            || !self.playback_candidates.is_empty()
        {
            return None;
        }
        super::arrangement_seek::capture_arrangement_fence(
            &audio.control,
            &state.project,
            audio_revision,
        )
    }

    pub(super) fn tick_prepared_arrangement_seek(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.retired_seek_result.take() {
            match self.service.retire_arrangement_seek_result(result) {
                Ok(()) => changed = true,
                Err(result) => self.retired_seek_result = Some(result),
            }
        }
        if self.retired_seek_result.is_some() {
            return changed;
        }
        if self.prepared_seek.is_none()
            && let Some(intent) = self.latched_seek_intent.take()
        {
            if let Some((_, audio_revision, _)) = self.current_preroll_source() {
                self.admit_prepared_seek(intent, audio_revision);
                changed = true;
            } else {
                self.error = Some("queued arrangement seek source is no longer available".into());
                changed = true;
            }
        }
        let Some(pending) = self.prepared_seek.as_ref() else {
            return changed;
        };
        if pending.stage != super::arrangement_seek::SeekStage::Preparing {
            return changed;
        }
        let Some(result) = self.service.try_arrangement_seek_result() else {
            return changed;
        };
        let matches = self
            .prepared_seek
            .as_ref()
            .is_some_and(|pending| pending.matches_result(&result));
        let is_current = self
            .prepared_seek
            .as_ref()
            .is_some_and(|pending| self.seek_candidate_is_current(pending, false));
        if !matches || !is_current {
            self.prepared_seek = None;
            self.service.invalidate_arrangement_seek();
            self.retire_seek_result(result);
            return true;
        }
        let Some(state) = self.state.as_ref().map(Arc::clone) else {
            self.prepared_seek = None;
            self.retire_seek_result(result);
            return true;
        };
        let ArrangementSeekResult {
            request_id,
            mode,
            generation,
            revision,
            prepared_revision,
            destination_frame,
            work_budget_frames,
            start_playing,
            result,
        } = result;
        match result {
            Ok(playback) => {
                let Some(pending) = self.prepared_seek.as_mut() else {
                    self.retire_seek_result(ArrangementSeekResult {
                        request_id,
                        mode,
                        generation,
                        revision,
                        prepared_revision,
                        destination_frame,
                        work_budget_frames,
                        start_playing,
                        result: Ok(playback),
                    });
                    return true;
                };
                pending.stage = super::arrangement_seek::SeekStage::CandidateReady;
                self.pending_playback = Some(PendingPlayback {
                    project_revision: pending.project_revision,
                    project_generation: pending.project_generation,
                    state,
                    playback,
                    install_fresh_runtime: false,
                    fresh_runtime_fence: Some(pending.fence),
                    prepared_revision: Some(prepared_revision),
                    seek_request_id: Some(request_id),
                    device_preview_identity: None,
                });
                self.status = "arrangement seek ready · waiting for audio".into();
                changed = true;
            }
            Err(error) => {
                self.prepared_seek = None;
                self.error = Some(format!("arrangement seek failed: {error:?}").to_lowercase());
                changed = true;
            }
        }
        changed
    }

    fn seek_candidate_is_current(
        &self,
        pending: &super::arrangement_seek::PendingSeek,
        allow_candidate: bool,
    ) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        let Some(audible) = self.audible_playback.as_ref() else {
            return false;
        };
        let active_revision = audio.control.status.applied_revision.load(Acquire);
        let mut entries = [hex_audio::prepared::SessionRouteEntry::default();
            hex_audio::prepared::SESSION_ROUTE_CAPACITY];
        let Some(routes) = audio.control.status.session_routes(&mut entries) else {
            return false;
        };
        audio_ready_for_seek(&audio.control)
            && state.revision == pending.project_revision
            && self.project_generation == pending.project_generation
            && self.applied_project_revision == Some(pending.project_revision)
            && active_revision == pending.fence.expected_active_revision()
            && audible.audio_revision == active_revision
            && audible.project_generation == pending.project_generation
            && audible.state.revision == pending.project_revision
            && audible
                .prepared_revision
                .as_ref()
                .is_some_and(|prepared| Arc::ptr_eq(prepared, &pending.prepared_revision))
            && routes.audio_revision == pending.fence.expected_active_revision()
            && routes.route_state_generation == pending.fence.route_state_generation()
            && routes.count <= entries.len()
            && routes.count == state.project.tracks().len()
            && routes.capability == hex_engine::SessionCapabilityStatus::Available
            && super::arrangement_seek::routes_are_current(&entries[..routes.count], &state.project)
            && !self.edit_pending()
            && !self.opening_project
            && !self.close_prompt_open
            && self.recovery.generation().is_none()
            && !self.device_gesture_pending()
            && self.device_gesture.snapshot().is_none()
            && (allow_candidate
                || (self.pending_playback.is_none() && self.playback_candidates.is_empty()))
    }

    fn retire_seek_result(&mut self, result: ArrangementSeekResult) {
        if let Err(result) = self.service.retire_arrangement_seek_result(result) {
            self.retired_seek_result = Some(result);
        }
    }

    pub(super) fn invalidate_prepared_seek(&mut self) {
        self.latched_seek_intent = None;
        let Some(mut pending) = self.prepared_seek.take() else {
            self.service.invalidate_arrangement_seek();
            return;
        };
        match pending.stage {
            super::arrangement_seek::SeekStage::Preparing => {
                self.service.invalidate_arrangement_seek();
            }
            super::arrangement_seek::SeekStage::CandidateReady => {
                if self
                    .pending_playback
                    .as_ref()
                    .is_some_and(|playback| playback.seek_request_id == Some(pending.request_id))
                    && let Some(playback) = self.pending_playback.take()
                {
                    self.retire_seek_result(ArrangementSeekResult {
                        request_id: pending.request_id,
                        mode: pending.mode,
                        generation: pending.project_generation,
                        revision: pending.project_revision,
                        prepared_revision: pending.prepared_revision,
                        destination_frame: pending.destination_frame,
                        work_budget_frames: pending.work_budget_frames,
                        start_playing: pending.start_playing,
                        result: Ok(playback.playback),
                    });
                }
            }
            super::arrangement_seek::SeekStage::Published { .. } => {
                pending.cancelled = true;
                self.prepared_seek = Some(pending);
            }
        }
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        // CandidateReady still lives in PendingPlayback, so convert it into
        // the same bounded result owner before the Service begins shutdown.
        self.invalidate_prepared_seek();
        let Some(result) = self.retired_seek_result.take() else {
            return;
        };
        if let Err(result) = self
            .service
            .retire_arrangement_seek_result_for_shutdown(result)
        {
            // The normal invariant leaves at least one worker handback slot
            // empty: UI consumes `ready` before retaining this result and does
            // not admit another seek while it is held. Keep ownership in the
            // field if that invariant is ever violated. This defensive
            // invariant failure cannot be reclaimed safely
            // without blocking or adding another owner slot. Rust will drop
            // the field after this method, so do not claim worker-side
            // destruction in that unreachable-by-contract case.
            self.retired_seek_result = Some(result);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToggleAction {
    Pause,
    PlayFrom(u64),
    PreparedPlayFrom(u64),
    Unavailable,
}

fn toggle_action(
    desired_playing: bool,
    pending_latched_play: Option<bool>,
    pending_prepared_play: Option<bool>,
    start_frame: u64,
    requires_preroll: Option<bool>,
) -> ToggleAction {
    if desired_playing_with_pending(desired_playing, pending_prepared_play, pending_latched_play) {
        ToggleAction::Pause
    } else {
        match requires_preroll {
            Some(true) => ToggleAction::PreparedPlayFrom(start_frame),
            Some(false) => ToggleAction::PlayFrom(start_frame),
            None => ToggleAction::Unavailable,
        }
    }
}

fn desired_playing_with_pending(
    base: bool,
    prepared_seek: Option<bool>,
    latched_seek: Option<bool>,
) -> bool {
    latched_seek.or(prepared_seek).unwrap_or(base)
}

pub(super) fn resolve_prepared_seek_terminal(
    pending_seek: &mut Option<PendingSeek>,
    status: &mut String,
    error: &mut Option<String>,
    applied_high_water: u64,
    rejected_revision: Option<(u64, PlaybackHandoffRejection)>,
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
    *pending_seek = None;
    if rejected && !cancelled {
        let reason = rejected_revision
            .map(|(_, reason)| reason.message())
            .unwrap_or("audio rejected the seek");
        *error = Some(format!("arrangement seek rejected: {reason}").to_lowercase());
    } else if !cancelled {
        *status = "cursor ready".into();
    }
    true
}
