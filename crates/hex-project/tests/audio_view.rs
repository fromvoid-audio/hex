//! Pure placement/snapshot laws, not Engine PCM or rendered-handle acceptance.
use hex_project::{
    AudioDomainClock, AudioViewMode, AudioViewPlacement, AudioViewProjection,
    AudioViewResizeOperation, BeatTime, ClipFade, ClipId, Command, Created, Editor, Project, Tempo,
    TrimEdge, preview_audio_view_resize, preview_view_resize,
};
const Q: u64 = 1 << 32;

fn review_overlap_fixture() -> (Editor, ClipId, ClipId, ClipId) {
    let (mut editor, clip) = frame_fixture(AudioViewMode::Repeat);
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().clip(clip).unwrap().asset();
    let Created::Clip(audio) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 12_000,
            duration: 60_000,
            source_offset: 2_000,
            source_length: 120_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(Q / 2),
            length: beat(5 * Q / 2),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    // Current-format load accepts these existing same-track overlaps.
    let loaded = Project::from_snapshot(editor.project().snapshot()).unwrap();
    (Editor::new(loaded, 8).unwrap(), clip, audio, midi)
}

#[test]
fn review_outer_noop_preserves_existing_overlap_ids_and_redo() {
    let (mut editor, clip, _, _) = review_overlap_fixture();
    editor
        .execute(Command::SetAudioClipGain { clip, millidb: 100 })
        .unwrap();
    let future = editor.project().snapshot();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::OuterEnd {
                project_frame: 48_000,
            },
        })
        .unwrap();
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), future);
}

