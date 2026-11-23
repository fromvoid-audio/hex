//! Retain the predecessor through the audible end of already-admitted Session
//! actions. Only that graph renders while waiting. The normal continuity
//! planner still decides whether the same prepared insert can install.
use super::*;

impl Publication {
    pub(super) fn retained_insert_fence(&self, mut fence: FreshRuntimeFence) -> FreshRuntimeFence {
        if let Some(receipt) = self.fresh_runtime_cancellation.as_ref() {
            fence.route_state_generation = receipt.0.retained_route_generation();
        }
        fence
    }
}

impl Callback {
    /// Return false while the accepted action prefix still belongs to the
    /// unchanged predecessor. The receipt stays cancellable throughout.
    pub(super) fn device_insert_cut_ready(
        &self,
        next: &mut Publication,
    ) -> Result<bool, PlaybackHandoffRejection> {
        let PublicationPolicy::DeviceInsert { predecessor, fence } = next.policy else {
            return Ok(true);
        };
        self.retain_device_insert_cut(
            predecessor,
            fence,
            next.playback.main_output_latency_frames(),
            &mut next.scheduled_cut_frame,
            next.fresh_runtime_cancellation.as_ref(),
        )
    }

    /// A source-free fitting removal renders its predecessor until the
    /// stage-local ramp commits. The exact receipt stays pending throughout.
    /// Called only after the ordinary source and command fence has been checked.
    pub(super) fn advance_native_device_removal(
        &mut self,
        next: &mut Publication,
    ) -> Result<bool, PlaybackHandoffRejection> {
        use hex_engine::session_playback::NativeRemovalProgress;

        let rendering_revoked = self.native_effects_revoked();
        let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut) else {
            return Ok(true);
        };
        let source_free = |graph: &Publication| {
            !graph.playback.contains_streamed_sources()
                && graph
                    .streams
                    .as_ref()
                    .is_none_or(|streams| streams.permits_paused_monitor())
        };
        // A previously accepted Session action keeps its original render cut.
        // It must not cross a newly armed stage fade.
        let pending_action = (0..active.playback.session_route_count()).any(|index| {
            active
                .playback
                .session_route_status(index)
                .is_some_and(|route| route.pending.is_some())
        });
        if rendering_revoked || pending_action {
            return if next.playback.native_removal_pending() {
                Err(PlaybackHandoffRejection::RuntimeFenceChanged)
            } else {
                Ok(true)
            };
        }
        if !source_free(active) || !source_free(next) {
            if active.streams.is_some() && next.streams.is_some() {
                next.playback
                    .plan_native_removal_from(&active.playback)
                    .map_err(PlaybackHandoffRejection::from_error)?;
            }
            return Ok(true);
        }
        let progress = next
            .playback
            .advance_native_removal_from(&mut active.playback)
            .map_err(PlaybackHandoffRejection::from_error)?;
        if next
            .playback
            .pending_hosted_track_effect_deadline()
            .is_some_and(|deadline| active.playback.performance_frame() > deadline)
        {
            return Err(PlaybackHandoffRejection::RuntimeFenceChanged);
        }
        if progress == NativeRemovalProgress::Fading {
            return Ok(false);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some((binding, frames)) = next.playback.pending_foreign_main_pass() {
            let Some(delay) = self
                .native_sources
                .settled_main_pass_delay(binding, frames)
                .map_err(|_| PlaybackHandoffRejection::RuntimeFenceChanged)?
            else {
                return Ok(false);
            };
            if !next.playback.stage_foreign_main_pass_delay(binding, delay) {
                return Err(PlaybackHandoffRejection::RuntimeFenceChanged);
            }
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some((identity, binding)) = next.playback.pending_foreign_return_pass() {
            if !self
                .native_sources
                .settled_return_pass(identity, binding, active.playback.performance_frame())
                .map_err(|_| PlaybackHandoffRejection::RuntimeFenceChanged)?
            {
                return Ok(false);
            }
            if !next
                .playback
                .stage_foreign_return_pass_delay(&active.playback, identity, binding)
            {
                return Err(PlaybackHandoffRejection::RuntimeFenceChanged);
            }
        }
        Ok(true)
    }

    /// The stream planner proved the candidate window and the current render
    /// window. Arm the stage only now so a cold request keeps all old DSP.
    pub(super) fn advance_paged_native_removal(
        &mut self,
        next: &mut Publication,
    ) -> Result<bool, PlaybackHandoffRejection> {
        if !next.playback.native_removal_pending() {
            return Ok(true);
        }
        if next
            .fresh_runtime_cancellation
            .as_ref()
            .is_some_and(PublicationReceiptOwner::fenced_before_admission)
        {
            return Err(PlaybackHandoffRejection::FreshRuntimeCancelled);
        }
        if !self.device_insert_cut_ready(next)? {
            return Err(PlaybackHandoffRejection::RuntimeFenceChanged);
        }
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .ok_or(PlaybackHandoffRejection::RuntimeFenceChanged)?;
        next.playback
            .advance_planned_native_removal_from(&mut active.playback)
            .map(|progress| progress != hex_engine::session_playback::NativeRemovalProgress::Fading)
            .map_err(PlaybackHandoffRejection::from_error)
    }

    /// A page miss after start cannot leave the predecessor with a faded
    /// stage while waiting on an unbounded candidate. Return it for restoration.
    pub(super) fn defer_for_stream_pages(&mut self, mut next: Publication) {
        if !next.playback.native_removal_started() {
            self.deferred = Some(next);
            return;
        }
        if let Some(streams) = next.streams.as_mut() {
            streams.cancel_install(&mut self.domain_pages);
        }
        self.reject_publication(
            next.revision,
            PlaybackHandoffRejection::StreamedPagesNotReady,
        );
        self.return_rejected_graph(next);
    }

    /// The rejected candidate may restore only the exact active predecessor.
    pub(super) fn restore_refused_native_removal(&mut self, next: &mut Publication) {
        if let PublicationPolicy::DeviceInsert { predecessor, .. } = next.policy
            && self.recording_source() == Some(predecessor)
            && let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut)
        {
            next.playback
                .cancel_native_removal_from(&mut active.playback);
        }
    }

