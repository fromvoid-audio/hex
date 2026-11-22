use hex_project::{
    BeatTime, BeatsPreserve, BeatsTail, Command, Created, Editor, Project, Tempo, TrimEdge,
    WarpMode, default_audio_warp, preview_audio_clip_stretch,
};
fn fixture() -> (Editor, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "drums".into(),
            path: "original.wav".into(),
            sample_rate: 44_100,
            frames: 88_200,
        })
        .unwrap()
    else {
        panic!()
    };
    let warp = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    (editor, clip)
}
fn roundtrip(editor: &Editor) {
    let data = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&data).unwrap(), *editor.project());
}
#[test]
fn original_bpm_sets_musical_duration_and_survives_tempo_undo_and_reopen() {
    let (mut editor, clip) = fixture();
    let before = editor.project().clone();
    editor
        .execute(Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let sample = editor.project().clip(clip).unwrap();
    assert_eq!(
        sample.duration(),
        44_100,
        "two seconds at source60 are two beats, one second at project120"
    );
    let edit = sample.audio_edit().unwrap();
    assert_eq!(edit.view_end - edit.view_start, BeatTime::ONE.raw() * 2);
    assert_eq!(edit.domain.source_length, 88_200);
    assert_eq!(edit.domain.sample_bpm.unwrap().micro_bpm(), 60_000_000);
    roundtrip(&editor);
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(*editor.project(), before);
    assert!(editor.redo());
    assert_eq!(*editor.project(), after);
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 58_800);
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .sample_bpm,
        Some(Tempo::new(60_000_000).unwrap())
    );
    roundtrip(&editor);
}
#[test]
fn both_stretch_edges_preserve_crop_source_phase_pitch_markers_and_one_history_step() {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        let (mut editor, clip) = fixture();
        editor
            .execute(Command::SetAudioClipSampleBpm {
                clip,
                tempo: Tempo::DEFAULT,
            })
            .unwrap();
        editor
            .execute(Command::SetClipLooping {
                clip,
                looping: true,
            })
            .unwrap();
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 137 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 700 })
            .unwrap();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 441,
                fade_out: 882,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 22_050,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame: 66_150,
            })
            .unwrap();
        let before = editor.project().clone();
        let original = before.clip(clip).unwrap();
        let frame = if edge == TrimEdge::Left { 0 } else { 88_200 };
        let preview =
            preview_audio_clip_stretch(original, edge, frame, before.tempo_map(), 88_200).unwrap();
        editor
            .execute(Command::StretchAudioClipEdge { clip, edge, frame })
            .unwrap();
        let stretched = editor.project().clip(clip).unwrap();
        assert_eq!(stretched, &preview);
        assert_eq!(stretched.duration(), 66_150);
        assert_eq!(stretched.source_offset(), original.source_offset());
        assert_eq!(stretched.source_length(), original.source_length());
        assert_eq!(stretched.source_phase(), 137);
        let domain = &stretched.audio_edit().unwrap().domain;
        assert_eq!(domain.pitch_cents, 700);
        assert_eq!(domain.sample_bpm, Some(Tempo::DEFAULT));
        let old = original.audio_edit().unwrap();
        assert_eq!(
            domain
                .warp
                .as_ref()
                .unwrap()
                .markers
                .last()
                .unwrap()
                .beat
                .raw(),
            old.domain
                .warp
                .as_ref()
                .unwrap()
                .markers
                .last()
                .unwrap()
                .beat
                .raw()
                * 3
                / 2
        );
        assert_eq!(
            if edge == TrimEdge::Left {
                stretched.start() + stretched.duration()
            } else {
                stretched.start()
            },
            if edge == TrimEdge::Left {
                66_150
            } else {
                22_050
            }
        );
        roundtrip(&editor);
        let after = editor.project().clone();
        assert!(editor.undo());
        assert_eq!(*editor.project(), before);
        assert!(editor.redo());
        assert_eq!(*editor.project(), after);
    }
}
#[test]
fn disabled_bpm_and_empty_stretches_refuse_atomically_without_losing_history() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    let before = editor.project().clone();
    for command in [
        Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::DEFAULT,
        },
        Command::StretchAudioClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 0,
        },
        Command::StretchAudioClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 88_200,
        },
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(*editor.project(), before);
    }
    roundtrip(&editor);
}

#[test]
fn redeclaring_bpm_preserves_manual_stretch_factor_and_custom_marker_shape() {
    let (mut editor, clip) = fixture();
    let mut settings = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .clone()
        .unwrap();
    settings.markers.insert(
        1,
        hex_project::WarpMarker {
            source_frame: 22_050,
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
        },
    );
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::StretchAudioClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 66_150,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let sample = editor.project().clip(clip).unwrap();
    assert_eq!(sample.duration(), 99_225);
    let markers = &sample
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .as_ref()
        .unwrap()
        .markers;
    assert_eq!(markers[1].source_frame, 22_050);
    assert_eq!(
        markers[1].beat.raw() * 8,
        markers.last().unwrap().beat.raw(),
        "custom off-grid relative marker shape survives"
    );
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    roundtrip(&editor);
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .sample_bpm
            .unwrap()
            .micro_bpm(),
        90_000_000
    );
}

#[test]
fn source_bpm_uses_musical_extent_across_tempo_points_and_left_stretch_keeps_end() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: false,
        })
        .unwrap();
    let track = editor
        .project()
        .tracks()
        .iter()
        .find(|track| track.clips().iter().any(|c| c.id() == clip))
        .unwrap()
        .id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 44_100,
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(3 * BeatTime::ONE.raw()),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Beats {
            preserve: BeatsPreserve::Transients,
            tail: BeatsTail::Forward,
            decay: 100,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let sample = editor.project().clip(clip).unwrap();
    assert_eq!(sample.start(), 44_100);
    assert_eq!(
        sample.duration(),
        22_050 + 29_400,
        "two beats straddle120 and90BPM"
    );
    editor
        .execute(Command::StretchAudioClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 22_050,
        })
        .unwrap();
    let sample = editor.project().clip(clip).unwrap();
    assert_eq!(sample.start(), 22_050);
    assert_eq!(sample.start() + sample.duration(), 95_550);
    assert_eq!(
        sample.audio_edit().unwrap().view_end - sample.audio_edit().unwrap().view_start,
        3 * BeatTime::ONE.raw()
    );
    roundtrip(&editor);
}
