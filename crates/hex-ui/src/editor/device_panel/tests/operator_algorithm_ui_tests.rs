use super::*;

fn assert_primary_diagram(
    visual: &mut VisualTestContext,
    device: DeviceId,
    choice: u32,
    parallel: bool,
) {
    let name =
        Box::leak(format!("operator-routing-current-{}-{choice}", device.value()).into_boxed_str());
    let graph = visual
        .debug_bounds(name)
        .unwrap_or_else(|| panic!("missing topology {choice}"));
    assert_eq!(graph.size.width, px(38.0));
    let quads = visual.rendered_filled_quads(name);
    let nodes: [_; 4] = std::array::from_fn(|operator| {
        let color = gpui::Background::from(gpui::rgb(
            theme::oscillator_identity_color(operator).unwrap(),
        ));
        let node = quads
            .iter()
            .find(|(_, fill)| *fill == color)
            .expect("filled oscillator square")
            .0;
        assert!(f64::from(node.size.width) > 3.0);
        assert!((f64::from(node.size.width) - f64::from(node.size.height)).abs() < 0.1);
        node.center()
    });
    if parallel {
        for pair in nodes.windows(2) {
            assert!(pair[0].x < pair[1].x);
            assert!((f64::from(pair[0].y) - f64::from(pair[1].y)).abs() < 0.1);
        }
    } else {
        for pair in nodes.windows(2) {
            assert!(pair[0].y > pair[1].y, "serial nodes={nodes:?}");
            assert!((f64::from(pair[0].x) - f64::from(pair[1].x)).abs() < 0.1);
        }
    }
}

#[gpui::test]
fn primary_routing_diagram_follows_canonical_pending_and_custom_choices(cx: &mut TestAppContext) {
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let initial = state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert_primary_diagram(&mut visual, device, 1, false);
    let key = ParameterKey {
        device,
        parameter: hex_lattice_contract::ParameterId::from_static(600),
    };
    for choice in [11, 0] {
        editor.update(cx, |view, cx| {
            // Hold the UI acknowledgement boundary while inspecting its preview.
            view.worker_requests_pending = 1;
            view.device_panel
                .pending_values
                .insert(key, ParameterValue::Choice(choice));
            cx.notify();
        });
        visual.run_until_parked();
        if choice == 11 {
            assert_primary_diagram(&mut visual, device, 11, true);
        } else {
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("operator-routing-current-{}-custom", device.value())
                            .into_boxed_str()
                    ))
                    .is_some()
            );
        }
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, initial)
        });
    }
    editor.update(cx, |view, _| {
        view.worker_requests_pending = 0;
        view.device_panel.pending_values.remove(&key);
        assert!(view.enqueue(Request::SetDeviceParameter {
            device,
            parameter: key.parameter,
            value: ParameterValue::Choice(11)
        }));
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 600) == ParameterValue::Choice(11)
            })
    });
    visual.run_until_parked();
    assert_primary_diagram(&mut visual, device, 11, true);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project == initial)
    });
    visual.run_until_parked();
    assert_primary_diagram(&mut visual, device, 1, false);
}

#[gpui::test]
fn legacy_mode_is_explicit_and_reselecting_its_algorithm_enables_global_controls(
    cx: &mut TestAppContext,
) {
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(608),
            value: ParameterValue::Choice(0),
        })
        .unwrap();
    let legacy = receive_service_event(&service);
    assert!(legacy.error.is_none());
    let original = legacy.state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, legacy.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .entry(device)
            .or_default()
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let native = view.device_panel.views[&device].2.as_ref().unwrap();
        for parameter in [601, 604, 605, 620, 639] {
            assert!(
                !native
                    .parameters
                    .iter()
                    .find(|entry| entry.descriptor.id.value() == parameter)
                    .unwrap()
                    .enabled
            );
        }
        let mode = native
            .parameters
            .iter()
            .find(|entry| entry.descriptor.id.value() == 608)
            .unwrap();
        assert!(mode.enabled);
        assert_eq!(mode.value, ParameterValue::Choice(0));
    });
    let mode = Box::leak(format!("device-{}-parameter-608", device.value()).into_boxed_str());
    assert!(visual.debug_bounds(mode).is_some());
    let diagram = visual
        .debug_bounds(Box::leak(
            format!("operator-algorithm-{}-1", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(diagram.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 608) == ParameterValue::Choice(1)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.device(device).unwrap().state() == &original)
    });
}

