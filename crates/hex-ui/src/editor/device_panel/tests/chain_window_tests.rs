//! Rendered windowing regression for both canonical chain owners.
use super::*;
use gpui::Focusable;

fn scroll(editor: &Entity<EditorView>, visual: &mut VisualTestContext, offset: f32) {
    editor.update(&mut visual.cx, |view, cx| {
        view.device_panel
            .chain_window
            .scroll
            .set_offset(point(px(offset), px(0.0)));
        cx.notify();
    });
    visual.run_until_parked();
}

fn selector(prefix: &str, device: DeviceId) -> &'static str {
    Box::leak(format!("{prefix}-{}", device.value()).into_boxed_str())
}

fn exercise(cx: &mut TestAppContext, is_return: bool) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, mut state, track, first, _, _) = actor_utility_fixture();
    let mut return_id = None;
    if is_return {
        service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "rail".into(),
            }))
            .unwrap();
        let event = receive_service_event(&service);
        let Some(Created::ReturnTrack(id)) = event.created else {
            panic!("return");
        };
        return_id = Some(id);
    }
    for _ in 0..16 {
        let device_state = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let request = if let Some(return_track) = return_id {
            Request::Edit(Command::InsertReturnDevice {
                return_track,
                before: None,
                state: device_state,
            })
        } else {
            Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(device_state.type_id()),
                before: None,
            }
        };
        service.submit(request).unwrap();
        state = receive_service_event(&service).state;
    }
    let ids: Vec<_> = if let Some(id) = return_id {
        state
            .project
            .return_tracks()
            .iter()
            .find(|t| t.id() == id)
            .unwrap()
            .devices()
            .iter()
            .map(|d| d.id())
            .collect()
    } else {
        state
            .project
            .track(track)
            .unwrap()
            .devices()
            .iter()
            .map(|d| d.id())
            .collect()
    };
    let first = if is_return { ids[0] } else { first };
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = return_id;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let initially_created: std::collections::HashMap<_, _> = editor.read_with(cx, |view, _| {
        view.device_panel
            .controls
            .iter()
            .map(|(key, control)| {
                let id = match control {
                    ParameterControl::Float { entity, .. } => entity.entity_id(),
                    ParameterControl::Bool { entity, .. } => entity.entity_id(),
                    ParameterControl::Choice { entity, .. } => entity.entity_id(),
                };
                (*key, id)
            })
            .collect()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.views.len(),
            ids.len() + usize::from(!is_return),
            "full chain metadata survives bounded first paint"
        );
        assert!(
            !view.device_panel.has_control_for(*ids.last().unwrap()),
            "offscreen controls are not constructed initially"
        );
        assert!(
            view.device_panel
                .views
                .get(ids.last().unwrap())
                .unwrap()
                .2
                .as_ref()
                .unwrap()
                .parameters
                .len()
                >= 12
        );
    });
    assert!(
        visual
            .debug_bounds(selector("effect-device", first))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(selector("device-spacer", *ids.last().unwrap()))
            .is_some()
    );
    let gain = hex_lattice_contract::ParameterId::from_static(1);
    let control = editor.read_with(cx, |view, _| {
        match view
            .device_panel
            .controls
            .get(&ParameterKey {
                device: first,
                parameter: gain,
            })
            .unwrap()
        {
            ParameterControl::Float { entity, .. } => entity.clone(),
            _ => panic!("gain"),
        }
    });
    // A focused control with no draft also remains in the focus hierarchy.
    visual.update(|window, cx| control.focus_handle(cx).focus(window));
    scroll(&editor, &mut visual, -1_000_000.0);
    assert!(
        visual
            .debug_bounds(selector("effect-device", first))
            .is_some()
    );
    assert!(visual.update(|window, cx| control.focus_handle(cx).is_focused(window)));
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    scroll(&editor, &mut visual, -1_000_000.0);
    assert!(
        visual
            .debug_bounds(selector("device-spacer", first))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(selector("effect-device", *ids.last().unwrap()))
            .is_some()
    );
    {
        editor.update(cx, |view, cx| {
            view.active_native_device_drag = Some(drag::NativeDeviceDrag {
                project_generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                source: drag::NativeDeviceDragSource::Existing {
                    device: first,
                    class: NativeDeviceClass::Effect,
                    type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                },
                destination: std::rc::Rc::new(std::cell::Cell::new((!is_return).then_some(
                    drag::NativeDeviceDropTarget {
                        track,
                        before: Some(ids[8]),
                        replace: None,
                    },
                ))),
                return_destination: std::rc::Rc::new(std::cell::Cell::new(return_id.map(
                    |owner| drag::ReturnDeviceDropTarget {
                        owner: EffectBusOwner::Return(owner),
                        before: Some(ids[8]),
                    },
                ))),
            });
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(selector(
                    if is_return {
                        "return-device-drop-card"
                    } else {
                        "device-drop-card"
                    },
                    first
                ))
                .is_some()
        );
        assert!(
            visual
                .debug_bounds(selector(
                    if is_return {
                        "return-device-drop-card"
                    } else {
                        "device-drop-card"
                    },
                    ids[8]
                ))
                .is_some()
        );
        editor.update(cx, |view, cx| {
            view.active_native_device_drag = None;
            cx.notify();
        });
    }
    scroll(&editor, &mut visual, -1100.0);
    assert!(
        visual
            .debug_bounds(selector("device-spacer", first))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(selector("effect-device", ids[8]))
            .is_some()
    );
    let narrow_mounted = ids
        .iter()
        .filter(|id| {
            visual
                .debug_bounds(selector("effect-device", **id))
                .is_some()
        })
        .count();
    visual.simulate_resize(size(px(1300.0), px(700.0)));
    visual.run_until_parked();
    let wide_mounted = ids
        .iter()
        .filter(|id| {
            visual
                .debug_bounds(selector("effect-device", **id))
                .is_some()
        })
        .count();
    assert!(
        wide_mounted > narrow_mounted,
        "resize must expand actual composed geometry"
    );
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.has_control_for(*ids.last().unwrap()),
            "scrolling creates the previously unmounted controls"
        );
        for (key, old_id) in &initially_created {
            let id = match view.device_panel.controls.get(key).unwrap() {
                ParameterControl::Float { entity, .. } => entity.entity_id(),
                ParameterControl::Bool { entity, .. } => entity.entity_id(),
                ParameterControl::Choice { entity, .. } => entity.entity_id(),
            };
            assert_eq!(
                id, *old_id,
                "creation of later cards must not replace existing entities"
            );
        }
    });
    scroll(&editor, &mut visual, 0.0);
    // Actual captured numeric press must keep its event handlers mounted
    // when the rail scrolls before the outside release.
    let value = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-{}-value", first.value(), gain.value()).into_boxed_str(),
        ))
        .unwrap();
    let press = point(
        value.origin.x + value.size.width / 2.0,
        value.origin.y + value.size.height / 2.0,
    );
    visual.simulate_mouse_down(press, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(press.x + px(20.0), press.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert!(control.read_with(cx, |control, _| control.is_interacting()));
    scroll(&editor, &mut visual, -1_000_000.0);
    assert!(
        visual
            .debug_bounds(selector("effect-device", first))
            .is_some()
    );
    visual.simulate_mouse_up(
        point(px(20.0), px(20.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    pump_until(&editor, cx, |view| {
        view.device_gesture.snapshot().is_none() && view.worker_requests_pending == 0
    });
    assert!(!control.read_with(cx, |control, _| control.is_interacting()));
    scroll(&editor, &mut visual, 0.0);
    enter_typed_value(&mut visual, first, gain, "invalid");
    let draft = control.read_with(cx, |control, _| control.display_text().to_owned());
    scroll(&editor, &mut visual, -1_000_000.0);
    assert!(
        visual
            .debug_bounds(selector("effect-device", first))
            .is_some()
    );
    assert_eq!(
        control.read_with(cx, |control, _| control.display_text().to_owned()),
        draft
    );
    control.update(cx, |control, cx| control.cancel(cx));
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    // A worker gesture pins its captured stable device even without focus.
    editor.update(cx, |view, _| {
        assert!(
            view.begin_device_gesture(
                GestureTarget::DeviceParameter {
                    device: first,
                    parameter: gain
                },
                ParameterValue::Float(1.0)
            )
            .is_ok()
        );
    });
    scroll(&editor, &mut visual, -1_000_000.0);
    assert!(
        visual
            .debug_bounds(selector("effect-device", first))
            .is_some()
    );
    editor.update(cx, |view, _| {
        let _ = view.cancel_device_gesture();
    });
    pump_until(&editor, cx, |view| view.device_gesture.snapshot().is_none());
    visual.run_until_parked();
    // Removing an offscreen owner retires its entity; Undo restores its ID and
    // canonical ordering without replacing surviving controls.
    let survivor = editor.read_with(cx, |view, _| {
        match view
            .device_panel
            .controls
            .get(&ParameterKey {
                device: ids[8],
                parameter: gain,
            })
            .unwrap()
        {
            ParameterControl::Float { entity, .. } => entity.entity_id(),
            _ => panic!("gain"),
        }
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveDevice(first))));
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.device(first).is_none())
    });
    visual.run_until_parked();
    assert!(!editor.read_with(cx, |view, _| view.device_panel.has_control_for(first)));
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Undo));
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.device(first).is_some())
    });
    scroll(&editor, &mut visual, 0.0);
    assert!(
        visual
            .debug_bounds(selector("effect-device", first))
            .is_some()
    );
    assert_eq!(
        editor.read_with(cx, |view, _| {
            match view
                .device_panel
                .controls
                .get(&ParameterKey {
                    device: ids[8],
                    parameter: gain,
                })
                .unwrap()
            {
                ParameterControl::Float { entity, .. } => entity.entity_id(),
                _ => panic!("gain"),
            }
        }),
        survivor
    );
}

