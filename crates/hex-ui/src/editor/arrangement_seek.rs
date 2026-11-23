//! Admission helpers for fresh, history-preserving and quantized Arrangement seeks.
//!
//! Preroll runs on the session service worker. This module only validates the
//! exact source and captures a control-side runtime fence; it never prepares a
//! graph or touches the audio callback.

mod admission;
#[cfg(test)]
pub(super) use admission::LAST_SEEK_RETIREMENT;
use admission::*;
pub(super) use admission::{
    PendingSeek, SeekIntent, SeekKind, SeekStage, audio_ready_for_seek,
    resolve_prepared_seek_terminal,
};
use hex_audio::prepared::{FreshRuntimeCancellationResult, FreshRuntimeFence};
use hex_session::arrangement_seek::{ArrangementSeekResult, ArrangementSeekSubmitError};
use hex_session::service::DevicePreviewIdentity;
use std::sync::Arc;

#[cfg(all(test, not(feature = "ui-test-support")))]
#[path = "arrangement_seek_unit_tests.rs"]
mod unit_tests;

use super::{EditorView, PendingPlayback};
use std::sync::atomic::Ordering::Acquire;

impl EditorView {
    fn seek_context_is_current(
        &self,
        generation: u64,
        revision: u64,
        recovery: Option<u64>,
    ) -> bool {
        (self.mode == super::EditorMode::Arrangement || recovery.is_some())
            && self.project_generation == generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == revision)
            && !(self.gain_recorder.blocks_edits() || self.midi_recorder.blocks_edits())
            && !self.opening_project
            && !self.close_prompt_open
            && self.recovery.permits_seek(recovery)
            && !self.device_gesture.has_pending_terminal()
            && self.device_gesture_barriers.is_empty()
            && self.tuning.client.snapshot().is_none()
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio_ready_for_seek(&audio.control))
    }

    fn active_seek_preview(&self) -> Option<DevicePreviewIdentity> {
        let state = self.state.as_ref()?;
        let snapshot = self.device_gesture.snapshot()?;
        (!self.device_gesture.has_pending_terminal()
            && snapshot.key.generation == self.project_generation)
            .then_some(DevicePreviewIdentity {
                project_generation: self.project_generation,
                canonical_revision: state.revision,
                key: snapshot.key,
                sequence: snapshot.sequence,
            })
    }

    // A preview seek remains a preview, even after its fresh runtime applies.
    // In particular, None must never admit a tuning/device preview as canonical.
    fn seek_source_is_current(
        &self,
        generation: u64,
        revision: u64,
        preview: Option<DevicePreviewIdentity>,
        recovery: Option<u64>,
    ) -> bool {
        self.seek_context_is_current(generation, revision, recovery)
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && match preview {
                Some(identity) => self.active_seek_preview() == Some(identity),
                None => {
                    self.device_gesture.snapshot().is_none()
                        && !self.device_gesture_pending()
                        && self.audible_playback.as_ref().is_some_and(|audible| {
                            audible.device_preview_identity.is_none()
                                && audible.tuning_preview_identity.is_none()
                                && audible.clip_preview_identity.is_none()
                        })
                }
            }
    }

    pub(super) fn pending_seek_source_is_current(&self, pending: &PendingSeek) -> bool {
        match pending.kind {
            SeekKind::CommittedLocator { source, .. } => {
                // A fast callback may install the candidate before the worker's
                // result reports its audio revision. Keep only the pending scalar
                // correlation in that interval; the callback fence still decides
                // whether the worker's candidate can actually be installed.
                (pending.stage == SeekStage::Preparing
                    && self.committed_locator_seek_context_is_current(source))
                    || self.committed_locator_seek_source_is_current(source)
                    || self.native_seek_applied_source(pending).is_some()
            }
            _ => self.seek_source_is_current(
                pending.project_generation,
                pending.project_revision,
                pending.device_preview_identity,
                pending.recovery_generation,
            ),
        }
    }

    pub(super) fn seek_publication_is_current(
        &self,
        request_id: u64,
        preview: Option<DevicePreviewIdentity>,
    ) -> bool {
        self.prepared_seek.as_ref().is_some_and(|pending| {
            pending.request_id == request_id
                && pending.native_output_identity.is_none_or(|identity| {
                    self.audio.as_ref().and_then(|audio| audio.control.output_identity()) == Some(identity)
                })
                // This predicate is ONLY applied-metadata eligibility, not
                // publication admission. Invalidation can lose the callback
                // CAS, and Complete can precede applied/rejected telemetry.
                // Keep the Published source eligible until actual terminal
                // promotion proves which runtime was installed or returned.
                && matches!(pending.stage, SeekStage::Published { .. })
                && pending.device_preview_identity == preview
                && (self.native_seek_applied_source(pending).is_some()
                    || self.pending_seek_source_is_current(pending))
        }) || (preview.is_none()
            && self.recovery.prepared_request() == Some(request_id)
            && self.state.as_ref().is_some_and(|state| {
                self.seek_source_is_current(
                    self.project_generation,
                    state.revision,
                    None,
                    self.recovery.generation(),
                )
            }))
    }

    /// Applied source correlation survives cancellation of musical intent. Retain only the
    /// existing scalar native slot until its protected metadata arrives; stale previews and
    /// superseded outputs/sources must never keep that slot alive.
    pub(super) fn native_seek_applied_source(&self, pending: &PendingSeek) -> Option<(u64, u64)> {
        let SeekStage::Published { audio_revision } = pending.stage else {
            return None;
        };
        let audio = self.audio.as_ref()?;
        if pending.prepared_revision.is_some()
            || pending.native_output_identity.is_none()
            || pending.native_output_identity != audio.control.output_identity()
            || pending.project_generation != self.project_generation
            || match pending.kind {
                SeekKind::CommittedLocator { source, .. } => self
                    .committed_locator_seek_ack_view(source)
                    .is_none_or(|state| state.revision != pending.project_revision),
                _ => self.state.as_ref()?.revision != pending.project_revision,
            }
            || audio.control.status()?.applied_revision.load(Acquire) != audio_revision
            || pending
                .device_preview_identity
                .is_some_and(|identity| self.active_seek_preview() != Some(identity))
        {
            return None;
        }
        Some((pending.request_id, audio_revision))
    }

    pub(super) fn current_seek_requires_preroll(&self) -> Option<bool> {
        let candidate = self.audible_playback.as_ref()?;
        let (audio, state) = match self.document_audio_mode {
            hex_session::DocumentAudioMode::Resident => {
                return candidate.worker_requires_preroll.or_else(|| {
                    candidate
                        .prepared_revision
                        .as_ref()
                        .map(|prepared| prepared.arrangement_seek_requires_preroll())
                });
            }
            hex_session::DocumentAudioMode::VerifiedStreaming => {
                (self.audio.as_ref()?, self.state.as_ref()?)
            }
        };
        let status = audio.control.status()?;
        let applied_revision = status.applied_revision.load(Acquire);
        let verified_candidate_is_applied =
            verified_audio_candidate_is_current(VerifiedSeekCandidateContext {
                client_mode: audio.mode,
                output_identity: audio.control.output_identity(),
                candidate_audio_revision: candidate.audio_revision,
                applied_audio_revision: applied_revision,
                candidate_generation: candidate.project_generation,
                current_generation: self.project_generation,
                candidate_project_revision: candidate.state.revision,
                current_project_revision: state.revision,
                applied_project_revision: self.applied_project_revision,
                has_prepared_revision: candidate.prepared_revision.is_some(),
                is_canonical_candidate: candidate.seek_request_id.is_none()
                    && candidate.device_preview_identity.is_none()
                    && candidate.tuning_preview_identity.is_none()
                    && candidate.clip_preview_identity.is_none(),
            });
        seek_requires_preroll_for_mode(
            self.document_audio_mode,
            candidate.prepared_revision.as_ref(),
            verified_candidate_is_applied,
        )
    }

    pub(super) fn current_preroll_source(
        &self,
    ) -> Option<(Option<Arc<hex_engine::PreparedRevision>>, u64, u64)> {
        self.current_seek_requires_preroll()?
            .then(|| self.current_prepared_seek_source())
            .flatten()
    }

    fn current_prepared_seek_source(
        &self,
    ) -> Option<(Option<Arc<hex_engine::PreparedRevision>>, u64, u64)> {
        let state = self.state.as_ref()?;
        let candidate = self.audible_playback.as_ref()?;
        Some((
            candidate.prepared_revision.clone(),
            candidate.audio_revision,
            state.revision,
        ))
    }

    pub(super) fn request_prepared_seek(&mut self, destination_frame: u64, start_playing: bool) {
        self.request_prepared_seek_with_kind(destination_frame, start_playing, SeekKind::Fresh);
    }

    /// Start only from the exact applied graph for a complete committed locator view.
    pub(super) fn request_committed_locator_seek(
        &mut self,
        destination_frame: u64,
        source: hex_audio::prepared::AppliedAudioSource,
        quantization: Option<hex_engine::LaunchQuantization>,
    ) {
        let Some(state) = self.committed_locator_seek_view(source) else {
            return;
        };
        if state
            .project
            .arrangement_locators()
            .binary_search_by_key(&destination_frame, |locator| locator.frame())
            .is_err()
        {
            return;
        }
        let intent = SeekIntent {
            note_chase: hex_engine::NoteChase::On,
            kind: SeekKind::CommittedLocator {
                source,
                quantization,
            },
            recovery_generation: None,
            destination_frame,
            start_playing: true,
            project_generation: self.project_generation,
            project_revision: state.revision,
            prepared_revision: None,
            device_preview_identity: None,
        };
        self.wake_poll();
        self.latency_rebuild = None;
        self.queue_normal_start(super::playback_settings::Start::Prepared {
            intent,
            audio_revision: source.audio_revision,
        });
    }

    pub(super) fn request_history_playback(&mut self, frame: u64) {
        self.playback_settings.pending = None;
        self.request_prepared_seek_with_kind(frame, true, SeekKind::History);
    }

    /// The section/loop owner already captured the gesture's policy before its loop edit.
    /// Keep the existing adjacent-refused-loop admission proof; do not re-gate this receipt.
    pub(super) fn request_section_launch_with_note_chase(
        &mut self,
        section: hex_session::arrangement_seek::SectionLaunch,
        note_chase: hex_engine::NoteChase,
    ) {
        if !section.matches_loop(
            self.state
                .as_ref()
                .and_then(|state| state.project.loop_region()),
        ) {
            self.error = Some("section launch requires its arrangement loop state".into());
            return;
        }
        let Some((prepared_revision, audio_revision, project_revision)) =
            self.current_prepared_seek_source()
        else {
            self.error = Some("section playback source is unavailable".into());
            return;
        };
        self.wake_poll();
        self.submit_prepared_seek_intent(
            SeekIntent {
                note_chase,
                kind: SeekKind::Section(section),
                recovery_generation: None,
                destination_frame: section.region.start(),
                start_playing: true,
                project_generation: self.project_generation,
                project_revision,
                prepared_revision,
                device_preview_identity: self.active_seek_preview(),
            },
            audio_revision,
        );
    }

    pub(super) fn request_quantized_locator_launch(
        &mut self,
        frame: u64,
        quantization: hex_engine::LaunchQuantization,
    ) {
        if self.section_install_pending() {
            self.error = Some("applying edit · wait for audio".into());
            return;
        }
        self.cancel_section_launch_wait();
        self.request_prepared_seek_with_kind(frame, true, SeekKind::Locator(quantization));
    }

    fn request_prepared_seek_with_kind(
        &mut self,
        destination_frame: u64,
        start_playing: bool,
        kind: SeekKind,
    ) {
        self.wake_poll();
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error = Some(
                match kind {
                    SeekKind::Section(_) => {
                        "section playback is unavailable in verified streaming mode"
                    }
                    SeekKind::Locator(_) => {
                        "locator launch quantization is unavailable in verified streaming mode"
                    }
                    _ => "stateful arrangement seek is unavailable for verified streaming audio",
                }
                .into(),
            );
            return;
        }
        let Some((prepared_revision, audio_revision, project_revision)) =
            (if !matches!(kind, SeekKind::Fresh) {
                self.current_prepared_seek_source()
            } else {
                self.current_preroll_source()
            })
        else {
            self.error = Some(
                match kind {
                    SeekKind::Section(_) => "section playback source is unavailable",
                    SeekKind::Locator(_) => "locator playback source is unavailable",
                    _ => "stateful arrangement seek source is unavailable",
                }
                .into(),
            );
            return;
        };
        let intent = super::arrangement_seek::SeekIntent {
            note_chase: hex_engine::NoteChase::On,
            kind,
            recovery_generation: None,
            destination_frame,
            start_playing,
            project_generation: self.project_generation,
            project_revision,
            prepared_revision,
            device_preview_identity: self.active_seek_preview(),
        };
        if matches!(kind, SeekKind::Section(_) | SeekKind::Locator(_))
            || (matches!(kind, SeekKind::Fresh) && start_playing)
        {
            self.queue_normal_start(super::playback_settings::Start::Prepared {
                intent,
                audio_revision,
            });
            return;
        }
        self.submit_prepared_seek_intent(intent, audio_revision);
    }

    pub(super) fn submit_prepared_seek_intent(&mut self, intent: SeekIntent, audio_revision: u64) {
        if intent.kind.quantization().is_some() && self.prepared_seek.is_some() {
            self.invalidate_prepared_seek();
        }
        if self.retired_seek_result.is_some() {
            latch_latest_intent(&mut self.latched_seek_intent, intent);
            self.status = "seek queued · reclaiming the previous candidate".into();
            return;
        }
        if let Some(pending) = self.prepared_seek.as_ref()
            && pending.defers_replacement()
        {
            if pending.prepared_revision.is_none() && pending.stage == SeekStage::Preparing {
                self.invalidate_prepared_seek();
            }
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
                .map(|intent| intent.start_playing)
                .or_else(|| self.section_launch_waiting().then_some(true)),
        )
    }

    /// Reuse scalar preparation for a recovery-owned stopped cursor, retaining all source,
    /// output, route and callback freshness checks. This is not a user seek barrier exemption.
    pub(super) fn request_recovery_prepared_seek(
        &mut self,
        position: u64,
        generation: u64,
    ) -> Option<u64> {
        if !self.recovery.permits_seek(Some(generation))
            || self.prepared_seek.is_some()
            || self.retired_seek_result.is_some()
            || self.latched_seek_intent.is_some()
            || self.recovery.seek_needed(true) != Some(position)
        {
            return None;
        }
        let (prepared_revision, audio_revision, project_revision) =
            self.current_preroll_source()?;
        let intent = SeekIntent {
            note_chase: hex_engine::NoteChase::On,
            kind: SeekKind::Fresh,
            recovery_generation: Some(generation),
            destination_frame: position,
            start_playing: false,
            project_generation: self.project_generation,
            project_revision,
            prepared_revision,
            device_preview_identity: None,
        };
        self.admit_prepared_seek(intent, audio_revision);
        self.prepared_seek
            .as_ref()
            .filter(|seek| seek.recovery_generation == Some(generation))
            .map(|seek| seek.request_id)
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
                .map(|intent| intent.start_playing)
                .or_else(|| self.section_launch_waiting().then_some(true)),
            self.prepared_seek
                .as_ref()
                .filter(|seek| !seek.cancelled)
                .map(|seek| seek.start_playing),
            start_frame,
            self.current_seek_requires_preroll()
                .or_else(|| self.audio.is_none().then_some(false)),
        )
    }

    pub(super) fn admit_prepared_seek(
        &mut self,
        mut intent: super::arrangement_seek::SeekIntent,
        mut audio_revision: u64,
    ) {
        if let SeekKind::CommittedLocator { source, .. } = intent.kind {
            let fence = self
                .committed_locator_seek_source_is_current(source)
                .then(|| {
                    let state = self.committed_locator_seek_view(source)?;
                    let audio = self.audio.as_ref()?;
                    super::arrangement_seek::capture_arrangement_fence(
                        &audio.control,
                        &state.project,
                        source.audio_revision,
                    )
                })
                .flatten();
            // A Stop may have landed while the fence was captured. The
            // callback separately checks later commands and priority Stop.
            if let Some(fence) =
                fence.filter(|_| self.committed_locator_seek_source_is_current(source))
            {
                self.admit_worker_owned_seek(intent, fence);
            } else {
                self.error = Some("arrangement seek source is no longer available".into());
            }
            return;
        }
        if !self.seek_context_is_current(
            intent.project_generation,
            intent.project_revision,
            intent.recovery_generation,
        ) {
            self.error = Some("arrangement seek source is no longer available".into());
            return;
        }
        if let Some(previous) = intent.device_preview_identity {
            let Some(current) = self.active_seek_preview().filter(|current| {
                current.key == previous.key
                    && current.project_generation == intent.project_generation
                    && current.canonical_revision == intent.project_revision
            }) else {
                return;
            };
            intent.device_preview_identity = Some(current);
            if let Some(audible) = self.audible_playback.as_ref().filter(|audible| {
                audible.device_preview_identity == Some(current)
                    && audible.tuning_preview_identity.is_none()
                    && audible.clip_preview_identity.is_none()
            }) {
                intent.prepared_revision = audible.prepared_revision.clone();
                audio_revision = audible.audio_revision;
            }
            if self
                .capture_seek_admission_fence(&intent, audio_revision)
                .is_none()
            {
                // Keep only the user's latest intent, not a runtime owner. A
                // newer sequence can settle without requiring mouse release.
                latch_latest_intent(&mut self.latched_seek_intent, intent);
                self.status = "seek queued · waiting for the current device preview".into();
                return;
            }
        }
        let Some(fence) = self.capture_seek_admission_fence(&intent, audio_revision) else {
            self.error =
                Some("arrangement seek requires the current applied arrangement graph".into());
            return;
        };
        if intent.prepared_revision.is_none() {
            self.admit_worker_owned_seek(intent, fence);
            return;
        }
        let Some(spec) = intent.spec() else {
            self.error = Some("arrangement seek requires a prepared graph".into());
            return;
        };
        match self.service.request_arrangement_seek(spec) {
            Ok(request_id) => {
                self.prepared_seek = Some(super::arrangement_seek::PendingSeek::from_intent(
                    request_id, &intent, fence,
                ));
                self.status = intent.kind.preparation_status().into();
                self.error = None;
            }
            Err(ArrangementSeekSubmitError::RetirementPending) => {
                self.service.invalidate_arrangement_seek();
                self.prepared_seek = None;
                self.latched_seek_intent = Some(intent);
                self.status = "seek queued · reclaiming the previous candidate".into();
            }
            Err(error) => {
                self.error = Some(
                    format!(
                        "could not prepare arrangement seek · {}",
                        super::user_errors::seek_submit(&error)
                    )
                    .into(),
                );
            }
        }
    }

    fn admit_worker_owned_seek(&mut self, intent: SeekIntent, fence: FreshRuntimeFence) {
        let Some(request_id) = self.seek_request_serial.checked_add(1) else {
            self.error = Some("arrangement seek request identity exhausted".into());
            return;
        };
        let Some(output_identity) = self
            .audio
            .as_ref()
            .and_then(|audio| audio.control.output_identity())
        else {
            self.error = Some("arrangement seek output is unavailable".into());
            return;
        };
        let mut pending = PendingSeek::from_intent(request_id, &intent, fence);
        pending.native_output_identity = Some(output_identity);
        let request = hex_session::service::ResidentArrangementSeekIntent {
            note_chase: intent.note_chase,
            history: if matches!(intent.kind, SeekKind::History) {
                hex_session::service::ResidentSeekHistory::Preserve
            } else {
                hex_session::service::ResidentSeekHistory::Fresh
            },
            request_id,
            project_generation: intent.project_generation,
            project_revision: intent.project_revision,
            destination_frame: intent.destination_frame,
            start_playing: intent.start_playing,
            section: intent.kind.section(),
            locator_quantization: match intent.kind {
                SeekKind::Locator(quantization) => Some(quantization),
                SeekKind::CommittedLocator { quantization, .. } => quantization,
                _ => None,
            },
            fence: hex_session::service::ResidentFreshRuntimeFence {
                output_identity,
                active_audio_revision: fence.expected_active_revision(),
                route_generation: fence.route_state_generation(),
                admitted_command_ticket: fence.admitted_command_ticket(),
                output_fault_epoch: fence.output_fault_epoch(),
                consumed_stop_generation: fence.consumed_stop_generation(),
            },
            device_preview: intent.device_preview_identity,
            tuning_preview: None,
            cancelled: Arc::clone(&pending.worker_cancel),
        };
        match self.service.submit_resident_arrangement_seek(request) {
            Ok(()) => {
                self.seek_request_serial = request_id;
                self.prepared_seek = Some(pending);
                self.status = intent.kind.preparation_status().into();
                self.error = None;
            }
            Err(hex_session::service::SubmitError::Full(_)) => {
                self.error = Some("arrangement seek request queue is full".into());
            }
            Err(hex_session::service::SubmitError::Closed(_)) => {
                self.error = Some("arrangement seek worker is unavailable".into());
            }
            Err(hex_session::service::SubmitError::Busy(_, reason)) => {
                self.error = Some(super::user_errors::busy(reason).into());
            }
        }
    }

    fn poll_worker_owned_seek(&mut self) -> bool {
        let Some(result) = self.service.try_resident_arrangement_seek_result() else {
            return false;
        };
        let Some(pending) = self.prepared_seek.as_mut().filter(|pending| {
            pending.prepared_revision.is_none()
                && pending.request_id == result.request_id
                && pending.project_generation == result.project_generation
                && pending.project_revision == result.project_revision
                && pending.note_chase == result.note_chase
        }) else {
            return true;
        };
        let worker_accepted = result.result.is_ok();
        let completion = result.result.and_then(|accepted| match accepted {
            hex_session::service::ResidentPublicationAcceptance::Submitted {
                output_identity,
                audio_revision,
            } if self
                .audio
                .as_ref()
                .and_then(|audio| audio.control.output_identity())
                == Some(output_identity) =>
            {
                Ok(audio_revision)
            }
            _ => Err("arrangement seek output is no longer available".into()),
        });
        match completion {
            Ok(audio_revision) => {
                pending.stage = SeekStage::Published { audio_revision };
                match pending.kind {
                    SeekKind::Section(_) => self.status = "section launch queued".into(),
                    SeekKind::Locator(_)
                    | SeekKind::CommittedLocator {
                        quantization: Some(_),
                        ..
                    } => self.status = "locator launch queued".into(),
                    _ => {}
                }
                self.recovery
                    .prepared_published(pending.request_id, audio_revision);
                if pending.cancelled {
                    let control = self.audio.as_ref().map(|audio| &audio.control);
                    pending.cancellation_retry = cancel_published_seek(control, pending)
                        == Some(FreshRuntimeCancellationResult::Busy);
                }
            }
            Err(error) => {
                let cancelled = pending.cancelled;
                let recovery = pending.recovery_generation;
                let request_id = pending.request_id;
                let superseded = !cancelled
                    && recovery.is_some()
                    && self.project_generation == pending.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.revision == pending.project_revision)
                    && self.audio.as_ref().is_some_and(|audio| {
                        audio.control.output_identity().is_some()
                            && audio.control.status().is_some_and(|status| {
                                !status.device_fault.load(Acquire)
                                    && !status.render_fault.load(Acquire)
                                    && !status.shutdown_complete.load(Acquire)
                                    && (worker_accepted
                                        || audio.control.output_identity()
                                            != pending.native_output_identity
                                        || status.applied_revision.load(Acquire)
                                            > pending.fence.expected_active_revision())
                            })
                    });
                self.prepared_seek = None;
                if let Some(generation) = recovery.filter(|generation| {
                    superseded && self.recovery.permits_seek(Some(*generation))
                }) {
                    if self.recovery.retry_prepared_seek(generation, request_id) {
                        self.status = "restarting audio · updating playback position".into();
                        self.error = None;
                    } else {
                        // A second advancing source still belongs to the healthy
                        // output. End this attempt with an explicit cursor retry.
                        self.clear_recovery();
                        self.pending_recovery_generation = None;
                        self.playback_settings.pending = None;
                        self.deferred_play = None;
                        self.status = "audio ready · playback position needs retry".into();
                        self.error = Some("audio changed again · recovery cancelled · set playback position and retry play".into());
                    }
                } else if !cancelled {
                    if recovery.is_some() && self.recovery.permits_seek(recovery) {
                        self.fail_audio_recovery(error);
                    } else {
                        self.error = Some(error.into());
                    }
                }
            }
        }
        true
    }

    fn capture_seek_admission_fence(
        &self,
        intent: &super::arrangement_seek::SeekIntent,
        audio_revision: u64,
    ) -> Option<hex_audio::prepared::FreshRuntimeFence> {
        let state = self.state.as_ref()?;
        let audio = self.audio.as_ref()?;
        let audible = self.audible_playback.as_ref()?;
        let status = audio.control.status()?;
        let applied = status.applied_revision.load(Acquire);
        let replaces_refused_loop = intent.prepared_revision.is_none()
            && intent.device_preview_identity.is_none()
            && intent
                .kind
                .section()
                .is_some_and(|section| self.section_can_replace_refused_loop(section));
        if (self.mode != super::EditorMode::Arrangement && intent.recovery_generation.is_none())
            || state.revision != intent.project_revision
            || self.project_generation != intent.project_generation
            || (!replaces_refused_loop
                && (self.applied_project_revision != Some(intent.project_revision)
                    || audible.state.revision != intent.project_revision))
            || audible.audio_revision != audio_revision
            || audible.audio_revision != applied
            || audible.project_generation != intent.project_generation
            || !seek_source_matches(audible, &intent.prepared_revision)
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || self.opening_project
            || self.close_prompt_open
            || !self.recovery.permits_seek(intent.recovery_generation)
            || !self.device_gesture_barriers.is_empty()
            || !self.seek_source_is_current(
                intent.project_generation,
                intent.project_revision,
                intent.device_preview_identity,
                intent.recovery_generation,
            )
            || audible.device_preview_identity != intent.device_preview_identity
            || audible.tuning_preview_identity.is_some()
            || !self.pending_audio_commands.is_empty()
            || self.pending_playback.is_some()
            || self.pending_resident_publication.is_some()
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
        let mut changed = self.poll_worker_owned_seek();
        if let Some(pending) = self
            .prepared_seek
            .as_ref()
            .filter(|pending| !pending.cancelled && !self.pending_seek_source_is_current(pending))
        {
            // Retain intent only for another sequence of this SAME held
            // gesture. Terminal, replacement, and other owners never restart it.
            let retry = pending.device_preview_identity.and_then(|previous| {
                self.active_seek_preview()
                    .filter(|current| {
                        current.key == previous.key
                            && current.project_generation == previous.project_generation
                            && current.canonical_revision == previous.canonical_revision
                    })
                    .map(|_| pending.intent())
            });
            let latest = self.latched_seek_intent.take();
            self.invalidate_prepared_seek();
            self.latched_seek_intent = latest.or(retry);
            changed = true;
        }
        if let Some(pending) = self
            .prepared_seek
            .as_mut()
            .filter(|pending| pending.cancellation_retry)
            && let Some(audio) = self.audio.as_ref()
        {
            let result = cancel_published_seek(Some(&audio.control), pending);
            pending.cancellation_retry = result == Some(FreshRuntimeCancellationResult::Busy);
            changed = !pending.cancellation_retry;
        }
        if let Some(result) = self.retired_seek_result.take() {
            changed |= self.retire_seek_result(result);
        }
        if self.retired_seek_result.is_some() {
            return changed;
        }
        if self.prepared_seek.is_none()
            && let Some(intent) = self.latched_seek_intent.take()
        {
            if let Some((_, audio_revision, _)) = if !matches!(intent.kind, SeekKind::Fresh) {
                self.current_prepared_seek_source()
            } else {
                self.current_preroll_source()
            } {
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
        if pending.prepared_revision.is_none() {
            // Native completion is scalar-only. Its actual graph can only be consumed by the
            // canonical worker; never use the legacy headless graph-return path here.
            return changed;
        }
        if pending.cancelled || pending.stage != super::arrangement_seek::SeekStage::Preparing {
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
        // The admitted runtime may derive from an uncommitted device preview.
        // Retain its exact view metadata as well as its prepared Arc/identity.
        let Some(state) = self
            .audible_playback
            .as_ref()
            .map(|audible| Arc::clone(&audible.state))
        else {
            self.prepared_seek = None;
            self.retire_seek_result(result);
            return true;
        };
        let ArrangementSeekResult {
            note_chase,
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
                        note_chase,
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
                    device_preview_identity: pending.device_preview_identity,
                    tuning_preview_identity: None,
                });
                self.status = "arrangement seek ready · waiting for audio".into();
                changed = true;
            }
            Err(error) => {
                self.prepared_seek = None;
                self.error = Some(
                    format!(
                        "arrangement seek failed · {}",
                        super::user_errors::seek_preparation(&error)
                    )
                    .to_lowercase()
                    .into(),
                );
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
        let Some(status) = audio.control.status() else {
            return false;
        };
        let active_revision = status.applied_revision.load(Acquire);
        let mut entries = [hex_audio::prepared::SessionRouteEntry::default();
            hex_audio::prepared::SESSION_ROUTE_CAPACITY];
        let Some(routes) = status.session_routes(&mut entries) else {
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
            && seek_source_matches(audible, &pending.prepared_revision)
            && routes.audio_revision == pending.fence.expected_active_revision()
            && routes.route_state_generation == pending.fence.route_state_generation()
            && routes.count <= entries.len()
            && routes.count == state.project.tracks().len()
            && routes.capability == hex_engine::SessionCapabilityStatus::Available
            && super::arrangement_seek::routes_are_current(&entries[..routes.count], &state.project)
            && self.worker_requests_pending == 0
            && self.pending_requests.is_empty()
            && !self.opening_project
            && !self.close_prompt_open
            && self.recovery.permits_seek(pending.recovery_generation)
            && self.device_gesture_barriers.is_empty()
            && self.seek_source_is_current(
                pending.project_generation,
                pending.project_revision,
                pending.device_preview_identity,
                pending.recovery_generation,
            )
            && audible.device_preview_identity == pending.device_preview_identity
            && audible.tuning_preview_identity.is_none()
            && audible.clip_preview_identity.is_none()
            && (allow_candidate
                || (self.pending_playback.is_none()
                    && self.pending_resident_publication.is_none()
                    && self.playback_candidates.is_empty()))
    }

    pub(super) fn retire_seek_before_playback_replacement(
        &mut self,
        incoming_preview: Option<DevicePreviewIdentity>,
    ) {
        let Some(pending) = self.prepared_seek.as_ref().filter(|pending| {
            pending.stage == SeekStage::CandidateReady
                && self
                    .pending_playback
                    .as_ref()
                    .is_some_and(|playback| playback.seek_request_id == Some(pending.request_id))
        }) else {
            return;
        };
        // Busy forwarding still leaves the exact runtime in PendingPlayback.
        // Retire it BEFORE receive replaces that slot, not on the later tick.
        // Only the same live held owner can carry the user's intent forward.
        let retry = pending
            .device_preview_identity
            .zip(incoming_preview)
            .and_then(|(previous, incoming)| {
                (incoming.key == previous.key
                    && incoming.project_generation == previous.project_generation
                    && incoming.canonical_revision == previous.canonical_revision
                    && incoming.sequence >= previous.sequence
                    && self.active_seek_preview().is_some_and(|active| {
                        active.key == incoming.key
                            && active.project_generation == incoming.project_generation
                            && active.canonical_revision == incoming.canonical_revision
                            && active.sequence >= incoming.sequence
                    }))
                .then(|| pending.intent())
            });
        let latest = self.latched_seek_intent.take();
        self.invalidate_prepared_seek();
        if retry.is_some() {
            self.latched_seek_intent = latest.or(retry);
        }
    }

    fn retire_seek_result(&mut self, result: ArrangementSeekResult) -> bool {
        // One seek is admitted at a time. Both request admission and result
        // consumption stop while the UI retirement fallback owns a result;
        // CandidateReady can therefore never coexist with that fallback.
        #[cfg(test)]
        assert!(
            self.retired_seek_result.is_none(),
            "occupied seek retirement owner"
        );
        #[cfg(test)]
        let identity = (
            result.request_id,
            Arc::as_ptr(&result.prepared_revision) as usize,
        );
        let handback = self.service.retire_arrangement_seek_result(result);
        #[cfg(test)]
        LAST_SEEK_RETIREMENT.set(Some((identity.0, identity.1, handback.is_ok())));
        if let Err(result) = handback {
            self.retired_seek_result = Some(result);
            false
        } else {
            true
        }
    }

    pub(super) fn invalidate_prepared_seek(&mut self) {
        self.section_launch_after_loop = None;
        self.latched_seek_intent = None;
        let Some(mut pending) = self.prepared_seek.take() else {
            self.service.invalidate_arrangement_seek();
            return;
        };
        pending
            .worker_cancel
            .store(true, std::sync::atomic::Ordering::Release);
        match pending.stage {
            super::arrangement_seek::SeekStage::Preparing => {
                if pending.prepared_revision.is_none() {
                    // The final enqueue can race cancellation. Retain scalar correlation until
                    // completion supplies the revision needed by conditional cancellation.
                    pending.cancelled = true;
                    self.prepared_seek = Some(pending);
                } else {
                    self.service.invalidate_arrangement_seek();
                }
            }
            super::arrangement_seek::SeekStage::CandidateReady => {
                let Some(prepared_revision) = pending.prepared_revision.clone() else {
                    self.error =
                        Some("worker-owned seek returned an unexpected local candidate".into());
                    self.prepared_seek = Some(pending);
                    return;
                };
                if self
                    .pending_playback
                    .as_ref()
                    .is_some_and(|playback| playback.seek_request_id == Some(pending.request_id))
                    && let Some(playback) = self.pending_playback.take()
                {
                    self.retire_seek_result(ArrangementSeekResult {
                        note_chase: pending.note_chase,
                        request_id: pending.request_id,
                        mode: pending.kind.mode(),
                        generation: pending.project_generation,
                        revision: pending.project_revision,
                        prepared_revision,
                        destination_frame: pending.destination_frame,
                        work_budget_frames: pending.work_budget_frames,
                        start_playing: pending.start_playing,
                        result: Ok(playback.playback),
                    });
                } else if self
                    .pending_resident_publication
                    .as_ref()
                    .is_some_and(|publication| {
                        publication.seek_request_id == Some(pending.request_id)
                    })
                {
                    // The existing publication worker owns the runtime now.
                    // Keep its token until the response supplies the revision
                    // needed by the existing conditional cancellation API.
                    pending.cancelled = true;
                    self.prepared_seek = Some(pending);
                }
            }
            super::arrangement_seek::SeekStage::Published { .. } => {
                let control = self.audio.as_ref().map(|audio| &audio.control);
                pending.cancellation_retry =
                    super::arrangement_seek::cancel_published_seek(control, &mut pending)
                        == Some(FreshRuntimeCancellationResult::Busy);
                self.prepared_seek = Some(pending);
            }
        }
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        // Request cancellation before Service shutdown; native custody remains
        // with its worker even when publication admission has already won.
        self.cancel_plugin_effect();
        if let Some(control) = &self.bounce_control {
            control.cancel();
        }
        self.cancel_browser_sample_playback();
        if let Some(control) = &self.audio_batch {
            control.request_cancel();
        }
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
            // Interactive starts reset effect history and chase held notes in
            // the already prepared runtime. Exact-history seeks retain their
            // separate worker path; Play must never replay the song prefix.
            Some(_) => ToggleAction::PlayFrom(start_frame),
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
