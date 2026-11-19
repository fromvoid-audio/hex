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
            let remaining = deadline.saturating_duration_since(Instant::now());
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
