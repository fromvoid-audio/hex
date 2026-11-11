use super::*;

#[gpui::test]
fn device_chain_navigation_pan_overview_release_escape_and_resize(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, mut state, track, _, _, _) = actor_utility_fixture();
    for _ in 0..8 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                    "hex.utility",
                )),
                before: None,
            })
            .expect("append utility");
        state = receive_service_event(&service).state;
    }
    let ids: Vec<_> = state
        .project
        .track(track)
        .unwrap()
        .devices()
        .iter()
        .map(|d| d.id())
        .collect();
    let first = ids[0];
    let last = *ids.last().unwrap();
    let card_selector = Box::leak(
        format!(
            "device-drop-card-{}",
            state.project.track(track).expect("track").devices()[2]
                .id()
                .value()
        )
        .into_boxed_str(),
    );
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let rail = visual.debug_bounds("device-chain-scroll").expect("rail");
    let card_before = visual
        .debug_bounds(card_selector)
        .expect("visible device card");
    let start = rail.center();
    // Begin over a device, then leave the rail while holding the middle button.
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    let outside = point(start.x - px(200.0), rail.origin.y - px(20.0));
    visual.simulate_mouse_move(outside, Some(MouseButton::Middle), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(-200.0));
        assert!(
            view.last_enqueued_edit.is_none(),
            "panning must not edit the project"
        );
    });
    let card_after = visual
        .debug_bounds(card_selector)
        .expect("panned device card");
    assert_eq!(card_after.origin.x, card_before.origin.x - px(200.0));
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    visual.simulate_mouse_move(start, None, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        assert!(
            !view.device_panel.navigation.cancel(),
            "outside release ends capture"
        )
    });
    let overview = visual
        .debug_bounds("device-chain-overview")
        .expect("overview");
    assert!(
        overview.right() <= px(880.0),
        "navigator fits minimum window"
    );
    assert!(
        visual
            .debug_bounds("device-panel-bottom-toggle")
            .expect("bottom toggle")
            .right()
            <= px(880.0)
    );
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: overview.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-75.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(-275.0))
    });
    let right = point(overview.right() - px(1.0), overview.center().y);
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let scroll = &view.device_panel.navigation.scroll;
        assert_eq!(
            scroll.offset().x,
            -scroll.max_offset().width,
            "click reaches final device"
        );
    });
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("effect-device-{}", last.value()).into_boxed_str()
            ))
            .is_some(),
        "navigator end mounts the actual final card"
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-spacer-{}", first.value()).into_boxed_str()
            ))
            .is_some(),
        "offscreen first card is windowed after navigation"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.navigation.scroll.offset(),
            view.device_panel.chain_window.scroll.offset()
        );
        assert_eq!(
            view.device_panel.navigation.scroll.max_offset(),
            view.device_panel.chain_window.scroll.max_offset()
        );
    });
    // Drag the focus frame all the way back to the beginning.
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    let left = point(overview.origin.x - px(100.0), overview.center().y);
    visual.simulate_mouse_move(left, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(left, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0))
    });
    visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_move(outside, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0))
    });
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let old_max = editor.read_with(cx, |view, _| {
        view.device_panel.navigation.scroll.max_offset().width
    });
    visual.simulate_resize(size(px(1800.0), px(760.0)));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let scroll = &view.device_panel.navigation.scroll;
        assert!(
            scroll.max_offset().width < old_max,
            "wider viewport reduces overflow"
        );
        assert_eq!(
            scroll.offset().x,
            -scroll.max_offset().width,
            "resize clamps focus at chain end"
        );
    });
}

