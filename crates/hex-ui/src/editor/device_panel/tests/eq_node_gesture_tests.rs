use super::*;

fn node_window(
    cx: &mut TestAppContext,
    x: f32,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    graph_layout_tests::BrowserRoot,
    DeviceId,
) {
    let (service, state, track, device) = fixture(false);
    let (handle, editor, root) = graph_layout_tests::bounded_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(650.0)));
    visual.run_until_parked();
    let start = plot(&mut visual, device, x, 0.5);
    visual.simulate_click(start, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    (handle, editor, root, device)
}

#[gpui::test]
fn eq_option_bell_drag_changes_only_q_and_has_one_canonical_undo_redo(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.5);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let start = plot(&mut visual, device, 0.5, 0.5);
    let moved = plot(&mut visual, device, 0.72, 0.73);
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(start, MouseButton::Left, option);
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), option);
    // Modifiers are captured at press, including a release before mouse-up.
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].q != before.nodes[0].q
    });
    let after = editor.read_with(cx, |view, _| read(view, device));
    let a = after.nodes[0];
    let b = before.nodes[0];
    assert_eq!(a.id, b.id);
    assert_eq!(a.frequency_hz, b.frequency_hz);
    assert_eq!(a.gain_db, b.gain_db);
    assert!(a.q > b.q && a.q <= 18.0);
    assert_eq!(
        EqState::decode(&after.encode().unwrap()).unwrap(),
        after,
        "current native state roundtrip"
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == after
    });
    let path = std::env::temp_dir().join(format!(
        "hex-eq-option-q-{}-{}.hex",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(path.clone())))
    });
    pump_until(&editor, cx, |view| settled(view) && path.exists());
    let reopened = Service::new(48_000, 8, 8_000_000).unwrap();
    let _ = receive_service_event(&reopened);
    reopened.submit(Request::Open(path.clone())).unwrap();
    let event = receive_service_event(&reopened);
    assert!(event.error.is_none());
    assert_eq!(
        EqState::decode(event.state.project.device(device).unwrap().state()).unwrap(),
        after
    );
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn eq_option_bell_drag_escape_and_blur_restore_without_hover_or_click_edits(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.5);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let start = plot(&mut visual, device, 0.5, 0.5);
    let moved = plot(&mut visual, device, 0.6, 0.3);
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_move(start, None, option);
    visual.simulate_click(start, option);
    visual.run_until_parked();
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
    for blur in [false, true] {
        visual.update(|window, _| window.activate_window());
        visual.simulate_mouse_down(start, MouseButton::Left, option);
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), option);
        visual.run_until_parked();
        if blur {
            visual.update(|window, _| window.blur());
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(moved, MouseButton::Left, option);
        pump_until(&editor, cx, settled);
        assert_eq!(
            editor.read_with(cx, |view, _| read(view, device)),
            before,
            "cancel blur={blur}"
        );
    }
}

fn wheel(
    visual: &mut VisualTestContext,
    position: gpui::Point<gpui::Pixels>,
    delta: gpui::ScrollDelta,
    phase: gpui::TouchPhase,
    inverted: bool,
) {
    visual.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta,
        touch_phase: phase,
        direction_inverted_from_device: inverted,
        ..Default::default()
    });
    visual.run_until_parked();
}
fn end_wheel(visual: &mut VisualTestContext, position: gpui::Point<gpui::Pixels>) {
    wheel(
        visual,
        position,
        gpui::ScrollDelta::default(),
        gpui::TouchPhase::Ended,
        false,
    );
}

#[gpui::test]
fn eq_cut_wheel_fractional_coalescing_preserves_other_fields_and_one_history_entry(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let position = plot(&mut visual, device, 0.02, 0.5);
    for index in 0..12 {
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Pixels(point(px(0.0), px(4.0))),
            if index == 0 {
                gpui::TouchPhase::Started
            } else {
                gpui::TouchPhase::Moved
            },
            false,
        );
        if index == 6 {
            visual.simulate_mouse_move(
                position + point(px(1.0), px(1.0)),
                None,
                Modifiers::default(),
            );
            visual.run_until_parked();
        }
        if index < 5 {
            assert_eq!(
                editor.read_with(cx, |view, _| read(view, device)),
                before,
                "fractional movement is not an edit"
            );
        }
    }
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 2
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 2;
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), expected);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == expected
    });
}

#[gpui::test]
fn eq_cut_wheel_clamps_and_reverses_immediately_with_single_natural_direction_mapping(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let position = plot(&mut visual, device, 0.02, 0.5);
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 8.0)),
        gpui::TouchPhase::Started,
        false,
    );
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 2
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    // The platform inverted this raw delta; physical up is still +one slope.
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        gpui::TouchPhase::Started,
        true,
    );
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
}

#[gpui::test]
fn eq_cut_wheel_escape_restores_and_mouse_wheel_quiet_commits_without_end_event(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let position = plot(&mut visual, device, 0.02, 0.5);
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    pump_until(&editor, cx, settled);
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    visual.run_until_parked();
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}

#[gpui::test]
fn eq_wheel_over_bell_blank_or_modified_input_never_invents_a_shape_or_edit(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.5);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    for x in [0.5, 0.8] {
        let position = plot(&mut visual, device, x, 0.5);
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        end_wheel(&mut visual, position);
    }
    assert_eq!(
        editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone()),
        before
    );
    assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
}

#[gpui::test]
fn eq_old_scroll_stream_cannot_finish_or_rebind_a_new_same_node_gesture(cx: &mut TestAppContext) {
    use hex_widgets::{ResponseGraphScrollEvent as Event, ResponseGraphScrollPhase as Phase};
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let graph = editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device));
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Event>::new()));
    let captured = events.clone();
    let _subscription = graph.update(cx, |_, cx| {
        cx.subscribe(&graph, move |_, _, event: &Event, _| {
            captured.borrow_mut().push(*event)
        })
    });
    let position = plot(&mut visual, device, 0.02, 0.5);
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let old = events
        .borrow()
        .iter()
        .find(|event| event.phase == Phase::Change)
        .copied()
        .unwrap();
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let key = editor.read_with(cx, |view, _| view.device_gesture.snapshot().unwrap().key);
    graph.update(cx, |_, cx| {
        cx.emit(Event {
            phase: Phase::Commit,
            steps: -3,
            ..old
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_gesture.snapshot().unwrap().key,
            key,
            "a delayed terminal from the previous stream cannot finish the new key"
        )
    });
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 2
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
}

