use super::*;

#[gpui::test]
fn device_card_selection_follows_clicks_and_retires_removed_devices(cx: &mut TestAppContext) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bodies: Vec<_> = [instrument, effect]
        .into_iter()
        .map(|device| {
            let selector: &'static str =
                Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
            (
                selector,
                visual.debug_bounds(selector).expect("device body"),
                visual
                    .rendered_background(selector)
                    .expect("device body paint"),
            )
        })
        .collect();
    for device in [instrument, effect] {
        let title = visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", device.value()).into_boxed_str(),
            ))
            .expect("device title")
            .center();
        visual.simulate_click(title, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.selected_device, Some(device));
            assert_eq!(view.state.as_ref().expect("state").revision, revision);
            assert_eq!(view.worker_requests_pending, 0);
        });
        for (selector, bounds, paint) in &bodies {
            assert_eq!(visual.debug_bounds(selector), Some(*bounds));
            assert_eq!(visual.rendered_background(selector), Some(*paint));
        }
    }
    // Right-clicking a different card must not change the left-click selection.
    let bounds = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", instrument.value()).into_boxed_str(),
        ))
        .expect("instrument");
    let title = bounds.center();
    visual.simulate_mouse_down(title, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(title, MouseButton::Right, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(effect))
    });
    let remove = visual
        .debug_bounds(Box::leak(
            format!("device-remove-{}", effect.value()).into_boxed_str(),
        ))
        .expect("remove action");
    visual.simulate_click(remove.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        !view
            .state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .expect("track")
            .devices()
            .iter()
            .any(|device| device.id() == effect)
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, None)
    });
}

#[gpui::test]
fn device_chain_clicks_support_ranges_and_command_toggles(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let type_id = state.project.device(effect).unwrap().state().type_id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let last = state.project.track(track).unwrap().devices()[1].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let title = |visual: &mut VisualTestContext, device: DeviceId| {
        visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", device.value()).into_boxed_str(),
            ))
            .unwrap()
            .center()
    };
    let instrument_title = title(&mut visual, instrument);
    let effect_title = title(&mut visual, effect);
    let last_title = title(&mut visual, last);
    visual.simulate_click(instrument_title, Modifiers::default());
    visual.simulate_click(
        last_title,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            [instrument, effect, last]
                .into_iter()
                .all(|id| view.device_panel.is_selected(id))
        );
        assert_eq!(view.device_panel.selected_device, Some(last));
    });
    visual.simulate_click(
        effect_title,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.is_selected(instrument));
        assert!(view.device_panel.is_selected(effect));
        assert!(!view.device_panel.is_selected(last));
    });
    visual.simulate_keystrokes("backspace");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()
                .len(),
            2
        );
        assert!(view.device_panel.is_selected(instrument));
        assert!(view.device_panel.is_selected(effect));
    });
    visual.simulate_click(
        last_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.simulate_click(
        effect_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.is_selected(instrument));
        assert!(!view.device_panel.is_selected(effect));
        assert!(view.device_panel.is_selected(last));
    });
    visual.simulate_click(effect_title, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(effect));
        assert!(view.device_panel.selected_others.is_empty());
    });
    visual.simulate_click(
        last_title,
        Modifiers {
            platform: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = None;
        view.device_panel.main_selected = Some(view.project_generation);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.owner, Some(DevicePanelOwner::Main));
        assert_eq!(view.device_panel.selected_device, None);
        assert!(view.device_panel.selected_others.is_empty());
    });
}

#[gpui::test]
fn main_and_return_device_chains_support_multi_selection(cx: &mut TestAppContext) {
    let (service, state, track, return_id, first_return, _) = actor_return_utility_fixture();
    let utility = state.project.device(first_return).unwrap().state().clone();
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track: return_id,
            before: None,
            state: utility.clone(),
        }))
        .unwrap();
    let second_return_state = receive_service_event(&service).state;
    let second_return = second_return_state
        .project
        .return_tracks()
        .iter()
        .find(|track| track.id() == return_id)
        .unwrap()
        .devices()[1]
        .id();
    for _ in 0..2 {
        service
            .submit(Request::Edit(Command::InsertMainDevice {
                before: None,
                state: utility.clone(),
            }))
            .unwrap();
        let state = receive_service_event(&service).state;
        if state.project.main_devices().len() == 2 {
            let main = [
                state.project.main_devices()[0].id(),
                state.project.main_devices()[1].id(),
            ];
            let (handle, editor) = actor_editor_window(cx, service, state, track);
            editor.update(cx, |view, cx| {
                view.session_grid.focused_return = Some(return_id);
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.simulate_resize(size(px(1400.0), px(800.0)));
            visual.run_until_parked();
            let title = |visual: &mut VisualTestContext, id: DeviceId| {
                visual
                    .debug_bounds(Box::leak(
                        format!("device-drag-title-{}", id.value()).into_boxed_str(),
                    ))
                    .unwrap()
                    .center()
            };
            let first = title(&mut visual, first_return);
            let second = title(&mut visual, second_return);
            visual.simulate_click(first, Modifiers::default());
            visual.simulate_click(
                second,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            );
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert!(view.device_panel.is_selected(first_return));
                assert!(view.device_panel.is_selected(second_return));
            });
            editor.update(cx, |view, cx| {
                view.session_grid.focused_return = None;
                view.selected_track = None;
                view.device_panel.main_selected = Some(view.project_generation);
                cx.notify();
            });
            visual.run_until_parked();
            let first = title(&mut visual, main[0]);
            let second = title(&mut visual, main[1]);
            visual.simulate_click(first, Modifiers::default());
            visual.simulate_click(
                second,
                Modifiers {
                    platform: true,
                    ..Default::default()
                },
            );
            visual.run_until_parked();
            editor.read_with(cx, |view, _| {
                assert_eq!(view.device_panel.owner, Some(DevicePanelOwner::Main));
                assert!(view.device_panel.is_selected(main[0]));
                assert!(view.device_panel.is_selected(main[1]));
                assert!(!view.device_panel.is_selected(first_return));
            });
            return;
        }
    }
    panic!("main fixture did not create two devices");
}

#[gpui::test]
fn device_keyboard_copy_cut_paste_operates_on_selected_group(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, first, _, _) = actor_utility_fixture();
    let type_id = state.project.device(first).unwrap().state().type_id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let pos = |visual: &mut VisualTestContext, id: DeviceId| {
        visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", id.value()).into_boxed_str(),
            ))
            .unwrap()
            .center()
    };
    let one = pos(&mut visual, first);
    let two = pos(&mut visual, second);
    visual.simulate_click(one, Modifiers::default());
    visual.simulate_click(
        two,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-c");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()
            .len()
            == 4
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-x");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()
            .len()
            == 2
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()
            .len()
            == 4
    });
}

#[gpui::test]
fn device_paste_during_playback_applies_to_actual_callback(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    cx.update(crate::editor::bind_editor_keys);
    let (mut fixture, track, device) = device_playback_fixture(cx);
    let handle = fixture.handle;
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();

    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(title, Modifiers::default());
    visual.simulate_keystrokes("space");
    let deadline = crate::test_wait::hang_deadline();
    while !fixture.backend.status().unwrap().playing.load(Acquire) {
        pump_device_playback(&mut fixture, cx);
        visual.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "playback did not start"
        );
    }
    visual.simulate_keystrokes("cmd-c");
    loop {
        pump_device_playback(&mut fixture, cx);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "copy did not complete"
        );
    }
    visual.simulate_keystrokes("cmd-v");
    loop {
        pump_device_playback(&mut fixture, cx);
        visual.run_until_parked();
        let applied = editor.read_with(cx, |view, _| {
            view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .track(track)
                    .is_some_and(|track| track.devices().len() == 2)
                    && view.applied_project_revision() == Some(state.revision)
            })
        });
        if applied && fixture.backend.status().unwrap().playing.load(Acquire) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pasted device was not applied during playback: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
    }
    fixture.finish(cx);
}

#[gpui::test]
fn device_copy_pastes_into_an_empty_track_chain(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _state, source, device, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "destination".into(),
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let destination = state
        .project
        .tracks()
        .iter()
        .find(|track| track.id() != source)
        .unwrap()
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, source);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(title, Modifiers::default());
    visual.simulate_keystrokes("cmd-c");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(destination);
        cx.notify();
    });
    visual.run_until_parked();
    let empty = visual
        .debug_bounds(Box::leak(
            format!("device-drop-end-{}", destination.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_click(empty, Modifiers::default());
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(destination)
            .unwrap()
            .devices()
            .len()
            == 1
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project.track(source).unwrap().devices()[0].state(),
            project.track(destination).unwrap().devices()[0].state()
        );
        assert_ne!(
            project.track(source).unwrap().devices()[0].id(),
            project.track(destination).unwrap().devices()[0].id()
        );
    });
}

