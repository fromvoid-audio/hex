use hex_project::{Command, Created, Editor, Project, TrimEdge};
use std::path::PathBuf;

fn editor_with_clip(
    looping: bool,
    start: u64,
    duration: u64,
    offset: u64,
    length: u64,
    asset: u64,
) -> (Editor, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: PathBuf::from("audio.wav"),
            sample_rate: 48_000,
            frames: asset,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let clip = match editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start,
            duration,
            source_offset: offset,
            source_length: length,
            looping,
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    (editor, clip)
}

#[test]
fn loop_split_keeps_source_region_and_advances_phase_recursively() {
    let (mut editor, clip) = editor_with_clip(true, 10, 11, 2, 3, 8);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 8,
            fade_out: 7,
        })
        .unwrap();
    editor
        .execute(Command::SplitClip { clip, frame: 14 })
        .unwrap();
    let right = editor.project().tracks()[0].clips()[1].id();
    let right_clip = editor.project().clip(right).unwrap();
    assert_eq!(
        (
            right_clip.source_offset(),
            right_clip.source_length(),
            right_clip.source_phase()
        ),
        (2, 3, 1)
    );
    editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 18,
        })
        .unwrap();
    let clips = editor.project().tracks()[0].clips();
    assert_eq!(
        clips
            .iter()
            .map(|clip| clip.source_phase())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert!(
        clips
            .iter()
            .all(|clip| clip.source_offset() == 2 && clip.source_length() == 3)
    );
    assert_eq!(
        clips
            .iter()
            .map(|clip| clip.fade().offset())
            .collect::<Vec<_>>(),
        [0, 4, 8]
    );
    let duplicate_source = clips[1].id();
    let duplicate = match editor
        .execute(Command::DuplicateClip(duplicate_source))
        .unwrap()
    {
        Created::Clip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    assert_eq!(editor.project().clip(duplicate).unwrap().source_phase(), 1);
}

#[test]
fn edge_trim_preserves_one_shot_silent_tail_and_pads_before_source() {
    let (mut editor, clip) = editor_with_clip(false, 10, 8, 4, 5, 20);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 6,
            fade_out: 5,
        })
        .unwrap();
    let prior = editor.project().clip(clip).unwrap().clone();
    let preview = prior.preview_edge_trim(TrimEdge::Left, 8, 20).unwrap();
    assert_eq!(
        (
            preview.start(),
            preview.duration(),
            preview.source_offset(),
            preview.source_length()
        ),
        (8, 10, 2, 7)
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 8,
        })
        .unwrap();
    let trimmed = editor.project().clip(clip).unwrap().clone();
    assert_eq!(
        (
            trimmed.start(),
            trimmed.duration(),
            trimmed.source_offset(),
            trimmed.source_length()
        ),
        (8, 10, 2, 7)
    );
    assert_eq!(trimmed.fade().span(), 10);
    assert_eq!(trimmed.fade().offset(), 0);
    let left_bound = prior.preview_edge_trim(TrimEdge::Left, 0, 20).unwrap();
    assert_eq!((left_bound.start(), left_bound.source_offset()), (0, 0));
    assert_eq!(left_bound.duration(), 18);
    let prefix_event = left_bound.audio_events().events[0];
    assert_eq!((prefix_event.start, prefix_event.source_start), (6, 0));
    assert_eq!(prefix_event.end, 18);
    let right_bound = trimmed
        .preview_edge_trim(TrimEdge::Right, u64::MAX, 20)
        .unwrap();
    assert_eq!(
        (
            right_bound.start() + right_bound.duration(),
            right_bound.source_length()
        ),
        (29, 18)
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 15,
        })
        .unwrap();
    // The three-frame silent tail is consumed before any audible source.
    let right = editor.project().clip(clip).unwrap();
    assert_eq!((right.duration(), right.source_length()), (7, 7));
    assert_eq!(right.source_offset(), 2);
    assert!(editor.undo());
    assert_eq!(editor.project().clip(clip).unwrap(), &trimmed);
}

/// Loop on, extend right past the source, loop off: a 100-frame one-shot at
/// frame 10 whose 10-frame source is followed by 90 silent frames.
fn silent_tail_fixture() -> (Editor, hex_project::ClipId) {
    let (mut editor, clip) = editor_with_clip(true, 10, 10, 0, 10, 100);
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 110,
        })
        .unwrap();
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: false,
        })
        .unwrap();
    let tail = editor.project().clip(clip).unwrap();
    assert_eq!((tail.duration(), tail.source_length()), (100, 10));
    (editor, clip)
}

