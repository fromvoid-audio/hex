use super::*;
use gpui::{FocusHandle, Focusable};

fn focused(visual: &mut VisualTestContext) -> FocusHandle {
    visual.update(|window, cx| window.focused(cx).expect("a tab target remains focused"))
}

#[gpui::test]
fn operator_tab_cycles_rendered_controls_in_both_directions_without_project_edits(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let coarse = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-10-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(coarse.center(), Modifiers::default());
    visual.run_until_parked();
    let first = focused(&mut visual);
    let mut order = vec![first.clone()];
    for _ in 0..96 {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        let next = focused(&mut visual);
        if order.contains(&next) {
            assert_eq!(
                next, first,
                "traversal wraps, rather than trapping in routing"
            );
            break;
        }
        order.push(next);
    }
    assert!(
        order.len() > 20,
        "oscillator, detail and section controls must be reachable"
    );
    assert_eq!(focused(&mut visual), first, "finite tab cycle");
    editor.read_with(cx, |view, _| {
        for focus in view.device_panel.operator_navigation_focus.values() {
            assert!(
                order.contains(focus),
                "oscillator and page selectors participate"
            );
        }
        let routing = view.device_panel.operator_previews[&device]
            .routing_focus
            .as_ref()
            .unwrap();
        assert!(order.contains(routing));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    // The page follows shell/global focus as before. Set it to the final page
    // so backward traversal compares the same rendered control inventory.
    let mut reverse = Vec::new();
    for _ in 0..96 {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        let previous = focused(&mut visual);
        if previous == first {
            break;
        }
        assert!(
            !reverse.contains(&previous),
            "backward traversal cannot trap"
        );
        reverse.push(previous);
    }
    assert!(reverse.len() > 20);
    assert_eq!(focused(&mut visual), first);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
}

#[gpui::test]
fn operator_section_activation_and_invalid_draft_keep_focus_and_state(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (page, key) in [
        (operator_panel::OperatorPage::Lfo, "enter"),
        (operator_panel::OperatorPage::Oscillator, "space"),
    ] {
        let target = editor.read_with(cx, |view, _| {
            view.device_panel.operator_navigation_focus
                [&(device, operator_panel::NavigationTarget::Page(page))]
                .clone()
        });
        visual.update(|window, _| target.focus(window));
        visual.simulate_keystrokes(key);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.device_panel.operator_pages[&device].page, page)
        });
        assert_eq!(focused(&mut visual), target);
    }
    let parameter = hex_lattice_contract::ParameterId::from_static(2);
    enter_typed_value(&mut visual, device, parameter, "invalid");
    let before = focused(&mut visual);
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert_eq!(
        focused(&mut visual),
        before,
        "invalid draft refuses traversal"
    );
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        let ParameterControl::Float { entity, .. } =
            &view.device_panel.controls[&ParameterKey { device, parameter }]
        else {
            panic!("numeric field")
        };
        assert!(entity.read(cx).is_editing());
        assert!(entity.focus_handle(cx) == before);
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert_ne!(focused(&mut visual), before);

    let original = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone()
    });
    enter_typed_value(&mut visual, device, parameter, "-6 db");
    visual.simulate_keystrokes("tab");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture_barriers.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    let authored = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state()
            .clone()
    });
    assert_ne!(authored, original, "valid typing commits before traversal");
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &original,
            "one Undo restores the full canonical state"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &authored
        )
    });
}

#[gpui::test]
fn operator_tab_targets_retire_on_track_owner_change(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "other".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(other) = event.created.unwrap() else {
        panic!("track")
    };
    let revision = event.state.revision;
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let key = (
        device,
        operator_panel::NavigationTarget::Page(operator_panel::OperatorPage::Lfo),
    );
    let old = editor.read_with(cx, |view, _| {
        view.device_panel.operator_navigation_focus[&key].clone()
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(other);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.operator_navigation_focus.is_empty())
    });
    visual.update(|window, _| old.focus(window));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(view.device_panel.operator_pages.is_empty());
    });
    editor.update(cx, |view, cx| {
        view.selected_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_ne!(view.device_panel.operator_navigation_focus[&key], old)
    });
}

