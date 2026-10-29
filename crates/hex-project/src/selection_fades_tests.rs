use crate::*;

fn adjacent_looping_clips() -> (Editor, TrackId, [ClipId; 2]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported {
        track, clip: left, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "model-only.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("fixture import")
    };
    editor
        .execute(Command::SetClipLooping {
            clip: left,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase {
            clip: left,
            phase: 16,
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::CopyClipTo {
            clip: left,
            track,
            start: 96_000,
        })
        .unwrap()
    else {
        panic!("fixture copy")
    };
    (editor, track, [left, right])
}

#[test]
fn explicitly_faded_source_handle_overlap_is_a_valid_saved_project() {
    let (editor, _, [left, right]) = adjacent_looping_clips();
    let mut candidate = editor.project().clone();
    let asset_frames = 96_000;
    let outgoing = candidate
        .clip(left)
        .unwrap()
        .preview_edge_trim(TrimEdge::Right, 100_000, asset_frames)
        .unwrap();
    let incoming = candidate
        .clip(right)
        .unwrap()
        .preview_edge_trim(TrimEdge::Left, 92_000, asset_frames)
        .unwrap();
    assert_eq!((outgoing.start(), outgoing.duration()), (0, 100_000));
    assert_eq!((incoming.start(), incoming.duration()), (92_000, 100_000));
    assert_eq!(incoming.source_phase(), 92_016);
    *candidate.clip_mut(left).unwrap() = outgoing;
    *candidate.clip_mut(right).unwrap() = incoming;
    candidate.clip_mut(left).unwrap().fade = ClipFade::new(100_000, 0, 0, 8_000).unwrap();
    candidate.clip_mut(right).unwrap().fade = ClipFade::new(100_000, 0, 8_000, 0).unwrap();
    let restored = Project::from_snapshot(candidate.snapshot());
    assert!(
        restored.is_ok(),
        "a bounded two-voice crossfade must be admissible: {:?}",
        restored.err()
    );
}

#[test]
fn selection_crossfade_keeps_source_alignment_other_ramps_ids_and_one_history_action() {
    let (mut editor, track, [left, right]) = adjacent_looping_clips();
    for clip in [left, right] {
        editor
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(96_000, 0, 128, 256)
                    .unwrap()
                    .with_shape(0, 0, 200, -200)
                    .unwrap()
                    .with_curve_positions(300, 700)
                    .unwrap(),
            })
            .unwrap();
    }
    let before = editor.project().snapshot();
    let command = Command::CreateArrangementFades {
        range: TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
    };
    editor.execute(command.clone()).unwrap();
    let a = editor.project().clip(left).unwrap();
    let b = editor.project().clip(right).unwrap();
    assert_eq!(
        (a.start(), a.duration(), b.start(), b.duration()),
        (0, 100_000, 92_000, 100_000)
    );
    assert_eq!(a.source_phase(), 16);
    assert_eq!(
        (b.source_phase() + 4_000) % b.source_length(),
        16,
        "the original join reads the same source sample"
    );
    assert_eq!(
        (
            a.fade().fade_in(),
            a.fade().in_curve(),
            a.fade().in_position()
        ),
        (128, 200, 300)
    );
    assert_eq!(
        (
            b.fade().fade_out(),
            b.fade().out_curve(),
            b.fade().out_position()
        ),
        (256, -200, 700)
    );
    assert_eq!((a.fade().fade_out(), b.fade().fade_in()), (8_000, 8_000));
    for frame in 92_000..100_000 {
        assert!(
            (a.fade().gain_at(frame) + b.fade().gain_at(frame - b.start()) - 1.0).abs() <= 1e-7
        );
    }
    assert_eq!(editor.project().snapshot().next_id, before.next_id);
    let accepted = editor.project().snapshot();
    editor.execute(command).unwrap();
    assert_eq!(
        editor.project().snapshot(),
        accepted,
        "repeating the same intent is a no-op"
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), accepted);
}

