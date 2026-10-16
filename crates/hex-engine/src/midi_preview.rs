//! Immutable, bounded Arrangement data prepared alongside MIDI playback.

pub(crate) mod persistent;
use crate::Invalid;
use hex_project::ClipId;

/// Maximum exact note samples retained for one MIDI clip preview.
pub const MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP: usize = 32;

/// One exact prepared note interval selected for compact Arrangement drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiPreviewSample {
    /// Absolute project frame of the note onset.
    pub frame_start: u64,
    /// Absolute project frame where the note gate ends, before instrument release.
    pub frame_gate_end: u64,
    pub pitch: u8,
}

/// Bounded preview for one MIDI clip, derived from admitted playback occurrences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiClipPreview {
    pub clip_id: ClipId,
    pub frame_start: u64,
    pub frame_end: u64,
    pub authored_note_count: usize,
    pub occurrence_count: usize,
    pub samples: Vec<MidiPreviewSample>,
}

/// Immutable project-wide preview index. Clips are sorted by ID for O(log n) lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiArrangementPreview {
    clips: crate::owned_shared::OwnedShared<[MidiClipPreview]>,
}

impl MidiArrangementPreview {
    pub(crate) fn from_sorted_clips(clips: Vec<MidiClipPreview>) -> Self {
        Self {
            clips: crate::owned_shared::OwnedShared::from_owned_vec(clips),
        }
    }

    /// Prepared summaries in clip-ID order.
    pub fn clips(&self) -> &[MidiClipPreview] {
        &self.clips
    }

    /// Find a clip summary without scanning the project or source notes.
    pub fn get(&self, clip_id: ClipId) -> Option<&MidiClipPreview> {
        self.clips
            .binary_search_by_key(&clip_id, |clip| clip.clip_id)
            .ok()
            .and_then(|index| self.clips.get(index))
    }
}

/// Evenly spaced occurrence ordinals, always including the first and last when
/// the clip has at least two occurrences. u128 keeps the product checked for
/// future cap changes as well as today's project-wide 65,536 occurrence limit.
pub(crate) fn sample_ordinals(occurrence_count: usize) -> Result<Vec<usize>, Invalid> {
    let sample_count = occurrence_count.min(MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP);
    let mut ordinals = Vec::new();
    ordinals
        .try_reserve_exact(sample_count)
        .map_err(|_| Invalid("could not reserve midi preview sample storage"))?;
    if sample_count == 0 {
        return Ok(ordinals);
    }
    if sample_count == 1 {
        ordinals.push(0);
        return Ok(ordinals);
    }
    let last = (occurrence_count - 1) as u128;
    let denominator = (sample_count - 1) as u128;
    for sample in 0..sample_count {
        let numerator = (sample as u128)
            .checked_mul(last)
            .ok_or(Invalid("midi preview sample index overflow"))?;
        let ordinal = usize::try_from(numerator / denominator)
            .map_err(|_| Invalid("midi preview sample index exceeds platform range"))?;
        ordinals.push(ordinal);
    }
    Ok(ordinals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_ordinals_are_bounded_deterministic_and_include_endpoints() {
        assert_eq!(sample_ordinals(0).expect("empty"), []);
        assert_eq!(sample_ordinals(1).expect("one"), [0]);
        assert_eq!(sample_ordinals(2).expect("two"), [0, 1]);
        let samples = sample_ordinals(1_000).expect("bounded selection");
        assert_eq!(samples.len(), MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP);
        assert_eq!(samples.first(), Some(&0));
        assert_eq!(samples.last(), Some(&999));
        assert!(samples.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(samples, sample_ordinals(1_000).expect("repeat selection"));
    }
}