#[test]
fn review_outer_shrink_preserves_existing_audio_midi_overlap_and_one_history_step() {
    let (mut editor, clip, audio, midi) = review_overlap_fixture();
    let before = editor.project().snapshot();
    let old_audio = editor.project().clip(audio).unwrap().clone();
    let old_midi = editor.project().midi_clip(midi).unwrap().clone();
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::OuterEnd {
                project_frame: 36_000,
            },
        })
        .unwrap();
    assert_eq!(editor.project().clip(audio), Some(&old_audio));
    assert_eq!(editor.project().midi_clip(midi), Some(&old_midi));
    assert_eq!(editor.project().snapshot().next_id, before.next_id);
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 36_000);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn review_outer_growth_cuts_only_new_tail_and_prepare_refusal_is_atomic() {
    let (mut editor, clip, audio, midi) = review_overlap_fixture();
    let before = editor.project().snapshot();
    let command = Command::ResizeAudioClipView {
        clip,
        operation: AudioViewResizeOperation::OuterEnd {
            project_frame: 84_000,
        },
    };
    let check = |candidate: &Project| {
        let left = candidate.clip(audio).unwrap();
        assert_eq!(
            (left.start(), left.duration(), left.source_offset()),
            (12_000, 36_000, 2_000)
        );
        let left = candidate.midi_clip(midi).unwrap();
        assert_eq!(left.start(), beat(Q / 2));
        assert_eq!(
            candidate
                .tempo_map()
                .beat_to_frame(beat(
                    left.start().raw().checked_add(left.length().raw()).unwrap()
                ))
                .unwrap(),
            48_000
        );
        assert_eq!(candidate.snapshot().next_id, before.next_id);
    };
    assert!(
        editor
            .execute_prepared(command.clone(), |candidate| {
                check(candidate);
                Err::<(), _>(hex_project::Invalid("injected prepare refusal"))
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    editor
        .execute_prepared(command, |candidate| {
            check(candidate);
            Ok::<_, hex_project::Invalid>(())
        })
        .unwrap();
    let after = editor.project().snapshot();
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

fn beat(raw: u64) -> BeatTime {
    BeatTime::from_raw(raw)
}
fn base(musical: bool) -> (Editor, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "root".into(),
            path: "never-opened.wav".into(),
            sample_rate: 48_000,
            frames: 480_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    e.execute(Command::SetAudioClipPitch { clip, cents: 123 })
        .unwrap();
    if musical {
        e.execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    }
    (e, clip)
}
fn frame_fixture(mode: AudioViewMode) -> (Editor, ClipId) {
    let (e, id) = base(false);
    let mut s = e.project().snapshot();
    let c = &mut s.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = c.edit.as_mut().unwrap();
    edit.view_start = 12_000;
    edit.view_end = 36_000;
    edit.placement = Some(AudioViewPlacement {
        mode,
        outer_length: AudioDomainClock::Frames(48_000),
        cycle_phase: 6_000,
    });
    c.duration = 48_000;
    c.fade = ClipFade::neutral(c.duration).unwrap();
    (
        Editor::new(Project::from_snapshot(s).unwrap(), 32).unwrap(),
        id,
    )
}
fn beat_fixture() -> (Editor, ClipId) {
    let (mut e, id) = base(true);
    for (at, bpm) in [(3 * Q, 241_123_456), (5 * Q, 60_333_333)] {
        e.execute(Command::SetTempoPoint {
            beat: beat(at),
            tempo: Tempo::new(bpm).unwrap(),
        })
        .unwrap();
    }
    let mut s = e.project().snapshot();
    let tempo = e.project().tempo_map();
    let c = &mut s.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = c.edit.as_mut().unwrap();
    edit.view_start = 2 * Q;
    edit.view_end = 4 * Q;
    edit.musical_start = Some(beat(Q));
    edit.placement = Some(AudioViewPlacement {
        mode: AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Beats(beat(7 * Q)),
        cycle_phase: Q / 2,
    });
    c.start = tempo.beat_to_frame(beat(Q)).unwrap();
    c.duration = tempo.beat_to_frame(beat(8 * Q)).unwrap() - c.start;
    c.fade = ClipFade::neutral(c.duration).unwrap();
    (
        Editor::new(Project::from_snapshot(s).unwrap(), 32).unwrap(),
        id,
    )
}

#[test]
fn negative_frame_origin_phase_and_once_tail_share_one_retained_cycle() {
    for (mode, expected) in [(AudioViewMode::Repeat, 3), (AudioViewMode::Once, 1)] {
        let (e, id) = frame_fixture(mode);
        let c = e.project().clip(id).unwrap();
        let p = AudioViewProjection::new(c, e.project().tempo_map()).unwrap();
        assert_eq!(p.outer_frame_bounds(), (0, 48_000));
        assert_eq!(p.logical_first_boundary(), 18_000);
        assert_eq!(p.occurrence_count(), expected);
        let a = p.occurrence(0).unwrap();
        assert_eq!(
            (a.root_origin, a.root_frame_origin, a.root_frame_length),
            (-18_000, -18_000, 480_000)
        );
        assert_eq!(
            (
                a.root_start,
                a.root_end,
                a.destination_start,
                a.destination_end
            ),
            (18_000, 36_000, 0, 18_000)
        );
        if mode == AudioViewMode::Repeat {
            let b = p.occurrence(1).unwrap();
            let d = p.occurrence(2).unwrap();
            assert_eq!(
                (
                    b.root_origin,
                    b.root_start,
                    b.root_end,
                    b.destination_start,
                    b.destination_end
                ),
                (6_000, 12_000, 36_000, 18_000, 42_000)
            );
            assert_eq!(
                (
                    d.root_origin,
                    d.root_start,
                    d.root_end,
                    d.destination_start,
                    d.destination_end
                ),
                (30_000, 12_000, 18_000, 42_000, 48_000)
            );
        }
        assert!(p.occurrence(expected).is_err());
        assert_eq!(c.audio_edit().unwrap().domain.pitch_cents, 123);
    }
}

#[test]
fn fractional_tempo_occurrences_use_absolute_beats_not_a_repeated_frame_period() {
    let (e, id) = beat_fixture();
    let tempo = e.project().tempo_map();
    let c = e.project().clip(id).unwrap();
    let p = AudioViewProjection::new(c, tempo).unwrap();
    assert_eq!(p.occurrence_count(), 4);
    let endpoints = [Q, 5 * Q / 2, 9 * Q / 2, 13 * Q / 2, 8 * Q];
    let expected: Vec<_> = endpoints
        .into_iter()
        .map(|b| tempo.beat_to_frame(beat(b)).unwrap())
        .collect();
    let mut periods = Vec::new();
    for index in 0..4 {
        let a = p.occurrence(index).unwrap();
        assert_eq!(
            (a.destination_start, a.destination_end),
            (expected[index as usize], expected[index as usize + 1])
        );
        assert_eq!(
            a.root_origin,
            -3 * i128::from(Q) / 2 + i128::from(index) * i128::from(2 * Q)
        );
        assert_eq!(
            a.root_end - a.root_start,
            a.destination_end - a.destination_start
        );
        periods.push(a.destination_end - a.destination_start);
    }
    assert_ne!(periods[1], periods[2]);
    assert_eq!(p.logical_first_boundary(), i128::from(expected[1]));
    let preview = preview_view_resize(
        c,
        tempo,
        AudioViewResizeOperation::OuterEnd {
            project_frame: tempo.beat_to_frame(beat(5 * Q / 4)).unwrap(),
        },
    )
    .unwrap();
    let mut s = e.project().snapshot();
    let n = &mut s.tracks[0].audio_clips_mut().unwrap()[0];
    n.edit.as_mut().unwrap().placement = Some(preview.placement);
    n.duration = preview.frame_end - preview.frame_start;
    n.fade = ClipFade::neutral(n.duration).unwrap();
    let shorter = Project::from_snapshot(s).unwrap();
    let short = AudioViewProjection::new(shorter.clip(id).unwrap(), shorter.tempo_map()).unwrap();
    assert!(short.logical_first_boundary() > i128::from(short.outer_frame_bounds().1));
    assert_eq!(short.logical_first_boundary(), i128::from(expected[1]));
}

#[test]
fn trim_across_cycles_and_split_preserve_phase_root_identity_and_single_history_edit() {
    for musical in [false, true] {
        let (mut e, id) = if musical {
            beat_fixture()
        } else {
            frame_fixture(AudioViewMode::Repeat)
        };
        let before = e.project().snapshot();
        let root = e
            .project()
            .clip(id)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .clone();
        let cut = if musical {
            e.project()
                .tempo_map()
                .beat_to_frame(beat(25 * Q / 4))
                .unwrap()
        } else {
            43_000
        };
        let original =
            AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map())
                .unwrap()
                .occurrence(2)
                .unwrap();
        e.execute(Command::TrimClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            frame: cut,
        })
        .unwrap();
        let c = e.project().clip(id).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &root,
            &c.audio_edit().unwrap().domain
        ));
        let a = AudioViewProjection::new(c, e.project().tempo_map())
            .unwrap()
            .occurrence(0)
            .unwrap();
        assert_eq!(a.root_origin, original.root_origin);
        assert_eq!(a.destination_start, cut);
        assert_eq!(a.root_start, (i128::from(cut) - a.root_frame_origin) as u64);
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
        let cut = if musical {
            e.project().tempo_map().beat_to_frame(beat(4 * Q)).unwrap()
        } else {
            30_000
        };
        let Created::Clip(right) = e
            .execute(Command::SplitClip {
                clip: id,
                frame: cut,
            })
            .unwrap()
        else {
            panic!("split")
        };
        let left = e.project().clip(id).unwrap();
        let right = e.project().clip(right).unwrap();
        assert_eq!(left.start() + left.duration(), right.start());
        assert!(std::sync::Arc::ptr_eq(
            &root,
            &left.audio_edit().unwrap().domain
        ));
        assert!(std::sync::Arc::ptr_eq(
            &root,
            &right.audio_edit().unwrap().domain
        ));
        assert_eq!(
            left.audio_edit().unwrap().view_start,
            right.audio_edit().unwrap().view_start
        );
        assert_eq!(
            left.audio_edit().unwrap().view_end,
            right.audio_edit().unwrap().view_end
        );
        let next_id = e.project().snapshot().next_id;
        assert!(e.undo());
        let mut expected = before.clone();
        expected.next_id = next_id;
        assert_eq!(e.project().snapshot(), expected);
        assert!(e.redo());
        assert_eq!(
            Project::from_snapshot(e.project().snapshot()).unwrap(),
            *e.project()
        );
    }
}