#[test]
fn one_shot_right_trim_shortens_silent_tail_before_source() {
    let (mut editor, clip) = silent_tail_fixture();
    for (frame, expected) in [(105, (95, 10)), (20, (10, 10)), (15, (5, 5)), (0, (1, 1))] {
        let preview = editor
            .project()
            .clip(clip)
            .unwrap()
            .preview_edge_trim(TrimEdge::Right, frame, 100)
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame,
            })
            .unwrap();
        let trimmed = editor.project().clip(clip).unwrap();
        assert_eq!(trimmed, &preview, "frame {frame}");
        assert_eq!(
            (trimmed.duration(), trimmed.source_length()),
            expected,
            "frame {frame}"
        );
        assert_eq!((trimmed.start(), trimmed.source_offset()), (10, 0));
    }
}

#[test]
fn split_inside_one_shot_silent_tail_succeeds_and_undoes_exactly() {
    for fades in [None, Some((2, 30))] {
        let (mut editor, clip) = silent_tail_fixture();
        if let Some((fade_in, fade_out)) = fades {
            editor
                .execute(Command::SetClipFades {
                    clip,
                    fade_in,
                    fade_out,
                })
                .unwrap();
        }
        let before = editor.project().clone();
        let right = match editor
            .execute(Command::SplitClip { clip, frame: 50 })
            .unwrap()
        {
            Created::Clip(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };
        let left = editor.project().clip(clip).unwrap();
        let tail = editor.project().clip(right).unwrap();
        assert_eq!((left.start(), left.duration()), (10, 40), "{fades:?}");
        assert_eq!((tail.start(), tail.duration()), (50, 60), "{fades:?}");
        assert!(editor.undo());
        // Identity allocation is never rewound; all authored state is restored.
        assert_eq!(editor.project().tracks(), before.tracks(), "{fades:?}");
    }
}

#[test]
fn looping_left_trim_and_extension_update_phase_and_right_trim_keeps_region() {
    let (mut editor, clip) = editor_with_clip(true, 10, 8, 2, 3, 8);
    editor
        .execute(Command::SplitClip { clip, frame: 14 })
        .unwrap();
    let right = editor.project().tracks()[0].clips()[1].id();
    editor
        .execute(Command::TrimClipEdge {
            clip: right,
            edge: TrimEdge::Left,
            frame: 15,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 2);
    editor
        .execute(Command::TrimClipEdge {
            clip: right,
            edge: TrimEdge::Left,
            frame: 14,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 1);
    editor
        .execute(Command::TrimClipEndBy {
            clip: right,
            frames: -1,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().duration(), 3);
    editor
        .execute(Command::TrimClipEndBy {
            clip: right,
            frames: 3,
        })
        .unwrap();
    let extended = editor.project().clip(right).unwrap();
    assert_eq!(extended.duration(), 6);
    assert_eq!(
        (
            extended.source_offset(),
            extended.source_length(),
            extended.source_phase()
        ),
        (2, 3, 1)
    );
    let (start, duration, source_length) = (
        extended.start(),
        extended.duration(),
        extended.source_length(),
    );
    editor
        .execute(Command::TrimClip {
            clip: right,
            start,
            duration,
            source_offset: 1,
            source_length,
        })
        .unwrap();
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 0);
}

#[test]
fn edge_trim_preview_noop_retains_fade_and_bounds_wrap_without_overflow() {
    let (mut editor, clip) = editor_with_clip(true, u64::MAX - 8, 8, 0, 3, 3);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 3,
            fade_out: 2,
        })
        .unwrap();
    let original = editor.project().clip(clip).unwrap().clone();
    let unchanged = original
        .preview_edge_trim(TrimEdge::Left, u64::MAX - 8, 3)
        .unwrap();
    assert_eq!(unchanged, original);
    let right = original
        .preview_edge_trim(TrimEdge::Right, u64::MAX, 3)
        .unwrap();
    assert_eq!(right.start() + right.duration(), u64::MAX);
    assert_eq!(right.duration(), 8);
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: u64::MAX,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap(), &original);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: u64::MAX,
        })
        .unwrap();
    assert!(editor.can_redo());
    assert!(editor.redo());
    assert!(editor.project().track(track).unwrap().muted());
}

#[test]
fn invalid_snapshot_phases_and_commands_are_rejected_without_history_changes() {
    let (mut editor, clip) = editor_with_clip(true, 0, 4, 0, 3, 3);
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_phase = 3;
    assert!(Project::from_snapshot(snapshot).is_err());
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].looping = false;
    snapshot.tracks[0].audio_clips_mut().unwrap()[0].source_phase = 1;
    assert!(Project::from_snapshot(snapshot).is_err());
    let before = editor.project().clone();
    let undo = editor.can_undo();
    assert!(
        editor
            .execute(Command::TrimClip {
                clip,
                start: 0,
                duration: 4,
                source_offset: 0,
                source_length: 0
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.can_undo(), undo);
    editor.execute(Command::RemoveClip(clip)).unwrap();
    let removed = editor.project().clone();
    let history = editor.can_undo();
    assert!(
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 0,
            })
            .is_err()
    );
    assert_eq!(editor.project(), &removed);
    assert_eq!(editor.can_undo(), history);
}

