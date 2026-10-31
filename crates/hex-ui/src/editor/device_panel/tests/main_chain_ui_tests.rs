//! Actual Main header/add-menu input and Service ownership.
mod barrier_tests;
use super::*;
use crate::editor::test_environment::Environment;

#[gpui::test]
fn main_and_return_catalog_menu_and_browser_drop_reach_worker_with_one_history_step(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1600.), px(900.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "empty".into()
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
            name: "return".into()
        })));
    });
    pump(&editor, &mut visual);
    let return_id = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.return_tracks()[0].id()
    });
    for owner in [EffectBusOwner::Main, EffectBusOwner::Return(return_id)] {
        // Return detail focus preserves ordinary-track selection; Main clears it.
        let expected_track = if owner == EffectBusOwner::Main {
            None
        } else {
            let track = editor.read_with(&visual, |view, _| {
                view.state.as_ref().unwrap().project.tracks()[0].id()
            });
            let selector = format!("track-name-{}", track.value());
            let name = visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .unwrap()
                .center();
            visual.simulate_click(name, Modifiers::default());
            visual.run_until_parked();
            Some(track)
        };
        let focus_selector = match owner {
            EffectBusOwner::Main => "main-track-header".to_owned(),
            EffectBusOwner::Return(id) => format!("return-track-focus-{}", id.value()),
        };
        let bounds = visual
            .debug_bounds(Box::leak(focus_selector.into_boxed_str()))
            .unwrap();
        let focus_point = if owner == EffectBusOwner::Main {
            point(bounds.left() + px(14.), bounds.center().y)
        } else {
            bounds.center()
        };
        visual.simulate_click(focus_point, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_track, expected_track, "{owner:?} focus");
        });
        let before_menu = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.clone()
        });
        let toggle = visual.debug_bounds("device-add-toggle").unwrap().center();
        visual.simulate_click(toggle, Modifiers::default());
        visual.run_until_parked();
        let selector = format!("add-{}-effect-hex.utility", owner.prefix());
        let add = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap()
            .center();
        visual.simulate_click(add, Modifiers::default());
        pump(&editor, &mut visual);
        let menu_project = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let state = view.state.as_ref().unwrap();
            assert_eq!(owner.devices(&state.project).unwrap().len(), 1);
            assert_eq!(
                owner.devices(&state.project).unwrap()[0].state(),
                &hex_engine::default_native_device_state_for_class(
                    NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                )
                .unwrap()
            );
            assert_eq!(view.selected_track, expected_track, "{owner:?} menu ACK");
            assert_eq!(
                view.focused_return,
                if owner == EffectBusOwner::Main {
                    None
                } else {
                    Some(return_id)
                }
            );
            assert!(
                state.project.tracks()[0]
                    .ordered_device_chain()
                    .next()
                    .is_none()
            );
            state.project.clone()
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let mut expected = before_menu.snapshot();
            expected.next_id = menu_project.next_identity();
            assert!(expected.next_id > before_menu.next_identity());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), expected)
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, menu_project)
        });
        let devices = visual
            .debug_bounds("browser-section-devices")
            .unwrap()
            .center();
        visual.simulate_click(devices, Modifiers::default());
        visual.run_until_parked();
        let source = visual
            .debug_bounds("browser-device-effect-hex.utility")
            .unwrap()
            .center();
        visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            source + point(px(12.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let target = visual
            .debug_bounds(Box::leak(owner.end_selector().into_boxed_str()))
            .unwrap()
            .center();
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project,
                menu_project,
                "held drop does not author"
            );
            assert!(view.active_native_device_drag.is_some());
        });
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        let dropped = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(view.active_native_device_drag.is_none());
            let state = view.state.as_ref().unwrap();
            assert_eq!(owner.devices(&state.project).unwrap().len(), 2);
            assert_eq!(view.selected_track, expected_track, "{owner:?} drop ACK");
            assert_eq!(
                view.focused_return,
                if owner == EffectBusOwner::Main {
                    None
                } else {
                    Some(return_id)
                }
            );
            assert!(
                state.project.tracks()[0]
                    .ordered_device_chain()
                    .next()
                    .is_none()
            );
            state.project.clone()
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let mut expected = menu_project.snapshot();
            expected.next_id = dropped.next_identity();
            assert!(expected.next_id > menu_project.next_identity());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), expected)
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, dropped)
        });
    }
}

fn review_window(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    (handle, editor)
}

fn focus_bus(visual: &mut VisualTestContext, owner: EffectBusOwner) {
    let selector = match owner {
        EffectBusOwner::Main => "main-track-header".to_owned(),
        EffectBusOwner::Return(id) => format!("return-track-focus-{}", id.value()),
    };
    let bounds = visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap();
    let target = if owner == EffectBusOwner::Main {
        point(bounds.left() + px(14.), bounds.center().y)
    } else {
        bounds.center()
    };
    visual.simulate_click(target, Modifiers::default());
    visual.run_until_parked();
}

