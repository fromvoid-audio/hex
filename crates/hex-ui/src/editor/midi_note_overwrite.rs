//! Thin held-paint adapter for canonical per-source note overwrite geometry.
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
        if let Some(gesture) = self.midi_note_gesture {
            return source
                .preview_note_overwrite(
                    &[gesture.note],
                    &[MidiNoteInsert {
                        start: BeatTime::from_raw(gesture.preview.start),
                        duration: BeatTime::from_raw(gesture.preview.duration),
                        pitch: gesture.preview.pitch,
                        channel: gesture.channel,
                        velocity: gesture.velocity,
                    }],
                )
                .map(Some);
        }
        if let Some(gesture) = &self.midi_group_gesture {
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
                authored.push(MidiNoteInsert {
                    start: BeatTime::from_raw(note.start),
                    duration: BeatTime::from_raw(note.duration),
                    pitch: note.pitch,
                    channel: note.channel,
                    velocity: note.velocity,
                });
            }
            return source.preview_note_overwrite(&removed, &authored).map(Some);
        }
        if let Some(stroke) = &self.midi_stroke {
            let preview = stroke.shared_preview();
            return source
                .preview_note_overwrite(&preview.removed, &preview.inserted)
                .map(Some);
        }
        if let Some(creation) = &self.midi_creation {
            return source
                .preview_note_overwrite(&[], &[creation.preview])
                .map(Some);
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
        Some(MidiNoteOverwriteChange::TruncateTo(duration)) => scene.preview_geometry(
            note.id,
            note.start,
            duration.raw(),
            note.pitch,
            note.channel,
            note.velocity,
        ),
        None => Some(note),
    }
}
