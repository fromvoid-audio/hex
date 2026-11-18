//! Whole requested-output transaction and exact content-origin tests.
use super::*;
use crate::signal_graph::tests::{fixture, prepare};
use crate::{Audio, SineTable};

#[test]
fn idle_pause_freezes_queued_arrangement_pcm() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    for instrument_track in [0, 1] {
        // Muting the instrument track leaves the other track's impulse in
        // either the graph's compensation ring or its own latency-only device.
        let mut f = fixture(&[&[7], &[]], 1024);
        let track = f.tracks[instrument_track];
        f.editor
            .execute(Command::InsertInstrument {
                track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        f.editor
            .execute(Command::Gain { track, gain: 0.0 })
            .unwrap();
        let revision = prepare(&f).unwrap();
        assert_eq!(revision.arrangement().main_latency_frames(), 7);
        for partition in [1, 257] {
            let mut paused = Playback::from_prepared(Arc::clone(&revision));
            let mut uninterrupted = Playback::from_prepared(Arc::clone(&revision));
            for playback in [&mut paused, &mut uninterrupted] {
                playback.apply(PlaybackCommand::Play);
                let mut prefix = [[0.0; 2]; 4];
                playback.render(&mut prefix).unwrap();
                assert_eq!(prefix, [[0.0; 2]; 4]);
            }
            paused.apply(PlaybackCommand::Pause);
            let mut idle = [[0.0; 2]; 257];
            for chunk in idle.chunks_mut(partition) {
                paused.render(chunk).unwrap();
            }
            assert_eq!(paused.position(), uninterrupted.position());
            paused.apply(PlaybackCommand::Play);
            let mut resumed = [[0.0; 2]; 32];
            let mut expected = resumed;
            paused.render(&mut resumed).unwrap();
            uninterrupted.render(&mut expected).unwrap();
            assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
            let leaked = idle.iter().any(|frame| *frame != [0.0; 2]);
            let resume_differs = resumed != expected;
            assert!(
                !leaked && !resume_differs,
                "instrument track {instrument_track}, partition {partition}, paused leak {leaked}, resume differs {resume_differs}"
            );
        }
    }
}

#[test]
fn paused_keyboard_audition_preserves_queued_arrangement_pcm() {
    use crate::computer_keyboard::ComputerKeyboardInput;
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    for instrument_track in [0, 1] {
        let mut f = fixture(&[&[7], &[]], 1024);
        let track = f.tracks[instrument_track];
        f.editor
            .execute(Command::InsertInstrument {
                track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        assert_eq!(revision.arrangement().main_latency_frames(), 7);
        for partition in [1, 257] {
            let input = Arc::new(ComputerKeyboardInput::default());
            let mut paused = Playback::from_prepared(Arc::clone(&revision));
            let mut uninterrupted = Playback::from_prepared(Arc::clone(&revision));
            for playback in [&mut paused, &mut uninterrupted] {
                playback.apply(PlaybackCommand::Play);
                let mut prefix = [[0.0; 2]; 4];
                playback.render(&mut prefix).unwrap();
                assert_eq!(prefix, [[0.0; 2]; 4]);
            }
            paused.apply(PlaybackCommand::Pause);
            let (mut monitor, hold) = crate::PausedKeyboardMonitor::prepare_admitted_on_control(
                Arc::clone(&revision),
                None,
                f.editor.project(),
                track,
                Arc::clone(&input),
                0,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
            input.set_key(0, Some((60, 100, 1))).unwrap();
            let mut pressed = [[0.0; 2]; 257];
            for chunk in pressed.chunks_mut(partition) {
                chunk.fill([9.0; 2]);
                paused.render(chunk).unwrap();
                assert!(chunk.iter().all(|frame| *frame == [0.0; 2]));
                paused
                    .render_paused_keyboard_monitor(&mut monitor, chunk)
                    .unwrap();
            }
            input.set_key(0, None).unwrap();
            let mut released = [[0.0; 2]; 257];
            for chunk in released.chunks_mut(partition) {
                paused
                    .render_paused_keyboard_monitor(&mut monitor, chunk)
                    .unwrap();
            }
            assert_eq!(paused.position(), uninterrupted.position());
            assert!(pressed[32..].iter().any(|frame| *frame != [0.0; 2]));
            assert!(released[32..].iter().any(|frame| *frame != [0.0; 2]));
            paused.apply(PlaybackCommand::Play);
            let mut refused = [[9.0; 2]; 1];
            assert!(
                paused
                    .render_paused_keyboard_monitor(&mut monitor, &mut refused)
                    .is_err()
            );
            assert_eq!(refused, [[0.0; 2]; 1]);
            assert!(!hold.released_on_control());
            drop(monitor);
            assert!(hold.released_on_control());
            let mut resumed = [[0.0; 2]; 32];
            let mut expected = resumed;
            paused.render(&mut resumed).unwrap();
            uninterrupted.render(&mut expected).unwrap();
            assert_eq!(expected[3], [0.5, -1.0]);
            assert_eq!(
                resumed[3], expected[3],
                "track {instrument_track}, partition {partition}"
            );
            assert_eq!(resumed, expected);
            assert_eq!(pressed[..7], [[0.0; 2]; 7]);
            assert!(pressed.iter().all(|frame| frame[0] == frame[1]));
        }
    }
}

#[test]
fn resident_keyboard_pause_resumes_with_the_arrangement_history() {
    use crate::computer_keyboard::ComputerKeyboardInput;
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    for track_index in [0, 1] {
        let mut f = fixture(&[&[7], &[]], 1024);
        let track = f.tracks[track_index];
        f.editor
            .execute(Command::InsertInstrument {
                track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        for partition in [1, 257] {
            let mut playback = Playback::from_prepared(Arc::clone(&revision));
            let mut oracle = Playback::from_prepared(Arc::clone(&revision));
            let keys = Arc::new(ComputerKeyboardInput::default());
            playback
                .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(&keys))
                .unwrap();
            playback
                .prepare_resident_paused_source_monitor(None)
                .unwrap();
            for player in [&mut playback, &mut oracle] {
                player.apply(PlaybackCommand::Play);
                player.render(&mut [[0.0; 2]; 4]).unwrap();
            }
            playback.apply(PlaybackCommand::Pause);
            keys.set_key(0, Some((60, 100, 1))).unwrap();
            let mut press = [[0.0; 2]; 257];
            for block in press.chunks_mut(partition) {
                playback.render(block).unwrap();
            }
            assert_eq!(press[..7], [[0.0; 2]; 7]);
            assert!(press.iter().any(|frame| *frame != [0.0; 2]));
            keys.set_key(0, None).unwrap();
            let mut release = [[0.0; 2]; 257];
            for block in release.chunks_mut(partition) {
                playback.render(block).unwrap();
            }
            assert!(release.iter().any(|frame| *frame != [0.0; 2]));
            // Let the released keyboard note finish before comparing bare
            // Arrangement PCM, including the impulse waiting at Pause.
            let mut tail = [[0.0; 2]; 512];
            for _ in 0..16 {
                playback.render(&mut tail).unwrap();
            }
            assert_eq!(tail, [[0.0; 2]; 512]);
            assert_eq!(playback.position(), oracle.position());
            playback.apply(PlaybackCommand::Play);
            let mut resumed = [[0.0; 2]; 32];
            let mut expected = resumed;
            playback.render(&mut resumed).unwrap();
            oracle.render(&mut expected).unwrap();
            assert_eq!(expected[3], [0.5, -1.0]);
            assert_eq!(
                resumed, expected,
                "track {track_index}, partition {partition}"
            );

            playback.apply(PlaybackCommand::Pause);
            keys.set_key(1, Some((67, 100, 2))).unwrap();
            playback.render(&mut press).unwrap();
            assert!(press.iter().any(|frame| *frame != [0.0; 2]));
            playback.apply(PlaybackCommand::Play);
            playback.render(&mut press).unwrap();
            assert!(press[7..].iter().any(|frame| *frame != [0.0; 2]));
            playback.apply(PlaybackCommand::Pause);
            playback.render(&mut press).unwrap();
            assert!(press[7..].iter().any(|frame| *frame != [0.0; 2]));
            playback.apply(PlaybackCommand::Stop);
            assert!(keys.is_cancelled());
            let mut stopped = [[9.0; 2]; 257];
            playback.render(&mut stopped).unwrap();
            assert_eq!(stopped, [[0.0; 2]; 257]);
        }
    }
}

#[test]
fn paused_keyboard_monitor_checks_admission_and_exact_revision() {
    use crate::computer_keyboard::ComputerKeyboardInput;
    use crate::{PausedKeyboardMonitor, PausedMonitorAdmissionError};
    use hex_project::Command;

    let mut f = fixture(&[&[7]], 512);
    let track = f.tracks[0];
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let project = f.editor.project();
    let keys = Arc::new(ComputerKeyboardInput::default());
    let denied = PausedKeyboardMonitor::prepare_admitted_on_control(
        Arc::clone(&revision),
        None,
        project,
        track,
        Arc::clone(&keys),
        0,
        |_| Err("no capacity"),
    );
    assert!(matches!(
        denied,
        Err(PausedMonitorAdmissionError::Admission("no capacity"))
    ));
    let (mut monitor, _hold) = PausedKeyboardMonitor::prepare_admitted_on_control(
        Arc::clone(&revision),
        None,
        project,
        track,
        keys,
        0,
        |_| Ok::<_, ()>(()),
    )
    .unwrap();
    let same_files = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    let mut refused = [[1.0; 2]; 1];
    assert_eq!(
        monitor.render(&same_files, &mut refused),
        Err(SessionPlaybackError::InvalidCommand)
    );
    assert_eq!(refused, [[0.0; 2]; 1]);

    let mut active = SessionPlayback::new(revision).unwrap();
    monitor.render(&active, &mut [[0.0; 2]; 1]).unwrap();
    active.apply(SessionPlaybackCommand::Play).unwrap();
    active.render(&mut [[0.0; 2]; 1]).unwrap();
    active.apply(SessionPlaybackCommand::Pause).unwrap();
    refused.fill([1.0; 2]);
    assert_eq!(
        monitor.render(&active, &mut refused),
        Err(SessionPlaybackError::InvalidCommand)
    );
    assert_eq!(refused, [[0.0; 2]; 1]);
}

#[test]
fn authored_pause_release_keeps_queued_arrangement_pcm() {
    check_authored_pause_release(hex_project::default_instrument_state().unwrap(), false);
}

#[test]
fn mutable_pause_release_keeps_queued_arrangement_pcm() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    let class = crate::NativeDeviceClass::Instrument;
    let operator = crate::default_native_device_state_for_class(
        class,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let filtered = crate::patch_native_device_parameter_for_class(
        class,
        &operator,
        ParameterId::from_static(20_001),
        ParameterValue::Bool(true),
    )
    .unwrap();
    check_authored_pause_release(filtered, true);
}

fn check_authored_pause_release(
    instrument: hex_lattice_contract::NativeDeviceState,
    mutable: bool,
) {
    use crate::computer_keyboard::ComputerKeyboardInput;
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::{Command, Created};

    for track_index in [0, 1] {
        let mut f = fixture(&[&[7], &[]], 1024);
        let mut samples = vec![[0.0; 2]; 1024];
        samples[253] = [0.25, -0.5];
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, samples.clone()).unwrap();
        }
        let track = f.tracks[track_index];
        f.editor
            .execute(Command::InsertInstrument {
                track,
                state: instrument.clone(),
            })
            .unwrap();
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("MIDI clip required");
        };
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        assert_eq!(revision.has_arrangement_mutable_instruments(), mutable);
        let mut authored_only = [[0.0; 2]; 257];
        for (partition, keyboard) in [(1, false), (257, false), (1, true), (257, true)] {
            let mut playback = Playback::from_prepared(Arc::clone(&revision));
            let mut oracle = Playback::from_prepared(Arc::clone(&revision));
            let keys = Arc::new(ComputerKeyboardInput::default());
            if keyboard {
                playback
                    .prepare_computer_keyboard(f.editor.project(), track, Arc::clone(&keys))
                    .unwrap();
            }
            playback
                .prepare_resident_paused_source_monitor(None)
                .unwrap();
            for player in [&mut playback, &mut oracle] {
                player.apply(PlaybackCommand::Play);
                player.render(&mut [[0.0; 2]; 257]).unwrap();
                player.apply(PlaybackCommand::Pause);
                assert_eq!(player.position(), 257);
            }
            if keyboard {
                keys.set_key(0, Some((67, 100, 1))).unwrap();
            }
            let mut paused = [[0.0; 2]; 257];
            for block in paused.chunks_mut(partition) {
                playback.render(block).unwrap();
            }
            assert!(paused[32..].iter().any(|frame| *frame != [0.0; 2]));
            if keyboard {
                assert_ne!(paused[32..], authored_only[32..]);
            } else if partition == 1 {
                authored_only = paused;
            } else {
                assert_eq!(paused, authored_only);
            }
            assert_eq!(playback.position(), oracle.position());
            for player in [&mut playback, &mut oracle] {
                player.apply(PlaybackCommand::Play);
            }
            let mut resumed = [[0.0; 2]; 7];
            let mut expected = resumed;
            playback.render(&mut resumed).unwrap();
            oracle.render(&mut expected).unwrap();
            // No newly generated voice can reach Main until the queued window
            // ends. This reference advances no output frames during Pause.
            assert!(expected[3][0] - expected[3][1] >= 0.749_999, "{expected:?}");
            assert_eq!(
                resumed, expected,
                "track {track_index}, partition {partition}, keyboard {keyboard}"
            );
            assert!(paused.iter().all(|frame| frame[0] == frame[1]));
        }
    }
}

#[test]
fn compensated_pause_still_renders_a_held_note_release() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::{Command, Created};

    let mut f = fixture(&[&[7], &[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[0.0; 2]; 1024]).unwrap();
    }
    let track = f.tracks[1];
    f.editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = f
        .editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("expected MIDI clip");
    };
    f.editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 7);
    let mut playback = Playback::from_prepared(revision);
    playback.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 257];
    playback.render(&mut output).unwrap();
    assert!(output[32..].iter().any(|frame| *frame != [0.0; 2]));
    playback.apply(PlaybackCommand::Pause);
    let position = playback.position();
    playback.render(&mut output).unwrap();
    assert!(
        output[32..].iter().any(|frame| *frame != [0.0; 2]),
        "the release must remain audible beyond the queued latency"
    );
    assert_eq!(playback.position(), position);
}