#[gpui::test]
fn return_device_control_click_selects_without_stealing_its_gesture(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, track, return_track, device, _) = actor_return_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = Some(return_track);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let body: &'static str = Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
    let body_bounds = visual.debug_bounds(body).expect("return device body");
    let body_paint = visual
        .rendered_background(body)
        .expect("return device body paint");
    let shell: &'static str =
        Box::leak(format!("device-shell-{}", device.value()).into_boxed_str());
    let border_before = visual.rendered_border(shell).expect("return device frame");
    let header_paint = visual.rendered_background("device-frame-header");
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-knob", device.value()).into_boxed_str(),
        ))
        .expect("gain knob");
    visual.simulate_mouse_down(knob.center(), MouseButton::Left, Modifiers::default());
    let moved = point(knob.center().x, knob.center().y - px(12.0));
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(device));
        assert!(view.device_panel.gesture.snapshot().is_some());
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(body), Some(body_bounds));
    assert_eq!(visual.rendered_background(body), Some(body_paint));
    assert_eq!(
        visual.rendered_background("device-frame-header"),
        header_paint
    );
    let border = visual
        .rendered_border(shell)
        .expect("selected return device frame");
    assert_eq!(border.0, gpui::rgb(device_layout_style().accent).into());
    assert_eq!(border.1, border_before.1);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, Some(device))
    });
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = None;
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.device_panel.gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, None)
    });
    editor.update(cx, |view, cx| {
        view.device_panel.selected_device = Some(device);
        view.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.selected_device, None)
    });
}

#[gpui::test]
fn settled_device_removal_has_no_stale_warning_or_empty_instrument_block(cx: &mut TestAppContext) {
    let (service, state, track, device, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("default instrument")
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let panel_before = visual.debug_bounds("device-panel").expect("device panel");
    let remove_effect = Box::leak(format!("device-remove-{}", device.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(remove_effect)
        .expect("effect remove action");
    let click_point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().is_empty())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.message, None);
    });
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.has_control_for(device));
        assert!(!view.device_panel.views.contains_key(&device));
    });
    let panel_after_effect = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after_effect.size.height, panel_before.size.height);

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.devices().iter().any(|item| item.id() == device))
        })
    });
    visual.run_until_parked();

    let remove_instrument =
        Box::leak(format!("instrument-remove-{}", instrument.value()).into_boxed_str());
    let bounds = visual
        .debug_bounds(remove_instrument)
        .expect("instrument remove action");
    let click_point = point(bounds.center().x, bounds.center().y);
    visual.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state
                .project
                .track(track)
                .is_some_and(|track| track.instrument().is_none())
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.message, None);
    });
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.views.contains_key(&instrument));
    });
    assert!(
        visual.debug_bounds("instrument-empty").is_none(),
        "removing the instrument leaves the chain without a placeholder block"
    );
    let panel_after_instrument = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after_instrument.size.height, px(229.0));
    assert_eq!(panel_after_instrument.size.height, panel_before.size.height);

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                track
                    .instrument()
                    .is_some_and(|item| item.id() == instrument)
            })
        })
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("instrument-empty").is_none());
}

#[gpui::test]
fn browser_device_drop_uses_hovered_card_half_and_commits_before_target(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, existing, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        view.audio_detail.clip_view_open = false;
        view.device_panel.active = false;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.search.read(cx).focus_handle().focus(window)
        });
    });

    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("registered utility in device browser");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-panel").is_some());
    editor.read_with(cx, |view, _| {
        assert!(
            view.audio_detail.clip_view_open,
            "browser drag reveals the closed pane"
        );
        assert!(view.device_panel.active);
        assert_eq!(view.selected_track, Some(track));
    });
    let card_selector =
        Box::leak(format!("device-drop-card-{}", existing.value()).into_boxed_str());
    let card = visual
        .debug_bounds(card_selector)
        .expect("existing utility card drop receiver");
    let destination = point(
        card.origin.x + px(24.0),
        card.origin.y + card.size.height / 2.0,
    );
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .device_panel
            .active_native_drag
            .as_ref()
            .expect("native device drag remains active over rail");
        assert_eq!(
            drag.destination.get(),
            Some(drag::NativeDeviceDropTarget {
                track,
                before: Some(existing),
                replace: None,
            })
        );
    });

    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let loading = visual.debug_bounds("device-chain-loading-slot").unwrap();
    let anchor = visual.debug_bounds(card_selector).unwrap();
    assert!(loading.right() <= anchor.left());
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
        is_held: false,
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(0)),
            "Space immediately after the device drop must reach transport"
        );
    });
    editor.update(cx, |view, _| view.test_probes.last_transport_command = None);
    visual.simulate_event(gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
        is_held: true,
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.test_probes.last_transport_command.is_none(),
            "held Space must not toggle playback repeatedly"
        )
    });
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
    });
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .is_some_and(|track| track.devices().len() == 2)
    });
    editor.read_with(cx, |view, _| {
        let devices = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices();
        assert_eq!(devices[1].id(), existing);
        assert_ne!(devices[0].id(), existing);
        assert!(view.device_panel.active_native_drag.is_none());
        assert!(
            view.audio_detail.clip_view_open,
            "successful drop keeps the pane open"
        );
        assert!(view.device_panel.active);
    });
}

#[gpui::test]
fn device_drop_then_space_starts_actual_callback_and_applies_device(cx: &mut TestAppContext) {
    exercise_device_drop_then_space(cx, false);
}

#[gpui::test]
fn device_drop_then_space_twice_keeps_installed_device_stopped(cx: &mut TestAppContext) {
    exercise_device_drop_then_space(cx, true);
}

#[gpui::test]
fn device_drop_then_space_does_not_start_when_worker_refuses_stale_drop(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    let (mut fixture, track, existing) = device_playback_fixture(cx);
    fixture.editor.update(cx, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        assert!(view.enqueue_catalog_device(Request::DropNativeDevice {
            project_generation: view.project_generation,
            revision: revision - 1,
            track,
            source: hex_session::service::NativeDeviceDropSource::Browser {
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: hex_devices::utility::UTILITY_TYPE_ID,
            },
            before: Some(existing),
            replace: None,
            control: None,
        }));
        view.toggle_play();
    });
    let deadline = crate::test_wait::hang_deadline();
    while fixture.editor.read_with(cx, |view, _| {
        view.device_panel.pending_plugin_effect.is_some()
    }) {
        pump_device_playback(&mut fixture, cx);
        assert!(
            std::time::Instant::now() < deadline,
            "missing stale drop refusal"
        );
    }
    pump_device_playback(&mut fixture, cx);
    fixture.editor.read_with(cx, |view, _| {
        assert!(view.error.is_some());
        assert!(view.audio_pub.deferred_toggle_play.is_none());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()
                .len(),
            1
        );
    });
    assert!(!fixture.backend.status().unwrap().playing.load(Acquire));
    fixture.finish(cx);
}