#[gpui::test]
fn eq_cut_wheel_preserves_modified_input_and_active_numeric_text_focus(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let position = plot(&mut visual, device, 0.02, 0.5);
    for modifiers in [
        Modifiers {
            alt: true,
            ..Default::default()
        },
        Modifiers {
            platform: true,
            ..Default::default()
        },
        Modifiers {
            control: true,
            ..Default::default()
        },
        Modifiers {
            shift: true,
            ..Default::default()
        },
    ] {
        visual.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            modifiers,
            ..Default::default()
        });
        visual.run_until_parked();
    }
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
    let q = before.nodes[0]
        .id
        .parameter(hex_engine::eq::NodeParameter::Q)
        .unwrap();
    enter_typed_value(&mut visual, device, q, "3");
    visual.run_until_parked();
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    end_wheel(&mut visual, position);
    editor.read_with(cx, |view, app| {
        assert!(
            matches!(view.device_panel.controls.get(&ParameterKey{device,parameter:q}),
            Some(ParameterControl::Float{entity,..}) if entity.read(app).is_editing()),
            "wheel cannot steal typed focus"
        );
        assert_eq!(read(view, device), before);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    pump_until(&editor, cx, settled);
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
}

#[gpui::test]
fn eq_two_cut_nodes_scroll_their_stable_parameters_and_preserve_other_nodes(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let right = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(right, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 2
    });
    let before = editor.read_with(cx, |view, _| read(view, device));
    for (index, x) in [0.02, 0.98].into_iter().enumerate() {
        let position = plot(&mut visual, device, x, 0.5);
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        end_wheel(&mut visual, position);
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device).nodes[index].slope == 1
        });
        let mut expected = before.clone();
        for node in expected.nodes.iter_mut().take(index + 1) {
            node.slope = 1;
        }
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), expected);
    }
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[1].slope == 0
    });
    assert_eq!(
        editor.read_with(cx, |view, _| read(view, device).nodes[0].slope),
        1
    );
}

#[gpui::test]
fn eq_consumed_cut_wheel_cannot_scroll_the_outer_chain(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(650.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        let scroll = &view.device_panel.chain_window.scroll;
        assert!(
            scroll.max_offset().width > px(0.0),
            "fixture must actually have scrollable overflow"
        );
        scroll.set_offset(point(-scroll.max_offset().width / 2.0, px(0.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset());
    let position = plot(&mut visual, device, 0.02, 0.5);
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Pixels(point(px(-48.0), px(24.0))),
        gpui::TouchPhase::Started,
        false,
    );
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
    assert_eq!(
        editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset()),
        before,
        "consumed diagonal parameter wheel must not move the outer chain"
    );
}

#[gpui::test]
fn eq_option_drag_refused_begin_preserves_the_other_canonical_gesture_key(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.5);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let key = editor.update(cx, |view, _| {
        view.begin_device_gesture(
            hex_session::device_gesture::GestureTarget::DeviceParameter {
                device,
                parameter: hex_engine::eq::OUTPUT_GAIN,
            },
            ParameterValue::Float(before.output_gain),
        )
        .unwrap();
        view.device_gesture.snapshot().unwrap().key
    });
    let start = plot(&mut visual, device, 0.5, 0.5);
    let moved = plot(&mut visual, device, 0.7, 0.8);
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(start, MouseButton::Left, option);
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), option);
    visual.simulate_mouse_up(moved, MouseButton::Left, option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.device_gesture.snapshot().unwrap().key, key);
        assert_eq!(read(view, device), before);
        assert!(
            view.device_panel.pending_values.is_empty(),
            "refused physical stream cannot echo Q values"
        );
    });
    editor.update(cx, |view, _| view.cancel_device_gesture().unwrap());
    pump_until(&editor, cx, settled);
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
}