#[gpui::test]
fn device_chain_bottom_toggle_preserves_return_owner_and_short_chain_is_safe(
    cx: &mut TestAppContext,
) {
    let (service, mut state, track, return_track, _, _) = actor_return_utility_fixture();
    for _ in 0..8 {
        service
            .submit(Request::Edit(Command::InsertReturnDevice {
                return_track,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .expect("utility"),
            }))
            .expect("append return device");
        state = receive_service_event(&service).state;
    }
    let devices = state
        .project
        .return_tracks()
        .iter()
        .find(|r| r.id() == return_track)
        .unwrap()
        .devices();
    let first = devices[0].id();
    let last = devices.last().unwrap().id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            assert!(view.focus_return_details(return_track, window, cx));
            cx.notify();
        });
    })
    .expect("focus return");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_resize(size(px(2400.0), px(760.0)));
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .expect("bottom toggle")
        .center();
    for expected_open in [false, true] {
        visual.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.clip_view_open, expected_open);
            assert_eq!(view.focused_return, Some(return_track));
        });
        assert_eq!(
            visual.debug_bounds("device-chain-scroll").is_some(),
            expected_open
        );
        assert!(visual.debug_bounds("device-chain-overview").is_some());
    }
    let rail = visual
        .debug_bounds("device-chain-scroll")
        .expect("short rail");
    visual.simulate_mouse_down(rail.center(), MouseButton::Middle, Modifiers::default());
    let far = point(rail.center().x - px(10000.0), rail.center().y);
    visual.simulate_mouse_move(far, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(far, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0))
    });
    visual.simulate_resize(size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let rail = visual
        .debug_bounds("device-chain-scroll")
        .expect("overflowing return rail");
    visual.simulate_mouse_down(rail.center(), MouseButton::Middle, Modifiers::default());
    let moved = point(rail.center().x - px(100.0), rail.center().y);
    visual.simulate_mouse_move(moved, Some(MouseButton::Middle), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Middle, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(-100.0))
    });
    let overview = visual.debug_bounds("device-chain-overview").unwrap();
    let right = point(overview.right() - px(1.0), overview.center().y);
    visual.simulate_mouse_down(right, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(right, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("return-device-drop-card-{}", last.value()).into_boxed_str()
            ))
            .is_some()
    );
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("device-spacer-{}", first.value()).into_boxed_str()
            ))
            .is_some()
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.navigation.scroll.offset(),
            view.device_panel.chain_window.scroll.offset()
        );
        assert_eq!(
            view.device_panel.navigation.scroll.max_offset(),
            view.device_panel.chain_window.scroll.max_offset()
        );
        assert!(
            view.last_enqueued_edit.is_none(),
            "Return navigation does not author history"
        );
    });
    assert_eq!(
        visual.debug_bounds("output-meter").unwrap().size,
        size(
            px(crate::output_meter::WIDTH),
            px(crate::output_meter::HEIGHT)
        )
    );
    // Changing to the empty regular chain retires the return navigation identity.
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Track(track))
        );
        assert_eq!(view.device_panel.navigation.scroll.offset().x, px(0.0));
    });
}

#[gpui::test]
fn workspace_three_returns_device_menu_from_root_or_track_focus(cx: &mut TestAppContext) {
    for header_focus in [false, true] {
        let cx = &mut cx.new_app();
        cx.update(crate::editor::bind_editor_keys);
        let (service, state, track, _, _, _) = actor_utility_fixture();
        let (handle, editor) = actor_editor_window(cx, service, state, track);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            window.activate_window();
            editor.update(cx, |v, cx| {
                v.mode = crate::editor::EditorMode::Arrangement;
                v.focus.focus(window);
                cx.notify();
            });
        });
        visual.run_until_parked();
        // Configure both real keyboard contexts explicitly; the prior pointer
        // fixture did not establish header focus, so it cannot prove that route.
        if header_focus {
            visual.update(|window, cx| {
                editor.update(cx, |v, cx| v.track_header_focus(track, cx).focus(window));
            });
        }
        visual.update(|window, cx| {
            editor.read_with(cx, |v, _| {
                if header_focus {
                    assert!(v.track_header_focused(window));
                } else {
                    assert!(v.focus.is_focused(window));
                }
            });
        });
        let add = visual.debug_bounds("device-add-toggle").unwrap().center();
        visual.simulate_click(add, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("device-add-menu").is_some());
        let before = editor.read_with(&visual, |v, _| {
            assert!(v.device_panel.add_menu_open);
            v.state.as_ref().unwrap().clone()
        });
        visual.update(|window, cx| {
            editor.read_with(cx, |v, _| {
                assert!(!v.focus.is_focused(window));
                assert!(
                    window
                        .context_stack()
                        .iter()
                        .any(|context| context.contains("hex_device_picker"))
                );
                assert!(
                    window
                        .context_stack()
                        .iter()
                        .any(|context| context.contains("hex_text_input"))
                );
            })
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.mode,
                crate::editor::EditorMode::Arrangement,
                "menu header={header_focus}"
            );
            assert!(v.device_panel.add_menu_open);
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        visual.simulate_click(add, Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(!v.device_panel.add_menu_open));
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, crate::editor::EditorMode::Session)
        });
    }
}