#[test]
fn adding_an_empty_track_keeps_compensated_pcm_continuous() {
    use hex_project::Command;
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_active_audio_track_preserves_old_pcm_without_playing_its_past() {
    use hex_project::{Command, Created};
    for (start, frames, partition, stateful_main) in [
        (0, 256, 1, false),
        (0, 256, 257, false),
        (32, 256, 1, false),
        (32, 256, 257, false),
        (0, 26, 1, false),
        (0, 26, 257, false),
        (0, 256, 1, true),
        (0, 256, 257, true),
    ] {
        let mut f = fixture(&[&[7]], 512);
        if stateful_main {
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: crate::signal_graph::tests::state(3).unwrap(),
                })
                .unwrap();
        }
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 512]).unwrap();
        }
        let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let Created::Imported { track, asset, clip } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "active".into(),
                path: "active.wav".into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("import")
        };
        if start != 0 {
            f.editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
        }
        let source: Vec<_> = (0..frames).map(|i| [(i as f32 + 1.0) / 512.0; 2]).collect();
        f.media
            .insert(asset, Audio::new(48_000, source.clone()).unwrap());
        let revision = prepare(&f).unwrap();
        let mut masked = source;
        if start == 0 {
            let prefix = masked.len().min(32);
            masked[..prefix].fill([0.0; 2]);
        }
        f.media.insert(asset, Audio::new(48_000, masked).unwrap());
        let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        reference.render(&mut [[0.0; 2]; 32]).unwrap();
        let mut next = SessionPlayback::new(revision).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 128];
        let mut expected = actual;
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(
            actual, expected,
            "start {start} frames {frames} partition {partition} main {stateful_main}"
        );
        if frames > 32 && !stateful_main {
            assert!(actual[7..].iter().any(|frame| frame[0] > 1.0));
        }
    }
}

#[test]
fn adding_ended_audio_track_preserves_existing_compensated_pcm() {
    use hex_project::{Command, Created};
    for (frames, partition) in [(16, 1), (16, 257), (25, 1), (25, 257)] {
        let mut f = fixture(&[&[7]], 512);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 512]).unwrap();
        }
        let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let Created::Imported { asset, .. } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "ended".into(),
                path: "ended.wav".into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("import")
        };
        f.media.insert(
            asset,
            Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap(),
        );
        let revision = prepare(&f).unwrap();
        let mut reference = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        reference.render(&mut [[0.0; 2]; 32]).unwrap();
        let mut next = SessionPlayback::new(revision).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 257];
        let mut expected = actual;
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "frames {frames} partition {partition}");
    }
}

#[test]
#[ignore = "explicit optimized handoff timing profile"]
fn ended_audio_track_handoff_profile() {
    profile_added_audio_track(16, false);
}

#[test]
#[ignore = "explicit optimized handoff timing profile"]
fn active_audio_track_handoff_profile() {
    profile_added_audio_track(256, false);
}

#[test]
#[ignore = "explicit optimized handoff timing profile"]
fn looped_active_audio_track_handoff_profile() {
    profile_added_audio_track(256, true);
}

fn profile_added_audio_track(frames: u64, looped: bool) {
    use hex_project::{Command, Created};
    use std::time::Instant;
    const WARMUP: usize = 20;
    const SAMPLES: usize = 100;
    let mut f = fixture(&[&[7]], 512);
    if looped {
        f.editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 64,
                enabled: true,
            })
            .unwrap();
    }
    let old_revision = prepare(&f).unwrap();
    let Created::Imported { asset, .. } = f
        .editor
        .execute(Command::ImportAudioTrack {
            name: "ended".into(),
            path: "ended.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("import")
    };
    f.media.insert(
        asset,
        Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap(),
    );
    let new_revision = prepare(&f).unwrap();
    let mut samples = Vec::with_capacity(SAMPLES);
    for iteration in 0..WARMUP + SAMPLES {
        let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        let mut next = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let start = Instant::now();
        next.try_inherit_state_from(&mut old).unwrap();
        let elapsed = start.elapsed().as_nanos();
        if iteration >= WARMUP {
            samples.push(elapsed);
        }
    }
    samples.sort_unstable();
    eprintln!(
        "audio_handoff rate=48000 tracks=1+1 delay=7 cut=32 clip_frames={frames} looped={looped} warmup={WARMUP} samples={SAMPLES} p50_ns={} p95_ns={}",
        samples[SAMPLES / 2],
        samples[SAMPLES * 95 / 100]
    );
}

