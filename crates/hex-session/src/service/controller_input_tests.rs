//! Worker-side lease admission and same-instrument revision retention.
use super::*;
use crate::test_wait::HANG_BOUND;
use hex_engine::live_expression::{LiveExpressionInput, Snapshot};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
#[track_caller]
fn next(service: &Service) -> Event {
    // Blocks on the event channel itself instead of polling against a short deadline.
    match service.events.recv_timeout(HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("worker event absent after {HANG_BOUND:?}: {error}"),
    }
}
#[test]
fn live_expression_worker_preserves_parameter_revisions_and_rejects_stale_generation() {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::InsertTrackWithDevice {
            name: "vector".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let added = next(&service);
    let track = added.state.project.tracks()[0].id();
    let instrument = added
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(608),
        ParameterValue::Choice(1),
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::ReplaceDeviceState {
            device: instrument,
            state: state.clone(),
        }))
        .unwrap();
    let edited = next(&service);
    let input = Arc::new(LiveExpressionInput::new(0, track, instrument));
    input.publish(Snapshot::new(100, 4096, 60).unwrap());
    service
        .submit(Request::PrepareLiveExpression {
            input: Arc::clone(&input),
            revision: edited.state.revision,
        })
        .unwrap();
    let admitted = next(&service);
    assert!(admitted.error.is_none(), "{:?}", admitted.error);
    assert!(admitted.playback.is_some());
    assert!(Arc::ptr_eq(
        admitted.live_expression_complete.as_ref().unwrap(),
        &input
    ));
    assert!(
        Arc::ptr_eq(&admitted.state, &edited.state),
        "activation does not edit project/history"
    );
    let changed = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(29),
        ParameterValue::Float(0.5),
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::ReplaceDeviceState {
            device: instrument,
            state: changed,
        }))
        .unwrap();
    let edited = next(&service);
    assert!(edited.error.is_none());
    assert!(edited.playback.is_some());
    assert!(
        !input.is_cancelled(),
        "same-instance parameter revisions retain the lease"
    );
    assert_eq!(input.snapshot(), Snapshot::new(100, 4096, 60).unwrap());
    let stale = Arc::new(LiveExpressionInput::new(1, track, instrument));
    service
        .submit(Request::PrepareLiveExpression {
            input: Arc::clone(&stale),
            revision: edited.state.revision,
        })
        .unwrap();
    let refused = next(&service);
    assert!(refused.error.as_deref().unwrap().contains("stale"));
    assert!(refused.live_expression_complete.is_none());
    assert!(refused.playback.is_none());
    assert!(stale.is_cancelled());
    assert!(!input.is_cancelled());
    service
        .submit(Request::Edit(hex_project::Command::RemoveInstrument {
            track,
        }))
        .unwrap();
    let removed = next(&service);
    assert!(removed.playback.is_some());
    assert!(
        input.is_cancelled(),
        "removal cancels before the replacement graph is published"
    );
}

#[test]
fn controller_activation_preserves_worker_prepared_computer_and_hardware_notes() {
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_engine::playback::Playback;
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::InsertTrackWithDevice {
            name: "vector".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let added = next(&service);
    let track = added.state.project.tracks()[0].id();
    let instrument = added
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::ReplaceDeviceState {
            device: instrument,
            state,
        }))
        .unwrap();
    let edited = next(&service);
    let keys = Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap());
    let oracle_keys = Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap());
    for input in [&keys, &oracle_keys] {
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let live = input.live_notes().unwrap();
        live.publish(
            live.current_activation_on_control().unwrap(),
            hex_engine::computer_keyboard::LiveNoteEvent::NoteOn {
                channel: 3,
                pitch: 64,
                velocity: 37,
            },
        )
        .unwrap();
    }
    service
        .submit(Request::PrepareComputerKeyboard {
            project_generation: 0,
            revision: edited.state.revision,
            request_id: 1,
            track,
            input: Arc::clone(&keys),
        })
        .unwrap();
    let mut keyboard = next(&service);
    assert!(keyboard.error.is_none());
    let mut active = keyboard.playback.take().unwrap();
    let mut oracle = Playback::from_prepared(Arc::clone(active.prepared_revision().unwrap()));
    oracle
        .prepare_computer_keyboard(&edited.state.project, track, Arc::clone(&oracle_keys))
        .unwrap();
    let mut actual = [[0.0; 2]; 257];
    let mut expected = actual;
    active.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    let controllers = Arc::new(LiveExpressionInput::new(0, track, instrument));
    service
        .submit(Request::PrepareLiveExpression {
            input: Arc::clone(&controllers),
            revision: edited.state.revision,
        })
        .unwrap();
    let mut activation = next(&service);
    assert!(activation.error.is_none(), "{:?}", activation.error);
    let mut candidate = activation.playback.take().unwrap();
    candidate.try_inherit_runtime_from(&mut active).unwrap();
    assert!(
        !keys.is_cancelled(),
        "controller activation must retain the physical-key lease"
    );
    active.cancel_computer_keyboard();
    drop(active);
    assert!(
        !keys.is_cancelled(),
        "retiring the old graph cannot cancel the new owner"
    );
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "mutable Vector phase and envelope must continue without a retrigger"
    );
    assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
    for input in [&keys, &oracle_keys] {
        let live = input.live_notes().unwrap();
        live.publish(
            live.current_activation_on_control().unwrap(),
            hex_engine::computer_keyboard::LiveNoteEvent::NoteOff {
                channel: 3,
                pitch: 64,
                velocity: 0,
            },
        )
        .unwrap();
    }
    keys.set_key(0, None).unwrap();
    oracle_keys.set_key(0, None).unwrap();
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn verified_streaming_refuses_controller_before_success_ack_or_playback() {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits::new(
                    hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                    1024 * 1024,
                ),
                max_retained_sources: 1,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 1024 * 1024,
            }),
        })
        .unwrap();
    let created = next(&service);
    assert!(created.error.is_none());
    service
        .submit(Request::Edit(hex_project::Command::InsertTrackWithDevice {
            name: "vector".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let added = next(&service);
    assert!(added.error.is_none());
    let track = added.state.project.tracks()[0].id();
    let instrument = added
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    service
        .submit(Request::Edit(hex_project::Command::ReplaceDeviceState {
            device: instrument,
            state,
        }))
        .unwrap();
    let edited = next(&service);
    assert!(edited.error.is_none());
    let generation = edited
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context()
        .project_generation;
    let input = Arc::new(LiveExpressionInput::new(generation, track, instrument));
    service
        .submit(Request::PrepareLiveExpression {
            input: Arc::clone(&input),
            revision: edited.state.revision,
        })
        .unwrap();
    let refused = next(&service);
    assert_eq!(
        refused.error.as_deref(),
        Some("controller input is unavailable for verified streaming audio")
    );
    assert!(input.is_cancelled());
    assert!(refused.live_expression_complete.is_none());
    assert!(refused.playback.is_none());
    assert!(
        Arc::ptr_eq(&refused.state, &edited.state),
        "refusal must not edit source or history"
    );
}
