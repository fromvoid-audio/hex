//! Complete selection projection and canonical atomic history; no media/backend fixture.
use super::*;
use crate::{ClipFlavor, Command, Editor, MAX_NAME_BYTES, MAX_NOTES, NoteId};

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
fn audio_name_candidates_preserve_source_history_and_owned_snapshots() {
    let mut f = fixture();
    let original = f.editor.project().clone();
    let original_snapshot = original.snapshot();
    let rename = |name: &str| Command::RenameClip {
        clip: f.audio,
        name: name.into(),
    };
    let changed = "Lead Äudio 01";
    f.editor.execute(rename(changed)).unwrap();
    assert_eq!(f.editor.project().clip_name(f.audio), Some(changed));
    assert_eq!(original.clip_name(f.audio), Some("kept audio"));
    assert_eq!(original.snapshot(), original_snapshot);
    let renamed = f.editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(renamed.clone()).unwrap(),
        *f.editor.project()
    );
    assert!(f.editor.undo());
    assert_eq!(f.editor.project().snapshot(), original_snapshot);
    // A new allocation containing the same label is still a semantic no-op.
    f.editor.execute(rename("kept audio")).unwrap();
    assert!(f.editor.can_redo());
    let refused: Result<(Created, ()), Invalid> =
        f.editor
            .execute_prepared_transition(rename("refused label"), |source, candidate| {
                assert_eq!(source.clip_name(f.audio), Some("kept audio"));
                assert_eq!(candidate.clip_name(f.audio), Some("refused label"));
                Err(Invalid("test preparation refusal"))
            });
    assert!(refused.is_err());
    assert_eq!(f.editor.project().snapshot(), original_snapshot);
    assert!(f.editor.can_redo());
    assert!(f.editor.redo());
    assert_eq!(f.editor.project().snapshot(), renamed);
    assert_eq!(original.snapshot(), original_snapshot);
}
#[test]
fn retained_root_labels_are_bounded_and_independent_of_the_project() {
    let f = fixture();
    let project = f.editor.project();
    let before = project.snapshot();
    let tempo = std::sync::Arc::new(project.tempo_map().clone());
    let mut snapshot = project.clip(f.audio).unwrap().retained_snapshot();
    snapshot.name = "Ā".repeat(MAX_NAME_BYTES / "Ā".len());
    let root = project
        .restore_retained_audio_root(1, snapshot.clone(), tempo.clone())
        .unwrap();
    assert_eq!(root.clip.retained_snapshot(), snapshot);
    for name in [String::new(), "\n".into(), "x".repeat(MAX_NAME_BYTES + 1)] {
        let mut invalid = snapshot.clone();
        invalid.name = name;
        assert!(
            project
                .restore_retained_audio_root(1, invalid, tempo.clone())
                .is_err()
        );
    }
    assert_eq!(project.snapshot(), before);
    assert_eq!(root.clip.retained_snapshot(), snapshot);
}

