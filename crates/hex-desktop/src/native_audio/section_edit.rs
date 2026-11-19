//! Scalar arrangement-edit custody on the existing publication endpoint. No PCM,
//! resource book or runtime is owned here. Accepted gates retire only after the
//! exact candidate installs or refuses, or with the closing output.
use super::*;
use hex_audio::prepared::{StoppedEditFence, StoppedEditFenceState};

pub(super) struct Pending {
    fence: StoppedEditFence,
    output: u64,
    submitted: Option<u64>,
    recovering: bool,
}
impl NativeAudioSink {
    pub(super) fn begin_section_edit(
        &mut self,
        context: AudioSourceContext,
        allow_playing: bool,
        recover: bool,
    ) -> Result<(), String> {
        self.poll_section_edit();
        if self.section_edit.is_some()
            || self.offline_section_edit.is_some()
            || self.sampler_parameter.is_some()
        {
            return Err("another arrangement edit is pending".into());
        }
        if !self
            .desired
            .as_ref()
            .is_some_and(|source| source.context() == context)
        {
            return Err("arrangement audio context changed".into());
        }
        if self.output.is_none() {
            // No output means no callback can render or move the cursor, so no
            // fence is needed: the edit is stopped at the last control-side
            // cursor and publishes offline as a fresh stopped install.
            self.offline_section_edit = Some(self.offline_stopped_cursor());
            return Ok(());
        }
        if let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            && (control.status.device_fault.load(Acquire)
                || control.status.render_fault.load(Acquire))
        {
            // A render fault recovers on this same output with the next applied
            // publication, so the callback may resume mid-edit; recovery is the
            // existing restart-audio path.
            return Err("audio unavailable; restart audio to retry the arrangement edit".into());
        }
        let submitted = self
            .resident
            .current_submission
            .ok_or("arrangement audio has not been submitted")?;
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("arrangement output is unavailable")?;
        let output = control
            .output_identity()
            .ok_or("arrangement output identity unavailable")?;
        if output != submitted.output_identity {
            return Err("arrangement output changed".into());
        }
        // The canonical project can be ahead of the active runtime after a
        // live edit was refused. Recover only from a known terminal on this
        // output. Pending publications still cannot be bypassed by a fence.
        let recovering = recover
            && submitted
                .source_context
                .is_some_and(|source| source.project_generation == context.project_generation)
            && control
                .status
                .rejected_revision()
                .is_some_and(|(revision, _)| revision == submitted.audio_revision);
        let revision = if recovering {
            control
                .status
                .recording_source()
                .filter(|source| {
                    source.output_identity == output
                        && source.context.project_generation == context.project_generation
                        && source.context.editor_revision <= context.editor_revision
                })
                .ok_or("arrangement audio has no applied project source")?
                .audio_revision
        } else if submitted.source_context == Some(context) {
            submitted.audio_revision
        } else {
            return Err("arrangement audio has not been submitted".into());
        };
        let fence = if allow_playing {
            control.request_live_arrangement_edit_fence(context.project_generation, revision)
        } else {
            control.request_arrangement_edit_fence(context.project_generation, revision)
        }
        .map_err(str::to_owned)?;
        self.section_edit = Some(Pending {
            fence,
            output,
            submitted: None,
            recovering,
        });
        Ok(())
    }
    pub(super) fn confirm_section_edit_recovery(
        &mut self,
        context: AudioSourceContext,
        acceptance: hex_session::service::ResidentPublicationAcceptance,
    ) -> Result<(), String> {
        if !self
            .section_edit
            .as_ref()
            .is_some_and(|edit| edit.recovering)
        {
            return Ok(());
        }
        let result =
            self.confirm_resident_edit_publication(context, acceptance, "arrangement edit");
        self.poll_section_edit();
        result
    }