#[gpui::test]
fn algorithms_replace_the_matrix_and_commit_one_canonical_history_edit(cx: &mut TestAppContext) {
    let (service, original, track, device) = operator_navigation_tests::operator_fixture();
    let original_state = original.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, original, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .entry(device)
            .or_default()
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("operator-routing-header").is_none());
    for choice in 1..=11 {
        let selector =
            Box::leak(format!("operator-algorithm-{}-{choice}", device.value()).into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("clickable topology");
        assert!(bounds.size.width > px(10.0));
    }
    let detail = visual
        .debug_bounds(Box::leak(
            format!("operator-detail-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    let voices = visual
        .debug_bounds(Box::leak(
            format!("operator-voices-{}", device.value()).into_boxed_str(),
        ))
        .unwrap();
    for parameter in
        (601..=608).chain((620..=639).filter(|parameter| !matches!(*parameter, 626 | 634)))
    {
        let selector =
            Box::leak(format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .expect("canonical global control");
        let region = if parameter == 601 { voices } else { detail };
        assert!(
            bounds.right() <= region.right() && bounds.bottom() <= region.bottom(),
            "parameter {parameter} bounds {bounds:?} outside its region {region:?}"
        );
    }
    for parameter in [626, 634] {
        let selector =
            Box::leak(format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_none(),
            "fixed assignments are labels"
        );
    }
    let selector = Box::leak(format!("operator-algorithm-{}-8", device.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    visual.simulate_click(bounds.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 600) == ParameterValue::Choice(8)
            })
    });
    let selected = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .clone()
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                state.project.device(device).unwrap().state() == &original_state
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.device(device).unwrap().state() == &selected)
    });
    visual.run_until_parked();
    let focus = editor.read_with(cx, |view, _| {
        view.device_panel.operator_navigation_focus
            [&(device, operator_panel::NavigationTarget::Algorithm(4))]
            .clone()
    });
    visual.update(|window, _| focus.focus(window));
    editor.update(cx, |view, _| view.last_transport_command = None);
    visual.simulate_keystrokes("space");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::PlaybackCommand::PlayFrom(0)),
            "Space controls transport while an algorithm is focused"
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .device(device)
                .unwrap()
                .state(),
            &selected,
            "Space must not change the algorithm"
        );
    });
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 600) == ParameterValue::Choice(4)
            })
    });
}

#[gpui::test]
fn rapid_algorithm_actions_use_pending_choice_and_restore_it_on_queue_refusal(
    cx: &mut TestAppContext,
) {
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        operator_panel::algorithms::select(view, cx, device, 4);
        operator_panel::algorithms::select(view, cx, device, 8);
        let key = ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(600),
        };
        assert_eq!(
            view.device_panel.pending_values[&key],
            ParameterValue::Choice(8)
        );
        while view.pending_requests.len() < crate::editor::UI_REQUEST_CAPACITY {
            view.pending_requests.push_back(Request::Undo);
        }
        operator_panel::algorithms::select(view, cx, device, 11);
        assert_eq!(
            view.device_panel.pending_values[&key],
            ParameterValue::Choice(8)
        );
        view.pending_requests.clear();
        view.device_panel.pending_values.clear();
    });
}

#[gpui::test]
fn custom_legacy_routing_keeps_sound_state_and_all_global_controls(cx: &mut TestAppContext) {
    let (service, _, track, device) = operator_navigation_tests::operator_fixture();
    service
        .submit(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(17),
            value: ParameterValue::Float(-0.31),
        })
        .unwrap();
    let custom = receive_service_event(&service);
    assert!(custom.error.is_none());
    let authored = custom.state.project.device(device).unwrap().state().clone();
    let (handle, editor) = actor_editor_window(cx, service, custom.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .entry(device)
            .or_default()
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    let custom_bounds = visual.debug_bounds("operator-routing-custom").unwrap();
    let mode_selector =
        Box::leak(format!("device-{}-parameter-608", device.value()).into_boxed_str());
    let mode_bounds = visual.debug_bounds(mode_selector).unwrap();
    assert!(
        custom_bounds.bottom() <= mode_bounds.top(),
        "custom routing marker must not consume the behavior selector's narrow cell",
    );
    assert!(visual.debug_bounds("operator-routing-header").is_none());
    for parameter in
        (601..=608).chain((620..=639).filter(|parameter| !matches!(*parameter, 626 | 634)))
    {
        let selector =
            Box::leak(format!("device-{}-parameter-{parameter}", device.value()).into_boxed_str());
        assert!(visual.debug_bounds(selector).is_some());
    }
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SetDeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(605),
            value: ParameterValue::Float(25.0),
        }))
    });
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 605) == ParameterValue::Float(25.0)
            })
    });
    let with_pan = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .device(device)
            .unwrap()
            .state()
            .clone()
    });
    assert_eq!(
        &with_pan.payload()[..128],
        &authored.payload()[..128],
        "global edits preserve every authored oscillator and route"
    );
    visual.run_until_parked();
    let diagram = visual
        .debug_bounds(Box::leak(
            format!("operator-algorithm-{}-3", device.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(diagram.center(), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.state.as_ref().is_some_and(|state| {
                operator_parameter_value(state, track, device, 600) == ParameterValue::Choice(3)
            })
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.device(device).unwrap().state() == &with_pan)
    });
}