#[test]
fn retained_root_nesting_has_a_bounded_depth() {
    use crate::{AudioClipEdit, AudioEvent, AudioEvents};
    use std::sync::Arc;

    let f = fixture();
    let project = f.editor.project();
    let clip = project.clip(f.audio).unwrap();
    let tempo = Arc::new(project.tempo_map().clone());
    let mut root = project
        .restore_retained_audio_root(1, clip.retained_snapshot(), tempo.clone())
        .unwrap();
    for depth in 2..=crate::retained_audio::MAX_RETAINED_AUDIO_DEPTH + 1 {
        let mut snapshot = clip.retained_snapshot();
        snapshot.looping = false;
        snapshot.source_phase = 0;
        let mut edit = AudioClipEdit::capture(clip);
        Arc::make_mut(&mut edit.domain).looping = false;
        Arc::make_mut(&mut edit.domain).source_phase = 0;
        Arc::make_mut(&mut edit.domain).events = Some(Arc::new(AudioEvents {
            next_id: 2,
            events: vec![AudioEvent {
                active: true,
                gain: crate::ClipGain::UNITY,
                source: 1,
                id: 1,
                start: 0,
                end: clip.duration(),
                source_start: 0,
                fade: None,
            }],
            sources: vec![root.clone()],
        }));
        snapshot.edit = Some(edit);
        let result = project.restore_retained_audio_root(1, snapshot, tempo.clone());
        if depth <= crate::retained_audio::MAX_RETAINED_AUDIO_DEPTH {
            root = result.unwrap();
            assert_eq!(root.clip.referenced_assets().count(), depth);
        } else {
            assert_eq!(
                result.unwrap_err().0,
                "retained audio nesting limit reached"
            );
            break;
        }
    }
    let before = root.clip.audio_events();
    let changed = before
        .edited(
            crate::AudioEventEdit::SetRetainedPathActive {
                sources: vec![1; crate::retained_audio::MAX_RETAINED_AUDIO_DEPTH - 1],
                events: vec![1],
                active: false,
            },
            root.clip.audio_edit().unwrap().domain.clock.length(),
        )
        .unwrap();
    let mut events = &changed;
    for _ in 0..crate::retained_audio::MAX_RETAINED_AUDIO_DEPTH - 1 {
        events = events.sources[0]
            .clip
            .audio_edit()
            .unwrap()
            .domain
            .events
            .as_ref()
            .unwrap();
    }
    assert!(!events.events[0].active);
    assert!(before.events[0].active);
    assert!(
        before
            .edited(
                crate::AudioEventEdit::SetRetainedPathActive {
                    sources: vec![1, 999],
                    events: vec![1],
                    active: false,
                },
                root.clip.audio_edit().unwrap().domain.clock.length(),
            )
            .is_err()
    );
    assert_eq!(root.clip.audio_events(), before);
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

fn copied_group_overwrites_overlapped_old_selected_members(kind: u8) {
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
    assert_eq!(f.editor.project().clip(a), before.clip(a));
    assert_eq!(f.editor.project().midi_clip(a), before.midi_clip(a));
    assert!(
        f.editor.project().clip(b).is_none() && f.editor.project().midi_clip(b).is_none(),
        "incoming copy replaces the overlapped old selected member"
    );
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
fn copy_overlap_overwrites_audio_old_member() {
    copied_group_overwrites_overlapped_old_selected_members(0);
}
#[test]
fn copy_overlap_overwrites_midi_old_member() {
    copied_group_overwrites_overlapped_old_selected_members(1);
}
#[test]
fn copy_overlap_overwrites_mixed_old_member() {
    copied_group_overwrites_overlapped_old_selected_members(2);
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

#[test]
fn review_full_capacity_copy_accounts_later_mixed_victims_before_first_split() {
    let mut f = fixture();
    let mut project = f.editor.project().clone();
    let asset = project.clip(f.audio).unwrap().asset();
    // First projected audio copy192k..288k splits this victim (+1).
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
    // The later projected MIDI copy336k..384k removes THREE mixed victims.
    project
        .apply(Command::PlaceMidiClip {
            track: f.tracks[1],
            start: BeatTime::from_quarters(14).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap();
    for start in [360_000, 372_000] {
        project
            .apply(Command::PlaceClip {
                track: f.tracks[1],
                asset,
                start,
                duration: 6_000,
                source_offset: 0,
                source_length: 6_000,
                looping: false,
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
    let mut preparations = 0;
    let result = f.editor.execute_validated(
        Command::MoveClipsWithOverwrite {
            clips: vec![f.audio, f.midi],
            anchor: f.audio,
            track: f.tracks[0],
            position: ClipGroupPosition::Audio(192_000),
            copy: true,
        },
        |candidate| {
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
        },
    );
    if let Err(error) = &result {
        assert_eq!(f.editor.project(), &before);
        assert_eq!(preparations, 0);
        assert!(!f.editor.can_undo() && !f.editor.can_redo());
        eprintln!(
            "REVIEW_CAPACITY actual_error={error:?}; final occupancy MAX +1split -3victims +2copies =MAX"
        );
    }
    if result.is_err() {
        let mut lower = before.clone();
        lower.audio_clips_mut(f.tracks[2]).unwrap().pop().unwrap();
        let mut control = Editor::new(lower, 8).unwrap();
        control
            .execute(Command::MoveClipsWithOverwrite {
                clips: vec![f.audio, f.midi],
                anchor: f.audio,
                track: f.tracks[0],
                position: ClipGroupPosition::Audio(192_000),
                copy: true,
            })
            .unwrap();
        assert_eq!(
            control
                .project()
                .tracks()
                .iter()
                .map(|t| t.clips().len() + t.midi_clips().len())
                .sum::<usize>(),
            MAX_CLIPS - 1
        );
        assert_eq!(control.project().clip(f.audio), before.clip(f.audio));
        assert_eq!(
            control.project().midi_clip(f.midi),
            before.midi_clip(f.midi)
        );
        eprintln!(
            "REVIEW_CAPACITY_MINUS_ONE_CONTROL success final={} originals retained; same split/victims/copies",
            MAX_CLIPS - 1
        );
    }
    result.expect("legal final occupancy must not refuse at an intermediate split");
    assert_eq!(preparations, 1);
    assert!(f.editor.undo());
    let mut undone = before.clone();
    undone.next_id = f.editor.project().next_id;
    assert_eq!(f.editor.project(), &undone);
    assert!(f.editor.redo());
}

#[test]
fn review_group_copy_response_identifies_created_members() {
    let mut f = fixture();
    let mut before = f.editor.project().clone();
    let created = f
        .editor
        .execute(Command::MoveClipsWithOverwrite {
            clips: vec![f.audio, f.midi],
            anchor: f.audio,
            track: f.tracks[0],
            position: ClipGroupPosition::Audio(144_000),
            copy: true,
        })
        .unwrap();
    let copies: Vec<_> = f
        .editor
        .project()
        .tracks()
        .iter()
        .flat_map(|t| {
            t.clips()
                .iter()
                .map(|c| c.id())
                .chain(t.midi_clips().iter().map(|c| c.id()))
        })
        .filter(|id| before.clip(*id).is_none() && before.midi_clip(*id).is_none())
        .collect();
    assert_eq!(copies.len(), 2);
    assert_eq!(f.editor.project().clip(f.audio), before.clip(f.audio));
    assert_eq!(
        f.editor.project().midi_clip(f.midi),
        before.midi_clip(f.midi)
    );
    assert!(f.editor.undo());
    // Undo deliberately retains the allocated ID high-water mark.
    before.next_id = f.editor.project().next_id;
    assert_eq!(f.editor.project(), &before);
    assert!(f.editor.redo());
    eprintln!("REVIEW_RESPONSE actual={created:?} copied_ids={copies:?}; history copy retained");
    assert_ne!(
        created,
        Created::None,
        "successful group copy must identify its new selection"
    );
}

#[test]
fn group_copy_disjoint_cuts_preserve_gaps_carry_and_fragment_ids_before_copies() {
    let mut f = fixture();
    f.editor
        .execute(Command::MoveMidiClip {
            clip: f.midi,
            track: f.tracks[1],
            start: BeatTime::from_quarters(8).unwrap(),
        })
        .unwrap();
    // Both valid hybrid rows have disjoint audio/MIDI members. Each destination
    // victim sees two cuts, without constructing overlapping old material.
    let Created::Clip(other_audio) = f
        .editor
        .execute(Command::CopyClipTo {
            clip: f.audio,
            track: f.tracks[1],
            start: 48_000,
        })
        .unwrap()
    else {
        panic!("other audio")
    };
    let Created::Clip(other_midi) = f
        .editor
        .execute(Command::CopyMidiClipTo {
            clip: f.midi,
            track: f.tracks[0],
            start: BeatTime::from_quarters(8).unwrap(),
        })
        .unwrap()
    else {
        panic!("other midi")
    };
    let asset = f.editor.project().clip(f.audio).unwrap().asset();
    let Created::Clip(audio_victim) = f
        .editor
        .execute(Command::PlaceClip {
            track: f.tracks[0],
            asset,
            start: 450_000,
            duration: 270_000,
            source_offset: 0,
            source_length: 96_000,
            looping: true,
        })
        .unwrap()
    else {
        panic!("audio victim")
    };
    let Created::Clip(midi_victim) = f
        .editor
        .execute(Command::PlaceMidiClip {
            track: f.tracks[1],
            start: BeatTime::from_quarters(18).unwrap(),
            length: BeatTime::from_quarters(14).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi victim")
    };
    f.editor
        .execute(Command::AddMidiNote {
            clip: midi_victim,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(14).unwrap(),
            pitch: 67,
            channel: 3,
            velocity: 87,
        })
        .unwrap();
    let before = f.editor.project().clone();
    let Created::Clips { first, last } = f
        .editor
        .execute(Command::MoveClipsWithOverwrite {
            clips: vec![f.audio, f.midi, other_audio, other_midi],
            anchor: f.audio,
            track: f.tracks[0],
            position: ClipGroupPosition::Audio(480_000),
            copy: true,
        })
        .unwrap()
    else {
        panic!("copy endpoints")
    };
    let after = f.editor.project().clone();
    let row = after.track(f.tracks[0]).unwrap();
    let births = after
        .tracks()
        .iter()
        .flat_map(|track| {
            track
                .clips()
                .iter()
                .map(|c| c.id())
                .chain(track.midi_clips().iter().map(|c| c.id()))
        })
        .filter(|id| *id >= first && *id <= last)
        .collect::<Vec<_>>();
    assert_eq!(
        births.len(),
        4,
        "fragment births must be outside the copy range"
    );
    assert!(births.contains(&first) && births.contains(&last));
    let mut audio = row
        .clips()
        .iter()
        .filter(|c| c.id() != f.audio && (c.id() < first || c.id() > last))
        .collect::<Vec<_>>();
    audio.sort_by_key(|c| c.start());
    assert_eq!(
        audio
            .iter()
            .map(|c| (c.start(), c.start() + c.duration()))
            .collect::<Vec<_>>(),
        [(450_000, 480_000), (576_000, 624_000), (672_000, 720_000)]
    );
    assert_eq!(audio[0].id(), audio_victim);
    assert!(audio.iter().all(|c| c.id() < first));
    let mut midi = after
        .track(f.tracks[1])
        .unwrap()
        .midi_clips()
        .iter()
        .filter(|c| c.id() != f.midi && (c.id() < first || c.id() > last))
        .collect::<Vec<_>>();
    midi.sort_by_key(|c| c.start());
    let bounds = midi
        .iter()
        .map(|c| {
            (
                after.tempo_map().beat_to_frame(c.start()).unwrap(),
                after
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(c.start().raw() + c.length().raw()))
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    // Audio cuts retain the existing lower-bound frame fence; sub-frame beat
    // endpoints may precede the next exact beat but map to these same frames.
    assert_eq!(
        bounds,
        [(432_000, 480_000), (576_000, 624_000), (672_000, 768_000)]
    );
    assert_eq!(midi[0].id(), midi_victim);
    assert!(midi.iter().all(|c| c.id() < first));
    for fragment in &midi[1..] {
        assert_eq!(fragment.notes().len(), 1);
        let n = &fragment.notes()[0];
        assert_eq!(n.start(), BeatTime::ZERO);
        assert_eq!(n.duration(), fragment.length());
        assert_eq!((n.pitch(), n.channel(), n.velocity()), (67, 3, 87));
        assert_eq!(fragment.source().source_start(), BeatTime::ZERO);
    }
    assert_eq!(after.clip(f.audio), before.clip(f.audio));
    assert_eq!(after.midi_clip(f.midi), before.midi_clip(f.midi));
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
    assert!(f.editor.undo());
    let mut undo = before;
    undo.next_id = after.next_id;
    assert_eq!(f.editor.project(), &undo);
    assert!(f.editor.redo());
    assert_eq!(f.editor.project(), &after);
}

#[test]
fn group_final_clip_capacity_includes_session_and_refusal_is_preparation_atomic() {
    for excess in [false, true] {
        let mut f = fixture();
        let Created::Scene(scene) = f
            .editor
            .execute(Command::AddScene {
                name: "session".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        f.editor
            .execute(Command::CopyArrangementMidiToSession {
                source: f.midi,
                scene,
            })
            .unwrap();
        let mut project = f.editor.project().clone();
        let template = project.clip(f.audio).unwrap().clone();
        let fillers = MAX_CLIPS - 5 + usize::from(excess); // 2 originals + 1 Session + 2 copies.
        project
            .audio_clips_mut(f.tracks[2])
            .unwrap()
            .reserve(fillers);
        for index in 0..fillers {
            let mut c = template.clone();
            c.id = ClipId(project.allocate_id().unwrap());
            c.start = 10_000_000 + index as u64 * 2;
            c.duration = 1;
            c.source_offset = 0;
            c.source_length = 1;
            c.source_phase = 0;
            c.looping = false;
            c.edit = None;
            c.fade = crate::ClipFade::neutral(1).unwrap();
            project.audio_clips_mut(f.tracks[2]).unwrap().push(c);
        }
        assert_eq!(Project::from_snapshot(project.snapshot()).unwrap(), project);
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        let mut prepared = 0;
        let result = editor.execute_validated(
            Command::MoveClipsWithOverwrite {
                clips: vec![f.audio, f.midi],
                anchor: f.audio,
                track: f.tracks[0],
                position: ClipGroupPosition::Audio(480_000),
                copy: true,
            },
            |p| {
                prepared += 1;
                assert_eq!(
                    p.tracks()
                        .iter()
                        .map(|t| t.clips().len() + t.midi_clips().len())
                        .sum::<usize>()
                        + 1,
                    MAX_CLIPS
                );
                Ok::<(), Invalid>(())
            },
        );
        if excess {
            assert_eq!(result, Err(Invalid("clip limit reached")));
            assert_eq!(prepared, 0);
            assert_eq!(editor.project(), &project);
            assert!(!editor.can_undo() && !editor.can_redo());
        } else {
            result.unwrap();
            assert_eq!(prepared, 1);
            assert_eq!(editor.project().session_cells(), project.session_cells());
        }
    }
}

#[test]
fn group_copy_id_exhaustion_preserves_project_history_and_never_prepares() {
    let f = fixture();
    let mut project = f.editor.project().clone();
    project.next_id = u64::MAX - 1;
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    let mut prepared = 0;
    let result = editor.execute_validated(
        Command::MoveClipsWithOverwrite {
            clips: vec![f.audio, f.midi],
            anchor: f.audio,
            track: f.tracks[0],
            position: ClipGroupPosition::Audio(480_000),
            copy: true,
        },
        |_| {
            prepared += 1;
            Ok::<(), Invalid>(())
        },
    );
    assert_eq!(result, Err(Invalid("project id space exhausted")));
    assert_eq!(prepared, 0);
    assert_eq!(editor.project(), &project);
    assert!(!editor.can_undo() && !editor.can_redo());
}

#[test]
fn group_final_note_capacity_includes_session_and_real_preparation_refusal_keeps_ids() {
    for excess in [false, true] {
        let mut f = fixture();
        let Created::Scene(scene) = f
            .editor
            .execute(Command::AddScene {
                name: "notes".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        f.editor
            .execute(Command::CopyArrangementMidiToSession {
                source: f.midi,
                scene,
            })
            .unwrap();
        let mut p = f.editor.project().clone();
        let template = match &p.session.cells[0].content {
            crate::SessionCellContent::Midi(c) => c.source.notes[0].clone(),
            _ => panic!("session"),
        };
        let mut notes = Vec::new();
        for index in 0..MAX_NOTES - 2 + usize::from(excess) {
            let mut n = template.clone();
            n.id = NoteId(p.allocate_id().unwrap());
            n.start = BeatTime::from_raw(index as u64);
            n.duration = BeatTime::from_raw(1);
            notes.push(n);
        }
        let crate::SessionCellContent::Midi(c) = &mut p.session.cells[0].content else {
            panic!("session")
        };
        std::sync::Arc::make_mut(&mut c.source).notes = notes;
        assert_eq!(Project::from_snapshot(p.snapshot()).unwrap(), p);
        let mut editor = Editor::new(p.clone(), 8).unwrap();
        let mut calls = 0;
        let command = Command::MoveClipsWithOverwrite {
            clips: vec![f.audio, f.midi],
            anchor: f.audio,
            track: f.tracks[0],
            position: ClipGroupPosition::Audio(480_000),
            copy: true,
        };
        let result = editor.execute_validated(command, |candidate| {
            calls += 1;
            assert_eq!(crate::midi::note_count(candidate), MAX_NOTES);
            // Refuse after the complete candidate/ID plan actually exists.
            Err::<(), Invalid>(Invalid("complete candidate preparation refused"))
        });
        if excess {
            assert_eq!(result, Err(Invalid("midi note limit reached")));
            assert_eq!(calls, 0);
        } else {
            assert_eq!(
                result,
                Err(Invalid("complete candidate preparation refused"))
            );
            assert_eq!(calls, 1);
        }
        assert_eq!(editor.project(), &p);
        assert!(!editor.can_undo() && !editor.can_redo());
    }
}

#[test]
fn group_bounds_occurrence_enumeration_separately_from_tiny_retained_tail() {
    for tail in [false, true] {
        let mut f = fixture();
        let start = BeatTime::from_quarters(14).unwrap();
        let end = f
            .editor
            .project()
            .tempo_map()
            .frame_to_beat(if tail { 432_002 } else { 432_000 })
            .unwrap();
        let Created::Clip(id) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track: f.tracks[0],
                start,
                length: BeatTime::from_raw(end.raw() - start.raw()),
            })
            .unwrap()
        else {
            panic!("victim")
        };
        let mut p = f.editor.project().clone();
        let mut note = p.midi_clip(f.midi).unwrap().notes()[0].clone();
        note.id = NoteId(p.allocate_id().unwrap());
        note.duration = BeatTime::from_raw(1);
        let length = BeatTime::from_raw(BeatTime::ONE.raw() / 65_536);
        let victim = p
            .track_mut(f.tracks[0])
            .unwrap()
            .content
            .midi_clips
            .iter_mut()
            .find(|c| c.id() == id)
            .unwrap();
        victim.source = std::sync::Arc::new(crate::MidiSource {
            content: std::sync::Arc::new(crate::MidiContent {
                source_length: length,
                notes: vec![note],
            }),
            one_shot_boundary: crate::MidiOneShotBoundary::SourceEnd,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: length,
            looping: true,
        });
        assert!(crate::midi::overwrite_occurrence_count(victim).unwrap() > MAX_NOTES);
        assert_eq!(Project::from_snapshot(p.snapshot()).unwrap(), p);
        let mut editor = Editor::new(p.clone(), 8).unwrap();
        let mut calls = 0;
        let result = editor.execute_validated(
            Command::MoveClipsWithOverwrite {
                clips: vec![f.audio, f.midi],
                anchor: f.audio,
                track: f.tracks[0],
                position: ClipGroupPosition::Audio(336_000),
                copy: true,
            },
            |candidate| {
                calls += 1;
                assert!(candidate.midi_clip(id).is_none());
                Ok::<(), Invalid>(())
            },
        );
        if tail {
            assert_eq!(
                result,
                Err(Invalid(
                    "midi overwrite note expansion exceeds project limit"
                ))
            );
            assert_eq!(calls, 0);
            assert_eq!(editor.project(), &p);
            assert!(!editor.can_undo());
        } else {
            result.unwrap();
            assert_eq!(calls, 1);
        }
    }
}

#[test]
fn all_midi_group_moves_by_exact_beats_at_non_integer_frame_tempo() {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: crate::Tempo::new(123_000_000).unwrap(),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack { name: "m".into() })
        .unwrap()
    else {
        panic!("track")
    };
    let four = BeatTime::from_quarters(4).unwrap();
    let mut clips = Vec::new();
    for start in [BeatTime::ZERO, four] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start,
                length: four,
            })
            .unwrap()
        else {
            panic!("midi")
        };
        clips.push(clip);
    }
    let one = BeatTime::ONE;
    let position = ClipGroupPosition::Midi(one);
    let source = std::sync::Arc::new(editor.project().clone());
    let mut captured = crate::ClipGroupMovePreview::capture(&source, &clips, clips[0]).unwrap();
    captured.update(&source, track, position).unwrap();
    assert_capture_matches_plan(
        captured.previews(),
        &source
            .preview_clip_group_move(&clips, clips[0], track, position)
            .unwrap(),
    );
    editor
        .execute(Command::MoveClipsWithOverwrite {
            clips: clips.clone(),
            anchor: clips[0],
            track,
            position,
            copy: false,
        })
        .unwrap();
    let project = editor.project();
    let (first, second) = (
        project.midi_clip(clips[0]).unwrap(),
        project.midi_clip(clips[1]).unwrap(),
    );
    assert_eq!(first.start(), one);
    assert_eq!(second.start(), BeatTime::from_quarters(5).unwrap());
    assert_eq!(first.length(), four);
    assert_eq!(second.length(), four);
}

#[test]
fn destination_alias_group_keeps_members_and_relative_placement() {
    let mut f = fixture();
    let before = f.editor.project().snapshot();
    let Created::Clips { first, last } = f
        .editor
        .execute(Command::DuplicateAliasClipsAt {
            clips: vec![f.audio, f.midi],
            anchor: f.audio,
            track: f.tracks[0],
            position: ClipGroupPosition::Audio(288_000),
        })
        .unwrap()
    else {
        panic!("alias group")
    };
    assert_ne!(first, last);
    assert!(f.editor.project().clip(f.audio).is_some());
    assert!(f.editor.project().midi_clip(f.midi).is_some());
    let copied_audio = f
        .editor
        .project()
        .track(f.tracks[0])
        .unwrap()
        .clips()
        .iter()
        .find(|clip| clip.id() != f.audio)
        .unwrap();
    let copied_midi = f
        .editor
        .project()
        .track(f.tracks[1])
        .unwrap()
        .midi_clips()
        .iter()
        .find(|clip| clip.id() != f.midi)
        .unwrap();
    assert_eq!(copied_audio.start(), 288_000);
    let project = f.editor.project();
    assert_eq!(
        project
            .tempo_map()
            .beat_to_frame(copied_midi.start())
            .unwrap(),
        project
            .tempo_map()
            .beat_to_frame(project.midi_clip(f.midi).unwrap().start())
            .unwrap()
            + 240_000,
    );
    assert_eq!(
        f.editor.project().clip_alias(copied_audio.id()),
        f.editor.project().clip_alias(f.audio)
    );
    assert_eq!(
        f.editor.project().clip_alias(copied_midi.id()),
        f.editor.project().clip_alias(f.midi)
    );
    let after = f.editor.project().snapshot();
    let mut expected = before;
    expected.next_id = after.next_id;
    assert!(f.editor.undo());
    assert_eq!(f.editor.project().snapshot(), expected);
    assert!(f.editor.redo());
    assert_eq!(f.editor.project().snapshot(), after);
}
