use crate::service::{Event, Request, Service, preset::Owner};
use hex_engine::NativeDeviceClass;
use hex_project::{Command, DeviceComparisonAction as Action, DeviceId, DeviceVariant as Slot};
fn event(service: &Service) -> Event {
    // Blocks on the event channel; bounded only against hangs.
    match service.events.recv_timeout(crate::test_wait::HANG_BOUND) {
        Ok(event) => event,
        Err(error) => panic!("event: worker event absent: {error}"),
    }
}
fn owner(event: &Event, device: DeviceId, class: NativeDeviceClass) -> Owner {
    Owner {
        project_generation: 0,
        revision: event.state.revision,
        device,
        class,
        type_id: event
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    }
}
#[test]
fn typed_comparison_copy_noop_switch_undo_redo_and_stale_owner_are_atomic() {
    let service = Service::new(48_000, 32, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let initial = event(&service);
    let device = initial.state.project.tracks()[0].instrument().unwrap().id();
    let target = owner(&initial, device, NativeDeviceClass::Instrument);
    service
        .submit(Request::CompareDevice {
            owner: target,
            expected: Slot::A,
            action: Action::CopyToInactive,
        })
        .unwrap();
    let noop = event(&service);
    assert!(noop.error.is_none());
    assert_eq!(noop.state.project, initial.state.project);
    assert_eq!(noop.state.revision, initial.state.revision);
    assert!(
        noop.playback.is_none(),
        "copying identical B must not replace live sound"
    );
    service
        .submit(Request::CompareDevice {
            owner: target,
            expected: Slot::A,
            action: Action::Switch,
        })
        .unwrap();
    let switched = event(&service);
    assert!(switched.error.is_none(), "{:?}", switched.error);
    assert_eq!(
        switched
            .state
            .project
            .device(device)
            .unwrap()
            .active_variant(),
        Slot::B
    );
    assert_eq!(switched.state.revision, initial.state.revision + 1);
    let before = switched.state.project.clone();
    service
        .submit(Request::CompareDevice {
            owner: target,
            expected: Slot::A,
            action: Action::Switch,
        })
        .unwrap();
    let stale = event(&service);
    assert!(stale.error.is_some());
    assert_eq!(stale.state.project, before);
    assert_eq!(stale.state.revision, switched.state.revision);
    assert!(stale.playback.is_none());
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert_eq!(undone.state.project, initial.state.project);
    service
        .submit(Request::CompareDevice {
            owner: owner(&undone, device, NativeDeviceClass::Instrument),
            expected: Slot::A,
            action: Action::CopyToInactive,
        })
        .unwrap();
    let noop = event(&service);
    assert!(noop.error.is_none());
    assert!(noop.state.can_redo);
    assert_eq!(noop.state.revision, undone.state.revision);
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert_eq!(redone.state.project, before);
}

fn changed_state(
    class: NativeDeviceClass,
    state: &hex_lattice_contract::NativeDeviceState,
) -> Option<hex_lattice_contract::NativeDeviceState> {
    use hex_lattice_contract::{ParameterKind, ParameterValue};
    let view = hex_engine::native_device_view_for_class(class, state).unwrap();
    let mut editable = 0;
    for parameter in view.parameters.iter().filter(|parameter| parameter.enabled) {
        if [
            hex_engine::DEVICE_INPUT_GAIN.id,
            hex_engine::DEVICE_OUTPUT_GAIN.id,
        ]
        .contains(&parameter.descriptor.id)
        {
            continue;
        }
        let value = match (parameter.descriptor.kind, parameter.value) {
            (ParameterKind::Float { min, max, .. }, ParameterValue::Float(current)) => {
                let middle = min * 0.5 + max * 0.5;
                ParameterValue::Float(if current != middle {
                    middle
                } else {
                    min * 0.75 + max * 0.25
                })
            }
            (ParameterKind::Bool, ParameterValue::Bool(current)) => ParameterValue::Bool(!current),
            (ParameterKind::Choice { labels }, ParameterValue::Choice(current))
                if labels.len() > 1 =>
            {
                ParameterValue::Choice((current + 1) % u32::try_from(labels.len()).unwrap())
            }
            _ => continue,
        };
        editable += 1;
        let changed = hex_engine::patch_native_device_parameter_for_class(
            class,
            state,
            parameter.descriptor.id,
            value,
        )
        .unwrap();
        if changed.payload() != state.payload() {
            return Some(changed);
        }
    }
    assert_eq!(
        editable, 0,
        "a declared editable provider parameter must change authored bytes"
    );
    eprintln!(
        "parameterless provider still exercises A/B ownership/history/codec: {:?}",
        state.type_id()
    );
    None
}

#[test]
fn every_available_catalog_device_uses_shared_pair_on_track_and_return() {
    let mut roles = 0;
    for class in [NativeDeviceClass::Instrument, NativeDeviceClass::Effect] {
        let catalog = hex_engine::native_device_catalog_for_class(class).unwrap();
        assert!(!catalog.is_empty());
        for entry in catalog {
            for on_return in if class == NativeDeviceClass::Effect {
                &[false, true][..]
            } else {
                &[false][..]
            } {
                let service = Service::new(48_000, 32, 0).unwrap();
                event(&service);
                let state = hex_engine::default_native_device_state_for_class(class, entry.type_id)
                    .unwrap();
                let added = if *on_return {
                    service
                        .submit(Request::Edit(Command::AddReturnTrack {
                            name: "return owner".into(),
                        }))
                        .unwrap();
                    let bus = event(&service).state.project.return_tracks()[0].id();
                    service
                        .submit(Request::Edit(Command::InsertReturnDevice {
                            return_track: bus,
                            before: None,
                            state,
                        }))
                        .unwrap();
                    event(&service)
                } else {
                    service
                        .submit(Request::Edit(Command::InsertTrackWithDevice {
                            name: "track owner".into(),
                            before_track: None,
                            slot: hex_project::TrackDeviceSlot::Instrument,
                            state: hex_project::default_instrument_state()
                                .expect("fixture instrument state"),
                        }))
                        .unwrap();
                    let added = event(&service);
                    let track = added.state.project.tracks()[0].id();
                    if class == NativeDeviceClass::Instrument {
                        let device = added
                            .state
                            .project
                            .track(track)
                            .unwrap()
                            .instrument()
                            .unwrap()
                            .id();
                        service
                            .submit(Request::Edit(Command::ReplaceDeviceState { device, state }))
                            .unwrap();
                    } else {
                        service
                            .submit(Request::Edit(Command::InsertDevice {
                                track,
                                before: None,
                                state,
                            }))
                            .unwrap();
                    }
                    event(&service)
                };
                assert!(
                    added.error.is_none(),
                    "{:?} return={on_return}: {:?}",
                    entry.type_id,
                    added.error
                );
                let device = if *on_return {
                    added.state.project.return_tracks()[0].devices()[0].id()
                } else if class == NativeDeviceClass::Instrument {
                    added.state.project.tracks()[0].instrument().unwrap().id()
                } else {
                    added.state.project.tracks()[0].devices()[0].id()
                };
                let a = added.state.project.device(device).unwrap().state().clone();
                service
                    .submit(Request::CompareDevice {
                        owner: owner(&added, device, class),
                        expected: Slot::A,
                        action: Action::Switch,
                    })
                    .unwrap();
                let switched = event(&service);
                assert!(switched.error.is_none(), "{:?}", switched.error);
                let b = changed_state(class, &a).unwrap_or_else(|| a.clone());
                service
                    .submit(Request::Edit(Command::ReplaceDeviceState {
                        device,
                        state: b.clone(),
                    }))
                    .unwrap();
                let edited = event(&service);
                assert!(edited.error.is_none(), "{:?}", edited.error);
                let pair = edited.state.project.device(device).unwrap();
                assert_eq!(pair.state(), &b);
                assert_eq!(pair.comparison().unwrap().inactive(), &a);
                service
                    .submit(Request::CompareDevice {
                        owner: owner(&edited, device, class),
                        expected: Slot::B,
                        action: Action::Switch,
                    })
                    .unwrap();
                let back = event(&service);
                assert!(back.error.is_none(), "{:?}", back.error);
                assert_eq!(back.state.project.device(device).unwrap().state(), &a);
                assert_eq!(
                    back.state
                        .project
                        .device(device)
                        .unwrap()
                        .comparison()
                        .unwrap()
                        .inactive(),
                    &b
                );
                let before_copy = back.state.project.clone();
                service
                    .submit(Request::CompareDevice {
                        owner: owner(&back, device, class),
                        expected: Slot::A,
                        action: Action::CopyToInactive,
                    })
                    .unwrap();
                let copied = event(&service);
                assert!(copied.error.is_none(), "{:?}", copied.error);
                let pair = copied.state.project.device(device).unwrap();
                assert_eq!(pair.active_variant(), Slot::A);
                assert_eq!(pair.state(), &a);
                assert_eq!(pair.comparison().unwrap().inactive(), &a);
                service.submit(Request::Undo).unwrap();
                let undone = event(&service);
                if a == b {
                    assert_eq!(
                        copied.state.revision, back.state.revision,
                        "parameterless identical copy is a no-op"
                    );
                    assert_eq!(
                        undone.state.project, edited.state.project,
                        "one Undo reverses the preceding switch, not a phantom copy"
                    );
                } else {
                    assert_eq!(undone.state.project, before_copy);
                }
                service.submit(Request::Redo).unwrap();
                let redone = event(&service);
                assert_eq!(redone.state.project, copied.state.project);
                let bytes = hex_storage::encode(&redone.state.project).unwrap();
                assert_eq!(hex_storage::decode(&bytes).unwrap(), *redone.state.project);
                roles += 1;
            }
        }
    }
    eprintln!("actual available registered device-role cases: {roles}");
}

#[test]
fn stale_comparison_does_not_finalize_an_unrelated_live_draft() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    use hex_lattice_contract::ParameterValue;
    let service = Service::new(48_000, 32, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::InsertTrackWithDevice {
            name: "draft owner".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let initial = event(&service);
    let track = initial.state.project.tracks()[0].id();
    let device = initial.state.project.tracks()[0].instrument().unwrap().id();
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }))
        .unwrap();
    assert!(event(&service).gesture_ack.unwrap().accepted);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }))
        .unwrap();
    let preview = event(&service);
    assert!(preview.gesture_ack.as_ref().unwrap().accepted);
    let mut stale = owner(&initial, device, NativeDeviceClass::Instrument);
    stale.project_generation += 1;
    service
        .submit(Request::CompareDevice {
            owner: stale,
            expected: Slot::A,
            action: Action::Switch,
        })
        .unwrap();
    let refused = event(&service);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, initial.state.project);
    assert_eq!(refused.state.revision, initial.state.revision);
    assert!(
        refused.gesture_ack.is_none(),
        "stale request must not commit the draft as a side effect"
    );
    service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .unwrap();
    let cancelled = event(&service);
    assert!(cancelled.gesture_ack.unwrap().accepted);
    assert_eq!(cancelled.state.project, initial.state.project);
    assert_eq!(cancelled.state.revision, initial.state.revision);
}

