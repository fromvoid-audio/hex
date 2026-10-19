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
        f.editor
            .execute(Command::SetAudioClipWarp {
                clip: f.audio,
                settings: Some(crate::WarpSettings {
                    mode: crate::WarpMode::RePitch,
                    markers: vec![
                        crate::WarpMarker {
                            source_frame: 0,
                            beat: BeatTime::ZERO,
                        },
                        crate::WarpMarker {
                            source_frame: 48_000,
                            beat: BeatTime::ONE,
                        },
                        crate::WarpMarker {
                            source_frame: 96_000,
                            beat: BeatTime::from_quarters(4).unwrap(),
                        },
                    ],
                }),
            })
            .unwrap();
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
        let old_audio = before.clip(f.audio).unwrap();
        let new_audio = after.clip(f.audio).unwrap();
        assert_eq!(new_audio.asset(), old_audio.asset());
        assert_eq!(new_audio.duration(), old_audio.duration());
        assert_eq!(new_audio.source_offset(), old_audio.source_offset());
        assert_eq!(new_audio.source_length(), old_audio.source_length());
        assert_eq!(new_audio.source_phase(), old_audio.source_phase());
        assert_eq!(new_audio.looping(), old_audio.looping());
        assert_eq!(new_audio.fade(), old_audio.fade());
        assert_eq!(new_audio.gain(), old_audio.gain());
        assert_eq!(new_audio.name(), old_audio.name());
        assert_eq!(new_audio.flavor(), old_audio.flavor());
        assert_eq!(
            new_audio.audio_edit().unwrap().domain.warp,
            old_audio.audio_edit().unwrap().domain.warp
        );
        let old_midi = before.midi_clip(f.midi).unwrap();
        let new_midi = after.midi_clip(f.midi).unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&old_midi.source, &new_midi.source),
            "immutable authored notes/expressions are shared, not rewritten"
        );
        assert_eq!(new_midi.length(), old_midi.length());
        assert_eq!(new_midi.name(), old_midi.name());
        assert_eq!(new_midi.flavor(), old_midi.flavor());
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

#[test]
fn canonical_subframe_midi_member_is_retained_in_whole_group_move() {
    let mut f = fixture();
    let start = f.editor.project().midi_clip(f.midi).unwrap().start();
    f.editor
        .execute(Command::TrimMidiClipEdge {
            clip: f.midi,
            edge: crate::TrimEdge::Right,
            beat: BeatTime::from_raw(start.raw() + 1),
        })
        .unwrap();
    let before = f.editor.project().clone();
    let destination = BeatTime::from_quarters(9).unwrap();
    // The existing canonical single-clip overwrite accepts this positive beat
    // extent despite both endpoints projecting to the same integer frame.
    let mut control = Editor::new(before.clone(), 8).unwrap();
    control
        .execute(Command::MoveMidiClipWithOverwrite {
            clip: f.midi,
            track: f.tracks[1],
            start: destination,
        })
        .unwrap();
    assert_eq!(
        control.project().midi_clip(f.midi).unwrap().length().raw(),
        1
    );
    let edit = command(&f, f.audio, f.tracks[0], ClipGroupPosition::Audio(72_000));
    let captured_source = std::sync::Arc::new(before.clone());
    let mut captured =
        crate::ClipGroupMovePreview::capture(&captured_source, &[f.audio, f.midi], f.midi).unwrap();
    captured
        .update(
            &captured_source,
            f.tracks[1],
            ClipGroupPosition::Midi(destination),
        )
        .unwrap();
    assert_capture_matches_plan(
        captured.previews(),
        &before
            .preview_clip_group_move(
                &[f.audio, f.midi],
                f.midi,
                f.tracks[1],
                ClipGroupPosition::Midi(destination),
            )
            .unwrap(),
    );
    f.editor
        .execute(edit)
        .expect("a legal subframe member must not block or disappear from the group");
    let after = f.editor.project();
    assert_eq!(after.clip(f.audio).unwrap().start(), 72_000);
    let midi = after.midi_clip(f.midi).unwrap();
    assert_eq!(midi.start(), destination);
    assert_eq!(midi.length().raw(), 1);
    assert!(std::sync::Arc::ptr_eq(
        &midi.source,
        &before.midi_clip(f.midi).unwrap().source
    ));
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), *after);
    assert!(f.editor.undo());
    assert_eq!(f.editor.project(), &before);
}