#[gpui::test]
fn fm4_browser_drop_admits_builtin_instrument_in_fixed_panel(cx: &mut TestAppContext) {
    let (service, _state, track, effect, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("clear default sine slot");
    let empty = receive_service_event(&service);
    assert!(
        empty
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );

    let (handle, editor) = actor_editor_window(cx, service, empty.state, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let panel_before = visual
        .debug_bounds("device-panel")
        .expect("fixed device panel");
    assert_eq!(panel_before.size.height, px(229.0));
    let source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.fm4")
        .expect("registered FM4 browser entry after sine");
    let down = point(
        source.origin.x + source.size.width / 2.0,
        source.origin.y + source.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();

    let card_selector = Box::leak(format!("device-drop-card-{}", effect.value()).into_boxed_str());
    let card = visual
        .debug_bounds(card_selector)
        .expect("effect card receives instrument drop");
    let destination = point(
        card.origin.x + px(24.0),
        card.origin.y + card.size.height / 2.0,
    );
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel
                .active_native_drag
                .as_ref()
                .and_then(|drag| drag.destination.get())
                .map(|target| target.before),
            Some(Some(effect)),
            "instrument drops target the canonical front position"
        );
    });
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|instrument| {
                instrument.state().type_id().as_str() == Ok("hex.instrument.fm4")
            })
    });
    visual.run_until_parked();
    let instrument_id = editor.read_with(cx, |view, _| {
        let instrument = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap();
        assert_eq!(
            instrument.state().type_id().as_str(),
            Ok("hex.instrument.fm4")
        );
        instrument.id()
    });
    let body = Box::leak(format!("instrument-device-{}", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(body).is_some(),
        "FM4 has a rendered card"
    );
    assert!(
        editor.read_with(cx, |view, _| view.device_panel.views[&instrument_id]
            .2
            .as_ref()
            .unwrap()
            .parameters
            .iter()
            .any(|p| p.descriptor.id.value() == 29)),
        "full carrier metadata remains available before scrolling"
    );
    editor.update(cx, |view, cx| {
        view.device_panel.parameter_rows[&instrument_id]
            .scroll
            .set_offset(point(px(0.0), px(-1_000_000.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let carrier =
        Box::leak(format!("device-{}-parameter-29-slider", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(carrier).is_some(),
        "FM4's real carrier descriptor is rendered"
    );
    let panel_after = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel_after.size.height, px(229.0));
    assert_eq!(panel_after.size.height, panel_before.size.height);
}

#[gpui::test]
fn browser_instrument_dropped_on_instrument_card_replaces_in_place_and_retires_old_ui(
    cx: &mut TestAppContext,
) {
    let (service, _state, track, effect, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ONE,
            length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
        }))
        .expect("place clip before replacement");
    let initial = receive_service_event(&service).state;
    let initial_revision = initial.revision;
    let original_track = initial.project.track(track).expect("track").clone();
    let instrument = original_track.instrument().expect("default sine").id();
    let (handle, editor) = actor_editor_window(cx, service, initial, track);
    editor.update(cx, |view, cx| {
        view.browser.section = crate::editor::browser::BrowserSection::Instruments;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let operator_source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.operator")
        .expect("operator browser row");
    let utility_card = visual
        .debug_bounds(Box::leak(
            format!("device-drop-card-{}", effect.value()).into_boxed_str(),
        ))
        .expect("effect card");
    let operator_down = operator_source.center();
    visual.simulate_mouse_down(operator_down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(operator_down.x + px(12.0), operator_down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(utility_card.origin.x + px(24.0), utility_card.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let target = view
            .device_panel
            .active_native_drag
            .as_ref()
            .and_then(|drag| drag.destination.get())
            .expect("drop target over effect card");
        assert_eq!(target.replace, None, "effects are not replacement targets");
    });
    let effect_drop = point(utility_card.origin.x + px(24.0), utility_card.center().y);
    visual.simulate_mouse_up(effect_drop, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_panel.message.as_deref() == Some("instrument slot is already occupied")
    });
    editor.read_with(cx, |view, _| {
        let current = view.state.as_ref().expect("state");
        assert_eq!(current.revision, initial_revision);
        assert_eq!(
            current
                .project
                .track(track)
                .expect("track")
                .instrument()
                .map(hex_project::Device::id),
            Some(instrument)
        );
    });

    let source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.operator")
        .expect("operator browser row after rejected effect-card drop");
    let card_selector =
        Box::leak(format!("device-drop-card-{}", instrument.value()).into_boxed_str());
    let card = visual.debug_bounds(card_selector).expect("instrument card");
    assert_eq!(
        card.size.width,
        px(DEVICE_FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH
            + DEVICE_CHAIN_PADDING)
    );
    let painted_card = visual
        .debug_bounds(Box::leak(
            format!("instrument-device-{}", instrument.value()).into_boxed_str(),
        ))
        .expect("painted instrument card");
    assert_eq!(
        painted_card.size.width,
        px(DEVICE_FRAME_WIDTH + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    let down = source.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let sine_destination = card.center();
    visual.simulate_mouse_move(sine_destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let target = view
            .device_panel
            .active_native_drag
            .as_ref()
            .and_then(|drag| drag.destination.get())
            .expect("replacement target on right side of wide card");
        assert_eq!(target.track, track);
        assert_eq!(target.before, None);
        assert_eq!(target.replace, Some(instrument));
    });
    visual.simulate_mouse_up(sine_destination, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|device| {
                device.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drop = view
            .test_probes
            .last_enqueued_native_device_drop
            .expect("replacement drop request");
        assert_eq!(drop.project_generation, view.project_generation);
        assert_eq!(drop.revision, initial_revision);
        assert_eq!(drop.track, track);
        assert_eq!(drop.before, None);
        assert_eq!(drop.replace, Some(instrument));
        assert!(matches!(
            drop.source,
            hex_session::service::NativeDeviceDropSource::Browser {
                class: NativeDeviceClass::Instrument,
                type_id,
            } if type_id.as_str() == Ok("hex.instrument.operator")
        ));
        let current = view.state.as_ref().expect("state");
        let current_track = current.project.track(track).expect("track");
        assert_eq!(
            current_track.instrument().map(hex_project::Device::id),
            Some(instrument)
        );
        assert_eq!(current_track.devices(), original_track.devices());
        assert_eq!(current_track.clips(), original_track.clips());
        assert_eq!(current_track.midi_clips(), original_track.midi_clips());
        assert!(
            view.device_panel
                .controls
                .keys()
                .any(|key| key.device == instrument)
        );
    });

    let sine_source = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .expect("sine browser row");
    let operator_card = visual
        .debug_bounds(card_selector)
        .expect("wide Operator card");
    assert_eq!(
        operator_card.size.width,
        px(operator_panel::OPERATOR_FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH
            + DEVICE_CHAIN_PADDING)
    );
    let right_side = point(
        operator_card.origin.x + operator_card.size.width - px(16.0),
        operator_card.center().y,
    );
    let operator_view = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        hex_engine::native_device_view_for_class(
            NativeDeviceClass::Instrument,
            state
                .project
                .track(track)
                .expect("track")
                .instrument()
                .expect("operator")
                .state(),
        )
        .expect("operator parameter view")
    });
    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            instrument,
            operator_panel::OperatorPanelState {
                selected_operator: 3,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        let parameter = operator_view.parameters[0];
        view.device_panel.pending_values.insert(
            ParameterKey {
                device: instrument,
                parameter: parameter.descriptor.id,
            },
            parameter.value,
        );
        cx.notify();
    });
    visual.run_until_parked();
    let sine_down = sine_source.center();
    visual.simulate_mouse_down(sine_down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(sine_down.x + px(12.0), sine_down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(right_side, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(right_side, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|device| device.state().type_id().as_str() == Ok("hex.instrument.sine"))
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.has_control_for(instrument));
        assert!(
            !view
                .device_panel
                .pending_values
                .keys()
                .any(|key| key.device == instrument)
        );
        assert!(!view.device_panel.operator_pages.contains_key(&instrument));
        assert_ne!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .and_then(|track| track.instrument())
            .is_some_and(|device| {
                device.state().type_id().as_str() == Ok("hex.instrument.operator")
            })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .controls
                .keys()
                .any(|key| key.device == instrument)
        );
        assert_ne!(
            view.device_panel.message.as_deref(),
            Some("device target is no longer available")
        );
    });
}

#[gpui::test]
fn eq8_panel_renders_on_regular_and_return_tracks_and_retires_on_owner_switch(
    cx: &mut TestAppContext,
) {
    let service = Service::new(48_000, 8, 8_000_000).expect("service");
    let _initial = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "eq8 source".into(),
        }))
        .expect("add track");
    let track = match receive_service_event(&service)
        .created
        .expect("track created")
    {
        Created::Track(track) => track,
        _ => panic!("track result"),
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static("hex.eq8")),
            before: None,
        })
        .expect("add regular EQ8");
    let regular_event = receive_service_event(&service);
    let regular_device = regular_event
        .state
        .project
        .track(track)
        .expect("regular track")
        .devices()[0]
        .id();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "eq8 return".into(),
        }))
        .expect("add return");
    let return_track = match receive_service_event(&service)
        .created
        .expect("return created")
    {
        Created::ReturnTrack(id) => id,
        _ => panic!("return result"),
    };
    service
        .submit(Request::Edit(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.eq8"),
            )
            .expect("EQ8 default state"),
        }))
        .expect("insert return EQ8");
    let state = receive_service_event(&service).state;
    let return_device = state
        .project
        .return_tracks()
        .iter()
        .find(|candidate| candidate.id() == return_track)
        .expect("return owner")
        .devices()[0]
        .id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let regular_body = Box::leak(format!("eq8-body-{}", regular_device.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(regular_body).is_some(),
        "regular EQ8 body"
    );
    let scale = visual.update(|window, _| window.scale_factor());
    assert_eq!(
        visual
            .rendered_surface_geometry("device-plot-section")
            .expect("shared rounded EQ8 graph")
            .0
            .top_left,
        px(hex_widgets::DEVICE_SECTION_RADIUS).scale(scale)
    );
    let eq_body = visual.debug_bounds(regular_body).unwrap();
    for id in [3, 4, 5, 202] {
        let bounds = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{id}", regular_device.value()).into_boxed_str(),
            ))
            .expect("selected-band control");
        assert_eq!(
            bounds.size,
            size(
                px(hex_widgets::MICRO_DIAL_WIDTH),
                px(hex_widgets::MICRO_DIAL_HEIGHT)
            )
        );
        assert!(bounds.origin.x >= eq_body.origin.x && bounds.right() <= eq_body.right());
        assert!(
            bounds.origin.y >= eq_body.origin.y
                && bounds.bottom_left().y <= eq_body.bottom_left().y
        );
    }
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = Some(return_track);
        cx.notify();
    });
    visual.run_until_parked();
    let return_body = Box::leak(format!("eq8-body-{}", return_device.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(return_body).is_some(),
        "Return EQ8 body"
    );
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel.eq8_panels.contains_key(&regular_device),
            "changing owner retires cached regular panel state"
        );
        assert!(view.device_panel.eq8_panels.contains_key(&return_device));
    });
    editor.update(cx, |view, cx| {
        view.session_grid.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(regular_body).is_some(),
        "regular EQ8 returns"
    );
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.eq8_panels.contains_key(&regular_device));
        assert!(!view.device_panel.eq8_panels.contains_key(&return_device));
    });
}

