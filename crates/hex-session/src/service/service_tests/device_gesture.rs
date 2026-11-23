use super::*;

#[test]
fn native_device_parameter_requests_patch_latest_actor_state() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "devices".into(),
        }))
        .expect("track request");
    let track = next_event(&service).state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("device request");
    let added = next_event(&service);
    assert!(added.error.is_none());
    let device = added.state.project.tracks()[0].devices()[0].id();
    let gain = hex_lattice_contract::ParameterId::new(1).unwrap();
    let width = hex_lattice_contract::ParameterId::new(3).unwrap();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: gain,
            value: hex_lattice_contract::ParameterValue::Float(2.0),
        })
        .expect("gain request");
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: width,
            value: hex_lattice_contract::ParameterValue::Float(2.5),
        })
        .expect("width request");
    let gain_changed = next_event(&service);
    let width_changed = next_event(&service);
    assert!(gain_changed.error.is_none());
    assert!(width_changed.error.is_none());
    let state = width_changed
        .state
        .project
        .device(device)
        .expect("device remains")
        .state();
    assert_eq!(
        hex_engine::native_device_parameter(state, gain).unwrap(),
        hex_lattice_contract::ParameterValue::Float(2.0)
    );
    assert_eq!(
        hex_engine::native_device_parameter(state, width).unwrap(),
        hex_lattice_contract::ParameterValue::Float(2.5)
    );
    service
        .submit(Request::Undo)
        .expect("undo latest parameter");
    let undone = next_event(&service);
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(device).unwrap().state(),
            gain
        )
        .unwrap(),
        hex_lattice_contract::ParameterValue::Float(2.0)
    );
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(device).unwrap().state(),
            width
        )
        .unwrap(),
        hex_lattice_contract::ParameterValue::Float(1.0)
    );

    let revision = undone.state.revision;
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::new(999).unwrap(),
            value: hex_lattice_contract::ParameterValue::Float(0.0),
        })
        .expect("invalid parameter request");
    let rejected = next_event(&service);
    assert_eq!(
        rejected.error.as_deref(),
        Some("device parameter is unknown")
    );
    assert_eq!(rejected.state.revision, revision);
}