#[test]
fn adding_future_audio_track_preserves_existing_compensated_pcm() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 2048);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 2048]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        let Created::Imported { track, asset, clip } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "future".into(),
                path: "future.wav".into(),
                sample_rate: 48_000,
                frames: 1024,
            })
            .unwrap()
        else {
            panic!("import")
        };
        f.editor
            .execute(Command::MoveClip {
                clip,
                track,
                start: 1024,
            })
            .unwrap();
        f.media
            .insert(asset, Audio::new(48_000, vec![[0.25, 0.25]; 1024]).unwrap());
        let updated = prepare(&f).unwrap();
        let mut future_oracle = SessionPlayback::new(Arc::clone(&updated)).unwrap();
        future_oracle.apply(SessionPlaybackCommand::Play).unwrap();
        future_oracle.render(&mut [[0.0; 2]; 32]).unwrap();
        let mut next = SessionPlayback::new(updated).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 1280];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            future_oracle.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
        let mut retained = [[0.0; 2]; 257];
        reference.render(&mut retained).unwrap();
        assert_eq!(&actual[..257], &retained);
        assert!(expected[1000..].iter().any(|frame| *frame != [1.0, -0.5]));
    }
}

#[test]
fn adding_future_audio_track_during_loop_keeps_compensated_pcm() {
    use hex_project::{Command, Created};
    for (elapsed, start) in [(32, 33), (32, 50), (96, 33), (96, 50), (60, 63), (124, 63)] {
        for partition in [1, 257] {
            let mut f = fixture(&[&[7]], 1024);
            for audio in f.media.values_mut() {
                *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
            }
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 64,
                    enabled: true,
                })
                .unwrap();
            let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
            let Created::Imported { track, asset, clip } = f
                .editor
                .execute(Command::ImportAudioTrack {
                    name: "future".into(),
                    path: "future-loop.wav".into(),
                    sample_rate: 48_000,
                    frames: 128,
                })
                .unwrap()
            else {
                panic!("import")
            };
            f.editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
            f.media
                .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 128]).unwrap());
            let revision = prepare(&f).unwrap();
            let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
            let mut next = SessionPlayback::new(revision).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let mut actual = [[0.0; 2]; 257];
            let mut expected = actual;
            for chunk in actual.chunks_mut(partition) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(partition) {
                oracle.render(chunk).unwrap();
            }
            assert_eq!(
                actual, expected,
                "elapsed={elapsed} start={start} partition={partition}"
            );
        }
    }
}

#[test]
fn adding_an_active_track_during_a_loop_preserves_audio_without_its_past() {
    use hex_project::{Command, Created};
    for (loop_end, elapsed, stateful_main) in [
        (64, 32, false),
        (64, 60, false),
        (64, 64, false),
        (64, 66, false),
        (64, 96, false),
        (64, 124, false),
        (4, 8, false),
        (4, 10, false),
        (4, 12, false),
        (64, 32, true),
        (64, 64, true),
        (4, 12, true),
    ] {
        for (start, frames) in [
            (0, 1024),
            ((elapsed % loop_end) as u64, 1024),
            (((elapsed + 1) % loop_end) as u64, 1024),
            (0, 16),
        ] {
            for partition in [1, 257] {
                let mut f = fixture(&[&[7]], 1024);
                if stateful_main {
                    f.editor
                        .execute(Command::InsertMainDevice {
                            before: None,
                            state: crate::signal_graph::tests::state(3).unwrap(),
                        })
                        .unwrap();
                }
                for audio in f.media.values_mut() {
                    let frames = (0..1024)
                        .map(|i| [(i % 64) as f32 / 128.0, -0.25])
                        .collect();
                    *audio = Audio::new(48_000, frames).unwrap();
                }
                f.editor
                    .execute(Command::SetLoopRegion {
                        start: 0,
                        end: loop_end as u64,
                        enabled: true,
                    })
                    .unwrap();
                let revision = prepare(&f).unwrap();
                let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
                let mut reference = SessionPlayback::new(revision).unwrap();
                for runtime in [&mut old, &mut reference] {
                    runtime.apply(SessionPlaybackCommand::Play).unwrap();
                    runtime.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
                }
                let Created::Imported { track, asset, clip } = f
                    .editor
                    .execute(Command::ImportAudioTrack {
                        name: "added".into(),
                        path: "added.wav".into(),
                        sample_rate: 48_000,
                        frames,
                    })
                    .unwrap()
                else {
                    panic!("import")
                };
                if start != 0 {
                    f.editor
                        .execute(Command::MoveClip { clip, track, start })
                        .unwrap();
                }
                f.media.insert(
                    asset,
                    Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap(),
                );
                let revision = prepare(&f).unwrap();
                let mut full_reference = SessionPlayback::new(Arc::clone(&revision)).unwrap();
                full_reference.apply(SessionPlaybackCommand::Play).unwrap();
                full_reference.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
                let mut next = SessionPlayback::new(revision).unwrap();
                next.try_inherit_state_from(&mut old).unwrap();
                let horizon = if stateful_main { 10 } else { 7 };
                for delay in 0..=horizon {
                    assert_eq!(
                        next.compensation
                            .as_ref()
                            .unwrap()
                            .delays
                            .stamp_at_delay(delay),
                        reference
                            .compensation
                            .as_ref()
                            .unwrap()
                            .delays
                            .stamp_at_delay(delay),
                        "elapsed {elapsed} initial stamp {delay}"
                    );
                }
                let mut actual = [[0.0; 2]; 257];
                let mut expected = actual;
                let mut complete = actual;
                for ((a, b), c) in actual
                    .chunks_mut(partition)
                    .zip(expected.chunks_mut(partition))
                    .zip(complete.chunks_mut(partition))
                {
                    next.render(a).unwrap();
                    reference.render(b).unwrap();
                    full_reference.render(c).unwrap();
                    assert_eq!(
                        next.compensation.as_ref().unwrap().delays.stamp_at_delay(0),
                        reference
                            .compensation
                            .as_ref()
                            .unwrap()
                            .delays
                            .stamp_at_delay(0)
                    );
                }
                for (i, ((a, b), c)) in actual.iter().zip(expected).zip(complete).enumerate() {
                    assert_eq!(
                        *a,
                        if i < horizon as usize { b } else { c },
                        "loop {loop_end}, elapsed {elapsed}, start {start}, frames {frames}, main {stateful_main}, partition {partition}, frame {i}"
                    );
                }
            }
        }
    }
}

#[test]
fn newly_added_clip_near_cut_keeps_compensated_onset_in_time() {
    use hex_project::{Command, Created};
    for start in [33, 34, 39] {
        for partition in [1, 257] {
            let mut f = fixture(&[&[7]], 512);
            for audio in f.media.values_mut() {
                *audio = Audio::new(48_000, vec![[0.5, -0.25]; 512]).unwrap();
            }
            let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut [[0.0; 2]; 32]).unwrap();
            let Created::Imported { track, asset, clip } = f
                .editor
                .execute(Command::ImportAudioTrack {
                    name: "new".into(),
                    path: "near.wav".into(),
                    sample_rate: 48_000,
                    frames: 128,
                })
                .unwrap()
            else {
                panic!("import")
            };
            f.editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
            f.media
                .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 128]).unwrap());
            let revision = prepare(&f).unwrap();
            let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.render(&mut [[0.0; 2]; 32]).unwrap();
            let mut next = SessionPlayback::new(revision).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let mut actual = [[0.0; 2]; 128];
            let mut expected = actual;
            for chunk in actual.chunks_mut(partition) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(partition) {
                oracle.render(chunk).unwrap();
            }
            assert_eq!(actual, expected, "start={start} partition={partition}");
        }
    }
}

#[test]
fn future_audio_handoff_keeps_unsafe_cases_on_restart_path() {
    use hex_project::{Command, Created};
    for case in [
        "underflow",
        "recent_seek",
        "loop_seek",
        "loop_change",
        "routed",
        "latency",
    ] {
        let mut f = fixture(&[&[7]], 2048);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 2048]).unwrap();
        }
        let return_track = if case == "routed" {
            let Created::ReturnTrack(id) = f
                .editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            Some(id)
        } else {
            None
        };
        if matches!(case, "loop_seek" | "loop_change") {
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 64,
                    enabled: true,
                })
                .unwrap();
        }
        let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        let elapsed = match case {
            "underflow" => 4,
            _ => 32,
        };
        old.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
        if matches!(case, "recent_seek" | "loop_seek") {
            old.apply(SessionPlaybackCommand::Seek(32)).unwrap();
            old.render(&mut [[0.0; 2]; 1]).unwrap();
        }
        if case == "loop_change" {
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 72,
                    enabled: true,
                })
                .unwrap();
        }
        let frames = match case {
            "recent_seek" | "loop_seek" => 16,
            "underflow" => 1,
            _ => 1024,
        };
        let Created::Imported { track, asset, clip } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "new".into(),
                path: "future.wav".into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let start = match case {
            "underflow" | "recent_seek" | "loop_seek" => 0,
            "loop_change" => 50,
            _ => 1024,
        };
        if start != 0 {
            f.editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
        }
        if let Some(return_track) = return_track {
            f.editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track,
                    gain: 0.5,
                })
                .unwrap();
        }
        f.media.insert(
            asset,
            Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap(),
        );
        if case == "latency" {
            let device = f.editor.project().track(f.tracks[0]).unwrap().devices()[0].id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: crate::signal_graph::tests::state(8).unwrap(),
                })
                .unwrap();
        }
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 7];
        next.render(&mut actual).unwrap();
        assert_eq!(actual, [[0.0; 2]; 7], "unsafe case {case}");
    }
}

