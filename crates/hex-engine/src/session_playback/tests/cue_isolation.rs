use super::*;

#[test]
fn cue_only_replaces_target_pending_and_leaves_independent_arrangement_sentinel_pcm() {
    let mut editor = Editor::new(session_only_project(&project(1.0, 0, 12 << 32)), 32).unwrap();
    let target = editor.project().tracks()[0].id();
    let Created::Track(sentinel) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sentinel".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(sentinel_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: sentinel,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: sentinel_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(16).unwrap(),
            pitch: 69,
            channel: 0,
            velocity: 37,
        })
        .unwrap();
    let revision = revision(editor.project(), Arc::new(hex_devices::SineTable::new()));
    let mut playback = SessionPlayback::new(revision).unwrap();
    let clip = playback.revision.session().midi_clips()[0].id;
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback.render(&mut [[0.0; 2]; 1000]).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: sentinel,
            quantization: LaunchQuantization::FourQuarter,
        })
        .unwrap();
    let sentinel_before = playback.route_status(1).unwrap();
    assert!(sentinel_before.pending.is_some());
    let before = (
        playback.performance_frame(),
        playback.arrangement_frame(),
        playback.arrangement_loop_pass,
    );
    playback
        .apply(SessionPlaybackCommand::CueMidiClip {
            track: target,
            clip,
            source_beat: BeatTime::from_quarters(2).unwrap(),
            expected_route_generation: playback.route_state_generation(),
        })
        .unwrap();
    assert_eq!(playback.route_status(1).unwrap(), sentinel_before);
    assert!(playback.route_status(0).unwrap().pending.is_none());
    assert_eq!(
        (
            playback.performance_frame(),
            playback.arrangement_frame(),
            playback.arrangement_loop_pass
        ),
        before
    );
    let release_end = playback
        .releases
        .iter()
        .map(|e| e.voice.end_frame())
        .max()
        .unwrap();
    let width = usize::try_from(release_end - before.0 + 257).unwrap();
    let mut expected = vec![[0.0; 2]; width];
    playback.revision.arrangement().tracks()[1]
        .render_additive(before.1, &mut expected)
        .unwrap();
    let mut actual = vec![[0.0; 2]; width];
    playback.render(&mut actual).unwrap();
    let tail = usize::try_from(release_end - before.0).unwrap();
    assert_eq!(
        actual[tail..],
        expected[tail..],
        "cued note began before source cursor and is not chased; independent sentinel keeps exact finite phase"
    );
    assert!(actual[tail..].iter().flatten().any(|s| s.abs() > 1e-5));
    assert_eq!(playback.route_status(1).unwrap(), sentinel_before);
}
