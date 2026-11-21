//! A short PCM delay can keep the Main clock without retaining an old DSP.
use super::{RouteOwner, SessionPlayback};
use crate::Track;
use hex_lattice::NativeRemovalTransition;
use hex_project::ReturnId;

impl SessionPlayback {
    pub(crate) fn main_output_latency_frames(&self) -> u32 {
        self.revision.arrangement().main_latency_frames()
            + self
                .main_pass_delay
                .as_ref()
                .filter(|_| self.native_removal.is_none())
                .map_or(0, |delay| delay.latency_frames())
    }

    pub(super) fn short_main_pass_removal_from(
        &self,
        old: &Self,
    ) -> Option<NativeRemovalTransition> {
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        let [device] = before.main_device_chain.as_ref() else {
            return None;
        };
        let short_native = [
            hex_devices::overdrive::OVERDRIVE_TYPE_ID,
            hex_devices::saturator::SATURATOR_TYPE_ID,
            hex_devices::rattle::RATTLE_TYPE_ID,
        ]
        .contains(&device.device_type);
        let fixed_short_limiter = device.device_type == hex_devices::limiter::TYPE_ID
            && before.main_latency_frames()
                == before.sample_rate().saturating_mul(3).div_ceil(2_000);
        let explicit_off = matches!(after.main_device_chain.as_ref(), [off]
            if off.bypassed && off.definition.as_ref().is_some_and(|factory| factory.is_powered_off()));
        // Removal can keep the already-summed input from two dry tracks.
        // The separate hosted power contract still admits one live dry track.
        let track_limit = if explicit_off { 1 } else { 2 };
        if !(after.main_device_chain.is_empty() || explicit_off)
            || self.compensation.is_some()
            || !self.same_direct_pass_input(old, track_limit)
            || old.main_pass_delay.is_some()
            || old
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
            || old.tail_output_active.iter().any(|active| *active)
            || !old.releases.is_empty()
            || !(short_native || fixed_short_limiter || device.foreign_processor().is_some())
            || after.main_latency_frames() != 0
        {
            return None;
        }
        self.main_chain.main_pass_removal_from(&old.main_chain)
    }

    /// One or two unshifted resident dry routes already pay the complete
    /// 512-frame bridge delay. Keep only their aligned PCM after the Return
    /// finishes its post-effect fade and returns with the predecessor graph.
    pub(super) fn short_hosted_return_pass_from(&self, old: &Self) -> Option<(ReturnId, u32)> {
        const BRIDGE: u32 = 512;
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        let [bus] = before.return_buses() else {
            return None;
        };
        let [effect] = bus.device_chain.as_ref() else {
            return None;
        };
        let dry_routes = before.tracks().len();
        if !(1..=2).contains(&dry_routes)
            || after.tracks().len() != dry_routes
            || old.routes.len() != dry_routes
            || self.routes.len() != dry_routes
        {
            return None;
        }
        let [send] = before.tracks()[0].sends() else {
            return None;
        };
        let old_signal = old.compensation.as_ref()?;
        let after_graph = after.signal_graph.as_ref()?;
        let carries_only_dry = old_signal.resident_return_graph_is_passthrough()
            && old_signal
                .graph
                .return_latency(0)?
                .default_route_delay_frames
                == 0
            && after_graph.independent_main_routes()
            && after_graph.main_input_arrival() == 0;
        if !carries_only_dry
            || before.main_latency_frames() != BRIDGE
            || after.main_latency_frames() != 0
            || !before.main_device_chain.is_empty()
            || !after.main_device_chain.is_empty()
            || before.master_mix() != after.master_mix()
            || !before.return_feed_plan.is_empty()
            || !after.return_feed_plan.is_empty()
            || bus.muted
            || bus.gain == 0.0
            || effect.bypassed
            || effect.device_type != hex_lattice_contract::VST3_DEVICE_TYPE_ID
            || effect.foreign_processor().is_none()
            || effect.algorithmic_latency_frames() != Some(BRIDGE)
            || send.return_index != 0
            || send.gain == 0.0
            || before.tracks()[1..]
                .iter()
                .any(|track| !track.sends().is_empty())
            || after.tracks().iter().any(|track| !track.sends().is_empty())
            || !after.return_buses().is_empty()
            || self.compensation.is_some()
            || old.main_pass_delay.is_some()
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
            || !before.tracks().iter().zip(after.tracks()).enumerate().all(
                |(index, (left, right))| {
                    left.canonical_track_id().is_some()
                        && left.canonical_track_id() == right.canonical_track_id()
                        && left.mix == right.mix
                        && arrangement_pass_source_at(old, left, index)
                        && arrangement_pass_source_at(self, right, index)
                        && super::device_move::same_clip(&left.clips[0], &right.clips[0])
                },
            )
        {
            return None;
        }
        let origin = old.committed_return_pass_signal()?;
        if dry_routes == 2 {
            origin.committed_hosted_return_pair()?;
        } else {
            origin.committed_resident_main_alignment(0, BRIDGE)?;
        }
        Some((bus.id, BRIDGE))
    }

