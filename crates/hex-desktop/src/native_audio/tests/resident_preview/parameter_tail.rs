//! Ordinary device gesture publication against an untouched rendered reference.
use super::*;
use hex_project::{Command, Created};
use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};

fn compare_block(h: &ResidentHarness, reference: &mut Playback) -> [hex_engine::Frame; 64] {
    let position = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .position
        .load(Acquire);
    assert_eq!(
        reference.position(),
        position,
        "every callback block is observed"
    );
    let actual = h.render();
    let mut expected = [[0.0; 2]; 64];
    reference.render(&mut expected).unwrap();
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-6,
                "frame {} channel {channel}: {actual:?}, expected {expected:?}",
                position + index as u64,
            );
        }
    }
    actual
}

fn submit_gesture(
    h: &mut ResidentHarness,
    reference: &mut Playback,
    request: GestureRequest,
) -> Event {
    h.service.submit(Request::device_gesture(request)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        h.pump();
        compare_block(h, reference);
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.gesture_ack.as_ref().unwrap().accepted);
            return event;
        }
        assert!(Instant::now() < deadline, "gesture terminal did not arrive");
        std::thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn saturator_gesture_keeps_quiet_output_and_the_existing_delay_decay() {
    let _native = native_fixture();
    let files = Files::new();
    let saturator_type = hex_engine::DeviceTypeId::from_static("hex.saturator");
    let drive = hex_engine::native_device_descriptors(saturator_type)
        .unwrap()
        .iter()
        .find(|descriptor| descriptor.label == "drive")
        .unwrap()
        .id;
    let path = files.constant_wav("burst.wav", 48_000, 32_768, &[0, 0]);
    let mut wav = fs::read(&path).unwrap();
    let mut samples = vec![[0.0; 2]; 32_768];
    for (frame, sample) in samples.iter_mut().take(96).enumerate() {
        let value = if frame % 5 < 2 { 8192_i16 } else { -4096_i16 };
        *sample = [f32::from(value) / 32768.0; 2];
        for channel in 0..2 {
            let offset = 44 + frame * 4 + channel * 2;
            wav[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    fs::write(&path, wav).unwrap();
    for with_delay in [false, true] {
        let mut h = ResidentHarness::with_frame_limit(262_144);
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let imported = h.run(Request::Import(path.clone()));
        let track = imported.state.project.tracks()[0].id();
        let asset = imported.state.project.assets().next().unwrap().id();
        applied = h.applied(applied).0;
        let inserted = h.run(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(saturator_type).unwrap(),
        }));
        let Some(Created::Device(device)) = inserted.created else {
            panic!("saturator")
        };
        applied = h.applied(applied).0;
        let project = if with_delay {
            let mut delay = hex_engine::default_native_device_state(
                hex_engine::DeviceTypeId::from_static("hex.delay"),
            )
            .unwrap();
            for (id, value) in [
                (hex_engine::delay::LEFT_SYNC, ParameterValue::Bool(false)),
                (hex_engine::delay::FILTER, ParameterValue::Bool(false)),
                (hex_engine::delay::HIGH_QUALITY, ParameterValue::Bool(false)),
                (hex_engine::delay::TIME, ParameterValue::Float(10.0)),
                (hex_engine::delay::FEEDBACK, ParameterValue::Float(0.8)),
                (hex_engine::delay::DRY_WET, ParameterValue::Float(1.0)),
            ] {
                delay = hex_engine::patch_native_device_parameter(&delay, id, value).unwrap();
            }
            let inserted = h.run(Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state: delay,
            }));
            applied = h.applied(applied).0;
            inserted.state.project.clone()
        } else {
            inserted.state.project.clone()
        };
        let prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::from([(
                    asset,
                    hex_engine::Audio::new(48_000, samples.clone()).unwrap(),
                )]),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut reference = Playback::try_from_prepared_with_session(prepared).unwrap();
        reference.apply(PlaybackCommand::PlayFrom(0));
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .unwrap();
        let mut heard_burst = false;
        for _ in 0..32 {
            heard_burst |= compare_block(&h, &mut reference)
                .iter()
                .flatten()
                .any(|v| v.abs() > 1e-6);
        }
        assert!(heard_burst);
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        submit_gesture(
            &mut h,
            &mut reference,
            GestureRequest::Begin {
                key,
                target: GestureTarget::DeviceParameter {
                    device,
                    parameter: drive,
                },
            },
        );
        for (sequence, commit) in [(1, false), (2, true)] {
            let mut heard_tail = false;
            let request = if commit {
                GestureRequest::Commit {
                    key,
                    sequence,
                    value: ParameterValue::Float(18.0),
                    manual_edit: true,
                }
            } else {
                GestureRequest::Change {
                    key,
                    sequence,
                    value: ParameterValue::Float(18.0),
                }
            };
            let event = submit_gesture(&mut h, &mut reference, request);
            if commit {
                assert_eq!(
                    hex_engine::native_device_parameter(
                        event.state.project.device(device).unwrap().state(),
                        drive,
                    )
                    .unwrap(),
                    ParameterValue::Float(18.0),
                );
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                h.pump();
                let block = compare_block(&h, &mut reference);
                heard_tail |= block.iter().flatten().any(|v| v.abs() > 1e-6);
                if !with_delay {
                    assert!(block.iter().flatten().all(|v| *v == 0.0));
                }
                let status = h.audio.as_ref().unwrap().status().unwrap();
                if let Some((revision, _)) = h.submitted
                    && revision > applied
                    && status.applied_revision.load(Acquire) == revision
                {
                    applied = revision;
                    break;
                }
                assert!(Instant::now() < deadline, "gesture graph was not installed");
                std::thread::yield_now();
            }
            for _ in 0..16 {
                let block = compare_block(&h, &mut reference);
                heard_tail |= block.iter().flatten().any(|v| v.abs() > 1e-6);
            }
            assert_eq!(heard_tail, with_delay, "tail present after commit={commit}");
        }
    }
}
