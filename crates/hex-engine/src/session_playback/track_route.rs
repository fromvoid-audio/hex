//! One Arrangement leaf changes its route to Main or an existing Group.
//! Classification borrows bounded prepared inventories only. Full handoff preview
//! still fences runtime and tails before the old route's post-device fade.
//! Unchanged processors stay on their existing clocks. A supported
//! compensated route waits for the departing output's aligned PCM to drain.
//! Verified streaming admits fixed-row, zero-delay moves between Main and a
//! Group that retains another child, or across the flat boundary when only
//! one Group route changes. Both page windows must be ready first. The flat
//! boundary cuts between the leaf and Group outputs with fresh DSP on both.
use super::{RouteOwner, SessionPlayback};
use crate::signal_graph::{SignalGraph, TrackOutput};

impl SessionPlayback {
    // The moving leaf can occupy different rows. Stage its predecessor fader
    // after proving that every other route and relative row order stays fixed.
    pub(super) fn group_reroute_from(&self, previous: &Self) -> Option<usize> {
        let streamed = self.revision.contains_streamed_sources();
        if streamed != previous.revision.contains_streamed_sources() {
            return None;
        }
        for playback in [self, previous] {
            if playback.foreign_runtime_bindings().next().is_some()
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
        }
        let old_graph = previous.group_cut_graph()?;
        let new_graph = self.group_cut_graph()?;
        // A flat side has no buffered route or latency history. Every
        // runtime-backed side still needs the closed zero-delay Group proof.
        if streamed
            && [old_graph, new_graph]
                .into_iter()
                .any(|graph| graph.needs_runtime() && !graph.supports_closed_group_stream())
        {
            return None;
        }
        let old = previous.revision.arrangement();
        let new = self.revision.arrangement();
        let main_latency = old.main_latency_frames();
        if old.sample_rate != new.sample_rate
            || old.tempo_map != new.tempo_map
            || old.loop_region != new.loop_region
            || old.master_gain != new.master_gain
            || main_latency != new.main_latency_frames()
            || old_graph.main_input_arrival() != new_graph.main_input_arrival()
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
        let mut changed = None;
        for (old_index, old) in old.tracks().iter().enumerate() {
            let id = old.canonical_track_id()?;
            let new_index = new_graph.track_index(id)?;
            let new = new.tracks().get(new_index)?;
            let old_chain = previous.device_chains.get(old_index)?;
            let new_chain = self.device_chains.get(new_index)?;
            if new.canonical_track_id() != Some(id)
                || old.mix != new.mix
                || old.source_mix != new.source_mix
                || old.source_input_enabled != new.source_input_enabled
                || old.audio_content_enabled != new.audio_content_enabled
                || old.clips.len() != new.clips.len()
                || !old
                    .clips
                    .iter()
                    .zip(&new.clips)
                    .all(|(a, b)| same_clip(a, b, streamed))
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
            let a = old_graph.tracks.get(old_index)?;
            let b = new_graph.tracks.get(new_index)?;
            if a.id != Some(id) || b.id != Some(id) {
                return None;
            }
            match (a.output, b.output) {
                (TrackOutput::Main { .. }, TrackOutput::Main { .. }) => {}
                (TrackOutput::Group { target: a, .. }, TrackOutput::Group { target: b, .. })
                    if old_graph.tracks.get(a)?.id? == new_graph.tracks.get(b)?.id? => {}
                (TrackOutput::Main { .. }, TrackOutput::Group { .. })
                | (TrackOutput::Group { .. }, TrackOutput::Main { .. })
                | (TrackOutput::Group { .. }, TrackOutput::Group { .. }) => {
                    // The fader can move one leaf without moving a subtree.
                    if changed.replace((old_index, new_index)).is_some()
                        || old.clips.is_empty()
                        || old_graph.tracks.iter().any(|track| matches!(track.output, TrackOutput::Group { target, .. } if target == old_index))
                    {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        let (from, to) = changed?;
        if streamed {
            if from != to {
                return None;
            }
            let (direct, grouped, target) = match (
                old_graph.tracks.get(from)?.output,
                new_graph.tracks.get(to)?.output,
            ) {
                (TrackOutput::Main { .. }, TrackOutput::Group { target, .. }) => {
                    (old_graph, new_graph, target)
                }
                (TrackOutput::Group { target, .. }, TrackOutput::Main { .. }) => {
                    (new_graph, old_graph, target)
                }
                _ => return None,
            };
            let group = direct.track_index(grouped.tracks.get(target)?.id?)?;
            if direct.needs_runtime()
                && !direct.tracks.iter().any(|track| {
                    matches!(track.output, TrackOutput::Group { target, .. } if target == group)
                })
            {
                return None;
            }
        }
        let upstream_delay = old_graph.main_input_arrival() != 0;
        // The aligned entry from above and its upward inverse retain the
        // exact old fader and surviving rings by TrackId across the row shift.
        if main_latency != 0 && from != to {
            let follows_aligned_row_shift = match (
                old_graph.tracks.get(from)?.output,
                new_graph.tracks.get(to)?.output,
            ) {
                (TrackOutput::Main { .. }, TrackOutput::Group { .. }) => from < to,
                (TrackOutput::Group { .. }, TrackOutput::Main { .. }) => from > to,
                _ => false,
            };
            if !upstream_delay || !follows_aligned_row_shift {
                return None;
            }
        }
        if upstream_delay {
            let track = old.tracks().get(from)?.canonical_track_id()?;
            new_graph.aligned_group_reroute_edges_from(old_graph, track)?;
        }
        // The one row shift may move other indices without reordering the
        // other tracks. Reject a compound edit before staging any fader.
        let other_old_ids = old
            .tracks()
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != from)
            .map(|(_, track)| track.canonical_track_id());
        let other_new_ids = new
            .tracks()
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != to)
            .map(|(_, track)| track.canonical_track_id());
        other_old_ids.eq(other_new_ids).then_some(from)
    }

    /// After the fixed-row route proof, identify the predecessor and successor
    /// output faders when the Group gains its first input or loses its last.
    pub(super) fn verified_flat_group_faders(
        &self,
        previous: &Self,
        leaf: usize,
    ) -> Option<(usize, usize)> {
        if !self.revision.contains_streamed_sources() {
            return None;
        }
        let old = previous.group_cut_graph()?.tracks.get(leaf)?.output;
        let new = self.group_cut_graph()?.tracks.get(leaf)?.output;
        match (
            previous.compensation.is_some(),
            self.compensation.is_some(),
            old,
            new,
        ) {
            (false, true, TrackOutput::Main { .. }, TrackOutput::Group { target, .. }) => {
                Some((leaf, target))
            }
            (true, false, TrackOutput::Group { target, .. }, TrackOutput::Main { .. }) => {
                Some((target, leaf))
            }
            _ => None,
        }
    }

    fn group_cut_graph(&self) -> Option<&SignalGraph> {
        let arrangement = self.revision.arrangement();
        let graph = arrangement.signal_graph.as_deref()?;
        if !arrangement.return_buses().is_empty() {
            return None;
        }
        // Before the first child joins (or after the last leaves), the flat
        // graph has no routing runtime. The zero-delay grouped side reads
        // only current-frame stamps, so there is no clock history to transfer.
        let supported = match &self.compensation {
            None => !graph.needs_runtime(),
            Some(runtime) => {
                std::ptr::eq(graph, runtime.graph.as_ref())
                    && ((runtime.horizon() == 0
                        && graph.supports_closed_group_stream()
                        && graph.supports_closed_compensated_audio_groups())
                        || runtime.group_route_main_delay_only()
                        || runtime.group_route_upstream_delay())
            }
        };
        supported.then_some(graph)
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

fn same_clip(a: &crate::Clip, b: &crate::Clip, streamed: bool) -> bool {
    let same_audio = if streamed {
        a.audio.is_none() && b.audio.is_none()
    } else {
        a.audio
            .as_ref()
            .zip(b.audio.as_ref())
            .is_some_and(|(a, b)| {
                a.sample_rate == b.sample_rate && std::sync::Arc::ptr_eq(&a.frames, &b.frames)
            })
    };
    a.active == b.active
        && a.start == b.start
        && a.end == b.end
        && a.looping == b.looping
        && a.source_offset == b.source_offset
        && a.source_length == b.source_length
        && same_audio
        && match (&a.metadata, &b.metadata) {
            (None, None) => !streamed,
            (Some(a), Some(b)) => {
                a.gain == b.gain
                    && a.source_phase == b.source_phase
                    && a.reversed == b.reversed
                    && a.fade == b.fade
                    && a.streamed.is_some() == streamed
                    && a.streamed == b.streamed
                    && a.sparse.is_none()
                    && b.sparse.is_none()
            }
            _ => false,
        }
}

#[cfg(test)]
mod tests;
