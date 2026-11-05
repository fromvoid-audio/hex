//! A delivered Resident output can hold one stopped document transition.
//! Session has already accepted the saved document and retained its graph.
//! The callback fence drains the outgoing command prefix. The exact applied
//! successor permits retirement of outgoing VST3 sources and client rebind.
use super::super::*;
use hex_audio::prepared::{PublicationControl, StoppedEditFence, StoppedEditFenceState};
use std::time::Instant;

pub(in crate::native_audio) struct Rebind {
    pub(in crate::native_audio) context: AudioSourceContext,
    pub(in crate::native_audio) fence: StoppedEditFence,
    pub(in crate::native_audio) restores_plugins: bool,
    outgoing_generation: u64,
    output_identity: u64,
    deadline: Instant,
}

impl Rebind {
    pub(in crate::native_audio) fn ready(&self, control: &PublicationControl) -> bool {
        control.stopped_edit_fence_state(self.fence) == StoppedEditFenceState::Ready
            && !self.interrupted(control)
    }

    fn interrupted(&self, control: &PublicationControl) -> bool {
        self.restores_plugins && control.stopped_arrangement_edit_interrupted(self.fence)
    }
}

impl NativeAudioSink {
    pub(in crate::native_audio) fn begin_document_rebind(
        &mut self,
        incoming: &CanonicalAudioSourceSnapshot,
    ) -> bool {
        let Some(previous) = self.desired.as_ref() else {
            return false;
        };
        let Some(output_identity) = self.output_identity else {
            return false;
        };
        let context = incoming.context();
        let outgoing_generation = previous.context().project_generation;
        if previous.audio_mode() != DocumentAudioMode::Resident
            || incoming.audio_mode() != DocumentAudioMode::Resident
            || previous.sample_rate() != incoming.sample_rate()
            || self.output_generation != Some(outgoing_generation)
            || self.document_rebind.is_some()
            || self.open_pending
            || self.opening.is_some()
            || self.open_error.is_some()
            || self.pending_open_event.is_some()
            || self.pending_release_event.is_some()
            || self.pending_observation.is_some()
            || self.pending_status_observation.is_some()
            || self.sampler.is_some()
            || self.sampler_parameter.is_some()
            || self.sampler_budget_wait.is_some()
            || self.suspended_output.is_some()
            || self.section_edit.is_some()
            || self.preparation.is_some()
            || self.domains.iter().any(Option::is_some)
            || self.submitted.iter().any(Option::is_some)
            || self.resident.retired.is_some()
            || !self.resident.has_pending_document(context)
        {
            return false;
        }
        let Some(control) = self
            .output
            .as_mut()
            .filter(|output| output.sample_rate == incoming.sample_rate())
            .and_then(|output| output.publication.control())
        else {
            return false;
        };
        if !self.plugins.can_retain_output_for_document(
            control,
            outgoing_generation,
            incoming.effect_preparation(),
        ) {
            return false;
        }
        let Some(active) = control.status.recording_source() else {
            return false;
        };
        if active.output_identity != output_identity
            || active.context != previous.context()
            || control.status.playing.load(Acquire)
            || !self.resident.current_submission.is_some_and(|submitted| {
                submitted.audio_revision == active.audio_revision
                    && submitted.output_identity == output_identity
            })
        {
            return false;
        }
        let Ok(fence) = control.request_arrangement_edit_fence(
            active.context.project_generation,
            active.audio_revision,
        ) else {
            return false;
        };
        self.document_rebind = Some(Rebind {
            context,
            output_identity,
            outgoing_generation,
            restores_plugins: incoming.effect_preparation().requires_plugin_profile(),
            fence,
            deadline: Instant::now() + STARTUP_OPEN_DEADLINE,
        });
        self.resident.applied_loop = None;
        self.last_context = None;
        self.last_state = None;
        self.blocked = None;
        self.last_stopped_frame = 0;
        true
    }

    pub(in crate::native_audio) fn document_rebind_ready_to_submit(&mut self) -> bool {
        let Some(rebind) = self.document_rebind.as_ref() else {
            return true;
        };
        if self
            .resident
            .current_submission
            .is_some_and(|submission| submission.source_context == Some(rebind.context))
        {
            return false;
        }
        self.output
            .as_mut()
            .and_then(|output| output.publication.control())
            .is_some_and(|control| rebind.ready(control))
    }

    pub(in crate::native_audio) fn poll_document_rebind(&mut self) {
        let Some(rebind) = self.document_rebind.as_ref() else {
            return;
        };
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            self.fallback_document_rebind();
            return;
        };
        if rebind.interrupted(control) {
            self.fallback_document_rebind();
            return;
        }
        let submitted = self.resident.current_submission.filter(|submission| {
            submission.source_context == Some(rebind.context)
                && submission.output_identity == rebind.output_identity
        });
        if let Some(submitted) = submitted
            && control.status.recording_source().is_some_and(|source| {
                source.context == rebind.context
                    && source.output_identity == rebind.output_identity
                    && source.audio_revision == submitted.audio_revision
            })
        {
            let generation = rebind.context.project_generation;
            let output_identity = rebind.output_identity;
            self.plugins
                .retire_document_sources(rebind.outgoing_generation);
            self.resident
                .discard_obsolete_pending_after_restore(rebind.context);
            control.release_stopped_edit_fence(rebind.fence);
            self.document_rebind = None;
            self.output_generation = Some(generation);
            self.pending_open_event = Some(EditorAudioEvent::OutputRebound {
                project_generation: generation,
                output_identity,
            });
        } else if Instant::now() >= rebind.deadline
            || control.stopped_edit_fence_state(rebind.fence) == StoppedEditFenceState::Refused
            || submitted.is_some_and(|submission| {
                control
                    .status
                    .rejected_revision()
                    .is_some_and(|(revision, _)| revision == submission.audio_revision)
            })
        {
            self.fallback_document_rebind();
        }
    }

    pub(in crate::native_audio) fn fallback_document_rebind(&mut self) {
        let Some(rebind) = self.document_rebind.take() else {
            return;
        };
        let generation = rebind.context.project_generation;
        let source = self
            .desired
            .as_ref()
            .filter(|source| source.context().project_generation == generation)
            .cloned();
        self.release_output_for_document(generation);
        if let Some(source) = source
            && self.resident.has_pending_document(rebind.context)
        {
            self.request_open(Some(source.sample_rate()), None);
            self.plugins.request_reopen(&source);
        } else {
            // An already submitted graph belongs to the retiring Root. The
            // accepted project stays open and Restart Audio can prepare it.
            self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                project_generation: generation,
                restart_request: None,
                error: "project audio rebind failed · restart audio to restore playback".into(),
            });
        }
    }
}
