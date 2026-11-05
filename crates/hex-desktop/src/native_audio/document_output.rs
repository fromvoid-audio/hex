//! Private rate-changing output preparation on the canonical Session worker.
//! The existing lifecycle lane owns the exclusive stream lease. The UI only
//! releases the old client; it cannot reach the candidate until Session commits.
use super::*;
use hex_session::service::RequestCancellation;
use std::time::{Duration, Instant};

pub(super) struct ReplacementOutput {
    pub(super) context: AudioSourceContext,
    pub(super) deadline: Instant,
    requires_plugin_profile: bool,
}

pub(super) fn check_deadline(
    deadline: Instant,
    cancellation: Option<&RequestCancellation>,
) -> Result<(), String> {
    if cancellation.is_some_and(RequestCancellation::is_shutting_down) {
        Err("project open cancelled".into())
    } else if Instant::now() >= deadline {
        Err("timed out preparing audio output".into())
    } else {
        Ok(())
    }
}

impl NativeAudioSink {
    pub(super) fn replacement_needs_output_preflight(&self, rate: u32) -> bool {
        self.desired
            .as_ref()
            .is_some_and(|old| old.sample_rate() != rate)
            || self
                .output
                .as_ref()
                .and_then(|output| output.route.as_ref())
                .is_some_and(|route| route.nominal_hz != rate)
    }

    pub(super) fn open_requires_plugin_profile(&self) -> bool {
        self.replacement_output.as_ref().map_or_else(
            || {
                self.desired
                    .as_ref()
                    .is_some_and(|source| source.effect_preparation().requires_plugin_profile())
            },
            |replacement| replacement.requires_plugin_profile,
        )
    }

    pub(super) fn release_output_for_document(&mut self, generation: u64) {
        self.relinquish_output();
        if self.pending_release_event.is_none()
            && let Some(old_output_identity) = self.relinquished_output_identity
        {
            self.pending_release_event = Some(EditorAudioEvent::OutputReleased {
                project_generation: generation,
                old_output_identity,
            });
        }
    }

    pub(super) fn open_replacement_output(
        &mut self,
        context: AudioSourceContext,
        sample_rate: u32,
        requires_plugin_profile: bool,
    ) -> Result<(), String> {
        let generation = self
            .desired
            .as_ref()
            .map(|source| source.context().project_generation)
            .filter(|old| old.checked_add(1) == Some(context.project_generation))
            .ok_or("document replacement context is stale")?;
        let deadline = Instant::now() + STARTUP_OPEN_DEADLINE;
        check_deadline(deadline, self.request_cancellation.as_ref())?;
        if self.sampler_blocks_client() || self.section_edit.is_some() {
            return Err("finish the active audio edit before opening a project".into());
        }
        if self.sampler.is_some() && !self.cancel_sampler() {
            return Err(
                "audio ownership cleanup is incomplete; save project and restart hex".into(),
            );
        }
        // Do not clear any old document/source state. Only its output must
        // retire before the exclusive lifecycle lane can build the candidate.
        self.release_output_for_document(generation);
        self.request_open(Some(sample_rate), None);
        self.replacement_output = Some(ReplacementOutput {
            context,
            deadline,
            requires_plugin_profile,
        });

        let result = loop {
            if let Err(error) = self.check_replacement_deadline() {
                break Err(error);
            }
            self.flush_events();
            self.poll_open();
            if let Some(error) = self.open_error.take() {
                break Err(error.to_string());
            }
            if self.output.is_some() {
                break Ok(());
            }
            if !self.open_pending {
                break Err("audio output preparation stopped".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        if let Err(ref reason) = result {
            self.refuse_replacement_output(reason);
        }
        result
    }

    pub(super) fn check_replacement_deadline(&self) -> Result<(), String> {
        let replacement = self
            .replacement_output
            .as_ref()
            .ok_or("project audio preparation is unavailable")?;
        check_deadline(replacement.deadline, self.request_cancellation.as_ref())
    }

    pub(super) fn wait_for_replacement_installation(&mut self) -> Result<(), String> {
        let Some(replacement) = self.replacement_output.as_ref() else {
            return Ok(()); // Existing same-rate deferred publication.
        };
        let context = replacement.context;
        let result = (|| {
            let submission = self
                .resident
                .current_submission
                .filter(|submission| submission.source_context == Some(context))
                .ok_or("project audio graph was not submitted")?;
            loop {
                self.check_replacement_deadline()?;
                let control = self
                    .output
                    .as_mut()
                    .and_then(|output| output.publication.control())
                    .ok_or("project audio output closed during publication")?;
                let status = &control.status;
                if status.device_fault.load(Acquire) || status.render_fault.load(Acquire) {
                    return Err("project audio output failed during publication".into());
                }
                if status.recording_source().is_some_and(|applied| {
                    applied.context == context
                        && applied.audio_revision == submission.audio_revision
                        && applied.output_identity == submission.output_identity
                }) {
                    return Ok(());
                }
                if let Some((revision, reason)) = status.rejected_revision()
                    && revision == submission.audio_revision
                {
                    return Err(format!("project audio graph refused: {}", reason.message()));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })();
        if let Err(ref reason) = result {
            self.refuse_replacement_output(reason);
        }
        result
    }

    pub(super) fn refuse_replacement_output(&mut self, reason: &str) {
        self.replacement_output = None;
        // A private candidate's identity never crossed the Client boundary.
        self.output_identity = None;
        self.relinquish_output();
        self.open_error = Some(
            format!(
                "project open failed: {reason}; use restart audio to restore the current project"
            )
            .into(),
        );
    }
}
