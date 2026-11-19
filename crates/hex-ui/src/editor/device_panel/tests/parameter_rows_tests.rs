use super::*;
use gpui::Focusable;

use super::super::parameter_rows::tests::parameters;

fn scroll_rows(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    device: DeviceId,
    offset: f32,
) {
    editor.update(&mut visual.cx, |view, cx| {
        view.device_panel
            .parameter_rows
            .get(&device)
            .unwrap()
            .scroll
            .set_offset(point(px(0.0), px(offset)));
        cx.notify();
    });
    visual.run_until_parked();
}

fn row_selector(device: DeviceId, row: usize) -> &'static str {
    Box::leak(format!("device-row-{}-{row}", device.value()).into_boxed_str())
}

fn exercise(
    cx: &mut TestAppContext,
    class: NativeDeviceClass,
    is_return: bool,
    capture_only: bool,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, device, return_id) = if is_return {
        let (service, state, track, owner, device, _) = actor_return_utility_fixture();
        (service, state, track, device, Some(owner))
    } else {
        let (service, state, track, effect, _, _) = actor_utility_fixture();
        let device = if class == NativeDeviceClass::Instrument {
            state
                .project
                .track(track)
                .unwrap()
                .ordered_device_chain()
                .find_map(|node| match node {
                    hex_project::TrackDeviceRef::Instrument(device) => Some(device.id()),
                    _ => None,
                })
                .unwrap()
        } else {
            effect
        };
        (service, state, track, device, None)
    };
    let fixture_type = hex_lattice_contract::DeviceTypeId::from_static("hex.test.rows");
    let fixture_state = NativeDeviceState::new(fixture_type, 1, Vec::new()).unwrap();
    let mut model = Editor::new((*state.project).clone(), 16).unwrap();
    model
        .execute(Command::ReplaceDeviceState {
            device,
            state: fixture_state.clone(),
        })
        .unwrap();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        let old = view.state.as_ref().unwrap();
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            audio_clip_waveforms: old.audio_clip_waveforms.clone(),
            project: Arc::new(model.project().clone()),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            midi_preview: old.midi_preview.clone(),
            gain_preview: old.gain_preview.clone(),
            timeline_frames: old.timeline_frames,
            can_undo: false,
            can_redo: false,
            revision: old.revision,
            unsaved: false,
            document_path: None,
        }));
        view.focused_return = return_id;
        view.device_panel.owner =
            Some(return_id.map_or(DevicePanelOwner::Track(track), DevicePanelOwner::Return));
        view.device_panel.views.insert(
            device,
            (
                class,
                fixture_state,
                Ok(NativeDeviceView {
                    type_id: fixture_type,
                    name: "mixed row fixture".into(),
                    parameters: parameters(),
                }),
            ),
        );
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let initial = editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.views[&device]
                .2
                .as_ref()
                .unwrap()
                .parameters
                .len(),
            72
        );
        let count = view
            .device_panel
            .controls
            .keys()
            .filter(|key| key.device == device)
            .count();
        assert!(count > 0 && count < 72, "initial controls={count}");
        assert!(!view.device_panel.controls.contains_key(&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(72)
        }));
        view.device_panel.parameter_rows[&device]
            .scroll
            .max_offset()
            .height
    });
    assert!(
        !editor.read_with(cx, |view, _| view.device_panel.controls.contains_key(
            &ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(10)
            }
        )),
        "unsupported descriptor is retained as a placeholder without a control"
    );
    // 36 two-column rows: 24 float rows, 12 discrete rows, 35 gaps and padding.
    assert_eq!(
        initial,
        px(24.0 * 26.0 + 12.0 * 22.0 + 35.0 * 2.0 + 6.0 - DEVICE_FRAME_BODY_HEIGHT)
    );
    assert!(visual.debug_bounds(row_selector(device, 0)).is_some());
    assert!(visual.debug_bounds(row_selector(device, 35)).is_none());
    let key = ParameterKey {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(1),
    };
    let numeric = editor.read_with(cx, |view, _| match &view.device_panel.controls[&key] {
        ParameterControl::Float { entity, .. } => entity.clone(),
        _ => panic!("float"),
    });
    visual.update(|window, cx| numeric.focus_handle(cx).focus(window));
    scroll_rows(&editor, &mut visual, device, -1_000_000.0);
    assert!(
        visual.debug_bounds(row_selector(device, 0)).is_some(),
        "focused row remains mounted"
    );
    assert!(visual.debug_bounds(row_selector(device, 35)).is_some());
    assert_eq!(
        editor.read_with(cx, |view, _| match &view.device_panel.controls[&key] {
            ParameterControl::Float { entity, .. } => entity.entity_id(),
            _ => panic!("float"),
        }),
        numeric.entity_id()
    );
    scroll_rows(&editor, &mut visual, device, 0.0);
    let value_selector =
        Box::leak(format!("device-{}-parameter-1-value", device.value()).into_boxed_str());
    let value = visual.debug_bounds(value_selector).unwrap();
    visual.simulate_mouse_down(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("unfinished");
    let draft = numeric.read_with(cx, |control, _| {
        (control.is_editing(), control.display_text().to_owned())
    });
    assert!(draft.0);
    scroll_rows(&editor, &mut visual, device, -1_000_000.0);
    assert_eq!(
        numeric.read_with(cx, |control, _| (
            control.is_editing(),
            control.display_text().to_owned()
        )),
        draft
    );
    assert!(visual.debug_bounds(row_selector(device, 0)).is_some());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!numeric.read_with(cx, |control, _| control.is_editing()));
    if capture_only {
        scroll_rows(&editor, &mut visual, device, 0.0);
        let value = visual.debug_bounds(value_selector).unwrap();
        let press = value.center();
        visual.simulate_mouse_down(press, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(press.x + px(20.0), press.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert!(numeric.read_with(cx, |control, _| control.is_interacting()));
        scroll_rows(&editor, &mut visual, device, -1_000_000.0);
        assert!(visual.debug_bounds(row_selector(device, 0)).is_some());
        visual.simulate_mouse_up(
            point(px(20.0), px(20.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert!(
            !numeric.read_with(cx, |control, _| control.is_interacting()),
            "outside release reaches mounted editor"
        );
        return;
    }
    // Pending optimistic values and both distinct paired-gesture IDs pin rows
    // even when the user is scrolling a different part of the body.
    let first = hex_lattice_contract::ParameterId::from_static(19);
    let second = hex_lattice_contract::ParameterId::from_static(43);
    editor.update(cx, |view, cx| {
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: first,
            },
            ParameterValue::Float(0.5),
        );
        view.device_gesture
            .begin_pair(
                view.project_generation,
                device,
                first,
                second,
                hex_session::device_gesture::ParameterPairValue {
                    first: ParameterValue::Float(0.5),
                    second: ParameterValue::Float(0.5),
                },
            )
            .unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    for row in [9, 21] {
        assert!(
            visual.debug_bounds(row_selector(device, row)).is_some(),
            "paired row {row}"
        );
    }
    // Current main also owns whole-device snapshot gestures (for example
    // harmonic edits). Keep their owner mounted until the gesture retires.
    editor.update(cx, |view, cx| {
        view.device_gesture.reset();
        view.device_gesture
            .begin_snapshot(
                view.project_generation,
                device,
                hex_lattice_contract::DeviceParameterSnapshot::new(&[
                    (first, ParameterValue::Float(0.5)),
                    (second, ParameterValue::Float(0.5)),
                ])
                .unwrap(),
            )
            .unwrap();
        cx.notify();
    });
    visual.run_until_parked();
    for row in [0, 9, 21, 35] {
        assert!(
            visual.debug_bounds(row_selector(device, row)).is_some(),
            "snapshot row {row}"
        );
    }
    editor.update(cx, |view, _| view.device_gesture.reset());
    // Selector dropdown state pins independently of keyboard focus.
    scroll_rows(&editor, &mut visual, device, 0.0);
    let choice_key = ParameterKey {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(4),
    };
    let choice = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&choice_key] {
            ParameterControl::Choice { entity, .. } => entity.clone(),
            _ => panic!("choice"),
        }
    });
    let selector = Box::leak(format!("device-{}-parameter-4", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).expect("choice selector");
    // The right side of this wide selector is clipped at the compact window edge.
    let press = point(bounds.left() + px(20.0), bounds.center().y);
    visual.simulate_mouse_down(press, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(press, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        choice.read_with(cx, |control, _| control.is_open()),
        "actual popup opened"
    );
    scroll_rows(&editor, &mut visual, device, -1_000_000.0);
    assert!(
        visual.debug_bounds(row_selector(device, 1)).is_some(),
        "open selector mounted"
    );
    assert!(choice.read_with(cx, |control, _| control.is_open()));
    // State-aware metadata owns descriptors. A changed range for an existing
    // ID retires its cached control and optimistic value before remounting,
    // while unrelated controls retain identity.
    editor.update(cx, |view, cx| {
        let changed_key = ParameterKey {
            device,
            parameter: second,
        };
        assert!(view.device_panel.controls.contains_key(&changed_key));
        view.device_panel
            .pending_values
            .insert(changed_key, ParameterValue::Float(0.75));
        let metadata = view
            .device_panel
            .views
            .get_mut(&device)
            .unwrap()
            .2
            .as_mut()
            .unwrap();
        metadata
            .parameters
            .iter_mut()
            .find(|p| p.descriptor.id == second)
            .unwrap()
            .descriptor
            .kind = ParameterKind::Float {
            min: 0.0,
            max: 2.0,
            unit: None,
        };
        super::super::parameter_rows::reconcile(view, cx);
        assert!(!view.device_panel.controls.contains_key(&changed_key));
        assert!(!view.device_panel.pending_values.contains_key(&changed_key));
    });
    // Metadata-only replacement invalidates an offscreen descriptor without
    // waiting for that row to mount again.
    editor.update(cx, |view, cx| {
        let metadata = view
            .device_panel
            .views
            .get_mut(&device)
            .unwrap()
            .2
            .as_mut()
            .unwrap();
        metadata.parameters.retain(|p| p.descriptor.id != first);
        super::super::parameter_rows::reconcile(view, cx);
        assert!(!view.device_panel.controls.contains_key(&ParameterKey {
            device,
            parameter: first
        }));
        assert!(
            !view
                .device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: first
                })
        );
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_resize(size(px(1100.0), px(800.0)));
    visual.run_until_parked();
    // Keep all successfully created entities when returning to the middle.
    scroll_rows(&editor, &mut visual, device, -400.0);
    assert_eq!(
        numeric.entity_id(),
        editor.read_with(cx, |view, _| match &view.device_panel.controls[&key] {
            ParameterControl::Float { entity, .. } => entity.entity_id(),
            _ => panic!("float"),
        })
    );
    assert_eq!(
        choice.entity_id(),
        editor.read_with(cx, |view, _| {
            match &view.device_panel.controls[&choice_key] {
                ParameterControl::Choice { entity, .. } => entity.entity_id(),
                _ => panic!("choice"),
            }
        })
    );
    assert!(
        visual.debug_bounds(row_selector(device, 18)).is_some(),
        "middle visible range composed"
    );
    // Owner retirement clears the row scroll state and all controls.
    editor.update(cx, |view, cx| {
        view.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(!editor.read_with(cx, |view, _| {
        view.device_panel.parameter_rows.contains_key(&device)
    }));
}

#[gpui::test]
fn generic_track_effect_rows_preserve_extent_identity_and_pins(cx: &mut TestAppContext) {
    exercise(cx, NativeDeviceClass::Effect, false, false);
}
#[gpui::test]
fn generic_return_effect_rows_preserve_extent_identity_and_pins(cx: &mut TestAppContext) {
    exercise(cx, NativeDeviceClass::Effect, true, false);
}
#[gpui::test]
fn generic_instrument_rows_preserve_extent_identity_and_pins(cx: &mut TestAppContext) {
    exercise(cx, NativeDeviceClass::Instrument, false, false);
}

#[gpui::test]
fn generic_rows_deliver_captured_outside_release(cx: &mut TestAppContext) {
    exercise(cx, NativeDeviceClass::Effect, false, true);
}

#[gpui::test]
fn generic_fm4_windowed_edit_and_removal_follow_worker_undo_redo(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .unwrap();
    receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.fm4",
            )),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let device = state
        .project
        .track(track)
        .unwrap()
        .ordered_device_chain()
        .find_map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(device) => Some(device.id()),
            _ => None,
        })
        .unwrap();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let key = ParameterKey {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(1),
    };
    let numeric = editor.read_with(cx, |view, _| match &view.device_panel.controls[&key] {
        ParameterControl::Float { entity, .. } => entity.clone(),
        _ => panic!("ratio"),
    });
    let bounds = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-value", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("2");
    scroll_rows(&editor, &mut visual, device, -1_000_000.0);
    assert!(numeric.read_with(cx, |control, _| control.is_editing()));
    assert!(visual.debug_bounds(row_selector(device, 0)).is_some());
    visual.simulate_keystrokes("enter");
    let value = |view: &EditorView| {
        let project = &view.state.as_ref().unwrap().project;
        let instrument = project
            .track(track)
            .unwrap()
            .ordered_device_chain()
            .find_map(|node| match node {
                hex_project::TrackDeviceRef::Instrument(device) => Some(device),
                _ => None,
            })
            .unwrap();
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            instrument.state(),
            key.parameter,
        )
        .unwrap()
    };
    pump_until(&editor, cx, |view| {
        value(view) == ParameterValue::Float(2.0) && view.worker_requests_pending == 0
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        value(view) == ParameterValue::Float(1.0) && view.worker_requests_pending == 0
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        value(view) == ParameterValue::Float(2.0) && view.worker_requests_pending == 0
    });
    visual.run_until_parked();
    assert_eq!(
        numeric.entity_id(),
        editor.read_with(cx, |view, _| match &view.device_panel.controls[&key] {
            ParameterControl::Float { entity, .. } => entity.entity_id(),
            _ => panic!("ratio"),
        })
    );
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveInstrument { track })))
    });
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .ordered_device_chain()
            .all(|node| !matches!(node, hex_project::TrackDeviceRef::Instrument(_)))
            && view.worker_requests_pending == 0
    });
    visual.run_until_parked();
    assert!(!editor.read_with(cx, |view, _| {
        view.device_panel.parameter_rows.contains_key(&device)
    }));
    assert!(!editor.read_with(cx, |view, _| view.device_panel.controls.contains_key(&key)));
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .ordered_device_chain()
            .any(|node| matches!(node, hex_project::TrackDeviceRef::Instrument(_)))
            && view.worker_requests_pending == 0
    });
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |view, _| value(view)),
        ParameterValue::Float(2.0)
    );
    assert!(editor.read_with(cx, |view, _| {
        view.device_panel.parameter_rows.contains_key(&device)
    }));
    assert_ne!(
        numeric.entity_id(),
        editor.read_with(cx, |view, _| match &view.device_panel.controls[&key] {
            ParameterControl::Float { entity, .. } => entity.entity_id(),
            _ => panic!("ratio"),
        }),
        "actual removal retires entities; undo reconstructs"
    );
}
