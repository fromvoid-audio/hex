//! Minimal independently authored streamed/native gain reproducer.
use super::resident_preview::ResidentHarness;
use super::*;
use hex_project::Command;
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

#[test]
fn streamed_native_effect_gain_and_mute_after_mid_song_start_reach_pcm() {
    gain_case(false);
}

#[test]
fn reopened_streamed_native_effect_gain_bursts_reach_latest_pcm() {
    gain_case(true);
}

fn gain_case(reopen_and_burst: bool) {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut h = ResidentHarness::with_frame_limit_at_rate(8_000_000, 44_100);
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
        .set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    let mut intent = h.seek_intent(1, authored.state.revision, None, None);
    intent.destination_frame = 65_536;
    intent.project_generation = generation;
    h.service.submit_resident_arrangement_seek(intent).unwrap();
    assert!(h.next().error.is_none());
    assert!(h.seek_result().result.is_ok());
    (applied, _) = h.applied(applied);
    for _ in 0..32 {
        h.render();
    }
    assert!(h.render().iter().flatten().any(|v| v.abs() > 1e-8));
    for token in 1..=6 {
        while h
            .audio
            .as_mut()
            .unwrap()
            .control
            .take_audio_diagnostic()
            .is_some()
        {}
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
}