    pub(super) fn section_edit_ready(
        &mut self,
    ) -> Option<Result<hex_session::service::ArrangementEditPosition, String>> {
        if let Some(position) = self.offline_section_edit {
            return Some(Ok(position));
        }
        let pending = self.section_edit.as_ref()?;
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return Some(Err("arrangement output closed".into()));
        };
        if control.output_identity() != Some(pending.output) {
            return Some(Err("arrangement output changed".into()));
        }
        match control.stopped_edit_fence_state(pending.fence) {
            StoppedEditFenceState::Pending => None,
            // Transport changed under the gate; the edit refuses without
            // asking for Stop, and the next edit may simply retry.
            StoppedEditFenceState::Refused => {
                Some(Err("arrangement playback changed before the edit".into()))
            }
            StoppedEditFenceState::Ready => Some(
                control
                    .arrangement_edit_position(pending.fence)
                    .zip(control.arrangement_edit_playing(pending.fence))
                    .ok_or_else(|| "arrangement position unavailable".into())
                    .and_then(|(frame, playing)| {
                        hex_session::service::ArrangementEditPosition::from_frames(
                            frame,
                            playing,
                            control.arrangement_edit_section_loop(pending.fence),
                        )
                    }),
            ),
        }
    }
    pub(super) fn section_publication_policy(
        &mut self,
    ) -> Result<hex_audio::prepared::PublicationPolicy, String> {
        let position = self
            .section_edit_ready()
            .ok_or("arrangement fence is pending")??;
        let pending = self
            .section_edit
            .as_ref()
            .ok_or("arrangement fence unavailable")?;
        if pending.submitted.is_some() {
            return Err("arrangement publication already submitted".into());
        }
        Ok(if position.playing {
            hex_audio::prepared::PublicationPolicy::InheritUnderArrangement(pending.fence)
        } else {
            hex_audio::prepared::PublicationPolicy::InstallUnderStoppedArrangement(pending.fence)
        })
    }
    pub(super) fn section_submitted(&mut self, revision: u64) {
        if let Some(pending) = self.section_edit.as_mut() {
            pending.submitted = Some(revision);
        }
    }
    pub(super) fn cancel_section_edit(&mut self) {
        self.offline_section_edit = None;
        if self
            .section_edit
            .as_ref()
            .is_some_and(|pending| pending.submitted.is_some())
        {
            return; // Admission transferred custody; cancellation cannot release it.
        }
        if let Some(pending) = self.section_edit.take()
            && let Some(control) = self
                .output
                .as_mut()
                .and_then(|output| output.publication.control())
            && control.output_identity() == Some(pending.output)
        {
            control.release_stopped_edit_fence(pending.fence);
        }
    }
    pub(super) fn poll_section_edit(&mut self) {
        let Some(pending) = self.section_edit.as_ref() else {
            return;
        };
        let Some(revision) = pending.submitted else {
            return;
        };
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return;
        };
        if control.output_identity() == Some(pending.output)
            && (control.status.applied_revision.load(Acquire) == revision
                || control
                    .status
                    .rejected_revision()
                    .is_some_and(|(rejected, _)| rejected == revision))
        {
            control.release_stopped_edit_fence(pending.fence);
            self.section_edit = None;
        }
    }
    /// The cursor control state knows without a callback: the retained offline
    /// publication's stopped cursor, else the last relinquished output's.
    fn offline_stopped_cursor(&self) -> hex_session::service::ArrangementEditPosition {
        self.resident
            .pending_cursor()
            .unwrap_or(hex_session::service::ArrangementEditPosition {
                frame: self.last_stopped_frame,
                playing: false,
                section_loop: None,
            })
    }
    /// Take an offline stopped edit for retention without an output. Its
    /// position is already in the Playback, so it installs as a fresh stopped
    /// runtime when an output opens; no fence or callback custody exists.
    pub(super) fn take_offline_section_edit(&mut self) -> bool {
        self.offline_section_edit.take().is_some()
    }
}
