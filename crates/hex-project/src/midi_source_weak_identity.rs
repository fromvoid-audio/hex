//! Nonowning equality for bounded view-history metadata. Weak headers prevent
//! address reuse without retaining note/content backings. This cannot borrow a
//! source, establish canonical audio freshness or authorize resource publication.
use super::{MidiSource, MidiSourceIdentity};
use std::sync::{Arc, Weak};

#[derive(Clone, Debug)]
pub struct MidiSourceWeakIdentity(Weak<MidiSource>);
impl MidiSourceIdentity {
    /// A comparison-only view token; it keeps no MIDI content alive.
    pub fn downgrade(&self) -> MidiSourceWeakIdentity {
        MidiSourceWeakIdentity(Arc::downgrade(&self.0))
    }
}
impl MidiSourceWeakIdentity {
    pub fn matches(&self, source: &MidiSourceIdentity) -> bool {
        Weak::ptr_eq(&self.0, &Arc::downgrade(&source.0))
    }
}

#[cfg(test)]
mod tests {
    use crate::{BeatTime, Command, Created, Editor, MidiNoteUpdate, Project};
    #[test]
    fn weak_view_identity_follows_real_history_without_retaining_source_content() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "view".into(),
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
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note")
        };
        let source = editor.project().midi_clip(clip).unwrap().source_identity();
        let weak = source.downgrade();
        editor
            .execute(Command::UpdateMidiNotesWithSourcePrefix {
                clip,
                prefix: BeatTime::ONE,
                notes: vec![MidiNoteUpdate {
                    note,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            })
            .unwrap();
        assert!(!weak.matches(&editor.project().midi_clip(clip).unwrap().source_identity()));
        assert!(editor.undo());
        assert!(weak.matches(&editor.project().midi_clip(clip).unwrap().source_identity()));
        assert!(editor.redo());
        assert!(!weak.matches(&editor.project().midi_clip(clip).unwrap().source_identity()));
        drop(editor);
        assert_eq!(weak.0.strong_count(), 1);
        drop(source);
        assert_eq!(weak.0.strong_count(), 0);
        assert!(weak.0.upgrade().is_none());
    }
}