fn click_bus_add(visual: &mut VisualTestContext, owner: EffectBusOwner) {
    let toggle = visual.debug_bounds("device-add-toggle").unwrap().center();
    visual.simulate_click(toggle, Modifiers::default());
    visual.run_until_parked();
    let selector = format!("add-{}-effect-hex.utility", owner.prefix());
    let item = visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap()
        .center();
    visual.simulate_click(item, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn main_and_return_add_after_changed_or_noop_gesture_preserves_two_history_steps(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = review_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1600.), px(900.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
            name: "return".into()
        })))
    });
    pump(&editor, &mut visual);
    let bus = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.return_tracks()[0].id()
    });
    for owner in [EffectBusOwner::Main, EffectBusOwner::Return(bus)] {
        focus_bus(&mut visual, owner);
        click_bus_add(&mut visual, owner);
        pump(&editor, &mut visual);
        let device = editor.read_with(&visual, |v, _| {
            owner.devices(&v.state.as_ref().unwrap().project).unwrap()[0].id()
        });
        for changed in [true, false] {
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
            let parameter = hex_lattice_contract::ParameterId::from_static(1);
            let initial = hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                before.device(device).unwrap().state(),
                parameter,
            )
            .unwrap();
            let value = if changed {
                ParameterValue::Float(0.5)
            } else {
                initial
            };
            editor.update(&mut visual, |v, _| {
                v.begin_device_gesture(
                    GestureTarget::DeviceParameter { device, parameter },
                    initial,
                )
                .unwrap();
                v.change_device_gesture(value).unwrap();
            });
            pump_control(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert!(v.device_gesture.snapshot().is_some());
                assert!(v.error.is_none(), "preview refused: {:?}", v.error);
                assert_eq!(
                    v.state.as_ref().unwrap().project,
                    before,
                    "preview remains transient"
                );
            });
            click_bus_add(&mut visual, owner);
            pump(&editor, &mut visual);
            let added = editor.read_with(&visual, |v, _| {
                assert!(
                    v.error.is_none(),
                    "{owner:?} changed={changed}: {:?}",
                    v.error
                );
                assert!(v.device_gesture.snapshot().is_none());
                assert!(v.device_gesture_barriers.is_empty());
                let project = &v.state.as_ref().unwrap().project;
                assert_eq!(
                    owner.devices(project).unwrap().len(),
                    owner.devices(&before).unwrap().len() + 1
                );
                assert_eq!(
                    hex_engine::native_device_parameter_for_class(
                        NativeDeviceClass::Effect,
                        project.device(device).unwrap().state(),
                        parameter
                    )
                    .unwrap(),
                    value
                );
                project.clone()
            });
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                let project = &v.state.as_ref().unwrap().project;
                assert_eq!(
                    owner.devices(project).unwrap().len(),
                    owner.devices(&before).unwrap().len()
                );
                assert_eq!(
                    hex_engine::native_device_parameter_for_class(
                        NativeDeviceClass::Effect,
                        project.device(device).unwrap().state(),
                        parameter
                    )
                    .unwrap(),
                    value
                );
            });
            if changed {
                editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| {
                    let mut expected = before.snapshot();
                    expected.next_id = added.next_identity();
                    assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
                });
                editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
                pump(&editor, &mut visual);
            }
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project, added)
            });
        }
    }
}

#[gpui::test]
fn main_utility_width_popup_dispatches_actual_mode_action_and_history(cx: &mut TestAppContext) {
    let (handle, editor) = review_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1600.), px(900.)));
    pump(&editor, &mut visual);
    focus_bus(&mut visual, EffectBusOwner::Main);
    click_bus_add(&mut visual, EffectBusOwner::Main);
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    let device = before.main_devices()[0].id();
    let selector = format!("utility-width-dial-{}", device.value());
    let point = visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap()
        .center();
    visual.simulate_mouse_down(point, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let item = visual
        .debug_bounds("context-menu-item")
        .expect("Main Utility mode popup remains available")
        .center();
    visual.simulate_click(item, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                project.device(device).unwrap().state(),
                hex_lattice_contract::ParameterId::from_static(9)
            )
            .unwrap(),
            ParameterValue::Bool(true)
        );
        project.clone()
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, after)
    });
    for wrong_type in [true, false] {
        let selector = format!("utility-width-dial-{}", device.value());
        let target = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap()
            .center();
        visual.simulate_mouse_down(target, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("context-menu-item").is_some());
        editor.update(&mut visual, |v, _| {
            let command = if wrong_type {
                Command::ReplaceDeviceState {
                    device,
                    state: hex_engine::default_native_device_state_for_class(
                        NativeDeviceClass::Effect,
                        hex_lattice_contract::DeviceTypeId::from_static("hex.saturator"),
                    )
                    .unwrap(),
                }
            } else {
                Command::RemoveDevice(device)
            };
            assert!(v.enqueue(Request::Edit(command)));
        });
        pump(&editor, &mut visual);
        assert!(
            visual.debug_bounds("context-menu-item").is_none(),
            "stale Utility popup retires on type/removal"
        );
        editor.read_with(&visual, |v, _| {
            assert!(v.context_menu.is_none());
            let project = &v.state.as_ref().unwrap().project;
            assert_eq!(project.next_identity(), after.next_identity());
            if wrong_type {
                assert_eq!(
                    project.device(device).unwrap().state().type_id().as_str(),
                    Ok("hex.saturator")
                );
            } else {
                assert!(project.device(device).is_none());
            }
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, after)
        });
    }
}