#[gpui::test]
fn eq_review_option_q_moves_both_ways_at_both_gain_boundaries(cx: &mut TestAppContext) {
    let mut completed = 0;
    for gain in [-24.0_f32, 24.0] {
        // Native text selection is strict before the first window of each app.
        // Gain variants must not reuse a context with earlier open windows.
        let cx = &mut cx.new_app();
        cx.update(hex_widgets::bind_numeric_control_keys);
        eprintln!("EQ relative-Q boundary variant: gain={gain}");
        let (handle, editor, _root, device) = node_window(cx, 0.5);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let id = editor.read_with(cx, |view, _| read(view, device).nodes[0].id);
        let gain_parameter = id.parameter(hex_engine::eq::NodeParameter::Gain).unwrap();
        enter_typed_value(&mut visual, device, gain_parameter, &gain.to_string());
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device).nodes[0].gain_db == gain
        });
        for pixels in [-24.0_f32, 24.0] {
            visual.run_until_parked();
            let before = editor.read_with(cx, |view, _| read(view, device));
            // Hit the visible half of a boundary node inside the plot: the
            // bottom/right rectangle edge itself is deliberately exclusive.
            let start = plot(
                &mut visual,
                device,
                0.5,
                ((gain + 24.0) / 48.0).clamp(0.001, 0.999),
            );
            let moved = start + point(px(0.0), px(pixels));
            let option = Modifiers {
                alt: true,
                ..Default::default()
            };
            visual.simulate_mouse_down(start, MouseButton::Left, option);
            visual.simulate_mouse_move(moved, Some(MouseButton::Left), option);
            visual.simulate_mouse_up(moved, MouseButton::Left, option);
            visual.run_until_parked();
            pump_until(&editor, cx, settled);
            let after = editor.read_with(cx, |view, _| read(view, device));
            let a = after.nodes[0];
            let b = before.nodes[0];
            assert_eq!(a.id, b.id);
            assert_eq!(a.frequency_hz, b.frequency_hz);
            assert_eq!(a.gain_db, gain);
            assert!(
                if pixels < 0.0 { a.q > b.q } else { a.q < b.q },
                "gain{gain}, pointerdy{pixels}: Q{} -> {}",
                b.q,
                a.q
            );
            assert!((0.1..=18.0).contains(&a.q));
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump_until(&editor, cx, |view| {
                settled(view) && read(view, device) == before
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
            pump_until(&editor, cx, |view| {
                settled(view) && read(view, device) == after
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump_until(&editor, cx, |view| {
                settled(view) && read(view, device) == before
            });
        }
        completed += 1;
    }
    assert_eq!(
        completed, 2,
        "both gain boundaries completed both drag directions"
    );
}

#[gpui::test]
fn eq_review_horizontal_cut_wheel_bubbles_while_fractional_vertical_still_coalesces(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.0), px(650.0)));
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        let scroll = &view.device_panel.chain_window.scroll;
        assert!(scroll.max_offset().width > px(0.0));
        scroll.set_offset(point(-scroll.max_offset().width / 2.0, px(0.0)));
        cx.notify();
    });
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.02, 0.5);
    let before = editor.read_with(cx, |view, _| read(view, device));
    let offset = editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset());
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Pixels(point(px(-24.0), px(0.0))),
        gpui::TouchPhase::Moved,
        false,
    );
    let moved = editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset());
    assert_ne!(
        moved, offset,
        "horizontal-only cut hover must reach the real outer chain"
    );
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
    assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
    let position = plot(&mut visual, device, 0.02, 0.5);
    visual.simulate_event(gpui::ScrollWheelEvent {
        position,
        delta: gpui::ScrollDelta::Pixels(point(px(-24.0), px(0.0))),
        touch_phase: gpui::TouchPhase::Moved,
        is_momentum: true,
        ..Default::default()
    });
    visual.run_until_parked();
    let momentum_offset =
        editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset());
    assert_ne!(
        momentum_offset, moved,
        "horizontal momentum bubbles to outer chain"
    );
    assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
    assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
    let moved = momentum_offset;
    // Re-resolve the actual moved plot. Fractional vertical intent remains owned,
    // even while no complete slope step has yet been accumulated.
    let position = plot(&mut visual, device, 0.02, 0.5);
    for i in 0..6 {
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Pixels(point(px(0.0), px(4.0))),
            if i == 0 {
                gpui::TouchPhase::Started
            } else {
                gpui::TouchPhase::Moved
            },
            false,
        );
        assert_eq!(
            editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset()),
            moved
        );
        if i < 5 {
            assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
        }
    }
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}

#[gpui::test]
fn eq_review_option_q_reaches_real_bounds_with_captured_outside_plot_drag(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.5);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (delta, q) in [(-400.0_f32, 18.0_f32), (400.0, 0.1)] {
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| read(view, device));
        let start = plot(&mut visual, device, 0.5, 0.5);
        // A captured pointer may leave the plot/window. Its displacement must
        // remain independent of the bounded frequency/gain display coordinates.
        let moved = start + point(px(0.0), px(delta));
        let option = Modifiers {
            alt: true,
            ..Default::default()
        };
        visual.simulate_mouse_down(start, MouseButton::Left, option);
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), option);
        visual.simulate_mouse_up(moved, MouseButton::Left, option);
        visual.run_until_parked();
        pump_until(&editor, cx, settled);
        let after = editor.read_with(cx, |view, _| read(view, device));
        assert_eq!(after.nodes[0].q, q, "captured displacement{delta}");
        assert_eq!(after.nodes[0].frequency_hz, before.nodes[0].frequency_hz);
        assert_eq!(after.nodes[0].gain_db, before.nodes[0].gain_db);
        assert_eq!(after.nodes[0].id, before.nodes[0].id);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before
        });
    }
}

#[gpui::test]
fn eq_review_physical_end_then_momentum_never_restarts_slope_history(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let position = plot(&mut visual, device, 0.02, 0.5);
    let before = editor.read_with(cx, |view, _| read(view, device));
    for phase in [gpui::TouchPhase::Started, gpui::TouchPhase::Moved] {
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Pixels(point(px(0.0), px(12.0))),
            phase,
            false,
        );
    }
    let physical_key = editor.read_with(cx, |view, _| view.device_gesture.snapshot().unwrap().key);
    for phase in [gpui::TouchPhase::Moved, gpui::TouchPhase::Ended] {
        visual.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Lines(point(0.0, 2.0)),
            touch_phase: phase,
            is_momentum: true,
            ..Default::default()
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_gesture.snapshot().unwrap().key,
                physical_key,
                "a momentum phase cannot terminate/rebind the physical key"
            );
        });
    }
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 1
    });
    let after = editor.read_with(cx, |view, _| read(view, device));
    for phase in [gpui::TouchPhase::Moved, gpui::TouchPhase::Ended] {
        visual.simulate_event(gpui::ScrollWheelEvent {
            position,
            delta: gpui::ScrollDelta::Lines(point(0.0, 2.0)),
            touch_phase: phase,
            is_momentum: true,
            ..Default::default()
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(read(view, device), after, "inertia phase={phase:?}");
            assert!(
                view.device_gesture.snapshot().is_none(),
                "inertia cannot allocate a second gesture"
            );
        });
    }
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == after
    });
    // A deliberate mouse wheel has no physical Started phase. It must remain
    // usable immediately after inertia, without a guessed timeout or phase.
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, position);
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[0].slope == 2
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == after
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}

