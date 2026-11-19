//! Exact tempo cuts for the ordinary publication slot. Control owns
//! the prepared anchor through the graph receipt. The callback borrows it
//! while inheriting, then returns the original graph through ordinary custody.
use super::*;

pub(super) const TEMPO_STREAM_WINDOW_FRAMES: usize = 256;

/// A move-only cut tied to its output, revisions and captured commands.
/// Admission moves it into the graph receipt for control-side reclamation.
#[derive(Debug)]
pub struct FencedTempoCut {
    pub(super) anchor: hex_engine::session_playback::PreparedTempoAnchor,
    pub(super) fence: FreshRuntimeFence,
    pub(super) output_identity: u64,
    pub(super) control_generation: u64,
    pub(super) performance_frame: u64,
}

pub(super) enum FencedAdmission<'a> {
    DeviceInsert(DeviceInsertAdmission<'a>),
    TempoCut(FencedTempoCut),
}

impl PublicationControl {
    /// Prepare a new frame for the exact missed edit, provided its original
    /// output, revisions and command/route fence survived. A later Stop or seek
    /// invalidates the original transaction even if it left the graph installed.
    pub fn reprepare_missed_tempo_cut(
        &self,
        missed_revision: u64,
        context: hex_engine::AudioSourceContext,
        previous: &Arc<hex_engine::PreparedRevision>,
        candidate: &Arc<hex_engine::PreparedRevision>,
        cut_performance_frame: u64,
    ) -> Result<Option<FencedTempoCut>, hex_engine::session_playback::SessionPlaybackError> {
        let original = self
            .latest_fresh_runtime_cancellation
            .as_ref()
            .filter(|receipt| {
                receipt.revision() == missed_revision
                    && receipt.context() == Some(context)
                    && receipt.output_identity() == self.candidate_root.root_id()
                    && receipt.state()
                        == PublicationReceiptState::Refused(
                            PlaybackHandoffRejection::TempoCutMissed,
                        )
            })
            .and_then(CancellablePublicationReceipt::tempo_cut)
            .filter(|cut| cut.anchor.matches_revisions(previous, candidate));
        let Some(original) = original else {
            return Ok(None);
        };
        self.prepare_fenced_tempo_anchor(
            original.fence.expected_active_revision,
            original.control_generation,
            Arc::clone(previous),
            Arc::clone(candidate),
            cut_performance_frame,
        )
        .map(|cut| cut.filter(|cut| cut.fence == original.fence))
    }

    /// Admit an ordinary tempo edit through the ordinary resource book. A
    /// submission has a cancellable receipt and may still miss the exact cut.
    #[allow(clippy::result_large_err)] // Refusal returns the actual graph to control.
    pub fn publish_tempo_cut_with_admission_on_control<E>(
        &mut self,
        playback: Playback,
        cut: FencedTempoCut,
        context: hex_engine::AudioSourceContext,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        self.publish_admitted_with_source(
            playback,
            PublicationPolicy::InheritAtTempoCut(cut.fence),
            context.project_generation,
            Some(context),
            Some(FencedAdmission::TempoCut(cut)),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }
}

impl Publication {
    pub(super) fn tempo_cut(&self) -> Option<&FencedTempoCut> {
        self.fresh_runtime_cancellation
            .as_ref()
            .and_then(|receipt| receipt.0.tempo_cut())
    }
}

impl Callback {
    pub(super) fn prewarm_tempo_cut(&mut self) {
        use crate::candidate_custody::TempoStreamWindow;
        let Some(next) = self
            .deferred
            .as_mut()
            .filter(|next| next.playback.contains_streamed_sources())
        else {
            return;
        };
        let Some(cut) = next
            .fresh_runtime_cancellation
            .as_ref()
            .and_then(|receipt| receipt.0.tempo_cut())
        else {
            return;
        };
        let Some(old) = self.active.as_mut().and_then(ActivePublication::legacy_mut) else {
            return;
        };
        if old.playback.performance_frame() >= cut.performance_frame {
            return;
        }
        let Some(context) = old.source_context else {
            return;
        };
        let Some(streams) = next.streams.as_mut() else {
            return;
        };
        if streams
            .prepare_install(
                &mut next.playback,
                Some((&mut old.playback, context, old.revision)),
                &mut self.domain_pages,
                false,
                self.stream_prefix,
                TEMPO_STREAM_WINDOW_FRAMES,
                false,
                false,
                None,
                Some(TempoStreamWindow::Forecast(&cut.anchor)),
            )
            .is_err()
        {
            // Speculative readiness must yield. Exact planning at the cut
            // reads the live donor and may choose a different page window.
            streams.cancel_install(&mut self.domain_pages);
        }
    }

    pub(super) fn tempo_publication_due(&mut self, fence: FreshRuntimeFence) -> bool {
        let Some(next) = self.deferred.as_ref() else {
            return true;
        };
        if self.held_retirement.is_some() {
            return false;
        }
        let old = self.active.as_ref().and_then(ActivePublication::legacy);
        let reason = if next
            .fresh_runtime_cancellation
            .as_ref()
            .is_some_and(PublicationReceiptOwner::fenced_before_admission)
        {
            PlaybackHandoffRejection::FreshRuntimeCancelled
        } else if Self::runtime_fence_changed(&self.status, old, fence)
            || self.status.consumed_command.load(Acquire) != fence.admitted_command_ticket
            || self.transport_fade.is_some()
            || self.stream_action_pending()
        {
            PlaybackHandoffRejection::RuntimeFenceChanged
        } else if let Some((old, cut)) = old.zip(next.tempo_cut())
            && old.playback.playing()
            && (!old.playback.contains_streamed_sources() || old.streams.is_some())
            && old.explicit_control_context
            && old.control_context_generation == cut.control_generation
        {
            let current = old.playback.performance_frame();
            if current <= cut.performance_frame {
                return current == cut.performance_frame;
            }
            PlaybackHandoffRejection::TempoCutMissed
        } else {
            PlaybackHandoffRejection::RuntimeFenceChanged
        };
        self.reject_scheduled_publication(reason)
    }
}
