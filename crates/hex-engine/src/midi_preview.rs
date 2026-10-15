//! Immutable, bounded Arrangement data prepared alongside MIDI playback.

use crate::Invalid;
use hex_project::ClipId;
pub(crate) mod persistent;
#[cfg(test)]
use std::sync::Arc;

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

impl MidiArrangementPreview {
    pub(crate) fn visit_owned_storage<'a, V: crate::storage_report::PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut crate::storage_report::Report<'a, '_, V>,
    ) -> Result<(), crate::storage_report::StorageReportError> {
        let Self { clips } = self;
        let owner = report.owned_arc(clips, "midi_preview_clips")?;
        for (index, clip) in clips.iter().enumerate() {
            let MidiClipPreview {
                clip_id: _,
                frame_start: _,
                frame_end: _,
                authored_note_count: _,
                occurrence_count: _,
                samples,
            } = clip;
            report.vec(owner, "preview.samples", [index, 0], samples)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod owned_storage_tests {
    use super::*;
    use crate::storage_report::{Report, StorageOwner, StorageReportError, owned_tests::Sink};
    #[test]
    fn preview_samples_follow_actual_nested_owner_not_parent_or_equal_content() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "track".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let hex_project::Created::Scene(scene) = editor
            .execute(hex_project::Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let hex_project::Created::SessionClip(clip_id) = editor
            .execute(hex_project::Command::CreateSessionMidiClip {
                track,
                scene,
                name: "clip".into(),
                source_length: hex_project::BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let mut samples = Vec::with_capacity(17);
        samples.push(MidiPreviewSample {
            frame_start: 0,
            frame_gate_end: 1,
            pitch: 60,
        });
        let preview = MidiArrangementPreview::from_sorted_clips(vec![MidiClipPreview {
            clip_id,
            frame_start: 0,
            frame_end: 1,
            authored_note_count: 1,
            occurrence_count: 1,
            samples,
        }]);
        let alias = preview.clone();
        let independent = MidiArrangementPreview::from_sorted_clips(preview.clips().to_vec());
        let capacity = preview.clips[0].samples.capacity();
        let strong = Arc::strong_count(&preview.clips);
        let mut sink = Sink::new(16);
        for value in [&preview, &alias, &independent] {
            value
                .visit_owned_storage(&mut Report::new(&mut sink))
                .unwrap();
        }
        let stride = std::mem::size_of::<MidiPreviewSample>();
        assert_eq!(
            sink.measured("preview.samples", [0; 2]),
            (capacity, stride, capacity * stride)
        );
        let owners: Vec<_> = sink
            .items
            .iter()
            .filter(|item| item.location.component == "preview.samples")
            .map(|item| {
                let StorageOwner::Shared(owner) = item.owner else {
                    panic!("nested owner")
                };
                owner
            })
            .collect();
        assert!(owners[0].same_allocation(owners[1]));
        assert!(!owners[0].same_allocation(owners[2]));
        assert_eq!(Arc::strong_count(&preview.clips), strong);
        drop(sink);
        for limit in 0..3 {
            let mut sink = Sink::new(limit);
            assert_eq!(
                preview.visit_owned_storage(&mut Report::new(&mut sink)),
                Err(StorageReportError::SinkCapacity)
            );
            assert_eq!(Arc::strong_count(&preview.clips), strong);
        }
    }
}