#[gpui::test]
fn operator_shell_selectors_precede_controls_and_sections_follow_child_focus(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for operator in 0..4 {
        let bounds = |visual: &mut VisualTestContext, selector: String| {
            visual
                .debug_bounds(Box::leak(selector.into_boxed_str()))
                .unwrap()
        };
        let selector = bounds(
            &mut visual,
            format!("operator-select-{}-{operator}", device.value()),
        );
        let level = bounds(
            &mut visual,
            format!(
                "device-{}-parameter-{}-knob",
                device.value(),
                operator * 3 + 2
            ),
        );
        let row = bounds(
            &mut visual,
            format!("operator-row-{}-{operator}", device.value()),
        );
        assert!(
            selector.origin.x >= level.origin.x + level.size.width,
            "oscillator squares stay at the far right"
        );
        assert!(selector.origin.x + selector.size.width <= row.origin.x + row.size.width);
    }
    let order = editor.read_with(cx, |view, cx| {
        let mut order = Vec::new();
        for operator in (0..4).rev() {
            order.push(
                view.device_panel.operator_navigation_focus[&(
                    device,
                    operator_panel::NavigationTarget::Oscillator(operator),
                )]
                    .clone(),
            );
            let base = operator as u32 * 3 + 1;
            let focus = |parameter| match &view.device_panel.controls[&ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(parameter),
            }] {
                ParameterControl::Float { entity, .. } => entity.focus_handle(cx),
                ParameterControl::Bool { entity, .. } => entity.focus_handle(cx),
                ParameterControl::Choice { entity, .. } => entity.focus_handle(cx),
            };
            order.push(focus(base));
            order.push(
                view.device_panel.operator_previews[&device].controls[&[
                    operator_panel::preview::Field::FineA,
                    operator_panel::preview::Field::FineB,
                    operator_panel::preview::Field::FineC,
                    operator_panel::preview::Field::FineD,
                ][operator]]
                    .entity
                    .focus_handle(cx),
            );
            order.push(focus(62 + operator as u32 * 3));
            order.push(focus(base + 1));
        }
        order
    });
    visual.update(|window, _| order[0].focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator, 3,
            "selector focus activates its oscillator"
        )
    });
    for expected in order.iter().skip(1) {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_eq!(
            &focused(&mut visual),
            expected,
            "selector, coarse, fine, fixed, level"
        );
    }
    for expected in order.iter().rev().skip(1) {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        assert_eq!(&focused(&mut visual), expected);
    }
    // Preview child focus reveals its row without making the row a Tab stop
    // or changing its switch, project state, or neighboring row's focus mode.
    for (field, title) in [
        (operator_panel::preview::Field::FilterFrequency, "filter"),
        (operator_panel::preview::Field::PitchEnvelope, "pitch env"),
        (operator_panel::preview::Field::Time, "global"),
    ] {
        let child = editor.read_with(cx, |view, cx| {
            view.device_panel.operator_previews[&device].controls[&field]
                .entity
                .focus_handle(cx)
        });
        visual.update(|window, _| child.focus(window));
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            for ((_, section), state) in &view.device_panel.operator_section_focus {
                assert!(
                    !state.handle.tab_stop,
                    "containers are excluded from traversal"
                );
                assert_eq!(
                    state.focused,
                    *section == operator_panel::Section::Preview(title)
                );
            }
            assert_eq!(
                view.device_panel.operator_previews[&device].enabled,
                [true; 2]
            );
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
        });
        assert_eq!(focused(&mut visual), child);
    }
}

#[gpui::test]
fn operator_preview_invalid_drafts_refuse_forward_and_reverse_tab(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (field, valid) in [
        (operator_panel::preview::Field::FineB, "12"),
        (operator_panel::preview::Field::FilterFrequency, "500"),
        (operator_panel::preview::Field::PitchEnvelope, "20"),
        (operator_panel::preview::Field::Time, "30"),
    ] {
        let selector = Box::leak(
            format!(
                "operator-preview-{}-{}-value",
                device.value(),
                field as usize
            )
            .into_boxed_str(),
        );
        let bounds = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input("invalid");
        visual.run_until_parked();
        let before = focused(&mut visual);
        for key in ["tab", "shift-tab"] {
            visual.simulate_keystrokes(key);
            visual.run_until_parked();
            assert_eq!(
                focused(&mut visual),
                before,
                "invalid preview draft must keep focus"
            );
            editor.read_with(cx, |view, cx| {
                assert!(
                    view.device_panel.operator_previews[&device].controls[&field]
                        .entity
                        .read(cx)
                        .is_editing()
                )
            });
        }
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(valid);
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_ne!(
            focused(&mut visual),
            before,
            "corrected draft permits traversal"
        );
        editor.read_with(cx, |view, cx| {
            assert!(
                !view.device_panel.operator_previews[&device].controls[&field]
                    .entity
                    .read(cx)
                    .is_editing()
            );
            assert_eq!(
                view.device_panel.operator_previews[&device].values[field as usize],
                valid.parse::<f64>().unwrap()
            );
            assert_eq!(
                view.state.as_ref().unwrap().project,
                original,
                "view-only previews do not author sound"
            );
        });
    }
}

