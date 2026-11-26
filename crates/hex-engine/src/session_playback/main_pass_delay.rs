//! General pass-level latency compensation floor retained at Main until Stop
//! and next Play per `docs/design/live-repatching-brief.md` rule 10.
//!
//! When a live device removal, power-off, or return removal reduces the
//! compiled graph's Main output latency (`after < before`), a passive
//! `MainPassDelay` retains only the needed compensation delay delta
//! (`old.main_output_latency_frames() - after_main_latency`, up to
//! `MAX_MAIN_PASS_DELAY_FRAMES`) so output timing remains rock-steady across
//! arbitrary multi-track, group, and return topologies with mixed native and
//! VST3 chains. Retired DSP still leaves the callback immediately.
use super::SessionPlayback;
use crate::Track;
use hex_lattice::{MAX_MAIN_PASS_DELAY_FRAMES, MainPassDelay, NativeRemovalTransition};
use hex_project::ReturnId;

impl SessionPlayback {
    pub(crate) fn main_output_latency_frames(&self) -> u32 {
        self.revision.arrangement().main_latency_frames()
            + self
                .main_pass_delay
                .as_ref()
                .filter(|_| {
                    self.native_removal
                        .is_none_or(|pending| pending.is_mute_fade())
                })
                .map_or(0, |delay| delay.latency_frames())
    }

    /// Detect a latency-reducing Main device removal or power-off in a single-
    /// or multi-device Main chain across arbitrary track/group/return
    /// topologies while keeping the pass output latency floor unchanged until
    /// Stop. Rejects simultaneous unrelated track mix/clip edits so delayed
    /// dry PCM cannot silently apply stale mix levels.
    pub(super) fn short_main_pass_removal_from(
        &self,
        old: &Self,
    ) -> Option<NativeRemovalTransition> {
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        let before_latency = before.main_latency_frames();
        let after_latency = after.main_latency_frames();
        if after_latency >= before_latency {
            return None;
        }
        let needed_floor = old
            .main_output_latency_frames()
            .checked_sub(after_latency)?;
        if needed_floor == 0 || needed_floor > MAX_MAIN_PASS_DELAY_FRAMES {
            return None;
        }
        if self.revision.contains_streamed_sources()
            || old.revision.contains_streamed_sources()
            || !self.upstream_matches_for_main_pass_cut(old)
            || old
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
            || old.tail_output_active.iter().any(|active| *active)
            || !old.releases.is_empty()
        {
            return None;
        }
        self.main_chain.main_pass_removal_from(&old.main_chain)
    }

    /// Detect a latency-reducing Return bus removal across arbitrary N-track,
    /// group, and multi-return topologies (native or VST3, any latency up to
    /// `MAX_MAIN_PASS_DELAY_FRAMES`). Returns `(removed_return_id, needed_pass_delay)`.
    pub(super) fn short_hosted_return_pass_from(&self, old: &Self) -> Option<(ReturnId, u32)> {
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        let before_latency = before.main_latency_frames();
        let after_latency = after.main_latency_frames();
        if after_latency >= before_latency {
            return None;
        }
        let needed_floor = old
            .main_output_latency_frames()
            .checked_sub(after_latency)?;
        if needed_floor == 0 || needed_floor > MAX_MAIN_PASS_DELAY_FRAMES {
            return None;
        }
        if self.revision.contains_streamed_sources()
            || old.revision.contains_streamed_sources()
            || before.master_mix() != after.master_mix()
            || old.stopped_input.is_some()
            || self.stopped_input.is_some()
            || old
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
            || old.tail_output_active.iter().any(|active| *active)
            || !old.releases.is_empty()
        {
            return None;
        }
        if before.return_buses().len().checked_sub(1) != Some(after.return_buses().len()) {
            return None;
        }
        let mut remaining = after.return_buses().iter().map(|bus| bus.id);
        let removed = before
            .return_buses()
            .iter()
            .find(|bus| remaining.next() != Some(bus.id))?;
        if !before
            .return_buses()
            .iter()
            .filter(|bus| bus.id != removed.id)
            .map(|bus| bus.id)
            .eq(after.return_buses().iter().map(|bus| bus.id))
            || !before
                .tracks()
                .iter()
                .map(|track| track.canonical_track_id())
                .eq(after
                    .tracks()
                    .iter()
                    .map(|track| track.canonical_track_id()))
        {
            return None;
        }
        // Reject simultaneous unrelated track mix or Main chain edits during a Return removal.
        if self.main_chain.len() != old.main_chain.len()
            || !self
                .main_chain
                .configuration_matches_range(&old.main_chain, 0..old.main_chain.len())
            || before
                .tracks()
                .iter()
                .zip(after.tracks())
                .any(|(left, right)| left.mix != right.mix)
        {
            return None;
        }
        Some((removed.id, needed_floor))
    }