#[test]
fn canonical_instrument_requests_route_by_slot_and_preserve_identity_and_bypass() {
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "instrument slot".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .expect("add track");
    let added_track = next_event(&service);
    let track = added_track.state.project.tracks()[0].id();
    let instrument = added_track
        .state
        .project
        .track(track)
        .and_then(|track| track.instrument())
        .expect("default instrument slot");
    let id = instrument.id();
    let removed_id = id;
    let retained_source = added_track
        .state
        .project
        .track(track)
        .unwrap()
        .source_id()
        .expect("instrument source");
    let sine_type = instrument.state().type_id();

    service
        .submit(Request::SetDeviceParameter {
            device: id,
            parameter: ParameterId::new(1).expect("parameter id"),
            value: ParameterValue::Float(2.0),
        })
        .expect("parameter request");
    let rejected = next_event(&service);
    assert_eq!(
        rejected.error.as_deref(),
        Some("device parameter is unknown")
    );
    assert_eq!(rejected.state.revision, added_track.state.revision);
    let gesture_key = crate::device_gesture::GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(
            crate::device_gesture::GestureRequest::Begin {
                key: gesture_key,
                target: crate::device_gesture::GestureTarget::DeviceParameter {
                    device: id,
                    parameter: ParameterId::new(1).expect("parameter id"),
                },
            },
        ))
        .expect("parameter gesture request");
    let rejected_gesture = next_event(&service);
    assert!(rejected_gesture.active_device_gesture.is_none());
    assert!(
        rejected_gesture
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| !ack.accepted)
    );
    assert_eq!(rejected_gesture.state.revision, added_track.state.revision);

    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove instrument");
    let removed = next_event(&service);
    assert!(
        removed
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    let source = removed
        .state
        .project
        .native_source(retained_source)
        .expect("unbinding retains the source");
    assert_eq!(source.receiver(), None);
    assert_eq!(source.instrument().id(), removed_id);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(sine_type),
            before: None,
        })
        .expect("insert instrument through its canonical class");
    let inserted = next_event(&service);
    assert!(inserted.error.is_none());
    let id = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .expect("instrument inserted into empty slot")
        .id();
    assert_ne!(id, instrument.id(), "a removed device id is not reused");
    assert_eq!(
        inserted
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        id,
        "an instrument added to an empty slot occupies the canonical instrument position"
    );
    service
        .submit(Request::Undo)
        .expect("undo instrument insertion");
    let undone = next_event(&service);
    assert!(
        undone
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    service
        .submit(Request::Redo)
        .expect("redo instrument insertion");
    let restored = next_event(&service);
    assert_eq!(
        restored
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        id
    );

    service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device: id,
            bypassed: true,
        }))
        .expect("bypass instrument");
    let bypassed = next_event(&service);
    assert!(bypassed.state.project.device(id).unwrap().bypassed());
    let before_add = bypassed.state.clone();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(sine_type),
            before: None,
        })
        .expect("add instrument request");
    let rejected_add = next_event(&service);
    assert_eq!(
        rejected_add.error.as_deref(),
        Some("instrument slot is already occupied")
    );
    assert_eq!(rejected_add.state.revision, before_add.revision);
    assert_eq!(rejected_add.state.project, before_add.project);
    assert_eq!(rejected_add.state.can_undo, before_add.can_undo);
    assert!(rejected_add.playback.is_none());

    service
        .submit(Request::ReplaceNativeDevice {
            device: id,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(sine_type),
        })
        .expect("explicit instrument replacement request");
    let replacement = next_event(&service);
    assert!(replacement.error.is_none());
    let instrument = replacement
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    assert_eq!(
        instrument.id(),
        id,
        "replacement keeps the canonical slot id"
    );
    assert!(
        instrument.bypassed(),
        "replacement keeps the current bypass flag"
    );
    assert_eq!(replacement.state.revision, rejected_add.state.revision);
    assert!(
        replacement.playback.is_none(),
        "same-state replacement is a no-op"
    );

    let directory = TestDir::new();
    let path = directory.0.join("instrument.hex");
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save instrument slot");
    let saved = next_event(&service);
    assert!(saved.error.is_none());
    let reopened_service = Service::new(48_000, 16, 0).expect("reload service");
    next_event(&reopened_service);
    reopened_service
        .submit(Request::Open(path))
        .expect("reopen instrument project");
    let reopened = next_event(&reopened_service);
    let persisted = reopened
        .state
        .project
        .track(track)
        .and_then(|track| track.instrument())
        .expect("persisted instrument slot");
    assert_eq!(persisted.id(), id);
    assert!(persisted.bypassed());

    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(sine_type),
            before: None,
        })
        .expect("wrong class request");
    let wrong_class = next_event(&service);
    assert_eq!(
        wrong_class.error.as_deref(),
        Some("native device default state is invalid")
    );
    assert_eq!(wrong_class.state.revision, replacement.state.revision);
    assert!(wrong_class.playback.is_none());

    service
        .submit(Request::ReplaceNativeDevice {
            device: instrument.id(),
            class: NativeDeviceClass::Effect,
            type_id: Box::new(sine_type),
        })
        .expect("replacement with the wrong class");
    let wrong_replacement_class = next_event(&service);
    assert_eq!(
        wrong_replacement_class.error.as_deref(),
        Some("replacement class does not match device")
    );
    assert_eq!(
        wrong_replacement_class.state.revision,
        replacement.state.revision
    );
    assert!(wrong_replacement_class.playback.is_none());

    service
        .submit(Request::ReplaceNativeDevice {
            device: instrument.id(),
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(
                hex_lattice_contract::DeviceTypeId::new("hex.instrument.unknown").unwrap(),
            ),
        })
        .expect("replacement with an unregistered type");
    let unsupported_replacement = next_event(&service);
    assert_eq!(
        unsupported_replacement.error.as_deref(),
        Some("native instrument default state is invalid")
    );
    assert_eq!(
        unsupported_replacement.state.revision,
        replacement.state.revision
    );
    assert!(unsupported_replacement.playback.is_none());

    // The old instrument is no longer bound to the track, but its source
    // remains canonical and can still be addressed by its device ID.
    service
        .submit(Request::ReplaceNativeDevice {
            device: removed_id,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(sine_type),
        })
        .expect("replacement for the retained device");
    let retained_replacement = next_event(&service);
    assert!(retained_replacement.request_complete);
    assert!(retained_replacement.error.is_none());
    assert!(
        retained_replacement
            .state
            .project
            .device(removed_id)
            .is_some()
    );
    assert_eq!(
        retained_replacement.state.revision,
        replacement.state.revision
    );

    service
        .submit(Request::Edit(Command::RemoveNativeSource(retained_source)))
        .expect("remove the unbound source");
    let retired = next_event(&service);
    assert!(retired.request_complete);
    assert!(retired.error.is_none(), "{:?}", retired.error);
    assert!(
        retired
            .state
            .project
            .native_source(retained_source)
            .is_none()
    );
    assert!(retired.state.project.device(removed_id).is_none());
    service
        .submit(Request::ReplaceNativeDevice {
            device: removed_id,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(sine_type),
        })
        .expect("replacement for a removed device");
    let missing_replacement = next_event(&service);
    assert!(missing_replacement.request_complete);
    assert_eq!(
        missing_replacement.error.as_deref(),
        Some("device no longer exists")
    );
    assert_eq!(missing_replacement.state.revision, retired.state.revision);
    assert_eq!(missing_replacement.state.project, retired.state.project);
    assert_eq!(missing_replacement.state.can_undo, retired.state.can_undo);
    assert_eq!(missing_replacement.state.can_redo, retired.state.can_redo);
    assert!(missing_replacement.playback.is_none());
}

#[test]
fn explicit_effect_replacement_preserves_identity_order_bypass_and_undo() {
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "effect replacement".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let utility = DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(utility),
            before: None,
        })
        .expect("add utility");
    let added = next_event(&service);
    assert!(added.error.is_none());
    let device = added.state.project.track(track).unwrap().devices()[0].id();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::new(1).unwrap(),
            value: ParameterValue::Float(2.0),
        })
        .expect("change utility gain");
    let changed = next_event(&service);
    assert!(changed.error.is_none());
    service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: true,
        }))
        .expect("bypass utility");
    let bypassed = next_event(&service);
    let prior_track = bypassed.state.project.track(track).unwrap();
    let prior_order: Vec<_> = prior_track
        .devices()
        .iter()
        .map(|entry| entry.id())
        .collect();
    assert_eq!(prior_order, vec![device]);
    assert!(prior_track.devices()[0].bypassed());

    service
        .submit(Request::ReplaceNativeDevice {
            device,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(utility),
        })
        .expect("replace Utility explicitly");
    let replaced = next_event(&service);
    assert!(replaced.error.is_none());
    assert!(replaced.playback.is_some());
    let replacement_track = replaced.state.project.track(track).unwrap();
    assert_eq!(
        replacement_track
            .devices()
            .iter()
            .map(|entry| entry.id())
            .collect::<Vec<_>>(),
        prior_order
    );
    let replacement = replaced.state.project.device(device).unwrap();
    assert!(replacement.bypassed());
    assert_eq!(
        hex_engine::native_device_parameter(replacement.state(), ParameterId::new(1).unwrap())
            .unwrap(),
        ParameterValue::Float(1.0),
        "replacement restores the selected type's default parameter state"
    );

    service.submit(Request::Undo).expect("undo replacement");
    let undone = next_event(&service);
    let restored = undone.state.project.device(device).unwrap();
    assert!(restored.bypassed());
    assert_eq!(
        hex_engine::native_device_parameter(restored.state(), ParameterId::new(1).unwrap())
            .unwrap(),
        ParameterValue::Float(2.0)
    );
    assert_eq!(
        undone
            .state
            .project
            .track(track)
            .unwrap()
            .devices()
            .iter()
            .map(|entry| entry.id())
            .collect::<Vec<_>>(),
        prior_order
    );
}

