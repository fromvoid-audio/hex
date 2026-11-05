//! Translate MIDI source coordinates before authoring a negative-local note.
//! Finite placement and directed/elapsed playback windows stay unchanged.
//! Shifting the source markers, notes and clip volume together preserves the
//! first-pass and loop phase of all unmoved material before tempo conversion.
//! Linked peers use the Editor's existing shared-content prefix reconciliation.
use crate::{BeatTime, ClipId, Command, Created, Invalid, MidiNoteUpdate, Project};

pub(crate) fn apply(
    project: &mut Project,
    clip: ClipId,
    prefix: BeatTime,
    notes: Vec<MidiNoteUpdate>,
) -> Result<Created, Invalid> {
    if notes.is_empty() || notes.len() > crate::MAX_NOTES {
        return Err(Invalid("invalid midi update batch size"));
    }
    if prefix == BeatTime::ZERO
        || notes
            .iter()
            .map(|note| note.start)
            .min()
            .is_none_or(|start| start >= prefix)
    {
        return Err(Invalid(
            "negative midi authoring needs a positive prefix containing an authored note",
        ));
    }
    let original = project.midi_clip(clip).ok_or(Invalid(
        "negative note movement requires an arrangement midi clip",
    ))?;
    let shift = |value: BeatTime| {
        value
            .raw()
            .checked_add(prefix.raw())
            .map(BeatTime::from_raw)
            .ok_or(Invalid("midi source prefix overflow"))
    };
    let source_length = shift(original.source_length())?;
    let volume = original.volume().shifted(prefix, source_length)?;
    let source = project.midi_source_mut(clip)?;
    source.source_length = source_length;
    source.source_start = shift(source.source_start())?;
    source.loop_start = shift(source.loop_start())?;
    source.loop_end = shift(source.loop_end())?;
    for note in &mut source.notes {
        note.start = shift(note.start())?;
    }
    project.replace_midi_clip_volume(clip, volume)?;
    // Reuse stable-ID, active-state and same-pitch overwrite semantics, including
    // a possible rightward extension in this same private Editor candidate.
    crate::midi::apply(
        project,
        Command::UpdateMidiNotesWithSourceExtent { clip, notes },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Editor, MidiClipGain, MidiClipVolume, MidiClipVolumePoint, NoteId};
    fn beat(n: u64) -> BeatTime {
        BeatTime::from_quarters(n).unwrap()
    }
    fn fixture() -> (Editor, ClipId, NoteId, NoteId) {
        let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = e
            .execute(Command::AddTrack {
                name: "prefix".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = e
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(4),
                length: beat(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let mut ids = Vec::new();
        for (start, pitch) in [(0, 60), (3, 72)] {
            let Created::Note(id) = e
                .execute(Command::AddMidiNote {
                    clip,
                    start: beat(start),
                    duration: BeatTime::from_ratio(1, 4).unwrap(),
                    pitch,
                    channel: 2,
                    velocity: 71,
                })
                .unwrap()
            else {
                panic!("note")
            };
            ids.push(id);
        }
        let volume = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25).unwrap()),
                MidiClipVolumePoint::linear(beat(4), MidiClipGain::new(0.75).unwrap()),
            ],
            beat(4),
        )
        .unwrap()
        .with_clip_gain(crate::ClipGain::new(-3000).unwrap());
        e.execute(Command::ReplaceMidiClipVolume { clip, volume })
            .unwrap();
        e.execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![ids[1]],
            active: false,
        })
        .unwrap();
        (e, clip, ids[0], ids[1])
    }
    fn update(note: NoteId) -> MidiNoteUpdate {
        MidiNoteUpdate {
            note,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 4).unwrap(),
            pitch: 72,
            channel: 2,
            velocity: 71,
        }
    }
    #[test]
    fn prefix_keeps_identity_window_volume_and_history_without_moving_arrangement() {
        let (mut e, clip, held, moved) = fixture();
        let before = e.project().clone();
        e.execute(Command::UpdateMidiNotesWithSourcePrefix {
            clip,
            prefix: beat(1),
            notes: vec![update(moved)],
        })
        .unwrap();
        let after = e.project().clone();
        let old = before.midi_clip(clip).unwrap();
        let next = after.midi_clip(clip).unwrap();
        assert_eq!(
            (next.start(), next.length(), next.playback_offset()),
            (old.start(), old.length(), old.playback_offset())
        );
        assert_eq!(
            (
                next.source_length(),
                next.source_start(),
                next.loop_start(),
                next.loop_end()
            ),
            (beat(5), beat(1), beat(1), beat(5))
        );
        let held_before = before.midi_note(held).unwrap();
        let held_after = after.midi_note(held).unwrap();
        assert_eq!(held_after.start(), beat(1));
        assert_eq!(held_after.id(), held_before.id());
        assert_eq!(held_after.duration(), held_before.duration());
        let moved = after.midi_note(moved).unwrap();
        assert_eq!(moved.start(), BeatTime::ZERO);
        assert!(!moved.active());
        assert_eq!((moved.channel(), moved.velocity()), (2, 71));
        for raw in [0, 1, 1 << 31, 1 << 32, 3 << 32, 4 << 32] {
            assert_eq!(
                old.volume().gain_at(BeatTime::from_raw(raw)),
                next.volume().gain_at(BeatTime::from_raw(raw + (1 << 32)))
            );
        }
        assert_eq!(old.volume().clip_gain(), next.volume().clip_gain());
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
        assert!(e.undo());
        assert_eq!(e.project(), &before);
        assert!(e.redo());
        assert_eq!(e.project(), &after);
    }
    #[test]
    fn ordinary_copy_keeps_its_independent_source_origin() {
        let (mut e, clip, _, moved) = fixture();
        let Created::Clips { first: copy, .. } =
            e.execute(Command::DuplicateClips(vec![clip])).unwrap()
        else {
            panic!("copy")
        };
        let untouched = e.project().midi_clip(copy).unwrap().clone();
        e.execute(Command::UpdateMidiNotesWithSourcePrefix {
            clip,
            prefix: beat(1),
            notes: vec![update(moved)],
        })
        .unwrap();
        assert_eq!(e.project().midi_clip(copy).unwrap(), &untouched);
    }

    #[test]
    fn invalid_note_and_console_target_cannot_publish_a_partial_translation() {
        let (mut e, clip, held, moved) = fixture();
        let Created::Scene(scene) = e
            .execute(Command::AddScene {
                name: "console".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let Created::SessionClip(console) = e
            .execute(Command::CopyArrangementMidiToSession {
                source: clip,
                scene,
            })
            .unwrap()
        else {
            panic!("console")
        };
        for (target, notes) in [
            (console, vec![update(moved)]),
            (clip, vec![update(held), update(held)]),
        ] {
            let mut candidate = Editor::new(e.project().clone(), 8).unwrap();
            let before = candidate.project().clone();
            assert!(
                candidate
                    .execute(Command::UpdateMidiNotesWithSourcePrefix {
                        clip: target,
                        prefix: beat(1),
                        notes
                    })
                    .is_err()
            );
            assert_eq!(candidate.project(), &before);
            assert!(!candidate.can_undo());
        }
    }
    #[test]
    fn invalid_prefixes_refuse_atomically() {
        let (e, clip, _, moved) = fixture();
        for prefix in [BeatTime::ZERO, BeatTime::from_raw(u64::MAX)] {
            let mut e = Editor::new(e.project().clone(), 8).unwrap();
            let before = e.project().clone();
            assert!(
                e.execute(Command::UpdateMidiNotesWithSourcePrefix {
                    clip,
                    prefix,
                    notes: vec![update(moved)]
                })
                .is_err()
            );
            assert_eq!(e.project(), &before);
            assert!(!e.can_undo());
        }
        let mut wrong = Editor::new(e.project().clone(), 8).unwrap();
        let before = wrong.project().clone();
        let mut note = update(moved);
        note.start = beat(1);
        assert!(
            wrong
                .execute(Command::UpdateMidiNotesWithSourcePrefix {
                    clip,
                    prefix: beat(1),
                    notes: vec![note]
                })
                .is_err()
        );
        assert_eq!(wrong.project(), &before);
    }
}