    /// Build the passive `MainPassDelay` for a Return removal. When all tracks
    /// have committed direct-Main alignment rings at the exact reduced latency,
    /// sums them bit-for-bit for click-free dry continuity; otherwise retains
    /// the general pass-level compensation delay floor until Stop.
    pub(super) fn committed_return_main_pass(&self, latency: u32) -> Option<MainPassDelay> {
        let len = usize::try_from(latency).ok()?;
        let tracks = self.revision.arrangement().tracks().len();
        if self.main_pass_delay.is_none()
            && let Some(signal) = self.committed_return_pass_signal()
            && let Some(summed) = signal.committed_resident_main_sum(tracks, latency)
        {
            return Some(summed);
        }
        if let Some(existing) = self.main_pass_delay {
            let existing_frames = existing.latency_frames();
            return if latency == existing_frames {
                Some(existing)
            } else if latency > existing_frames {
                existing.with_additional_delay((latency - existing_frames) as usize)
            } else {
                existing.shrink_to(len)
            };
        }
        MainPassDelay::zeroed(len)
    }

    /// Detect a latency-reducing device removal or genuine power-off on ANY
    /// Return or Track chain across arbitrary multi-track/group/return
    /// topologies with mixed native and VST3 chains.
    pub(super) fn short_hosted_return_off_from(
        &self,
        old: &Self,
    ) -> Option<NativeRemovalTransition> {
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        let before_latency = before.main_latency_frames();
        let after_latency = after.main_latency_frames();
        if after_latency >= before_latency {
            return None;
        }
        let needed_floor = old
            .main_output_latency_frames()
            .checked_sub(after_latency)?;
        if needed_floor == 0 || needed_floor > MAX_MAIN_PASS_DELAY_FRAMES {
            return None;
        }
        if self.revision.contains_streamed_sources()
            || old.revision.contains_streamed_sources()
            || before.master_mix() != after.master_mix()
            || old.stopped_input.is_some()
            || self.stopped_input.is_some()
            || old.return_fade.is_some()
            || old.live_gain.audible().is_some()
            || old.track_faders[..old.routes.len()]
                .iter()
                .any(|fader| fader.active())
            || old
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
            || old.tail_output_active.iter().any(|active| *active)
            || !old.releases.is_empty()
            || !before
                .tracks()
                .iter()
                .map(|track| track.canonical_track_id())
                .eq(after
                    .tracks()
                    .iter()
                    .map(|track| track.canonical_track_id()))
            || !before
                .return_buses()
                .iter()
                .map(|bus| bus.id)
                .eq(after.return_buses().iter().map(|bus| bus.id))
            || self.main_chain.len() != old.main_chain.len()
            || !self
                .main_chain
                .configuration_matches_range(&old.main_chain, 0..old.main_chain.len())
            || before
                .tracks()
                .iter()
                .zip(after.tracks())
                .any(|(left, right)| left.mix != right.mix)
        {
            return None;
        }

        let mut transition: Option<NativeRemovalTransition> = None;

        // Check Return chains for a single latency-reducing removal or power-off.
        for ((old_bus, new_bus), (old_chain, new_chain)) in before
            .return_buses()
            .iter()
            .zip(after.return_buses())
            .zip(old.return_chains.iter().zip(&self.return_chains))
        {
            if old_bus.muted != new_bus.muted
                || old_bus.gain != new_bus.gain
                || old_bus.matrix != new_bus.matrix
            {
                return None;
            }
            if new_chain.len() == old_chain.len()
                && new_chain.configuration_matches_range(old_chain, 0..old_chain.len())
            {
                continue;
            }
            let found = new_chain.return_power_off_from(old_chain, old_bus.id.value())?;
            if transition.replace(found).is_some() {
                return None;
            }
        }

        // Check Track/Group chains for a single latency-reducing removal or power-off.
        for ((old_track, _new_track), (old_chain, new_chain)) in before
            .tracks()
            .iter()
            .zip(after.tracks())
            .zip(old.device_chains.iter().zip(&self.device_chains))
        {
            if new_chain.len() == old_chain.len()
                && new_chain.configuration_matches_range(old_chain, 0..old_chain.len())
            {
                continue;
            }
            let track_id = old_track.canonical_track_id()?.value();
            let found = new_chain.track_pass_removal_from(old_chain, track_id)?;
            if transition.replace(found).is_some() {
                return None;
            }
        }

        transition
    }

