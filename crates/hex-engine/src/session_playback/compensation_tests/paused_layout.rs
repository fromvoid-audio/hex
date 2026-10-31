//! A latency-layout change published while paused away from the origin.
use super::*;
use hex_project::Command;

fn limiter() -> hex_lattice_contract::NativeDeviceState {
    crate::default_native_device_state(crate::DeviceTypeId::from_static("hex.limiter")).unwrap()
}

/// Built-in registries, so real latency devices (the limiter) are prepared.
fn prepare(f: &crate::signal_graph::tests::Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

/// Pause, change one track's latency, then make a further unrelated edit.
/// Both handoffs apply: the paused cursor is kept and the candidate starts
/// with empty device/compensation history there, exactly as a Seek to that
/// cursor would. Resuming then sounds like the new revision played from it.
#[test]
fn paused_latency_layout_change_applies_at_the_cursor_and_later_edits_follow() {
    let mut f = fixture(&[&[], &[]], 512);
    let first = f.tracks[0];
    f.editor
        .execute(Command::InsertDevice {
            track: first,
            before: None,
            state: limiter(),
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 40]).unwrap();
    old.apply(SessionPlaybackCommand::Pause).unwrap();
    old.render(&mut [[0.0; 2]; 16]).unwrap();
    assert_eq!(old.performance_frame, 40);

    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let latency = prepare(&f);
    assert_ne!(
        latency.arrangement().main_latency_frames(),
        old.revision.arrangement().main_latency_frames()
    );
    let mut next = SessionPlayback::new(Arc::clone(&latency)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(!next.playing);
    assert_eq!((next.performance_frame, next.arrangement_frame), (40, 40));

    // The following edit (another track) compares against the applied
    // revision above, not the stale pre-latency one.
    f.editor
        .execute(Command::Gain {
            track: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let edited = prepare(&f);
    let mut later = SessionPlayback::new(Arc::clone(&edited)).unwrap();
    later.try_inherit_state_from(&mut next).unwrap();
    assert_eq!(later.performance_frame, 40);

    let mut oracle = SessionPlayback::new(edited).unwrap();
    oracle.apply(SessionPlaybackCommand::Seek(40)).unwrap();
    for runtime in [&mut later, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = vec![[0.0; 2]; 2048];
    let mut expected = actual.clone();
    later.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "resume equals a seek to the paused cursor"
    );
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
}

/// Playing: the change applies at the next block with the transport still
/// running. Audio restarts there (tails/PDC empty, notes chased), exactly as a
/// Play-from at that cursor; a following edit applies too.
#[test]
fn playing_latency_layout_change_restarts_audio_without_stopping() {
    let mut f = fixture(&[&[], &[]], 512);
    let first = f.tracks[0];
    f.editor
        .execute(Command::InsertDevice {
            track: first,
            before: None,
            state: limiter(),
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 40]).unwrap();
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f)).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert!(next.playing);
    assert_eq!((next.performance_frame, next.arrangement_frame), (40, 40));
    f.editor
        .execute(Command::Gain {
            track: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let edited = prepare(&f);
    let mut later = SessionPlayback::new(Arc::clone(&edited)).unwrap();
    later.try_inherit_state_from(&mut next).unwrap();
    assert!(later.playing);
    assert_eq!(later.performance_frame, 40);

    let mut oracle = SessionPlayback::new(edited).unwrap();
    oracle.play_from(40).unwrap();
    let mut actual = vec![[0.0; 2]; 2048];
    let mut expected = actual.clone();
    later.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "running restart equals play-from the cursor"
    );
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
}
