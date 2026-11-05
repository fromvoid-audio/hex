//! Canonical metadata-only copies for Arrangement clip drag duplication.
use crate::{ClipId, Created, Invalid, Project, TrackId};

pub(crate) fn audio_to(
    project: &mut Project,
    clip_id: ClipId,
    track: TrackId,
    start: u64,
) -> Result<Created, Invalid> {
    let mut copied = project
        .clip(clip_id)
        .cloned()
        .ok_or(Invalid("clip no longer exists"))?;
    crate::audio_edit::move_clip(&mut copied, start, project.tempo_map())?;
    project
        .track(track)
        .ok_or(Invalid("destination track no longer exists"))?;
    if original_audio_at(project, track, clip_id, start) {
        return Ok(Created::None);
    }

    crate::clip_group::place_one(project, track, crate::ClipGroupContent::Audio(copied), true)
}

pub(crate) fn midi_to(
    project: &mut Project,
    clip_id: ClipId,
    track: TrackId,
    start: crate::BeatTime,
) -> Result<Created, Invalid> {
    let source_track = project
        .tracks
        .iter()
        .find(|candidate| candidate.midi_clips().iter().any(|item| item.id == clip_id))
        .map(|candidate| candidate.id)
        .ok_or(Invalid("midi clip no longer exists"))?;
    let copied = project
        .midi_clip(clip_id)
        .cloned()
        .ok_or(Invalid("midi clip no longer exists"))?;
    crate::midi::validate_clip(start, copied.length())?;
    project
        .track(track)
        .ok_or(Invalid("destination track no longer exists"))?;
    if source_track == track && copied.start == start {
        return Ok(Created::None);
    }
    let mut copied = copied;
    copied.start = start;
    crate::clip_group::place_one(project, track, crate::ClipGroupContent::Midi(copied), true)
}