#[gpui::test]
fn eq_review_nearest_unsupported_node_blocks_nearby_cut_and_bubbles(cx: &mut TestAppContext) {
    let mut completed = 0;
    for kind in [
        hex_engine::eq::NodeKind::Bell,
        hex_engine::eq::NodeKind::Notch,
    ] {
        let cx = &mut cx.new_app();
        let (handle, editor, _root, device) = node_window(cx, 0.02);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let bell = plot(&mut visual, device, 0.5, 0.5);
        visual.simulate_click(bell, Modifiers::default());
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device).nodes.len() == 2
        });
        visual.simulate_resize(size(px(880.0), px(650.0)));
        visual.run_until_parked();
        let left = plot(&mut visual, device, 0.02, 0.5);
        let middle = plot(&mut visual, device, 0.5, 0.5);
        let width = f32::from(middle.x - left.x) / 0.48;
        let frequency = 20.0 * 1000.0_f32.powf(0.02 + 4.0 / width);
        let cut_q = editor.read_with(cx, |view, _| read(view, device).nodes[0].q);
        let id = editor.read_with(cx, |view, _| read(view, device).nodes[1].id);
        for (parameter, value) in [
            (
                hex_engine::eq::NodeParameter::Kind,
                ParameterValue::Choice(kind as u32),
            ),
            (
                hex_engine::eq::NodeParameter::Frequency,
                ParameterValue::Float(frequency),
            ),
            (
                hex_engine::eq::NodeParameter::Q,
                ParameterValue::Float(cut_q),
            ),
        ] {
            editor.update(cx, |view, _| {
                assert!(view.enqueue(Request::SetDeviceParameter {
                    device,
                    parameter: id.parameter(parameter).unwrap(),
                    value,
                }))
            });
            pump_until(&editor, cx, settled);
        }
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| read(view, device));
        let cut_y = (before.nodes[0].q / 0.1).ln() / (18.0_f32 / 0.1).ln();
        // Align both real node heights: notch Q uses the cut axis; Bell gain
        // defaults to zero, matching the node created at the plot midpoint.
        let x = (frequency / 20.0).ln() / 1000.0_f32.ln();
        let y = if kind == hex_engine::eq::NodeKind::Bell {
            0.5
        } else {
            cut_y
        };
        let position = plot(&mut visual, device, x, y);
        let cut_position = plot(&mut visual, device, 0.02, cut_y);
        let delta = position - cut_position;
        let distance = (f32::from(delta.x).powi(2) + f32::from(delta.y).powi(2)).sqrt();
        assert!(
            distance > 0.0 && distance < 8.0,
            "actual nodes must overlap hit radii: {distance}"
        );
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        end_wheel(&mut visual, position);
        pump_until(&editor, cx, settled);
        assert_eq!(
            editor.read_with(cx, |view, _| read(view, device)),
            before,
            "nearest unsupported {kind:?} must not edit the obscured cut"
        );
        assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
        editor.update(cx, |view, cx| {
            let scroll = &view.device_panel.chain_window.scroll;
            assert!(scroll.max_offset().width > px(48.0));
            scroll.set_offset(point(-scroll.max_offset().width / 2.0, px(0.0)));
            cx.notify();
        });
        visual.run_until_parked();
        let position = plot(&mut visual, device, x, y);
        let offset = editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset());
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Pixels(point(px(-12.0), px(24.0))),
            gpui::TouchPhase::Moved,
            false,
        );
        assert_ne!(
            editor.read_with(cx, |view, _| view.device_panel.chain_window.scroll.offset()),
            offset,
            "unsupported nearest must bubble diagonal wheel to actual outer chain"
        );
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), before);
        // Reverse nearest ownership within the same overlap. An eligible cut
        // must still work even though an unsupported node is within its radius.
        let position = plot(&mut visual, device, 0.02, cut_y);
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        end_wheel(&mut visual, position);
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device).nodes[0].slope == 1
        });
        let mut expected = before.clone();
        expected.nodes[0].slope = 1;
        assert_eq!(editor.read_with(cx, |view, _| read(view, device)), expected);
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before
        });
        completed += 1;
    }
    assert_eq!(completed, 2, "both Bell and Notch overlap cases executed");
}

#[gpui::test]
fn eq_review_stream_boundary_wheel_outer_hits_but_not_blank_or_outside(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let initial = editor.read_with(cx, |view, _| read(view, device));
    let mid_q = initial.nodes[0].q;
    for (frequency, q, x, y, outward) in [
        (20.0, mid_q, 0.0, 0.5, point(px(-4.0), px(0.0))),
        (20000.0, mid_q, 1.0, 0.5, point(px(4.0), px(0.0))),
        (
            initial.nodes[0].frequency_hz,
            0.1,
            0.02,
            0.0,
            point(px(0.0), px(4.0)),
        ),
        (
            initial.nodes[0].frequency_hz,
            18.0,
            0.02,
            1.0,
            point(px(0.0), px(-4.0)),
        ),
    ] {
        let mut before = initial.clone();
        before.nodes[0].frequency_hz = frequency;
        before.nodes[0].q = q;
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::ReplaceDeviceState {
                device,
                state: before.encode().unwrap()
            })))
        });
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before
        });
        visual.run_until_parked();
        let center = plot(&mut visual, device, x, y);
        let hit = center + outward;
        wheel(
            &mut visual,
            hit,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        end_wheel(&mut visual, hit);
        pump_until(&editor, cx, settled);
        let mut expected = before.clone();
        expected.nodes[0].slope = 1;
        assert_eq!(
            editor.read_with(cx, |view, _| read(view, device)),
            expected,
            "outer visible half frequency{frequency}/q{q}"
        );
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before
        });
        for outside in [
            center + outward.map(|value| value * 2.5),
            hit + point(px(24.0), px(24.0)),
        ] {
            wheel(
                &mut visual,
                outside,
                gpui::ScrollDelta::Lines(point(0.0, 1.0)),
                gpui::TouchPhase::Started,
                false,
            );
            end_wheel(&mut visual, outside);
            pump_until(&editor, cx, settled);
            assert_eq!(
                editor.read_with(cx, |view, _| read(view, device)),
                before,
                "blank/outside full radius must not clamp into node"
            );
        }
    }
}

