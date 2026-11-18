//! Built-in effect publication through a real verified-file source.
use super::*;
use hex_project::{Command, Created};
use hex_session::device_gesture::{
    GestureKey, GestureRequest, GestureTarget, ParameterId, ParameterValue,
};

fn run_insert(h: &mut Harness, command: Command) -> Event {
    let staged = h.run(Request::Edit(command));
    assert!(!staged.request_complete);
    let mut applied = h.next_with_audio();
    assert!(applied.error.is_none(), "{:?}", applied.error);
    if let Some(offer) = applied.device_insert_confirmation.take() {
        assert!(offer.proceed());
        applied = h.next_with_audio();
        assert!(applied.error.is_none(), "{:?}", applied.error);
    }
    let receipt = applied.device_insert_complete.as_ref().unwrap();
    assert_eq!(
        receipt.outcome,
        hex_session::service::DeviceInsertOutcome::Installed
    );
    applied
}

#[test]
fn verified_editor_insert_commits_only_the_confirmed_graph_and_one_undo() {
    use hex_session::service::DeviceInsertOutcome;
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(2);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 96_000)));
    let predecessor = h.applied(h.context.unwrap());
    let saved = files.0.join("verified-insert.hex");
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    wait_for_audio(&h);

    for proceed in [false, true] {
        let limiter = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        let staged = h.run(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: limiter,
        }));
        assert!(!staged.request_complete);
        assert_eq!(staged.state.project, imported.state.project);
        if !proceed {
            assert!(h.run(Request::SaveNew(saved.clone())).save_complete);
            assert!(hex_storage::load(&saved).unwrap().main_devices().is_empty());
        }
        let offered = h.next_with_audio();
        assert!(offered.error.is_none(), "{:?}", offered.error);
        assert_eq!(offered.state.revision, imported.state.revision);
        let offer = offered.device_insert_confirmation.unwrap();
        assert_eq!(
            offer.submission().output_identity,
            predecessor.output_identity
        );
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_audio_source(),
            Some(predecessor)
        );
        assert_eq!(h.render(), [[0.25; 2]; 64]);
        let decided = if proceed {
            offer.proceed()
        } else {
            offer.cancel()
        };
        assert!(decided);
        let terminal = h.next_with_audio();
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let receipt = terminal.device_insert_complete.unwrap();
        assert!(terminal.request_complete);
        if proceed {
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
            assert_ne!(
                receipt.submission.audio_revision,
                offer.submission().audio_revision
            );
            assert_eq!(receipt.submission.context, offer.submission().context);
            assert_eq!(terminal.state.revision, imported.state.revision + 1);
            assert_eq!(terminal.state.project.main_devices().len(), 1);
            let installed = h.applied(h.context.unwrap());
            assert_eq!(installed.audio_revision, receipt.submission.audio_revision);
            assert_eq!(installed.output_identity, predecessor.output_identity);
        } else {
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Cancelled);
            assert_eq!(terminal.state.project, imported.state.project);
            assert_eq!(terminal.state.revision, imported.state.revision);
            assert_eq!(h.render(), [[0.25; 2]; 64]);
        }
    }
    assert!(h.run(Request::SaveCurrent).save_complete);
    assert_eq!(hex_storage::load(&saved).unwrap().main_devices().len(), 1);
    let undone = h.run(Request::Undo);
    let mut restored = undone.state.project.snapshot();
    let before = imported.state.project.snapshot();
    assert!(restored.next_id > before.next_id);
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(undone.state.can_redo);
}

#[test]
fn stopped_verified_stream_installs_auto_pan_and_renders_processed_audio() {
    check_auto_pan(false);
}

#[test]
fn running_verified_stream_installs_auto_pan_and_renders_processed_audio() {
    check_auto_pan(true);
}

