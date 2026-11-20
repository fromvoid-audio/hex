//! A short PCM delay can keep the Main clock without retaining an old DSP.
use super::{RouteOwner, SessionPlayback};
use crate::Track;
use hex_lattice::NativeRemovalTransition;

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
        if !after.main_device_chain.is_empty()
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

    /// No upstream history changes meaning when the final, short delay moves
    /// from the outgoing Main stage to the passive output of the same graph.
    pub(super) fn same_direct_pass_input(&self, old: &Self) -> bool {
        let one_track = |playback: &Self| {
            let arrangement = playback.revision.arrangement();
            let [track] = arrangement.tracks() else {
                return false;
            };
            let [route] = playback.routes.as_slice() else {
                return false;
            };
            !playback.revision.contains_streamed_sources()
                && arrangement.return_buses().is_empty()
                && arrangement.signal_graph.as_ref().is_some_and(|graph| {
                    graph.main_input_arrival() == 0
                        && graph.independent_main_routes()
                        && graph
                            .resident_main_alignment(0)
                            .is_some_and(|(_, arrival)| arrival == 0)
                })
                && matches!(route.owner, RouteOwner::Arrangement)
                && route.pending.is_none()
                && route.audio_release.is_none()
                && playback.revision.gain_recording_track().is_none()
                && !playback.source_effects.has_chain(0)
                && simple_track(track)
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

fn simple_track(track: &Track) -> bool {
    track.clips.len() == 1
        && track.device_chain.is_empty()
        && track.sends.is_empty()
        && track.instrument_id.is_none()
        && track.source_id.is_none()
        && track.midi_source_index.is_none()
        && !track.has_gain_automation()
}
