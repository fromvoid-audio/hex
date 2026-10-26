//! Minimal independently authored streamed/native gain reproducer.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::Command;
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

#[test]
fn streamed_native_effect_gain_and_mute_after_mid_song_start_reach_pcm() {
    gain_case(false, false);
}

#[test]
fn reopened_streamed_native_effect_gain_bursts_reach_latest_pcm() {
    gain_case(true, false);
}

#[test]
fn immediate_play_from_gain_and_mute_reach_pcm() {
    gain_case(false, true);
}

#[test]
fn reopened_immediate_play_from_gain_bursts_reach_latest_pcm() {
    gain_case(true, true);
}

fn gain_case(reopen_and_burst: bool, immediate: bool) {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit_at_rate(8_000_000, 44_100);
    let case = format!("reopen-{reopen_and_burst}-immediate-{immediate}");
    let directory = std::env::var_os("HEX_AUDIO_AUDIT_LOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| files.0.join("diagnostics"))
        .join(&case);
    let mut log = hex_diagnostics::SessionLog::start_with_context(directory, &format!("headless_gain_audit case={case} build={} rate=44100 frames=64 native_device_creation=substituted", hex_ui::diagnostic_build_identity())).unwrap();
    let handle = log.handle();
    h.diagnostic_log = Some(handle.clone());
    h.service.set_diagnostic_observer(Arc::new(hex_session::service::diagnostics::DiagnosticObserver::new(move |event| {
        let _ = handle.record(&format!("action_diag v=1 stage=session service={} request={:?} observation={:?} details_truncated={}{}", event.service_id, event.request, event.stage, event.details_truncated, event.details));
    }))).unwrap();
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(44_100, 262_144)));
    let track = imported.state.project.tracks()[0].id();
    (applied, _) = h.applied(applied);
    let mut authored = h.run(Request::Edit(Command::SetLoopRegion {
        start: 0,
        end: 262_144,
        enabled: true,
    }));
    (applied, _) = h.applied(applied);
    for type_id in ["hex.delay", "hex.reverb", "hex.saturator"] {
        authored = h.run(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_engine::DeviceTypeId::from_static(type_id)),
            before: None,
        });
        assert!(authored.error.is_none(), "{:?}", authored.error);
        (applied, _) = h.applied(applied);
    }
    let mut generation = 0;
    if reopen_and_burst {
        let path = files.0.join("native-gain.hex");
        assert!(h.run(Request::SaveNew(path.clone())).save_complete);
        let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
        authored = h.run(Request::Open(path));
        assert!(authored.open_complete && authored.project_replaced);
        h.release_relinquished_output();
        h.wait_fresh_output(old_output);
        let (revision, metadata) = h.applied(0);
        applied = revision;
        generation = metadata.context.project_generation;
        assert!(generation > 0);
    }
    h.audio
        .as_ref()
        .unwrap()
        .control
        .set_audio_diagnostics("publication,readiness,transport".parse().unwrap());
    if immediate {
        let ticket = h
            .audio
            .as_mut()
            .unwrap()
            .control
            .command_with_ticket(PlaybackCommand::PlayFrom(65_536))
            .unwrap();
        let _ = log.handle().record(&format!("audio_audit stage=immediate_play_from ticket={ticket} frame=65536 generation={generation}"));
        let first = h.render();
        let audible = first.iter().flatten().any(|sample| sample.abs() > 1e-8);
        let _ = log.handle().record(&format!(
            "audio_audit stage=first_play_from_block audible={audible}"
        ));
        assert!(
            audible,
            "fresh play-from must render the audible source in its first block"
        );
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .stream_applied_command_prefix
                .load(Acquire),
            ticket
        );
    } else {
        let mut intent = h.seek_intent(1, authored.state.revision, None, None);
        intent.destination_frame = 65_536;
        intent.project_generation = generation;
        h.service.submit_resident_arrangement_seek(intent).unwrap();
        assert!(h.next().error.is_none());
        assert!(h.seek_result().result.is_ok());
        (applied, _) = h.applied(applied);
    }
    for _ in 0..32 {
        h.render();
    }
    assert!(h.render().iter().flatten().any(|v| v.abs() > 1e-8));
    for token in 1..=6 {
        drain_capture(&mut h);
        let key = GestureKey { generation, token };
        h.run(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }));
        let changes = if reopen_and_burst { 12 } else { 1 };
        for sequence in 1..=changes {
            let value = if sequence == changes { 0.0 } else { 0.25 };
            let preview = h.run(Request::DeviceGesture(GestureRequest::Change {
                key,
                sequence,
                value: ParameterValue::Float(value),
            }));
            let ack = preview.gesture_ack.as_ref().unwrap();
            if !ack.accepted {
                // A later edit cannot use a predecessor which is still queued.
                // Observe the explicit refusal, let that exact graph apply,
                // then retry the same user value/sequence rather than drop it.
                assert_eq!(
                    ack.reason.as_deref(),
                    Some("playback publication already pending")
                );
                let _ = log.handle().record(&format!("audio_audit stage=expected_backpressure token={token} sequence={sequence} reason=publication_already_pending"));
                (applied, _) = h.applied(applied);
                let retry = h.run(Request::DeviceGesture(GestureRequest::Change {
                    key,
                    sequence,
                    value: ParameterValue::Float(value),
                }));
                assert!(
                    retry.gesture_ack.as_ref().unwrap().accepted,
                    "{:?}",
                    retry.gesture_ack
                );
            }
        }
        let zero = loop {
            let (revision, metadata) = h.applied(applied);
            applied = revision;
            if metadata.device_preview.unwrap().sequence == changes {
                break revision;
            }
        };
        assert_eq!(
            h.render(),
            [[0.0; 2]; 64],
            "zero gain must silence all track output"
        );
        let committed = h.run(Request::DeviceGesture(GestureRequest::Commit {
            key,
            sequence: changes + 1,
            value: ParameterValue::Float(1.0),
        }));
        assert!(committed.gesture_ack.as_ref().unwrap().accepted);
        (applied, _) = h.applied(zero);
        assert!(h.render().iter().flatten().any(|v| v.abs() > 1e-8));
    }
    h.run(Request::Edit(Command::Mute { track, muted: true }));
    (applied, _) = h.applied(applied);
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    h.run(Request::Edit(Command::Mute {
        track,
        muted: false,
    }));
    h.applied(applied);
    assert!(h.render().iter().flatten().any(|v| v.abs() > 1e-8));
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    drain_capture(&mut h);
    let callback_losses = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .audio_diagnostics_dropped();
    let _ = log.handle().record(&format!(
        "audio_audit stage=finished callback_dropped={callback_losses}"
    ));
    log.shutdown().unwrap();
    let text = fs::read_to_string(log.path()).unwrap();
    assert!(text.contains("device_gesture"));
    assert!(text.contains("gesture_sequence=1"));
    assert!(text.contains("PublicationApplied"));
    assert!(text.contains("ClientReceived"));
    assert!(text.contains("PriorityStop"));
    if reopen_and_burst {
        assert!(text.contains("stage=expected_backpressure"));
    }
    println!(
        "audio audit case={case} log={} callback_dropped={callback_losses} logger_statistics={:?}",
        log.path().display(),
        log.handle().statistics()
    );
}

fn drain_capture(h: &mut ResidentHarness) {
    if let Some(audio) = h.audio.as_mut() {
        for _ in 0..hex_audio::audio_diagnostics::TRACE_CAPACITY {
            let Some(event) = audio.control.take_audio_diagnostic() else {
                break;
            };
            if let Some(log) = &h.diagnostic_log {
                let _ = log.record(&format!("audio_diag v=1 stage=callback event={event:?}"));
            }
        }
    }
}