#[test]
fn clock_toggle_and_move_retain_selected_cycle_phase_and_project_current_origin() {
    let (mut e, id) = frame_fixture(AudioViewMode::Repeat);
    let before = e.project().snapshot();
    e.execute(Command::SetAudioClipMusical {
        clip: id,
        enabled: true,
    })
    .unwrap();
    let p =
        AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map()).unwrap();
    assert_eq!(p.outer_frame_bounds(), (0, 48_000));
    assert_eq!(p.logical_first_boundary(), 18_000);
    assert_eq!(p.occurrence(0).unwrap().root_origin, -3 * i128::from(Q) / 4);
    e.execute(Command::SetAudioClipMusical {
        clip: id,
        enabled: false,
    })
    .unwrap();
    assert_eq!(e.project().snapshot(), before);
    let track = e.project().tracks()[0].id();
    e.execute(Command::MoveClip {
        clip: id,
        track,
        start: 10_000,
    })
    .unwrap();
    let p =
        AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map()).unwrap();
    assert_eq!(p.outer_frame_bounds(), (10_000, 58_000));
    assert_eq!(p.logical_first_boundary(), 28_000);
    assert_eq!(p.occurrence(0).unwrap().root_origin, -8_000);
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn once_silence_tail_trim_split_and_extend_back_preserve_consumed_origin() {
    for cut in [18_000, 30_000] {
        let (mut e, id) = frame_fixture(AudioViewMode::Once);
        let before = e.project().snapshot();
        let root = e
            .project()
            .clip(id)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .clone();
        e.execute(Command::TrimClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            frame: cut,
        })
        .unwrap();
        let c = e.project().clip(id).unwrap();
        let p = AudioViewProjection::new(c, e.project().tempo_map()).unwrap();
        assert_eq!(p.outer_frame_bounds(), (cut, 48_000));
        assert_eq!(p.root_origin(), -18_000);
        assert_eq!(p.logical_first_boundary(), 18_000);
        assert_eq!(p.occurrence_count(), 0);
        assert!(p.occurrence(0).is_err());
        assert_eq!(
            c.audio_edit().unwrap().placement.unwrap().cycle_phase,
            6_000 + cut
        );
        assert!(std::sync::Arc::ptr_eq(
            &root,
            &c.audio_edit().unwrap().domain
        ));
        e.execute(Command::TrimClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            frame: 0,
        })
        .unwrap();
        assert_eq!(e.project().snapshot(), before);
        let p = AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map())
            .unwrap();
        assert_eq!(p.occurrence(0).unwrap().destination_end, 18_000);
        assert!(e.undo());
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
    }
    let (mut e, id) = frame_fixture(AudioViewMode::Once);
    let root = e
        .project()
        .clip(id)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .clone();
    let Created::Clip(right) = e
        .execute(Command::SplitClip {
            clip: id,
            frame: 30_000,
        })
        .unwrap()
    else {
        panic!("split");
    };
    let c = e.project().clip(right).unwrap();
    let p = AudioViewProjection::new(c, e.project().tempo_map()).unwrap();
    assert_eq!(p.outer_frame_bounds(), (30_000, 48_000));
    assert_eq!((p.root_origin(), p.occurrence_count()), (-18_000, 0));
    assert!(std::sync::Arc::ptr_eq(
        &root,
        &c.audio_edit().unwrap().domain
    ));
    assert!(e.undo());
    assert!(e.redo());
    assert_eq!(
        AudioViewProjection::new(e.project().clip(right).unwrap(), e.project().tempo_map())
            .unwrap()
            .occurrence_count(),
        0
    );
}

#[test]
fn consumed_once_boundary_toggle_marker_and_clock_conversion_are_explicit() {
    let (e, id) = frame_fixture(AudioViewMode::Once);
    let mut s = e.project().snapshot();
    s.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap()
        .placement
        .as_mut()
        .unwrap()
        .cycle_phase = 60_000;
    let mut e = Editor::new(Project::from_snapshot(s).unwrap(), 32).unwrap();
    let original = e.project().snapshot();
    let p =
        AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map()).unwrap();
    assert_eq!(
        (
            p.root_origin(),
            p.logical_first_boundary(),
            p.occurrence_count()
        ),
        (-72_000, -36_000, 0)
    );
    e.execute(Command::SetAudioClipMusical {
        clip: id,
        enabled: true,
    })
    .unwrap();
    let p =
        AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map()).unwrap();
    assert_eq!(
        (p.logical_first_boundary(), p.occurrence_count()),
        (-36_000, 0)
    );
    e.execute(Command::SetAudioClipMusical {
        clip: id,
        enabled: false,
    })
    .unwrap();
    assert_eq!(e.project().snapshot(), original);
    e.execute(Command::ResizeAudioClipView {
        clip: id,
        operation: AudioViewResizeOperation::OuterEnd {
            project_frame: 96_000,
        },
    })
    .unwrap();
    let c = e.project().clip(id).unwrap();
    assert_eq!(
        c.audio_edit().unwrap().placement.unwrap().mode,
        AudioViewMode::Once
    );
    assert_eq!(
        AudioViewProjection::new(c, e.project().tempo_map())
            .unwrap()
            .occurrence_count(),
        0
    );
    e.execute(Command::ResizeAudioClipView {
        clip: id,
        operation: AudioViewResizeOperation::CycleEnd {
            project_frame: 12_000,
        },
    })
    .unwrap();
    assert_eq!(
        e.project()
            .clip(id)
            .unwrap()
            .audio_edit()
            .unwrap()
            .placement
            .unwrap()
            .cycle_phase,
        60_000
    );
    assert_eq!(
        AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map())
            .unwrap()
            .occurrence_count(),
        1
    );
    assert!(e.undo());
    assert!(e.undo());
    e.execute(Command::SetClipLooping {
        clip: id,
        looping: true,
    })
    .unwrap();
    let p =
        AudioViewProjection::new(e.project().clip(id).unwrap(), e.project().tempo_map()).unwrap();
    assert_eq!(p.occurrence_count(), 3);
    assert_eq!(
        e.project()
            .clip(id)
            .unwrap()
            .audio_edit()
            .unwrap()
            .placement
            .unwrap()
            .cycle_phase,
        12_000
    );
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), original);
}

