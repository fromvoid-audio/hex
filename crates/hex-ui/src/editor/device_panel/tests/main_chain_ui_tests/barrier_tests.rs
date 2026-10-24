//! Admission and retirement controls use the actual Service and existing local FIFO.
use super::*;

fn main_window(cx: &mut TestAppContext) -> (Entity<EditorView>, VisualTestContext, DeviceId) {
    let (handle, editor) = review_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1600.), px(900.)));
    pump(&editor, &mut visual);
    focus_bus(&mut visual, EffectBusOwner::Main);
    click_bus_add(&mut visual, EffectBusOwner::Main);
    pump(&editor, &mut visual);
    let device = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.main_devices()[0].id()
    });
    (editor, visual, device)
}

fn begin(editor: &Entity<EditorView>, visual: &mut VisualTestContext, device: DeviceId) {
    editor.update(&mut *visual, |v, _| {
        v.error = None;
        let parameter = hex_lattice_contract::ParameterId::from_static(1);
        let initial = hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Effect,
            v.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
            parameter,
        )
        .unwrap();
        v.begin_device_gesture(
            GestureTarget::DeviceParameter { device, parameter },
            initial,
        )
        .unwrap();
    });
    pump_control(editor, visual);
}

fn addition(view: &EditorView) -> Request {
    EffectBusOwner::Main.insert_request(
        view.project_generation,
        view.state.as_ref().unwrap().revision,
        None,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
}

#[gpui::test]
fn effect_bus_cancel_and_actual_refused_terminal_retire_only_dependent_add(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, device) = main_window(cx);
    for cancel in [true, false] {
        let before = editor.read_with(&visual, |v, _| Arc::clone(v.state.as_ref().unwrap()));
        begin(&editor, &mut visual, device);
        editor.update(&mut visual, |v, _| {
            if cancel {
                v.cancel_device_gesture().unwrap();
            } else {
                v.change_device_gesture(ParameterValue::Float(-1.0))
                    .unwrap();
            }
        });
        click_bus_add(&mut visual, EffectBusOwner::Main);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(
                v.error.is_some(),
                "canceled/refused dependent addition must report its refusal"
            );
            let state = v.state.as_ref().unwrap();
            assert_eq!(state.project, before.project);
            assert_eq!(state.revision, before.revision);
            assert_eq!(
                (state.can_undo, state.can_redo, state.unsaved),
                (before.can_undo, before.can_redo, before.unsaved)
            );
            assert!(v.device_gesture.snapshot().is_none());
            assert!(v.device_gesture_barriers.is_empty());
        });
        // A retired proof cannot poison a later independent, explicitly selected Add.
        editor.update(&mut visual, |v, _| v.error = None);
        click_bus_add(&mut visual, EffectBusOwner::Main);
        pump(&editor, &mut visual);
        let added = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let project = &v.state.as_ref().unwrap().project;
            assert_eq!(
                project.main_devices().len(),
                before.project.main_devices().len() + 1
            );
            project.clone()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let mut expected = before.project.snapshot();
            expected.next_id = added.next_identity();
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
        });
    }
}

#[gpui::test]
fn effect_bus_old_or_unmatched_ack_cannot_rebind_add_or_consume_generic_fifo(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, device) = main_window(cx);
    begin(&editor, &mut visual, device);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    editor.update(&mut visual, |v, _| {
        v.change_device_gesture(ParameterValue::Float(0.5)).unwrap();
        assert!(v.enqueue_device_barrier(addition(v)));
        assert!(v.enqueue_device_barrier(Request::Undo));
        assert!(v.pump_device_gesture());
    });
    let event = editor.update(&mut visual, |v, _| receive_service_event(&v.service));
    let actual = event
        .gesture_ack
        .as_ref()
        .expect("actual worker terminal ACK")
        .clone();
    assert!(actual.accepted);
    assert_eq!(
        actual.phase,
        hex_session::device_gesture::GesturePhase::Commit
    );
    for variant in 0..3 {
        editor.update(&mut visual, |v, _| {
            if variant != 0 {
                assert!(v.enqueue_device_barrier(addition(v)));
            }
            let mut wrong = actual.clone();
            match variant {
                0 => wrong.key.token = wrong.key.token.checked_add(1).unwrap(),
                1 => wrong.sequence = wrong.sequence.checked_add(1).unwrap(),
                _ => wrong.phase = hex_session::device_gesture::GesturePhase::Change,
            }
            // Deliberately misdelivered ACK boundary; the real Service event is
            // retained untouched and delivered normally after these refusals.
            assert!(!v.acknowledge_device_gesture(&wrong));
            assert!(v.error.is_some());
            assert_eq!(v.device_gesture_barriers.len(), 1);
            assert!(matches!(
                v.device_gesture_barriers.front(),
                Some(Request::Undo)
            ));
            assert_eq!(v.state.as_ref().unwrap().project, before);
        });
    }
    editor.update(&mut visual, |v, _| {
        v.receive(event);
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project,
            before,
            "the retained generic Undo still follows its actual terminal"
        );
        assert!(v.device_gesture.snapshot().is_none());
        assert!(v.device_gesture_barriers.is_empty());
    });
}