fn copied_group_preserves_overlapped_selected_originals(kind: u8) {
    let mut f = fixture();
    let track = f.tracks[0];
    let (a, b, victim, destination) = match kind {
        0 => {
            let Created::Clip(b) = f
                .editor
                .execute(Command::CopyClipTo {
                    clip: f.audio,
                    track,
                    start: 144_000,
                })
                .unwrap()
            else {
                panic!("copy")
            };
            let Created::Clip(victim) = f
                .editor
                .execute(Command::CopyClipTo {
                    clip: f.audio,
                    track,
                    start: 240_000,
                })
                .unwrap()
            else {
                panic!("victim")
            };
            (f.audio, b, victim, ClipGroupPosition::Audio(144_000))
        }
        1 => {
            let track = f.tracks[1];
            let Created::Clip(b) = f
                .editor
                .execute(Command::CopyMidiClipTo {
                    clip: f.midi,
                    track,
                    start: BeatTime::from_quarters(10).unwrap(),
                })
                .unwrap()
            else {
                panic!("copy")
            };
            let Created::Clip(victim) = f
                .editor
                .execute(Command::CopyMidiClipTo {
                    clip: f.midi,
                    track,
                    start: BeatTime::from_quarters(12).unwrap(),
                })
                .unwrap()
            else {
                panic!("victim")
            };
            (
                f.midi,
                b,
                victim,
                ClipGroupPosition::Midi(BeatTime::from_quarters(10).unwrap()),
            )
        }
        _ => {
            f.editor
                .execute(Command::MoveMidiClip {
                    clip: f.midi,
                    track,
                    start: BeatTime::from_quarters(6).unwrap(),
                })
                .unwrap();
            let asset = f.editor.project().clip(f.audio).unwrap().asset();
            let Created::Clip(victim) = f
                .editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start: 240_000,
                    duration: 48_000,
                    source_offset: 0,
                    source_length: 48_000,
                    looping: false,
                })
                .unwrap()
            else {
                panic!("victim")
            };
            (f.audio, f.midi, victim, ClipGroupPosition::Audio(144_000))
        }
    };
    let track = if kind == 1 { f.tracks[1] } else { track };
    let before = f.editor.project().clone();
    f.editor
        .execute(Command::MoveClipsWithOverwrite {
            clips: vec![a, b],
            anchor: a,
            track,
            position: destination,
            copy: true,
        })
        .unwrap();
    for id in [a, b] {
        assert_eq!(
            f.editor.project().clip(id),
            before.clip(id),
            "selected audio original must retain exact metadata/ID: kind={kind} id={id:?}"
        );
        assert_eq!(
            f.editor.project().midi_clip(id),
            before.midi_clip(id),
            "selected MIDI original must retain exact metadata/ID: kind={kind} id={id:?}"
        );
    }
    assert!(
        f.editor.project().clip(victim).is_none() && f.editor.project().midi_clip(victim).is_none(),
        "unselected destination is still overwritten"
    );
    let after = f.editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(f.editor.undo());
    // Undo restores content/IDs, but canonical allocation high-water never
    // rolls back newly allocated copy/note identities.
    let mut restored = before.snapshot();
    assert!(after.next_id > restored.next_id);
    restored.next_id = after.next_id;
    assert_eq!(f.editor.project().snapshot(), restored);
    assert!(f.editor.redo());
    assert_eq!(f.editor.project().snapshot(), after);
}
#[test]
fn copy_overlap_preserves_audio_selected_originals() {
    copied_group_preserves_overlapped_selected_originals(0);
}
#[test]
fn copy_overlap_preserves_midi_selected_originals() {
    copied_group_preserves_overlapped_selected_originals(1);
}
#[test]
fn copy_overlap_preserves_mixed_selected_originals() {
    copied_group_preserves_overlapped_selected_originals(2);
}