#[test]
fn adding_an_unused_return_keeps_compensated_pcm_continuous() {
    use hex_project::Command;
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_an_unused_return_preserves_a_sounding_return() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "active".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[0],
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
        assert!(expected.iter().any(|frame| frame[0] > 1.0));
    }
}

#[test]
fn changing_an_unused_return_preserves_an_existing_reverb_tail() {
    for remove in [false, true] {
        unused_return_reverb_tail_case(remove);
    }
}

fn unused_return_reverb_tail_case(remove: bool) {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "impulse".into(),
            path: "memory.wav".into(),
            sample_rate: 48_000,
            frames: 12_000,
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: crate::default_native_device_state(crate::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "room".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    let reverb = &hex_devices::reverb::REVERB_DEFINITION;
    let state = reverb
        .patch_parameter(
            &reverb.default_state().unwrap(),
            hex_devices::reverb::parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state,
        })
        .unwrap();
    let unused = if remove {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        Some(id)
    } else {
        None
    };
    let mut samples = vec![[0.0; 2]; 12_000];
    samples[..512].fill([0.25, -0.125]);
    let media = std::collections::BTreeMap::from([(asset, Audio::new(48_000, samples).unwrap())]);
    let table = Arc::new(SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(PreparedRevision::prepare(project, &media, Arc::clone(&table), None).unwrap())
    };
    let revision = prepare(editor.project());
    let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut reference = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut old, &mut reference] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut vec![[0.0; 2]; 6000]).unwrap();
    }
    if let Some(id) = unused {
        editor.execute(Command::RemoveReturnTrack(id)).unwrap();
    } else {
        editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap();
    }
    let mut next = SessionPlayback::new(prepare(editor.project())).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 512];
    let mut expected = actual;
    next.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(
        expected.iter().flatten().any(|sample| sample.abs() > 1e-7),
        "reverb tail must be audible"
    );
}

#[test]
fn adding_a_routed_return_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    next.render(&mut actual).unwrap();
    assert_eq!(&actual[..7], &[[0.0; 2]; 7]);
    assert!(actual[7][0] > 0.0);
}

#[test]
fn removing_an_unused_return_keeps_compensated_pcm_continuous() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn removing_and_undoing_an_earlier_unused_return_preserves_later_send() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 2048);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 2048]).unwrap();
        }
        let Created::ReturnTrack(unused) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "unused".into(),
            })
            .unwrap()
        else {
            panic!("unused return")
        };
        let Created::ReturnTrack(active) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: "active".into(),
            })
            .unwrap()
        else {
            panic!("active return")
        };
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[0],
                return_track: active,
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor
            .execute(Command::RemoveReturnTrack(unused))
            .unwrap();
        for restored in [false, true] {
            if restored {
                assert!(f.editor.undo());
                assert_eq!(f.editor.project().return_tracks()[0].id(), unused);
            }
            let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let mut actual = vec![[0.0; 2]; 257];
            let mut expected = actual.clone();
            for chunk in actual.chunks_mut(partition) {
                next.render(chunk).unwrap();
            }
            for chunk in expected.chunks_mut(partition) {
                reference.render(chunk).unwrap();
            }
            assert_eq!(
                actual, expected,
                "partition {partition}, restored={restored}"
            );
            assert!(expected.iter().any(|frame| frame[0] > 1.0));
            old = next;
        }
    }
}

#[test]
fn removing_an_unused_return_does_not_hide_a_changed_send_destination() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut returns = Vec::new();
    for name in ["unused", "first", "second"] {
        let Created::ReturnTrack(id) = f
            .editor
            .execute(Command::AddReturnTrack { name: name.into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
    }
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[1],
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::RemoveReturnTrack(returns[0]))
        .unwrap();
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[1],
            gain: 0.0,
        })
        .unwrap();
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: returns[2],
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    next.render(&mut actual).unwrap();
    assert_eq!(&actual[..7], &[[0.0; 2]; 7]);
    assert!(actual[7][0] > 0.0);
}

#[test]
fn removing_a_routed_return_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    next.render(&mut actual).unwrap();
    assert_eq!(&actual[..7], &[[0.0; 2]; 7]);
    assert_eq!(actual[7], [1.0, -0.5]);
}

#[test]
fn removing_a_sounding_audio_track_keeps_other_compensated_pcm_continuous() {
    use hex_project::{Command, Created};
    for (partition, stateful_main) in [(1, false), (257, false), (1, true), (257, true)] {
        let mut f = fixture(&[&[7]], 1024);
        if stateful_main {
            f.editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: crate::signal_graph::tests::state(3).unwrap(),
                })
                .unwrap();
        }
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::Imported { track, asset, .. } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "removed".into(),
                path: "removed.wav".into(),
                sample_rate: 48_000,
                frames: 1024,
            })
            .unwrap()
        else {
            panic!("import")
        };
        f.media
            .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 1024]).unwrap());
        let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        // The source is cut seven frames before the handoff so its isolated
        // compensation ring is silent at the cut, while Main retains its past.
        let mut masked = vec![[0.25; 2]; 1024];
        masked[25..].fill([0.0; 2]);
        f.media.insert(asset, Audio::new(48_000, masked).unwrap());
        let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        reference.render(&mut [[0.0; 2]; 32]).unwrap();
        f.editor.execute(Command::RemoveTrack(track)).unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 257];
        let mut expected = actual;
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(
            actual, expected,
            "partition {partition}, main {stateful_main}"
        );
    }
}

#[test]
fn removing_a_sounding_track_during_a_loop_preserves_other_pcm() {
    use hex_project::{Command, Created};
    for (loop_end, elapsed) in [
        (64, 32),
        (64, 60),
        (64, 64),
        (64, 66),
        (64, 96),
        (64, 124),
        (4, 8),
        (4, 10),
        (4, 12),
        (4, 18),
    ] {
        for partition in [1, 257] {
            let mut f = fixture(&[&[7]], 1024);
            for audio in f.media.values_mut() {
                let frames = (0..1024)
                    .map(|i| [(i % 64) as f32 / 128.0, -0.25])
                    .collect();
                *audio = Audio::new(48_000, frames).unwrap();
            }
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: loop_end,
                    enabled: true,
                })
                .unwrap();
            let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            reference.apply(SessionPlaybackCommand::Play).unwrap();
            reference.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
            let Created::Imported { track, asset, .. } = f
                .editor
                .execute(Command::ImportAudioTrack {
                    name: "removed".into(),
                    path: "removed.wav".into(),
                    sample_rate: 48_000,
                    frames: 1024,
                })
                .unwrap()
            else {
                panic!("import")
            };
            f.media
                .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 1024]).unwrap());
            let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            old.apply(SessionPlaybackCommand::Play).unwrap();
            old.render(&mut vec![[0.0; 2]; elapsed]).unwrap();
            f.editor.execute(Command::RemoveTrack(track)).unwrap();
            let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            for delay in 0..=7 {
                assert_eq!(
                    next.compensation
                        .as_ref()
                        .unwrap()
                        .delays
                        .stamp_at_delay(delay),
                    reference
                        .compensation
                        .as_ref()
                        .unwrap()
                        .delays
                        .stamp_at_delay(delay),
                    "elapsed {elapsed}, initial stamp delay {delay}"
                );
            }
            let mut actual = [[0.0; 2]; 257];
            let mut expected = actual;
            for (actual, expected) in actual
                .chunks_mut(partition)
                .zip(expected.chunks_mut(partition))
            {
                next.render(actual).unwrap();
                reference.render(expected).unwrap();
                assert_eq!(
                    next.compensation.as_ref().unwrap().delays.stamp_at_delay(0),
                    reference
                        .compensation
                        .as_ref()
                        .unwrap()
                        .delays
                        .stamp_at_delay(0),
                    "elapsed {elapsed}, partition {partition}"
                );
            }
            for (i, (got, want)) in actual.iter().zip(expected).enumerate() {
                assert_eq!(
                    *got, want,
                    "elapsed {elapsed}, partition {partition}, frame {i}"
                );
            }
        }
    }
}

#[test]
fn looped_track_removal_retains_main_device_history() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 512);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 512]).unwrap();
    }
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 64,
            enabled: true,
        })
        .unwrap();
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: crate::signal_graph::tests::state(3).unwrap(),
        })
        .unwrap();
    let Created::Imported { track, asset, .. } = f
        .editor
        .execute(Command::ImportAudioTrack {
            name: "removed".into(),
            path: "removed.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("import")
    };
    f.media
        .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 512]).unwrap());
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    let mut masked = vec![[0.25; 2]; 512];
    masked[25..].fill([0.0; 2]);
    f.media.insert(asset, Audio::new(48_000, masked).unwrap());
    let mut reference = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    reference.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor.execute(Command::RemoveTrack(track)).unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut actual = [[0.0; 2]; 16];
    let mut expected = actual;
    next.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert!(actual[..3].iter().any(|frame| frame[0] > 1.0));
}