#[gpui::test]
fn eq_review_stream_physical_pause_keeps_one_history_until_end_including_zero_start(
    cx: &mut TestAppContext,
) {
    let mut completed = 0;
    for zero_start in [false, true] {
        let cx = &mut cx.new_app();
        let (handle, editor, _root, device) = node_window(cx, 0.02);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let before = editor.read_with(cx, |view, _| read(view, device));
        let graph = editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<
            hex_widgets::ResponseGraphScrollEvent,
        >::new()));
        let captured = events.clone();
        let _subscription = graph.update(cx, |_, cx| {
            cx.subscribe(
                &graph,
                move |_, _, event: &hex_widgets::ResponseGraphScrollEvent, _| {
                    captured.borrow_mut().push(*event)
                },
            )
        });
        let position = plot(&mut visual, device, 0.02, 0.5);
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, if zero_start { 0.0 } else { 1.0 })),
            gpui::TouchPhase::Started,
            false,
        );
        if zero_start {
            assert!(editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()));
            assert!(
                events.borrow().is_empty(),
                "zeroY start must not begin/consume a parameter edit"
            );
        }
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
        let first_slope: u8 = if zero_start { 1 } else { 2 };
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture.snapshot().is_some_and(|snapshot| {
                    snapshot.value == ParameterValue::Choice(u32::from(first_slope))
                })
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        visual.run_until_parked();
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.pending_requests.is_empty()
                && view.device_gesture_barriers.is_empty()
        });
        assert!(
            editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_some()),
            "physical fingers still held after >250ms, zero_start={zero_start}"
        );
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Commit)
                .count(),
            0
        );
        wheel(
            &mut visual,
            position,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
        end_wheel(&mut visual, position);
        pump_until(&editor, cx, settled);
        let after = editor.read_with(cx, |view, _| read(view, device));
        assert_eq!(after.nodes[0].slope, first_slope + 1);
        let events = events.borrow();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Begin)
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Commit)
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .all(|event| event.gesture_id == events[0].gesture_id)
        );
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == before
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == after
        });
        completed += 1;
    }
    assert_eq!(completed, 2);
}

#[gpui::test]
fn eq_review_stream_end_outside_graph_retires_key_and_keeps_next_wheel_history_separate(
    cx: &mut TestAppContext,
) {
    use gpui::Focusable;
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let graph = editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device));
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<
        hex_widgets::ResponseGraphScrollEvent,
    >::new()));
    let captured = events.clone();
    let _subscription = graph.update(cx, |_, cx| {
        cx.subscribe(
            &graph,
            move |_, _, event: &hex_widgets::ResponseGraphScrollEvent, _| {
                captured.borrow_mut().push(*event)
            },
        )
    });
    let position = plot(&mut visual, device, 0.02, 0.5);
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view
                .device_gesture
                .snapshot()
                .is_some_and(|snapshot| snapshot.value == ParameterValue::Choice(2))
    });
    let key = editor.read_with(cx, |view, _| view.device_gesture.snapshot().unwrap().key);
    assert!(visual.update(|window, cx| graph.read(cx).focus_handle(cx).is_focused(window)));
    let outside = visual
        .debug_bounds("session-scenes-header")
        .expect("actual other panel header")
        .center();
    let graph_bounds = visual
        .debug_bounds(selector("eq-response", device))
        .unwrap();
    assert!(!graph_bounds.contains(&outside));
    // No click, focus transition, fabricated graph callback or synthetic terminal:
    // ordinary GPUI hit testing now points at the separate Session panel.
    visual.simulate_mouse_move(outside, None, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.update(|window, cx| graph.read(cx).focus_handle(cx).is_focused(window)));
    assert_eq!(
        editor.read_with(cx, |view, _| view.device_gesture.snapshot().unwrap().key),
        key
    );
    end_wheel(&mut visual, outside);
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
    });
    eprintln!(
        "outside_graph_end graph={graph_bounds:?} outside={outside:?} phases={:?} active_key={:?}",
        events.borrow().iter().map(|e| e.phase).collect::<Vec<_>>(),
        editor.read_with(cx, |view, _| view.device_gesture.snapshot().map(|s| s.key))
    );
    assert!(
        editor.read_with(cx, |view, _| view.device_gesture.snapshot().is_none()),
        "physical Ended over another panel must retire the original graph's key"
    );
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Commit)
            .count(),
        1
    );
    let after = editor.read_with(cx, |view, _| read(view, device));
    let mut expected = before.clone();
    expected.nodes[0].slope = 2;
    assert_eq!(after, expected);
    assert!(
        events
            .borrow()
            .iter()
            .all(|e| e.gesture_id == events.borrow()[0].gesture_id)
    );
    // Returning with a deliberate unphased wheel is a NEW gesture, with its
    // existing quiet fallback; two Undo/Redo steps prove the histories separate.
    visual.simulate_mouse_move(position, None, Modifiers::default());
    wheel(
        &mut visual,
        position,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    visual.run_until_parked();
    pump_until(&editor, cx, settled);
    let next = editor.read_with(cx, |view, _| read(view, device));
    expected.nodes[0].slope = 3;
    assert_eq!(next, expected);
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Begin)
            .count(),
        2
    );
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| e.phase == hex_widgets::ResponseGraphScrollPhase::Commit)
            .count(),
        2
    );
    for expected in [&after, &before] {
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == *expected
        });
    }
    for expected in [&after, &next] {
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            settled(view) && read(view, device) == *expected
        });
    }
}

