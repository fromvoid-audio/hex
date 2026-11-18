//! Keep one staged verified insert on the same native worker and candidate
//! queue as other verified graphs. The Session owns the exact editor commit.
use super::*;
use hex_audio::prepared::{AppliedAudioSource, DeviceInsertConsent};
use hex_session::service::{RequestCancellation, VerifiedDeviceInsertAttempt};

#[derive(Clone)]
pub(super) struct Insert {
    pub(super) attempt: VerifiedDeviceInsertAttempt,
    pub(super) predecessor: AppliedAudioSource,
    pub(super) approval: Option<DeviceInsertConsent>,
    pub(super) cancellation: Option<RequestCancellation>,
}

impl NativeAudioSink {
    pub(super) fn begin_verified_insert(
        &mut self,
        attempt: VerifiedDeviceInsertAttempt,
    ) -> Result<(), String> {
        let canonical = self
            .desired
            .as_ref()
            .filter(|source| {
                source.context() == attempt.predecessor()
                    && source.audio_mode() == DocumentAudioMode::VerifiedStreaming
            })
            .ok_or("verified device insert needs the installed canonical source")?;
        // Retain the owned gain preview until the removal actually installs.
        // A refused graph edit leaves that preview and its gesture intact.
        let preview_blocks_edit = match self.verified_preview.as_ref() {
            None => false,
            Some(VerifiedSource::Gain(gain)) => !attempt.retiring_gain().is_some_and(|identity| {
                gain.preview_identity() == Some(identity)
                    && Arc::ptr_eq(gain.canonical(), canonical)
            }),
            Some(_) => true,
        };
        if self.sampler.is_some() || preview_blocks_edit {
            return Err("verified device insert needs the installed canonical source".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("audio output is unavailable")?;
        let predecessor = control
            .status
            .applied_audio_source()
            .filter(|source| source.context == attempt.predecessor())
            .ok_or("verified device insert needs the installed canonical source")?;
        let approval = match (attempt.confirmation(), self.verified_insert.as_ref()) {
            (None, None) => None,
            (Some(submission), Some(old))
                if old.attempt.submission() == Some(submission)
                    && old.predecessor == predecessor
                    && Arc::ptr_eq(old.attempt.source(), attempt.source()) =>
            {
                Some(
                    resident::resident_insert_consent(control, submission)
                        .ok_or("device insert confirmation is stale")?,
                )
            }
            _ => return Err("verified device insert preparation changed".into()),
        };
        self.verified_insert = Some(Insert {
            attempt,
            predecessor,
            approval,
            cancellation: self.request_cancellation.clone(),
        });
        Ok(())
    }

    pub(super) fn finish_verified_insert(
        &mut self,
        attempt: &VerifiedDeviceInsertAttempt,
        installed: bool,
    ) {
        if self
            .verified_insert
            .as_ref()
            .is_none_or(|insert| !insert.attempt.same_attempt(attempt))
        {
            return;
        }
        self.verified_insert = None;
        if installed && let Some(submission) = attempt.submission() {
            self.verified_last =
                Some(VerifiedSource::Canonical(Arc::clone(attempt.source())).key());
            self.last_context = Some(attempt.source().context());
            // Keep the exact installed source until the Session's canonical
            // event supplies the committed snapshot at the same revision.
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
