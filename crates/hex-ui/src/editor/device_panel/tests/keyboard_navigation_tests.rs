use super::*;

fn ids(n: u64) -> Vec<DeviceId> {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack { name: "ids".into() })
        .unwrap()
    else {
        panic!("track");
    };
    for _ in 0..n {
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
    }
    editor
        .project()
        .track(track)
        .unwrap()
        .devices()
        .iter()
        .map(|device| device.id())
        .collect()
}

#[test]
fn step_target_walks_selection_and_insertion_gaps() {
    let order = ids(3);
    let mut panel = DevicePanelState::default();
    assert_eq!(panel.step_target(&order, true), Some(order[0]));
    assert_eq!(panel.step_target(&order, false), Some(order[2]));
    assert_eq!(panel.step_target(&[], true), None);

    panel.select_only(Some(order[1]));
    assert_eq!(panel.step_target(&order, true), Some(order[2]));
    assert_eq!(panel.step_target(&order, false), Some(order[0]));
    panel.select_only(Some(order[2]));
    assert_eq!(panel.step_target(&order, true), None, "end does not wrap");

    panel.select_insertion(Some(order[1]));
    assert!(panel.selected_device.is_none(), "a gap replaces selection");
    assert_eq!(panel.step_target(&order, false), Some(order[0]));
    assert_eq!(panel.step_target(&order, true), Some(order[1]));
    panel.select_insertion(Some(order[0]));
    assert_eq!(panel.step_target(&order, false), None);
    panel.select_insertion(None);
    assert_eq!(panel.step_target(&order, false), Some(order[2]));
    assert_eq!(panel.step_target(&order, true), None);

    // A removed gap device leaves no stale paste target.
    panel.select_insertion(Some(order[1]));
    panel.reconcile_selection(&[order[0], order[2]].into_iter().collect());
    assert_eq!(panel.insertion_before(&order), None);
    panel.select_insertion(Some(order[1]));
    panel.select_click(order[0], &order, Modifiers::default());
    assert_eq!(
        panel.insertion_before(&order),
        None,
        "selection replaces a gap"
    );
}

fn chain(editor: &Entity<EditorView>, cx: &mut TestAppContext, track: TrackId) -> Vec<DeviceId> {
    editor.read_with(cx, |view, _| {
        chain_ids(
            &view.state.as_ref().unwrap().project,
            DevicePanelOwner::Track(track),
        )
    })
}

fn center(visual: &mut VisualTestContext, selector: String) -> gpui::Point<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(selector.into_boxed_str()))
        .unwrap_or_else(|| panic!("missing debug bounds"))
        .center()
}

#[gpui::test]
fn arrows_step_devices_and_paste_lands_in_the_clicked_gap(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, first, _, _) = actor_utility_fixture();
    let type_id = state.project.device(first).unwrap().state().type_id();
    service
        .submit(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(type_id),
            before: None,
        })
        .unwrap();
    let state = receive_service_event(&service).state;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    // Instrument, first effect, second effect.
    let order = chain(&editor, cx, track);
    assert_eq!(order.len(), 3);
    let selected = |editor: &Entity<EditorView>, cx: &mut TestAppContext| {
        editor.read_with(cx, |view, _| view.device_panel.selected_device)
    };

    let title = center(
        &mut visual,
        format!("device-drag-title-{}", order[1].value()),
    );
    visual.simulate_click(title, Modifiers::default());
    visual.simulate_keystrokes("right");
    assert_eq!(selected(&editor, cx), Some(order[2]));
    visual.simulate_keystrokes("right");
    assert_eq!(selected(&editor, cx), Some(order[2]), "end does not wrap");
    visual.simulate_keystrokes("left left");
    assert_eq!(selected(&editor, cx), Some(order[0]));
    let focused = visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.device_panel
                .chain_window
                .device_is_focused(order[0], window)
        })
    });
    assert!(focused, "the stepped-to card owns delete/copy keys");

    visual.simulate_keystrokes("right");
    visual.simulate_keystrokes("cmd-c");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0 && view.pending_requests.is_empty()
    });

    // Paste between the two effects.
    let gap = center(&mut visual, format!("device-insert-{}", order[2].value()));
    visual.simulate_click(gap, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.device_panel.selected_device.is_none());
        assert!(view.device_panel.insertion_is(Some(order[2])));
    });
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && chain_ids(
                &view.state.as_ref().unwrap().project,
                DevicePanelOwner::Track(track),
            )
            .len()
                == 4
    });
    let pasted = chain(&editor, cx, track);
    assert_eq!(&pasted[..2], &order[..2]);
    assert_eq!(pasted[3], order[2], "pasted before the clicked gap");
    visual.run_until_parked();
    visual.simulate_keystrokes("left");
    assert_eq!(
        selected(&editor, cx),
        Some(pasted[2]),
        "left from a gap selects the device before it"
    );

    // The chain tail is the gap after the last device.
    let tail = center(&mut visual, format!("device-drop-end-{}", track.value()));
    visual.simulate_click(tail, Modifiers::default());
    editor.read_with(cx, |view, _| assert!(view.device_panel.insertion_is(None)));
    visual.simulate_keystrokes("cmd-v");
    pump_until(&editor, cx, |view| {
        chain_ids(
            &view.state.as_ref().unwrap().project,
            DevicePanelOwner::Track(track),
        )
        .len()
            == 5
    });
    let appended = chain(&editor, cx, track);
    assert_eq!(&appended[..4], &pasted[..]);
}