#[gpui::test]
fn four_review_two_cuts_preserve_second_burst_while_real_first_ack_is_held(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let right = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(right, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 2
    });
    let before = editor.read_with(cx, |view, _| read(view, device));
    let left = plot(&mut visual, device, 0.02, 0.5);
    let right = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        left,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    // Submit the actual first-node Begin to the real Service; retain its actual
    // acknowledgement instead of calling tick/receive. Virtual time is unadvanced.
    editor.update(cx, |view, _| assert!(view.pump_device_gesture()));
    let held = editor.read_with(cx, |view, _| receive_service_event(&view.service));
    assert!(held.error.is_none());
    assert!(held.gesture_ack.as_ref().is_some_and(|ack| ack.accepted));
    assert!(editor.read_with(cx, |view, _| view.worker_requests_pending > 0));
    wheel(
        &mut visual,
        right,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, right);
    eprintln!(
        "before releasing actual first ACK: {:?}",
        editor.read_with(cx, |view, _| view.device_panel.message.clone())
    );
    editor.update(cx, |view, _| {
        view.receive(held);
    });
    pump_until(&editor, cx, settled);
    let after = editor.read_with(cx, |view, _| read(view, device));
    eprintln!(
        "two-cut slopes before={:?} after={:?}",
        before.nodes.iter().map(|n| n.slope).collect::<Vec<_>>(),
        after.nodes.iter().map(|n| n.slope).collect::<Vec<_>>()
    );
    let mut expected = before.clone();
    expected.nodes[0].slope = 1;
    expected.nodes[1].slope = 1;
    assert_eq!(
        after, expected,
        "a consumed second-node burst must reach its own canonical parameter"
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes[1].slope == 0
    });
    assert_eq!(
        editor.read_with(cx, |view, _| read(view, device).nodes[0].slope),
        1
    );
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
}

fn held_real_scroll_begin(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) -> Event {
    editor.update(cx, |view, _| assert!(view.pump_device_gesture()));
    let event = editor.read_with(cx, |view, _| receive_service_event(&view.service));
    assert!(event.error.is_none());
    assert!(event.gesture_ack.as_ref().is_some_and(
        |ack| ack.accepted && ack.phase == hex_session::device_gesture::GesturePhase::Begin
    ));
    event
}

#[gpui::test]
fn four_review_three_cut_fifo_and_return_to_a_use_installed_state_and_four_undos(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (x, count) in [(0.98, 2), (0.5, 3)] {
        let p = plot(&mut visual, device, x, 0.5);
        visual.simulate_click(p, Modifiers::default());
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == count
        });
    }
    let third = editor.read_with(cx, |v, _| read(v, device).nodes[2].id);
    editor.update(cx, |v, _| {
        assert!(
            v.enqueue(Request::SetDeviceParameter {
                device,
                parameter: third
                    .parameter(hex_engine::eq::NodeParameter::Kind)
                    .unwrap(),
                value: ParameterValue::Choice(hex_engine::eq::NodeKind::LowCut as u32),
            })
        )
    });
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    let c_y = (before.nodes[2].q / 0.1).ln() / (18.0_f32 / 0.1).ln();
    let c = plot(&mut visual, device, 0.5, c_y);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let held = held_real_scroll_begin(&editor, cx);
    for p in [b, c, a] {
        wheel(
            &mut visual,
            p,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
    }
    end_wheel(&mut visual, a);
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (3, true)
    );
    assert_eq!(editor.read_with(cx, |v, _| read(v, device)), before);
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 2;
    expected.nodes[1].slope = 1;
    expected.nodes[2].slope = 1;
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (0, false)
    );
    let after = expected.clone();
    for index in [0, 2, 1, 0] {
        expected.nodes[index].slope -= 1;
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    }
    assert_eq!(expected, before);
    for _ in 0..4 {
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Redo)));
        pump_until(&editor, cx, settled);
    }
    assert_eq!(editor.read_with(cx, |v, _| read(v, device)), after);
}

#[gpui::test]
fn four_review_queued_saturation_keeps_reversal_before_and_after_replay(cx: &mut TestAppContext) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let b = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(b, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let held = held_real_scroll_begin(&editor, cx);
    wheel(
        &mut visual,
        b,
        gpui::ScrollDelta::Lines(point(0.0, 10.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    wheel(
        &mut visual,
        b,
        gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (1, true)
    );
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    // Do not end physical B yet: replay must continue the SAME stream and
    // preserve its original cumulative steps when subsequent input arrives.
    pump_until(&editor, cx, |v| {
        v.worker_requests_pending == 0
            && v.device_gesture
                .snapshot()
                .is_some_and(|s| s.value == ParameterValue::Choice(2))
    });
    wheel(
        &mut visual,
        b,
        gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, b);
    let mut expected = before.clone();
    expected.nodes[0].slope = 1;
    expected.nodes[1].slope = 1;
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    for index in [1, 0] {
        expected.nodes[index].slope = 0;
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    }
    assert_eq!(expected, before);
}

#[gpui::test]
fn four_review_cancel_and_later_blur_retire_queued_streams_without_committing_them(
    cx: &mut TestAppContext,
) {
    for blur_after_end in [false, true] {
        let cx = &mut cx.new_app();
        let (handle, editor, _root, device) = node_window(cx, 0.02);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let b = plot(&mut visual, device, 0.98, 0.5);
        visual.simulate_click(b, Modifiers::default());
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == 2
        });
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| read(v, device));
        let a = plot(&mut visual, device, 0.02, 0.5);
        let b = plot(&mut visual, device, 0.98, 0.5);
        wheel(
            &mut visual,
            a,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        let held = held_real_scroll_begin(&editor, cx);
        wheel(
            &mut visual,
            b,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
        if blur_after_end {
            end_wheel(&mut visual, b);
            assert_eq!(
                editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
                (1, true)
            );
            visual.update(|window, _| window.blur());
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        assert_eq!(
            editor
                .read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device))
                .0,
            0
        );
        editor.update(cx, |v, _| {
            v.receive(held);
        });
        let mut expected = before.clone();
        expected.nodes[0].slope = 1;
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
        assert_eq!(
            editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
            (0, false)
        );
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == before);
    }
}