#[test]
fn unary_selection_and_refusals_are_atomic_across_tracks() {
    let (mut editor, track, [left, _]) = adjacent_looping_clips();
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 0, 8_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(left).unwrap().fade().fade_in(), 8_000);
    let before = editor.project().snapshot();
    for (start, end) in [(1, 100), (0, 96_000)] {
        assert!(
            editor
                .execute(Command::CreateArrangementFades {
                    range: TimeRange::new(vec![track], start, end).unwrap()
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
    }
    let Created::Track(midi) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("fixture track")
    };
    editor
        .execute(Command::PlaceMidiClip {
            track: midi,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::CreateArrangementFades {
                range: TimeRange::new(vec![track, midi], 0, 4_000).unwrap()
            })
            .is_err()
    );
    assert_eq!(
        editor.project().snapshot(),
        before,
        "a later incompatible row must not leave the first row faded"
    );
}

#[test]
fn split_and_group_copy_retain_crossfade_fragments_without_third_voice() {
    let (mut editor, track, [left, _right]) = adjacent_looping_clips();
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
        })
        .unwrap();
    let Created::Clip(_fragment) = editor
        .execute(Command::SplitClip {
            clip: left,
            frame: 96_000,
        })
        .unwrap()
    else {
        panic!("fixture split")
    };
    assert_eq!(editor.project().tracks()[0].clips().len(), 3);
    Project::from_snapshot(editor.project().snapshot()).unwrap();
    editor
        .execute(Command::SplitTimeRange {
            range: TimeRange::new(vec![track], 94_000, 98_000).unwrap(),
        })
        .unwrap();
    let before = editor.project().snapshot();
    let ids = editor.project().tracks()[0]
        .clips()
        .iter()
        .map(|clip| clip.id())
        .collect();
    editor.execute(Command::DuplicateClips(ids)).unwrap();
    assert_eq!(editor.project().tracks()[0].clips().len(), 14);
    Project::from_snapshot(editor.project().snapshot()).unwrap();
    assert!(editor.undo());
    let mut expected = before;
    expected.next_id = editor.project().snapshot().next_id;
    assert_eq!(editor.project().snapshot(), expected);
}

#[test]
fn gaps_insufficient_handles_and_three_clip_selections_refuse_without_mutation() {
    for case in 0..3 {
        let (mut editor, track, [left, right]) = adjacent_looping_clips();
        let (start, end) = match case {
            0 => {
                editor
                    .execute(Command::MoveClip {
                        clip: right,
                        track,
                        start: 100_000,
                    })
                    .unwrap();
                (92_000, 108_000)
            }
            1 => {
                for clip in [left, right] {
                    editor
                        .execute(Command::SetClipLooping {
                            clip,
                            looping: false,
                        })
                        .unwrap();
                }
                (92_000, 100_000)
            }
            _ => {
                editor
                    .execute(Command::CopyClipTo {
                        clip: left,
                        track,
                        start: 192_000,
                    })
                    .unwrap();
                (92_000, 200_000)
            }
        };
        let before = editor.project().snapshot();
        assert!(
            editor
                .execute(Command::CreateArrangementFades {
                    range: TimeRange::new(vec![track], start, end).unwrap()
                })
                .is_err(),
            "case {case}"
        );
        assert_eq!(editor.project().snapshot(), before);
    }
}

#[test]
fn nonlooping_offset_source_handles_extend_without_translating_audio() {
    let (mut editor, track, [left, right]) = adjacent_looping_clips();
    editor.execute(Command::RemoveClip(right)).unwrap();
    editor
        .execute(Command::SetClipLooping {
            clip: left,
            looping: false,
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip: left,
            frame: 48_000,
        })
        .unwrap()
    else {
        panic!("fixture split")
    };
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 44_000, 52_000).unwrap(),
        })
        .unwrap();
    let a = editor.project().clip(left).unwrap();
    let b = editor.project().clip(right).unwrap();
    assert_eq!(
        (a.start(), a.duration(), b.start(), b.duration()),
        (0, 52_000, 44_000, 52_000)
    );
    assert_eq!((a.source_offset(), b.source_offset()), (0, 44_000));
    assert!(!a.looping() && !b.looping());
    assert_eq!(b.source_offset() + 48_000 - b.start(), 48_000);
}

#[test]
fn warped_edge_fade_uses_current_time_without_requantizing_the_other_ramp() {
    let (mut editor, track, [left, _]) = adjacent_looping_clips();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip: left,
            fade: ClipFade::new(96_000, 0, 128, 257)
                .unwrap()
                .with_shape(0, 0, 200, -200)
                .unwrap()
                .with_curve_positions(300, 700)
                .unwrap(),
        })
        .unwrap();
    let warp = default_audio_warp(
        editor.project().clip(left).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: left,
            settings: Some(warp),
        })
        .unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(180_000_000).unwrap(),
        })
        .unwrap();
    let original = editor.project().clip(left).unwrap().clone();
    let tempo = editor.project().tempo_map().clone();
    let original_edit = original.audio_edit().unwrap();
    let length = original.duration();
    let gains: Vec<_> = (length - 1_000..length)
        .map(|frame| {
            original_edit
                .projected_fade(&tempo)
                .unwrap()
                .gain_at(frame)
                .unwrap()
        })
        .collect();
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 0, 8_000).unwrap(),
        })
        .unwrap();
    let changed = editor.project().clip(left).unwrap().audio_edit().unwrap();
    assert_eq!(
        changed.domain.fade.fade_out(),
        original_edit.domain.fade.fade_out()
    );
    assert_eq!(
        changed.domain.fade.out_curve(),
        original_edit.domain.fade.out_curve()
    );
    assert_eq!(
        changed.domain.fade.out_position(),
        original_edit.domain.fade.out_position()
    );
    assert_eq!(
        changed.domain.fade_projection,
        original_edit.domain.fade_projection
    );
    assert_eq!(
        (length - 1_000..length)
            .map(|frame| changed
                .projected_fade(&tempo)
                .unwrap()
                .gain_at(frame)
                .unwrap())
            .collect::<Vec<_>>(),
        gains
    );
    let projected = changed.projected_fade(&tempo).unwrap();
    assert_eq!(projected.gain_at(0).unwrap(), 0.0);
    assert!((projected.gain_at(7_999).unwrap() - 1.0).abs() < 1e-7);
}