    pub(super) fn committed_return_main_pass(
        &self,
        latency: u32,
    ) -> Option<hex_lattice::MainPassDelay> {
        let signal = self.committed_return_pass_signal()?;
        match self.revision.arrangement().tracks().len() {
            1 => signal.committed_resident_main_alignment(0, latency),
            2 if latency == 512 => {
                let [(first, first_next), (second, second_next)] =
                    signal.committed_hosted_return_pair()?;
                let sum = std::array::from_fn::<_, 512, _>(|offset| {
                    let a = first[(first_next + offset) % 512];
                    let b = second[(second_next + offset) % 512];
                    [a[0] + b[0], a[1] + b[1]]
                });
                hex_lattice::MainPassDelay::copy_committed_ring(&sum, 0)
            }
            _ => None,
        }
    }

    /// Keep one unshifted Resident dry route and its exact static send when the
    /// sole hosted Return switches to a workerless copy of its input.
    pub(super) fn short_hosted_return_off_from(
        &self,
        old: &Self,
    ) -> Option<NativeRemovalTransition> {
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        let ([track], [next_track]) = (before.tracks(), after.tracks()) else {
            return None;
        };
        let ([bus], [next_bus]) = (before.return_buses(), after.return_buses()) else {
            return None;
        };
        let ([send], [next_send]) = (track.sends(), next_track.sends()) else {
            return None;
        };
        let ([chain], [next_chain]) = (old.return_chains.as_slice(), self.return_chains.as_slice())
        else {
            return None;
        };
        let origin = old.committed_return_pass_signal()?;
        let next_graph = after.signal_graph.as_ref()?;
        if before.main_latency_frames() != 512
            || after.main_latency_frames() != 0
            || !before.main_device_chain.is_empty()
            || !after.main_device_chain.is_empty()
            || before.master_mix() != after.master_mix()
            || !before.return_feed_plan.is_empty()
            || !after.return_feed_plan.is_empty()
            || self.compensation.is_some()
            || old.main_pass_delay.is_some()
            || old.stopped_input.is_some()
            || self.stopped_input.is_some()
            || old.return_fade.is_some()
            || old.live_gain.audible().is_some()
            || old.track_faders.first().is_none_or(|fader| fader.active())
            || old
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
            || old.tail_output_active.iter().any(|active| *active)
            || !old.releases.is_empty()
            || bus.id != next_bus.id
            || bus.muted
            || next_bus.muted
            || bus.gain <= 0.0
            || bus.gain != next_bus.gain
            || bus.matrix.is_some()
            || next_bus.matrix.is_some()
            || send.return_index != 0
            || next_send.return_index != 0
            || send.gain <= 0.0
            || send.gain != next_send.gain
            || origin.delays.latest_stamp()?.authored_clock_epoch != Some(old.authored_clock_epoch)
            || !origin.graph.direct_return_send(bus.id, send.gain)
            || origin.graph.return_latency(0)?.default_route_delay_frames != 0
            || !next_graph.direct_return_send(bus.id, send.gain)
            || next_graph.needs_runtime()
            || track.canonical_track_id().is_none()
            || track.canonical_track_id() != next_track.canonical_track_id()
            || track.mix != next_track.mix
            || !same_arrangement_pass_source(old, track)
            || !same_arrangement_pass_source(self, next_track)
            || !super::device_move::same_clip(&track.clips[0], &next_track.clips[0])
        {
            return None;
        }
        next_chain.return_power_off_from(chain, bus.id.value())
    }

