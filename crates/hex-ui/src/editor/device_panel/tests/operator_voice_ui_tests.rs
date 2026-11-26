use super::*;
use hex_lattice_contract::ParameterId;

fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[gpui::test]
fn primary_voices_preserve_loaded_counts_context_and_single_edit_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: ParameterId::from_static(601),
            value: ParameterValue::Choice(2),
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let initial_revision = state.revision;
    let mode = operator_parameter_value(&state, track, device, 608);
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field_id = selector(format!("device-{}-parameter-601", device.value()));
    let popup_id = selector(format!("device-{}-parameter-601-popup", device.value()));
    let field = visual
        .debug_bounds(field_id)
        .expect("voice selector on envelope page");
    let preview = visual
        .debug_bounds(selector(format!("operator-preview-{}", device.value())))
        .unwrap();
    assert!(preview.contains(&field.center()));
    assert!(field.bottom() <= preview.bottom());
    assert!(preview.size.height <= px(200.0));
    let entity = editor.read_with(cx, |view, cx| {
        let ParameterControl::Choice { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ParameterId::from_static(601),
        }] else {
            panic!("voices selector")
        };
        assert_eq!(
            entity.read(cx).selected(),
            2,
            "loaded three-voice count is displayed without replacement"
        );
        entity.clone()
    });
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_none());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
        assert_eq!(
            view.worker_requests_pending, 0,
            "popup Escape does not enqueue transport stop"
        );
        assert!(view.pending_requests.is_empty());
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    for _ in 0..5 {
        visual.simulate_keystrokes("down");
    }
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 601),
            ParameterValue::Choice(2)
        );
    });
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 601) == ParameterValue::Choice(7)
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision + 1);
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 608),
            mode
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Envelope
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        );
    });
    for (request, value) in [(Request::Undo, 2), (Request::Redo, 7)] {
        editor.update(cx, |view, _| assert!(view.enqueue(request)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, 601)
                        == ParameterValue::Choice(value)
                })
        });
    }
    visual.run_until_parked();
    let link = visual
        .debug_bounds(selector(format!(
            "operator-preview-{}-route-link",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(link.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let ParameterControl::Choice {
            entity: current, ..
        } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ParameterId::from_static(601),
        }]
        else {
            panic!("voices")
        };
        assert_eq!(
            current.entity_id(),
            entity.entity_id(),
            "one cached owner across detail pages"
        );
    });
    let field = visual.debug_bounds(field_id).unwrap();
    let row = visual
        .debug_bounds(selector(format!("operator-voices-{}", device.value())))
        .unwrap();
    assert!(
        row.contains(&field.center()),
        "routing detail does not mount a second selector"
    );
}

#[gpui::test]
fn primary_voice_popup_cancels_outside_and_refuses_invalid_working_drafts(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let initial = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let field_id = selector(format!("device-{}-parameter-601", device.value()));
    let popup_id = selector(format!("device-{}-parameter-601-popup", device.value()));
    let field = visual.debug_bounds(field_id).unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_some());
    visual.simulate_keystrokes("up");
    visual.simulate_click(point(px(10.0), px(400.0)), Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds(popup_id).is_none());
    let gain = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-11-value",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(gain.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.simulate_click(field.center(), Modifiers::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-601-option-11",
            device.value()
        )))
        .unwrap();
    visual.simulate_click(option.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project, initial);
        assert!(view.error.is_some());
        let ParameterControl::Choice { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: ParameterId::from_static(601),
        }] else {
            panic!("voices")
        };
        assert_eq!(
            entity.read(cx).selected(),
            7,
            "refused value returns to canonical count"
        );
    });
}