fn check_auto_pan(running: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(1);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 100_000)));
    assert!(matches!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
            .1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    h.applied(h.context.unwrap());
    if running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        wait_for_audio(&h);
    }
    let track = imported.state.project.tracks()[0].id();
    let auto_pan = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.auto-pan"),
    )
    .unwrap();
    let inserted = run_insert(
        &mut h,
        Command::InsertMainDevice {
            before: None,
            state: auto_pan,
        },
    );
    let Some(Created::Device(device)) = inserted.created else {
        panic!("Auto Pan device")
    };
    h.applied(h.context.unwrap());
    if !running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    wait_for_audio(&h);
    let mut asymmetric = false;
    for _ in 0..20 {
        let block = h.render();
        asymmetric |= block
            .iter()
            .any(|frame| (frame[0] - frame[1]).abs() > 0.0001);
    }
    assert!(
        asymmetric,
        "Auto Pan must process the mono source into different channels"
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(status.rejected_revision().is_none());
    if running {
        let key = GestureKey {
            generation: h.context.unwrap().project_generation,
            token: 1,
        };
        for request in [
            GestureRequest::Begin {
                key,
                target: GestureTarget::TrackGain { track },
            },
            GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.1),
            },
        ] {
            let event = h.run(Request::DeviceGesture(request));
            assert!(
                event.gesture_ack.as_ref().unwrap().accepted,
                "{:?}",
                event.gesture_ack
            );
        }
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if frame[0].abs().max(frame[1].abs()) < 0.05 && frame != [0.0; 2] {
                break;
            }
            assert!(
                Instant::now() < until,
                "gain preview did not reach audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let event = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
        assert!(event.gesture_ack.as_ref().unwrap().accepted);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if frame[0].abs().max(frame[1].abs()) > 0.1 {
                break;
            }
            assert!(
                Instant::now() < until,
                "gain cancel did not restore audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let key = GestureKey { token: 2, ..key };
        for request in [
            GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: ParameterId::from_static(1),
                },
            },
            GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.0),
            },
        ] {
            let event = h.run(Request::DeviceGesture(request));
            assert!(
                event.gesture_ack.as_ref().unwrap().accepted,
                "{:?}",
                event.gesture_ack
            );
        }
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if (frame[0] - frame[1]).abs() < 0.00001 && frame[0].abs() > 0.1 {
                break;
            }
            assert!(
                Instant::now() < until,
                "device preview did not reach audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let event = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
        assert!(event.gesture_ack.as_ref().unwrap().accepted);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let frame = h.render()[63];
            if (frame[0] - frame[1]).abs() > 0.0001 {
                break;
            }
            assert!(
                Instant::now() < until,
                "device cancel did not restore audio: {frame:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn wait_for_audio(h: &Harness) {
    let ready = Instant::now() + Duration::from_secs(3);
    while !h
        .render()
        .iter()
        .any(|frame| frame[0] != 0.0 || frame[1] != 0.0)
    {
        assert!(
            Instant::now() < ready,
            "verified source did not become audible"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn verified_track_limiter_running_tempo_edit_preserves_beat_and_audio() {
    let _native = native_fixture();
    for partition in [1, 257] {
        verified_limiter_tempo_case(partition, None);
    }
}

#[test]
fn verified_track_limiter_running_tempo_edit_wraps_the_retimed_loop() {
    let _native = native_fixture();
    for partition in [1, 257] {
        verified_limiter_tempo_case(partition, Some(6015..6207));
    }
}

fn verified_limiter_tempo_case(partition: usize, loop_range: Option<std::ops::Range<u64>>) {
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(3);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 100_000)));
    assert!(matches!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
            .1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    h.applied(h.context.unwrap());
    let track = imported.state.project.tracks()[0].id();
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    run_insert(
        &mut h,
        Command::InsertDevice {
            track,
            before: None,
            state: limiter,
        },
    );
    let mut previous = h.applied(h.context.unwrap());
    if let Some(range) = &loop_range {
        h.run(Request::Edit(Command::SetLoopRegion {
            start: range.start,
            end: range.end,
            enabled: true,
        }));
        previous = h.applied(h.context.unwrap());
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(match &loop_range {
            Some(range) => PlaybackCommand::PlayFrom(range.start),
            None => PlaybackCommand::Play,
        })
        .unwrap();
    wait_for_audio(&h);
    let old_tempo = imported.state.project.tempo_map().clone();
    let edited = h.run(Request::Edit(Command::SetProjectTempo {
        tempo: hex_project::Tempo::new(90_000_000).unwrap(),
    }));
    let target = h.context.unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let (before, after) = loop {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let old = status.applied_audio_source();
        let before = status.position.load(Acquire);
        let mut output = [[0.0; 2]; 257];
        let mut backend = h.backend.lock().unwrap();
        let measured = allocation_counter::measure(|| {
            backend
                .as_mut()
                .unwrap()
                .render(&mut output[..partition])
                .unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        drop(backend);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if old.is_some_and(|source| source == previous)
            && status
                .applied_audio_source()
                .is_some_and(|source| source.context == target)
        {
            break (before, status.position.load(Acquire));
        }
        assert!(
            Instant::now() < deadline,
            "verified tempo edit did not publish: {:?}",
            status.rejected_revision()
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let beat = old_tempo.frame_to_beat(before).unwrap();
    let expected = edited
        .state
        .project
        .tempo_map()
        .beat_to_frame(beat)
        .unwrap();
    let advanced = expected + partition as u64;
    let expected = if let Some(old_range) = &loop_range {
        let region = edited.state.project.loop_region().unwrap();
        assert!(region.enabled());
        assert_eq!(region.start()..region.end(), 8020..8276);
        assert!(old_range.contains(&before));
        assert!((region.start()..region.end()).contains(&expected));
        if partition == 257 {
            assert!(advanced >= region.end(), "publication block must wrap");
        }
        region.start() + (advanced - region.start()) % (region.end() - region.start())
    } else {
        advanced
    };
    assert_eq!(after, expected, "partition {partition}");
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(status.rejected_revision().is_none());
    if loop_range.is_some() {
        // Clear any samples buffered in the limiter before observing new audio.
        for _ in 0..8 {
            h.render();
        }
    }
    wait_for_audio(&h);
}

#[test]
fn stopped_verified_stream_installs_main_saturator() {
    check_main_saturator(false);
}

#[test]
fn running_verified_stream_installs_main_saturator() {
    check_main_saturator(true);
}

fn check_main_saturator(running: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.wait = Duration::from_secs(1);
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    h.run(Request::Import(files.wav(48_000, 100_000)));
    let prior = h.applied(h.context.unwrap());
    if running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        wait_for_audio(&h);
    }
    let saturator = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_engine::DeviceTypeId::from_static("hex.saturator"),
    )
    .unwrap();
    run_insert(
        &mut h,
        Command::InsertMainDevice {
            before: None,
            state: saturator,
        },
    );
    let installed = h.applied(h.context.unwrap());
    assert_ne!(installed.audio_revision, prior.audio_revision);
    if !running {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    wait_for_audio(&h);
    for _ in 0..8 {
        h.render();
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire));
    assert!(
        status.position.load(Acquire) > status.audible_position.load(Acquire),
        "Main processor reports latency"
    );
    if running {
        assert!(matches!(
            status.rejected_revision(),
            Some((revision, hex_engine::PlaybackHandoffRejection::DeviceInsertNeedsConfirmation))
                if revision < installed.audio_revision
        ));
    } else {
        assert!(status.rejected_revision().is_none());
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Seek(70_000))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire)
        < 70_000
    {
        h.render();
        assert!(Instant::now() < deadline, "streamed seek did not commit");
        std::thread::sleep(Duration::from_millis(1));
    }
    wait_for_audio(&h);
}
