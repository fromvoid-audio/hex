use super::*;

fn chain(view: &EditorView, owner: DevicePanelOwner) -> Vec<DeviceId> {
    let project = &view.state.as_ref().expect("state").project;
    match owner {
        DevicePanelOwner::Track(track) => project.track(track).expect("track").devices(),
        DevicePanelOwner::Return(owner) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == owner)
            .expect("return")
            .devices(),
    }
    .iter()
    .map(|device| device.id())
    .collect()
}

fn drag_title(
    visual: &mut VisualTestContext,
    device: DeviceId,
    target: DeviceId,
    owner: DevicePanelOwner,
    right_half: bool,
) -> gpui::Point<gpui::Pixels> {
    visual.run_until_parked();
    let title_selector =
        Box::leak(format!("device-drag-title-{}", device.value()).into_boxed_str());
    let prefix = match owner {
        DevicePanelOwner::Track(_) => "device-drop-card",
        DevicePanelOwner::Return(_) => "return-device-drop-card",
    };
    let target_selector = Box::leak(format!("{prefix}-{}", target.value()).into_boxed_str());
    let title = visual
        .debug_bounds(title_selector)
        .expect("draggable title");
    let target = visual.debug_bounds(target_selector).expect("target card");
    assert!(
        title.size.width > px(20.0),
        "title offers a useful grab area"
    );
    visual.simulate_mouse_down(title.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(title.center().x + px(12.0), title.center().y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let landing = point(
        if right_half {
            // The spacing after each card is part of its insertion hit area.
            target.right() - px(1.0)
        } else {
            target.left() + px(8.0)
        },
        target.center().y,
    );
    visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    landing
}

#[gpui::test]
fn device_titles_move_between_left_and_right_without_editing_on_hover(cx: &mut TestAppContext) {
    let (service, mut state, track, first, _, _) = actor_utility_fixture();
    for _ in 0..2 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                    "hex.utility",
                )),
                before: None,
            })
            .expect("add effect");
        state = receive_service_event(&service).state;
    }
    let second = state.project.track(track).expect("track").devices()[1].id();
    let third = state.project.track(track).expect("track").devices()[2].id();
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let owner = DevicePanelOwner::Track(track);
    let mut visual = VisualTestContext::from_window(handle, cx);

    for (source, target, right, expected) in [
        (first, second, true, vec![second, first, third]),
        (third, second, false, vec![third, second, first]),
        (third, first, true, vec![second, first, third]),
    ] {
        let before = editor.read_with(cx, |view, _| chain(view, owner));
        let landing = drag_title(&mut visual, source, target, owner, right);
        editor.read_with(cx, |view, _| {
            assert_eq!(chain(view, owner), before, "hover does not edit project");
            assert!(
                view.active_native_device_drag
                    .as_ref()
                    .expect("drag")
                    .destination
                    .get()
                    .is_some()
            );
        });
        visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |view| {
            chain(view, owner) == expected && view.worker_requests_pending == 0
        });
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().expect("state").revision,
            initial_revision + 3
        )
    });
    editor.update(cx, |view, cx| {
        view.enqueue(Request::Undo);
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        chain(view, owner) == vec![third, second, first]
    });
    let revision = editor.read_with(cx, |view, _| view.state.as_ref().expect("state").revision);
    let landing = drag_title(&mut visual, third, third, owner, false);
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().expect("state").revision, revision);
        assert_eq!(
            view.worker_requests_pending, 0,
            "self-drop does not enqueue a move"
        );
    });
    let instrument = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .expect("state")
            .project
            .track(track)
            .expect("track")
            .instrument()
            .expect("instrument")
            .id()
    });
    let landing = drag_title(&mut visual, instrument, second, owner, false);
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view
                .state
                .as_ref()
                .expect("state")
                .project
                .track(track)
                .expect("track")
                .ordered_device_chain()
                .map(|node| match node {
                    hex_project::TrackDeviceRef::Instrument(device)
                    | hex_project::TrackDeviceRef::Effect(device) => device.id(),
                })
                .collect::<Vec<_>>()
                == vec![third, instrument, second, first]
    });
}

#[gpui::test]
fn return_titles_reorder_with_undo_and_cancel_or_noop_without_history(cx: &mut TestAppContext) {
    let (service, mut state, track, owner, first, _) = actor_return_utility_fixture();
    for _ in 0..2 {
        service
            .submit(Request::Edit(Command::InsertReturnDevice {
                return_track: owner,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .expect("utility"),
            }))
            .expect("add return effect");
        state = receive_service_event(&service).state;
    }
    let devices = state
        .project
        .return_tracks()
        .iter()
        .find(|track| track.id() == owner)
        .expect("return")
        .devices();
    let (second, third) = (devices[1].id(), devices[2].id());
    let initial_revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(owner);
        cx.notify();
    });
    let panel_owner = DevicePanelOwner::Return(owner);
    let mut visual = VisualTestContext::from_window(handle, cx);

    for (source, target, right, expected) in [
        (first, second, true, vec![second, first, third]),
        (third, second, false, vec![third, second, first]),
        (third, first, true, vec![second, first, third]),
    ] {
        let before = editor.read_with(cx, |view, _| chain(view, panel_owner));
        let landing = drag_title(&mut visual, source, target, panel_owner, right);
        editor.read_with(cx, |view, _| {
            assert_eq!(chain(view, panel_owner), before);
            assert!(
                view.active_native_device_drag
                    .as_ref()
                    .expect("drag")
                    .return_destination
                    .get()
                    .is_some()
            );
        });
        visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, |view| {
            chain(view, panel_owner) == expected && view.worker_requests_pending == 0
        });
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().expect("state").revision,
            initial_revision + 3
        )
    });
    editor.update(cx, |view, cx| {
        view.enqueue(Request::Undo);
        cx.notify();
    });
    pump_until(&editor, cx, |view| {
        chain(view, panel_owner) == vec![third, second, first] && view.worker_requests_pending == 0
    });
    let revision = editor.read_with(cx, |view, _| view.state.as_ref().expect("state").revision);
    let landing = drag_title(&mut visual, third, third, panel_owner, false);
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for escape in [true, false] {
        let _ = drag_title(&mut visual, third, first, panel_owner, true);
        if escape {
            visual.simulate_keystrokes("escape");
        }
        let outside = point(px(500.0), px(100.0));
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                view.active_native_device_drag
                    .as_ref()
                    .is_none_or(|drag| drag.return_destination.get().is_none())
            );
        });
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        assert!(view.active_native_device_drag.is_none());
        assert_eq!(view.state.as_ref().expect("state").revision, revision);
        assert_eq!(chain(view, panel_owner), vec![third, second, first]);
        assert_eq!(view.worker_requests_pending, 0);
    });
}
