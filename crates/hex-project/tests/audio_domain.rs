use hex_project::{
    AudioDomainClock, BeatTime, Command, Created, Editor, Project, Tempo, TrimEdge, WarpMode,
    audio_projection_frame, default_audio_warp,
};

fn fixture(looping: bool) -> (Editor, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "root.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetClipLooping { clip, looping })
        .unwrap();
    if looping {
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 7 })
            .unwrap();
    }
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 137,
            fade_out: 211,
        })
        .unwrap();
    (editor, clip)
}
#[test]
fn transformed_split_and_crop_retain_source_phase_and_unsliced_root() {
    let (mut editor, clip) = fixture(true);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let root = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .clone();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip,
            frame: 17_123,
        })
        .unwrap()
    else {
        panic!("split")
    };
    let left = editor.project().clip(clip).unwrap();
    let right_clip = editor.project().clip(right).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &root,
        &left.audio_edit().unwrap().domain
    ));
    assert!(std::sync::Arc::ptr_eq(
        &root,
        &right_clip.audio_edit().unwrap().domain
    ));
    assert_eq!(left.source_offset(), right_clip.source_offset());
    assert_eq!(right_clip.source_phase(), 7);
    assert_eq!(right_clip.audio_edit().unwrap().view_start, 17_123);
    assert_eq!(root.fade.fade_in(), 137);
    editor
        .execute(Command::TrimClipEdge {
            clip: right,
            edge: TrimEdge::Left,
            frame: 19_000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip: right,
            edge: TrimEdge::Left,
            frame: 17_123,
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .clip(right)
            .unwrap()
            .audio_edit()
            .unwrap()
            .view_start,
        17_123
    );
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::TrimClipEdge {
                clip: right,
                edge: TrimEdge::Right,
                frame: 48_001
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert_eq!(Project::from_snapshot(before.snapshot()).unwrap(), before);
}
#[test]
fn musical_endpoints_tempo_split_and_tail_to_zero_use_one_projection() {
    let (mut editor, clip) = fixture(true);
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip,
            frame: 17_123,
        })
        .unwrap()
    else {
        panic!("split")
    };
    let cut = editor
        .project()
        .clip(right)
        .unwrap()
        .audio_edit()
        .unwrap()
        .view_start;
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .view_end,
        cut
    );
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let left = editor.project().clip(clip).unwrap();
    let tail = editor.project().clip(right).unwrap();
    assert_eq!(left.start() + left.duration(), tail.start());
    assert_eq!(tail.start() + tail.duration(), 72_000);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip: right,
            track,
            start: 0,
        })
        .unwrap();
    let tail = editor.project().clip(right).unwrap();
    let edit = tail.audio_edit().unwrap();
    assert_eq!(tail.start(), 0);
    assert!(edit.projection_origin().unwrap() < 0);
    assert_eq!(tail.source_phase(), 7);
    assert!(matches!(edit.domain.clock, AudioDomainClock::Beats(_)));
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}
#[test]
fn negative_virtual_projection_half_ties_later_and_bounds_are_checked() {
    let map = hex_project::TempoMap::constant(8_000, Tempo::new(1_000_000_000).unwrap()).unwrap();
    // This tempo maps one beat to 480 frames; exactly -1/32 beat is -15 frames.
    assert_eq!(
        audio_projection_frame(&map, -i128::from(BeatTime::ONE.raw() / 32)).unwrap(),
        -15
    );
    // At 8 kHz/120 bpm, 1/64 beat is 62.5 frames. Both half ties move later.
    let map = hex_project::TempoMap::constant(8_000, Tempo::DEFAULT).unwrap();
    let half = i128::from(BeatTime::ONE.raw() / 64);
    assert_eq!(audio_projection_frame(&map, half).unwrap(), 63);
    assert_eq!(audio_projection_frame(&map, -half).unwrap(), -62);
    assert!(audio_projection_frame(&map, i128::MIN).is_err());
}

