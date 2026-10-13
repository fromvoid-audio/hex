use super::*;

fn routing_window(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    gpui::Entity<EditorView>,
    TrackId,
    DeviceId,
    u64,
) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = operator_navigation_tests::operator_fixture();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    editor.update(cx, |view, cx| {
        view.device_panel
            .operator_pages
            .get_mut(&device)
            .unwrap()
            .page = operator_panel::OperatorPage::Routing;
        cx.notify();
    });
    visual.run_until_parked();
    (visual, editor, track, device, revision)
}

#[gpui::test]
fn typed_units_keep_every_character_without_editor_shortcuts(cx: &mut TestAppContext) {
    let (mut visual, editor, _, device, _) = routing_window(cx);
    let parameter = hex_lattice_contract::ParameterId::from_static(25);
    enter_typed_value(&mut visual, device, parameter, "-7.99 turns");
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } =
            &view.device_panel.controls[&ParameterKey { device, parameter }]
        else {
            panic!("numeric phase")
        };
        entity.read_with(cx, |control, _| {
            assert!(control.is_editing());
            assert_eq!(control.display_text(), "-7.99 turns");
        });
    });
}

#[gpui::test]
fn typed_units_do_not_mutate_canonical_solo_or_history(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device, revision) = routing_window(cx);
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(25),
        "-7.99 turns",
    );
    // A worker round trip fences any accidentally enqueued shortcut command.
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track,
            name: "fence".into()
        })))
    });
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .name()
            == "fence"
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert!(
            !state.project.track(track).unwrap().soloed(),
            "typed s must not solo the track"
        );
        assert_eq!(
            state.revision,
            revision + 1,
            "only the explicit fence may change history"
        );
    });
}

fn phase_control(
    editor: &gpui::Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
) -> gpui::Entity<hex_widgets::NumericControl> {
    editor.read_with(cx, |view, _| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(25),
        }] else {
            panic!("phase control")
        };
        entity.clone()
    })
}

#[gpui::test]
fn numeric_text_owns_uppercase_space_and_direct_solo_actions(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device, revision) = routing_window(cx);
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(25),
        "s l ",
    );
    visual.simulate_keystrokes("shift-s");
    visual.update(|window, cx| {
        window.dispatch_action(Box::new(crate::editor::ToggleSolo), cx);
        window.dispatch_action(Box::new(crate::editor::AdditiveSolo), cx);
    });
    visual.run_until_parked();
    phase_control(&editor, cx, device)
        .read_with(cx, |control, _| assert_eq!(control.display_text(), "s l S"));
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track,
            name: "fence".into()
        })))
    });
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .name()
            == "fence"
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 1);
        assert!(!state.project.track(track).unwrap().soloed());
        assert!(view.last_transport_command.is_none(), "space remains text");
    });
}

#[gpui::test]
fn numeric_clipboard_cancel_commit_and_single_undo_preserve_owner(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device, revision) = routing_window(cx);
    let parameter = hex_lattice_contract::ParameterId::from_static(25);
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
    let expected = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &original,
        parameter,
        ParameterValue::Float(-7.99),
    )
    .unwrap();
    enter_typed_value(&mut visual, device, parameter, "-7.99 turns");
    visual.simulate_keystrokes("cmd-a cmd-c cmd-x cmd-v");
    visual.run_until_parked();
    phase_control(&editor, cx, device).read_with(cx, |control, _| {
        assert_eq!(control.display_text(), "-7.99 turns")
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision)
    });
    phase_control(&editor, cx, device).read_with(cx, |control, _| assert!(!control.is_editing()));
    enter_typed_value(&mut visual, device, parameter, "-7.99 turns");
    // Existing owner-finalization path must still commit exactly once.
    editor.update(cx, |view, cx| assert!(super::finalize_controls(view, cx)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision > revision
            && view.device_gesture.snapshot().is_none()
            && view.worker_requests_pending == 0
    });
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 1);
        assert!(!state.project.track(track).unwrap().soloed());
        assert_eq!(
            state
                .project
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            &expected
        );
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision == revision + 2
    });
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
            &original
        )
    });
    enter_typed_value(&mut visual, device, parameter, "-7.99 turns");
    visual.simulate_keystrokes("enter");
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision == revision + 3
            && view.device_gesture.snapshot().is_none()
    });
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
            &expected
        )
    });
}

#[gpui::test]
fn plain_and_shift_s_still_solo_outside_numeric_text_edit(cx: &mut TestAppContext) {
    let (mut visual, editor, track, device, revision) = routing_window(cx);
    enter_typed_value(
        &mut visual,
        device,
        hex_lattice_contract::ParameterId::from_static(25),
        "0",
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    // Leave focus on the numeric control: focus alone must not disable Solo.
    visual.simulate_keystrokes("s");
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision == revision + 1
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .soloed()
        )
    });
    visual.simulate_keystrokes("shift-s");
    pump_until(&editor, cx, |view| {
        view.state.as_ref().unwrap().revision == revision + 2
    });
    editor.read_with(cx, |view, _| {
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .soloed()
        )
    });
}
