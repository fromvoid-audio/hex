//! Verified gain uses the actual native worker, publication and callback.
use super::*;
use crate::native_audio::tests::{
    Files, Harness, native_fixture, verified_gain::actual_native_harness,
};
use hex_engine::PlaybackCommand;
use hex_session::{
    device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue},
    service::Request,
};
use std::{
    fs,
    sync::Mutex,
    time::{Duration, Instant},
};

#[test]
fn restored_stream_source_recovers_through_actual_desktop_restart() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let path = files.wav(48_000, 16_384);
    let original = fs::read(&path).unwrap();
    let imported = h.run(Request::Import(path.clone()));
    let context = h.context.unwrap();
    let expected_project = imported.state.project.clone();
    let expected_revision = imported.state.revision;
    let expected_undo = imported.state.can_undo;
    let expected_redo = imported.state.can_redo;
    h.applied(context);
    let audio = h.audio.as_mut().unwrap();
    let old_identity = audio.control.output_identity().unwrap();
    audio.control.command(PlaybackCommand::Play).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.render();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
            >= 64
        {
            assert_eq!(output, [[0.25; 2]; 64]);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    fs::write(&path, b"changed source bytes").unwrap();
    let seek = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Seek(4096))
        .unwrap();
    loop {
        h.render();
        if let Some((ticket, reason)) = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .rejected_action()
            && ticket == seek
        {
            assert_eq!(
                reason,
                hex_engine::PlaybackActionRejection::StreamSourceChanged
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "changed source seek was not rejected"
        );
        std::thread::yield_now();
    }
    fs::write(&path, &original).unwrap();
    h.audio.take().unwrap().control.close();
    h.backend.lock().unwrap().take();
    let restarted = h.run(Request::RestartAudio {
        project_generation: context.project_generation,
        request_id: 71,
    });
    assert_eq!(restarted.state.project, expected_project);
    assert_eq!(restarted.state.revision, expected_revision);
    assert_eq!(restarted.state.can_undo, expected_undo);
    assert_eq!(restarted.state.can_redo, expected_redo);
    h.open();
    assert_ne!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(old_identity)
    );
    h.applied(context);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    loop {
        let output = h.render();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire)
            >= 64
        {
            assert_eq!(output, [[0.25; 2]; 64]);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "reverified source did not resume"
        );
        std::thread::yield_now();
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn repeated_device_errors_refuse_publication_until_desktop_restart() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 16_384)));
    let track = imported.state.project.tracks()[0].id();
    h.applied(h.context.unwrap());
    let mut retired_identities = Vec::new();
    for (request_id, gain, expected_sample) in [(72, 0.5, 0.125), (73, 0.25, 0.0625)] {
        while h.events.try_recv().is_ok() {}
        let old_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        assert!(!retired_identities.contains(&old_identity));
        retired_identities.push(old_identity);
        let measured = {
            let mut backend = h.backend.lock().unwrap();
            allocation_counter::measure(|| backend.as_mut().unwrap().report_device_error().unwrap())
        };
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .device_fault
                .load(Acquire)
        );
        let changed = h.run(Request::Edit(hex_project::Command::Gain { track, gain }));
        let context = h.context.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(EditorAudioEvent::Publication {
                context: seen,
                state: EditorPublicationState::Fault(reason),
                ..
            }) = h.events.try_recv()
                && seen == context
            {
                assert!(reason.contains("restart audio"));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "faulted publication was not reported"
            );
            std::thread::yield_now();
        }
        h.audio.take().unwrap().control.close();
        h.backend.lock().unwrap().take();
        let restarted = h.run(Request::RestartAudio {
            project_generation: context.project_generation,
            request_id,
        });
        assert_eq!(restarted.state.project, changed.state.project);
        assert_eq!(restarted.state.revision, changed.state.revision);
        assert_eq!(restarted.state.can_undo, changed.state.can_undo);
        assert_eq!(restarted.state.can_redo, changed.state.can_redo);
        h.open();
        assert!(
            !retired_identities
                .contains(&h.audio.as_ref().unwrap().control.output_identity().unwrap())
        );
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .device_fault
                .load(Acquire)
        );
        h.applied(context);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        loop {
            let output = h.render();
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire)
                >= 64
            {
                assert_eq!(output, [[expected_sample; 2]; 64]);
                break;
            }
            assert!(Instant::now() < deadline, "restarted output did not resume");
            std::thread::yield_now();
        }
    }
}

