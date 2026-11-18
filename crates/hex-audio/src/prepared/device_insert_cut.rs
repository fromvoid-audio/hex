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
        let stale = PlaybackHandoffRejection::RuntimeFenceChanged;
        let active = self.active.as_ref().and_then(ActivePublication::legacy);
        let evidence = next.retained_insert_fence(fence);
        let active = active.ok_or(stale)?;
        let actual = active.playback.session_route_state_generation();
        // With no intervening consumed command, only rendering an admitted
        // route action can change the installed graph's generation. Permit
        // that change only after this callback owner has retained its cut.
        let expected = FreshRuntimeFence {
            route_state_generation: if next.scheduled_cut_frame.is_some() {
                actual
            } else {
                evidence.route_state_generation
            },
            ..evidence
        };
        if self.recording_source() != Some(predecessor)
            || Self::runtime_fence_changed(&self.status, Some(active), expected)
            || self.status.consumed_command.load(Acquire) != fence.admitted_command_ticket
            || self.status.closing.load(Acquire)
            || self.transport_fade.is_some()
            || self.stream_pending_play.is_some()
            || self.stream_action_pending()
        {
            return Err(stale);
        }
        if !active.playback.playing()
            || active.playback.main_output_latency_frames()
                == next.playback.main_output_latency_frames()
        {
            return Ok(true);
        }
        let current = active.playback.performance_frame();
        if next.scheduled_cut_frame.is_some_and(|cut| current < cut) {
            return Ok(false);
        }
        let pending = (0..active.playback.session_route_count())
            .filter_map(|index| active.playback.session_route_status(index)?.pending)
            .map(|pending| pending.performance_frame)
            .max();
        let receipt = next.fresh_runtime_cancellation.as_ref().ok_or(stale)?;
        if next.scheduled_cut_frame.is_some() {
            // The end is exclusive: the old renderer has emitted the sample
            // at the last admitted cut, including its output delay.
            if pending.is_some() || actual == evidence.route_state_generation {
                return Err(stale);
            }
            receipt.record_retained_route_generation(actual);
            next.scheduled_cut_frame = None;
            return Ok(true);
        }
        if let Some(last) = pending
            && let Some(cut) = last
                .checked_add(u64::from(active.playback.main_output_latency_frames()))
                .and_then(|frame| frame.checked_add(1))
            && cut > current
            && let Some(deadline) = receipt.device_insert_deadline()
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let available =
                remaining.as_nanos() * u128::from(active.playback.sample_rate()) / 1_000_000_000;
            if u128::from(cut - current) <= available {
                next.scheduled_cut_frame = Some(cut);
                return Ok(false);
            }
        }
        Ok(true)
    }
}