    /// Preserve the predecessor's already-admitted audible route cut before
    /// asking the Engine to classify the edit. A retained cut is an end
    /// boundary for the old output. It never seeds the successor.
    ///
    /// In linear resident playback after the pipeline fills, the donor at
    /// processing frame C emits source frame C - old latency. Priming the
    /// successor to that source origin would leave its processing clock at
    /// C - old latency + new latency. Session quantization and Arrangement
    /// automation use the processing coordinates. A command admitted against
    /// the primed coordinates could fall on a different grid. During playback
    /// a changed main clock takes the refusal/consent path with the predecessor
    /// still active. Matching main latency proceeds to the Engine's normal
    /// route and history validation.
    pub(super) fn retain_device_insert_cut(
        &self,
        predecessor: AppliedAudioSource,
        fence: FreshRuntimeFence,
        successor_latency: u32,
        scheduled_cut: &mut Option<u64>,
        receipt: Option<&PublicationReceiptOwner>,
    ) -> Result<bool, PlaybackHandoffRejection> {
        let stale = PlaybackHandoffRejection::RuntimeFenceChanged;
        let active = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .ok_or(stale)?;
        let retained_routes = receipt.map_or(fence.route_state_generation, |receipt| {
            receipt.0.retained_route_generation()
        });
        let actual = active.playback.session_route_state_generation();
        // With no intervening consumed command, only rendering an admitted
        // route action can change the installed graph's generation. Permit
        // that change only after this callback owner has retained its cut.
        let expected = FreshRuntimeFence {
            route_state_generation: if scheduled_cut.is_some() {
                actual
            } else {
                retained_routes
            },
            ..fence
        };
        if self.recording_source() != Some(predecessor)
            || Self::runtime_frontier_changed(&self.status, active.revision, actual, expected)
            || self.status.consumed_command.load(Acquire) != fence.admitted_command_ticket
            || self.status.closing.load(Acquire)
            || self.transport_fade.is_some()
            || self.stream_pending_play.is_some()
            || self.stream_action_pending()
        {
            return Err(stale);
        }
        let output_latency = active.playback.main_output_latency_frames().ok_or(stale)?;
        if !active.playback.playing() || output_latency == successor_latency {
            return Ok(true);
        }
        let current = active.playback.performance_frame();
        if scheduled_cut.is_some_and(|cut| current < cut) {
            return Ok(false);
        }
        let pending = (0..active.playback.session_route_count())
            .filter_map(|index| active.playback.session_route_status(index)?.pending)
            .map(|pending| pending.performance_frame)
            .max();
        let receipt = receipt.ok_or(stale)?;
        if scheduled_cut.is_some() {
            // The end is exclusive: the old renderer has emitted the sample
            // at the last admitted cut, including its output delay.
            if pending.is_some() || actual == retained_routes {
                return Err(stale);
            }
            receipt.record_retained_route_generation(actual);
            *scheduled_cut = None;
            return Ok(true);
        }
        if let Some(last) = pending
            && let Some(cut) = last
                .checked_add(u64::from(output_latency))
                .and_then(|frame| frame.checked_add(1))
            && cut > current
            && let Some(deadline) = receipt.device_insert_deadline()
        {
            // Only wait for an imminent quantized cut; keep the wider install
            // window for callback delivery, fades, and streamed page prewarm.
            const MAX_PENDING_ACTION_WAIT: std::time::Duration =
                std::time::Duration::from_millis(250);
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(MAX_PENDING_ACTION_WAIT);
            let available =
                remaining.as_nanos() * u128::from(active.playback.sample_rate()) / 1_000_000_000;
            if u128::from(cut - current) <= available {
                *scheduled_cut = Some(cut);
                return Ok(false);
            }
        }
        Ok(true)
    }
}
