//! One Resident Arrangement leaf enters an existing root Group at zero latency.
//! Classification borrows bounded prepared inventories only. Full handoff preview
//! still fences runtime/tails before the child's existing post-device fader moves.
//! The Group and Main faders never participate, and compensation copy rules stay
//! unchanged: zero-delay graphs consume the current transport stamp at the cut.
use super::{RouteOwner, SessionPlayback};
use crate::signal_graph::TrackOutput;

impl SessionPlayback {
    pub(super) fn resident_group_entry_from(&self, previous: &Self) -> Option<usize> {
        for playback in [self, previous] {
            if playback.revision.contains_streamed_sources()
                || playback.foreign_runtime_bindings().next().is_some()
                || playback.revision.scalar_automation.is_some()
                || playback.revision.native_parameter_input_target().is_some()
                || playback.revision.gain_recording_track().is_some()
                || playback.routes.iter().any(|route| {
                    !matches!(route.owner, RouteOwner::Arrangement)
                        || route.pending.is_some()
                        || route.audio_release.is_some()
                })
            {
                return None;
            }
            let graph = &playback.compensation.as_ref()?.graph;
            if !graph.supports_closed_group_stream()
                || !graph.supports_closed_compensated_audio_groups()
                || playback.revision.arrangement().main_latency_frames() != 0
            {
                return None;
            }
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        if old.sample_rate != new.sample_rate
            || old.tempo_map != new.tempo_map
            || old.loop_region != new.loop_region
            || old.master_gain != new.master_gain
            || !old.midi_routing.is_empty()
            || !new.midi_routing.is_empty()
            || self.main_chain.len() != previous.main_chain.len()
            || !self
                .main_chain
                .configuration_matches_range(&previous.main_chain, 0..previous.main_chain.len())
            || new.main_device_chain.len() != self.main_chain.len()
            || old.main_device_chain.len() != previous.main_chain.len()
            || !same_devices(&old.main_device_chain, &new.main_device_chain)
            || old.tracks().len() != new.tracks().len()
        {
            return None;
        }
        let old_graph = &previous.compensation.as_ref()?.graph;
        let new_graph = &self.compensation.as_ref()?.graph;
        let mut changed = None;
        for (index, (old, new)) in old.tracks().iter().zip(new.tracks()).enumerate() {
            let old_chain = previous.device_chains.get(index)?;
            let new_chain = self.device_chains.get(index)?;
            if old.canonical_track_id() != new.canonical_track_id()
                || old.mix != new.mix
                || old.source_mix != new.source_mix
                || old.source_input_enabled != new.source_input_enabled
                || old.audio_content_enabled != new.audio_content_enabled
                || old.clips.len() != new.clips.len()
                || !old
                    .clips
                    .iter()
                    .zip(&new.clips)
                    .all(|(a, b)| same_clip(a, b))
                || !same_devices(&old.device_chain, &new.device_chain)
                || old_chain.len() != new_chain.len()
                || !new_chain.configuration_matches_range(old_chain, 0..old_chain.len())
            {
                return None;
            }
            for (track, chain) in [(old, old_chain), (new, new_chain)] {
                if track.instrument_id.is_some()
                    || track.source_id.is_some()
                    || track.source_receiver_index.is_some()
                    || track.midi_source_index.is_some()
                    || track.has_gain_automation()
                    || !track.sends.is_empty()
                    || track.device_chain.len() != chain.len()
                {
                    return None;
                }
            }
            let a = old_graph.tracks.get(index)?;
            let b = new_graph.tracks.get(index)?;
            if a.id != b.id || a.id != old.canonical_track_id() {
                return None;
            }
            match (a.output, b.output) {
                (TrackOutput::Main { .. }, TrackOutput::Main { .. }) => {}
                (TrackOutput::Group { target: a, .. }, TrackOutput::Group { target: b, .. }) if a == b => {}
                (TrackOutput::Main { .. }, TrackOutput::Group { target, .. }) => {
                    if changed.replace(index).is_some()
                        || old.clips.is_empty()
                        || !matches!(old_graph.tracks.get(target)?.output, TrackOutput::Main { .. })
                        || !matches!(new_graph.tracks.get(target)?.output, TrackOutput::Main { .. })
                        || !old_graph.tracks.iter().any(|track| matches!(track.output, TrackOutput::Group { target: parent, .. } if parent == target))
                        || old_graph.tracks.iter().any(|track| matches!(track.output, TrackOutput::Group { target, .. } if target == index))
                    {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        changed
    }
}

// Runtime compatibility also admits smooth power changes. This route-only class
// additionally requires unchanged authored power/configuration and no programs.
fn same_devices(
    a: &[hex_lattice::PreparedRuntimeInstance],
    b: &[hex_lattice::PreparedRuntimeInstance],
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.foreign_processor().is_none()
                && b.foreign_processor().is_none()
                && a.owner == b.owner
                && a.device_id == b.device_id
                && a.device_type == b.device_type
                && a.bypassed == b.bypassed
                && a.matrix == b.matrix
                && a.sample_rate == b.sample_rate
                && a.modulation.is_none()
                && b.modulation.is_none()
                && a.automation.is_empty()
                && b.automation.is_empty()
                && match (&a.configuration, &b.configuration) {
                    (Some(a), Some(b)) => a.processing_eq_on_callback(b),
                    (None, None) => true,
                    _ => false,
                }
        })
}

fn same_clip(a: &crate::Clip, b: &crate::Clip) -> bool {
    a.active == b.active
        && a.start == b.start
        && a.end == b.end
        && a.looping == b.looping
        && a.source_offset == b.source_offset
        && a.source_length == b.source_length
        && a.audio
            .as_ref()
            .zip(b.audio.as_ref())
            .is_some_and(|(a, b)| {
                a.sample_rate == b.sample_rate && std::sync::Arc::ptr_eq(&a.frames, &b.frames)
            })
        && match (&a.metadata, &b.metadata) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.gain == b.gain
                    && a.source_phase == b.source_phase
                    && a.reversed == b.reversed
                    && a.fade == b.fade
                    && a.streamed.is_none()
                    && b.streamed.is_none()
                    && a.sparse.is_none()
                    && b.sparse.is_none()
            }
            _ => false,
        }
}

#[cfg(test)]
mod tests;