#[test]
fn scalar_resize_captures_current_crop_and_marker_keeps_outer_bounds() {
    let (e, id) = frame_fixture(AudioViewMode::Repeat);
    let c = e.project().clip(id).unwrap();
    let tempo = e.project().tempo_map();
    let preview = preview_view_resize(
        c,
        tempo,
        AudioViewResizeOperation::CycleEnd {
            project_frame: 12_000,
        },
    )
    .unwrap();
    assert_eq!((preview.frame_start, preview.frame_end), (0, 48_000));
    assert_eq!(
        (
            preview.view_start,
            preview.view_end,
            preview.placement.cycle_phase
        ),
        (12_000, 30_000, 6_000)
    );
    assert!(
        preview_view_resize(
            c,
            tempo,
            AudioViewResizeOperation::CycleEnd {
                project_frame: 500_000
            }
        )
        .is_err()
    );
    assert!(
        preview_view_resize(
            c,
            tempo,
            AudioViewResizeOperation::OuterEnd { project_frame: 0 }
        )
        .is_err()
    );
    let (e, id) = base(false);
    let c = e.project().clip(id).unwrap();
    let p = preview_view_resize(
        c,
        e.project().tempo_map(),
        AudioViewResizeOperation::OuterEnd {
            project_frame: 960_000,
        },
    )
    .unwrap();
    assert_eq!(
        (p.view_start, p.view_end, p.placement.cycle_phase),
        (0, 480_000, 0)
    );
    assert_eq!(p.placement.outer_length, AudioDomainClock::Frames(960_000));
    assert!(c.audio_edit().unwrap().placement.is_none());
    let (e, id) = frame_fixture(AudioViewMode::Repeat);
    let mut snapshot = e.project().snapshot();
    let c = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    c.edit.as_mut().unwrap().placement = None;
    c.duration = 24_000;
    c.fade = ClipFade::neutral(c.duration).unwrap();
    let cropped = Project::from_snapshot(snapshot).unwrap();
    let p = preview_view_resize(
        cropped.clip(id).unwrap(),
        cropped.tempo_map(),
        AudioViewResizeOperation::OuterEnd {
            project_frame: 96_000,
        },
    )
    .unwrap();
    assert_eq!(
        (p.view_start, p.view_end, p.placement.cycle_phase),
        (12_000, 36_000, 0)
    );
}

#[test]
fn invalid_clock_phase_zero_span_and_overflow_refuse_without_altering_original() {
    let (e, _) = frame_fixture(AudioViewMode::Repeat);
    let original = e.project().snapshot();
    for p in [
        AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Beats(beat(Q)),
            cycle_phase: 0,
        },
        AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Frames(48_000),
            cycle_phase: 24_000,
        },
        AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Frames(0),
            cycle_phase: 0,
        },
    ] {
        let mut invalid = original.clone();
        invalid.tracks[0].audio_clips_mut().unwrap()[0]
            .edit
            .as_mut()
            .unwrap()
            .placement = Some(p);
        assert!(Project::from_snapshot(invalid).is_err());
    }
    let (e, _) = beat_fixture();
    let mut invalid = e.project().snapshot();
    let c = &mut invalid.tracks[0].audio_clips_mut().unwrap()[0];
    c.edit
        .as_mut()
        .unwrap()
        .placement
        .as_mut()
        .unwrap()
        .outer_length = AudioDomainClock::Beats(beat(u64::MAX));
    assert!(Project::from_snapshot(invalid).is_err());
    let mut invalid = e.project().snapshot();
    let placement = invalid.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap()
        .placement
        .as_mut()
        .unwrap();
    placement.cycle_phase = 2 * Q - 1;
    assert!(
        Project::from_snapshot(invalid).is_err(),
        "positive beat span that quantizes to no first frame must refuse"
    );
    assert_eq!(
        Project::from_snapshot(original.clone()).unwrap().snapshot(),
        original
    );
}

#[test]
fn canonical_audio_loop_growth_overwrites_both_kinds_at_capacity_and_marker_keeps_occupancy() {
    for at_capacity in [false, true] {
        let (mut e, id) = frame_fixture(AudioViewMode::Repeat);
        let track = e.project().tracks()[0].id();
        let asset = e.project().clip(id).unwrap().asset();
        let Created::Clip(audio) = e
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 60_000,
                duration: 40_000,
                source_offset: 2_000,
                source_length: 48_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        let Created::Clip(midi) = e
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(2 * Q),
                length: beat(Q / 2),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        if at_capacity {
            let mut s = e.project().snapshot();
            let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
                &mut s.tracks[0].content
            else {
                panic!("hybrid")
            };
            let template = midi_clips[0].clone();
            for n in 0..hex_project::MAX_CLIPS - 3 {
                let mut c = template.clone();
                c.id = s.next_id;
                s.next_id += 1;
                c.start_raw = (10 + n as u64) * Q;
                midi_clips.push(c);
            }
            e = Editor::new(Project::from_snapshot(s).unwrap(), 16).unwrap();
        }
        let before = e.project().snapshot();
        let root = e
            .project()
            .clip(id)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .clone();
        let mut prepares = 0;
        e.execute_prepared(
            Command::ResizeAudioClipView {
                clip: id,
                operation: AudioViewResizeOperation::OuterEnd {
                    project_frame: 84_000,
                },
            },
            |candidate| {
                prepares += 1;
                assert_eq!(candidate.clip(id).unwrap().duration(), 84_000);
                assert!(candidate.midi_clip(midi).is_none());
                let tail = candidate.clip(audio).unwrap();
                assert_eq!(
                    (tail.start(), tail.duration(), tail.source_offset()),
                    (84_000, 16_000, 26_000)
                );
                assert!(std::sync::Arc::ptr_eq(
                    &root,
                    &candidate.clip(id).unwrap().audio_edit().unwrap().domain
                ));
                assert_eq!(
                    Project::from_snapshot(candidate.snapshot()).unwrap(),
                    *candidate
                );
                Ok::<_, hex_project::Invalid>(())
            },
        )
        .unwrap();
        assert_eq!(prepares, 1);
        let after = e.project().snapshot();
        let count = |s: &hex_project::Snapshot| {
            s.tracks
                .iter()
                .map(|t| match &t.content {
                    hex_project::TrackContentSnapshot::Audio(clips) => clips.len(),
                    hex_project::TrackContentSnapshot::Instrument(clips) => clips.len(),
                    hex_project::TrackContentSnapshot::Hybrid {
                        audio_clips,
                        midi_clips,
                    } => audio_clips.len() + midi_clips.len(),
                })
                .sum::<usize>()
        };
        assert_eq!(count(&after), count(&before) - 1);
        if at_capacity {
            assert_eq!(count(&before), hex_project::MAX_CLIPS);
        }
        println!(
            "audio_loop_outer capacity={at_capacity} before={} after={} tail_source_offset={}",
            count(&before),
            count(&after),
            e.project().clip(audio).unwrap().source_offset()
        );
        let tail = e.project().clip(audio).unwrap().clone();
        e.execute(Command::ResizeAudioClipView {
            clip: id,
            operation: AudioViewResizeOperation::CycleEnd {
                project_frame: 12_000,
            },
        })
        .unwrap();
        assert_eq!(e.project().clip(audio), Some(&tail));
        assert_eq!(e.project().clip(id).unwrap().duration(), 84_000);
        assert_eq!(
            e.project().clip(id).unwrap().audio_edit().unwrap().view_end,
            30_000
        );
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), after);
        assert!(e.undo());
        let mut restored = before.clone();
        restored.next_id = after.next_id;
        assert_eq!(e.project().snapshot(), restored);
        let prior = e.project().snapshot();
        let refused = e.execute_prepared(
            Command::ResizeAudioClipView {
                clip: id,
                operation: AudioViewResizeOperation::OuterEnd {
                    project_frame: 84_000,
                },
            },
            |_| Err::<(), _>(hex_project::Invalid("injected graph admission refusal")),
        );
        assert!(refused.is_err());
        assert_eq!(e.project().snapshot(), prior);
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
}

