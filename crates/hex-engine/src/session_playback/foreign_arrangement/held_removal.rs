//! The ordinary held-cut path may retire one direct hosted instrument.
use super::super::{RouteOwner, SessionPlayback, device_move::same_clip};
use hex_project::TrackId;

/// Prove the one hosted-to-dry graph shape before relaxing the outgoing gate
/// check. The exact old gate and binding still belong to ForeignArrangement.
pub(in crate::session_playback) fn candidate(
    next: &SessionPlayback,
    previous: &SessionPlayback,
) -> Option<TrackId> {
    let before = previous.revision.arrangement();
    let after = next.revision.arrangement();
    if before.main_latency_frames() == 0
        || after.main_latency_frames() != 0
        || before.tracks().len() != 2
        || after.tracks().len() != 2
        || before.master_gain != after.master_gain
        || !before.midi_routing.is_empty()
        || !after.midi_routing.is_empty()
        || ![before, after].into_iter().all(|arrangement| {
            arrangement.main_device_chain.is_empty()
                && arrangement
                    .signal_graph
                    .as_ref()
                    .is_some_and(|graph| graph.independent_main_routes())
        })
        || ![previous, next].into_iter().all(|playback| {
            playback.main_pass_delay.is_none()
                && !playback.revision.contains_streamed_sources()
                && !playback.revision.source_routing().has_shared_sources()
                && playback.revision.gain_recording_track().is_none()
                && playback.revision.scalar_automation.is_none()
                && playback.revision.native_parameter_input_target().is_none()
                && playback.routes.iter().all(|route| {
                    route.owner == RouteOwner::Arrangement
                        && route.pending.is_none()
                        && route.audio_release.is_none()
                })
        })
    {
        return None;
    }
    let mut removed = None;
    for (index, (old, new)) in before.tracks().iter().zip(after.tracks()).enumerate() {
        let id = old.canonical_track_id()?;
        if new.canonical_track_id() != Some(id)
            || old.mix != new.mix
            || old.source_mix != new.source_mix
            || old.audio_content_enabled != new.audio_content_enabled
            || old.clips.len() != new.clips.len()
            || !old
                .clips
                .iter()
                .zip(&new.clips)
                .all(|(a, b)| same_clip(a, b))
            || previous.source_effects.has_chain(index)
            || next.source_effects.has_chain(index)
            || ![old, new].into_iter().all(|track| {
                track.device_chain.is_empty()
                    && track.sends.is_empty()
                    && track.midi_source_index.is_none()
                    && !track.has_gain_automation()
            })
            || new.instrument_id.is_some()
            || new.source_id.is_some()
        {
            return None;
        }
        if old.instrument_slot_enabled()
            && old.foreign_instrument.is_some()
            && old.source_id.is_some()
            && old.source_receiver_index == Some(index)
            && old.receives_source_audio()
            && old.clips.is_empty()
            && removed.is_none()
        {
            removed = Some(id);
        } else if old.instrument_id.is_some() || old.source_id.is_some() || old.clips.is_empty() {
            return None;
        }
    }
    removed
}