fn capture_project() -> std::sync::Arc<Project> {
    let mut f = fixture();
    let settings = crate::audio_edit::default_audio_warp(
        f.editor.project().clip(f.audio).unwrap(),
        f.editor.project().tempo_map(),
        crate::WarpMode::RePitch,
    )
    .unwrap();
    f.editor
        .execute(Command::SetAudioClipWarp {
            clip: f.audio,
            settings: Some(settings),
        })
        .unwrap();
    f.editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(12).unwrap(),
            tempo: crate::Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    std::sync::Arc::new(f.editor.project().clone())
}
fn assert_capture_matches_plan(actual: &[ClipGroupPreview], expected: &[ClipGroupPreview]) {
    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(expected) {
        assert_eq!(a.track, b.track);
        match (&a.content, &b.content) {
            (ClipGroupContent::Audio(a), ClipGroupContent::Audio(b)) => assert_eq!(a, b),
            (ClipGroupContent::Midi(a), ClipGroupContent::Midi(b)) => assert_eq!(a, b),
            _ => panic!("member kind/order changed"),
        }
    }
}
#[test]
fn capture_projection_matches_canonical_and_recovers_absolute_original_after_invalid_and_roundtrips()
 {
    use crate::ClipGroupMovePreview;
    let project = capture_project();
    let audio = project.tracks()[0].clips()[0].id();
    let midi = project.tracks()[1].midi_clips()[0].id();
    assert!(crate::ClipGroupMovePreview::capture(&project, &[], audio).is_err());
    assert!(crate::ClipGroupMovePreview::capture(&project, &[audio, audio], audio).is_err());
    assert!(crate::ClipGroupMovePreview::capture(&project, &[audio], midi).is_err());
    assert!(
        crate::ClipGroupMovePreview::capture(&project, &[audio, ClipId(u64::MAX)], audio).is_err()
    );
    assert!(
        crate::ClipGroupMovePreview::capture(&project, &vec![audio; MAX_CLIPS + 1], audio).is_err()
    );
    for anchor in [audio, midi] {
        let (track, a, b, invalid) = if anchor == audio {
            (
                project.tracks()[0].id(),
                ClipGroupPosition::Audio(project.clip(audio).unwrap().start()),
                ClipGroupPosition::Audio(288_123),
                project.tracks()[2].id(),
            )
        } else {
            (
                project.tracks()[1].id(),
                ClipGroupPosition::Midi(project.midi_clip(midi).unwrap().start()),
                ClipGroupPosition::Midi(BeatTime::from_raw(BeatTime::ONE.raw() * 16 + 17)),
                project.tracks()[0].id(),
            )
        };
        let ids = [audio, midi];
        let mut capture = ClipGroupMovePreview::capture(&project, &ids, anchor).unwrap();
        assert_eq!(
            std::sync::Arc::strong_count(&project),
            1,
            "capture must retain only Weak source"
        );
        assert!(capture.previews().is_empty());
        for position in [
            a,
            b,
            a,
            if anchor == audio {
                ClipGroupPosition::Audio(0)
            } else {
                ClipGroupPosition::Midi(BeatTime::ZERO)
            },
            a,
        ] {
            let expected = project
                .preview_clip_group_move(&ids, anchor, track, position)
                .unwrap();
            capture.update(&project, track, position).unwrap();
            assert_capture_matches_plan(capture.previews(), &expected);
        }
        assert!(capture.update(&project, invalid, b).is_err());
        assert!(
            capture.previews().is_empty(),
            "partial projection cannot become visible"
        );
        capture.update(&project, track, a).unwrap();
        assert_capture_matches_plan(
            capture.previews(),
            &project
                .preview_clip_group_move(&ids, anchor, track, a)
                .unwrap(),
        );
        assert!(std::ptr::eq(
            capture.original_audio(&project, 0).unwrap(),
            project.clip(audio).unwrap()
        ));
        capture.invalidate();
        assert!(capture.previews().is_empty());
        capture.update(&project, track, b).unwrap();
        assert_capture_matches_plan(
            capture.previews(),
            &project
                .preview_clip_group_move(&ids, anchor, track, b)
                .unwrap(),
        );
    }
}
#[test]
fn capture_source_witness_refuses_equal_replacement_and_cow_reorder_delete_without_strong_owner() {
    use crate::ClipGroupMovePreview;
    for mutation in 0..3 {
        let mut project = capture_project();
        let ids = [
            project.tracks()[0].clips()[0].id(),
            project.tracks()[1].midi_clips()[0].id(),
        ];
        let track = project.tracks()[0].id();
        let mut capture = ClipGroupMovePreview::capture(&project, &ids, ids[0]).unwrap();
        capture
            .update(&project, track, ClipGroupPosition::Audio(72_000))
            .unwrap();
        let same = std::sync::Arc::clone(&project);
        assert!(capture.matches_source(&same));
        capture
            .update(&same, track, ClipGroupPosition::Audio(96_000))
            .unwrap();
        drop(same);
        assert_eq!(std::sync::Arc::strong_count(&project), 1);
        assert!(
            std::sync::Arc::get_mut(&mut project).is_none(),
            "Weak prevents silent in-place mutation"
        );
        if mutation == 0 {
            project = std::sync::Arc::new((*project).clone());
        } else if mutation == 1 {
            std::sync::Arc::make_mut(&mut project).tracks.swap(0, 1);
        } else {
            std::sync::Arc::make_mut(&mut project).tracks[0]
                .content
                .audio_clips
                .clear();
        }
        assert!(!capture.matches_source(&project));
        assert!(capture.original_audio(&project, 0).is_none());
        assert!(
            capture
                .update(&project, track, ClipGroupPosition::Audio(72_000))
                .is_err()
        );
        assert!(capture.previews().is_empty());
    }
}
#[test]
fn capture_rejects_retained_musical_preview_shell_alias_then_recovers_without_clone_reset() {
    let project = capture_project();
    let ids = [
        project.tracks()[0].clips()[0].id(),
        project.tracks()[1].midi_clips()[0].id(),
    ];
    let track = project.tracks()[0].id();
    let mut capture = crate::ClipGroupMovePreview::capture(&project, &ids, ids[0]).unwrap();
    capture
        .update(&project, track, ClipGroupPosition::Audio(72_000))
        .unwrap();
    let alias = capture.previews()[0].clone();
    assert!(
        capture
            .update(&project, track, ClipGroupPosition::Audio(96_000))
            .is_err()
    );
    assert!(capture.previews().is_empty());
    drop(alias);
    capture
        .update(&project, track, ClipGroupPosition::Audio(96_000))
        .unwrap();
    assert_capture_matches_plan(
        capture.previews(),
        &project
            .preview_clip_group_move(&ids, ids[0], track, ClipGroupPosition::Audio(96_000))
            .unwrap(),
    );
}
