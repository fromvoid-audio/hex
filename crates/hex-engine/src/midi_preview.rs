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
/// Immutable constructor-paired export. Raw Arc ownership cannot be adopted.
/// ```compile_fail
/// let raw = std::sync::Arc::new(hex_engine::MidiArrangementPreview::default());
/// let _: hex_engine::SharedMidiArrangementPreview = raw.into();
/// ```
pub type SharedMidiArrangementPreview = crate::SharedMetadata<MidiArrangementPreview>;

pub fn empty_midi_preview_on_control() -> SharedMidiArrangementPreview {
    SharedMidiArrangementPreview::new_on_control(MidiArrangementPreview::empty_on_control())
}

/// Actual paired outer plus private clip-slice and sample Vec backings. Every
/// other physical owner and construction/custody/publication remain separate.
pub fn visit_shared_midi_preview_storage_on_control<'a>(
    owner: &'a SharedMidiArrangementPreview,
    ledger: &mut crate::AudioRootStorageLedger<'a>,
) -> Result<(), crate::AudioStorageError> {
    ledger.borrow_shared_metadata_body_on_control(owner)?;
    ledger.mutate(|ledger| {
        let MidiArrangementPreview { clips } = owner.as_ref();
        if !ledger.skip_observed_shared_field(clips) {
            ledger.observe_shared(
                clips.arc(),
                clips.parts(),
                crate::audio_storage::Owner::exclusive(clips),
            )?;
        }
        for MidiClipPreview {
            clip_id,
            frame_start,
            frame_end,
            authored_note_count,
            occurrence_count,
            samples,
        } in clips.iter()
        {
            let _: (&ClipId, &u64, &u64, &usize, &usize) = (
                clip_id,
                frame_start,
                frame_end,
                authored_note_count,
                occurrence_count,
            );
            for MidiPreviewSample {
                frame_start,
                frame_gate_end,
                pitch,
            } in samples
            {
                let _: (&u64, &u64, &u8) = (frame_start, frame_gate_end, pitch);
            }
            ledger.vec(samples)?;
        }
        Ok(())
    })
}

impl MidiArrangementPreview {
    /// Distinct fresh sized-empty-array origin, not adoption of an empty Arc.
    pub fn empty_on_control() -> Self {
        Self {
            clips: crate::owned_shared::OwnedShared::new_owned_empty_slice(),
        }
    }
    pub(crate) fn from_sorted_clips(clips: Vec<MidiClipPreview>) -> Self {
        if clips.is_empty() {
            // Dispose any input Vec spare backing on control BEFORE allocating
            // the new shared empty array. Input/preparation peak stays separate.
            drop(clips);
            return Self::empty_on_control();
        }
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

#[cfg(test)]
mod shared_midi_storage_tests {
    use super::*;
    use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    use std::{
        alloc::Layout,
        collections::BTreeMap,
        sync::{Arc, atomic::AtomicUsize},
    };

    fn reservation() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 8,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        }
    }
    fn actual_revision() -> crate::PreparedRevision {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "preview".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        crate::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            crate::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap()
    }
    fn requested_arc(payload: Layout) -> usize {
        Layout::array::<AtomicUsize>(2)
            .unwrap()
            .extend(payload)
            .unwrap()
            .0
            .pad_to_align()
            .size()
    }
    #[test]
    fn shared_midi_preview_actual_prepared_pair_aliases_and_exact_retained_limits() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported midi preview constructor; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED actual PreparedRevision MIDI owner exports and retained aliases");
        let prepared = actual_revision();
        let owner = prepared.midi_preview_on_control();
        let alias = prepared.midi_preview_on_control();
        assert!(std::ptr::eq(
            owner.as_ref(),
            prepared.midi_preview().as_ref()
        ));
        assert!(owner.shares_owner_with(&alias));
        assert!(owner.cache_identity().is_none());
        assert_eq!(owner.clips()[0].samples[0].pitch, 60);
        let mut omitted = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        visit_shared_midi_preview_storage_on_control(&owner, &mut omitted).unwrap();
        visit_shared_midi_preview_storage_on_control(&alias, &mut omitted).unwrap();
        // The ACTUAL still-live prepared field is another outer owner. Selected
        // exports cannot silently stand in for it or establish whole-root closure.
        assert!(
            omitted
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        drop(omitted);
        drop(prepared);
        let expected = requested_arc(Layout::new::<MidiArrangementPreview>())
            + requested_arc(Layout::array::<MidiClipPreview>(owner.clips().len()).unwrap())
            + owner
                .clips()
                .iter()
                .map(|clip| clip.samples.capacity() * std::mem::size_of::<MidiPreviewSample>())
                .sum::<usize>();
        let collect = |budget| -> Result<_, AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
            visit_shared_midi_preview_storage_on_control(&owner, &mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            visit_shared_midi_preview_storage_on_control(&alias, &mut ledger)?;
            visit_shared_midi_preview_storage_on_control(&alias, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let summary = ledger.summary()?;
            assert_eq!(summary.data_bytes, expected);
            assert!(summary.require_complete().is_err());
            Ok(summary)
        };
        let measured = collect(reservation()).unwrap();
        let mut exact = reservation();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        drop(owner);
        assert_eq!(alias.clips()[0].samples[0].pitch, 60);
    }
    #[test]
    fn shared_midi_preview_reserved_samples_and_independent_empty_headers_are_charged() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported midi preview constructor; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED reserved empty MIDI samples and distinct fresh empty headers");
        let prepared = actual_revision();
        let mut clips = prepared.midi_preview_on_control().clips().to_vec();
        clips[0].samples.clear();
        clips[0].samples.try_reserve_exact(19).unwrap();
        let sample_bytes = clips[0].samples.capacity() * std::mem::size_of::<MidiPreviewSample>();
        assert!(sample_bytes >= 19 * std::mem::size_of::<MidiPreviewSample>());
        let owner = SharedMidiArrangementPreview::new_on_control(
            MidiArrangementPreview::from_sorted_clips(clips),
        );
        let empty = empty_midi_preview_on_control();
        let independent = empty_midi_preview_on_control();
        assert!(!empty.shares_owner_with(&independent));
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        for value in [&owner, &empty, &independent] {
            visit_shared_midi_preview_storage_on_control(value, &mut ledger).unwrap();
        }
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            3 * requested_arc(Layout::new::<MidiArrangementPreview>())
                + requested_arc(Layout::array::<MidiClipPreview>(1).unwrap())
                + 2 * requested_arc(Layout::array::<MidiClipPreview>(0).unwrap())
                + sample_bytes
        );
        assert_eq!(ledger.summary().unwrap().data_allocations, 7);
    }
    #[test]
    fn shared_midi_preview_raw_default_and_unsupported_origins_are_not_upgraded() {
        let raw = crate::owned_shared::OwnedShared::untracked(Arc::new(
            MidiArrangementPreview::empty_on_control(),
        ));
        let export = crate::SharedMetadata::clone_prepared_midi_on_control(&raw);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        assert_eq!(
            visit_shared_midi_preview_storage_on_control(&export, &mut ledger),
            Err(AudioStorageError::UnknownStorage)
        );
        let general_default =
            SharedMidiArrangementPreview::new_on_control(MidiArrangementPreview::default());
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        assert!(
            visit_shared_midi_preview_storage_on_control(&general_default, &mut ledger).is_err()
        );
        if !crate::audio_storage_constructor_layout_supported() {
            let explicit = empty_midi_preview_on_control();
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
            assert!(visit_shared_midi_preview_storage_on_control(&explicit, &mut ledger).is_err());
        }
    }
}