#[test]
fn device_gesture_preview_is_noncanonical_and_commit_is_one_undo_step() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gesture".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("add utility");
    let before = next_event(&service);
    let device = before.state.project.tracks()[0].devices()[0].id();
    let gain = ParameterId::new(1).unwrap();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: gain,
            },
        }))
        .expect("begin gesture");
    let begun = next_event(&service);
    assert!(begun.gesture_request);
    assert!(begun.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    assert_eq!(begun.state.revision, before.state.revision);

    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.25),
        }))
        .expect("preview value");
    let preview = next_event(&service);
    assert!(preview.gesture_request);
    assert_eq!(preview.state.revision, before.state.revision);
    assert_eq!(preview.state.unsaved, before.state.unsaved);
    assert_eq!(
        hex_engine::native_device_parameter(
            preview.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(1.0)
    );
    let device_preview = preview.device_preview.as_ref().expect("preview view");
    assert_eq!(
        device_preview.identity.canonical_revision,
        before.state.revision
    );
    assert_eq!(device_preview.identity.key, key);
    assert_eq!(device_preview.identity.sequence, 1);
    assert_eq!(
        hex_engine::native_device_parameter(
            device_preview.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(2.25)
    );

    service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 2,
            value: ParameterValue::Float(3.5),
        }))
        .expect("commit final value");
    let committed = next_event(&service);
    assert!(committed.error.is_none());
    assert!(committed.state.unsaved);
    assert_eq!(committed.state.revision, before.state.revision + 1);
    assert!(committed.device_preview.is_none());
    assert_eq!(
        hex_engine::native_device_parameter(
            committed.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(3.5)
    );
    service.submit(Request::Undo).expect("undo gesture once");
    let undone = next_event(&service);
    assert_eq!(undone.state.revision, committed.state.revision + 1);
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(1.0)
    );
    service.submit(Request::Redo).expect("redo gesture once");
    let redone = next_event(&service);
    assert_eq!(
        hex_engine::native_device_parameter(
            redone.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(3.5)
    );
}

