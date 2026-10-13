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
