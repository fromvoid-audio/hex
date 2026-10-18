//! Complete selection projection and canonical atomic history; no media/backend fixture.
use super::*;
use crate::{ClipFlavor, Command, Editor};

struct Fixture {
    editor: Editor,
    tracks: [TrackId; 3],
    audio: ClipId,
    midi: ClipId,
}
fn fixture() -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported {
        track: a,
        clip: audio,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "model-only.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::MoveClip {
            clip: audio,
            track: a,
            start: 48_000,
        })
        .unwrap();
    editor
        .execute(Command::SetClipLooping {
            clip: audio,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip: audio,
            fade_in: 120,
            fade_out: 240,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase {
            clip: audio,
            phase: 16,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFlavor {
            clip: audio,
            flavor: ClipFlavor::Accent,
        })
        .unwrap();
    editor
        .execute(Command::RenameClip {
            clip: audio,
            name: "kept audio".into(),
        })
        .unwrap();
    let mut tracks = vec![a];
    for name in ["middle", "last"] {
        let Created::Track(id) = editor
            .execute(Command::AddMidiTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        tracks.push(id);
    }
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track: tracks[1],
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::RenameClip {
            clip: midi,
            name: "kept midi".into(),
        })
        .unwrap();
    Fixture {
        editor,
        tracks: tracks.try_into().unwrap(),
        audio,
        midi,
    }
}
fn command(f: &Fixture, anchor: ClipId, track: TrackId, position: ClipGroupPosition) -> Command {
    Command::MoveClipsWithOverwrite {
        clips: vec![f.audio, f.midi],
        anchor,
        track,
        position,
        copy: false,
    }
}
#[test]
fn mixed_group_either_anchor_preserves_offsets_payloads_ids_and_one_history_step() {
    for midi_anchor in [false, true] {
        let mut f = fixture();
        let before = f.editor.project().clone();
        let (anchor, target, position) = if midi_anchor {
            (
                f.midi,
                f.tracks[2],
                ClipGroupPosition::Midi(BeatTime::from_quarters(12).unwrap()),
            )
        } else {
            (f.audio, f.tracks[1], ClipGroupPosition::Audio(144_000))
        };
        let plan = before
            .preview_clip_group_move(&[f.midi, f.audio], anchor, target, position)
            .unwrap();
        let edit = command(&f, anchor, target, position);
        f.editor.execute(edit).unwrap();
        let after = f.editor.project().clone();
        for item in plan {
            let row = after.track(item.track).unwrap();
            match item.content {
                ClipGroupContent::Audio(c) => assert!(row.clips().contains(&c)),
                ClipGroupContent::Midi(c) => assert!(row.midi_clips().contains(&c)),
            }
        }
        assert_eq!(after.track(f.tracks[0]).unwrap().clips().len(), 0);
        assert_eq!(after.track(f.tracks[1]).unwrap().midi_clips().len(), 0);
        assert_eq!(after.clip(f.audio).unwrap().start(), 144_000);
        assert_eq!(
            after.midi_clip(f.midi).unwrap().start(),
            BeatTime::from_quarters(12).unwrap()
        );
        assert_eq!(
            after.assets().collect::<Vec<_>>(),
            before.assets().collect::<Vec<_>>()
        );
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
        assert!(f.editor.undo());
        assert_eq!(f.editor.project(), &before);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &after);
    }
}
#[test]
fn left_bound_clamps_one_shared_delta_for_audio_and_midi_anchors() {
    for midi_anchor in [false, true] {
        let f = fixture();
        let (anchor, target, position) = if midi_anchor {
            (f.midi, f.tracks[1], ClipGroupPosition::Midi(BeatTime::ZERO))
        } else {
            (f.audio, f.tracks[0], ClipGroupPosition::Audio(0))
        };
        let plan = f
            .editor
            .project()
            .preview_clip_group_move(&[f.audio, f.midi], anchor, target, position)
            .unwrap();
        for item in plan {
            match item.content {
                ClipGroupContent::Audio(c) => assert_eq!(c.start(), 0),
                ClipGroupContent::Midi(c) => {
                    assert_eq!(c.start(), BeatTime::from_quarters(6).unwrap())
                }
            }
        }
    }
}
#[test]
fn invalid_group_members_or_vertical_fit_refuse_whole_edit_and_preserve_redo() {
    let mut f = fixture();
    f.editor
        .execute(Command::RenameClip {
            clip: f.audio,
            name: "redo name".into(),
        })
        .unwrap();
    assert!(f.editor.undo());
    let before = f.editor.project().clone();
    for ids in [
        vec![],
        vec![f.audio, f.audio],
        vec![f.audio, ClipId(u64::MAX)],
        vec![f.midi],
    ] {
        assert!(
            f.editor
                .execute(Command::MoveClipsWithOverwrite {
                    clips: ids,
                    anchor: f.audio,
                    track: f.tracks[1],
                    position: ClipGroupPosition::Audio(144_000),
                    copy: false
                })
                .is_err()
        );
        assert_eq!(f.editor.project(), &before);
        assert!(f.editor.can_redo());
    }
    // Moving the lower selected member past the last real track cannot move
    // only the picked audio member or invent an extra destination track.
    let edit = command(&f, f.audio, f.tracks[2], ClipGroupPosition::Audio(144_000));
    assert!(f.editor.execute(edit).is_err());
    assert_eq!(f.editor.project(), &before);
    assert!(f.editor.can_redo());
    assert!(f.editor.redo());
    assert_eq!(
        f.editor.project().clip(f.audio).unwrap().name(),
        "redo name"
    );
}
#[test]
fn prepare_failure_preserves_complete_group_ids_project_and_both_history_stacks() {
    let mut f = fixture();
    f.editor
        .execute(Command::RenameClip {
            clip: f.audio,
            name: "redo name".into(),
        })
        .unwrap();
    assert!(f.editor.undo());
    let before = f.editor.project().clone();
    let history = (f.editor.can_undo(), f.editor.can_redo());
    let edit = command(&f, f.audio, f.tracks[1], ClipGroupPosition::Audio(144_000));
    let mut calls = 0;
    assert!(
        f.editor
            .execute_validated(edit, |candidate| {
                calls += 1;
                assert_eq!(candidate.clip(f.audio).unwrap().start(), 144_000);
                assert_eq!(
                    candidate.midi_clip(f.midi).unwrap().start(),
                    BeatTime::from_quarters(12).unwrap()
                );
                Err::<(), Invalid>(Invalid("intentional prepared candidate refusal"))
            })
            .is_err()
    );
    assert_eq!(calls, 1);
    assert_eq!(f.editor.project(), &before);
    assert_eq!((f.editor.can_undo(), f.editor.can_redo()), history);
    assert!(f.editor.redo());
    assert_eq!(
        f.editor.project().clip(f.audio).unwrap().name(),
        "redo name"
    );
}
#[test]
fn identical_group_move_preserves_redo_and_canonical_data() {
    let mut f = fixture();
    f.editor
        .execute(Command::RenameClip {
            clip: f.audio,
            name: "redo name".into(),
        })
        .unwrap();
    assert!(f.editor.undo());
    let before = f.editor.project().clone();
    let edit = command(&f, f.audio, f.tracks[0], ClipGroupPosition::Audio(48_000));
    f.editor.execute(edit).unwrap();
    assert_eq!(f.editor.project(), &before);
    assert!(f.editor.can_redo());
}

