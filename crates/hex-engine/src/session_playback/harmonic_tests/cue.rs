use super::*;

#[test]
fn cue_refuses_mutable_fanout_before_cutting_source_or_destination() {
    let (original, source, destination, clip) = routed_session_fixture();
    let custom = edit_track(
        &original,
        destination,
        &[
            (harmonic_parameter(1, 1), ParameterValue::Float(0.0)),
            (harmonic_parameter(1, 2), ParameterValue::Float(1.0)),
        ],
    );
    let revision = prepared(&custom, None);
    let mut playback = Box::new(SessionPlayback::new(Arc::clone(&revision)).unwrap());
    let mut reference = Box::new(SessionPlayback::new(revision).unwrap());
    for runtime in [&mut playback, &mut reference] {
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 512]).unwrap();
    }
    assert!(playback.mutable_instruments[1].is_some());
    let before: Vec<_> = (0..playback.route_count())
        .map(|i| playback.route_status(i))
        .collect();
    let generation = playback.route_state_generation();
    assert_eq!(
        playback.apply(SessionPlaybackCommand::CueMidiClip {
            track: source,
            clip,
            source_beat: BeatTime::ONE,
            expected_route_generation: generation
        }),
        Err(SessionPlaybackError::MutableInstrumentDiagnostic(
            "midi cue does not support mutable fan-out"
        ))
    );
    assert_eq!(playback.route_state_generation(), generation);
    assert_eq!(
        (0..playback.route_count())
            .map(|i| playback.route_status(i))
            .collect::<Vec<_>>(),
        before
    );
    let mut actual = [[0.0; 2]; 37];
    let mut expected = actual;
    playback.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "both source and mutable destination remain uninterrupted on refusal"
    );
}
