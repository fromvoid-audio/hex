use super::*;

fn audio_with_filter() -> SessionPlayback {
    let project = with_utility_filter_state(&audio_project(0, 32, 4), true, true);
    let audio = crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).unwrap();
    let media = audio_media(&project, &audio, &audio);
    let revision = Arc::new(
        PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let clip = revision.session().audio_clips()[0].id;
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 32];
    playback.render(&mut prefix).unwrap();
    playback
}
fn silence(playback: &mut SessionPlayback) {
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: playback.routes[0].id,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    assert!(matches!(playback.routes[0].owner, RouteOwner::Silence));
    assert!(playback.releases.is_empty());
}

#[test]
fn stopped_audio_session_drains_filter_history_like_exhausted_one_shot() {
    let mut reference = audio_with_filter();
    let mut stopped = audio_with_filter();
    silence(&mut stopped);
    let mut expected = [[0.0; 2]; 512];
    reference.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|value| value.abs() > 1.0e-6));
    let mut actual = [[0.0; 2]; 512];
    let mut offset = 0;
    for size in [1, 17, 127, 256, 111] {
        stopped.render(&mut actual[offset..offset + size]).unwrap();
        offset += size;
    }
    assert_eq!(actual, expected);
}

#[test]
fn silent_route_filter_residual_freezes_on_pause_and_cuts_on_global_discontinuity() {
    let mut reference = audio_with_filter();
    let mut paused = audio_with_filter();
    silence(&mut reference);
    silence(&mut paused);
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let position = paused.performance_frame;
    let mut output = [[1.0; 2]; 64];
    paused.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 64]);
    assert_eq!(paused.performance_frame, position);
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 64];
    reference.render(&mut expected).unwrap();
    paused.render(&mut output).unwrap();
    assert_eq!(output, expected);
    for command in [
        SessionPlaybackCommand::Stop,
        SessionPlaybackCommand::Seek(0),
    ] {
        let mut playback = audio_with_filter();
        silence(&mut playback);
        playback.apply(command).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 64]);
        assert!(
            playback.device_chains[0]
                .committed_range_is_quiescent(0..playback.device_chains[0].len())
        );
    }
}