#[gpui::test]
fn four_review_real_worker_refusal_and_context_changes_never_replay_stale_queue(
    cx: &mut TestAppContext,
) {
    for mode in ["remove", "project", "variant", "source"] {
        let cx = &mut cx.new_app();
        let (handle, editor, _root, device) = node_window(cx, 0.02);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let b = plot(&mut visual, device, 0.98, 0.5);
        visual.simulate_click(b, Modifiers::default());
        pump_until(&editor, cx, |v| {
            settled(v) && read(v, device).nodes.len() == 2
        });
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| read(v, device));
        let a = plot(&mut visual, device, 0.02, 0.5);
        let b = plot(&mut visual, device, 0.98, 0.5);
        wheel(
            &mut visual,
            a,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Started,
            false,
        );
        let held = held_real_scroll_begin(&editor, cx);
        wheel(
            &mut visual,
            b,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
        end_wheel(&mut visual, b);
        assert_eq!(
            editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
            (1, true)
        );
        let request = match mode {
            "remove" => Request::Edit(Command::RemoveDevice(device)),
            "project" => Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            },
            "variant" => editor.read_with(cx, |v, _| Request::CompareDevice {
                owner: hex_session::service::preset::Owner {
                    project_generation: v.project_generation,
                    revision: v.state.as_ref().unwrap().revision,
                    device,
                    class: NativeDeviceClass::Effect,
                    type_id: hex_engine::eq::EQ_TYPE_ID,
                },
                expected: hex_project::DeviceVariant::A,
                action: hex_project::DeviceComparisonAction::Switch,
            }),
            "source" => Request::SetDeviceParameter {
                device,
                parameter: before.nodes[1]
                    .id
                    .parameter(hex_engine::eq::NodeParameter::Frequency)
                    .unwrap(),
                value: ParameterValue::Float(1000.0),
            },
            _ => unreachable!(),
        };
        // Actual external control-plane race: the real Service processes this
        // context change BEFORE the UI's now-released predecessor Commit.
        // No synthetic Event, ACK, preview, or canonical project is installed.
        editor.update(cx, |v, _| {
            v.service.submit(request).unwrap();
            v.receive(held);
            assert!(v.pump_device_gesture());
        });
        let changed = editor.read_with(cx, |v, _| receive_service_event(&v.service));
        assert!(
            changed.error.is_none(),
            "context {mode}: {:?}",
            changed.error
        );
        let mut expected = changed.state.project.clone();
        let mut expected_revision = changed.state.revision;
        match mode {
            "remove" => assert!(expected.device(device).is_none()),
            "project" => assert_eq!(expected.tracks().len(), 0),
            "variant" => assert_eq!(
                expected.device(device).unwrap().active_variant(),
                hex_project::DeviceVariant::B
            ),
            "source" => {
                let current = EqState::decode(expected.device(device).unwrap().state()).unwrap();
                assert_eq!(current.nodes[1].frequency_hz, 1000.0);
                assert_eq!(current.nodes[1].slope, 0);
            }
            _ => unreachable!(),
        }
        let refused = editor.read_with(cx, |v, _| receive_service_event(&v.service));
        assert!(refused.gesture_request);
        let ack = refused.gesture_ack.as_ref().unwrap();
        assert_eq!(ack.phase, hex_session::device_gesture::GesturePhase::Commit);
        eprintln!(
            "actual worker terminal {mode}: accepted={} reason={:?}",
            ack.accepted, ack.reason
        );
        if mode == "source" {
            // An unrelated parameter edit need not cancel the already-captured
            // A command. It MUST invalidate queued B, preserving both real
            // authored edits in their actual order without retargeting A.
            assert!(ack.accepted && ack.reason.is_none());
            let mut state = EqState::decode(expected.device(device).unwrap().state()).unwrap();
            state.nodes[0].slope = 1;
            let mut oracle = Editor::new(expected.as_ref().clone(), 2).unwrap();
            oracle
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: state
                        .encode()
                        .unwrap()
                        .with_host_metadata_from(expected.device(device).unwrap().state())
                        .unwrap(),
                })
                .unwrap();
            expected = Arc::new(oracle.project().clone());
            expected_revision += 1;
        } else {
            assert!(!ack.accepted && ack.reason.is_some());
        }
        assert_eq!(refused.state.project, expected);
        assert_eq!(refused.state.revision, expected_revision);
        editor.update(cx, |v, _| {
            v.receive(changed);
            v.receive(refused);
        });
        visual.run_until_parked();
        pump_until(&editor, cx, settled);
        editor.read_with(cx, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project,
                expected,
                "no stale successor authored for {mode}"
            );
            assert_eq!(v.state.as_ref().unwrap().revision, expected_revision);
            assert!(v.device_gesture.snapshot().is_none());
            if v.state.as_ref().unwrap().project.device(device).is_some() {
                assert_eq!(eq_panel::queued_scrolls_for_test(v, device), (0, false));
                assert_eq!(read(v, device).nodes[1].slope, 0);
            }
        });
    }
}