#[test]
fn clip_limit_uses_complete_final_group_and_refuses_atomically_before_preparation() {
    for split_victim in [false, true] {
        let mut f = fixture();
        let mut project = f.editor.project().clone();
        let asset = project.clip(f.audio).unwrap().asset();
        if split_victim {
            // The original group does not overlap this later victim. Its
            // destination is strictly inside, adding one retained fragment.
            project
                .apply(Command::PlaceClip {
                    track: f.tracks[0],
                    asset,
                    start: 168_000,
                    duration: 312_000,
                    source_offset: 0,
                    source_length: 96_000,
                    looping: true,
                })
                .unwrap();
        }
        let count = project
            .tracks()
            .iter()
            .map(|t| t.clips().len() + t.midi_clips().len())
            .sum::<usize>();
        let template = project.clip(f.audio).unwrap().clone();
        project
            .audio_clips_mut(f.tracks[2])
            .unwrap()
            .try_reserve_exact(MAX_CLIPS - count)
            .unwrap();
        for index in 0..MAX_CLIPS - count {
            let mut clip = template.clone();
            clip.id = ClipId(project.allocate_id().unwrap());
            clip.start = 10_000_000 + index as u64 * 2;
            clip.duration = 1;
            clip.source_offset = 0;
            clip.source_length = 1;
            clip.source_phase = 0;
            clip.looping = false;
            clip.edit = None;
            clip.fade = crate::ClipFade::neutral(1).unwrap();
            project.audio_clips_mut(f.tracks[2]).unwrap().push(clip);
        }
        assert_eq!(Project::from_snapshot(project.snapshot()).unwrap(), project);
        f.editor = Editor::new(project, 8).unwrap();
        let before = f.editor.project().clone();
        let edit = command(
            &f,
            f.audio,
            f.tracks[0],
            ClipGroupPosition::Audio(if split_victim { 192_000 } else { 72_000 }),
        );
        let mut preparations = 0;
        let result = f.editor.execute_validated(edit, |candidate| {
            preparations += 1;
            assert_eq!(
                candidate
                    .tracks()
                    .iter()
                    .map(|t| t.clips().len() + t.midi_clips().len())
                    .sum::<usize>(),
                MAX_CLIPS
            );
            Ok::<(), Invalid>(())
        });
        if split_victim {
            assert!(result.is_err());
            assert_eq!(preparations, 0);
            assert_eq!(f.editor.project(), &before);
            assert!(!f.editor.can_undo() && !f.editor.can_redo());
        } else {
            result.unwrap();
            assert_eq!(preparations, 1);
            assert!(f.editor.undo());
            assert_eq!(f.editor.project(), &before);
            assert!(f.editor.redo());
        }
    }
}
