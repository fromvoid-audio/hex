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

/// Resident linear PCM demonstrates why a primed graph cannot use the host
/// cursor as its processing clock when latency changes. This is not a live
/// publication contract: loops, commands, automation and source custody still
/// require independent proof.
#[test]
fn primed_pcm_origin_requires_processing_clock_offset() {
    let mut f = fixture(&[&[]], 4096);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(
            48_000,
            (0..4096).map(|i| [i as f32 / 8192.0, 0.0]).collect(),
        )
        .unwrap();
    }
    let no_latency = prepare(&f);
    f.editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: limiter(),
        })
        .unwrap();
    let with_latency = prepare(&f);
    let delay = with_latency.arrangement().main_latency_frames() as u64;
    assert_eq!(delay, 72);
    for (label, old_revision, new_revision, old_delay, new_delay) in [
        ("increase", &no_latency, &with_latency, 0, delay),
        ("decrease", &with_latency, &no_latency, delay, 0),
        ("equal", &with_latency, &with_latency, delay, delay),
    ] {
        for partition in [1, 257] {
            let mut old = SessionPlayback::new(Arc::clone(old_revision)).unwrap();
            old.play_from(0).unwrap();
            old.render(&mut vec![[0.0; 2]; 1024]).unwrap();
            let mut primed = SessionPlayback::new(Arc::clone(new_revision)).unwrap();
            primed.play_from(1024 - new_delay).unwrap();
            primed
                .render(&mut vec![[0.0; 2]; new_delay as usize])
                .unwrap();
            let mut old_pcm = [[0.0; 2]; 257];
            let mut primed_pcm = old_pcm;
            for chunk in old_pcm.chunks_mut(partition) {
                old.render(chunk).unwrap();
            }
            for chunk in primed_pcm.chunks_mut(partition) {
                primed.render(chunk).unwrap();
            }
            for i in 0..257 {
                let old_expected = (1024 + i as u64 - old_delay) as f32 / 8192.0;
                let new_expected = (1024 + i as u64 - new_delay) as f32 / 8192.0;
                assert!(
                    (old_pcm[i][0] - old_expected).abs() < 1e-6,
                    "{label} old {i}"
                );
                assert!(
                    (primed_pcm[i][0] - new_expected).abs() < 1e-6,
                    "{label} candidate {i}"
                );
            }
            // Align the content origin instead of the scalar processing cursor.
            let mut aligned = SessionPlayback::new(Arc::clone(new_revision)).unwrap();
            aligned.play_from(1024 - old_delay).unwrap();
            aligned
                .render(&mut vec![[0.0; 2]; new_delay as usize])
                .unwrap();
            let expected_processing_frame = 1024 - old_delay + new_delay;
            assert_eq!(aligned.arrangement_frame, expected_processing_frame);
            let mut aligned_pcm = [[0.0; 2]; 257];
            for chunk in aligned_pcm.chunks_mut(partition) {
                aligned.render(chunk).unwrap();
            }
            assert_eq!(aligned_pcm, old_pcm, "{label} partition={partition}");
        }
    }
}
