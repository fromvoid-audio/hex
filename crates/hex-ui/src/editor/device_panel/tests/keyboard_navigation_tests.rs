use super::*;

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

fn click_device(visual: &mut VisualTestContext, device: DeviceId, modifiers: Modifiers) {
    let point = center(visual, format!("device-drag-title-{}", device.value()));
    visual.simulate_click(point, modifiers);
    visual.run_until_parked();
}

fn apply(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = receive_service_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

fn settled(view: &EditorView) -> bool {
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.device_panel.pending_duplicate.is_none()
}

#[gpui::test]
fn device_d_duplicates_single_and_group_in_place_selects_copies_and_undoes(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, first, parameter, original) = actor_utility_fixture();
    apply(
        &service,
        Request::SetDeviceParameter {
            device: first,
            parameter,
            value: ParameterValue::Bool(!original),
        },
    );
    apply(
        &service,
        Request::Edit(Command::ToggleDeviceBypass { device: first }),
    );
    let state = apply(
        &service,
        Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(state.project.device(first).unwrap().state().type_id()),
            before: None,
        },
    )
    .state;
    let second = state.project.track(track).unwrap().devices()[1].id();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let original_devices = vec![instrument, first, second];
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();

    click_device(&mut visual, first, Modifiers::default());
    visual.simulate_keystrokes("d");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    let single = chain(&editor, cx, track);
    let copy = single[2];
    assert_eq!(single, [instrument, first, copy, second]);
    assert!(!original_devices.contains(&copy));
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(view.device_panel.selected_device, Some(copy));
        assert_eq!(
            project.device(copy).unwrap().state(),
            project.device(first).unwrap().state()
        );
        assert!(project.device(copy).unwrap().bypassed());
    });

    // The same keyboard scope remains active after worker acknowledgement.
    visual.simulate_keystrokes("d");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    let repeated = chain(&editor, cx, track);
    assert_eq!(repeated, [instrument, first, copy, repeated[3], second]);
    assert!(!single.contains(&repeated[3]));
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    assert_eq!(chain(&editor, cx, track), single);
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    assert_eq!(chain(&editor, cx, track), original_devices);
    visual.run_until_parked();

    click_device(&mut visual, first, Modifiers::default());
    click_device(
        &mut visual,
        second,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    visual.simulate_keystrokes("d");
    pump_until(&editor, cx, settled);
    visual.run_until_parked();
    let group = chain(&editor, cx, track);
    assert_eq!(group.len(), 5);
    assert_eq!(&group[..3], &original_devices);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        for (source, copy) in [first, second].into_iter().zip(group[3..].iter().copied()) {
            assert!(!original_devices.contains(&copy));
            assert!(view.device_panel.is_selected(copy));
            assert!(!view.device_panel.is_selected(source));
            assert_eq!(
                project.device(copy).unwrap().state(),
                project.device(source).unwrap().state()
            );
        }
    });
    visual.simulate_keystrokes("cmd-z");
    pump_until(&editor, cx, settled);
    assert_eq!(chain(&editor, cx, track), original_devices);
    visual.simulate_keystrokes("cmd-shift-z");
    pump_until(&editor, cx, settled);
    assert_eq!(chain(&editor, cx, track), group);
}

#[gpui::test]
fn device_d_refuses_stale_cards_and_instruments_and_leaves_nested_text_alone(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let gain = hex_engine::native_device_view(state.project.device(effect).unwrap().state())
        .unwrap()
        .parameters
        .into_iter()
        .find(|parameter| parameter.descriptor.label == "gain")
        .unwrap()
        .descriptor
        .id;
    let newer = apply(
        &service,
        Request::Edit(Command::RenameTrack {
            track,
            name: "newer revision".into(),
        }),
    )
    .state;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    click_device(&mut visual, effect, Modifiers::default());
    // Advance the view revision without repainting the focused card.
    visual.update(|window, cx| {
        editor.update(cx, |view, _| view.state = Some(newer.clone()));
        window.dispatch_keystroke(gpui::Keystroke::parse("d").unwrap(), cx);
        editor.update(cx, |view, cx| {
            assert!(settled(view));
            assert_eq!(
                view.error.as_deref(),
                Some("device copy canceled · selection changed")
            );
            cx.notify();
        });
    });
    visual.run_until_parked();

    click_device(&mut visual, instrument, Modifiers::default());
    visual.simulate_keystrokes("d");
    editor.read_with(cx, |view, _| {
        assert!(settled(view));
        assert_eq!(
            view.error.as_deref(),
            Some("destination cannot accept another instrument")
        );
    });
    click_device(&mut visual, effect, Modifiers::default());
    enter_typed_value(&mut visual, effect, gain, "1");
    visual.simulate_keystrokes("d");
    editor.read_with(cx, |view, cx| {
        let ParameterControl::Float { entity, .. } = &view.device_panel.controls[&ParameterKey {
            device: effect,
            parameter: gain,
        }] else {
            panic!("gain control")
        };
        assert_eq!(entity.read(cx).display_text(), "1d");
        assert!(settled(view));
        assert_eq!(view.state.as_ref().unwrap().revision, newer.revision);
    });
}

#[gpui::test]
fn device_d_copies_a_nested_unit_after_its_children_and_preserves_newer_selection(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, root, _, _) = actor_utility_fixture();
    let authored = state.project.device(root).unwrap().state().clone();
    let child_event = apply(
        &service,
        Request::Edit(Command::InsertNestedDevice {
            parent: root,
            state: authored.clone(),
        }),
    );
    let Some(Created::Device(child)) = child_event.created else {
        panic!("child")
    };
    let next_event = apply(
        &service,
        Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: authored,
        }),
    );
    let Some(Created::Device(next)) = next_event.created else {
        panic!("next")
    };
    let (handle, editor) = actor_editor_window(cx, service, next_event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    click_device(&mut visual, root, Modifiers::default());
    visual.simulate_keystrokes("d");
    // Move selection while the worker's terminal still belongs to the copy.
    assert!(editor.read_with(cx, |view, _| view.device_panel.pending_duplicate.is_some()));
    click_device(&mut visual, next, Modifiers::default());
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let devices = project.track(track).unwrap().devices();
        assert_eq!(devices.len(), 5);
        assert_eq!(
            [devices[0].id(), devices[1].id(), devices[4].id()],
            [root, child, next]
        );
        assert_eq!(devices[1].parent(), Some(root));
        assert_eq!(devices[2].parent(), None);
        assert_eq!(devices[3].parent(), Some(devices[2].id()));
        assert_eq!(view.device_panel.selected_device, Some(next));
    });
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