#[gpui::test]
fn performance_dial_column_binds_time_tone_volume_and_pan_within_body_height(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2000.0), px(540.0)));
    visual.run_until_parked();

    let column = visual
        .debug_bounds(selector(format!(
            "operator-performance-column-{}",
            device.value()
        )))
        .expect("far-right performance dial column");
    let preview = visual
        .debug_bounds(selector(format!("operator-preview-{}", device.value())))
        .unwrap();
    let body = visual
        .debug_bounds(selector(format!("operator-body-{}", device.value())))
        .unwrap();
    assert!(column.left() >= preview.right());
    assert!(column.right() <= body.right());
    assert!(column.size.height <= px(crate::device_composition::BODY_HEIGHT));
    assert_eq!(
        visual
            .debug_bounds(selector(format!("instrument-device-{}", device.value())))
            .unwrap()
            .size
            .height,
        px(200.0)
    );

    // Four stacked dials (time, tone, volume, pan) sit top-to-bottom without overlap.
    let mut prev_bottom = px(0.0);
    for parameter in [617_u32, 618, 619, 605] {
        let dial = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}",
                device.value()
            )))
            .expect("performance dial mounted");
        assert!(column.contains(&dial.center()));
        assert!(dial.top() >= prev_bottom);
        assert!(dial.bottom() <= column.bottom());
        prev_bottom = dial.bottom();
    }
    // Pan uses bipolar DialLayout::Pan (centre reset marker present).
    assert!(
        visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-605-pan-reset",
                device.value()
            )))
            .is_some()
    );

    // Every shown control binds its canonical parameter through the shared adapter
    // with single-step Undo/Redo.
    for (parameter, typed, expected) in [
        (617_u32, "-25", -25.0_f32),
        (618_u32, "60", 60.0_f32),
        (619_u32, "-6", -6.0_f32),
        (605_u32, "20 r", 20.0_f32),
    ] {
        let before = editor.read_with(cx, |view, _| {
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, parameter)
        });
        let value_bounds = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-value",
                device.value()
            )))
            .expect("dial readout");
        visual.simulate_click(value_bounds.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(typed);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Float(expected)
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter) == before
                })
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending == 0
                && view.state.as_ref().is_some_and(|state| {
                    operator_parameter_value(state, track, device, parameter)
                        == ParameterValue::Float(expected)
                })
        });
    }
}

#[gpui::test]
fn compact_glide_controls_dispatch_canonical_edits_preserve_time_context_and_undo(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(2000.0), px(540.0)));
    visual.run_until_parked();

    let preview = visual
        .debug_bounds(selector(format!("operator-preview-{}", device.value())))
        .unwrap();
    let voicing_row = visual
        .debug_bounds(selector(format!("operator-voices-{}", device.value())))
        .unwrap();
    let voices = visual
        .debug_bounds(selector(format!("device-{}-parameter-601", device.value())))
        .unwrap();
    let glide_toggle = visual
        .debug_bounds(selector(format!("device-{}-parameter-642", device.value())))
        .expect("compact glide toggle");
    let glide_time = visual
        .debug_bounds(selector(format!("device-{}-parameter-643", device.value())))
        .expect("compact glide time");
    assert!(voicing_row.contains(&glide_toggle.center()));
    assert!(voicing_row.contains(&glide_time.center()));
    assert!(glide_toggle.left() >= voices.right());
    assert!(glide_time.left() >= glide_toggle.right());
    assert!(glide_time.right() <= preview.right());
    assert!(glide_time.bottom() <= preview.bottom());

    // Toggle glide on -> single canonical history step, envelope context preserved.
    visual.simulate_click(glide_toggle.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 642) == ParameterValue::Bool(true)
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision + 1);
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Envelope
        );
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            0
        );
    });

    // Edit glide time to 120 ms -> single canonical step.
    let time_value = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-643-value",
            device.value()
        )))
        .expect("glide time readout");
    visual.simulate_click(time_value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("120 ms");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                matches!(
                    operator_parameter_value(state, track, device, 643),
                    ParameterValue::Float(seconds) if (seconds - 0.12).abs() < 1e-4
                )
            })
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision + 2);
    });

    // Toggle glide off -> configured glide time is retained intact.
    visual.simulate_click(glide_toggle.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 642) == ParameterValue::Bool(false)
            })
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, initial_revision + 3);
        assert!(matches!(
            operator_parameter_value(state, track, device, 643),
            ParameterValue::Float(seconds) if (seconds - 0.12).abs() < 1e-4
        ));
    });

    // Single-step Undo/Redo restores glide state cleanly.
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 642) == ParameterValue::Bool(true)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 642) == ParameterValue::Bool(false)
            })
    });

    // Invalid draft in glide time blocks subsequent edits without mutating project state.
    let before_invalid =
        editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    visual.simulate_click(time_value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("not-a-time");
    visual.simulate_click(glide_toggle.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before_invalid);
        assert_eq!(
            operator_parameter_value(view.state.as_ref().unwrap(), track, device, 642),
            ParameterValue::Bool(false)
        );
        assert!(view.error.is_some());
    });
}