#[gpui::test]
fn operator_panel_exposes_shared_controls_across_operator_envelope_and_routing_pages(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "preview owner".into(),
        }))
        .expect("add a second device-panel owner");
    let second_state = receive_service_event(&service);
    let second_track = match second_state.created.expect("created track") {
        Created::Track(track) => track,
        _ => panic!("created track result"),
    };
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let added = receive_service_event(&service);
    let state = added.state;
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator instrument");
    assert_eq!(
        instrument.state().type_id().as_str(),
        Ok("hex.instrument.operator")
    );
    let instrument_id = instrument.id();
    // Custom authored routing retains all new global controls without exposing a matrix.
    service
        .submit(Request::SetDeviceParameter {
            device: instrument_id,
            parameter: hex_lattice_contract::ParameterId::from_static(17),
            value: ParameterValue::Float(0.31),
        })
        .unwrap();
    let custom = receive_service_event(&service);
    assert!(custom.error.is_none());
    let state = custom.state;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();

    let frame_selector =
        Box::leak(format!("instrument-device-{}", instrument_id.value()).into_boxed_str());
    let frame = visual.debug_bounds(frame_selector).expect("Operator frame");
    assert_eq!(
        frame.size.width,
        px(operator_panel::OPERATOR_FRAME_WIDTH
            + hex_widgets::DEVICE_FRAME_HEADER_WIDTH
            + device_meter::WIDTH)
    );
    assert_eq!(frame.size.height, px(200.0));
    // One passive stereo meter follows the device: left and right bars, no gain handles.
    let meter_selector =
        Box::leak(format!("device-{}-output-meter", instrument_id.value()).into_boxed_str());
    assert!(visual.debug_bounds(meter_selector).is_some());
    for side in ["left", "right"] {
        let selector = Box::leak(format!("{meter_selector}-{side}").into_boxed_str());
        let bar = visual.debug_bounds(selector).expect("channel bar");
        assert_eq!(bar.size.width, px(hex_widgets::STEREO_PEAK_BAR_WIDTH));
        assert!(
            bar.origin.x
                >= frame.origin.x
                    + px(operator_panel::OPERATOR_FRAME_WIDTH
                        + hex_widgets::DEVICE_FRAME_HEADER_WIDTH)
        );
        assert!(bar.right() <= frame.right());
    }
    for side in ["input", "output"] {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("{meter_selector}-{side}-meter-gain-handle").into_boxed_str()
                ))
                .is_none()
        );
    }
    let body_selector =
        Box::leak(format!("operator-body-{}", instrument_id.value()).into_boxed_str());
    let body = visual.debug_bounds(body_selector).expect("Operator body");
    assert_eq!(body.size.height, px(198.0));
    let shell_selector =
        Box::leak(format!("operator-shell-{}", instrument_id.value()).into_boxed_str());
    let shell = visual.debug_bounds(shell_selector).expect("operator shell");
    assert_eq!(
        shell.size.width,
        px(crate::device_composition::VECTOR_SHELL_WIDTH)
    );
    let envelope_graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("amp envelope is the default contextual view");
    assert!(
        envelope_graph.size.width >= px(180.0)
            && envelope_graph.size.width
                <= px(crate::device_composition::VECTOR_DETAIL_WIDTH + 30.0)
    );
    assert!(
        envelope_graph.size.height >= px(80.0),
        "amp graph bounds: {:?}",
        envelope_graph.size
    );
    let tab = visual
        .debug_bounds(Box::leak(
            format!("operator-page-{}-envelope", instrument_id.value()).into_boxed_str(),
        ))
        .expect("flat envelope tab");
    assert!(
        tab.origin.y >= envelope_graph.origin.y + envelope_graph.size.height,
        "page tabs belong beneath the envelope plot"
    );
    let detail = visual
        .debug_bounds(Box::leak(
            format!("operator-detail-{}", instrument_id.value()).into_boxed_str(),
        ))
        .expect("central contextual detail");
    let preview = visual
        .debug_bounds(Box::leak(
            format!("operator-preview-{}", instrument_id.value()).into_boxed_str(),
        ))
        .expect("right side control column");
    assert!(shell.origin.x + shell.size.width <= detail.origin.x);
    assert!(detail.origin.x + detail.size.width <= preview.origin.x);
    for section in ["lfo", "filter", "feedback", "global"] {
        let row = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-{section}", instrument_id.value())
                    .into_boxed_str(),
            ))
            .expect("reference-ordered right-side section");
        assert_eq!(row.size.height, px(40.0));
        assert!(row.origin.x >= preview.origin.x);
        assert!(row.origin.x + row.size.width <= preview.origin.x + preview.size.width);
    }
    for field in 0..6 {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("operator-preview-{}-{field}", instrument_id.value()).into_boxed_str()
                ))
                .is_none(),
            "native Vector omits unsupported preview dials"
        );
    }
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-preview-{}-toggle-0", instrument_id.value()).into_boxed_str()
            ))
            .is_none()
    );
    // Keep the narrow-window geometry checks above, then expose the right rail
    // before testing a mouse click on the wider companion inspector.
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    let route_link = Box::leak(
        format!("operator-preview-{}-route-link", instrument_id.value()).into_boxed_str(),
    );
    let route_bounds = visual
        .debug_bounds(route_link)
        .expect("global routing link");
    assert!(route_bounds.origin.x + route_bounds.size.width <= px(1440.0));
    visual.simulate_click(route_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Envelope,
            "routing picker preserves the selected editor context"
        );
    });
    let envelope_tab =
        Box::leak(format!("operator-page-{}-envelope", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(envelope_tab).is_some(),
        "routing popup leaves the envelope working surface mounted"
    );
    let shell = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-slider", instrument_id.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(shell.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    let background =
        operator_context_selection_tests::row_background(&mut visual, instrument_id, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let envelope_bounds = visual.debug_bounds(envelope_tab).expect("amp envelope tab");
    visual.simulate_click(envelope_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_resize(size(px(1440.0), px(540.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(second_track);
        cx.notify();
    });
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    for operator in 0..4 {
        let row = Box::leak(
            format!("operator-row-{}-{operator}", instrument_id.value()).into_boxed_str(),
        );
        let bounds = visual.debug_bounds(row).expect("operator row");
        assert_eq!(bounds.size.height, px(44.0));
        let level_id = operator * 3 + 2;
        let slider = Box::leak(
            format!(
                "device-{}-parameter-{level_id}-slider",
                instrument_id.value()
            )
            .into_boxed_str(),
        );
        let bounds = visual.debug_bounds(slider).expect("filled level slider");
        assert_eq!(bounds.size, size(px(58.0), px(14.0)));
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{level_id}-knob", instrument_id.value())
                        .into_boxed_str(),
                ))
                .is_none(),
            "level no longer reserves a rotary control"
        );
    }
    for parameter in [1, 2, 4, 5, 7, 8, 10, 11] {
        let readout = Box::leak(
            format!(
                "device-{}-parameter-{parameter}-value",
                instrument_id.value()
            )
            .into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(readout).is_some(),
            "shell parameter {parameter}"
        );
    }

    let envelope_tab =
        Box::leak(format!("operator-page-{}-envelope", instrument_id.value()).into_boxed_str());
    let oscillator_tab =
        Box::leak(format!("operator-page-{}-oscillator", instrument_id.value()).into_boxed_str());
    let oscillator_tab_bounds = visual
        .debug_bounds(oscillator_tab)
        .expect("oscillator detail remains available");
    visual.simulate_click(oscillator_tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let waveform_preview = visual
        .debug_bounds("operator-waveform-cycle")
        .expect("selected oscillator cycle preview");
    assert!(
        (px(200.0)..=px(crate::device_composition::VECTOR_DETAIL_WIDTH + 30.0))
            .contains(&waveform_preview.size.width)
    );
    open_operator_options(&mut visual, instrument_id);
    assert_eq!(waveform_preview.size.height, px(48.0));
    let ratio = hex_lattice_contract::ParameterId::from_static(1);
    enter_typed_value(&mut visual, instrument_id, ratio, "invalid");
    let fix_selector =
        Box::leak(format!("device-{}-parameter-62", instrument_id.value()).into_boxed_str());
    let fix_bounds = visual
        .debug_bounds(fix_selector)
        .expect("operator fixed toggle");
    visual.simulate_click(fix_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let Some(ParameterControl::Bool { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: instrument_id,
                parameter: hex_lattice_contract::ParameterId::from_static(62),
            })
        else {
            panic!("operator fixed toggle remains cached")
        };
        entity.read_with(cx, |toggle, _| {
            assert!(
                !toggle.checked(),
                "hidden invalid ratio blocks fixed-mode switch"
            )
        });
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: instrument_id,
                parameter: ratio,
            })
        else {
            panic!("ratio field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    enter_typed_value(&mut visual, instrument_id, ratio, "1");

    let phase = hex_lattice_contract::ParameterId::from_static(3);
    enter_typed_value(&mut visual, instrument_id, phase, "invalid");
    let tab_bounds = visual.debug_bounds(envelope_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Oscillator,
            "invalid numeric drafts block page navigation"
        );
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: instrument_id,
                parameter: phase,
            })
        else {
            panic!("phase field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    enter_typed_value(&mut visual, instrument_id, phase, "0");

    let osc_bounds = visual
        .debug_bounds(oscillator_tab)
        .expect("oscillator tab remains available");
    visual.simulate_click(osc_bounds.center(), Modifiers::default());
    visual.run_until_parked();

    let waveform_parameter = ParameterKey {
        device: instrument_id,
        parameter: hex_lattice_contract::ParameterId::from_static(57),
    };
    let waveform_selector = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Choice { entity, .. }) =
            view.device_panel.controls.get(&waveform_parameter)
        else {
            panic!("operator waveform selector should be shared and cached")
        };
        entity.clone()
    });
    let waveform_bounds = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-57", instrument_id.value()).into_boxed_str(),
        ))
        .expect("waveform selector bounds");
    visual.simulate_click(waveform_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| {
        assert!(
            selector.is_open(),
            "waveform selector opens before navigation"
        )
    });

    let tab_bounds = visual.debug_bounds(envelope_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| {
        assert!(
            !selector.is_open(),
            "page navigation dismisses selector popup"
        )
    });
    assert!(visual.debug_bounds("operator-envelope-graph").is_some());
    for parameter in 33..=38 {
        let selector = Box::leak(
            format!(
                "device-{}-parameter-{parameter}-value",
                instrument_id.value()
            )
            .into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(selector).is_some(),
            "envelope {parameter}"
        );
    }

    let routing_tab =
        Box::leak(format!("operator-page-{}-routing", instrument_id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(routing_tab).is_none(),
        "routing has no standalone tab"
    );
    let routing_rail = Box::leak(
        format!("operator-preview-row-{}-global", instrument_id.value()).into_boxed_str(),
    );
    let tab_bounds = visual.debug_bounds(routing_rail).expect("routing rail");
    let routing = visual.debug_bounds(route_link).expect("routing picker");
    visual.simulate_click(
        point(routing.right() + px(2.0), tab_bounds.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("operator-routing-{}", instrument_id.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(visual.debug_bounds("operator-routing-header").is_none());
    for parameter in [
        14, 15, 16, 17, 19, 20, 21, 22, 24, 25, 26, 27, 29, 30, 31, 32, 61,
    ] {
        let selector = Box::leak(
            format!("device-{}-parameter-{parameter}", instrument_id.value()).into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(selector).is_none(),
            "legacy routing is preserved without a matrix editor"
        );
    }
    for parameter in
        (601..=608).chain((620..=639).filter(|parameter| !matches!(*parameter, 626 | 634)))
    {
        let selector = Box::leak(
            format!("device-{}-parameter-{parameter}", instrument_id.value()).into_boxed_str(),
        );
        assert!(
            visual.debug_bounds(selector).is_some(),
            "global parameter {parameter}"
        );
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Routing
        );
    });

    let background =
        operator_context_selection_tests::row_background(&mut visual, instrument_id, 1);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let waveform_b = ParameterKey {
        device: instrument_id,
        parameter: hex_lattice_contract::ParameterId::from_static(58),
    };
    let waveform_selector = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Choice { entity, .. }) =
            view.device_panel.controls.get(&waveform_b)
        else {
            panic!("operator b waveform selector should be shared and cached")
        };
        entity.clone()
    });
    let waveform_bounds = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-58", instrument_id.value()).into_boxed_str(),
        ))
        .expect("operator b waveform selector bounds");
    visual.simulate_click(waveform_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| assert!(selector.is_open()));
    let tab_bounds = visual.debug_bounds(envelope_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| {
        assert!(
            !selector.is_open(),
            "page navigation dismisses selector popup"
        )
    });
    let background =
        operator_context_selection_tests::row_background(&mut visual, instrument_id, 1);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id].page,
            operator_panel::OperatorPage::Envelope,
            "choosing another oscillator keeps the amp envelope visible"
        );
    });
    let osc_bounds = visual
        .debug_bounds(oscillator_tab)
        .expect("oscillator tab remains available");
    visual.simulate_click(osc_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-58", instrument_id.value()).into_boxed_str()
            ))
            .is_some(),
        "selecting oscillator b reveals its own waveform control"
    );
    editor.read_with(cx, |view, _| {
        let page = view.device_panel.operator_pages[&instrument_id];
        assert_eq!(page.selected_operator, 1);
        assert_eq!(page.page, operator_panel::OperatorPage::Oscillator);
    });
}

