//! Scalar stopped-edit custody on the existing publication endpoint. No PCM,
//! resource book or runtime is owned here. Accepted gates retire only after the
//! exact candidate is installed, or with the closing output.
use super::*;
use hex_audio::prepared::{StoppedEditFence, StoppedEditFenceState};

pub(super) struct Pending {
    fence: StoppedEditFence,
    output: u64,
    submitted: Option<u64>,
}
impl NativeAudioSink {
    pub(super) fn begin_section_edit(&mut self, context: AudioSourceContext) -> Result<(), String> {
        self.poll_section_edit();
        if self.section_edit.is_some() || self.sampler_parameter.is_some() {
            return Err("another stopped edit is pending".into());
        }
        if !self
            .desired
            .as_ref()
            .is_some_and(|source| source.context() == context)
        {
            return Err("arrangement audio context changed".into());
        }
        let (_, revision) = self
            .resident
            .current_submission
            .filter(|(source, _)| *source == context)
            .ok_or("arrangement audio has not been submitted")?;
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("arrangement output is unavailable")?;
        let output = control
            .output_identity()
            .ok_or("arrangement output identity unavailable")?;
        let fence = control
            .request_arrangement_edit_fence(context.project_generation, revision)
            .map_err(str::to_owned)?;
        self.section_edit = Some(Pending {
            fence,
            output,
            submitted: None,
        });
        Ok(())
    }
    pub(super) fn section_edit_ready(&mut self) -> Option<Result<u64, String>> {
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
            StoppedEditFenceState::Refused => {
                Some(Err("stop playback before reordering sections".into()))
            }
            StoppedEditFenceState::Ready => Some(
                control
                    .arrangement_edit_position(pending.fence)
                    .ok_or_else(|| "arrangement stopped position unavailable".into()),
            ),
        }
    }
    pub(super) fn section_publication_policy(
        &mut self,
    ) -> Result<hex_audio::prepared::PublicationPolicy, String> {
        self.section_edit_ready()
            .ok_or("arrangement stopped fence is pending")??;
        let pending = self
            .section_edit
            .as_ref()
            .ok_or("arrangement stopped fence unavailable")?;
        if pending.submitted.is_some() {
            return Err("arrangement publication already submitted".into());
        }
        Ok(hex_audio::prepared::PublicationPolicy::InstallUnderStoppedArrangement(pending.fence))
    }
    pub(super) fn section_submitted(&mut self, revision: u64) {
        if let Some(pending) = self.section_edit.as_mut() {
            pending.submitted = Some(revision);
        }
    }
    pub(super) fn cancel_section_edit(&mut self) {
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
            && control.status.applied_revision.load(Acquire) == revision
        {
            control.release_stopped_edit_fence(pending.fence);
            self.section_edit = None;
        }
    }
}