#[test]
fn event_selection_fade_retains_source_domain_and_other_authored_knots() {
    let (mut editor, _, [clip, _]) = adjacent_looping_clips();
    let shape = ClipFade::new(96_000, 0, 64_000, 32_000)
        .unwrap()
        .with_shape(0, 0, 200, -200)
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetFade {
                event: 1,
                fade: Some(AudioEventFade::new(0, shape).unwrap()),
            },
        })
        .unwrap();
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::CycleRange {
                start: 24_000,
                end: 72_000,
                looping: true,
            },
        })
        .unwrap();
    let before = editor.project().snapshot();
    editor
        .execute(Command::CreateAudioEventFades {
            clip,
            start: 24_000,
            end: 32_000,
        })
        .unwrap();
    let event = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_events()
        .events
        .iter()
        .find(|e| e.start == 24_000)
        .copied()
        .unwrap();
    let fade = event.fade.unwrap();
    assert_eq!((fade.source_start(), fade.shape().span()), (0, 96_000));
    assert_eq!(
        (fade.shape().in_start(), fade.shape().fade_in()),
        (24_000, 8_000)
    );
    assert_eq!(
        (fade.shape().fade_out(), fade.shape().out_curve()),
        (shape.fade_out(), shape.out_curve())
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn current_model_still_refuses_unfaded_and_third_voice_overlaps() {
    let (mut editor, track, [left, _]) = adjacent_looping_clips();
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
        })
        .unwrap();
    let accepted = editor.project().snapshot();
    let clips = editor.project().tracks()[0]
        .clips()
        .iter()
        .map(|clip| clip.id())
        .collect();
    let error = editor.execute(Command::GroupClips(clips)).unwrap_err();
    assert!(
        error
            .0
            .contains("grouping overlapping audio crossfades is unsupported")
    );
    assert_eq!(editor.project().snapshot(), accepted);
    let mut unfaded = editor.project().clone();
    let duration = unfaded.clip(left).unwrap().duration();
    unfaded.clip_mut(left).unwrap().fade = ClipFade::neutral(duration).unwrap();
    assert!(Project::from_snapshot(unfaded.snapshot()).is_err());
    assert!(
        editor
            .execute(Command::SetClipFades {
                clip: left,
                fade_in: 0,
                fade_out: 0
            })
            .is_err()
    );
    assert_eq!(
        editor.project().snapshot(),
        accepted,
        "resetting a required crossfade must not publish invalid overlap"
    );
    let mut triple = accepted;
    let fresh = triple.next_id;
    triple.next_id += 1;
    let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut triple.tracks[0].content else {
        panic!("hybrid fixture")
    };
    let mut extra = audio_clips[0].clone();
    extra.id = fresh;
    extra.start = 94_000;
    extra.duration = 4_000;
    extra.fade = ClipFade::new(4_000, 0, 4_000, 4_000).unwrap();
    audio_clips.push(extra);
    assert!(
        Project::from_snapshot(triple)
            .unwrap_err()
            .0
            .contains("at most two")
    );
}

#[test]
fn crossfade_preparation_refusal_preserves_both_clips_history_and_redo() {
    let (mut editor, track, [left, right]) = adjacent_looping_clips();
    editor
        .execute(Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 0, 8_000).unwrap(),
        })
        .unwrap();
    let admitted = editor.project().snapshot();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    let result: Result<(Created, ()), Invalid> = editor.execute_prepared(
        Command::CreateArrangementFades {
            range: TimeRange::new(vec![track], 92_000, 100_000).unwrap(),
        },
        |candidate| {
            assert_eq!(candidate.clip(left).unwrap().fade().fade_out(), 8_000);
            assert_eq!(candidate.clip(right).unwrap().fade().fade_in(), 8_000);
            Err(Invalid("fixture preparation refused"))
        },
    );
    assert_eq!(result, Err(Invalid("fixture preparation refused")));
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), admitted);
}