#[gpui::test]
fn operator_envelope_graph_keyboard_edit_is_canonical_undoable_and_persistent(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let initial = receive_service_event(&service).state;
    let device = initial
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator")
        .id();
    let get = |state: &ViewState, parameter: u32| {
        operator_parameter_value(state, track, device, parameter)
    };
    let original_peak = get(&initial, 34);
    let original_attack = get(&initial, 36);
    let envelope_values: Vec<f32> = (33..=38)
        .map(|parameter| match get(&initial, parameter) {
            ParameterValue::Float(value) => value,
            _ => panic!("Operator envelope value is not numeric"),
        })
        .collect();
    let attack_x = |seconds: f32| {
        ((f64::from(seconds) / 0.001).ln_1p() / (20.0_f64 / 0.001).ln_1p()) as f32 / 3.0
    };
    let stage_x =
        |seconds: f32| ((f64::from(seconds) / 0.001).ln() / (60.0_f64 / 0.001).ln()) as f32 / 3.0;
    let path = std::env::temp_dir().join(format!(
        "hex-operator-envelope-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ));
    service
        .submit(Request::SaveNew(path.clone()))
        .expect("save initial Operator fixture");
    let saved = receive_service_event(&service);
    assert!(saved.save_complete);
    assert!(saved.error.is_none());
    let (handle, editor) = actor_editor_window(cx, service, saved.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let env_tab = Box::leak(format!("operator-page-{}-envelope", device.value()).into_boxed_str());
    let tab_bounds = visual.debug_bounds(env_tab).expect("envelope tab");
    visual.simulate_click(tab_bounds.center(), Modifiers::default());
    visual.run_until_parked();
    let graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("editable Operator envelope graph");
    let width = graph.size.width - px(14.0);
    let peak = point(
        graph.origin.x + px(7.0) + width * attack_x(envelope_values[3]),
        graph.origin.y + px(7.0),
    );
    visual.simulate_click(peak, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("right");
    pump_until(&editor, cx, |view| {
        view.device_panel.gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| get(state, 36) != original_attack)
    });
    let edited_attack = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("edited state");
        assert_eq!(
            get(state, 34),
            original_peak,
            "horizontal peak edit preserves level"
        );
        let ParameterValue::Float(attack) = get(state, 36) else {
            panic!("attack remains numeric")
        };
        assert!(
            attack
                > match original_attack {
                    ParameterValue::Float(value) => value,
                    _ => -1.0,
                }
        );
        attack
    });

    // A terminal commit must not fit the changed total duration. Cumulative
    // decay follows attack while release stays in its fixed final third.
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        let peak = operator_panel::envelope::displayed_node(view, device, 0, 1, cx).unwrap();
        let decay = operator_panel::envelope::displayed_node(view, device, 0, 2, cx).unwrap();
        let release = operator_panel::envelope::displayed_node(view, device, 0, 3, cx).unwrap();
        assert!(
            (peak[0] - attack_x(edited_attack)).abs() < 1e-6,
            "commit preserves zoom"
        );
        assert!(
            (decay[0] - (attack_x(edited_attack) + stage_x(envelope_values[4]))).abs() < 1e-6,
            "decay follows cumulative attack"
        );
        assert!(
            (release[0] - (2.0 / 3.0 + stage_x(envelope_values[5]))).abs() < 1e-6,
            "release stays in fixed final section without fitting"
        );
    });

    editor.update(cx, |view, cx| view.save_project(cx));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
            && !view.persistence.saving_project
            && view.state.as_ref().is_some_and(|state| !state.unsaved)
    });
    let reload = Service::new(48_000, 8, 8_000_000).expect("reload service");
    let _initial = receive_service_event(&reload);
    reload
        .submit(Request::Open(path.clone()))
        .expect("reopen saved Operator project");
    let reopened = receive_service_event(&reload).state;
    assert_eq!(get(&reopened, 36), ParameterValue::Float(edited_attack));

    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| get(state, 36) == original_attack)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            get(view.state.as_ref().expect("undo state"), 34),
            original_peak
        );
    });
    let initial_parameter = hex_lattice_contract::ParameterId::from_static(33);
    enter_typed_value(&mut visual, device, initial_parameter, "invalid");
    let graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("envelope graph remains visible during numeric draft");
    let peak = point(
        graph.origin.x + px(7.0) + (graph.size.width - px(14.0)) * attack_x(envelope_values[3]),
        graph.origin.y + px(7.0),
    );
    let moved = point(peak.x + px(12.0), peak.y);
    visual.simulate_mouse_down(peak, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.gesture.snapshot().is_none());
        assert!(view.device_panel.pending_values.is_empty());
        assert_eq!(
            get(view.state.as_ref().expect("draft state"), 33),
            ParameterValue::Float(0.0)
        );
        let key = ParameterKey {
            device,
            parameter: initial_parameter,
        };
        let Some(ParameterControl::Float { entity, .. }) = view.device_panel.controls.get(&key)
        else {
            panic!("Operator initial value field remains cached")
        };
        entity.read_with(cx, |control, _| {
            assert!(
                control.is_editing(),
                "invalid numeric draft stays available for repair"
            )
        });
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    let _ = std::fs::remove_file(path);
}