#[gpui::test]
fn operator_lfo_and_routing_widgets_have_one_focus_group_and_release_previous_group(
    cx: &mut TestAppContext,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let original = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let parameter_focus = |parameter, cx: &TestAppContext| {
        editor.read_with(cx, |view, app| {
            match &view.device_panel.controls[&ParameterKey {
                device,
                parameter: hex_lattice_contract::ParameterId::from_static(parameter),
            }] {
                ParameterControl::Float { entity, .. } => entity.focus_handle(app),
                ParameterControl::Bool { entity, .. } => entity.focus_handle(app),
                ParameterControl::Choice { entity, .. } => entity.focus_handle(app),
            }
        })
    };
    let lfo = parameter_focus(82, cx);
    visual.update(|window, _| lfo.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].page,
            operator_panel::OperatorPage::Lfo
        )
    });
    // A direct focus update changes page context; draw its newly mounted controls
    // before looking up their handles (native windows draw on the next frame).
    visual.update(|window, cx| {
        window.draw(cx).clear();
    });
    visual.run_until_parked();
    let sync = editor.read_with(cx, |view, cx| {
        let ParameterControl::Bool { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(84),
        }] else {
            panic!("sync toggle");
        };
        entity.read(cx).checked()
    });
    let alternate_rate = if sync { 85 } else { 86 };
    let detail_order: Vec<_> = [84, alternate_rate, 88, 89, 90, 91, 92]
        .into_iter()
        .map(|parameter| parameter_focus(parameter, cx))
        .collect();
    visual.update(|window, _| detail_order[0].focus(window));
    visual.run_until_parked();
    for target in detail_order.iter().skip(1) {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        assert_eq!(
            &focused(&mut visual),
            target,
            "LFO detail has no duplicate summary widgets"
        );
    }
    for target in detail_order.iter().rev().skip(1) {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        assert_eq!(&focused(&mut visual), target);
    }
    for parameter in 82..=92 {
        let target = parameter_focus(parameter, cx);
        visual.update(|window, _| target.focus(window));
        visual.run_until_parked();
        let section =
            if matches!(parameter, 82 | 83 | 87) || parameter == if sync { 86 } else { 85 } {
                operator_panel::Section::Preview("lfo")
            } else {
                operator_panel::Section::Detail
            };
        visual.update(|window, app| {
            editor.read_with(app, |view, _| {
                for ((_, candidate), state) in &view.device_panel.operator_section_focus {
                    assert_eq!(
                        state.focused,
                        *candidate == section,
                        "one active group for LFO parameter {parameter}"
                    );
                    assert_eq!(
                        state.handle.contains_focused(window, app),
                        *candidate == section,
                        "rendered ancestry matches focus mode"
                    );
                }
            })
        });
    }
    // The routing icon is a widget target, followed by detail's FM algorithm selector.
    let routing = editor.read_with(cx, |view, _| {
        view.device_panel.operator_previews[&device]
            .routing_focus
            .as_ref()
            .unwrap()
            .clone()
    });
    visual.update(|window, _| routing.focus(window));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let algorithm = parameter_focus(61, cx);
    visual.update(|window, _| algorithm.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        for ((_, section), state) in &view.device_panel.operator_section_focus {
            assert_eq!(
                state.focused,
                *section == operator_panel::Section::Detail,
                "algorithm focus releases the global and LFO rows"
            );
        }
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    let knob = visual
        .debug_bounds(Box::leak(
            format!("device-{}-parameter-87-knob", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_mouse_down(knob.center(), MouseButton::Left, Modifiers::default());
    let moved = point(knob.center().x, knob.center().y - px(20.0));
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.device_gesture.snapshot().is_none()
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.state.as_ref().unwrap().project,
            original,
            "LFO focus activation preserves the initiating pointer gesture"
        );
    });
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project,
            original,
            "one Undo restores the LFO drag"
        )
    });
    let outside = editor.read_with(cx, |view, _| view.focus.clone());
    visual.update(|window, _| outside.focus(window));
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.device_panel
                .operator_section_focus
                .values()
                .all(|state| !state.focused),
            "leaving Operator deactivates every section"
        )
    });
}
