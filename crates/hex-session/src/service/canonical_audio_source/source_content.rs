//! Conservative control-side source-content evidence; no audio or page custody.
use super::CanonicalAudioSourceSnapshot;
use crate::AudioSourceBinding;
use hex_engine::AudioSourceContext;
use hex_project::{AssetId, Project, TrackId};

/// Exact canonical correlations for one unchanged raw Arrangement audio track.
///
/// This does not establish that either snapshot is the active publication,
/// that a stream domain or its pages are retained, or that routing, effects,
/// timing, or a fade can be inherited. Consumers must separately prove those
/// facts against the actual callback owners before using an outgoing source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawArrangementContentWitness {
    previous: AudioSourceContext,
    candidate: AudioSourceContext,
    track: TrackId,
}

impl RawArrangementContentWitness {
    /// Compare only the captured correlations; this does not authorize a handoff.
    pub fn matches(
        &self,
        previous: AudioSourceContext,
        candidate: AudioSourceContext,
        track: TrackId,
    ) -> bool {
        self.previous == previous && self.candidate == candidate && self.track == track
    }

    /// A staged mute leaves the worker's source bindings unchanged. Compare
    /// the authored content against that borrowed resident inventory; the
    /// compiler and output still check the actual PCM and predecessor.
    pub(in crate::service) fn for_resident_mute(
        previous: (&Project, AudioSourceContext),
        candidate: (&Project, AudioSourceContext),
        track: TrackId,
        resident: impl Fn(AssetId) -> bool,
    ) -> Option<Self> {
        if previous.0.track(track)?.muted() || !candidate.0.track(track)?.muted() {
            return None;
        }
        compare_content(previous, candidate, track, resident)
    }
}

impl CanonicalAudioSourceSnapshot {
    /// Compare source content on control, conservatively refusing complex or
    /// unsupported tracks. Work is bounded by the validated Project inventory.
    /// Equal file facts do not establish that any file or stream lease is live.
    pub fn raw_arrangement_content_witness(
        &self,
        candidate: &Self,
        track: TrackId,
    ) -> Option<RawArrangementContentWitness> {
        if self.mode != candidate.mode {
            return None;
        }
        compare_content(
            (&self.project, self.context),
            (&candidate.project, candidate.context),
            track,
            |asset| match (self.sources.get(&asset), candidate.sources.get(&asset)) {
                (
                    Some(AudioSourceBinding::Resident(old)),
                    Some(AudioSourceBinding::Resident(new)),
                ) => old.sample_rate() == new.sample_rate() && old.shares_pcm_with(new),
                (
                    Some(AudioSourceBinding::VerifiedFile(old)),
                    Some(AudioSourceBinding::VerifiedFile(new)),
                ) => old == new,
                _ => false,
            },
        )
    }
}

fn compare_content(
    (old_project, previous): (&Project, AudioSourceContext),
    (new_project, candidate): (&Project, AudioSourceContext),
    track: TrackId,
    same_binding: impl Fn(AssetId) -> bool,
) -> Option<RawArrangementContentWitness> {
    if previous.project_generation != candidate.project_generation
        || previous.editor_revision >= candidate.editor_revision
        || old_project.sample_rate() != new_project.sample_rate()
        || old_project.tempo_map() != new_project.tempo_map()
    {
        return None;
    }
    let old = old_project.track(track)?;
    let new = new_project.track(track)?;
    if old.is_group()
        || new.is_group()
        || old.source_id().is_some()
        || new.source_id().is_some()
        || old.instrument().is_some()
        || new.instrument().is_some()
        || !old.midi_clips().is_empty()
        || !new.midi_clips().is_empty()
        || old.clips().is_empty()
        || old.clips() != new.clips()
        || old.clips().iter().any(|clip| clip.audio_edit().is_some())
        || old_project
            .session_cells()
            .iter()
            .any(|cell| cell.track() == track)
        || new_project
            .session_cells()
            .iter()
            .any(|cell| cell.track() == track)
    {
        return None;
    }
    for asset in old.clips().iter().map(|clip| clip.asset()) {
        if old_project.asset(asset)? != new_project.asset(asset)? || !same_binding(asset) {
            return None;
        }
    }
    Some(RawArrangementContentWitness {
        previous,
        candidate,
        track,
    })
}

#[cfg(test)]
mod tests;
