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

impl MidiArrangementPreview {
    /// Display-only index from summaries in any order, for UI previews of an
    /// uncommitted layout. Duplicate clip IDs are refused; nothing is compiled.
    pub fn from_clips(mut clips: Vec<MidiClipPreview>) -> Result<Self, Invalid> {
        clips.sort_by_key(|clip| clip.clip_id);
        if clips
            .windows(2)
            .any(|pair| pair[0].clip_id == pair[1].clip_id)
        {
            return Err(Invalid("duplicate midi clip preview"));
        }
        Ok(Self::from_sorted_clips(clips))
    }
}

/// Bounded editor summary of one clip, equal to the summary playback
/// compilation prepares for the same project, without preparing playback.
/// Cost is linear in the clip's authored notes plus the sampled occurrences.
pub fn midi_clip_preview(
    project: &hex_project::Project,
    clip: &hex_project::MidiClip,
) -> Result<MidiClipPreview, Invalid> {
    let tempo = project.tempo_map();
    let frame = |beat: u64| {
        tempo
            .beat_to_frame(hex_project::BeatTime::from_raw(beat))
            .map_err(|_| Invalid("midi preview beat exceeds frame range"))
    };
    let source = clip.source_playback().map_err(|e| Invalid(e.0))?;
    let start = clip.start().raw();
    let end = start
        .checked_add(source.arrangement_duration().raw())
        .ok_or(Invalid("midi clip end beat overflow"))?;
    let mut plans = Vec::new();
    plans
        .try_reserve_exact(clip.notes().len())
        .map_err(|_| Invalid("could not reserve midi preview notes"))?;
    let mut occurrence_count = 0_usize;
    for note in clip.notes() {
        let plan = source
            .note_occurrences(note.start(), note.duration())
            .map_err(|e| Invalid(e.0))?;
        let count = usize::try_from(plan.count())
            .map_err(|_| Invalid("midi preview occurrence count overflow"))?;
        occurrence_count = occurrence_count
            .checked_add(count)
            .ok_or(Invalid("midi preview occurrence count overflow"))?;
        plans.push((note.pitch(), count, plan));
    }
    let ordinals = sample_ordinals(occurrence_count)?;
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(ordinals.len())
        .map_err(|_| Invalid("could not reserve midi preview samples"))?;
    let (mut base, mut selected) = (0_usize, 0_usize);
    for (pitch, count, plan) in &plans {
        let limit = base + count;
        while let Some(&ordinal) = ordinals.get(selected).filter(|&&o| o < limit) {
            let hit = plan
                .iter()
                .nth(ordinal - base)
                .ok_or(Invalid("midi preview occurrence is missing"))?;
            let onset = start
                .checked_add(hit.timeline_start().raw())
                .ok_or(Invalid("midi preview onset overflow"))?;
            let gate = onset
                .checked_add(hit.gate_duration().raw())
                .ok_or(Invalid("midi preview gate overflow"))?;
            samples.push(MidiPreviewSample {
                frame_start: frame(onset)?,
                frame_gate_end: frame(gate)?,
                pitch: *pitch,
            });
            selected += 1;
        }
        base = limit;
    }
    Ok(MidiClipPreview {
        clip_id: clip.id(),
        frame_start: frame(start)?,
        frame_end: frame(end)?,
        authored_note_count: clip.notes().len(),
        occurrence_count,
        samples,
    })
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
