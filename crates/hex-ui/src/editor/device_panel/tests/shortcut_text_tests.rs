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