#[test]
fn canonical_audio_view_loop_toggle_retains_cycle_phase_domain_and_undo() {
    let (mut e, id) = frame_fixture(AudioViewMode::Repeat);
    let before = e.project().snapshot();
    let root = e
        .project()
        .clip(id)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .clone();
    e.execute(Command::SetClipLooping {
        clip: id,
        looping: false,
    })
    .unwrap();
    let c = e.project().clip(id).unwrap();
    let edit = c.audio_edit().unwrap();
    assert_eq!(edit.placement.unwrap().mode, AudioViewMode::Once);
    assert_eq!(
        (
            edit.view_start,
            edit.view_end,
            edit.placement.unwrap().cycle_phase
        ),
        (12_000, 36_000, 6_000)
    );
    assert!(std::sync::Arc::ptr_eq(&root, &edit.domain));
    let p = AudioViewProjection::new(c, e.project().tempo_map()).unwrap();
    assert_eq!(p.occurrence_count(), 1);
    assert_eq!(p.occurrence(0).unwrap().destination_end, 18_000);
    e.execute(Command::SetClipLooping {
        clip: id,
        looping: true,
    })
    .unwrap();
    assert_eq!(e.project().snapshot(), before);
    assert!(e.undo());
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn shift_stretch_placement_keeps_requested_edge_and_opposite_outer_edge() {
    let (e, id) = frame_fixture(AudioViewMode::Repeat);
    let clip = e.project().clip(id).unwrap();
    for (edge, target, expected) in [
        (TrimEdge::Right, 72_000, (0, 72_000)),
        (TrimEdge::Left, 12_000, (12_000, 48_000)),
    ] {
        let next = hex_project::preview_audio_clip_stretch(
            clip,
            edge,
            target,
            e.project().tempo_map(),
            480_000,
        )
        .unwrap();
        assert_eq!((next.start(), next.start() + next.duration()), expected);
        assert!(next.audio_edit().unwrap().domain.warp_enabled);
        assert_eq!(
            next.audio_edit().unwrap().placement.unwrap().mode,
            AudioViewMode::Repeat
        );
    }
}

#[test]
fn lower_fade_resize_placement_does_not_clamp_outer_extent_to_single_root_end() {
    let (mut e, id) = frame_fixture(AudioViewMode::Repeat);
    e.execute(Command::ResizeAudioClipView {
        clip: id,
        operation: AudioViewResizeOperation::OuterEnd {
            project_frame: 960_000,
        },
    })
    .unwrap();
    let clip = e.project().clip(id).unwrap();
    let next = clip
        .preview_fade_edge_trim(TrimEdge::Right, 984_000, 480_000, e.project().tempo_map())
        .unwrap();
    assert_eq!((next.start(), next.duration()), (0, 984_000));
    let edit = next.audio_edit().unwrap();
    assert_eq!(edit.placement.unwrap().cycle_phase, 6_000);
    assert_eq!(
        (
            edit.domain.source_offset,
            edit.domain.source_length,
            edit.domain.pitch_cents
        ),
        (0, 480_000, 123)
    );
    assert_eq!(edit.domain.clock, AudioDomainClock::Frames(480_000));
}

#[test]
fn canonical_operation_recomputes_current_crop_instead_of_accepting_old_preview_scalars() {
    let (mut e, id) = base(false);
    let operation = AudioViewResizeOperation::OuterEnd {
        project_frame: 960_000,
    };
    let old = preview_view_resize(
        e.project().clip(id).unwrap(),
        e.project().tempo_map(),
        operation,
    )
    .unwrap();
    assert_eq!(old.view_end, 480_000);
    e.execute(Command::TrimClipEdge {
        clip: id,
        edge: TrimEdge::Right,
        frame: 240_000,
    })
    .unwrap();
    let before = e.project().snapshot();
    e.execute(Command::ResizeAudioClipView {
        clip: id,
        operation,
    })
    .unwrap();
    let clip = e.project().clip(id).unwrap();
    assert_eq!(clip.duration(), 960_000);
    assert_eq!(clip.audio_edit().unwrap().view_end, 240_000);
    let projection = AudioViewProjection::new(clip, e.project().tempo_map()).unwrap();
    assert_eq!(projection.logical_first_boundary(), 240_000);
    assert_eq!(projection.occurrence_count(), 4);
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    let mut prepares = 0;
    assert!(
        e.execute_prepared(
            Command::ResizeAudioClipView {
                clip: id,
                operation: AudioViewResizeOperation::CycleEnd {
                    project_frame: 100_000
                }
            },
            |_| {
                prepares += 1;
                Ok::<_, hex_project::Invalid>(())
            }
        )
        .is_err()
    );
    assert_eq!(prepares, 0);
    assert_eq!(e.project().snapshot(), before);
    assert!(e.redo());
    assert_eq!(e.project().clip(id).unwrap().duration(), 960_000);
}

#[test]
fn explicit_repeat_outer_activates_once_in_both_clocks_without_changing_retained_cycle() {
    for musical in [false, true] {
        for exhausted in [false, true] {
            let (editor, id) = if musical {
                beat_fixture()
            } else {
                frame_fixture(AudioViewMode::Once)
            };
            let mut snapshot = editor.project().snapshot();
            let saved = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
            let edit = saved.edit.as_mut().unwrap();
            let cycle = edit.view_end - edit.view_start;
            let phase = cycle / 4 + if exhausted { 2 * cycle } else { 0 };
            let placement = edit.placement.as_mut().unwrap();
            placement.mode = AudioViewMode::Once;
            placement.cycle_phase = phase;
            let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
            let before = e.project().snapshot();
            let clip = e.project().clip(id).unwrap();
            let original = clip.audio_edit().unwrap().clone();
            let target = if musical {
                e.project().tempo_map().beat_to_frame(beat(10 * Q)).unwrap()
            } else {
                96_000
            };
            let plain = preview_view_resize(
                clip,
                e.project().tempo_map(),
                AudioViewResizeOperation::OuterEnd {
                    project_frame: target,
                },
            )
            .unwrap();
            assert_eq!(plain.placement.mode, AudioViewMode::Once);
            assert_eq!(plain.placement.cycle_phase, phase);
            let operation = AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: target,
            };
            let preview = preview_view_resize(clip, e.project().tempo_map(), operation).unwrap();
            assert_eq!(preview.placement.mode, AudioViewMode::Repeat);
            assert_eq!(preview.placement.cycle_phase, cycle / 4);
            assert_eq!(
                (preview.frame_start, preview.frame_end),
                (clip.start(), target)
            );
            assert_eq!(e.project().snapshot(), before, "preview cannot mutate");
            e.execute(Command::ResizeAudioClipView {
                clip: id,
                operation,
            })
            .unwrap();
            let clip = e.project().clip(id).unwrap();
            let edit = clip.audio_edit().unwrap();
            assert_eq!(edit.placement, Some(preview.placement));
            assert_eq!(
                (edit.view_start, edit.view_end),
                (original.view_start, original.view_end)
            );
            assert!(std::sync::Arc::ptr_eq(&original.domain, &edit.domain));
            let projection = AudioViewProjection::new(clip, e.project().tempo_map()).unwrap();
            let start = original.musical_start.map_or(clip.start(), BeatTime::raw);
            let first = start + cycle - cycle / 4;
            let expected_boundary = if musical {
                e.project().tempo_map().beat_to_frame(beat(first)).unwrap()
            } else {
                first
            };
            assert_eq!(
                projection.logical_first_boundary(),
                i128::from(expected_boundary)
            );
            assert!(projection.occurrence_count() > 1);
            assert_eq!(
                projection.occurrence(0).unwrap().destination_end,
                expected_boundary
            );
            let after = e.project().snapshot();
            assert_eq!(
                Project::from_snapshot(after.clone()).unwrap().snapshot(),
                after
            );
            e.execute(Command::ResizeAudioClipView {
                clip: id,
                operation,
            })
            .unwrap();
            assert_eq!(e.project().snapshot(), after, "reasserting is a no-op");
            assert!(e.undo());
            assert_eq!(e.project().snapshot(), before);
            assert!(e.redo());
            assert_eq!(e.project().snapshot(), after);
            println!(
                "repeat_outer musical={musical} exhausted={exhausted} phase={phase} normalized={} boundary={expected_boundary}",
                cycle / 4
            );
        }
    }
}