#[test]
fn sounding_track_removal_keeps_unproven_cases_on_restart_path() {
    use hex_project::{Command, Created};
    for case in [
        "loop_change",
        "loop_seek",
        "routed",
        "latency",
        "recent_seek",
    ] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::Imported { track, asset, .. } = f
            .editor
            .execute(Command::ImportAudioTrack {
                name: "removed".into(),
                path: "removed.wav".into(),
                sample_rate: 48_000,
                frames: 1024,
            })
            .unwrap()
        else {
            panic!("import")
        };
        f.media
            .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 1024]).unwrap());
        if matches!(case, "loop_change" | "loop_seek") {
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 64,
                    enabled: true,
                })
                .unwrap();
        }
        if case == "routed" {
            let Created::ReturnTrack(bus) = f
                .editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            f.editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 0.5,
                })
                .unwrap();
        }
        let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        if matches!(case, "recent_seek" | "loop_seek") {
            old.apply(SessionPlaybackCommand::Seek(32)).unwrap();
            old.render(&mut [[0.0; 2]; 1]).unwrap();
        }
        if case == "loop_change" {
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 72,
                    enabled: true,
                })
                .unwrap();
        }
        f.editor.execute(Command::RemoveTrack(track)).unwrap();
        if case == "latency" {
            let device = f.editor.project().track(f.tracks[0]).unwrap().devices()[0].id();
            f.editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: crate::signal_graph::tests::state(8).unwrap(),
                })
                .unwrap();
        }
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = [[0.0; 2]; 7];
        next.render(&mut actual).unwrap();
        assert_eq!(actual, [[0.0; 2]; 7], "case {case}");
    }
}

#[test]
#[ignore = "explicit optimized handoff timing profile"]
fn sounding_audio_track_removal_handoff_profile() {
    profile_sounding_audio_track_removal(false);
}

#[test]
#[ignore = "explicit optimized handoff timing profile"]
fn looped_sounding_audio_track_removal_handoff_profile() {
    profile_sounding_audio_track_removal(true);
}

fn profile_sounding_audio_track_removal(looped: bool) {
    use hex_project::{Command, Created};
    use std::time::Instant;
    const WARMUP: usize = 20;
    const SAMPLES: usize = 100;
    let mut f = fixture(&[&[7]], 512);
    if looped {
        f.editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 64,
                enabled: true,
            })
            .unwrap();
    }
    let Created::Imported { track, asset, .. } = f
        .editor
        .execute(Command::ImportAudioTrack {
            name: "removed".into(),
            path: "removed.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("import")
    };
    f.media
        .insert(asset, Audio::new(48_000, vec![[0.25; 2]; 512]).unwrap());
    let old_revision = prepare(&f).unwrap();
    f.editor.execute(Command::RemoveTrack(track)).unwrap();
    let new_revision = prepare(&f).unwrap();
    let mut samples = Vec::with_capacity(SAMPLES);
    for iteration in 0..WARMUP + SAMPLES {
        let mut old = SessionPlayback::new(Arc::clone(&old_revision)).unwrap();
        let mut next = SessionPlayback::new(Arc::clone(&new_revision)).unwrap();
        old.apply(SessionPlaybackCommand::Play).unwrap();
        old.render(&mut [[0.0; 2]; 32]).unwrap();
        let start = Instant::now();
        next.try_inherit_state_from(&mut old).unwrap();
        let elapsed = start.elapsed().as_nanos();
        if iteration >= WARMUP {
            samples.push(elapsed);
        }
    }
    samples.sort_unstable();
    eprintln!(
        "removed_audio_handoff rate=48000 tracks=2-1 delay=7 cut=32 looped={looped} warmup={WARMUP} samples={SAMPLES} p50_ns={} p95_ns={}",
        samples[SAMPLES / 2],
        samples[SAMPLES * 95 / 100]
    );
}

#[test]
fn removing_an_empty_track_keeps_compensated_pcm_continuous() {
    use hex_project::{Command, Created};
    for partition in [1, 257] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let Created::Track(empty) = f
            .editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("track creation");
        };
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        f.editor.execute(Command::RemoveTrack(empty)).unwrap();
        let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        next.try_inherit_state_from(&mut old).unwrap();
        let mut actual = vec![[0.0; 2]; 257];
        let mut expected = actual.clone();
        for chunk in actual.chunks_mut(partition) {
            next.render(chunk).unwrap();
        }
        for chunk in expected.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert_eq!(actual, expected, "partition {partition}");
    }
}