#[test]
fn gain_diagnostics_correlate_session_outcome_submission_and_callback_application() {
    use hex_audio::audio_diagnostics::{AudioDiagnosticKind, AudioDiagnosticsFlags};
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};

    let _native = native_fixture();
    let files = Files::new();
    let mut h = actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(files.wav(48_000, 480_000)));
    let track = imported.state.project.tracks()[0].id();
    let before = h.applied(h.context.unwrap());
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    wait_gain(&h, before.audio_revision.saturating_sub(1), 1.0);
    while h.events.try_recv().is_ok() {}

    let rows = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&rows);
    let observer = Arc::new(DiagnosticObserver::new(move |event| {
        sink.lock()
            .unwrap()
            .push((event.request, event.stage, event.details.clone()));
    }));
    h.service
        .as_ref()
        .unwrap()
        .set_diagnostic_observer(Arc::clone(&observer))
        .unwrap();
    let audio = h.audio.as_mut().unwrap();
    let output = audio.control.output_identity().unwrap();
    audio
        .control
        .set_audio_diagnostics(AudioDiagnosticsFlags::PUBLICATION);
    while audio.control.take_audio_diagnostic().is_some() {}

    let scope = observer.enter_ui_action(42);
    let changed = h.run(Request::Edit(hex_project::Command::Gain {
        track,
        gain: 0.5,
    }));
    drop(scope);
    let context = changed
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    let rows = rows.lock().unwrap();
    let request = rows
        .iter()
        .find_map(|(request, stage, _)| {
            (*stage == DiagnosticStage::SubmitAttempt)
                .then_some(*request)
                .flatten()
        })
        .unwrap();
    assert_eq!(request.ui_action_id, Some(42));
    assert_eq!(request.operation, Some("gain"));
    assert!(rows.iter().any(|(id, stage, _)| *id == Some(request)
        && matches!(stage,
        DiagnosticStage::ClientReceived { generation: Some(generation), revision, error: false }
        if *generation == context.project_generation && *revision == context.editor_revision)));
    drop(rows);

    let deadline = Instant::now() + Duration::from_secs(10);
    let submitted = loop {
        match h.events.try_recv() {
            Ok(EditorAudioEvent::Publication {
                context: seen,
                output_identity,
                state: EditorPublicationState::Submitted { audio_revision },
            }) if seen == context && output_identity == output => break audio_revision,
            Ok(EditorAudioEvent::Publication {
                context: seen,
                state:
                    EditorPublicationState::Refused(reason) | EditorPublicationState::Fault(reason),
                ..
            }) if seen == context => panic!("gain publication refused: {reason}"),
            _ => {}
        }
        h.render();
        assert!(
            Instant::now() < deadline,
            "gain submission was not observed"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    let applied = wait_gain(&h, before.audio_revision, 0.5);
    assert_eq!(applied.audio_revision, submitted);
    let audio = h.audio.as_mut().unwrap();
    assert_eq!(audio.status().unwrap().audio_diagnostics_dropped(), 0);
    let events: Vec<_> = std::iter::from_fn(|| audio.control.take_audio_diagnostic()).collect();
    assert!(events.iter().any(|event| event.output == output
        && event.revision == submitted
        && event.context == Some(context)
        && event.kind == AudioDiagnosticKind::PublicationApplied));
    audio
        .control
        .set_audio_diagnostics(AudioDiagnosticsFlags::OFF);
    h.render();
    assert!(
        h.audio
            .as_mut()
            .unwrap()
            .control
            .take_audio_diagnostic()
            .is_none()
    );
}

#[test]
fn verified_arrangement_gain_reaches_callback_before_commit() {
    verified_gain_case(false);
}

#[test]
fn verified_session_gain_reaches_callback_before_commit() {
    verified_gain_case(true);
}
fn verified_gain_case(session: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 480_000);
    let mut h = actual_native_harness();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    h.applied(h.context.unwrap());
    let imported = h.run(Request::Import(path));
    let source = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(matches!(
        source.required_sources().next().unwrap().1,
        AudioSourceBinding::VerifiedFile(_)
    ));
    let track = imported.state.project.tracks()[0].id();
    let session_clip = if session {
        let scene = h
            .run(Request::Edit(hex_project::Command::AddScene {
                name: "gain".into(),
            }))
            .state
            .project
            .scenes()[0]
            .id();
        let source = imported.state.project.track(track).unwrap().clips()[0].id();
        let copied = h.run(Request::Edit(
            hex_project::Command::CopyArrangementAudioClipToSession { source, scene },
        ));
        let hex_project::SessionCellContent::Audio(clip) =
            copied.state.project.session_cells()[0].content()
        else {
            panic!("audio cue");
        };
        Some(clip.id())
    } else {
        None
    };
    let mut canonical = h.run(Request::Edit(hex_project::Command::Gain {
        track,
        gain: 0.0,
    }));
    let applied = h.applied(h.context.unwrap());
    if let Some(clip) = session_clip {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(h.render().iter().flatten().all(|sample| *sample == 0.0));
            let mut routes = [hex_audio::prepared::SessionRouteEntry::default(); 1];
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .session_routes(&mut routes)
                .is_some_and(|snapshot| snapshot.count == 1)
                && routes[0].owner
                    == (hex_audio::prepared::SessionRouteOwner::Session {
                        clip_id: clip.value(),
                    })
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "actual verified Session route did not launch: rejected={:?}, consumed={}, scheduled={}, active={}, playing={}",
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .rejected_action(),
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .consumed_command
                    .load(Acquire),
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .stream_scheduled_command
                    .load(Acquire),
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .stream_activated_command
                    .load(Acquire),
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .playing
                    .load(Acquire)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    } else {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    for _ in 0..4 {
        assert!(h.render().iter().flatten().all(|sample| *sample == 0.0));
    }
    // A constant Arrangement source could otherwise hide a lost Console owner.
    // Verify the actual route on every gain/terminal publication as well as PCM.
    let wait = |h: &Harness, after, gain| {
        let actual = wait_gain(h, after, gain);
        if let Some(clip) = session_clip {
            let mut routes = [hex_audio::prepared::SessionRouteEntry::default(); 1];
            let snapshot = h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .session_routes(&mut routes)
                .unwrap();
            assert_eq!(snapshot.audio_revision, actual.audio_revision);
            assert_eq!(snapshot.count, 1);
            assert_eq!(routes[0].track_id, track.value());
            assert_eq!(
                routes[0].owner,
                hex_audio::prepared::SessionRouteOwner::Session {
                    clip_id: clip.value()
                }
            );
        }
        actual
    };
    let key = GestureKey {
        generation: applied.context.project_generation,
        token: 1,
    };
    let began = h.run(Request::DeviceGesture(GestureRequest::Begin {
        key,
        target: GestureTarget::TrackGain { track },
    }));
    assert!(
        began.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        began.gesture_ack
    );
    let began_at = Instant::now();
    let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
        key,
        sequence: 1,
        value: ParameterValue::Float(1.0),
    }));
    assert!(
        preview.gesture_ack.as_ref().unwrap().accepted,
        "{:?}",
        preview.gesture_ack
    );
    assert!(
        preview.playback.is_none(),
        "verified sources never use resident fallback"
    );
    assert_eq!(preview.state.project, canonical.state.project);
    assert_eq!(preview.state.revision, canonical.state.revision);
    assert_eq!(preview.state.unsaved, canonical.state.unsaved);
    let mut applied = wait(&h, applied.audio_revision, 1.0);
    report_timing(session, "zero-to-positive", began_at);
    let initial_preview = h
        .observation
        .lock()
        .unwrap()
        .received_verified_gain
        .clone()
        .unwrap();
    assert_eq!(
        initial_preview.preview_identity(),
        Some(preview.device_preview.as_ref().unwrap().identity)
    );
    for (sequence, gain) in [(2, 0.0), (3, 2.0)] {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(gain),
        }));
        assert!(changed.gesture_ack.as_ref().unwrap().accepted);
        assert_eq!(changed.state.project, canonical.state.project);
        assert_eq!(changed.state.revision, canonical.state.revision);
        applied = wait(&h, applied.audio_revision, gain);
    }
    // Fill native publication capacity while the callback is deliberately idle.
    // Logical changes stay bounded/coalesced; the final absolute value must win.
    let backlog_at = Instant::now();
    for sequence in 4..=35 {
        let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence,
            value: ParameterValue::Float(sequence as f32 / 64.0),
        }));
        assert!(changed.gesture_ack.as_ref().unwrap().accepted);
        assert_eq!(changed.state.project, canonical.state.project);
    }
    let final_gain = 35.0 / 64.0;
    applied = wait(&h, applied.audio_revision, final_gain);
    report_timing(session, "32-change-idle-callback-backlog", backlog_at);
    canonical = h.run(Request::Edit(hex_project::Command::RenameTrack {
        track,
        name: "rebased source".into(),
    }));
    assert_eq!(canonical.state.project.track(track).unwrap().gain(), 0.0);
    assert_eq!(
        canonical
            .device_preview
            .as_ref()
            .unwrap()
            .state
            .project
            .track(track)
            .unwrap()
            .gain(),
        final_gain
    );
    applied = wait(&h, applied.audio_revision, final_gain);
    let old_preview = h
        .observation
        .lock()
        .unwrap()
        .received_verified_gain
        .clone()
        .unwrap();
    let cancel_at = Instant::now();
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    assert!(cancelled.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(cancelled.state.project, canonical.state.project);
    assert_eq!(cancelled.state.can_undo, canonical.state.can_undo);
    applied = wait(&h, applied.audio_revision, 0.0);
    report_timing(session, "cancel", cancel_at);
    let old_terminal = h
        .observation
        .lock()
        .unwrap()
        .received_verified_gain
        .clone()
        .unwrap();
    h.observation.lock().unwrap().replay_verified_gain = Some(Arc::clone(&old_preview));
    stable_gain(&h, applied, 0.0);

    let refused_future = h.run(Request::DeviceGesture(GestureRequest::Cancel {
        key: GestureKey { token: 99, ..key },
    }));
    assert!(!refused_future.gesture_ack.as_ref().unwrap().accepted);
    let second = GestureKey { token: 2, ..key };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: second,
        target: GestureTarget::TrackGain { track },
    }));
    let changed = h.run(Request::DeviceGesture(GestureRequest::Change {
        key: second,
        sequence: 1,
        value: ParameterValue::Float(2.0),
    }));
    assert!(changed.gesture_ack.as_ref().unwrap().accepted);
    applied = wait(&h, applied.audio_revision, 2.0);
    h.observation.lock().unwrap().replay_verified_gain = Some(old_terminal);
    stable_gain(&h, applied, 2.0);
    let commit_at = Instant::now();
    let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
        manual_edit: true,
        key: second,
        sequence: 1,
        value: ParameterValue::Float(2.0),
    }));
    assert!(committed.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(committed.state.revision, canonical.state.revision + 1);
    assert_eq!(committed.state.project.track(track).unwrap().gain(), 2.0);
    applied = wait(&h, applied.audio_revision, 2.0);
    report_timing(session, "commit", commit_at);
    let undo = h.run(Request::Undo);
    assert_eq!(undo.state.project, canonical.state.project);
    applied = wait(&h, applied.audio_revision, 0.0);
    let redone = h.run(Request::Redo);
    applied = wait(&h, applied.audio_revision, 2.0);
    assert_eq!(applied.context, h.context.unwrap());

    let third = GestureKey { token: 3, ..key };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: third,
        target: GestureTarget::TrackGain { track },
    }));
    h.run(Request::DeviceGesture(GestureRequest::Change {
        key: third,
        sequence: 1,
        value: ParameterValue::Float(0.5),
    }));
    applied = wait(&h, applied.audio_revision, 0.5);
    let invalid = h.run(Request::DeviceGesture(GestureRequest::Commit {
        manual_edit: true,
        key: third,
        sequence: 2,
        value: ParameterValue::Float(f32::NAN),
    }));
    assert!(!invalid.gesture_ack.as_ref().unwrap().accepted);
    assert!(
        invalid.active_device_gesture.is_none(),
        "retired refusal has an owned terminal"
    );
    assert_eq!(invalid.state.project, redone.state.project);
    assert_eq!(invalid.state.revision, redone.state.revision);
    applied = wait(&h, applied.audio_revision, 2.0);
    let fourth = GestureKey { token: 4, ..key };
    h.run(Request::DeviceGesture(GestureRequest::Begin {
        key: fourth,
        target: GestureTarget::TrackGain { track },
    }));
    h.run(Request::DeviceGesture(GestureRequest::Change {
        key: fourth,
        sequence: 1,
        value: ParameterValue::Float(0.75),
    }));
    wait(&h, applied.audio_revision, 0.75);
    let removed = h.run(Request::Edit(hex_project::Command::RemoveTrack(track)));
    assert!(removed.state.project.track(track).is_none());
    assert!(removed.active_device_gesture.is_none());
    let empty_current = h.applied(h.context.unwrap());
    stable_gain(&h, empty_current, 0.0);

    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    let empty = h.applied(h.context.unwrap());
    h.observation.lock().unwrap().replay_verified_gain = Some(old_preview);
    stable_gain(&h, empty, 0.0);
}