    pub(super) fn committed_return_off_main_pass(
        &self,
        owner: hex_lattice::ProcessorChainOwner,
    ) -> Option<hex_lattice::MainPassDelay> {
        let arrangement = self.revision.arrangement();
        let [track] = arrangement.tracks() else {
            return None;
        };
        let [bus] = arrangement.return_buses() else {
            return None;
        };
        let [send] = track.sends() else { return None };
        if owner != hex_lattice::ProcessorChainOwner::Return(bus.id.value()) {
            return None;
        }
        let signal = self.committed_return_pass_signal()?;
        if signal.delays.latest_stamp()?.authored_clock_epoch != Some(self.authored_clock_epoch) {
            return None;
        }
        let mut pass = signal.committed_resident_main_alignment(0, 512)?;
        // Match the renderer's multiply/multiply/add order. The same fader
        // produced both branches and neither the send nor Return output adds delay.
        pass.map_frames(|frame| frame.map(|sample| sample + (sample * send.gain) * bus.gain));
        Some(pass)
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

    /// No upstream history changes meaning when the final, short delay moves
    /// from the outgoing Main stage to the passive output of the same graph.
    fn same_direct_pass_input(&self, old: &Self, track_limit: usize) -> bool {
        let direct_input = |playback: &Self| {
            let arrangement = playback.revision.arrangement();
            let count = arrangement.tracks().len();
            (1..=track_limit).contains(&count)
                && playback.routes.len() == count
                && arrangement.return_buses().is_empty()
                && arrangement.signal_graph.as_ref().is_some_and(|graph| {
                    graph.main_input_arrival() == 0
                        && graph.independent_main_routes()
                        && (0..count).all(|index| {
                            graph
                                .resident_main_alignment(index)
                                .is_some_and(|(_, arrival)| arrival == 0)
                        })
                })
        };
        let before = old.revision.arrangement();
        let after = self.revision.arrangement();
        if !direct_input(self)
            || !direct_input(old)
            || before.tracks().len() != after.tracks().len()
            || before.master_mix() != after.master_mix()
        {
            return false;
        }
        before
            .tracks()
            .iter()
            .zip(after.tracks())
            .enumerate()
            .all(|(index, (left, right))| {
                left.canonical_track_id() == right.canonical_track_id()
                    && left.mix == right.mix
                    && left.source_mix == right.source_mix
                    && left.source_input_enabled() == right.source_input_enabled()
                    && left.audio_content_enabled() == right.audio_content_enabled()
                    && left.sends().is_empty()
                    && right.sends().is_empty()
                    && arrangement_pass_source_at(old, left, index)
                    && arrangement_pass_source_at(self, right, index)
                    && super::device_move::same_clip(&left.clips[0], &right.clips[0])
            })
    }
}

fn same_arrangement_pass_source(playback: &SessionPlayback, track: &Track) -> bool {
    playback.routes.len() == 1 && arrangement_pass_source_at(playback, track, 0)
}

fn arrangement_pass_source_at(playback: &SessionPlayback, track: &Track, index: usize) -> bool {
    let Some(route) = playback.routes.get(index) else {
        return false;
    };
    !playback.revision.contains_streamed_sources()
        && matches!(route.owner, RouteOwner::Arrangement)
        && route.pending.is_none()
        && route.audio_release.is_none()
        && playback.revision.gain_recording_track().is_none()
        && !playback.source_effects.has_chain(index)
        && track.clips.len() == 1
        && track.device_chain.is_empty()
        && track.instrument_id.is_none()
        && track.source_id.is_none()
        && track.midi_source_index.is_none()
        && !track.has_gain_automation()
}
