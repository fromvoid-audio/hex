//! A verified insert retains the constructor's own source, page, and runtime
//! custody. Consent authorizes a cut only on the exact installed output.
use super::*;
use crate::candidate_custody::{QueuedEligibleCandidate, StreamInstall};

impl Callback {
    pub(super) fn abort_blocked_verified_insert(&mut self) {
        let held = matches!(self.active.as_ref(), Some(ActivePublication::Eligible(old))
            if old.runtime().is_some_and(hex_engine::ClaimedAudioCandidateRuntime::device_insert_cut_pending));
        if held && let Some(next) = self.deferred_eligible.take() {
            self.return_verified_insert(&next, Some(PlaybackHandoffRejection::RuntimeFenceChanged));
            self.deferred_eligible = Some(next);
        }
    }

    pub(super) fn verified_device_insert_ready(
        &mut self,
        next: &mut QueuedEligibleCandidate,
    ) -> Result<bool, PlaybackHandoffRejection> {
        let stale = PlaybackHandoffRejection::RuntimeFenceChanged;
        let cancelled = PlaybackHandoffRejection::FreshRuntimeCancelled;
        let unavailable = PlaybackHandoffRejection::StreamedPagesNotReady;
        let receipt = next.device_insert().ok_or(stale)?;
        if receipt.fenced_before_admission()
            || !matches!(
                receipt.0.state(),
                PublicationReceiptState::Pending | PublicationReceiptState::Applying
            )
        {
            return Err(cancelled);
        }
        let basis = receipt.0.device_insert_basis().ok_or(stale)?;
        let (predecessor, fence) = basis.predecessor_fence();
        if basis.approved.is_none() {
            let mut cut = next.insert_cut;
            let ready = self.retain_device_insert_cut(
                predecessor,
                fence,
                next.insert_latency().ok_or(stale)?,
                &mut cut,
                Some(receipt),
            );
            next.insert_cut = cut;
            return ready;
        }
        let active = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .ok_or(stale)?;
        if self.recording_source() != Some(predecessor)
            || !basis.verified_cut_is_current(
                &self.status,
                active.revision,
                active.playback.session_route_state_generation(),
                fence.route_state_generation,
            )
            || self.transport_fade.is_some()
            || self.stream_pending_play.is_some()
            || self.stream_action_pending()
            || self.output_transport.is_some()
            || (0..active.playback.session_route_count()).any(|index| {
                active
                    .playback
                    .session_route_status(index)
                    .is_some_and(|route| route.pending.is_some())
            })
        {
            return Err(stale);
        }
        let Some(ActivePublication::Eligible(old)) = self.active.as_mut() else {
            return Err(stale);
        };
        let runtime = old.runtime().ok_or(stale)?;
        if runtime.device_insert_cut_pending() {
            if runtime.device_insert_held() {
                // `prepare_install` proves actual current-cut readiness and
                // the same protected role before inheriting any owner.
                return Ok(true);
            }
            if self.status.stream_buffering.load(Acquire)
                || self.status.stream_source_changed.load(Acquire)
                || self.status.stream_read_failed.load(Acquire)
            {
                return Err(unavailable);
            }
            return Ok(false);
        }
        let fade = runtime.device_insert_fade_frames().ok_or(stale)?;
        let pins = old
            .stream_view()
            .reserve_outgoing_fade(&mut self.domain_pages, fade as usize)
            .map_err(|error| match error {
                crate::candidate_custody::QuantumFailure::Capacity => {
                    PlaybackHandoffRejection::StreamedPageCapacity
                }
                _ => unavailable,
            })?;
        let Some(pins) = pins else {
            next.cancel_stream_readiness(&mut self.domain_pages);
            return Ok(false);
        };
        self.device_insert_fade_pins = Some(pins);
        let ready = next.protect_device_insert_window(
            old,
            &mut StreamInstall {
                domains: &mut self.domain_pages,
                pending_play: false,
                command_prefix: self.stream_prefix,
                frames: 256,
                fenced_restart: false,
                foreign_continuation: false,
                confirmed_fade: Some(fade),
                tempo_anchor: None,
            },
        )?;
        if !ready {
            self.release_device_insert_fade_pins();
            return Ok(false);
        }
        let receipt = next.device_insert().ok_or(stale)?;
        let basis = receipt.0.device_insert_basis().ok_or(stale)?;
        let active = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .ok_or(stale)?;
        if !receipt.begin_confirmed_fade() {
            return Err(cancelled);
        }
        if !basis.verified_cut_is_current(
            &self.status,
            active.revision,
            active.playback.session_route_state_generation(),
            fence.route_state_generation,
        ) {
            return Err(stale);
        }
        if let Some(ActivePublication::Eligible(old)) = self.active.as_mut()
            && old
                .runtime_mut()
                .is_some_and(hex_engine::ClaimedAudioCandidateRuntime::begin_device_insert_cut)
        {
            Ok(false)
        } else {
            Err(stale)
        }
    }

    pub(super) fn return_verified_insert(
        &mut self,
        next: &QueuedEligibleCandidate,
        rejection: Option<PlaybackHandoffRejection>,
    ) {
        let Some(receipt) = next.device_insert() else {
            return;
        };
        receipt.returned(rejection);
        if let Some(predecessor) = receipt
            .0
            .device_insert_basis()
            .and_then(|basis| basis.approved)
            .map(|offer| offer.predecessor())
        {
            self.release_device_insert_fade_pins();
            if !self.status.closing.load(Acquire)
                && self.recording_source() == Some(predecessor)
                && let Some(ActivePublication::Eligible(active)) = self.active.as_mut()
                && let Some(runtime) = active.runtime_mut()
            {
                runtime.release_device_insert_cut();
            }
        }
    }
}
