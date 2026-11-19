//! Retain one staged edit on the existing verified constructor pipeline.
//! Session commits only after the exact audio receipt is installed.
use super::*;
use hex_audio::prepared::{
    AppliedAudioSource, CancellablePublicationReceipt, DeviceInsertConsent, FreshRuntimeFence,
    PublicationControl,
};
use hex_session::service::{RequestCancellation, VerifiedGraphEditAttempt};

pub(super) const TEMPO_CUT_LEAD_FRAMES: u64 = 1024;

#[derive(Clone)]
pub(super) struct Edit {
    pub(super) attempt: VerifiedGraphEditAttempt,
    pub(super) predecessor: AppliedAudioSource,
    transition: Transition,
    pub(super) cancellation: Option<RequestCancellation>,
}

#[derive(Clone, Copy)]
enum Transition {
    DeviceInsert(Option<DeviceInsertConsent>),
    TempoCut(FreshRuntimeFence),
}

impl Edit {
    pub(super) fn stage(
        &self,
        control: &mut PublicationControl,
        token: hex_audio::AudioCandidatePreparationToken,
        context: AudioSourceContext,
    ) -> Result<
        (
            hex_audio::AudioCandidateTicket,
            CancellablePublicationReceipt,
        ),
        &'static str,
    > {
        match self.transition {
            Transition::DeviceInsert(approval) => {
                control.stage_verified_device_insert(token, context, self.predecessor, approval)
            }
            Transition::TempoCut(fence) => {
                let (observation, _) = control
                    .capture_running_transport_fence()
                    .filter(|(_, current)| *current == fence)
                    .ok_or("tempo edit lost its running transport")?;
                // Let the page worker service up to four callback quanta
                // before the scheduled performance-frame cut.
                let frame = observation
                    .performance_frame
                    .checked_add(TEMPO_CUT_LEAD_FRAMES)
                    .ok_or("tempo cut performance frame exhausted")?;
                control.stage_verified_tempo_cut(token, context, self.predecessor, fence, frame)
            }
        }
    }
}

impl NativeAudioSink {
    pub(super) fn begin_verified_edit(
        &mut self,
        attempt: VerifiedGraphEditAttempt,
    ) -> Result<(), String> {
        let canonical = self
            .desired
            .as_ref()
            .filter(|source| {
                source.context() == attempt.predecessor()
                    && source.audio_mode() == DocumentAudioMode::VerifiedStreaming
            })
            .ok_or("verified graph edit needs the installed canonical source")?;
        // Retain the owned gain preview until the removal actually installs.
        // A refused graph edit leaves that preview and its gesture intact.
        let retiring_key = match self.verified_preview.as_ref() {
            None => None,
            Some(preview @ VerifiedSource::Gain(gain))
                if attempt.retiring_gain().is_some_and(|identity| {
                    gain.preview_identity() == Some(identity)
                        && Arc::ptr_eq(gain.canonical(), canonical)
                }) =>
            {
                Some(preview.key())
            }
            Some(_) => {
                return Err("verified graph edit needs the installed canonical source".into());
            }
        };
        if self.sampler.is_some() {
            return Err("verified graph edit needs the installed canonical source".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("audio output is unavailable")?;
        let canonical_key = VerifiedKey::canonical(attempt.predecessor());
        // A preview and the canonical project can share the editor context.
        // Only the exact output ACK and its submitted candidate can establish
        // which is installed. The audio owner still revalidates at callback.
        let predecessor = control
            .status
            .applied_audio_source()
            .filter(|source| {
                control.output_identity() == Some(source.output_identity)
                    && self.submitted.iter().flatten().any(|submission| {
                        submission.source == *source
                            && std::ptr::eq(submission.canonical.as_ptr(), Arc::as_ptr(canonical))
                            && (submission.key == canonical_key
                                || Some(submission.key) == retiring_key)
                    })
            })
            .ok_or("verified graph edit needs the installed canonical source")?;
        let transition = match (attempt.confirmation(), self.verified_edit.as_ref()) {
            (None, None) if attempt.needs_tempo_cut() => {
                let (_, fence) = control
                    .capture_running_transport_fence()
                    .filter(|(observation, _)| {
                        observation.audio_revision == predecessor.audio_revision
                    })
                    .ok_or("tempo edit needs the running canonical arrangement")?;
                Transition::TempoCut(fence)
            }
            (None, None) => Transition::DeviceInsert(None),
            (Some(submission), Some(old))
                if old.attempt.submission() == Some(submission)
                    && old.predecessor == predecessor
                    && Arc::ptr_eq(old.attempt.source(), attempt.source()) =>
            {
                Transition::DeviceInsert(Some(
                    resident::resident_insert_consent(control, submission)
                        .ok_or("device insert confirmation is stale")?,
                ))
            }
            _ => return Err("verified graph edit preparation changed".into()),
        };
        self.verified_edit = Some(Edit {
            attempt,
            predecessor,
            transition,
            cancellation: self.request_cancellation.clone(),
        });
        Ok(())
    }

    pub(super) fn finish_verified_edit(
        &mut self,
        attempt: &VerifiedGraphEditAttempt,
        installed: bool,
    ) {
        if self
            .verified_edit
            .as_ref()
            .is_none_or(|insert| !insert.attempt.same_attempt(attempt))
        {
            return;
        }
        self.verified_edit = None;
        if installed && let Some(submission) = attempt.submission() {
            self.verified_last =
                Some(VerifiedSource::Canonical(Arc::clone(attempt.source())).key());
            self.last_context = Some(attempt.source().context());
            // Session publishes this same snapshot when it emits the committed edit.
            self.desired = Some(Arc::clone(attempt.source()));
            self.observe(
                submission.context,
                "submitted",
                EditorPublicationState::Submitted {
                    audio_revision: submission.audio_revision,
                },
            );
        }
    }
}