#[test]
fn operator_device_gesture_retargets_a_playing_feedback_note_and_commits_once() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_engine::playback::{PlaybackAction, PlaybackCommand};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    fn render_active(playback: &mut Playback, frames: usize) -> Vec<hex_engine::Frame> {
        let mut output = vec![[0.0; 2]; frames];
        playback
            .render(&mut output)
            .expect("active Operator render");
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|frame| frame[0].abs() > 1.0e-6));
        output
    }

    fn assert_scaled_tail(
        actual: &[hex_engine::Frame],
        reference: &[hex_engine::Frame],
        scale: f32,
    ) {
        assert_eq!(actual.len(), reference.len());
        // Operator retargets carrier weight over 240 frames at 48 kHz.
        // Inspect a later section where the target is fully applied.
        for (actual_frame, reference_frame) in actual[384..].iter().zip(&reference[384..]) {
            for channel in 0..2 {
                let expected = reference_frame[channel] * scale;
                assert!(
                    (actual_frame[channel] - expected).abs() <= 3.0e-6,
                    "carrier-scaled PCM differs: actual={}, expected={expected}",
                    actual_frame[channel]
                );
            }
        }
    }

    fn operator_parameter(
        state: &hex_lattice_contract::NativeDeviceState,
        parameter: ParameterId,
    ) -> ParameterValue {
        hex_engine::native_device_parameter_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            state,
            parameter,
        )
        .expect("read Operator parameter")
    }

    let service = Service::new(48_000, 8, 0).expect("service");
    let _initial = next_event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "operator gesture".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .expect("add track");
    let track_event = next_event(&service);
    let track = track_event.state.project.tracks()[0].id();
    let device = track_event.state.project.tracks()[0]
        .instrument()
        .expect("new hybrid track instrument")
        .id();
    service
        .submit(Request::ReplaceNativeDevice {
            device,
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: Box::new(
                hex_lattice_contract::DeviceTypeId::new("hex.instrument.operator")
                    .expect("Operator type"),
            ),
        })
        .expect("replace default instrument with Operator");
    let added = next_event(&service);
    assert!(added.error.is_none(), "{:?}", added.error);
    assert_eq!(
        added
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id()
            .as_str()
            .unwrap(),
        "hex.instrument.operator"
    );

    // Parameter 78 is Operator's authored oscillator-one feedback. A
    // nonzero value selects the mutable note law whose live handoff is
    // intentionally supported; analytic zero-feedback notes retain their
    // original immutable law during active playback.
    let feedback = ParameterId::new(78).expect("Operator feedback parameter");
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: feedback,
            value: ParameterValue::Float(0.3),
        })
        .expect("enable mutable feedback");
    let configured = next_event(&service);
    assert!(configured.error.is_none(), "{:?}", configured.error);

    let beat = |quarters| hex_project::BeatTime::from_quarters(quarters).expect("beat");
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: beat(4),
        }))
        .expect("place note clip");
    let clip_event = next_event(&service);
    let clip = clip_event.state.project.tracks()[0].midi_clips()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: beat(2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }))
        .expect("add held note");
    let before = next_event(&service);
    assert!(before.error.is_none(), "{:?}", before.error);
    let canonical = before.state.project.as_ref().clone();
    let canonical_revision = before.state.revision;
    let canonical_can_undo = before.state.can_undo;
    let original_prepared = Arc::clone(
        before
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("Operator prepared revision"),
    );
    let mut active = before.playback.expect("Operator playback");
    let mut baseline = Playback::try_from_prepared_with_session(original_prepared)
        .expect("matching active runtime");
    active
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .expect("play active Operator");
    baseline
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .expect("play unchanged baseline");
    let initial_active = render_active(&mut active, 128);
    let initial_baseline = render_active(&mut baseline, 128);
    assert_eq!(initial_active, initial_baseline);

    let carrier = ParameterId::new(29).expect("Operator oscillator-one carrier");
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: carrier,
            },
        }))
        .expect("begin carrier gesture");
    let begun = next_event(&service);
    assert!(begun.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }))
        .expect("preview carrier value");
    let preview = next_event(&service);
    assert!(preview.error.is_none(), "{:?}", preview.error);
    assert!(preview.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    assert_eq!(preview.state.revision, canonical_revision);
    assert_eq!(preview.state.can_undo, canonical_can_undo);
    assert_eq!(preview.state.project.as_ref(), &canonical);
    assert_eq!(preview.state.unsaved, before.state.unsaved);
    let overlay = preview.device_preview.as_ref().expect("prepared preview");
    assert_eq!(overlay.identity.canonical_revision, canonical_revision);
    assert_eq!(overlay.identity.sequence, 1);
    assert_eq!(
        operator_parameter(
            overlay.state.project.device(device).unwrap().state(),
            carrier,
        ),
        ParameterValue::Float(0.25)
    );
    assert_eq!(
        operator_parameter(
            preview.state.project.device(device).unwrap().state(),
            carrier,
        ),
        ParameterValue::Float(1.0)
    );

    let mut preview_playback = preview.playback.expect("preview playback");
    let preview_start = active.position();
    assert_eq!(preview_start, baseline.position());
    preview_playback
        .try_inherit_runtime_from(&mut active)
        .expect("compatible mutable Operator handoff");
    assert!(preview_playback.playing());
    assert_eq!(preview_playback.position(), preview_start);
    let changed = render_active(&mut preview_playback, 512);
    let unchanged = render_active(&mut baseline, 512);
    assert_scaled_tail(&changed, &unchanged, 0.25);
    assert_eq!(preview_playback.position(), preview_start + 512);
    assert_eq!(preview_playback.position(), baseline.position());

    service
        .submit(Request::DeviceGesture(GestureRequest::Commit {
            manual_edit: false,
            key,
            sequence: 2,
            value: ParameterValue::Float(0.6),
        }))
        .expect("commit carrier value");
    let committed = next_event(&service);
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert!(committed.device_preview.is_none());
    assert_eq!(committed.state.revision, canonical_revision + 1);
    assert_eq!(committed.state.project.tracks()[0].id(), track);
    assert!(committed.state.unsaved);
    assert_eq!(
        operator_parameter(
            committed.state.project.device(device).unwrap().state(),
            carrier
        ),
        ParameterValue::Float(0.6)
    );
    let mut committed_playback = committed.playback.expect("committed playback");
    let commit_start = preview_playback.position();
    committed_playback
        .try_inherit_runtime_from(&mut preview_playback)
        .expect("second compatible mutable Operator handoff");
    assert!(committed_playback.playing());
    assert_eq!(committed_playback.position(), commit_start);
    let committed_audio = render_active(&mut committed_playback, 512);
    let commit_baseline = render_active(&mut baseline, 512);
    assert_scaled_tail(&committed_audio, &commit_baseline, 0.6);
    assert_eq!(committed_playback.position(), commit_start + 512);
    assert_eq!(committed_playback.position(), baseline.position());

    service
        .submit(Request::Undo)
        .expect("undo one gesture edit");
    let undone = next_event(&service);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.revision, committed.state.revision + 1);
    assert_eq!(undone.state.project.as_ref(), &canonical);
    assert_eq!(
        operator_parameter(
            undone.state.project.device(device).unwrap().state(),
            carrier
        ),
        ParameterValue::Float(1.0)
    );
    let mut undo_playback = undone.playback.expect("undo playback");
    let undo_start = committed_playback.position();
    undo_playback
        .try_inherit_runtime_from(&mut committed_playback)
        .expect("undo restores compatible active runtime");
    assert!(undo_playback.playing());
    let undo_audio = render_active(&mut undo_playback, 512);
    let undo_baseline = render_active(&mut baseline, 512);
    assert_scaled_tail(&undo_audio, &undo_baseline, 1.0);
    assert_eq!(undo_playback.position(), undo_start + 512);
    assert_eq!(undo_playback.position(), baseline.position());
}

#[test]
fn undo_barrier_finishes_active_gesture_before_undoing_it() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gesture barrier".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("add utility");
    let before = next_event(&service);
    let device = before.state.project.tracks()[0].devices()[0].id();
    let gain = ParameterId::new(1).unwrap();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: gain,
            },
        }))
        .expect("begin gesture");
    next_event(&service);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.0),
        }))
        .expect("preview value");
    next_event(&service);
    service.submit(Request::Undo).expect("undo barrier");
    let undone = next_event(&service);
    assert!(undone.gesture_protocol_ack);
    assert!(!undone.gesture_request);
    assert!(undone.gesture_ack.as_ref().is_some_and(|ack| {
        ack.phase == crate::device_gesture::GesturePhase::Commit && ack.accepted
    }));
    assert_eq!(
        undone.state.revision,
        before.state.revision + 2,
        "one commit and one undo each advance canonical revision"
    );
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(1.0)
    );
}

