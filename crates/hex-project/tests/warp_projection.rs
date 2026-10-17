use hex_project::{
    BeatTime, BeatsPreserve, BeatsTail, Command, Created, Editor, Project, Tempo, TrimEdge,
    WarpMarker, WarpMode, WarpSettings, preview_audio_clip_stretch,
};
const Q32: i128 = 1_i128 << 32;
fn raw_fixture() -> (Editor, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "unchanged.wav".into(),
            sample_rate: 44_100,
            frames: 88_200,
        })
        .unwrap()
    else {
        panic!()
    };
    (editor, clip)
}
fn mode() -> WarpMode {
    WarpMode::Beats {
        preserve: BeatsPreserve::Transients,
        tail: BeatsTail::Forward,
        decay: 100,
    }
}
#[test]
fn canonical_marker_source_inverse_retains_piecewise_shape_and_loop_cycles() {
    let (mut editor, clip) = raw_fixture();
    let settings = WarpSettings {
        mode: mode(),
        markers: vec![
            WarpMarker {
                source_frame: 0,
                beat: BeatTime::ZERO,
            },
            WarpMarker {
                source_frame: 22_050,
                beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            },
            WarpMarker {
                source_frame: 88_200,
                beat: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
            },
        ],
    };
    assert_eq!(
        settings.beat_for_source_frame(22_050).unwrap().raw(),
        BeatTime::ONE.raw() / 2
    );
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 701 })
        .unwrap();
    let domain = &editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain;
    assert_eq!(
        domain.warp_phase_beat().unwrap(),
        BeatTime::ONE.raw() / 2 * 701 / 22_050
    );
    for (beat, source) in [(0, 0), (1, 31_500), (2, 50_400), (4, 88_200), (10, 226_800)] {
        assert_eq!(
            domain
                .warp_source_position_q32(u128::from(BeatTime::ONE.raw()) * beat)
                .unwrap(),
            source * Q32
        );
    }
}
#[test]
fn new_raw_stretch_maps_source_peaks_at_tempo_points_without_old_bitmap_distortion() {
    let (mut editor, clip) = raw_fixture();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(2 * BeatTime::ONE.raw()),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let project = editor.project();
    let original = project.clip(clip).unwrap();
    assert!(original.audio_edit().is_none());
    let preview = preview_audio_clip_stretch(
        original,
        TrimEdge::Right,
        132_300,
        project.tempo_map(),
        88_200,
    )
    .unwrap();
    let edit = preview.audio_edit().unwrap();
    assert_eq!(edit.domain.clock.length(), 4 * BeatTime::ONE.raw());
    let at = edit.root_position_at(22_050, project.tempo_map()).unwrap();
    assert_eq!(at, BeatTime::ONE.raw());
    assert_eq!(
        edit.domain
            .warp_source_position_q32(u128::from(at))
            .unwrap(),
        22_050 * Q32,
        "quarter-source peak follows declared source markers"
    );
    // The rejected old-output bitmap shortcut would put this real source peak
    // at old beat0.75/frame16538, rather than source frame22050.
    let old_beat = BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 4);
    assert_ne!(project.tempo_map().beat_to_frame(old_beat).unwrap(), 22_050);
    assert_eq!(project.clip(clip).unwrap(), original);
}
#[test]
fn public_projection_rejects_degenerate_marker_shapes_without_panicking() {
    for markers in [
        vec![],
        vec![WarpMarker {
            source_frame: 0,
            beat: BeatTime::ZERO,
        }],
        vec![
            WarpMarker {
                source_frame: 0,
                beat: BeatTime::ZERO
            };
            2
        ],
    ] {
        let settings = WarpSettings {
            mode: mode(),
            markers,
        };
        assert!(settings.beat_for_source_frame(0).is_err());
    }
}