#[gpui::test]
fn operator_envelope_release_drag_cancels_and_commits_after_outside_release(
    cx: &mut TestAppContext,
) {
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let _operator_added = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "second device owner".into(),
        }))
        .expect("add second owner");
    let second_added = receive_service_event(&service);
    let second_track = match second_added.created.expect("created track") {
        Created::Track(track) => track,
        _ => panic!("created track result"),
    };
    let state = second_added.state;
    let device = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator")
        .id();
    let float_value = |state: &ViewState, parameter| match operator_parameter_value(
        state, track, device, parameter,
    ) {
        ParameterValue::Float(value) => value,
        _ => panic!("Operator envelope parameter is numeric"),
    };
    let original_release = float_value(&state, 38);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let env_tab = Box::leak(format!("operator-page-{}-envelope", device.value()).into_boxed_str());
    let tab = visual.debug_bounds(env_tab).expect("envelope tab");
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();

    let release_endpoint = |visual: &mut VisualTestContext, cx: &mut TestAppContext| {
        let graph = visual
            .debug_bounds("operator-envelope-graph")
            .expect("envelope graph");
        let x = editor.read_with(cx, |view, cx| {
            operator_panel::envelope::displayed_node(view, device, 0, 3, cx).unwrap()[0]
        });
        point(
            graph.origin.x + px(7.0) + (graph.size.width - px(14.0)) * x,
            graph.origin.y + graph.size.height - px(7.0),
        )
    };

    let start = release_endpoint(&mut visual, cx);
    let moved = point(start.x + px(16.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    // Pointer feedback is already in the retained graph and numeric entity
    // before another whole-editor layout or worker acknowledgement is pumped.
    let bounds = visual
        .debug_bounds("operator-envelope-graph")
        .expect("graph bounds");
    let feedback_x = f32::from(moved.x - bounds.origin.x - px(7.0))
        / f32::from(bounds.size.width - px(14.0))
        + 0.02;
    editor.update(cx, |view, cx| {
        operator_panel::envelope::dispatch_change(view, device, 0, 3, [feedback_x, 0.0], cx);
        let node = operator_panel::envelope::displayed_node(view, device, 0, 3, cx)
            .expect("immediate dragged node");
        assert!(
            (node[0] - feedback_x).abs() < 1e-5,
            "node follows this input turn"
        );
        let key = ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(38),
        };
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&key] else {
            panic!("release readout");
        };
        let ParameterValue::Float(pending) = view.device_panel.pending_values[&key] else {
            panic!("pending release");
        };
        assert_eq!(entity.read(cx).value(), f64::from(pending));
        assert_eq!(
            float_value(view.state.as_ref().expect("canonical state"), 38),
            original_release
        );
    });

    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let snapshot = view
            .device_panel
            .gesture
            .snapshot()
            .expect("release drag begins");
        assert_eq!(
            snapshot.target,
            GestureTarget::DeviceParameter {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(38),
            }
        );
        assert!(
            view.device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(38),
                })
        );
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_panel.gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| float_value(state, 38) == original_release)
    });
    editor.read_with(cx, |view, _| {
        assert!(
            !view
                .device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(38),
                })
        );
    });

    let start = release_endpoint(&mut visual, cx);
    let graph = visual
        .debug_bounds("operator-envelope-graph")
        .expect("graph after cancel");
    let outside = point(graph.origin.x + graph.size.width + px(16.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel.gesture.snapshot().is_some(),
            "pointer capture continues past the plot edge"
        );
    });
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_panel.gesture.snapshot().is_none()
            && view
                .state
                .as_ref()
                .is_some_and(|state| float_value(state, 38) > original_release)
    });
    let committed_release = editor.read_with(cx, |view, _| {
        float_value(view.state.as_ref().expect("committed state"), 38)
    });
    assert_eq!(
        committed_release, 60.0,
        "outside release clamps to hard maximum"
    );

    let start = release_endpoint(&mut visual, cx);
    let moved = point(start.x - px(12.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(editor.read_with(cx, |view, _| view.device_panel.gesture.snapshot().is_some()));
    // The oscillator page retains the envelope; routing actually hides it.
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .expect("Operator page")
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_panel.gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            float_value(view.state.as_ref().expect("page state"), 38),
            committed_release
        );
        assert!(
            !view
                .device_panel
                .pending_values
                .contains_key(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(38),
                })
        );
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel
                .operator_pages
                .get(&device)
                .expect("page")
                .page,
            operator_panel::OperatorPage::Routing
        );
        assert!(view.device_panel.operator_envelope_graphs.is_empty());
    });

    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .expect("Operator page")
            .page = operator_panel::OperatorPage::Envelope;
        cx.notify();
    });
    visual.run_until_parked();
    let start = release_endpoint(&mut visual, cx);
    let moved = point(start.x - px(12.0), start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.selected_track = Some(second_track);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Track(track))
        );
        assert!(
            view.device_panel.gesture.snapshot().is_some(),
            "owner switching keeps the active graph visible until its gesture ends"
        );
    });
    assert!(visual.debug_bounds("operator-envelope-graph").is_some());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.device_panel.gesture.snapshot().is_none()
            && view.device_panel.owner == Some(DevicePanelOwner::Track(second_track))
    });
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.pending_values.is_empty());
        assert!(view.device_panel.operator_envelope_graphs.is_empty());
    });
}

#[gpui::test]
fn operator_shell_controls_preserve_edits_across_explicit_context_selection(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, utility, utility_parameter, utility_enabled) =
        actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let state = receive_service_event(&service).state;
    let operator = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator instrument")
        .id();
    let operator_level = |state: &ViewState, parameter: u32| {
        let instrument = state
            .project
            .track(track)
            .and_then(hex_project::Track::instrument)
            .expect("Operator instrument");
        hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, instrument.state())
            .expect("Operator view")
            .parameters
            .into_iter()
            .find(|item| item.descriptor.id.value() == parameter)
            .expect("Operator parameter")
            .value
    };
    let original_d_level = operator_level(&state, 11);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            operator,
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        cx.notify();
    });
    visual.run_until_parked();
    let oscillator_tab =
        Box::leak(format!("operator-page-{}-oscillator", operator.value()).into_boxed_str());
    assert!(visual.debug_bounds(oscillator_tab).is_none());
    let shell = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-1-slider", operator.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(shell.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    let background = operator_context_selection_tests::row_background(&mut visual, operator, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    let tab = visual.debug_bounds(oscillator_tab).unwrap();
    visual.simulate_click(tab.center(), Modifiers::default());
    visual.run_until_parked();

    let waveform_a = ParameterKey {
        device: operator,
        parameter: hex_lattice_contract::ParameterId::from_static(57),
    };
    let waveform_selector = editor.read_with(cx, |view, _| {
        let Some(ParameterControl::Choice { entity, .. }) =
            view.device_panel.controls.get(&waveform_a)
        else {
            panic!("operator a waveform selector is cached")
        };
        entity.clone()
    });
    let waveform = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-57", operator.value()).into_boxed_str(),
        ))
        .expect("operator a waveform selector");
    visual.simulate_click(waveform.center(), Modifiers::default());
    visual.run_until_parked();
    waveform_selector.read_with(cx, |selector, _| assert!(selector.is_open()));

    for selected in [1_usize, 2, 3, 0] {
        let parameter = selected as u32 * 3 + 2;
        let knob = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{parameter}-slider", operator.value())
                    .into_boxed_str(),
            ))
            .expect("Operator shell level slider");
        let background =
            operator_context_selection_tests::row_background(&mut visual, operator, selected);
        visual.simulate_click(background, Modifiers::default());
        visual.simulate_click(knob.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&operator],
                operator_panel::OperatorPanelState {
                    selected_operator: selected,
                    page: operator_panel::OperatorPage::Oscillator,
                    ..Default::default()
                }
            );
        });
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{}", operator.value(), 57 + selected)
                        .into_boxed_str(),
                ))
                .is_some(),
            "the selected oscillator's center detail is shown"
        );
    }
    waveform_selector.read_with(cx, |selector, _| {
        assert!(!selector.is_open(), "context changes dismiss the old popup")
    });

    open_operator_options(&mut visual, operator);
    let phase_a = hex_lattice_contract::ParameterId::from_static(3);
    enter_typed_value(&mut visual, operator, phase_a, "invalid");
    let d_level = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-11-slider", operator.value()).into_boxed_str(),
        ))
        .expect("operator d level slider");
    visual.simulate_click(d_level.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: operator,
                parameter: phase_a,
            })
        else {
            panic!("operator a phase field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    let phase_a_value = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-3-value", operator.value()).into_boxed_str(),
        ))
        .expect("operator a phase value");
    visual.simulate_click(phase_a_value.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();

    let d_level = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-11-slider", operator.value()).into_boxed_str(),
        ))
        .expect("operator d level slider");
    let start = point(d_level.center().x, d_level.center().y);
    let moved = point(start.x, start.y - px(30.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
    });
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
        let gesture = view
            .device_panel
            .gesture
            .snapshot()
            .expect("shell drag starts its parameter gesture");
        assert_eq!(
            gesture.target,
            GestureTarget::DeviceParameter {
                device: operator,
                parameter: hex_lattice_contract::ParameterId::from_static(11),
            }
        );
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture_barriers.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            operator_level(view.state.as_ref().expect("state"), 11),
            original_d_level,
            "dragging the shell control changes its own parameter"
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .is_some_and(|state| operator_level(state, 11) == original_d_level)
    });

    let original_b_level = editor.read_with(cx, |view, _| {
        operator_level(view.state.as_ref().expect("state"), 5)
    });
    let b_level = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-5-slider", operator.value()).into_boxed_str(),
        ))
        .expect("operator b level slider");
    visual.simulate_click(b_level.center(), Modifiers::default());
    // A filled field opens text entry on click; return to numeric stepping.
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            operator,
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("up");
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .is_some_and(|state| operator_level(state, 5) != original_b_level)
            && view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture_barriers.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&operator],
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
            "numeric adjustment preserves the explicitly selected editor"
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .is_some_and(|state| operator_level(state, 5) == original_b_level)
    });

    let c_level_value = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-8-value", operator.value()).into_boxed_str(),
        ))
        .expect("operator c level numeric field");
    visual.simulate_click(c_level_value.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.device_panel.operator_pages[&operator].selected_operator,
            0
        );
        let Some(ParameterControl::Float { entity, .. }) =
            view.device_panel.controls.get(&ParameterKey {
                device: operator,
                parameter: hex_lattice_contract::ParameterId::from_static(8),
            })
        else {
            panic!("operator c level field remains cached")
        };
        entity.read_with(cx, |control, _| assert!(control.is_editing()));
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-6 db");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_panel.gesture_barriers.is_empty()
            && view.device_panel.gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().expect("state");
        assert_eq!(
            operator_level(state, 8),
            ParameterValue::Float(10.0_f32.powf(-6.0 / 20.0)),
            "the newly focused numeric field accepts and commits typed input"
        );
        assert_eq!(
            parameter_value(state, track, utility, utility_parameter,),
            ParameterValue::Bool(utility_enabled),
            "Operator context changes leave the neighboring Utility unchanged"
        );
        assert!(
            view.device_panel
                .controls
                .keys()
                .any(|key| key.device == utility)
        );
    });
}

