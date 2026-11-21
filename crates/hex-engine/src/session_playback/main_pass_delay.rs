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
        if !(after.main_device_chain.is_empty() || explicit_off)
            || self.compensation.is_some()
            || !self.same_direct_pass_input(old)
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
        if before.tracks()[0].mix != after.tracks()[0].mix {
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
    pub(super) fn same_direct_pass_input(&self, old: &Self) -> bool {
        let one_track = |playback: &Self| {
            let arrangement = playback.revision.arrangement();
            let [track] = arrangement.tracks() else {
                return false;
            };
            arrangement.return_buses().is_empty()
                && arrangement.signal_graph.as_ref().is_some_and(|graph| {
                    graph.main_input_arrival() == 0
                        && graph.independent_main_routes()
                        && graph
                            .resident_main_alignment(0)
                            .is_some_and(|(_, arrival)| arrival == 0)
                })
                && track.sends().is_empty()
                && same_arrangement_pass_source(playback, track)
        };
        if !one_track(self) || !one_track(old) {
            return false;
        }
        let left = &self.revision.arrangement().tracks()[0];
        let right = &old.revision.arrangement().tracks()[0];
        left.canonical_track_id() == right.canonical_track_id()
            && super::device_move::same_clip(&left.clips[0], &right.clips[0])
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
