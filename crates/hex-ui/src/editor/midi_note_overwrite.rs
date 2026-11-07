//! Thin held/released-paint adapter for canonical per-source note overwrite geometry.
use super::{
    EditorView,
    midi_scene::{MidiScene, NoteGeometry},
};
use hex_project::{
    BeatTime, Invalid, MidiNoteInsert, MidiNoteOverwriteChange, MidiNoteOverwritePreview,
    MidiSource,
};

impl EditorView {
    pub(super) fn midi_overwrite_preview(
        &self,
        source: &MidiSource,
    ) -> Result<Option<MidiNoteOverwritePreview>, Invalid> {
        if let Some(gesture) = self.displayed_midi_note_gesture() {
            // A negative author can affect old unsigned notes only in its
            // intersection with [0, source end). Its full signed geometry remains
            // in the held/frozen gesture, including the part before source zero.
            let local_start = i128::from(gesture.preview.start) - i128::from(gesture.source_prefix);
            let clipped_prefix = (-local_start).max(0) as u64;
            let duration = gesture.preview.duration.saturating_sub(clipped_prefix);
            let author = MidiNoteInsert {
                active: gesture.active,
                start: BeatTime::from_raw(local_start.max(0) as u64),
                duration: BeatTime::from_raw(duration),
                pitch: gesture.preview.pitch,
                channel: gesture.channel,
                velocity: gesture.velocity,
            };
            let authors = if duration > 0 {
                std::slice::from_ref(&author)
            } else {
                &[]
            };
            return source
                .preview_note_overwrite(&[gesture.note], authors)
                .map(Some);
        }
        if let Some(gesture) = self.displayed_midi_group_gesture() {
            let originals = gesture.shared_originals();
            let mut removed = Vec::new();
            let mut authored = Vec::new();
            removed
                .try_reserve_exact(originals.len())
                .map_err(|_| Invalid("could not reserve midi preview identities"))?;
            authored
                .try_reserve_exact(originals.len())
                .map_err(|_| Invalid("could not reserve midi preview geometry"))?;
            for original in originals.iter() {
                let note = gesture
                    .preview(original.id)
                    .ok_or(Invalid("midi preview target changed"))?;
                if !gesture.is_copy() {
                    removed.push(original.id);
                }
                let local_start = i128::from(note.start) - i128::from(gesture.source_prefix());
                let clipped_prefix = (-local_start).max(0) as u64;
                let duration = note.duration.saturating_sub(clipped_prefix);
                if duration == 0 {
                    continue;
                }
                authored.push(MidiNoteInsert {
                    active: note.active,
                    start: BeatTime::from_raw(local_start.max(0) as u64),
                    duration: BeatTime::from_raw(duration),
                    pitch: note.pitch,
                    channel: note.channel,
                    velocity: note.velocity,
                });
            }
            return source.preview_note_overwrite(&removed, &authored).map(Some);
        }
        if let Some(preview) = self.displayed_midi_stroke_preview() {
            return source
                .preview_note_overwrite(&preview.removed, &preview.inserted)
                .map(Some);
        }
        if let Some(note) = self.displayed_midi_creation() {
            return source.preview_note_overwrite(&[], &[note]).map(Some);
        }
        Ok(None)
    }
}

pub(super) fn retained_geometry(
    scene: &MidiScene,
    note: NoteGeometry,
    preview: Option<&MidiNoteOverwritePreview>,
) -> Option<NoteGeometry> {
    match preview.and_then(|preview| preview.existing_change(note.id)) {
        Some(MidiNoteOverwriteChange::Remove) => None,
        Some(MidiNoteOverwriteChange::TruncateTo(duration)) => scene
            .preview_geometry(
                note.id,
                note.start,
                duration.raw(),
                note.pitch,
                note.channel,
                note.velocity,
            )
            .map(|mut geometry| {
                geometry.active = note.active;
                geometry
            }),
        None => Some(note),
    }
}