#[test]
fn review_2845_public_reserved_edit_cannot_retarget_an_identical_slot_switch() {
    let service = Service::new(48_000, 32, 0).unwrap();
    event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "reserved".into(),
        }))
        .unwrap();
    let initial = event(&service);
    let track = initial.state.project.tracks()[0].id();
    let utility = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: utility,
        }))
        .unwrap();
    let initial = event(&service);
    let device = initial.state.project.track(track).unwrap().devices()[0].id();
    let entry = initial.state.project.device(device).unwrap();
    let command = Command::ReplaceDeviceStateWithIdentityReservation {
        device,
        expected_state: entry.state().clone(),
        expected_variant: entry.active_variant(),
        state: changed_state(NativeDeviceClass::Effect, entry.state()).unwrap(),
        expected_next_id: initial.state.project.next_identity(),
        reserved_ids: 0,
    };
    service
        .submit(Request::CompareDevice {
            owner: owner(&initial, device, NativeDeviceClass::Effect),
            expected: Slot::A,
            action: Action::Switch,
        })
        .unwrap();
    let switched = event(&service);
    assert!(switched.error.is_none());
    service.submit(Request::Edit(command)).unwrap();
    let refused = event(&service);
    assert!(refused.error.is_some(), "deferred A edit must refuse on B");
    assert_eq!(refused.state.project, switched.state.project);
    assert_eq!(refused.state.revision, switched.state.revision);
    assert!(refused.playback.is_none());
    service.submit(Request::Undo).unwrap();
    assert_eq!(event(&service).state.project, initial.state.project);
    service.submit(Request::Redo).unwrap();
    assert_eq!(event(&service).state.project, switched.state.project);
}

#[path = "metadata_tests.rs"]
mod metadata_tests;