#[test]
fn lower_fade_resize_uses_source_trim_preserves_upper_samples_and_one_step_history() {
    use hex_project::ClipFade;
    for looping in [false, true] {
        let (mut editor, clip) = editor_with_clip(looping, 10, 20, 5, 20, 40);
        let fade = ClipFade::new(20, 0, 8, 7)
            .unwrap()
            .with_shape(2, 3, 700, -600)
            .unwrap();
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        let original = editor.project().clone();
        for (edge, requested, expected_start, expected_duration, expected_in, expected_out) in [
            (TrimEdge::Left, 14, 14, 16, 6, 7),
            (TrimEdge::Right, 25, 10, 15, 8, 5),
            (TrimEdge::Left, 0, 0, 30, 20, 7),
            (
                TrimEdge::Right,
                u64::MAX,
                10,
                if looping { u64::MAX - 10 } else { 35 },
                8,
                if looping { u64::MAX - 20 } else { 25 },
            ),
            (TrimEdge::Left, 100, 19, 11, 1, 7),
            (TrimEdge::Right, 0, 10, 11, 8, 1),
        ] {
            let before = editor.project().clip(clip).unwrap();
            let preview = before
                .preview_fade_edge_trim(edge, requested, 40, editor.project().tempo_map())
                .unwrap();
            editor
                .execute(Command::TrimClipFadeEdge {
                    clip,
                    edge,
                    frame: requested,
                })
                .unwrap();
            let changed = editor.project().clip(clip).unwrap();
            assert_eq!(&preview, changed);
            assert_eq!(
                (changed.start(), changed.duration()),
                (expected_start, expected_duration)
            );
            assert_eq!(
                (
                    changed.retained_fade().fade_in(),
                    changed.retained_fade().fade_out()
                ),
                (expected_in, expected_out)
            );
            assert_eq!(
                (
                    changed.retained_fade().in_curve(),
                    changed.retained_fade().out_curve()
                ),
                (700, -600)
            );
            match edge {
                TrimEdge::Left => {
                    assert_eq!(changed.retained_fade().in_start(), 0);
                    assert_eq!(
                        changed.retained_fade().out_end_offset(),
                        3.min(changed.duration() - 1)
                    );
                    if looping {
                        assert_eq!((changed.source_offset(), changed.source_length()), (5, 20));
                        assert_eq!(
                            changed.source_phase(),
                            ((i128::from(changed.start()) - 10).rem_euclid(20)) as u64
                        );
                    } else if changed.start() < 5 {
                        assert_eq!((changed.source_offset(), changed.source_length()), (0, 25));
                        let event = changed.audio_events().events[0];
                        assert_eq!((event.start, event.source_start), (5, 0));
                    } else {
                        assert_eq!(changed.source_offset(), changed.start() - 5);
                        assert_eq!(changed.source_length(), 30 - changed.start());
                    }
                }
                TrimEdge::Right => {
                    assert_eq!(changed.retained_fade().out_end_offset(), 0);
                    assert_eq!(changed.retained_fade().in_start(), 2);
                }
            }
            let result = editor.project().clone();
            assert!(editor.undo());
            assert_eq!(editor.project(), &original);
            assert!(editor.redo());
            assert_eq!(editor.project(), &result);
            assert!(editor.undo());
        }
    }
}

#[test]
fn neutral_lower_resize_and_unchanged_or_source_clamped_edges_preserve_noop_history() {
    let (mut editor, clip) = editor_with_clip(false, 0, 10, 0, 10, 10);
    editor
        .execute(Command::TrimClipFadeEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 4,
        })
        .unwrap();
    assert_eq!(
        (
            editor.project().clip(clip).unwrap().start(),
            editor.project().clip(clip).unwrap().duration()
        ),
        (4, 6)
    );
    assert!(editor.project().clip(clip).unwrap().fade().is_neutral());
    assert!(editor.undo());
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        editor
            .execute(Command::TrimClipFadeEdge {
                clip,
                edge,
                frame: if edge == TrimEdge::Left { 0 } else { u64::MAX },
            })
            .unwrap();
    }
    assert!(editor.redo(), "no-op preserves the redo branch");
}
