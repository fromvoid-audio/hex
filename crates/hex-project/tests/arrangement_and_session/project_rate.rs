use hex_project::{
    BeatTime, Command, Created, Editor, GainInterpolation, GainPoint, Project, SessionCellContent,
    Tempo,
};

#[test]
fn sample_rate_keeps_retained_sources_beats_loop_and_console_windows() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, clip, asset } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "Original.wav".into(),
            source_sample_rate: 44_100,
            source_frames: 44_101,
            playback_frames: 48_001,
            conversion_version: 1,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 960,
            fade_out: 480,
        })
        .unwrap();
    let Created::Clip(second) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 60_800,
            duration: 8_000,
            source_offset: 8_000,
            source_length: 8_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("second audio clip");
    };
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(vec![clip, second]))
        .unwrap()
    else {
        panic!("retained audio group");
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(8).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 1,
            velocity: 92,
        })
        .unwrap();
    let gain = GainPoint {
        beat: BeatTime::ONE,
        gain: 0.5,
        incoming_gain: None,
        outgoing: GainInterpolation::Linear,
    };
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![gain],
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(4).unwrap(),
            tempo: Tempo::new(100_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 24_000,
            enabled: true,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "cell".into(),
            asset,
            source_start: 8_000,
            source_end: 48_001,
        })
        .unwrap();

    let source = editor.project();
    for (rate, end, expected_offset, fade) in
        [(44_100, 44_101, 7_350, 882), (96_000, 96_002, 16_000, 1920)]
    {
        let converted = source.at_sample_rate(rate).unwrap();
        assert_eq!(converted.sample_rate(), rate);
        assert_eq!(converted.tempo_map().points(), source.tempo_map().points());
        assert_eq!(converted.midi_clip(midi), source.midi_clip(midi));
        assert_eq!(converted.track(track).unwrap().gain_envelope(), &[gain]);
        assert_eq!(converted.loop_region().unwrap().end(), u64::from(rate) / 2);
        let media = converted.asset(asset).unwrap();
        assert_eq!(media.path(), source.asset(asset).unwrap().path());
        assert_eq!(media.source_frames(), 44_101);
        assert_eq!(media.source_sample_rate(), 44_100);
        assert_eq!(media.playback_frames(), end);
        let SessionCellContent::Audio(cell) = converted.session_cell(track, scene).unwrap() else {
            panic!("audio cell");
        };
        assert_eq!(
            (cell.source_start(), cell.source_end()),
            (expected_offset, end)
        );
        let grouped = converted.clip(group).unwrap();
        let roots: Vec<_> = grouped.retained_audio_roots().collect();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].clip.id(), clip);
        assert_eq!(roots[0].tempo.sample_rate(), rate);
        assert_eq!(roots[0].clip.fade().fade_in(), fade);
        assert_eq!(roots[1].clip.source_offset(), expected_offset);
    }
    assert_eq!(source.sample_rate(), 48_000);
}

#[test]
fn collapsed_loop_refuses_and_same_rate_is_an_exact_no_op() {
    let mut editor = Editor::new(Project::new(96_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 1,
            enabled: true,
        })
        .unwrap();
    let project = editor.project();
    assert_eq!(project.at_sample_rate(96_000).unwrap(), *project);
    assert!(project.at_sample_rate(44_100).is_err());
    assert!(project.at_sample_rate(0).is_err());
    assert_eq!(project.loop_region().unwrap().end(), 1);
}