#[test]
fn adding_and_removing_a_silent_routed_track_keeps_compensated_pcm_continuous() {
    use hex_project::{Command, Created, TrackDestination, TrackSelectionIds};
    for (partition, grouped) in [(1, false), (257, false), (1, true), (257, true)] {
        let mut f = fixture(&[&[7], &[]], 1024);
        for audio in f.media.values_mut() {
            let frames = (0..1024)
                .map(|frame| [frame as f32 / 1024.0, -0.25])
                .collect();
            *audio = Audio::new(48_000, frames).unwrap();
        }
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return");
        };
        f.editor
            .execute(Command::SetReturnSend {
                track: f.tracks[1],
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        let group = grouped.then(|| {
            // Inserting into the first group shifts the index of the second
            // group while the existing child-to-parent route stays identical.
            let mut groups = f.tracks.iter().map(|&track| {
                let Created::Track(group) = f
                    .editor
                    .execute(Command::GroupTracks {
                        tracks: TrackSelectionIds::new(&[track]).unwrap(),
                        name: "group".into(),
                    })
                    .unwrap()
                else {
                    panic!("group");
                };
                group
            });
            let first = groups.next().unwrap();
            groups.next().unwrap();
            first
        });
        let revision = prepare(&f).unwrap();
        let mut old = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut old, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            runtime.render(&mut [[0.0; 2]; 32]).unwrap();
        }
        let Created::Track(empty) = f
            .editor
            .execute(Command::AddTrack {
                name: "empty".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let route = if let Some(group) = group {
            Command::ReparentTracks {
                tracks: TrackSelectionIds::new(&[empty]).unwrap(),
                destination: TrackDestination {
                    parent_group: Some(group),
                    before: None,
                },
            }
        } else {
            Command::SetReturnSend {
                track: empty,
                return_track: bus,
                gain: 0.75,
            }
        };
        f.editor.execute(route).unwrap();
        for removed in [false, true] {
            if removed {
                f.editor.execute(Command::RemoveTrack(empty)).unwrap();
            }
            let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let mut actual = [[0.0; 2]; 257];
            let mut expected = actual;
            for (actual, expected) in actual
                .chunks_mut(partition)
                .zip(expected.chunks_mut(partition))
            {
                next.render(actual).unwrap();
                reference.render(expected).unwrap();
            }
            for (frame, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
                assert_eq!(
                    actual, expected,
                    "frame {frame}, partition {partition}, grouped {grouped}, removed {removed}"
                );
            }
            old = next;
        }
    }
}

#[test]
fn changing_an_empty_track_preserves_existing_audio_feed_history() {
    use hex_project::{Command, Created, RoutingOwner};
    for (partition, from_return) in [(1, false), (257, false), (1, true), (257, true)] {
        let mut f = fixture(&[&[7]], 1024);
        for audio in f.media.values_mut() {
            *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
        }
        let [destination, alternate] = ["destination", "alternate"].map(|name| {
            let Created::Track(track) = f
                .editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track");
            };
            track
        });
        f.editor
            .execute(Command::Gain {
                track: alternate,
                gain: 0.25,
            })
            .unwrap();
        let (source, sounding) = if from_return {
            let Created::ReturnTrack(bus) = f
                .editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return");
            };
            f.editor
                .execute(Command::SetReturnSend {
                    track: f.tracks[0],
                    return_track: bus,
                    gain: 0.5,
                })
                .unwrap();
            (RoutingOwner::Return(bus), [1.75, -0.875])
        } else {
            (RoutingOwner::Track(f.tracks[0]), [1.5, -0.75])
        };
        f.editor
            .execute(Command::SetAudioFeed {
                source,
                destination,
                gain: 0.5,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut current = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut reference = SessionPlayback::new(revision).unwrap();
        for runtime in [&mut current, &mut reference] {
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
            let mut warm = [[0.0; 2]; 32];
            runtime.render(&mut warm).unwrap();
            assert_eq!(warm[31], sounding);
        }
        let insert = || Command::InsertTrack {
            name: "empty".into(),
            before_track: Some(destination),
        };
        let Created::Track(empty) = f.editor.execute(insert()).unwrap() else {
            panic!("empty track");
        };
        for removed in [false, true] {
            if removed {
                f.editor.execute(Command::RemoveTrack(empty)).unwrap();
            }
            let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            next.try_inherit_state_from(&mut current).unwrap();
            let mut actual = [[0.0; 2]; 257];
            let mut expected = actual;
            for (actual, expected) in actual
                .chunks_mut(partition)
                .zip(expected.chunks_mut(partition))
            {
                next.render(actual).unwrap();
                reference.render(expected).unwrap();
                assert_eq!(
                    actual, expected,
                    "partition {partition}, source {source:?}, removed {removed}"
                );
            }
            current = next;
        }

        f.editor.execute(insert()).unwrap();
        for (destination, gain) in [(destination, 0.0), (alternate, 0.5)] {
            f.editor
                .execute(Command::SetAudioFeed {
                    source,
                    destination,
                    gain,
                })
                .unwrap();
        }
        let mut rerouted = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
        rerouted.try_inherit_state_from(&mut current).unwrap();
        let mut output = [[0.0; 2]; 16];
        for chunk in output.chunks_mut(partition) {
            rerouted.render(chunk).unwrap();
        }
        assert_eq!(&output[..7], &[[0.0; 2]; 7]);
        assert_eq!(
            output[7],
            if from_return {
                [1.5625, -0.78125]
            } else {
                [1.125, -0.5625]
            }
        );
    }
}

#[test]
fn adding_an_empty_track_with_an_incoming_feed_keeps_the_conservative_restart() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return");
    };
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    let Created::Track(track) = f
        .editor
        .execute(Command::AddTrack {
            name: "routed".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    f.editor
        .execute(Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Return(bus),
            destination: track,
            gain: 0.5,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut output = [[0.0; 2]; 16];
    next.render(&mut output).unwrap();
    assert_eq!(&output[..7], &[[0.0; 2]; 7]);
    assert_eq!(output[7], [1.0, -0.5]);
}
#[test]
fn compensated_fader_ramps_at_the_delayed_content_origin() {
    let mut f = fixture(&[&[7]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(hex_project::Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut frames = [[0.0; 2]; 256];
    next.render(&mut frames).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
        assert!(
            (frame[1] + 0.5 * gain).abs() < 1e-5,
            "frame {index}: {frame:?}"
        );
    }
}

#[test]
fn noncompensated_device_fader_ramps_without_applying_gain_twice() {
    let mut f = fixture(&[&[0]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    assert!(old.compensation.is_none());
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(hex_project::Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut frames = [[0.0; 2]; 256];
    next.render(&mut frames).unwrap();
    for (index, frame) in frames.iter().enumerate() {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
        assert!(
            (frame[1] + 0.5 * gain).abs() < 1e-5,
            "frame {index}: {frame:?}"
        );
    }
}

#[test]
fn compensated_fader_waits_for_origin_and_rolls_back_on_failure() {
    let f = fixture(&[&[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.track_faders[0].retarget([0.0; 2], 48000);
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let initial = tested.track_faders[0];
    tested.render(&mut [[0.0; 2]; 6]).unwrap();
    oracle.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(tested.track_faders[0], initial);
    tested.arrangement_frame = u64::MAX - 300;
    assert!(tested.render(&mut [[0.0; 2]; 512]).is_err());
    assert_eq!(tested.track_faders[0], initial);
    tested.arrangement_frame = 6;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(tested.track_faders[0], oracle.track_faders[0]);
}

#[test]
fn track_fader_ramp_feeds_capture_and_send_once() {
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0, -0.5]; 1024]).unwrap();
    }
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut next = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let plan = next
        .output_capture_plan(
            &[
                OutputCaptureId::Track(f.tracks[0]),
                OutputCaptureId::Return(bus),
            ],
            256,
        )
        .unwrap();
    let mut main = [[0.0; 2]; 256];
    let mut track = main;
    let mut returned = main;
    let mut buffers = [&mut track[..], &mut returned[..]];
    let mut capture = OutputCapture::new(&plan, &mut buffers);
    next.render_with_capture(&mut main, &mut capture).unwrap();
    let track = capture.output(0).unwrap();
    let returned = capture.output(1).unwrap();
    for index in 0..256 {
        let gain = 1.0 - (index + 1).min(240) as f32 / 240.0;
        assert!((track[index][0] - gain).abs() < 1e-5);
        assert!((returned[index][0] - 0.5 * gain).abs() < 1e-5);
        assert!((main[index][0] - 1.5 * gain).abs() < 1e-5);
    }
}

#[test]
fn rapid_gain_retarget_and_equivalent_publication_continue_from_current_level() {
    use hex_project::Command;
    let mut f = fixture(&[&[]], 1024);
    for audio in f.media.values_mut() {
        *audio = Audio::new(48000, vec![[1.0; 2]; 1024]).unwrap();
    }
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    old.render(&mut [[0.0; 2]; 32]).unwrap();
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.0,
        })
        .unwrap();
    let mut down = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    down.try_inherit_state_from(&mut old).unwrap();
    let mut down_frames = [[0.0; 2]; 64];
    down.render(&mut down_frames).unwrap();
    let current = 1.0 - 64.0 / 240.0;
    assert!((down_frames[63][0] - current).abs() < 1e-5);
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 1.0,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut up = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    up.try_inherit_state_from(&mut down).unwrap();
    let mut first = [[0.0; 2]; 32];
    up.render(&mut first).unwrap();
    let mut republished = SessionPlayback::new(revision).unwrap();
    republished.try_inherit_state_from(&mut up).unwrap();
    let mut rest = [[0.0; 2]; 208];
    republished.render(&mut rest).unwrap();
    for (index, frame) in first.iter().chain(rest.iter()).enumerate() {
        let gain = current + (1.0 - current) * (index + 1) as f32 / 240.0;
        assert!((frame[0] - gain).abs() < 1e-5, "frame {index}: {frame:?}");
    }
}
#[test]
fn compensation_late_failure_rolls_back_all_rings_and_node_histories() {
    let f = fixture(&[&[], &[7]], 1024);
    let revision = prepare(&f).unwrap();
    let mut tested = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut tested, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 4]).unwrap();
    }
    tested.arrangement_frame = u64::MAX - 300;
    let mut observed = [0; 2];
    let mut output = [[9.0; 2]; 512];
    assert!(
        tested
            .render_with_node_signal_observer(
                &mut output,
                Some(&mut |_, phase, _, _: &[Frame]| {
                    let index = match phase {
                        crate::NodeSignalPhase::Input => 0,
                        crate::NodeSignalPhase::Output => 1,
                    };
                    observed[index] += 1;
                })
            )
            .is_err()
    );
    assert!(
        observed.iter().all(|count| *count > 0),
        "fault must occur after speculative processing"
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(tested.performance_frame, 4);
    assert_eq!(tested.arrangement_frame, u64::MAX - 300);
    tested.arrangement_frame = 4;
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    tested.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual[3], [0.5, -1.0]);
}
#[test]
fn recording_clock_waits_for_real_fader_origin_and_preserves_content_loop_pass() {
    use hex_project::Command;
    let mut f = fixture(&[&[7]], 8);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let revision = Arc::try_unwrap(revision)
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    let id = f.tracks[0].value();
    assert!(!runtime.gain_recording_ready(id));
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    runtime.render(&mut [[0.0; 2]; 6]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
    runtime.render(&mut [[0.0; 2]; 1]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((0, 0, 0)));
    runtime.render(&mut [[0.0; 2]; 3]).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), Some((3, 0, 0)));
    runtime.apply(SessionPlaybackCommand::Stop).unwrap();
    assert_eq!(runtime.gain_recording_clock(id), None);
}
#[test]
fn dry_fader_clock_reports_downstream_main_delivery_delay() {
    let f = fixture(&[&[], &[7]], 8);
    let revision = Arc::try_unwrap(prepare(&f).unwrap())
        .unwrap()
        .arm_gain_recording(f.editor.project(), f.tracks[0])
        .unwrap();
    let runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
    assert_eq!(
        runtime.gain_recording_clock(f.tracks[0].value()),
        Some((0, 7, 0))
    );
}

/// Pulse #3772: a latency device on an unrelated track no longer disables
/// computer-keyboard monitoring. The keyboard plays its instrument through the
/// compensated graph, delayed only by the project's PDC alignment.
#[test]
fn computer_keyboard_plays_through_positive_latency_on_another_track() {
    let keyboard_output = |delays: &[&[u32]], playing: bool| {
        let mut f = fixture(delays, 1024);
        f.editor
            .execute(hex_project::Command::InsertInstrument {
                track: f.tracks[1],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        // Silence the clip track so only the keyboard track is compared.
        f.editor
            .execute(hex_project::Command::Gain {
                track: f.tracks[0],
                gain: 0.0,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let latency = revision.arrangement().main_latency_frames() as usize;
        let mut playback = crate::playback::Playback::from_prepared(revision);
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        playback
            .prepare_computer_keyboard(f.editor.project(), f.tracks[1], input)
            .unwrap();
        if playing {
            playback.apply(crate::playback::PlaybackCommand::Play);
        }
        let mut output = vec![[0.0; 2]; 512];
        playback.render(&mut output).unwrap();
        (latency, output)
    };
    for playing in [false, true] {
        let (latency, delayed) = keyboard_output(&[&[7], &[]], playing);
        let (none, direct) = keyboard_output(&[&[], &[]], playing);
        assert_eq!((latency, none), (7, 0));
        assert!(direct.iter().flatten().any(|sample| *sample != 0.0));
        assert!(
            delayed[..latency]
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0)
        );
        for (index, (actual, expected)) in delayed[latency..].iter().zip(&direct).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-6,
                    "playing={playing} frame {index}: {actual:?} != {expected:?}"
                );
            }
        }
    }
}

#[test]
fn paused_source_monitor_preserves_buffered_arrangement_audio() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    let mut mismatches = Vec::new();
    for (effect_delay, keyboard, partition) in [
        (false, false, 1),
        (false, false, 257),
        (false, true, 1),
        (false, true, 257),
        (true, false, 1),
        (true, false, 257),
        (true, true, 1),
        (true, true, 257),
    ] {
        let mut f = fixture(&[&[7], &[]], 1024);
        let keyboard_track = f.tracks[usize::from(effect_delay)];
        f.editor
            .execute(Command::InsertInstrument {
                track: keyboard_track,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        // The muted keyboard track contributes no PCM. The other track's
        // impulse is queued either in compensation or a device delay.
        f.editor
            .execute(Command::Gain {
                track: keyboard_track,
                gain: 0.0,
            })
            .unwrap();
        let revision = prepare(&f).unwrap();
        let mut actual = Playback::from_prepared(Arc::clone(&revision));
        let mut oracle = Playback::from_prepared(revision);
        let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        if keyboard {
            actual
                .prepare_computer_keyboard(f.editor.project(), keyboard_track, Arc::clone(&keys))
                .unwrap();
        }
        for playback in [&mut actual, &mut oracle] {
            playback.apply(PlaybackCommand::Play);
            playback.render(&mut [[0.0; 2]; 4]).unwrap();
            playback.apply(PlaybackCommand::Pause);
        }
        if keyboard {
            keys.set_key(0, Some((60, 100, 1))).unwrap();
        }
        let mut monitored = [[0.0; 2]; 257];
        for block in monitored.chunks_mut(partition) {
            actual.render(block).unwrap();
        }
        if keyboard {
            keys.set_key(0, None).unwrap();
        }
        for playback in [&mut actual, &mut oracle] {
            playback.apply(PlaybackCommand::Play);
        }
        let mut got = [[0.0; 2]; 32];
        let mut expected = got;
        actual.render(&mut got).unwrap();
        oracle.render(&mut expected).unwrap();
        assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
        let leaked_while_paused = monitored.iter().any(|frame| *frame != [0.0; 2]);
        let resume_differs = got != expected;
        if leaked_while_paused || resume_differs {
            mismatches.push((
                effect_delay,
                keyboard,
                partition,
                leaked_while_paused,
                resume_differs,
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "mismatches: (device delay, keyboard active, paused partition, leaked, resume differs) {mismatches:?}"
    );
}

#[test]
fn paused_keyboard_preserves_same_track_device_history() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    let mut f = fixture(&[&[7]], 1024);
    f.editor
        .execute(Command::InsertInstrument {
            track: f.tracks[0],
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut monitored = Playback::from_prepared(Arc::clone(&revision));
    let mut oracle = Playback::from_prepared(revision);
    let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    monitored
        .prepare_computer_keyboard(f.editor.project(), f.tracks[0], keys)
        .unwrap();
    for playback in [&mut monitored, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
        playback.render(&mut [[0.0; 2]; 4]).unwrap();
        playback.apply(PlaybackCommand::Pause);
    }
    let mut paused = [[0.0; 2]; 257];
    monitored.render(&mut paused).unwrap();
    for playback in [&mut monitored, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
    }
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    monitored.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    let leaked_while_paused = paused.iter().any(|frame| *frame != [0.0; 2]);
    let resume_differs = actual != expected;
    assert!(
        !leaked_while_paused && !resume_differs,
        "queued clip audio leaked while paused: {leaked_while_paused}; resume differs: {resume_differs}"
    );
}

#[test]
fn paused_keyboard_sounds_without_consuming_same_track_clip_history() {
    use crate::playback::{Playback, PlaybackCommand};
    use hex_project::Command;

    let mut f = fixture(&[&[7]], 1024);
    f.editor
        .execute(Command::InsertInstrument {
            track: f.tracks[0],
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let with_clip = prepare(&f).unwrap();
    for audio in f.media.values_mut() {
        *audio = Audio::new(48_000, vec![[0.0; 2]; 1024]).unwrap();
    }
    let without_clip = prepare(&f).unwrap();
    let mut actual = Playback::from_prepared(Arc::clone(&with_clip));
    let mut live_only = Playback::from_prepared(without_clip);
    let mut oracle = Playback::from_prepared(with_clip);
    let keys = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
    for playback in [&mut actual, &mut live_only] {
        playback
            .prepare_computer_keyboard(f.editor.project(), f.tracks[0], Arc::clone(&keys))
            .unwrap();
    }
    for playback in [&mut actual, &mut live_only, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
        playback.render(&mut [[0.0; 2]; 4]).unwrap();
        playback.apply(PlaybackCommand::Pause);
    }
    keys.set_key(0, Some((60, 100, 1))).unwrap();
    let mut paused = [[0.0; 2]; 257];
    let mut live_paused = paused;
    actual.render(&mut paused).unwrap();
    live_only.render(&mut live_paused).unwrap();
    assert!(live_paused.iter().flatten().any(|sample| *sample != 0.0));
    let leaked = paused
        .iter()
        .zip(&live_paused)
        .any(|(a, b)| (a[0] - b[0]).abs() > 1e-6 || (a[1] - b[1]).abs() > 1e-6);
    for playback in [&mut actual, &mut live_only, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
    }
    let mut resumed = [[0.0; 2]; 32];
    let mut live_resumed = resumed;
    let mut expected_clip = resumed;
    actual.render(&mut resumed).unwrap();
    live_only.render(&mut live_resumed).unwrap();
    oracle.render(&mut expected_clip).unwrap();
    assert!(expected_clip.iter().any(|frame| *frame != [0.0; 2]));
    let missing =
        resumed
            .iter()
            .zip(&live_resumed)
            .zip(&expected_clip)
            .any(|((actual, live), expected)| {
                (actual[0] - live[0] - expected[0]).abs() > 1e-6
                    || (actual[1] - live[1] - expected[1]).abs() > 1e-6
            });
    assert!(
        !leaked && !missing,
        "clip leaked while paused: {leaked}; resume differs: {missing}"
    );
}

#[test]
fn operator_sync_companions_preserve_pcm_through_compensated_startup_and_partitions() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{BeatTime, Command, Created};
    let frames = 16_000;
    let configure = |delay: u32| {
        let mut f = fixture(&[&[delay]], frames);
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        let class = crate::NativeDeviceClass::Instrument;
        let mut state = crate::default_native_device_state_for_class(
            class,
            DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        for (id, value) in [
            (357, ParameterValue::Bool(false)),
            (360, ParameterValue::Float(0.5)),
            (362, ParameterValue::Choice(3)),
            (364, ParameterValue::Choice(0)),
        ] {
            state = crate::patch_native_device_parameter_for_class(
                class,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
        f.editor
            .execute(Command::RemoveInstrument { track: f.tracks[0] })
            .unwrap();
        f.editor
            .execute(Command::InsertInstrument {
                track: f.tracks[0],
                state,
            })
            .unwrap();
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track: f.tracks[0],
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(8).unwrap(),
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        prepare(&f).unwrap()
    };
    let dry = configure(0);
    let delayed = configure(7);
    assert_eq!(delayed.arrangement().main_latency_frames(), 7);
    let render = |revision, length, partition| {
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut pcm = vec![[0.0; 2]; length];
        for chunk in pcm.chunks_mut(partition) {
            runtime.render(chunk).unwrap();
        }
        pcm
    };
    let expected = render(dry, frames, 127);
    assert!(
        expected[512..1024]
            .iter()
            .any(|frame| frame[0].abs() > 1e-6)
    );
    for partition in [1, 257] {
        let actual = render(Arc::clone(&delayed), frames + 7, partition);
        assert_eq!(actual[..7], [[0.0; 2]; 7]);
        assert_eq!(actual[7..], expected);
    }
}

#[test]
fn main_removal_and_serial_replacement_keep_old_larger_stamp_horizon_and_pending_pcm() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    for routed in [false, true] {
        for replacement in [false, true] {
            let mut f = if routed {
                fixture(&[&[7]], 64)
            } else {
                fixture(&[&[]], 64)
            };
            if routed {
                let Created::ReturnTrack(bus) = f
                    .editor
                    .execute(Command::AddReturnTrack {
                        name: "delayed".into(),
                    })
                    .unwrap()
                else {
                    panic!("return")
                };
                f.editor
                    .execute(Command::InsertReturnDevice {
                        return_track: bus,
                        before: None,
                        state: state(5).unwrap(),
                    })
                    .unwrap();
                f.editor
                    .execute(Command::SetReturnSend {
                        track: f.tracks[0],
                        return_track: bus,
                        gain: 1.0,
                    })
                    .unwrap();
            }
            let Created::Device(main) = f
                .editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state: state(3).unwrap(),
                })
                .unwrap()
            else {
                panic!("main")
            };
            let original = prepare(&f).unwrap();
            let old_horizon = original.arrangement().main_latency_frames();
            let warm = old_horizon as usize - 1;
            let mut old = SessionPlayback::new(Arc::clone(&original)).unwrap();
            let mut oracle = SessionPlayback::new(original).unwrap();
            for runtime in [&mut old, &mut oracle] {
                runtime.apply(SessionPlaybackCommand::Play).unwrap();
                runtime.render(&mut vec![[0.0; 2]; warm]).unwrap();
            }
            f.editor.execute(Command::RemoveDevice(main)).unwrap();
            if replacement {
                for _ in 0..2 {
                    f.editor
                        .execute(Command::InsertMainDevice {
                            before: None,
                            state: state(1).unwrap(),
                        })
                        .unwrap();
                }
            }
            let revision = prepare(&f).unwrap();
            let graph = Arc::clone(revision.arrangement().signal_graph.as_ref().unwrap());
            assert!(revision.arrangement().main_latency_frames() < old_horizon);
            let mut next = SessionPlayback::new(revision).unwrap();
            next.try_inherit_state_from(&mut old).unwrap();
            let history = next.compensation.as_ref().unwrap();
            assert!(Arc::ptr_eq(&history.graph, &graph));
            assert!(
                history.delays.history_horizon() >= old_horizon,
                "old origins cannot shrink with current Main"
            );
            // A second coherent handoff while the detached Main is still
            // audible carries the same bank owner, without refeeding it.
            let mut continued = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
            continued.try_inherit_state_from(&mut next).unwrap();
            let mut next = continued;
            let mut expected = [[0.0; 2]; 4];
            let mut actual = expected;
            oracle.render(&mut expected).unwrap();
            assert!(expected.iter().flatten().any(|s| *s != 0.0));
            next.render(&mut actual).unwrap();
            assert_eq!(
                actual, expected,
                "old output once; routed={routed}, replacement={replacement}"
            );
        }
    }
}

#[test]
fn neutral_zero_graph_metadata_allocates_no_delay_runtime_and_keeps_first_sample() {
    let f = fixture(&[&[]], 8);
    let revision = prepare(&f).unwrap();
    let graph = revision.arrangement().signal_graph.as_ref().unwrap();
    assert!(!graph.needs_runtime());
    let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(runtime.compensation.is_none());
    let renderer = crate::Renderer::new(revision.arrangement());
    assert!(renderer.runtime.compensation.is_none());
    assert!(runtime.accounted_runtime_storage_bytes().unwrap() >= graph.table_bytes().unwrap());
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; 8];
    runtime.render(&mut actual).unwrap();
    assert_eq!(&actual[..], f.samples.as_slice());
}

#[test]
fn main_removal_retains_old_tail_and_applies_future_source_mix_once() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 64);
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::Balance {
            track: f.tracks[0],
            balance: 0.25,
        })
        .unwrap();
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let Created::Device(main) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    let original = prepare(&f).unwrap();
    let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
    let mut old_tail = SessionPlayback::new(original).unwrap();
    for runtime in [&mut donor, &mut old_tail] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 2]).unwrap();
    }
    f.editor.execute(Command::RemoveDevice(main)).unwrap();
    let current = prepare(&f).unwrap();
    assert!(
        !current
            .arrangement()
            .signal_graph
            .as_ref()
            .unwrap()
            .needs_runtime()
    );
    let mut ordinary = SessionPlayback::new(Arc::clone(&current)).unwrap();
    assert!(ordinary.compensation.is_none());
    ordinary.apply(SessionPlaybackCommand::Seek(2)).unwrap();
    ordinary.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 64];
    ordinary.render(&mut expected).unwrap();
    assert!(expected[61].iter().any(|sample| *sample != 0.0));
    let mut residual = [[0.0; 2]; 64];
    old_tail.render(&mut residual).unwrap();
    assert!(residual[1].iter().any(|sample| *sample != 0.0));
    assert!(residual[2..].iter().flatten().all(|sample| *sample == 0.0));
    for (frame, tail) in expected.iter_mut().zip(residual) {
        frame[0] += tail[0];
        frame[1] += tail[1];
    }
    let mut carried = SessionPlayback::new(current).unwrap();
    carried.try_inherit_state_from(&mut donor).unwrap();
    assert!(carried.compensation.is_some());
    let mut actual = [[0.0; 2]; 64];
    carried.render(&mut actual).unwrap();
    assert_eq!(
        actual, expected,
        "future source mix once plus independent old residual"
    );
}