fn pump_control(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let end = crate::test_wait::hang_deadline();
    let mut idle = 0;
    while idle < 2 {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        visual.run_until_parked();
        idle = if done { idle + 1 } else { 0 };
        assert!(
            std::time::Instant::now() < end,
            "pump_control: hang guard expired"
        );
        std::thread::yield_now();
    }
}

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let end = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && !view.edit_pending()
        });
        visual.run_until_parked();
        if done {
            break;
        }
        assert!(std::time::Instant::now() < end, "pump: hang guard expired");
        std::thread::yield_now();
    }
}
#[gpui::test]
fn main_header_add_effect_uses_actual_main_chain_and_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.), px(850.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "ordinary".into()
        })))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| assert!(view.selected_track.is_some()));
    let main = visual.debug_bounds("main-track-header").unwrap();
    visual.simulate_click(
        point(main.left() + px(14.), main.center().y),
        Modifiers::default(),
    );
    editor.read_with(&visual, |view, _| {
        assert!(
            view.selected_track.is_none(),
            "Main selection must not retain the ordinary track target"
        );
        assert!(view.focused_return.is_none());
    });
    visual.run_until_parked();
    let add = visual
        .debug_bounds("device-add-toggle")
        .expect("Main device add control");
    visual.simulate_click(add.center(), Modifiers::default());
    visual.run_until_parked();
    let utility = visual
        .debug_bounds("add-main-effect-hex.utility")
        .expect("actual Main effect menu");
    visual.simulate_click(utility.center(), Modifiers::default());
    pump(&editor, &mut visual);
    let changed = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.main_devices().len(), 1);
        assert!(project.tracks()[0].devices().is_empty());
        assert_eq!(project.tracks().len(), 1);
        project.main_devices()[0].id()
    });
    for _ in 0..3 {
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |view, _| assert!(view.selected_track.is_none()));
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .main_devices()
                .is_empty()
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.main_devices()[0].id(),
            changed
        )
    });
    let add = visual.debug_bounds("device-add-toggle").unwrap();
    visual.simulate_click(add.center(), Modifiers::default());
    visual.run_until_parked();
    let utility = visual.debug_bounds("add-main-effect-hex.utility").unwrap();
    visual.simulate_click(utility.center(), Modifiers::default());
    pump(&editor, &mut visual);
    let (first, second, before_drag) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.main_devices().len(), 2);
        (
            state.project.main_devices()[0].id(),
            state.project.main_devices()[1].id(),
            state.project.snapshot(),
        )
    });
    let landing = super::device_reorder_tests::drag_title(
        &mut visual,
        first,
        second,
        DevicePanelOwner::Main,
        true,
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before_drag)
    });
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let state = view.state.as_ref().unwrap();
        assert_eq!(
            state
                .project
                .main_devices()
                .iter()
                .map(|d| d.id())
                .collect::<Vec<_>>(),
            vec![second, first]
        );
        assert!(state.project.tracks()[0].devices().is_empty());
        assert!(view.active_native_device_drag.is_none());
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before_drag)
    });
}