#[test]
fn cancel_restores_latest_state_after_same_and_independent_parameter_edits() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "gesture rebase".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("add utility");
    let before = next_event(&service);
    let device = before.state.project.tracks()[0].devices()[0].id();
    let gain = ParameterId::new(1).unwrap();
    let width = ParameterId::new(3).unwrap();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: gain,
            },
        }))
        .expect("begin gesture");
    next_event(&service);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.0),
        }))
        .expect("preview gain");
    next_event(&service);
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: gain,
            value: ParameterValue::Float(2.5),
        })
        .expect("same-parameter edit");
    let same_parameter_edit = next_event(&service);
    assert!(same_parameter_edit.device_preview.is_some());
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: width,
            value: ParameterValue::Float(2.5),
        })
        .expect("independent width edit");
    let rebased = next_event(&service);
    assert!(rebased.error.is_none());
    assert_eq!(rebased.state.revision, before.state.revision + 2);
    assert_eq!(
        hex_engine::native_device_parameter(
            rebased.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(2.5)
    );
    let preview = rebased.device_preview.as_ref().expect("rebased preview");
    assert_eq!(preview.identity.canonical_revision, rebased.state.revision);
    let preview_device = preview.state.project.device(device).unwrap();
    assert_eq!(
        hex_engine::native_device_parameter(preview_device.state(), gain).unwrap(),
        ParameterValue::Float(2.0)
    );
    assert_eq!(
        hex_engine::native_device_parameter(preview_device.state(), width).unwrap(),
        ParameterValue::Float(2.5)
    );
    service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .expect("cancel gesture");
    let canceled = next_event(&service);
    assert!(canceled.device_preview.is_none());
    assert_eq!(canceled.state.revision, rebased.state.revision);
    let canonical = canceled.state.project.device(device).unwrap().state();
    assert_eq!(
        hex_engine::native_device_parameter(canonical, gain).unwrap(),
        ParameterValue::Float(2.5)
    );
    assert_eq!(
        hex_engine::native_device_parameter(canonical, width).unwrap(),
        ParameterValue::Float(2.5)
    );
    service
        .submit(Request::Undo)
        .expect("undo only independent edit");
    let undone = next_event(&service);
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(device).unwrap().state(),
            width,
        )
        .unwrap(),
        ParameterValue::Float(1.0)
    );
    service
        .submit(Request::Undo)
        .expect("undo same-parameter edit");
    let undone_gain = next_event(&service);
    assert_eq!(
        hex_engine::native_device_parameter(
            undone_gain.state.project.device(device).unwrap().state(),
            gain,
        )
        .unwrap(),
        ParameterValue::Float(1.0)
    );
}

#[test]
fn removing_gesture_target_cancels_only_its_preview_and_keeps_the_edit() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_lattice_contract::{ParameterId, ParameterValue};

    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "removed device".into(),
        }))
        .expect("add track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .expect("add utility");
    let added = next_event(&service);
    let device = added.state.project.tracks()[0].devices()[0].id();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter {
                device,
                parameter: ParameterId::new(1).unwrap(),
            },
        }))
        .expect("begin");
    next_event(&service);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.0),
        }))
        .expect("preview");
    next_event(&service);
    service
        .submit(Request::Edit(Command::RemoveDevice(device)))
        .expect("remove target");
    let removed = next_event(&service);
    assert!(removed.error.is_none());
    assert!(removed.active_device_gesture.is_none());
    assert!(removed.device_preview.is_none());
    assert!(removed.gesture_ack.as_ref().is_some_and(|ack| {
        ack.reason
            .as_deref()
            .is_some_and(|reason| reason.contains("removed or replaced"))
    }));
    assert!(removed.state.project.device(device).is_none());
    assert!(removed.state.unsaved);
}

#[test]
fn failed_device_chain_compile_preserves_candidate_revision_and_undo_state() {
    let service = Service::new(48_000, 64, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "dense devices".into(),
        }))
        .expect("track request");
    let mut state = next_event(&service).state;
    let track = state.project.tracks()[0].id();
    let type_id = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    let mut devices = Vec::new();
    for _ in 0..24 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            })
            .expect("device request");
        let event = next_event(&service);
        assert!(event.error.is_none());
        devices.push(
            event.state.project.tracks()[0]
                .devices()
                .last()
                .unwrap()
                .id(),
        );
        state = event.state;
    }
    let gain = hex_lattice_contract::ParameterId::new(1).unwrap();
    let mut last_accepted = None;
    let mut rejected_device = None;
    for device in devices.iter().copied() {
        let before_revision = state.revision;
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter: gain,
                value: hex_lattice_contract::ParameterValue::Float(56.234_13),
            })
            .expect("gain request");
        let event = next_event(&service);
        if event.error.is_some() {
            assert_eq!(event.state.revision, before_revision);
            assert_eq!(
                hex_engine::native_device_parameter(
                    event.state.project.device(device).unwrap().state(),
                    gain
                )
                .unwrap(),
                hex_lattice_contract::ParameterValue::Float(1.0)
            );
            rejected_device = Some(device);
            break;
        }
        last_accepted = Some(device);
        state = event.state;
    }
    let rejected = rejected_device.expect("serial gain chain should overflow");
    let revision = state.revision;
    service
        .submit(Request::Undo)
        .expect("undo last accepted parameter");
    let undone = next_event(&service);
    assert_eq!(undone.state.revision, revision + 1);
    let accepted = last_accepted.expect("at least one parameter accepted");
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(accepted).unwrap().state(),
            gain
        )
        .unwrap(),
        hex_lattice_contract::ParameterValue::Float(1.0)
    );
    assert_eq!(
        hex_engine::native_device_parameter(
            undone.state.project.device(rejected).unwrap().state(),
            gain
        )
        .unwrap(),
        hex_lattice_contract::ParameterValue::Float(1.0)
    );
}

