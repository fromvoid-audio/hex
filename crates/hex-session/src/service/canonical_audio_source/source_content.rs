//! Conservative control-side source-content evidence; no audio or page custody.
use super::CanonicalAudioSourceSnapshot;
use crate::AudioSourceBinding;
use hex_engine::AudioSourceContext;
use hex_project::TrackId;

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
        if self.context.project_generation != candidate.context.project_generation
            || self.context.editor_revision >= candidate.context.editor_revision
            || self.mode != candidate.mode
            || self.project.sample_rate() != candidate.project.sample_rate()
            || self.project.tempo_map() != candidate.project.tempo_map()
        {
            return None;
        }
        let old = self.project.track(track)?;
        let new = candidate.project.track(track)?;
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
            || self
                .project
                .session_cells()
                .iter()
                .any(|cell| cell.track() == track)
            || candidate
                .project
                .session_cells()
                .iter()
                .any(|cell| cell.track() == track)
        {
            return None;
        }
        for asset in old.clips().iter().map(|clip| clip.asset()) {
            let old_asset = self.project.asset(asset)?;
            let new_asset = candidate.project.asset(asset)?;
            if old_asset != new_asset {
                return None;
            }
            let same = match (self.sources.get(&asset)?, candidate.sources.get(&asset)?) {
                (AudioSourceBinding::Resident(old), AudioSourceBinding::Resident(new)) => {
                    old.sample_rate() == new.sample_rate() && old.shares_pcm_with(new)
                }
                (AudioSourceBinding::VerifiedFile(old), AudioSourceBinding::VerifiedFile(new)) => {
                    old == new
                }
                _ => false,
            };
            if !same {
                return None;
            }
        }
        Some(RawArrangementContentWitness {
            previous: self.context,
            candidate: candidate.context,
            track,
        })
    }
}

#[cfg(test)]
mod tests;