#[test]
fn neutral_metadata_edits_preserve_legacy_fades_and_musical_fade_switch_refuses_atomically() {
    let (mut editor, clip) = fixture(true);
    let original = editor.project().clone();
    for command in [
        Command::SetAudioClipPitch { clip, cents: 0 },
        Command::SetAudioClipWarp {
            clip,
            settings: None,
        },
        Command::SetAudioClipMusical {
            clip,
            enabled: false,
        },
        Command::SetAudioClipSourceGain {
            clip,
            points: vec![],
        },
    ] {
        editor.execute(command).unwrap();
        assert_eq!(editor.project(), &original);
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
    }
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let Created::Clip(tail) = editor
        .execute(Command::SplitClip {
            clip,
            frame: 17_123,
        })
        .unwrap()
    else {
        panic!("split")
    };
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ONE,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetAudioClipMusical {
                clip: tail,
                enabled: false
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(editor.undo());
    assert!(editor.redo());
    assert_eq!(editor.project(), &before);
}
#[test]
fn invalid_pitch_markers_envelope_and_domain_caps_do_not_change_project_or_history() {
    let (mut editor, clip) = fixture(false);
    let before = editor.project().clone();
    for command in [
        Command::SetAudioClipPitch { clip, cents: 4_801 },
        Command::SetAudioClipSourceGain {
            clip,
            points: vec![hex_project::SourceGainPoint {
                frame: 0,
                gain_q16: hex_project::MAX_SOURCE_GAIN + 1,
            }],
        },
        Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                hex_project::SourceGainPoint {
                    frame: 2,
                    gain_q16: 0,
                },
                hex_project::SourceGainPoint {
                    frame: 1,
                    gain_q16: 1,
                },
            ],
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
    }
    let mut settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    settings.markers[1].source_frame = 0;
    assert!(
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings)
            })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn copied_musical_edits_isolate_crop_placement_domain_and_history() {
    let (mut editor, clip) = fixture(true);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let initial = editor.project().clone();
    let original = initial.clip(clip).unwrap().clone();
    let track = initial.tracks()[0].id();
    let mut states = vec![initial.clone()];
    let Created::Clip(copied) = editor
        .execute(Command::CopyClipTo {
            clip,
            track,
            start: 96_000,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    states.push(editor.project().clone());
    assert!(std::sync::Arc::ptr_eq(
        &original.audio_edit().unwrap().domain,
        &editor
            .project()
            .clip(copied)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain,
    ));
    for command in [
        Command::MoveClip {
            clip: copied,
            track,
            start: 120_000,
        },
        Command::TrimClipEdge {
            clip: copied,
            edge: TrimEdge::Left,
            frame: 121_000,
        },
        Command::SetAudioClipPitch {
            clip: copied,
            cents: -257,
        },
        Command::SetClipSourcePhase {
            clip: copied,
            phase: 29,
        },
        Command::SetClipFades {
            clip: copied,
            fade_in: 89,
            fade_out: 97,
        },
    ] {
        editor.execute(command).unwrap();
        assert_eq!(editor.project().clip(clip).unwrap(), &original);
        assert_eq!(initial.clip(clip).unwrap(), &original);
        states.push(editor.project().clone());
    }
    let copied_edit = editor.project().clip(copied).unwrap().audio_edit().unwrap();
    assert_ne!(
        copied_edit.live_musical_start(),
        original.audio_edit().unwrap().live_musical_start()
    );
    assert!(copied_edit.view_start > 0);
    assert_eq!(copied_edit.domain.pitch_cents, -257);
    assert_eq!(copied_edit.domain.source_phase, 29);
    assert_eq!(original.audio_edit().unwrap().domain.pitch_cents, 137);
    assert_eq!(original.audio_edit().unwrap().domain.source_phase, 7);
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
    for expected in states.iter().rev().skip(1) {
        assert!(editor.undo());
        // Stable IDs are never reused: undo preserves the allocation high-water mark.
        let mut expected = expected.snapshot();
        let actual = editor.project().snapshot();
        assert!(actual.next_id >= expected.next_id);
        expected.next_id = actual.next_id;
        assert_eq!(actual, expected);
    }
    for expected in states.iter().skip(1) {
        assert!(editor.redo());
        assert_eq!(editor.project(), expected);
    }
}

#[test]
fn lower_fade_resize_crops_musical_warp_events_without_changing_the_processing_root() {
    use hex_project::{AudioEventEdit, ClipFade};
    let (mut editor, clip) = fixture(true);
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let root_length = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .clock
        .length();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split {
                at: root_length / 2,
            },
        })
        .unwrap();
    let fade = ClipFade::new(48_000, 0, 12_000, 10_000)
        .unwrap()
        .with_shape(1000, 2000, 700, -600)
        .unwrap()
        .with_curve_positions(250, 750)
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    // Existing handle edits explicitly fit retained lengths to this 64k-frame
    // crop; the old projected domain would put the upper knots elsewhere.
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 64_000);
    for (edge, frame) in [(TrimEdge::Left, 4000), (TrimEdge::Right, 60_000)] {
        let original = editor.project().clone();
        let before = original.clip(clip).unwrap();
        let preview = before
            .preview_fade_edge_trim(edge, frame, 48_000, original.tempo_map())
            .unwrap();
        editor
            .execute(Command::TrimClipFadeEdge { clip, edge, frame })
            .unwrap();
        let changed = editor.project().clip(clip).unwrap();
        assert_eq!(changed, &preview);
        assert_eq!(
            changed.start() + changed.duration(),
            if edge == TrimEdge::Left {
                64_000
            } else {
                frame
            }
        );
        let before_root = before.audio_edit().unwrap();
        let after_root = changed.audio_edit().unwrap();
        let mut expected_domain = (*before_root.domain).clone();
        expected_domain.fade = after_root.domain.fade;
        expected_domain.fade_start = after_root.domain.fade_start;
        expected_domain.fade_projection = after_root.domain.fade_projection.clone();
        assert_eq!(
            *after_root.domain, expected_domain,
            "source, warp, pitch, phase, source gains and embedded events are retained"
        );
        assert_eq!(
            after_root.live_musical_start(),
            Some(
                editor
                    .project()
                    .tempo_map()
                    .frame_to_beat(changed.start())
                    .unwrap()
            )
        );
        let edited = changed.retained_fade();
        match edge {
            TrimEdge::Left => assert_eq!(changed.start() + edited.fade_in(), 13_000),
            TrimEdge::Right => assert_eq!(
                changed.start() + changed.duration() - edited.fade_out(),
                52_000
            ),
        }
        assert_eq!(changed.retained_fade().in_curve(), 700);
        assert_eq!(changed.retained_fade().out_curve(), -600);
        assert_eq!(
            (
                changed.retained_fade().in_position(),
                changed.retained_fade().out_position()
            ),
            (250, 750)
        );
        let snapshot = Project::from_snapshot(editor.project().snapshot()).unwrap();
        assert_eq!(&snapshot, editor.project());
        assert!(editor.undo());
        assert_eq!(editor.project(), &original);
        assert!(editor.redo());
        assert_eq!(editor.project(), &snapshot);
        assert!(editor.undo());
    }
    // Neutral retained roots also clamp beyond their immutable processing bounds.
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let before = editor.project().clone();
    editor
        .execute(Command::TrimClipFadeEdge {
            clip,
            edge: TrimEdge::Right,
            frame: u64::MAX,
        })
        .unwrap();
    assert_eq!(editor.project(), &before);
    editor
        .execute(Command::TrimClipFadeEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 4000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipFadeEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 0,
        })
        .unwrap();
    assert_eq!(editor.project(), &before);
}