fn wait_gain(h: &Harness, after: u64, gain: f32) -> hex_audio::prepared::AppliedAudioSource {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let before = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            status.position.load(Acquire) >= before,
            "gain must not reset the musical clock"
        );
        if let Some(actual) = status.applied_audio_source()
            && actual.context == h.context.unwrap()
            && actual.audio_revision > after
            && pcm
                .iter()
                .flatten()
                .all(|sample| (*sample - 0.25 * gain).abs() < 0.0001)
        {
            return actual;
        }
        assert!(
            Instant::now() < deadline,
            "absolute gain {gain} did not reach literal verified PCM"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn stable_gain(h: &Harness, applied: hex_audio::prepared::AppliedAudioSource, gain: f32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut frames = 0;
    while frames < 20 * 64 {
        let pcm = h.render();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let committed = if status.stream_buffering.load(Acquire) {
            usize::try_from(status.stream_committed_frames.load(Acquire)).unwrap()
        } else {
            pcm.len()
        };
        assert!(committed <= pcm.len());
        // This headless producer can outrun the page worker. Validate its exact
        // committed prefix and explicit BUFFERING silence, never count missing
        // pages as successful target PCM or as physical-device timing evidence.
        for (index, frame) in pcm.iter().enumerate() {
            let expected = if index < committed { 0.25 * gain } else { 0.0 };
            assert!(
                frame
                    .iter()
                    .all(|sample| (*sample - expected).abs() < 0.0001),
                "gain {gain}, committed={committed}, frame={index}: {frame:?}"
            );
        }
        assert_eq!(status.applied_audio_source(), Some(applied));
        frames += committed;
        assert!(
            Instant::now() < deadline,
            "verified page worker made no sustained progress"
        );
        if committed == 0 {
            std::thread::sleep(Duration::from_millis(1));
        } else {
            std::thread::yield_now();
        }
    }
}

// Observational headless timing only: includes worker polling and 1ms fixture
// sleeps, not UI input latency or a physical audio-device deadline.
fn report_timing(session: bool, phase: &str, start: Instant) {
    eprintln!(
        "verified_gain_timing route={} phase={phase} elapsed_us={} sample_rate=48000 block_frames=64 optimized={} physical_audio=false",
        if session { "session" } else { "arrangement" },
        start.elapsed().as_micros(),
        !cfg!(debug_assertions)
    );
}
