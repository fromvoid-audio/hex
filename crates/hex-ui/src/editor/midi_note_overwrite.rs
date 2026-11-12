//! Thin held/released-paint adapter for canonical per-source note overwrite geometry.
use super::{
    EditorView,
    midi_scene::{MidiScene, NoteGeometry},
};
use hex_project::{BeatTime, Invalid, MidiNoteInsert, MidiNoteOverwriteGeometry};

impl EditorView {
    pub(super) fn midi_overwrite_preview(
        &self,
    ) -> Result<Option<MidiNoteOverwriteGeometry>, Invalid> {
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
            return MidiNoteOverwriteGeometry::new(authors).map(Some);
        }
        if let Some(gesture) = self.displayed_midi_group_gesture() {
            let originals = gesture.shared_originals();
            let mut authored = Vec::new();
            authored
                .try_reserve_exact(originals.len())
                .map_err(|_| Invalid("could not reserve midi preview geometry"))?;
            for original in originals.iter() {
                let note = gesture
                    .preview(original.id)
                    .ok_or(Invalid("midi preview target changed"))?;
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
            return MidiNoteOverwriteGeometry::new(&authored).map(Some);
        }
        if let Some(preview) = self.displayed_midi_stroke_preview() {
            return MidiNoteOverwriteGeometry::new(&preview.inserted).map(Some);
        }
        if let Some(note) = self.displayed_midi_creation() {
            return MidiNoteOverwriteGeometry::new(&[note]).map(Some);
        }
        Ok(None)
    }
}

pub(super) fn retained_geometry(
    scene: &MidiScene,
    note: NoteGeometry,
    preview: Option<&MidiNoteOverwriteGeometry>,
) -> Option<NoteGeometry> {
    let Some(preview) = preview else {
        return Some(note);
    };
    let duration = preview
        .retained_duration(
            note.pitch,
            BeatTime::from_raw(note.start),
            BeatTime::from_raw(note.duration),
        )?
        .raw();
    if duration == note.duration {
        return Some(note);
    }
    scene
        .preview_geometry(
            note.id,
            note.start,
            duration,
            note.pitch,
            note.channel,
            note.velocity,
        )
        .map(|mut geometry| {
            geometry.active = note.active;
            geometry
        })
}