#[gpui::test]
fn operator_shell_fixed_rows_fit_without_shrinking_controls(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, _state, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::RemoveInstrument { track }))
        .expect("remove default instrument");
    let _empty = receive_service_event(&service);
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Instrument,
            type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                "hex.instrument.operator",
            )),
            before: None,
        })
        .expect("add native Operator");
    let mut state = receive_service_event(&service).state;
    let instrument = state
        .project
        .track(track)
        .expect("track")
        .instrument()
        .expect("Operator instrument");
    let instrument_id = instrument.id();
    for operator in 0..4_u32 {
        service
            .submit(Request::SetDeviceParameter {
                device: instrument_id,
                parameter: hex_lattice_contract::ParameterId::from_static(62 + operator * 3),
                value: ParameterValue::Bool(true),
            })
            .expect("enable fixed frequency");
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for width in [880.0, 1280.0] {
        visual.simulate_resize(size(px(width), px(540.0)));
        visual.run_until_parked();
        let shell = visual
            .debug_bounds(Box::leak(
                format!("operator-shell-{}", instrument_id.value()).into_boxed_str(),
            ))
            .expect("Operator shell");
        assert_eq!(
            shell.size.width,
            px(crate::device_composition::VECTOR_SHELL_WIDTH)
        );
        for operator in 0..4 {
            let row = Box::leak(
                format!("operator-row-{}-{operator}", instrument_id.value()).into_boxed_str(),
            );
            let row_bounds = visual.debug_bounds(row).expect("fixed-mode row");
            assert_eq!(row_bounds.size.height, px(44.0));
            assert!(row_bounds.origin.y >= shell.origin.y);
            assert!(
                row_bounds.origin.y + row_bounds.size.height <= shell.origin.y + shell.size.height,
                "wider gutters must not push the bottom oscillator outside its shell"
            );
            if operator < 3 {
                let previous = visual
                    .debug_bounds(Box::leak(
                        format!("operator-row-{}-{}", instrument_id.value(), operator + 1)
                            .into_boxed_str(),
                    ))
                    .expect("previous oscillator row");
                assert_eq!(
                    row_bounds.origin.y - previous.origin.y - previous.size.height,
                    px(2.0),
                    "dark oscillator rows have a visible two-pixel gutter"
                );
            }
            let badge = visual
                .debug_bounds(Box::leak(
                    format!("operator-select-{}-{operator}", instrument_id.value())
                        .into_boxed_str(),
                ))
                .expect("compact oscillator badge");
            assert_eq!(badge.size, gpui::size(px(6.0), px(36.0)));
            let ratio_id = 1 + operator * 3;
            let level_id = operator * 3 + 2;
            for parameter in [ratio_id, level_id, 609 + operator] {
                let field = Box::leak(
                    format!("device-{}-parameter-{parameter}", instrument_id.value())
                        .into_boxed_str(),
                );
                let bounds = visual.debug_bounds(field).expect("fixed-mode control");
                assert_eq!(bounds.size.width, px(60.0));
                let slider = visual
                    .debug_bounds(Box::leak(
                        format!(
                            "device-{}-parameter-{parameter}-slider",
                            instrument_id.value()
                        )
                        .into_boxed_str(),
                    ))
                    .unwrap();
                assert_eq!(slider.size, size(px(58.0), px(14.0)));
                assert!(
                    visual
                        .debug_bounds(Box::leak(
                            format!(
                                "device-{}-parameter-{parameter}-knob",
                                instrument_id.value()
                            )
                            .into_boxed_str()
                        ))
                        .is_none()
                );
                assert!(bounds.origin.x >= row_bounds.origin.x + px(4.0));
                assert!(
                    bounds.origin.x + bounds.size.width
                        <= row_bounds.origin.x + row_bounds.size.width - px(4.0),
                    "oscillator controls need an inset from the dark row edges"
                );
            }
            let field_bounds = |visual: &mut VisualTestContext, parameter| {
                visual
                    .debug_bounds(Box::leak(
                        format!("device-{}-parameter-{parameter}", instrument_id.value())
                            .into_boxed_str(),
                    ))
                    .unwrap()
            };
            let coarse = field_bounds(&mut visual, ratio_id);
            let fine = field_bounds(&mut visual, 609 + operator);
            let level = field_bounds(&mut visual, level_id);
            let wave = field_bounds(&mut visual, 57 + operator);
            assert!(badge.right() < coarse.left());
            assert!(coarse.bottom() <= fine.top());
            assert!(coarse.right() <= level.left());
            assert!(level.bottom() <= wave.top());
            for field in [coarse, fine, level, wave] {
                assert!(field.top() >= row_bounds.top() && field.bottom() <= row_bounds.bottom());
                assert!(field.right() <= row_bounds.right());
            }
            let keytrack = field_bounds(&mut visual, 613 + operator);
            assert!(keytrack.left() >= level.right());
            assert!(keytrack.right() <= row_bounds.right() - px(4.0));
        }
        let lfo = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-lfo", instrument_id.value()).into_boxed_str(),
            ))
            .expect("native LFO row");
        for parameter in [82, 83, 85, 87] {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", instrument_id.value())
                        .into_boxed_str(),
                ))
                .expect("native LFO control");
            assert!(control.origin.x >= lfo.origin.x + px(4.0));
            assert!(
                control.origin.x + control.size.width <= lfo.origin.x + lfo.size.width - px(4.0)
            );
        }
        let filter = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-filter", instrument_id.value()).into_boxed_str(),
            ))
            .unwrap();
        for parameter in [20001, 20002, 20005, 20006] {
            let control = visual
                .debug_bounds(Box::leak(
                    format!("device-{}-parameter-{parameter}", instrument_id.value())
                        .into_boxed_str(),
                ))
                .unwrap();
            assert!(control.origin.x >= filter.origin.x + px(4.0));
            assert!(
                control.origin.x + control.size.width
                    <= filter.origin.x + filter.size.width - px(4.0)
            );
        }
        for field in 0..6 {
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("operator-preview-{}-{field}", instrument_id.value())
                            .into_boxed_str(),
                    ))
                    .is_none(),
                "unsupported preview controls are not mounted"
            );
        }
        let feedback = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-81", instrument_id.value()).into_boxed_str(),
            ))
            .expect("real feedback slider");
        let row = visual
            .debug_bounds(Box::leak(
                format!("operator-preview-row-{}-feedback", instrument_id.value()).into_boxed_str(),
            ))
            .unwrap();
        assert!(feedback.left() >= row.left() && feedback.right() <= row.right());
    }
    // LFO uses canonical controls after PR154, with the same accepted row inset.
    let lfo_row = visual
        .debug_bounds(Box::leak(
            format!("operator-preview-row-{}-lfo", instrument_id.value()).into_boxed_str(),
        ))
        .expect("canonical LFO row");
    for parameter in [82, 85, 87] {
        let control = visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{parameter}", instrument_id.value()).into_boxed_str(),
            ))
            .expect("canonical LFO control");
        assert!(control.origin.x >= lfo_row.origin.x + px(4.0));
        assert!(
            control.origin.x + control.size.width
                <= lfo_row.origin.x + lfo_row.size.width - px(4.0)
        );
    }
    open_operator_options(&mut visual, instrument_id);
    // Fixed frequency and multiplier have distinct IDs from ratio controls.
    // A keyboard edit must follow the focused fixed-frequency field too.
    let at = operator_context_selection_tests::row_background(&mut visual, instrument_id, 1);
    visual.simulate_click(at, Modifiers::default());
    visual.run_until_parked();
    let frequency = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-66-value", instrument_id.value()).into_boxed_str(),
        ))
        .expect("operator b fixed frequency");
    visual.simulate_click(frequency.center(), Modifiers::default());
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        view.device_panel.operator_pages.insert(
            instrument_id,
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            },
        );
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("up");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&instrument_id],
            operator_panel::OperatorPanelState {
                selected_operator: 0,
                page: operator_panel::OperatorPage::Routing,
                ..Default::default()
            }
        );
    });
}