#[test]
fn move_track_devices_refuses_stale_requests_and_commits_once() {
    let service = Service::new(48_000, 16, 0).unwrap();
    next_event(&service);
    let run = |request| {
        service.submit(request).unwrap();
        next_event(&service)
    };
    let add_track = |name: &str| {
        let Created::Track(track) = run(Request::Edit(Command::AddTrack { name: name.into() }))
            .created
            .unwrap()
        else {
            panic!("track")
        };
        track
    };
    let source = add_track("source");
    let destination = add_track("destination");
    let add_effect = || {
        run(Request::AddNativeDevice {
            track: source,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.utility",
            )),
            before: None,
        })
    };
    let first = add_effect();
    let original = add_effect().state;
    let source_devices = original.project.track(source).unwrap().devices();
    let selected =
        hex_project::DeviceSelectionIds::new(&[source_devices[1].id(), source_devices[0].id()])
            .unwrap();
    let request = |project_generation, revision| Request::MoveTrackDevices {
        project_generation,
        revision,
        source,
        selected: selected.clone(),
        destination,
    };
    let reorder = |project_generation, revision| Request::ReorderDevices {
        project_generation,
        revision,
        owner: hex_project::DeviceChainOwner::Track(source),
        selected: selected.clone(),
        before: Some(source_devices[1].id()),
    };
    for stale in [
        reorder(0, first.state.revision),
        reorder(1, original.revision),
    ] {
        let refused = run(stale);
        assert_eq!(
            refused.error.as_deref(),
            Some("device reorder canceled · project changed")
        );
        assert_eq!(refused.state.revision, original.revision);
        assert_eq!(refused.state.project, original.project);
    }
    let unchanged = run(reorder(0, original.revision));
    assert!(unchanged.error.is_none(), "{:?}", unchanged.error);
    assert_eq!(unchanged.state.revision, original.revision);
    assert_eq!(unchanged.state.project, original.project);
    for stale in [
        request(0, first.state.revision),
        request(1, original.revision),
    ] {
        let refused = run(stale);
        assert_eq!(
            refused.error.as_deref(),
            Some("device move canceled · project changed")
        );
        assert_eq!(refused.state.revision, original.revision);
        assert_eq!(refused.state.project, original.project);
    }
    let moved = run(request(0, original.revision));
    assert!(moved.error.is_none(), "{:?}", moved.error);
    assert_eq!(moved.state.revision, original.revision + 1);
    assert_eq!(
        moved.state.project.track(destination).unwrap().devices(),
        source_devices
    );
    assert_eq!(run(Request::Undo).state.project, original.project);
    assert_eq!(run(Request::Redo).state.project, moved.state.project);
}