#[test]
fn explicit_repeat_outer_recomputes_current_raw_loop_crop_and_preserves_processing() {
    let (mut e, id) = base(false);
    e.execute(Command::SetClipLooping {
        clip: id,
        looping: true,
    })
    .unwrap();
    e.execute(Command::TrimClipEdge {
        clip: id,
        edge: TrimEdge::Left,
        frame: 12_000,
    })
    .unwrap();
    let operation = AudioViewResizeOperation::RepeatOuterEnd {
        project_frame: 960_000,
    };
    let stale = preview_view_resize(
        e.project().clip(id).unwrap(),
        e.project().tempo_map(),
        operation,
    )
    .unwrap();
    e.execute(Command::TrimClipEdge {
        clip: id,
        edge: TrimEdge::Right,
        frame: 240_000,
    })
    .unwrap();
    let before = e.project().snapshot();
    let original = e.project().clip(id).unwrap().audio_edit().unwrap().clone();
    assert!(original.domain.looping);
    assert!(original.placement.is_none());
    e.execute(Command::ResizeAudioClipView {
        clip: id,
        operation,
    })
    .unwrap();
    let clip = e.project().clip(id).unwrap();
    let edit = clip.audio_edit().unwrap();
    assert_ne!(edit.view_end, stale.view_end);
    assert_eq!(
        (edit.view_start, edit.view_end),
        (original.view_start, original.view_end)
    );
    assert_eq!(edit.placement.unwrap().cycle_phase, 0);
    assert_eq!(edit.placement.unwrap().mode, AudioViewMode::Repeat);
    assert!(std::sync::Arc::ptr_eq(&original.domain, &edit.domain));
    assert_eq!(
        (clip.start(), clip.start() + clip.duration()),
        (12_000, 960_000)
    );
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn explicit_repeat_outer_overwrites_both_kinds_and_refusal_preserves_history() {
    let (mut e, id) = frame_fixture(AudioViewMode::Once);
    let track = e.project().tracks()[0].id();
    let asset = e.project().clip(id).unwrap().asset();
    let Created::Clip(audio) = e
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 60_000,
            duration: 40_000,
            source_offset: 2_000,
            source_length: 48_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let Created::Clip(midi) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(2 * Q),
            length: beat(Q / 2),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    let before = e.project().snapshot();
    let command = Command::ResizeAudioClipView {
        clip: id,
        operation: AudioViewResizeOperation::RepeatOuterEnd {
            project_frame: 84_000,
        },
    };
    let check = |candidate: &Project| {
        assert_eq!(
            candidate
                .clip(id)
                .unwrap()
                .audio_edit()
                .unwrap()
                .placement
                .unwrap()
                .mode,
            AudioViewMode::Repeat
        );
        assert!(candidate.midi_clip(midi).is_none());
        let tail = candidate.clip(audio).unwrap();
        assert_eq!(
            (tail.start(), tail.duration(), tail.source_offset()),
            (84_000, 16_000, 26_000)
        );
    };
    e.execute_prepared(command.clone(), |candidate| {
        check(candidate);
        Ok::<_, hex_project::Invalid>(())
    })
    .unwrap();
    let after = e.project().snapshot();
    assert!(e.undo());
    let undone = e.project().snapshot();
    let mut expected = before;
    expected.next_id = undone.next_id;
    assert_eq!(undone, expected);
    let mut prepares = 0;
    assert!(
        e.execute_prepared(command, |candidate| {
            prepares += 1;
            check(candidate);
            Err::<(), _>(hex_project::Invalid("injected preparation refusal"))
        })
        .is_err()
    );
    assert_eq!(prepares, 1);
    assert_eq!(e.project().snapshot(), undone);
    assert!(
        e.execute_prepared(
            Command::ResizeAudioClipView {
                clip: id,
                operation: AudioViewResizeOperation::RepeatOuterEnd { project_frame: 0 },
            },
            |_| {
                prepares += 1;
                Ok::<_, hex_project::Invalid>(())
            }
        )
        .is_err()
    );
    assert_eq!(prepares, 1, "invalid bounds refuse before prepare");
    assert_eq!(e.project().snapshot(), undone);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn materialized_audio_view_preview_matches_canonical_candidate_without_editing_source() {
    for musical in [false, true] {
        let (mut e, id) = if musical {
            beat_fixture()
        } else {
            frame_fixture(AudioViewMode::Once)
        };
        for marker in [false, true] {
            let before = e.project().snapshot();
            let original = e.project().clip(id).unwrap();
            let root = original.audio_edit().unwrap().domain.clone();
            let frame = if musical {
                e.project()
                    .tempo_map()
                    .beat_to_frame(beat(if marker { 2 * Q } else { 10 * Q }))
                    .unwrap()
            } else if marker {
                12_000
            } else {
                96_000
            };
            let operation = if marker {
                AudioViewResizeOperation::CycleEnd {
                    project_frame: frame,
                }
            } else {
                AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: frame,
                }
            };
            let scalar = preview_view_resize(original, e.project().tempo_map(), operation).unwrap();
            let preview =
                preview_audio_view_resize(original, e.project().tempo_map(), 480_000, operation)
                    .unwrap();
            assert_eq!(
                (preview.start(), preview.start() + preview.duration()),
                (scalar.frame_start, scalar.frame_end)
            );
            assert_eq!(
                preview.audio_edit().unwrap().placement,
                Some(scalar.placement)
            );
            assert!(std::sync::Arc::ptr_eq(
                &root,
                &preview.audio_edit().unwrap().domain
            ));
            assert_eq!(e.project().snapshot(), before);
            e.execute_prepared(
                Command::ResizeAudioClipView {
                    clip: id,
                    operation,
                },
                |candidate| {
                    assert_eq!(candidate.clip(id), Some(&preview));
                    Ok::<_, hex_project::Invalid>(())
                },
            )
            .unwrap();
            assert!(e.undo());
            assert_eq!(e.project().snapshot(), before);
            println!(
                "materialized_audio_view musical={musical} marker={marker} bounds={:?}",
                (scalar.frame_start, scalar.frame_end)
            );
        }
    }
}

#[test]
fn materialized_audio_view_refuses_wrong_asset_and_does_not_submit_stale_preview() {
    let (mut e, id) = base(false);
    let operation = AudioViewResizeOperation::RepeatOuterEnd {
        project_frame: 960_000,
    };
    let original = e.project().clip(id).unwrap();
    assert!(preview_view_resize(original, e.project().tempo_map(), operation).is_ok());
    assert!(preview_audio_view_resize(original, e.project().tempo_map(), 0, operation).is_err());
    let old =
        preview_audio_view_resize(original, e.project().tempo_map(), 480_000, operation).unwrap();
    e.execute(Command::TrimClipEdge {
        clip: id,
        edge: TrimEdge::Right,
        frame: 240_000,
    })
    .unwrap();
    let before = e.project().snapshot();
    let current = preview_audio_view_resize(
        e.project().clip(id).unwrap(),
        e.project().tempo_map(),
        480_000,
        operation,
    )
    .unwrap();
    assert_ne!(
        old.audio_edit().unwrap().view_end,
        current.audio_edit().unwrap().view_end
    );
    e.execute_prepared(
        Command::ResizeAudioClipView {
            clip: id,
            operation,
        },
        |candidate| {
            assert_eq!(candidate.clip(id), Some(&current));
            assert_ne!(candidate.clip(id), Some(&old));
            Ok::<_, hex_project::Invalid>(())
        },
    )
    .unwrap();
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn session_audio_binding_keeps_ordinary_copy_and_refuses_unrepresented_placement_atomically() {
    for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
        let (mut e, clip) = frame_fixture(mode);
        let Created::Scene(scene) = e
            .execute(Command::AddScene {
                name: "target".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let before = e.project().snapshot();
        assert!(
            e.execute(Command::CopyArrangementAudioClipToSession {
                source: clip,
                scene
            })
            .is_err()
        );
        assert_eq!(e.project().snapshot(), before);
        assert!(e.undo(), "refusal must not consume a history entry");
        assert!(e.project().scenes().is_empty());
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), before);
    }
    let mut e = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "ordinary".into(),
            path: "not-opened.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let before = e.project().snapshot();
    let Created::SessionClip(copied) = e
        .execute(Command::CopyArrangementAudioClipToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    let copied = e.project().session_audio_clip(copied).unwrap();
    assert_eq!(copied.asset(), e.project().clip(clip).unwrap().asset());
    assert_eq!((copied.source_start(), copied.source_end()), (0, 48_000));
    assert!(e.undo());
    let mut expected = before;
    expected.next_id = e.project().snapshot().next_id;
    assert_eq!(e.project().snapshot(), expected);
}

#[test]
fn review_phase_fade_uses_actual_root_origin_before_and_after_tempo_edit() {
    for phase in [0, Q / 2] {
        let (mut setup, clip) = beat_fixture();
        setup
            .execute(Command::SetTempoPoint {
                beat: beat(3 * Q / 2),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let mut snapshot = setup.project().snapshot();
        snapshot.tracks[0].audio_clips_mut().unwrap()[0]
            .edit
            .as_mut()
            .unwrap()
            .placement
            .as_mut()
            .unwrap()
            .cycle_phase = phase;
        let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
        let before = e.project().snapshot();
        let authored = e.project().tempo_map().clone();
        let fade = ClipFade::new(e.project().clip(clip).unwrap().duration(), 0, 48_000, 0).unwrap();
        e.execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        let faded = e.project().snapshot();
        let check = |project: &Project| {
            let c = project.clip(clip).unwrap();
            let edit = c.audio_edit().unwrap();
            let projection = AudioViewProjection::new(c, project.tempo_map()).unwrap();
            let expected_origin = projection.root_origin() + i128::from(edit.domain.fade_start);
            let delta = i128::from(3 * Q - edit.domain.fade_start);
            let age = u64::try_from(
                hex_project::audio_projection_frame(&authored, expected_origin + delta).unwrap()
                    - hex_project::audio_projection_frame(&authored, expected_origin).unwrap(),
            )
            .unwrap();
            let frame = edit.root_frame_at(3 * Q, project.tempo_map()).unwrap();
            let actual = edit
                .projected_fade(project.tempo_map())
                .unwrap()
                .gain_at(frame)
                .unwrap();
            let expected = fade.gain_at(age);
            eprintln!(
                "phase={phase} root={} fade_start={} anchor={} stored={} age={age} gain={actual} expected={expected}",
                projection.root_origin(),
                edit.domain.fade_start,
                expected_origin,
                edit.domain
                    .fade_projection
                    .as_ref()
                    .unwrap()
                    .origin_beat_raw
            );
            assert!(
                (actual - expected).abs() < 1e-6,
                "actual projected envelope age phase={phase}"
            );
            assert_eq!(
                edit.domain
                    .fade_projection
                    .as_ref()
                    .unwrap()
                    .origin_beat_raw,
                expected_origin
            );
            assert_eq!(
                edit.domain
                    .fade_projection
                    .as_ref()
                    .unwrap()
                    .local_start_raw,
                edit.domain.fade_start
            );
            assert_eq!(edit.placement.unwrap().cycle_phase, phase);
        };
        check(e.project());
        e.execute(Command::SetTempoPoint {
            beat: beat(Q),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
        check(e.project());
        let changed = e.project().snapshot();
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), faded);
        assert!(e.undo());
        assert_eq!(e.project().snapshot(), before);
        assert!(!e.undo());
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), faded);
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), changed);
        check(e.project());
    }
}

