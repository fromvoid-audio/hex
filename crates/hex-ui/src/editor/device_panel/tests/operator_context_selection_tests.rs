use super::*;
use gpui::Focusable;
fn selector(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
pub(super) fn row_background(
    visual: &mut VisualTestContext,
    device: DeviceId,
    operator: usize,
) -> gpui::Point<gpui::Pixels> {
    let row = visual
        .debug_bounds(selector(format!(
            "operator-row-{}-{operator}",
            device.value()
        )))
        .unwrap();
    // The bottom strip is content, below the numeric fields. Its left end also
    // stays clear of the waveform popup; the structural side border is not a hit target.
    point(row.left() + px(16.0), row.bottom() - px(1.0))
}

#[gpui::test]
fn working_controls_preserve_the_explicit_envelope_selection(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (parameter, text, expected) in [
        (11, "-12 db", 10.0_f32.powf(-12.0 / 20.0)),
        (7, "2", 2.0),
        (610, "150 hz", 150.0),
        (81, "25%", 0.25),
    ] {
        let field = visual
            .debug_bounds(selector(format!(
                "device-{}-parameter-{parameter}-value",
                device.value()
            )))
            .unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(text);
        visual.simulate_keystrokes("enter");
        pump_until(&editor, cx, |view| {
            view.worker_requests_pending==0&&view.state.as_ref().is_some_and(|s|matches!(operator_parameter_value(s,track,device,parameter),ParameterValue::Float(v) if (v-expected).abs()<0.00001))
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_panel.operator_pages[&device],
                operator_panel::OperatorPanelState {
                    selected_operator: 0,
                    page: operator_panel::OperatorPage::Envelope,
                    ..Default::default()
                }
            );
            assert_eq!(
                view.device_panel.selected_device,
                Some(device),
                "the device host still owns control clicks"
            );
        });
        visual.run_until_parked();
    }
}

#[gpui::test]
fn row_background_selection_is_explicit_and_refuses_invalid_or_stale_contexts(
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
    let background = row_background(&mut visual, device, 3);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device],
            operator_panel::OperatorPanelState {
                selected_operator: 3,
                page: operator_panel::OperatorPage::Envelope,
                ..Default::default()
            }
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    let field = visual
        .debug_bounds(selector(format!(
            "device-{}-parameter-4-value",
            device.value()
        )))
        .unwrap();
    let entity = editor.read_with(cx, |view, _| {
        match &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(4),
        }] {
            ParameterControl::Float { entity, .. } => entity.clone(),
            _ => panic!("ratio"),
        }
    });
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    let background = row_background(&mut visual, device, 0);
    visual.simulate_click(background, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            3
        );
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    assert!(visual.update(|window, app| entity.focus_handle(app).is_focused(window)));
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, cx| {
        let old = operator_panel::context::Owner::capture(view, device).unwrap();
        view.project_generation += 1;
        assert!(!operator_panel::context::select_oscillator(
            view, cx, old, 0
        ));
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            3
        );
    });
}

#[gpui::test]
fn unassigned_operator_context_rejects_stale_source_and_selection(cx: &mut TestAppContext) {
    let (service, _, selected_track, _, _, _) = actor_utility_fixture();
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unassigned operator".into(),
            devices: vec![state],
            instrument_index: 0,
        }))
        .unwrap();
    let created = receive_service_event(&service);
    let Some(Created::NativeSource(source)) = created.created else {
        panic!("source")
    };
    let device = created
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }))
        .unwrap();
    let unbound = receive_service_event(&service);
    let Some(Created::Track(contributor)) = unbound.created else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let bound = receive_service_event(&service);
    let replacement = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::ReplaceTrackInstrument {
            track: contributor,
            replacing: device,
            state: replacement,
        }))
        .unwrap();
    let replaced = receive_service_event(&service);
    assert_ne!(
        replaced
            .state
            .project
            .native_source(source)
            .unwrap()
            .instrument()
            .id(),
        device
    );
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: None,
        }))
        .unwrap();
    let replaced_unbound = receive_service_event(&service);
    service
        .submit(Request::Edit(Command::RemoveNativeSource(source)))
        .unwrap();
    let removed = receive_service_event(&service);
    let original_track = unbound.state.project.track(selected_track).unwrap().clone();
    let (_, editor) = actor_editor_window(cx, service, unbound.state.clone(), selected_track);
    editor.update(cx, |view, cx| {
        let generation = view.project_generation;
        view.device_panel.owner = Some(DevicePanelOwner::Source(source));
        view.device_panel.source_manager.source_view = Some((
            generation,
            source,
            Some(DevicePanelOwner::Track(selected_track)),
        ));
        let owner = operator_panel::context::Owner::capture(view, device).expect("source owner");
        assert_eq!(
            owner.state(view),
            Some(
                unbound
                    .state
                    .project
                    .native_source(source)
                    .unwrap()
                    .instrument()
                    .state()
            )
        );
        assert!(operator_panel::context::select_oscillator(
            view, cx, owner, 2
        ));
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            2
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(selected_track)
                .unwrap(),
            &original_track
        );
        view.selected_track = None;
        assert!(owner.state(view).is_none());
        assert!(!operator_panel::context::select_oscillator(
            view, cx, owner, 1
        ));
        view.selected_track = Some(selected_track);
        let other = unbound
            .state
            .project
            .track(selected_track)
            .unwrap()
            .source_id()
            .unwrap();
        view.device_panel.source_manager.source_view = Some((
            generation,
            other,
            Some(DevicePanelOwner::Track(selected_track)),
        ));
        assert!(owner.state(view).is_none());
        view.device_panel.source_manager.source_view = Some((
            generation,
            source,
            Some(DevicePanelOwner::Track(selected_track)),
        ));
        view.project_generation = generation + 1;
        assert!(owner.state(view).is_none());
        view.project_generation = generation;
        view.state = Some(bound.state.clone());
        assert!(owner.state(view).is_none());
        assert!(operator_panel::context::Owner::capture(view, device).is_none());
        view.state = Some(replaced_unbound.state.clone());
        assert!(owner.state(view).is_none());
        assert!(operator_panel::context::Owner::capture(view, device).is_none());
        view.state = Some(removed.state.clone());
        assert!(owner.state(view).is_none());
        assert!(operator_panel::context::Owner::capture(view, device).is_none());
        assert_eq!(
            view.device_panel.operator_pages[&device].selected_operator,
            2
        );
    });
}