#[gpui::test]
fn track_chain_window_preserves_focus_drafts_gestures_and_history(cx: &mut TestAppContext) {
    exercise(cx, false);
}
#[gpui::test]
fn return_chain_window_preserves_focus_drafts_gestures_and_history(cx: &mut TestAppContext) {
    exercise(cx, true);
}

#[gpui::test]
fn short_chain_window_does_not_add_scroll_extent(cx: &mut TestAppContext) {
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |view, _| view
            .device_panel
            .chain_window
            .scroll
            .max_offset()
            .width),
        px(0.0),
        "observing the viewport must not enlarge the rail"
    );
}

#[gpui::test]
fn return_chain_type_change_retires_old_identity_and_metadata(cx: &mut TestAppContext) {
    let (service, state, track, return_track, device, parameter) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_track);
        cx.notify();
    });
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let old = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey { device, parameter }] {
            ParameterControl::Float { entity, .. } => entity.entity_id(),
            _ => panic!("gain"),
        }
    });
    let type_id = hex_lattice_contract::DeviceTypeId::from_static("hex.saturator");
    let replacement = hex_engine::default_native_device_state(type_id).unwrap();
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::ReplaceDeviceState {
            device,
            state: replacement
        })))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id()
                == type_id
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.views[&device].1.type_id(), type_id);
        assert!(
            !view
                .device_panel
                .controls
                .values()
                .any(|control| match control {
                    ParameterControl::Float { entity, .. } => entity.entity_id() == old,
                    _ => false,
                })
        );
        assert!(
            !view
                .device_panel
                .pending_values
                .keys()
                .any(|key| key.device == device)
        );
    });
}