fn main_gain(project: &Project) -> f32 {
    let device = &project.main_devices()[0];
    let metadata = hex_engine::native_device_view(device.state()).unwrap();
    let value = metadata
        .parameters
        .iter()
        .find(|p| p.descriptor.id.value() == 1)
        .unwrap()
        .value;
    let ParameterValue::Float(value) = value else {
        panic!("Utility gain")
    };
    value
}
fn card_point(
    visual: &mut VisualTestContext,
    device: DeviceId,
    prefix: &str,
) -> gpui::Point<Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", device.value()).into_boxed_str(),
        ))
        .unwrap_or_else(|| panic!("missing actual {prefix} for {}", device.value()))
        .center()
}
#[gpui::test]
fn console_main_parameter_banks_presets_and_project_roundtrip(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1600.), px(850.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "ordinary".into()
        })))
    });
    pump(&editor, &mut visual);
    let mode = visual.debug_bounds("show console view · tab").unwrap();
    visual.simulate_click(mode.center(), Modifiers::default());
    visual.run_until_parked();
    let main = visual.debug_bounds("console-main-header").unwrap();
    visual.simulate_click(
        point(main.left() + px(14.), main.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let add = visual.debug_bounds("device-add-toggle").unwrap();
    visual.simulate_click(add.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("add-main-effect-hex.instrument.sine")
            .is_none()
    );
    let utility = visual.debug_bounds("add-main-effect-hex.utility").unwrap();
    visual.simulate_click(utility.center(), Modifiers::default());
    pump(&editor, &mut visual);
    let device = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.main_devices()[0].id()
    });
    let parameter = hex_lattice_contract::ParameterId::from_static(1);
    let type_gain = |visual: &mut VisualTestContext, value: &str| {
        enter_typed_value(visual, device, parameter, value);
        visual.simulate_keystrokes("enter");
    };
    type_gain(&mut visual, "-6 db");
    pump(&editor, &mut visual);
    let a = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        assert!((main_gain(p) - 10.0_f32.powf(-6.0 / 20.0)).abs() < 1e-5);
        p.clone()
    });
    enter_typed_value(&mut visual, device, parameter, "12 db");
    visual.simulate_keystrokes("escape");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, a)
    });
    let title = card_point(&mut visual, device, "device-drag-title");
    visual.simulate_click(title, Modifiers::default());
    visual.simulate_keystrokes("p");
    pump(&editor, &mut visual);
    type_gain(&mut visual, "6 db");
    pump(&editor, &mut visual);
    let saved = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let entry = p.device(device).unwrap();
        assert_eq!(entry.active_variant(), hex_project::DeviceVariant::B);
        assert!((main_gain(p) - 10.0_f32.powf(6.0 / 20.0)).abs() < 1e-5);
        assert_ne!(
            entry.state().payload(),
            entry.comparison().unwrap().inactive().payload()
        );
        assert!(v.selected_track.is_none());
        assert!(p.tracks()[0].devices().is_empty());
        p.clone()
    });
    let path = environment.root().join("main-banks.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    service.submit(Request::Open(path)).unwrap();
    let opened = receive_service_event(&service);
    assert!(opened.error.is_none(), "{:?}", opened.error);
    assert_eq!(opened.state.project, saved);
    let owner = |v: &EditorView| hex_session::service::preset::Owner {
        project_generation: v.project_generation,
        revision: v.state.as_ref().unwrap().revision,
        device,
        class: NativeDeviceClass::Effect,
        type_id: v
            .state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .type_id(),
    };
    let preset = environment.root().join("main.hex-device-preset.json");
    editor.update(&mut visual, |v, _| {
        let owner = owner(v);
        assert!(v.enqueue(Request::SaveDevicePreset {
            owner,
            path: preset.clone()
        }));
    });
    pump(&editor, &mut visual);
    type_gain(&mut visual, "0 db");
    pump(&editor, &mut visual);
    let before_load = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    editor.update(&mut visual, |v, _| {
        let owner = owner(v);
        assert!(v.enqueue(Request::LoadDevicePreset {
            owner,
            path: preset
        }));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, saved)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before_load)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    for _ in 0..2 {
        let title = card_point(&mut visual, device, "device-drag-title");
        visual.simulate_click(title, Modifiers::default());
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let menu = card_point(&mut visual, device, "device-comparison");
        visual.simulate_mouse_down(menu, MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(menu, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("down enter");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            let d = p.device(device).unwrap();
            assert_eq!(
                d.active_variant(),
                before.device(device).unwrap().active_variant()
            );
            assert_eq!(
                d.state().payload(),
                d.comparison().unwrap().inactive().payload()
            );
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project, before)
        });
        let title = card_point(&mut visual, device, "device-drag-title");
        visual.simulate_click(title, Modifiers::default());
        visual.simulate_keystrokes("p");
        pump(&editor, &mut visual);
    }
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    let bypass = card_point(&mut visual, device, "device-bypass");
    visual.simulate_click(bypass, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .bypassed()
        )
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    let remove = card_point(&mut visual, device, "return-device-remove");
    visual.simulate_click(remove, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.state.as_ref().unwrap().project.main_devices().is_empty())
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });

    let instrument = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
    )
    .unwrap();
    let before_refusal = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.clone(),
            v.state.as_ref().unwrap().revision,
        )
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: instrument
        })))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert!(
            v.error.is_some(),
            "instrument Main insertion must be refused"
        );
        assert_eq!(state.project, before_refusal.0);
        assert_eq!(state.revision, before_refusal.1);
    });
}