#[path = "compensation_tests/monitor_clock.rs"]
mod monitor_clock;

#[path = "compensation_tests/cue.rs"]
mod cue;

#[test]
fn ordinary_stopped_compensation_preview_is_pure_and_retains_candidate_admission() {
    let mut f = fixture(&[&[], &[]], 128);
    let mut old = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    f.editor
        .execute(hex_project::Command::SetAudioFeed {
            source: hex_project::RoutingOwner::Track(f.tracks[0]),
            destination: f.tracks[1],
            gain: 0.5,
        })
        .unwrap();
    let revision = prepare(&f).unwrap();
    let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(candidate.compensation.is_some());
    let before = (
        old.capture_epoch,
        candidate.capture_epoch,
        old.performance_frame,
        candidate.performance_frame,
        candidate.candidate_admission_open,
    );
    let mut pages = Vec::new();
    let (window, report) = candidate
        .preview_stream_handoff_with_policy_and_report(
            &mut old,
            64,
            None,
            &mut |extent| {
                pages.push(extent);
                Ok(())
            },
            true,
            false,
        )
        .unwrap();
    assert!(!report.running_audio_restart());
    assert_eq!(window.frames, 0);
    assert!(pages.is_empty());
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    let running = candidate
        .preview_stream_handoff_with_scope(&mut old, 64, Some(true), &mut |_| Ok(()), true)
        .unwrap();
    assert_eq!(running.frames, 64);
    assert_eq!(
        (
            old.capture_epoch,
            candidate.capture_epoch,
            old.performance_frame,
            candidate.performance_frame,
            candidate.candidate_admission_open
        ),
        before
    );
    let committed = candidate.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        report.running_audio_restart(),
        committed.running_audio_restart()
    );
    let mut oracle = SessionPlayback::new(revision).unwrap();
    for runtime in [&mut candidate, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
    }
    let mut actual = [[0.0; 2]; 64];
    let mut expected = actual;
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn gain_recording_delivery_delay_includes_the_complete_serial_main_chain() {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    for (lookaheads, expected_delay) in [(&[2_u32][..], 288_u64), (&[0_u32, 2][..], 360_u64)] {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "serial main".into(),
                path: "memory/serial-main.wav".into(),
                source_sample_rate: 48000,
                source_frames: 4096,
                playback_frames: 4096,
                conversion_version: hex_project::IDENTITY_CONVERSION,
            })
            .unwrap()
        else {
            panic!("import")
        };
        for &lookahead in lookaheads {
            let definition = &hex_devices::limiter::DEFINITION;
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    hex_devices::limiter::parameter::LOOKAHEAD,
                    ParameterValue::Choice(lookahead),
                )
                .unwrap();
            editor
                .execute(Command::InsertMainDevice {
                    before: None,
                    state,
                })
                .unwrap();
        }
        let media = std::collections::BTreeMap::from([(
            asset,
            Audio::new(48000, vec![[0.01, -0.02]; 4096]).unwrap(),
        )]);
        let revision = PreparedRevision::prepare_for_gain_recording(
            editor.project(),
            &media,
            Arc::new(SineTable::new()),
            None,
            track,
        )
        .unwrap();
        assert_eq!(
            u64::from(revision.arrangement().main_latency_frames()),
            expected_delay
        );
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        assert_eq!(
            runtime.gain_recording_clock(track.value()),
            Some((0, expected_delay, 0)),
            "serial Main delay must be added after the pre-Main sum"
        );
        let mut initial = [[1.0; 2]; 16];
        runtime.render(&mut initial).unwrap();
        assert_eq!(initial, [[0.0; 2]; 16]);
        assert_eq!(
            runtime.gain_recording_clock(track.value()),
            Some((16, expected_delay, 0))
        );
    }
}