    /// Build the passive `MainPassDelay` when a hosted Return switches to Off
    /// or a Return/Track stage retires. Preserves exact dry+return committed
    /// ring sum on the classic single-track direct-send topology (at any
    /// latency up to `MAX_MAIN_PASS_DELAY_FRAMES`) and uses the general pass
    /// latency floor for arbitrary multi-track/group/return topologies.
    pub(super) fn committed_return_off_main_pass(
        &self,
        owner: hex_lattice::ProcessorChainOwner,
        latency: u32,
    ) -> Option<MainPassDelay> {
        let arrangement = self.revision.arrangement();
        if self.main_pass_delay.is_none()
            && let [track] = arrangement.tracks()
            && let [bus] = arrangement.return_buses()
            && let [send] = track.sends()
            && owner == hex_lattice::ProcessorChainOwner::Return(bus.id.value())
            && !bus.muted
            && bus.gain > 0.0
            && bus.matrix.is_none()
            && send.return_index == 0
            && send.gain > 0.0
            && let Some(signal) = self.committed_return_pass_signal()
            && signal
                .delays
                .latest_stamp()
                .is_some_and(|stamp| stamp.authored_clock_epoch == Some(self.authored_clock_epoch))
            && signal.graph.direct_return_send(bus.id, send.gain)
            && signal
                .graph
                .return_latency(0)
                .is_some_and(|info| info.default_route_delay_frames == 0)
            && let Some(mut pass) = signal.committed_resident_main_alignment(0, latency)
        {
            // Match the renderer's multiply/multiply/add order.
            pass.map_frames(|frame| frame.map(|sample| sample + (sample * send.gain) * bus.gain));
            return Some(pass);
        }
        self.committed_return_main_pass(latency)
    }

    fn committed_return_pass_signal(&self) -> Option<&crate::signal_graph::SignalRuntime> {
        let signal = self.compensation.as_ref()?;
        let stamp = signal.delays.latest_stamp()?;
        if !stamp.transport_is_running
            || stamp.performance_frame.checked_add(1) != Some(self.performance_frame)
        {
            return None;
        }
        Some(signal)
    }

    /// Verify that upstream tracks and return buses did not simultaneously
    /// change their topology, mix, or chains during a Main latency reduction.
    fn upstream_matches_for_main_pass_cut(&self, old: &Self) -> bool {
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        if before.tracks().len() != after.tracks().len()
            || before.return_buses().len() != after.return_buses().len()
            || before.master_mix() != after.master_mix()
            || old.routes.len() != self.routes.len()
        {
            return false;
        }
        let tracks_unchanged =
            before
                .tracks()
                .iter()
                .zip(after.tracks())
                .enumerate()
                .all(|(index, (left, right))| {
                    left.canonical_track_id() == right.canonical_track_id()
                        && left.mix == right.mix
                        && left.source_input_enabled() == right.source_input_enabled()
                        && left.audio_content_enabled() == right.audio_content_enabled()
                        && left.sends().len() == right.sends().len()
                        && left.sends().iter().zip(right.sends()).all(|(a, b)| {
                            a.return_index == b.return_index && a.gain.to_bits() == b.gain.to_bits()
                        })
                        && left.clips.len() == right.clips.len()
                        && left
                            .clips
                            .iter()
                            .zip(&right.clips)
                            .all(|(a, b)| super::device_move::same_clip(a, b))
                        && self
                            .device_chains
                            .get(index)
                            .zip(old.device_chains.get(index))
                            .is_some_and(|(next, prev)| {
                                next.len() == prev.len()
                                    && next.configuration_matches_range(prev, 0..prev.len())
                            })
                        && track_route_unchanged(old, self, left, right, index)
                });
        let returns_unchanged = before
            .return_buses()
            .iter()
            .zip(after.return_buses())
            .enumerate()
            .all(|(index, (left, right))| {
                left.id == right.id
                    && left.muted == right.muted
                    && left.gain == right.gain
                    && left.matrix == right.matrix
                    && self
                        .return_chains
                        .get(index)
                        .zip(old.return_chains.get(index))
                        .is_some_and(|(next, prev)| {
                            next.len() == prev.len()
                                && next.configuration_matches_range(prev, 0..prev.len())
                        })
            });
        tracks_unchanged && returns_unchanged
    }
}

fn track_route_unchanged(
    old: &SessionPlayback,
    next: &SessionPlayback,
    _left: &Track,
    _right: &Track,
    index: usize,
) -> bool {
    let Some((old_route, next_route)) = old.routes.get(index).zip(next.routes.get(index)) else {
        return false;
    };
    old_route.id == next_route.id
        && old_route.pending.is_none()
        && next_route.pending.is_none()
        && old_route.audio_release.is_none()
        && next_route.audio_release.is_none()
        && old.revision.gain_recording_track().is_none()
        && next.revision.gain_recording_track().is_none()
}
