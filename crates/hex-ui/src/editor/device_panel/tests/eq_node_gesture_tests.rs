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