#[gpui::test]
fn utility_panel_matches_compact_geometry_and_uses_shared_mode_popup(cx: &mut TestAppContext) {
    let (service, mut state, track, device, _, _) = actor_utility_fixture();
    let descriptors = hex_engine::native_device_descriptors_for_class(
        hex_engine::NativeDeviceClass::Effect,
        state.project.track(track).expect("track").devices()[0]
            .state()
            .type_id(),
    )
    .expect("Utility descriptors");
    let parameter = |label: &str| {
        descriptors
            .iter()
            .find(|descriptor| descriptor.label == label)
            .expect("Utility parameter")
            .id
    };
    let width = parameter("width");
    let channel = parameter("channel");
    let mid_side = parameter("mid-side");
    let mid_side_balance = parameter("mid-side balance");
    assert_eq!(descriptors.len(), 14);
    for (parameter, value) in [
        (width, ParameterValue::Float(2.5)),
        (mid_side, ParameterValue::Bool(false)),
        (mid_side_balance, ParameterValue::Float(-0.4)),
        (channel, ParameterValue::Choice(0)),
    ] {
        service
            .submit(Request::SetDeviceParameter {
                device,
                parameter,
                value,
            })
            .expect("Utility setup request");
        state = receive_service_event(&service).state;
    }

    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.worker_requests_pending = 1;
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: mid_side,
            },
            ParameterValue::Bool(true),
        );
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let panel = visual.debug_bounds("device-panel").expect("device panel");
    assert_eq!(panel.size.height, px(229.0));
    let frame_selector = Box::leak(format!("effect-device-{}", device.value()).into_boxed_str());
    let frame = visual.debug_bounds(frame_selector).expect("utility frame");
    assert_eq!(
        frame.size.width,
        px(144.0 + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH)
    );
    assert_eq!(frame.size.height, px(200.0));
    let input_selector = Box::leak(format!("utility-input-{}", device.value()).into_boxed_str());
    let output_selector = Box::leak(format!("utility-output-{}", device.value()).into_boxed_str());
    let input = visual.debug_bounds(input_selector).expect("input column");
    let output = visual.debug_bounds(output_selector).expect("output column");
    assert_eq!(input.size.width, px(64.0));
    assert_eq!(output.size.width, px(64.0));
    let scale = visual.update(|window, _| window.scale_factor());
    for section in ["device-input-section", "device-output-section"] {
        let bounds = visual
            .debug_bounds(section)
            .expect("shared rounded Utility section");
        assert_eq!(
            bounds.size.width,
            px(64.0),
            "rounded sections preserve control width"
        );
        let corners = visual
            .rendered_surface_geometry(section)
            .expect("painted rounded section")
            .0;
        assert_eq!(
            corners.top_left,
            px(hex_widgets::DEVICE_SECTION_RADIUS).scale(scale)
        );
        assert_eq!(
            visual.rendered_background(section),
            Some(gpui::Background::from(rgb(device_layout_style().rail)))
        );
    }
    let body_selector = Box::leak(format!("device-body-{}", device.value()).into_boxed_str());
    let body = visual.debug_bounds(body_selector).expect("Utility body");
    assert_eq!(body.size.width, px(140.0));
    assert_eq!(body.size.height, px(198.0));
    let mut parameter_bounds = |id: u32, suffix: &str| {
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{id}{suffix}", device.value()).into_boxed_str(),
            ))
            .unwrap_or_else(|| panic!("Utility parameter {id}{suffix} is rendered"))
    };
    for id in [1, 2, 10] {
        let dial = parameter_bounds(id, "-knob");
        assert!(dial.size.width >= px(24.0) && dial.size.width <= px(26.0));
        assert!(dial.size.height >= px(24.0) && dial.size.height <= px(26.0));
        let readout = parameter_bounds(id, "-value");
        assert_eq!(readout.size.width, px(64.0));
        assert_eq!(readout.size.height, px(14.0));
    }
    let cutoff = parameter_bounds(13, "");
    assert_eq!(cutoff.size.width, px(46.0));
    assert_eq!(cutoff.size.height, px(14.0));
    assert_eq!(parameter_bounds(13, "-value").size.height, px(14.0));
    for (id, width) in [
        (5, 31.0),
        (6, 31.0),
        (7, 64.0),
        (8, 31.0),
        (11, 31.0),
        (12, 64.0),
    ] {
        let bounds = parameter_bounds(id, "");
        assert_eq!(bounds.size.width, px(width));
        assert_eq!(bounds.size.height, px(14.0));
        assert!(bounds.origin.x >= body.origin.x);
        assert!(bounds.origin.x + bounds.size.width <= body.origin.x + body.size.width);
    }
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-2-pan-reset", device.value()).into_boxed_str()
            ))
            .is_some(),
        "balance uses shared pan semantics"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-10-pan-reset", device.value()).into_boxed_str()
            ))
            .is_none(),
        "mid-side bipolar is not automatically pan"
    );
    let mode_knob = Box::leak(
        format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            mid_side_balance.value()
        )
        .into_boxed_str(),
    );
    let mode_bounds = visual
        .debug_bounds(mode_knob)
        .expect("pending mid-side dial");
    assert!(mode_bounds.origin.x >= input.origin.x);
    assert!(mode_bounds.origin.x + mode_bounds.size.width <= input.origin.x + input.size.width);
    editor.update(cx, |view, cx| {
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: mid_side,
        });
        view.worker_requests_pending = 0;
        cx.notify();
    });
    visual.run_until_parked();
    let width_knob = Box::leak(
        format!("device-{}-parameter-{}-knob", device.value(), width.value()).into_boxed_str(),
    );
    assert!(
        visual.debug_bounds(width_knob).is_some(),
        "canonical width mode is shown after pending state clears"
    );
    let mut parameter_bounds = |id: u32, suffix: &str| {
        visual
            .debug_bounds(Box::leak(
                format!("device-{}-parameter-{id}{suffix}", device.value()).into_boxed_str(),
            ))
            .unwrap_or_else(|| panic!("Utility parameter {id}{suffix} is rendered"))
    };
    assert!(parameter_bounds(3, "-knob").size.width >= px(24.0));
    assert!(parameter_bounds(3, "-value").size.width >= px(60.0));
    let gain_value =
        Box::leak(format!("device-{}-parameter-1-value", device.value()).into_boxed_str());
    let gain_bounds = visual.debug_bounds(gain_value).expect("gain readout");
    assert!(gain_bounds.origin.x >= output.origin.x);
    assert!(gain_bounds.origin.x + gain_bounds.size.width <= output.origin.x + output.size.width);
    let readout = |parameter: u32| {
        editor.read_with(cx, |view, _| {
            let Some(ParameterControl::Float { entity, .. }) =
                view.device_panel.controls.get(&ParameterKey {
                    device,
                    parameter: hex_lattice_contract::ParameterId::from_static(parameter),
                })
            else {
                panic!("Utility numeric control is mounted")
            };
            entity.read_with(cx, |control, _| control.display_text().to_owned())
        })
    };
    assert_eq!(readout(1), "0.00 db");
    assert_eq!(readout(2), "c");
    assert_eq!(readout(3), "250%");
    assert_eq!(readout(13), "200 hz");
    let audition = visual
        .debug_bounds(Box::leak(
            format!("utility-bass-mono-audition-{}", device.value()).into_boxed_str(),
        ))
        .expect("headphone audition control");
    assert_eq!(audition.size.width, px(14.0));
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .expect("Utility title");
    for suffix in ["device-bypass", "device-preset-menu", "device-remove"] {
        let selector = Box::leak(format!("{suffix}-{}", device.value()).into_boxed_str());
        let action = visual.debug_bounds(selector).expect("header action");
        assert!(action.origin.x >= frame.origin.x);
        assert!(action.origin.x + action.size.width <= frame.origin.x + frame.size.width);
        assert_eq!(action.size.height, px(12.0));
        assert!(action.origin.y >= frame.origin.y);
        assert!(action.bottom() <= title.top());
    }

    let dial = visual
        .debug_bounds(Box::leak(
            format!("utility-width-dial-{}", device.value()).into_boxed_str(),
        ))
        .expect("width mode affordance");
    let position = point(dial.center().x, dial.center().y);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let menu = visual
        .debug_bounds("context-menu-panel")
        .expect("shared popup");
    let item = visual.debug_bounds("context-menu-item").expect("mode item");
    assert!(menu.origin.x >= px(0.0));
    let select = point(item.center().x, item.center().y);
    visual.simulate_mouse_down(select, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(select, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            state.project.track(track).is_some_and(|track| {
                hex_engine::native_device_parameter_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    track.devices()[0].state(),
                    mid_side,
                ) == Ok(ParameterValue::Bool(true))
            })
        })
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!(
                    "device-{}-parameter-{}-knob",
                    device.value(),
                    mid_side_balance.value()
                )
                .into_boxed_str(),
            ))
            .is_some()
    );
    let mode_key = ParameterKey {
        device,
        parameter: mid_side,
    };
    editor.read_with(cx, |view, _| {
        assert!(
            !view.device_panel.pending_values.contains_key(&mode_key),
            "the acknowledged mode request retires its pending display value"
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Undo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                mid_side,
            ) == Ok(ParameterValue::Bool(false))
        })
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(width_knob).is_some(),
        "undo restores width mode"
    );
    let mid_side_knob = Box::leak(
        format!(
            "device-{}-parameter-{}-knob",
            device.value(),
            mid_side_balance.value()
        )
        .into_boxed_str(),
    );
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.pending_values.contains_key(&mode_key));
    });

    editor.update(cx, |view, cx| {
        assert!(view.enqueue(Request::Redo));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                mid_side,
            ) == Ok(ParameterValue::Bool(true))
        })
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(mid_side_knob).is_some(),
        "redo restores mid/side mode"
    );
    editor.read_with(cx, |view, _| {
        assert!(!view.device_panel.pending_values.contains_key(&mode_key));
    });

    editor.read_with(cx, |view, _| {
        let utility = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                width,
            ),
            Ok(ParameterValue::Float(2.5)),
            "switching modes preserves the hidden width value"
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                mid_side_balance,
            ),
            Ok(ParameterValue::Float(-0.4))
        );
    });
    editor.update(cx, |view, cx| {
        assert!(view.enqueue_device_barrier(Request::SetDeviceParameter {
            device,
            parameter: channel,
            value: ParameterValue::Choice(1),
        }));
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        view.state.as_ref().is_some_and(|state| {
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                state.project.track(track).unwrap().devices()[0].state(),
                channel,
            ) == Ok(ParameterValue::Choice(1))
        })
    });
    editor.read_with(cx, |view, _| {
        let utility = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .state();
        let rendered = hex_engine::native_device_view_for_class(
            hex_engine::NativeDeviceClass::Effect,
            utility,
        )
        .unwrap();
        assert!(
            rendered
                .parameters
                .iter()
                .filter(|parameter| matches!(
                    parameter.descriptor.label,
                    "width" | "mid-side" | "mid-side balance"
                ))
                .all(|parameter| !parameter.enabled)
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                width,
            ),
            Ok(ParameterValue::Float(2.5))
        );
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                hex_engine::NativeDeviceClass::Effect,
                utility,
                mid_side_balance,
            ),
            Ok(ParameterValue::Float(-0.4))
        );
    });
}