fn original_audio_at(project: &Project, track: TrackId, clip: ClipId, start: u64) -> bool {
    project.track(track).is_some_and(|destination| {
        destination
            .clips()
            .iter()
            .any(|item| item.id() == clip && item.start() == start)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Command, Editor, Project};

    fn audio_fixture() -> (Editor, TrackId, TrackId, ClipId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "missing.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .expect("audio")
        else {
            panic!("import result")
        };
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "target".into(),
            })
            .expect("target")
        else {
            panic!("track result")
        };
        (editor, track, target, clip)
    }

    #[test]
    fn audio_copy_uses_fresh_id_and_preserves_source_metadata() {
        let (mut editor, source, target, clip) = audio_fixture();
        editor
            .execute(Command::SetClipLooping {
                clip,
                looping: true,
            })
            .expect("looping");
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 120,
                fade_out: 240,
            })
            .expect("fade");
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 16 })
            .expect("phase");
        let original = editor.project().clip(clip).expect("original").clone();
        let Created::Clip(copy) = editor
            .execute(Command::CopyClipTo {
                clip,
                track: target,
                start: 120_000,
            })
            .expect("copy")
        else {
            panic!("copy result")
        };
        let copied = editor.project().clip(copy).expect("copy exists");
        assert_ne!(copy, clip);
        assert_eq!(copied.asset(), original.asset());
        assert_eq!(copied.source_offset(), original.source_offset());
        assert_eq!(copied.source_length(), original.source_length());
        assert_eq!(copied.source_phase(), original.source_phase());
        assert_eq!(copied.fade(), original.fade());
        assert_eq!(
            editor.project().track(source).unwrap().clips()[0].id(),
            clip
        );
    }

    #[test]
    fn audio_copy_replaces_only_destination_overlap_and_keeps_source_clip() {
        let (mut editor, source, target, clip) = audio_fixture();
        let asset = editor.project().clip(clip).expect("source").asset();
        let Created::Clip(victim) = editor
            .execute(Command::PlaceClip {
                track: target,
                asset,
                start: 120_000,
                duration: 96_000,
                source_offset: 0,
                source_length: 96_000,
                looping: false,
            })
            .expect("destination victim")
        else {
            panic!("clip result")
        };
        let Created::Clip(copy) = editor
            .execute(Command::CopyClipTo {
                clip,
                track: target,
                start: 144_000,
            })
            .expect("copy")
        else {
            panic!("copy result")
        };
        let destination = editor.project().track(target).expect("target").clips();
        assert_eq!(destination.len(), 2);
        assert!(destination.iter().any(|item| {
            item.id() == victim && item.start() == 120_000 && item.duration() == 24_000
        }));
        assert!(destination.iter().any(|item| {
            item.id() == copy && item.start() == 144_000 && item.duration() == 96_000
        }));
        assert_eq!(
            editor.project().track(source).unwrap().clips()[0].id(),
            clip
        );
        assert!(editor.undo());
        assert!(editor.project().track(target).unwrap().clips()[0].id() == victim);
    }

    #[test]
    fn audio_copy_can_insert_new_track_and_undo_atomically() {
        let (mut editor, source, _, clip) = audio_fixture();
        let before = editor.project().tracks().len();
        let result = editor
            .execute(Command::CopyClipToNewTrack {
                initial_gain: None,
                clip,
                name: "track 3".into(),
                before_track: None,
                start: 144_000,
            })
            .expect("copy to new track");
        let Created::Track(created_track) = result else {
            panic!("track result")
        };
        assert_eq!(editor.project().tracks().len(), before + 1);
        assert_eq!(
            editor.project().track(source).unwrap().clips()[0].id(),
            clip
        );
        assert_eq!(
            editor.project().track(created_track).unwrap().clips().len(),
            1
        );
        assert!(editor.undo());
        assert_eq!(editor.project().tracks().len(), before);
    }

    #[test]
    fn audio_clip_new_track_gain_is_atomic_for_move_and_copy() {
        for copy in [false, true] {
            let (mut editor, source, anchor, clip) = audio_fixture();
            let before = editor.project().clone();
            let command = |gain| {
                if copy {
                    Command::CopyClipToNewTrack {
                        clip,
                        name: "new".into(),
                        before_track: Some(anchor),
                        start: 144_000,
                        initial_gain: Some(gain),
                    }
                } else {
                    Command::MoveClipToNewTrack {
                        clip,
                        name: "new".into(),
                        before_track: Some(anchor),
                        start: 144_000,
                        initial_gain: Some(gain),
                    }
                }
            };
            for invalid in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
                assert!(editor.execute(command(invalid)).is_err());
                assert_eq!(editor.project(), &before);
            }
            let refusal =
                editor.execute_prepared(command(0.25), |_| Err::<(), _>(Invalid("refused")));
            assert!(refusal.is_err());
            assert_eq!(editor.project(), &before);
            let Created::Track(created) = editor.execute(command(0.25)).unwrap() else {
                panic!("track")
            };
            let target = editor.project().track(created).unwrap();
            assert_eq!(target.gain(), 0.25);
            assert_eq!(target.clips().len(), 1);
            assert_eq!(target.clips()[0].id() == clip, !copy);
            assert_eq!(
                editor.project().track(source).unwrap().clips().len(),
                usize::from(copy)
            );
            let after = editor.project().clone();
            assert!(editor.undo());
            let mut restored = editor.project().snapshot();
            restored.next_id = before.snapshot().next_id;
            assert_eq!(restored, before.snapshot());
            assert!(editor.redo());
            assert_eq!(editor.project(), &after);
        }
    }

    #[test]
    fn midi_copy_allocates_fresh_clip_and_note_ids_and_keeps_loop_metadata() {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let Created::Track(source_track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "target".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: source_track,
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(4).expect("length"),
            })
            .expect("midi clip")
        else {
            panic!("clip result")
        };
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: BeatTime::from_quarters(8).expect("source length"),
                source_start: BeatTime::from_quarters(1).expect("source start"),
                loop_start: BeatTime::from_quarters(2).expect("loop start"),
                loop_end: BeatTime::from_quarters(6).expect("loop end"),
                looping: true,
                length: BeatTime::from_quarters(4).expect("clip length"),
            })
            .expect("loop source");
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .expect("note")
        else {
            panic!("note result")
        };
        let Created::Clip(copy) = editor
            .execute(Command::CopyMidiClipTo {
                clip,
                track: target,
                start: BeatTime::from_quarters(8).expect("start"),
            })
            .expect("copy")
        else {
            panic!("copy result")
        };
        let copied = editor.project().midi_clip(copy).expect("copied");
        let copied_note = copied.notes().first().expect("copied note").id();
        assert_ne!(copy, clip);
        assert_ne!(copied_note, note);
        assert_eq!(copied.notes()[0].pitch(), 60);
        assert!(copied.looping());
        assert_eq!(copied.source_start(), BeatTime::from_quarters(1).unwrap());
        assert_eq!(copied.loop_start(), BeatTime::from_quarters(2).unwrap());
        assert_eq!(copied.loop_end(), BeatTime::from_quarters(6).unwrap());
        assert_eq!(
            editor.project().midi_clip(clip).unwrap().notes()[0].id(),
            note
        );
    }
}