#[gpui::test]
fn workspace_three_returns_device_navigation_preserves_capture(cx: &mut TestAppContext) {
    for overview in [false, true] {
        let cx = &mut cx.new_app();
        cx.update(crate::editor::bind_editor_keys);
        let (service, mut state, track, _, _, _) = actor_utility_fixture();
        for _ in 0..8 {
            service
                .submit(Request::AddNativeDevice {
                    track,
                    class: NativeDeviceClass::Effect,
                    type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                        "hex.utility",
                    )),
                    before: None,
                })
                .unwrap();
            state = receive_service_event(&service).state;
        }
        let (handle, editor) = actor_editor_window(cx, service, state, track);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.update(|window, cx| {
            window.activate_window();
            editor.read(cx).focus.focus(window);
        });
        visual.run_until_parked();
        let selector = if overview {
            "device-chain-overview"
        } else {
            "device-chain-scroll"
        };
        let start = visual.debug_bounds(selector).unwrap().center();
        let button = if overview {
            MouseButton::Left
        } else {
            MouseButton::Middle
        };
        let end = start - point(px(20.0), px(0.0));
        visual.simulate_mouse_down(start, button, Modifiers::default());
        visual.simulate_mouse_move(end, button, Modifiers::default());
        let (before, offset) = editor.read_with(&visual, |v, _| {
            assert!(v.device_panel.navigation.is_active());
            (
                v.state.as_ref().unwrap().clone(),
                v.device_panel.navigation.scroll.offset(),
            )
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.mode,
                crate::editor::EditorMode::Session,
                "overview={overview}"
            );
            assert!(v.device_panel.navigation.is_active());
            assert_eq!(v.device_panel.navigation.scroll.offset(), offset);
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        visual.simulate_mouse_up(end, button, Modifiers::default());
        editor.update(&mut visual, |v, _| {
            assert!(
                !v.device_panel.navigation.cancel(),
                "released capture already retired"
            )
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, crate::editor::EditorMode::Arrangement)
        });
    }
}

#[gpui::test]
fn workspace_four_returns_hidden_add_menu_does_not_block_tab(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| {
        window.activate_window();
        editor.update(cx, |v, cx| {
            v.mode = crate::editor::EditorMode::Arrangement;
            v.focus.focus(window);
            cx.notify();
        });
    });
    visual.run_until_parked();
    let add = visual.debug_bounds("device-add-toggle").unwrap().center();
    visual.simulate_click(add, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_some());
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, crate::editor::EditorMode::Arrangement);
        assert!(v.device_panel.add_menu_open);
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    // Cancel the visible picker before using another control. Hidden-state
    // coverage below deliberately installs the stale flag, independent of UI placement.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .unwrap()
        .center();
    visual.simulate_click(toggle, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    editor.read_with(&visual, |v, _| assert!(!v.clip_view_open));
    editor.update(&mut visual, |v, cx| {
        v.device_panel.add_menu_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    visual.update(|window, cx| editor.read_with(cx, |v, _| assert!(v.focus.is_focused(window))));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.mode,
            crate::editor::EditorMode::Session,
            "hidden menu must not own Tab"
        );
        assert_eq!(v.state.as_ref().unwrap().project, before.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
    // Reopening makes the retained menu visible and its local refusal applies again.
    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .unwrap()
        .center();
    visual.simulate_click(toggle, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_some());
    visual.update(|window, _| {
        let contexts = window.context_stack();
        assert!(
            contexts
                .iter()
                .any(|context| context.contains("hex_device_picker"))
        );
        assert!(
            contexts
                .iter()
                .any(|context| context.contains("hex_text_input"))
        );
    });
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, crate::editor::EditorMode::Session)
    });
    let add = visual.debug_bounds("device-add-toggle").unwrap().center();
    visual.simulate_click(add, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, crate::editor::EditorMode::Arrangement)
    });
}