#[gpui::test]
fn effect_bus_interposed_canonical_edit_refuses_add_before_terminal_release(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, device) = main_window(cx);
    begin(&editor, &mut visual, device);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    let event = editor.update(&mut visual, |v, _| {
        v.change_device_gesture(ParameterValue::Float(0.5)).unwrap();
        assert!(v.enqueue_device_barrier(addition(v)));
        assert!(v.enqueue_device_barrier(Request::Undo));
        // A canonical edit arriving through Service is not this local terminal.
        v.service
            .submit(Request::Edit(Command::MasterGain { gain: 0.75 }))
            .unwrap();
        receive_service_event(&v.service)
    });
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(&mut visual, |v, _| {
        v.receive(event);
        assert!(v.error.is_some());
        assert_eq!(v.state.as_ref().unwrap().project.master_gain(), 0.75);
        assert!(
            !v.device_gesture_barriers
                .iter()
                .chain(v.pending_requests.iter())
                .any(|request| matches!(request, Request::InsertEffectBusDevice { .. }))
        );
        assert_eq!(
            v.device_gesture_barriers
                .iter()
                .chain(v.pending_requests.iter())
                .filter(|request| matches!(request, Request::Undo))
                .count(),
            1
        );
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.main_devices(),
            before.main_devices()
        );
        assert_eq!(
            v.state.as_ref().unwrap().project.next_identity(),
            before.next_identity()
        );
        assert!(v.device_gesture.snapshot().is_none());
        assert!(v.device_gesture_barriers.is_empty());
    });
}

#[gpui::test]
fn effect_bus_project_replacement_aba_never_revives_old_destination_identity(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, device) = main_window(cx);
    begin(&editor, &mut visual, device);
    let generation = editor.read_with(&visual, |v, _| v.project_generation);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue_device_barrier(addition(v)))
    });
    for transition in 0..2 {
        let event = editor.update(&mut visual, |v, _| {
            v.service
                .submit(Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap();
            receive_service_event(&v.service)
        });
        assert!(event.project_replaced);
        editor.update(&mut visual, |v, _| {
            v.receive(event);
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.project_generation, generation + transition + 1);
            assert!(v.state.as_ref().unwrap().project.main_devices().is_empty());
            assert!(v.device_gesture_barriers.is_empty());
            assert!(v.device_gesture.snapshot().is_none());
        });
    }
    focus_bus(&mut visual, EffectBusOwner::Main);
    click_bus_add(&mut visual, EffectBusOwner::Main);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let devices = v.state.as_ref().unwrap().project.main_devices();
        assert_eq!(
            devices.len(),
            1,
            "only the new intent exists after returning to the same IDs"
        );
        assert_eq!(devices[0].id(), device);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.state.as_ref().unwrap().project.main_devices().is_empty())
    });
}

#[gpui::test]
fn effect_bus_invalid_deferred_intent_cannot_finalize_unrelated_live_control(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, device) = main_window(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddReturnTrack {
            name: "removed".into()
        })))
    });
    pump(&editor, &mut visual);
    let missing = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.return_tracks()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RemoveReturnTrack(missing))))
    });
    pump(&editor, &mut visual);
    begin(&editor, &mut visual, device);
    editor.update(&mut visual, |v, _| {
        let before = Arc::clone(v.state.as_ref().unwrap());
        let snapshot = v.device_gesture.snapshot();
        for case in 0..7 {
            let mut request = addition(v);
            let Request::InsertEffectBusDevice {
                project_generation,
                revision,
                destination,
                class,
                type_id,
                before: anchor,
            } = &mut request
            else {
                panic!("typed addition");
            };
            match case {
                0 => *project_generation = project_generation.checked_add(1).unwrap(),
                1 => *revision = revision.checked_add(1).unwrap(),
                2 => *class = NativeDeviceClass::Instrument,
                3 => *type_id = hex_lattice_contract::DeviceTypeId::from_static("missing.effect"),
                4 => *destination = hex_session::service::EffectBusDestination::Return(missing),
                5 => {
                    *destination = hex_session::service::EffectBusDestination::Return(missing);
                    *anchor = Some(device);
                }
                _ => {
                    *type_id =
                        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine")
                }
            }
            assert!(!v.enqueue_device_barrier(request));
            assert!(v.error.is_some());
            assert_eq!(v.device_gesture.snapshot(), snapshot);
            assert!(!v.device_gesture.has_pending_terminal());
            assert!(v.device_gesture_barriers.is_empty());
            assert!(v.pending_requests.is_empty());
            assert_eq!(v.worker_requests_pending, 0);
            let state = v.state.as_ref().unwrap();
            assert_eq!(state.project, before.project);
            assert_eq!(
                (
                    state.revision,
                    state.can_undo,
                    state.can_redo,
                    state.unsaved
                ),
                (
                    before.revision,
                    before.can_undo,
                    before.can_redo,
                    before.unsaved
                )
            );
        }
        v.cancel_device_gesture().unwrap();
    });
    pump(&editor, &mut visual);
    click_bus_add(&mut visual, EffectBusOwner::Main);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(v.state.as_ref().unwrap().project.main_devices().len(), 2);
    });
}