#[test]
fn compensated_mutable_source_applies_live_input_with_and_without_shared_contributors() {
    use hex_lattice::{NativeTransientOperation, NativeTransientValue};
    use hex_lattice_contract::{
        DeviceAutomationTarget, NativeDeviceMetadata, ParameterId, ParameterValue,
    };
    use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};
    use std::collections::BTreeMap;

    for shared in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "instrument".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_devices::fm4::device::FM4_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("instrument track")
        };
        let track = editor.project().track(receiver).unwrap();
        let device = track.instrument().unwrap().id();
        let source = track.source_id().unwrap();
        let note_track = if shared {
            let Created::Track(contributor) = editor
                .execute(Command::AddMidiTrack {
                    name: "contributor".into(),
                })
                .unwrap()
            else {
                panic!("contributor")
            };
            editor
                .execute(Command::BindTrackSource {
                    track: contributor,
                    source: Some(source),
                })
                .unwrap();
            contributor
        } else {
            receiver
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: note_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(2).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        editor
            .execute(Command::InsertDevice {
                track: receiver,
                before: None,
                state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
            })
            .unwrap();
        let parameter = ParameterId::from_static(29);
        let target = DeviceAutomationTarget::Parameter(parameter);
        let revision = PreparedRevision::prepare_for_native_parameter_input(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            device,
            target,
        )
        .unwrap();
        assert!(revision.arrangement().main_latency_frames() > 0);
        assert_eq!(revision.source_routing().has_shared_sources(), shared);
        let mut runtime = SessionPlayback::new(Arc::new(revision)).unwrap();
        assert!(
            runtime
                .compensation
                .as_ref()
                .unwrap()
                .graph
                .needs_upstream_frame_routing()
        );
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let operation = NativeTransientOperation::Set(NativeTransientValue::Parameter {
            id: parameter,
            value: ParameterValue::Float(0.5),
        });
        runtime.set_native_parameter_input(operation).unwrap();
        let mut heard = false;
        let mut applied = None;
        for _ in 0..8 {
            let mut output = [[0.0; 2]; 256];
            runtime.render(&mut output).unwrap();
            heard |= output.iter().flatten().any(|value| value.abs() > 1e-8);
            applied = applied.or_else(|| runtime.take_native_parameter_application());
        }
        assert!(
            heard,
            "mutable instrument must remain audible, shared={shared}"
        );
        assert_eq!(
            applied.expect("live input must be applied").operation,
            operation
        );
    }
}

#[path = "compensation_tests/paused_layout.rs"]
mod paused_layout;

#[test]
fn removing_isolated_return_refuses_active_detached_main_tail() {
    use crate::signal_graph::tests::state;
    use hex_project::{Command, Created};
    let mut f = fixture(&[&[]], 64);
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "unused".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(main) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    f.editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(3).unwrap(),
        })
        .unwrap();
    let original = prepare(&f).unwrap();
    let mut donor = SessionPlayback::new(Arc::clone(&original)).unwrap();
    let mut oracle = SessionPlayback::new(original).unwrap();
    for runtime in [&mut donor, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 5]).unwrap();
    }
    f.editor.execute(Command::RemoveDevice(main)).unwrap();
    let mut detached = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    detached.try_inherit_state_live_from(&mut donor).unwrap();
    assert!(
        detached
            .return_effect_tails
            .slots()
            .iter()
            .flatten()
            .any(|tail| tail.output_active && tail.return_id().is_none())
    );
    f.editor.execute(Command::RemoveReturnTrack(bus)).unwrap();
    let mut candidate = SessionPlayback::new(prepare(&f).unwrap()).unwrap();
    assert!(matches!(
        candidate.try_inherit_state_live_from(&mut detached),
        Err(SessionPlaybackError::LatencyLayoutChanged)
    ));
    let mut actual = [[0.0; 2]; 8];
    let mut expected = actual;
    detached.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}