#[gpui::test]
fn workspace_adb3_returns_removed_last_track_menu_does_not_own_tab(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    assert_eq!(state.project.tracks().len(), 1);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| {
        window.activate_window();
        editor.update(cx, |v, cx| {
            v.mode = crate::editor::EditorMode::Arrangement;
            v.focus.focus(window);
            cx.notify();
        });
    });
    visual.run_until_parked();
    let add = visual.debug_bounds("device-add-toggle").unwrap().center();
    visual.simulate_click(add, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_some());
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, crate::editor::EditorMode::Arrangement)
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    let remove = visual
        .debug_bounds("remove selected track")
        .unwrap()
        .center();
    visual.simulate_click(remove, Modifiers::default());
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0 && v.pending_requests.is_empty()
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    assert!(visual.debug_bounds("device-add-toggle").is_none());
    // Preserve the legacy stale-menu regression after the rendered owner is gone.
    editor.update(&mut visual, |v, cx| {
        v.device_panel.add_menu_open = true;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_none());
    let before = editor.read_with(&visual, |v, _| {
        assert!(v.state.as_ref().unwrap().project.tracks().is_empty());
        assert!(v.selected_track.is_none());
        assert!(v.clip_view_open && v.device_panel_active && v.device_panel.add_menu_open);
        v.state.as_ref().unwrap().clone()
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.mode,
            crate::editor::EditorMode::Session,
            "a menu without a rendered owner must not own Tab"
        );
        assert_eq!(v.state.as_ref().unwrap().project, before.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0 && v.pending_requests.is_empty()
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(v.state.as_ref().unwrap().project.track(track).is_some());
            v.selected_track = Some(track);
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_some());
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, crate::editor::EditorMode::Session)
    });
    let add = visual.debug_bounds("device-add-toggle").unwrap().center();
    visual.simulate_click(add, Modifiers::default());
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, crate::editor::EditorMode::Arrangement)
    });
}

#[gpui::test]
fn workspace_adb3_returns_gain_point_drag_keeps_tab_until_release_or_escape(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    for cancel in [false, true] {
        let (service, _, track, _, _, _) = actor_utility_fixture();
        service
            .submit(Request::Edit(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ZERO,
                        gain: 1.0,
                        outgoing: hex_project::GainInterpolation::Linear,
                    },
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ONE,
                        gain: 0.5,
                        outgoing: hex_project::GainInterpolation::Linear,
                    },
                ],
            }))
            .unwrap();
        let state = receive_service_event(&service).state;
        let (handle, editor) = actor_editor_window(cx, service, state, track);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            window.activate_window();
            editor.update(cx, |v, cx| {
                v.mode = crate::editor::EditorMode::Arrangement;
                v.focus.focus(window);
                cx.notify();
            });
        });
        visual.run_until_parked();
        let show = visual
            .debug_bounds(Box::leak(
                format!("gain-lane-show-{}", track.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        visual.simulate_click(show, Modifiers::default());
        visual.run_until_parked();
        let plot = visual.debug_bounds("gain-lane-plot").unwrap();
        let start = point(plot.origin.x + px(0.5), plot.center().y);
        let end = start - point(px(0.0), px(10.0));
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        let draft = editor.read_with(&visual, |v, _| {
            assert!(v.gain_lane.draft.is_some());
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            format!("{:?}", v.gain_lane.draft)
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.mode,
                crate::editor::EditorMode::Arrangement,
                "held gain point owns Tab; cancel={cancel}"
            );
            assert_eq!(format!("{:?}", v.gain_lane.draft), draft);
            assert_eq!(v.state.as_ref().unwrap().project, before.project);
            assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        if cancel {
            visual.simulate_keystrokes("escape");
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |v| {
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.gain_lane.draft.is_none());
            v.state.as_ref().unwrap().clone()
        });
        if cancel {
            assert_eq!(after.project, before.project);
            assert_eq!(after.revision, before.revision);
        } else {
            assert_ne!(after.project, before.project);
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            pump_until(&editor, cx, |v| {
                v.worker_requests_pending == 0 && v.pending_requests.is_empty()
            });
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project, before.project)
            });
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
            pump_until(&editor, cx, |v| {
                v.worker_requests_pending == 0 && v.pending_requests.is_empty()
            });
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project, after.project)
            });
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.pending_requests.is_empty());
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, crate::editor::EditorMode::Session)
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.mode, crate::editor::EditorMode::Arrangement)
        });
    }
}