#[gpui::test]
fn four_review_noop_predecessor_preserves_successor_and_does_not_add_history(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let b = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(b, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 3.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let held = held_real_scroll_begin(&editor, cx);
    for p in [b, a, b] {
        wheel(
            &mut visual,
            p,
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
    }
    end_wheel(&mut visual, b);
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 3;
    expected.nodes[1].slope = 2;
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    for (index, slope) in [(1, 1), (1, 0), (0, 0)] {
        expected.nodes[index].slope = slope;
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    }
    assert_eq!(
        expected, before,
        "the saturated A→A no-op must not add an extra history item"
    );
}

// This capacity oracle drains real FIFO edits, rather than benchmarking a full
// window repaint for every worker poll. Input and the settled result still paint.
fn pump_capacity_replay(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    expected: &EqState,
    device: DeviceId,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
        });
        if editor.read_with(cx, |view, _| {
            settled(view) && read(view, device) == *expected
        }) {
            editor.update(cx, |_, cx| cx.notify());
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "capacity replay did not complete: {}",
            editor.read_with(cx, |view, _| format!(
                "worker={} queued={} gesture={:?} barriers={} error={:?} revision={:?}",
                view.worker_requests_pending,
                view.pending_requests.len(),
                view.device_gesture.snapshot(),
                view.device_gesture_barriers.len(),
                view.error,
                view.state.as_ref().map(|state| state.revision),
            ))
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn four_review_stream_capacity_refuses_whole_new_burst_and_preserves_accepted_history(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let b = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(b, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let held = held_real_scroll_begin(&editor, cx);
    for index in 0..crate::editor::UI_REQUEST_CAPACITY {
        wheel(
            &mut visual,
            if index % 2 == 0 { b } else { a },
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
    }
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (crate::editor::UI_REQUEST_CAPACITY, true)
    );
    // A new negative B stream would measurably undo its saturated value if it
    // were partially admitted. Capacity must refuse it, not replace an older B.
    wheel(
        &mut visual,
        b,
        gpui::ScrollDelta::Lines(point(0.0, -1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, b);
    assert!(editor.read_with(cx, |v, _| {
        v.device_panel
            .message
            .as_deref()
            .is_some_and(|s| s.contains("too many pending node edits"))
    }));
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (crate::editor::UI_REQUEST_CAPACITY, true)
    );
    assert_eq!(editor.read_with(cx, |v, _| read(v, device)), before);
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 3;
    expected.nodes[1].slope = 3;
    pump_capacity_replay(&editor, cx, &expected, device);
    visual.run_until_parked();
    assert!(editor.read_with(cx, |v, _| settled(v) && read(v, device) == expected));
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (0, false)
    );
    for index in [1, 0, 1, 0, 1, 0] {
        expected.nodes[index].slope -= 1;
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    }
    assert_eq!(
        expected, before,
        "only the six actual value changes add history"
    );
}

#[gpui::test]
fn four_review_stale_cancel_cannot_discard_a_newer_queued_stream(cx: &mut TestAppContext) {
    use hex_widgets::{ResponseGraphScrollEvent as Event, ResponseGraphScrollPhase as Phase};
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let b = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(b, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let graph = editor.read_with(cx, |v, _| eq_panel::graph_for_test(v, device));
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Event>::new()));
    let captured = events.clone();
    let _subscription = graph.update(cx, |_, cx| {
        cx.subscribe(&graph, move |_, _, e: &Event, _| {
            captured.borrow_mut().push(*e)
        })
    });
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let old = events
        .borrow()
        .iter()
        .find(|e| e.phase == Phase::Change)
        .copied()
        .unwrap();
    let held = held_real_scroll_begin(&editor, cx);
    wheel(
        &mut visual,
        b,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, b);
    // Controlled stale typed graph delivery, as in the existing stale-stream
    // regression. No worker acknowledgement is fabricated or replaced.
    graph.update(cx, |_, cx| {
        cx.emit(Event {
            phase: Phase::Cancel,
            ..old
        });
        cx.emit(Event {
            phase: Phase::Begin,
            steps: 0,
            ..old
        });
    });
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (1, true)
    );
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 1;
    expected.nodes[1].slope = 1;
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    for index in [1, 0] {
        expected.nodes[index].slope = 0;
        editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
        pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    }
}

#[gpui::test]
fn four_review_escape_on_refused_overflow_stream_cancels_only_queued_context(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let b = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(b, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let held = held_real_scroll_begin(&editor, cx);
    for index in 0..=crate::editor::UI_REQUEST_CAPACITY {
        wheel(
            &mut visual,
            if index % 2 == 0 { b } else { a },
            gpui::ScrollDelta::Lines(point(0.0, 1.0)),
            gpui::TouchPhase::Moved,
            false,
        );
    }
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (crate::editor::UI_REQUEST_CAPACITY, true)
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_eq!(
        editor
            .read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device))
            .0,
        0
    );
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 1;
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == before);
}

#[gpui::test]
fn four_review_blurred_queue_cannot_readmit_a_late_old_begin(cx: &mut TestAppContext) {
    use hex_widgets::{ResponseGraphScrollEvent as Event, ResponseGraphScrollPhase as Phase};
    let (handle, editor, _root, device) = node_window(cx, 0.02);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let b = plot(&mut visual, device, 0.98, 0.5);
    visual.simulate_click(b, Modifiers::default());
    pump_until(&editor, cx, |v| {
        settled(v) && read(v, device).nodes.len() == 2
    });
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| read(v, device));
    let graph = editor.read_with(cx, |v, _| eq_panel::graph_for_test(v, device));
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Event>::new()));
    let captured = events.clone();
    let _subscription = graph.update(cx, |_, cx| {
        cx.subscribe(&graph, move |_, _, e: &Event, _| {
            captured.borrow_mut().push(*e)
        })
    });
    let a = plot(&mut visual, device, 0.02, 0.5);
    let b = plot(&mut visual, device, 0.98, 0.5);
    wheel(
        &mut visual,
        a,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Started,
        false,
    );
    let old = events
        .borrow()
        .iter()
        .find(|e| e.phase == Phase::Begin)
        .copied()
        .unwrap();
    let held = held_real_scroll_begin(&editor, cx);
    wheel(
        &mut visual,
        b,
        gpui::ScrollDelta::Lines(point(0.0, 1.0)),
        gpui::TouchPhase::Moved,
        false,
    );
    end_wheel(&mut visual, b);
    visual.update(|window, _| window.blur());
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (0, true)
    );
    // Controlled stale typed delivery after the real blur, using the actual old
    // graph identity. Clearing intent must not reset its monotonic watermark.
    graph.update(cx, |_, cx| cx.emit(old));
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |v, _| eq_panel::queued_scrolls_for_test(v, device)),
        (0, true)
    );
    editor.update(cx, |v, _| {
        v.receive(held);
    });
    let mut expected = before.clone();
    expected.nodes[0].slope = 1;
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == expected);
    editor.update(cx, |v, _| assert!(v.enqueue(Request::Undo)));
    pump_until(&editor, cx, |v| settled(v) && read(v, device) == before);
}