#[test]
fn review_phase_repeat_cycle_end_refuses_at_or_before_start_without_history_change() {
    for mode in [AudioViewMode::Repeat, AudioViewMode::Once] {
        for phase in [0, 6_000] {
            for delta in [-1i64, 0, 1] {
                let (mut setup, clip) = frame_fixture(mode);
                let track = setup.project().tracks()[0].id();
                setup
                    .execute(Command::MoveClip {
                        clip,
                        track,
                        start: 12_000,
                    })
                    .unwrap();
                let mut snapshot = setup.project().snapshot();
                snapshot.tracks[0].audio_clips_mut().unwrap()[0]
                    .edit
                    .as_mut()
                    .unwrap()
                    .placement
                    .as_mut()
                    .unwrap()
                    .cycle_phase = phase;
                let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
                e.execute(Command::SetAudioClipGain { clip, millidb: 200 })
                    .unwrap();
                let redo = e.project().snapshot();
                assert!(e.undo());
                let before = e.project().snapshot();
                let old_origin = AudioViewProjection::new(
                    e.project().clip(clip).unwrap(),
                    e.project().tempo_map(),
                )
                .unwrap()
                .root_origin();
                let target = u64::try_from(12_000i64 + delta).unwrap();
                let command = Command::ResizeAudioClipView {
                    clip,
                    operation: AudioViewResizeOperation::CycleEnd {
                        project_frame: target,
                    },
                };
                let result = e.execute(command);
                let must_refuse = (mode == AudioViewMode::Repeat && delta <= 0)
                    || i128::from(phase) + i128::from(delta) <= 0;
                if must_refuse {
                    assert!(
                        result.is_err(),
                        "Repeat cannot normalize an invalid requested boundary: mode={mode:?} phase={phase} delta={delta}"
                    );
                    assert_eq!(e.project().snapshot(), before);
                    assert!(!e.can_undo());
                    assert!(e.can_redo());
                    assert!(e.redo());
                    assert_eq!(e.project().snapshot(), redo);
                } else {
                    result.unwrap();
                    let c = e.project().clip(clip).unwrap();
                    let p = AudioViewProjection::new(c, e.project().tempo_map()).unwrap();
                    assert_eq!(p.logical_first_boundary(), i128::from(target));
                    assert_eq!(p.root_origin(), old_origin);
                    assert_eq!(p.outer_frame_bounds(), (12_000, 60_000));
                    assert_eq!(
                        c.audio_edit().unwrap().placement.unwrap().cycle_phase,
                        phase
                    );
                    if mode == AudioViewMode::Once && delta <= 0 {
                        assert_eq!(p.occurrence_count(), 0);
                    }
                    let after = e.project().snapshot();
                    assert!(e.undo());
                    assert_eq!(e.project().snapshot(), before);
                    assert!(e.redo());
                    assert_eq!(e.project().snapshot(), after);
                }
            }
        }
    }
}