#[test]
fn native_device_drops_add_move_and_reject_stale_or_occupied_targets() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "devices".into(),
        }))
        .expect("first track");
    let first = next_event(&service);
    let first_track = first.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::RemoveInstrument {
            track: first_track,
        }))
        .expect("empty the built-in instrument slot");
    let first = next_event(&service);
    let utility = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: first.state.revision,
            source: NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: utility,
            },
            track: first_track,
            before: None,
            replace: None,
            control: None,
        })
        .expect("drop utility from browser");
    let added_effect = next_event(&service);
    assert!(added_effect.error.is_none());
    assert_eq!(added_effect.status.as_deref(), Some("device added"));
    let utility_id = added_effect.state.project.tracks()[0].devices()[0].id();

    let sine = hex_lattice_contract::DeviceTypeId::new("hex.instrument.sine").unwrap();
    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: added_effect.state.revision,
            source: NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: sine,
            },
            track: first_track,
            before: Some(utility_id),
            replace: None,
            control: None,
        })
        .expect("drop sine into empty instrument slot");
    let added_instrument = next_event(&service);
    assert!(added_instrument.error.is_none());
    let instrument_id = added_instrument.state.project.tracks()[0]
        .instrument()
        .expect("sine inserted")
        .id();
    assert!(matches!(
        added_instrument.state.project.tracks()[0]
            .ordered_device_chain()
            .next(),
        Some(hex_project::TrackDeviceRef::Instrument(device)) if device.id() == instrument_id
    ));

    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: first_track,
            start: hex_project::BeatTime::ONE,
            length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
        }))
        .expect("place a clip before instrument replacement");
    let with_clip = next_event(&service);
    let original_track = with_clip
        .state
        .project
        .track(first_track)
        .expect("track with clip")
        .clone();

    let before_rejection = with_clip.state.clone();
    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: before_rejection.revision,
            source: NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: sine,
            },
            track: first_track,
            before: None,
            replace: None,
            control: None,
        })
        .expect("occupied instrument drop");
    let occupied = next_event(&service);
    assert_eq!(
        occupied.error.as_deref(),
        Some("instrument slot is already occupied")
    );
    assert_eq!(occupied.state.revision, before_rejection.revision);
    assert_eq!(occupied.state.project, before_rejection.project);
    assert_eq!(occupied.state.can_undo, before_rejection.can_undo);
    assert!(occupied.playback.is_none());

    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: before_rejection.revision,
            source: NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: sine,
            },
            track: first_track,
            before: None,
            replace: Some(utility_id),
            control: None,
        })
        .expect("invalid replacement target");
    let invalid_replace = next_event(&service);
    assert_eq!(
        invalid_replace.error.as_deref(),
        Some("instrument replacement target changed")
    );
    assert_eq!(invalid_replace.state.revision, before_rejection.revision);
    assert_eq!(invalid_replace.state.project, before_rejection.project);
    assert_eq!(invalid_replace.state.can_undo, before_rejection.can_undo);

    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: before_rejection.revision,
            source: NativeDeviceDropSource::Existing {
                device: instrument_id,
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: sine,
            },
            track: first_track,
            before: None,
            replace: Some(instrument_id),
            control: None,
        })
        .expect("existing-device drag cannot claim replacement intent");
    let invalid_source = next_event(&service);
    assert_eq!(
        invalid_source.error.as_deref(),
        Some("only a browser instrument drop can replace an instrument")
    );
    assert_eq!(invalid_source.state.revision, before_rejection.revision);
    assert_eq!(invalid_source.state.project, before_rejection.project);
    assert_eq!(invalid_source.state.can_undo, before_rejection.can_undo);

    let operator = hex_lattice_contract::DeviceTypeId::new("hex.instrument.operator").unwrap();
    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: with_clip.state.revision,
            source: NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: operator,
            },
            track: first_track,
            before: None,
            replace: Some(instrument_id),
            control: None,
        })
        .expect("replace sine in its instrument card");
    let replaced = next_event(&service);
    assert!(replaced.error.is_none(), "{:?}", replaced.error);
    assert_eq!(replaced.status.as_deref(), Some("instrument replaced"));
    let replaced_track = replaced
        .state
        .project
        .track(first_track)
        .expect("replaced track");
    assert_eq!(
        replaced_track.instrument().map(hex_project::Device::id),
        Some(instrument_id)
    );
    assert_eq!(
        replaced_track
            .instrument()
            .expect("replacement instrument")
            .state()
            .type_id(),
        operator
    );
    assert_eq!(replaced_track.devices(), original_track.devices());
    assert_eq!(replaced_track.clips(), original_track.clips());
    assert_eq!(replaced_track.midi_clips(), original_track.midi_clips());

    service
        .submit(Request::Undo)
        .expect("undo the single replacement edit");
    let undone = next_event(&service);
    let undone_track = undone
        .state
        .project
        .track(first_track)
        .expect("track after undo");
    assert_eq!(
        undone_track.instrument().map(hex_project::Device::id),
        Some(instrument_id)
    );
    assert_eq!(
        undone_track
            .instrument()
            .expect("restored instrument")
            .state()
            .type_id(),
        sine
    );
    assert_eq!(undone_track.devices(), original_track.devices());
    assert_eq!(undone_track.clips(), original_track.clips());
    assert_eq!(undone_track.midi_clips(), original_track.midi_clips());

    service
        .submit(Request::Edit(Command::AddTrack {
            name: "destination".into(),
        }))
        .expect("destination track");
    let destination = next_event(&service);
    let destination_track = destination.state.project.tracks()[1].id();
    service
        .submit(Request::Edit(Command::RemoveInstrument {
            track: destination_track,
        }))
        .expect("empty the destination slot");
    let destination = next_event(&service);
    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: destination.state.revision,
            source: NativeDeviceDropSource::Existing {
                device: instrument_id,
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: sine,
            },
            track: destination_track,
            before: None,
            replace: None,
            control: None,
        })
        .expect("move instrument to destination");
    let moved_instrument = next_event(&service);
    assert!(
        moved_instrument.error.is_none(),
        "{:?}",
        moved_instrument.error
    );
    assert_eq!(
        moved_instrument.state.project.tracks()[1]
            .instrument()
            .map(hex_project::Device::id),
        Some(instrument_id)
    );
    assert!(
        moved_instrument.state.project.tracks()[0]
            .instrument()
            .is_none()
    );

    let stable = moved_instrument.state.clone();
    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: stable.revision,
            source: NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: utility,
            },
            track: destination_track,
            before: Some(utility_id),
            replace: None,
            control: None,
        })
        .expect("anchor from another track");
    let rejected_anchor = next_event(&service);
    assert_eq!(
        rejected_anchor.error.as_deref(),
        Some("device insertion anchor no longer exists")
    );
    assert_eq!(rejected_anchor.state.revision, stable.revision);
    assert_eq!(rejected_anchor.state.project, stable.project);
    assert_eq!(rejected_anchor.state.can_undo, stable.can_undo);
    assert!(rejected_anchor.playback.is_none());

    for (generation, revision) in [(1, stable.revision), (0, stable.revision.saturating_sub(1))] {
        service
            .submit(Request::DropNativeDevice {
                project_generation: generation,
                revision,
                source: NativeDeviceDropSource::Browser {
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: utility,
                },
                track: destination_track,
                before: None,
                replace: None,
                control: None,
            })
            .expect("stale device drop");
        let rejected = next_event(&service);
        assert_eq!(
            rejected.error.as_deref(),
            Some("device drag canceled · project changed")
        );
        assert_eq!(rejected.state.revision, stable.revision);
        assert_eq!(rejected.state.project, stable.project);
        assert_eq!(rejected.state.can_undo, stable.can_undo);
        assert!(rejected.playback.is_none());
    }
}

