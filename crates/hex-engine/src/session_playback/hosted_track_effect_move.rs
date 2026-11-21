//! Exact hosted effect cuts for two direct Resident audio rows.
//! Cross-track movement drains the old dry alignment before changing its owner.
//! A within-track reorder keeps the dry alignment and starts the edited chain fresh.
use super::{RouteOwner, SessionPlayback, device_move::same_clip};
use crate::{
    playback::{HostedMoveRequest, HostedTrackEffectMove},
    signal_graph::{FreshHistory, hosted_track_effect_move::HostedEffectClock},
};
use hex_lattice::{PreparedRuntimeInstance, ProcessorChainOwner};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct HostedEffectCut {
    pub(super) source: usize,
    pub(super) destination: usize,
    pub(super) request: HostedTrackEffectMove,
    pub(super) clock: HostedEffectClock,
}

impl SessionPlayback {
    pub(super) fn direct_hosted_effect_cut_from(&self, previous: &Self) -> Option<HostedEffectCut> {
        let HostedMoveRequest::Effect(request) = self.hosted_move? else {
            return None;
        };
        if !self.same_direct_hosted_effect_rows(previous) {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let source = old
            .tracks()
            .iter()
            .position(|track| track.canonical_track_id() == Some(request.donor))?;
        let destination = old
            .tracks()
            .iter()
            .position(|track| track.canonical_track_id() == Some(request.receiver))?;
        if source == destination {
            return None;
        }
        let [original] = old.tracks()[source].device_chain.as_ref() else {
            return None;
        };
        let [moved] = new.tracks()[destination].device_chain.as_ref() else {
            return None;
        };
        let processor = original.foreign_processor()?;
        let original_binding = processor.binding()?;
        let fresh = moved.foreign_processor()?;
        if !old.tracks()[destination].device_chain.is_empty()
            || !new.tracks()[source].device_chain.is_empty()
            || original.owner != ProcessorChainOwner::Track(request.donor.value())
            || moved.owner != ProcessorChainOwner::Track(request.receiver.value())
            || original.device_id != request.device.value()
            || moved.device_id != original.device_id
            || moved.device_type != original.device_type
            || moved.sample_rate != original.sample_rate
            || moved.matrix != original.matrix
            || original.bypassed
            || moved.bypassed
            || !original.automation.is_empty()
            || !moved.automation.is_empty()
            || original.modulation.is_some()
            || moved.modulation.is_some()
            || processor.is_instrument_profile()
            || processor.quantum_frames() != 256
            || processor.processor_latency_frames() != 0
            || processor.bridge_latency_frames() != 512
            || fresh.binding() != Some(request.binding)
            || original_binding == request.binding
            || original_binding.root != request.binding.root
            || original_binding.instance != original.device_id
            || request.binding.instance != moved.device_id
            || fresh.with_binding(original_binding).ok() != Some(processor)
            || !original
                .configuration
                .as_ref()
                .zip(moved.configuration.as_ref())
                .is_some_and(|(before, after)| before.processing_eq(after))
        {
            return None;
        }
        let (next, old) = self
            .compensation
            .as_ref()
            .zip(previous.compensation.as_ref())?;
        for runtime in [next, old] {
            let latency = runtime
                .graph
                .processor_latency(request.device.value(), None)?;
            if latency.upstream_frames != 0 || latency.stage_frames != 512 {
                return None;
            }
        }
        let clock = next.hosted_effect_clock_from(old, source, destination)?;
        Some(HostedEffectCut {
            source,
            destination,
            request,
            clock,
        })
    }

    pub(super) fn direct_hosted_effect_reorder_from(
        &self,
        previous: &Self,
    ) -> Option<(usize, FreshHistory)> {
        let HostedMoveRequest::Reorder(request) = self.hosted_move? else {
            return None;
        };
        if !self.same_direct_hosted_effect_rows(previous) {
            return None;
        }
        let old = previous.revision.arrangement().tracks();
        let new = self.revision.arrangement().tracks();
        let index = old
            .iter()
            .position(|track| track.canonical_track_id() == Some(request.track))?;
        let dry = 1 - index;
        if !old[dry].device_chain.is_empty() || !new[dry].device_chain.is_empty() {
            return None;
        }
        let ([first, second], [next_first, next_second]) = (
            old[index].device_chain.as_ref(),
            new[index].device_chain.as_ref(),
        ) else {
            return None;
        };
        let device = request.device.value();
        let (original, fresh, native, next_native) = if first.device_id == device {
            (first, next_second, second, next_first)
        } else {
            (second, next_first, first, next_second)
        };
        let owner = ProcessorChainOwner::Track(request.track.value());
        if original.device_id != device
            || native.device_id == device
            || !same_enabled_stage(original, fresh, owner)
            || !same_enabled_stage(native, next_native, owner)
            || native.foreign_processor().is_some()
            || next_native.foreign_processor().is_some()
            || native.algorithmic_latency_frames() != Some(0)
            || next_native.algorithmic_latency_frames() != Some(0)
        {
            return None;
        }
        let before = original.foreign_processor()?;
        let after = fresh.foreign_processor()?;
        if before.binding() != Some(request.previous_binding)
            || after.binding() != Some(request.binding)
            || request.previous_binding == request.binding
            || request.previous_binding.root != request.binding.root
            || before.is_instrument_profile()
            || before.quantum_frames() != 256
            || before.processor_latency_frames() != 0
            || before.bridge_latency_frames() != 512
            || after.with_binding(request.previous_binding).ok() != Some(before)
        {
            return None;
        }
        let (next, old) = self
            .compensation
            .as_ref()
            .zip(previous.compensation.as_ref())?;
        for runtime in [next, old] {
            let graph = &runtime.graph;
            let latency = graph.processor_latency(device, None)?;
            if !graph.independent_main_routes()
                || runtime.horizon() != 512
                || graph.main_input_arrival() != 512
                || graph.resident_main_alignment(index)?.1 != 512
                || graph.resident_main_alignment(dry)?.1 != 0
                || latency.upstream_frames != 0
                || latency.stage_frames != 512
            {
                return None;
            }
        }
        let history = FreshHistory::ChangedTrack {
            track: request.track,
            device,
            second_device: Some(native.device_id),
            cut_predecessor_pcm: false,
        };
        next.can_copy_from(old, Some(history))
            .then_some((index, history))
    }

    fn same_direct_hosted_effect_rows(&self, previous: &Self) -> bool {
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if !previous.playing
            || [self, previous].into_iter().any(|runtime| {
                let arrangement = runtime.revision.arrangement();
                arrangement.tracks().len() != 2
                    || !arrangement.main_device_chain.is_empty()
                    || !arrangement.return_buses().is_empty()
                    || !arrangement.return_feed_plan.is_empty()
                    || !arrangement.midi_routing.is_empty()
                    || runtime.revision.contains_streamed_sources()
                    || runtime.revision.gain_recording_track().is_some()
                    || runtime.loop_region().is_some()
                    || runtime.main_pass_delay.is_some()
                    || runtime.stopped_input.is_some()
                    || runtime.computer_keyboard.is_some()
                    || runtime.live_expression.is_some()
                    || !runtime.releases.is_empty()
                    || runtime.tail_output_active.iter().any(|active| *active)
                    || runtime
                        .return_effect_tails
                        .slots()
                        .iter()
                        .flatten()
                        .any(|tail| tail.output_active)
            })
            || old.sample_rate != new.sample_rate
            || old.tempo_map != new.tempo_map
            || old.master_gain != new.master_gain
        {
            return false;
        }
        old.tracks()
            .iter()
            .zip(new.tracks())
            .enumerate()
            .all(|(index, (before, after))| {
                let direct = |runtime: &Self, row: &crate::Track| {
                    runtime.routes.get(index).is_some_and(|route| {
                        matches!(route.owner, RouteOwner::Arrangement)
                            && route.pending.is_none()
                            && route.audio_release.is_none()
                    }) && runtime
                        .device_chains
                        .get(index)
                        .is_some_and(|chain| chain.len() == row.device_chain.len())
                        && row.instrument_id.is_none()
                        && row.source_id.is_none()
                        && row.midi_source_index.is_none()
                        && row.sends().is_empty()
                        && !row.has_gain_automation()
                        && !runtime.source_effects.has_chain(index)
                };
                let ([before_clip], [after_clip]) =
                    (before.clips.as_slice(), after.clips.as_slice())
                else {
                    return false;
                };
                before.canonical_track_id() == after.canonical_track_id()
                    && before.mix == after.mix
                    && direct(previous, before)
                    && direct(self, after)
                    && same_clip(before_clip, after_clip)
            })
    }
}

fn same_enabled_stage(
    old: &PreparedRuntimeInstance,
    new: &PreparedRuntimeInstance,
    owner: ProcessorChainOwner,
) -> bool {
    old.owner == owner
        && new.owner == owner
        && old.device_id == new.device_id
        && old.device_type == new.device_type
        && old.sample_rate == new.sample_rate
        && old.matrix == new.matrix
        && !old.bypassed
        && !new.bypassed
        && old.automation.is_empty()
        && new.automation.is_empty()
        && old.modulation.is_none()
        && new.modulation.is_none()
        && old
            .configuration
            .as_ref()
            .zip(new.configuration.as_ref())
            .is_some_and(|(before, after)| {
                before.processing_eq_on_callback(after)
                    && before.input_gain_millidb() == after.input_gain_millidb()
                    && before.output_gain_millidb() == after.output_gain_millidb()
            })
}