#[test]
fn native_device_drop_refuses_while_a_parameter_gesture_is_active() {
    let service = Service::new(48_000, 16, 0).expect("service");
    next_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "devices".into(),
        }))
        .expect("track");
    let track = next_event(&service).state.project.tracks()[0].id();
    let utility = hex_lattice_contract::DeviceTypeId::new("hex.utility").unwrap();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(utility),
            before: None,
        })
        .expect("utility");
    let added = next_event(&service);
    let device = added.state.project.tracks()[0].devices()[0].id();
    let gesture = crate::device_gesture::GestureKey {
        generation: 0,
        token: 41,
    };
    service
        .submit(Request::DeviceGesture(
            crate::device_gesture::GestureRequest::Begin {
                key: gesture,
                target: crate::device_gesture::GestureTarget::DeviceParameter {
                    device,
                    parameter: hex_lattice_contract::ParameterId::new(1).unwrap(),
                },
            },
        ))
        .expect("begin parameter gesture");
    let active = next_event(&service);
    assert!(active.active_device_gesture.is_some());

    service
        .submit(Request::DropBrowserDeviceOnArrangement {
            project_generation: 0,
            revision: active.state.revision,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: utility,
            destination: ArrangementDeviceDestination::NewTrack { before_track: None },
            new_track_gain: None,
            control: None,
        })
        .expect("arrangement drop during parameter gesture");
    let refused_arrangement = next_event(&service);
    assert_eq!(
        refused_arrangement.error.as_deref(),
        Some("device edit is still applying; drag again")
    );
    assert_eq!(refused_arrangement.state.project, active.state.project);
    assert_eq!(refused_arrangement.state.revision, active.state.revision);
    assert_eq!(refused_arrangement.state.can_undo, active.state.can_undo);

    service
        .submit(Request::DropNativeDevice {
            project_generation: 0,
            revision: active.state.revision,
            source: NativeDeviceDropSource::Existing {
                device,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: utility,
            },
            track,
            before: None,
            replace: None,
            control: None,
        })
        .expect("drop during parameter gesture");
    let refused = next_event(&service);
    assert_eq!(
        refused.error.as_deref(),
        Some("device edit is still applying; drag again")
    );
    assert!(refused.active_device_gesture.is_some());
    assert_eq!(refused.state.revision, active.state.revision);
    assert_eq!(refused.state.project, active.state.project);
    assert_eq!(refused.state.can_undo, active.state.can_undo);
    assert!(refused.playback.is_none());
}

#[test]
fn whole_project_export_matches_automatic_export_range_including_effect_tail() {
    let directory = TestDir::new();
    let source = directory.0.join("tail.wav");
    fs::write(&source, wav_mono(&[16_384, -8_192])).unwrap();
    let service = Service::new(48_000, 8, 8).unwrap();
    next_event(&service);
    service.submit(Request::Import(source)).unwrap();
    let track = next_event(&service).state.project.tracks()[0].id();
    let state = hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
        "hex.reverb",
    ))
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state,
        }))
        .unwrap();
    assert!(next_event(&service).error.is_none());
    let whole = directory.0.join("whole.wav");
    service.submit(Request::Export(whole.clone())).unwrap();
    next_event(&service);
    let exported = next_export_event(&service).result.unwrap();
    let range = directory.0.join("range.wav");
    service
        .submit(Request::ExportRange {
            path: range.clone(),
            start: 0,
            frames: None,
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            publication: ExportPublication::CreateNew,
        })
        .unwrap();
    next_event(&service);
    let ranged = next_export_event(&service).result.unwrap();
    // The automatic range drains the reverb's declared tail past the 2-frame clip.
    assert!(exported.frames > 2, "{}", exported.frames);
    assert_eq!(exported.frames, ranged.frames);
    let bytes = fs::read(whole).unwrap();
    assert_eq!(bytes.len() as u64, 44 + exported.frames * 4);
    assert_eq!(bytes, fs::read(range).unwrap());
}

#[test]
fn tuning_gesture_event_keeps_preview_separate_and_commit_undoes_once() {
    use crate::tuning_gesture::{TuningGestureKey, TuningGestureRequest};

    let service = Service::new(48_000, 16, 0).expect("service");
    let initial = next_event(&service);
    let canonical = initial.state.project.tuning();
    let key = TuningGestureKey {
        generation: 0,
        token: 1,
    };
    let changed = hex_tuning::ProjectTuning::new(
        false,
        hex_tuning::Scale::default(),
        hex_tuning::Audition::new(hex_tuning::Waveform::Saw, 0.4, 15.0, 600.0).expect("audition"),
    )
    .expect("tuning");
    service
        .submit(Request::TuningGesture(TuningGestureRequest::Begin { key }))
        .expect("begin tuning gesture");
    let begun = next_event(&service);
    assert!(
        begun
            .tuning_gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted)
    );
    service
        .submit(Request::TuningGesture(TuningGestureRequest::Change {
            key,
            sequence: 1,
            tuning: changed,
        }))
        .expect("preview tuning gesture");
    let preview = next_event(&service);
    assert_eq!(preview.state.project.tuning(), canonical);
    assert_eq!(preview.state.revision, initial.state.revision);
    assert!(preview.tuning_preview.is_some());
    assert!(preview.active_tuning_gesture.is_some());
    service
        .submit(Request::TuningGesture(TuningGestureRequest::Commit {
            key,
            sequence: 2,
            tuning: changed,
        }))
        .expect("commit tuning gesture");
    let committed = next_event(&service);
    assert!(
        committed
            .tuning_gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted)
    );
    assert!(committed.tuning_preview.is_none());
    assert!(committed.active_tuning_gesture.is_none());
    assert_eq!(committed.state.project.tuning(), changed);
    assert_eq!(committed.state.revision, initial.state.revision + 1);
    service.submit(Request::Undo).expect("undo tuning gesture");
    let undone = next_event(&service);
    assert_eq!(undone.state.project.tuning(), canonical);
    assert_eq!(undone.state.revision, committed.state.revision + 1);
}
