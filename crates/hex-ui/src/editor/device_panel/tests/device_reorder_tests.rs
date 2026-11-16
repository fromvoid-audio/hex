use super::*;

fn chain(view: &EditorView, owner: DevicePanelOwner) -> Vec<DeviceId> {
    let project = &view.state.as_ref().expect("state").project;
    match owner {
        DevicePanelOwner::Main => project.main_devices(),
        DevicePanelOwner::Track(track) => project.track(track).expect("track").devices(),
        DevicePanelOwner::Return(owner) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == owner)
            .expect("return")
            .devices(),
        DevicePanelOwner::Source(_) => panic!("source view has no reorder chain"),
    }
    .iter()
    .map(|device| device.id())
    .collect()
}

pub(super) fn drag_title(
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
        DevicePanelOwner::Main => "main-device-drop-card",
        DevicePanelOwner::Track(_) => "device-drop-card",
        DevicePanelOwner::Return(_) => "return-device-drop-card",
        DevicePanelOwner::Source(_) => panic!("source view has no reorder card"),
    };
    let target_selector = Box::leak(format!("{prefix}-{}", target.value()).into_boxed_str());
    let title = visual
        .debug_bounds(title_selector)
        .expect("draggable title");
    let target = visual.debug_bounds(target_selector).expect("target card");
    assert!(
        title.size.width.min(title.size.height) >= px(16.0)
            && title.size.width.max(title.size.height) > px(20.0),
        "the horizontal title or vertical sidebar offers a useful grab area: {title:?}"
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
fn plain_drag_reorders_the_whole_selection_in_each_chain(cx: &mut TestAppContext) {
    for kind in 0..3 {
        let (service, mut state, track, owner) = if kind == 2 {
            let (service, state, track, id, _, _) = actor_return_utility_fixture();
            (service, state, track, DevicePanelOwner::Return(id))
        } else {
            let (service, state, track, _, _, _) = actor_utility_fixture();
            let owner = if kind == 0 {
                DevicePanelOwner::Track(track)
            } else {
                DevicePanelOwner::Main
            };
            (service, state, track, owner)
        };
        let state_for_effect = || {
            hex_engine::default_native_device_state(hex_devices::utility::UTILITY_TYPE_ID).unwrap()
        };
        for _ in 0..if kind == 1 { 4 } else { 3 } {
            let command = match owner {
                DevicePanelOwner::Track(track) => Command::InsertDevice {
                    track,
                    before: None,
                    state: state_for_effect(),
                },
                DevicePanelOwner::Main => Command::InsertMainDevice {
                    before: None,
                    state: state_for_effect(),
                },
                DevicePanelOwner::Return(return_track) => Command::InsertReturnDevice {
                    return_track,
                    before: None,
                    state: state_for_effect(),
                },
                DevicePanelOwner::Source(_) => panic!("source view does not insert devices"),
            };
            service.submit(Request::Edit(command)).unwrap();
            state = receive_service_event(&service).state;
        }
        let original = state.project.clone();
        let revision = state.revision;
        let (handle, editor) = actor_editor_window(cx, service, state, track);
        editor.update(cx, |view, cx| {
            match owner {
                DevicePanelOwner::Track(_) => {}
                DevicePanelOwner::Main => {
                    view.selected_track = None;
                    view.device_panel.main_selected = Some(view.project_generation)
                }
                DevicePanelOwner::Return(id) => view.focused_return = Some(id),
                DevicePanelOwner::Source(_) => panic!("source view is not a reorder owner"),
            }
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(2000.), px(800.)));
        visual.run_until_parked();
        let [a, b, c, d] = editor
            .read_with(cx, |view, _| chain(view, owner))
            .try_into()
            .unwrap();
        let title = |visual: &mut VisualTestContext, id: DeviceId| {
            visual
                .debug_bounds(Box::leak(
                    format!("device-drag-title-{}", id.value()).into_boxed_str(),
                ))
                .unwrap_or_else(|| panic!("missing title for {id:?} in {owner:?}"))
                .center()
        };
        let c_title = title(&mut visual, c);
        let a_title = title(&mut visual, a);
        visual.simulate_click(c_title, Modifiers::default());
        visual.simulate_click(
            a_title,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );

        let drop_at = |visual: &mut VisualTestContext, source, target, right| {
            let landing = drag_title(visual, source, target, owner, right);
            visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
        };
        let idle = |view: &EditorView| {
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        };
        drop_at(&mut visual, a, d, true);
        pump_until(&editor, cx, idle);
        let later = editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(chain(view, owner), [b, d, a, c]);
            assert!(view.device_panel.is_selected(a) && view.device_panel.is_selected(c));
            assert_eq!(view.state.as_ref().unwrap().revision, revision + 1);
            view.state.as_ref().unwrap().project.clone()
        });

        drop_at(&mut visual, c, b, false);
        pump_until(&editor, cx, idle);
        let earlier = editor.read_with(cx, |view, _| {
            assert_eq!(chain(view, owner), [a, c, b, d]);
            assert_eq!(view.state.as_ref().unwrap().revision, revision + 2);
            view.state.as_ref().unwrap().project.clone()
        });
        drop_at(&mut visual, a, c, false);
        pump_until(&editor, cx, idle);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().revision, revision + 2)
        });
        for (request, project) in [
            (Request::Undo, later.clone()),
            (Request::Undo, original),
            (Request::Redo, later),
            (Request::Redo, earlier),
        ] {
            editor.update(cx, |view, _| assert!(view.enqueue(request)));
            pump_until(&editor, cx, idle);
            editor.read_with(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, project)
            });
        }
    }
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

#[gpui::test]
fn option_drag_copies_the_selected_group_and_undoes_together(cx: &mut TestAppContext) {
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
            .unwrap();
        state = receive_service_event(&service).state;
    }
    let second = state.project.track(track).unwrap().devices()[1].id();
    let third = state.project.track(track).unwrap().devices()[2].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1800.0), px(800.0)));
    visual.run_until_parked();
    let title = |visual: &mut VisualTestContext, id: DeviceId| {
        visual
            .debug_bounds(Box::leak(
                format!("device-drag-title-{}", id.value()).into_boxed_str(),
            ))
            .unwrap()
            .center()
    };
    let first_title = title(&mut visual, first);
    let second_title = title(&mut visual, second);
    visual.simulate_click(first_title, Modifiers::default());
    visual.simulate_click(
        second_title,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    visual.run_until_parked();
    let target = visual
        .debug_bounds(Box::leak(
            format!("device-drop-card-{}", third.value()).into_boxed_str(),
        ))
        .unwrap();
    let landing = point(target.left() + px(8.0), target.center().y);
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    // Press Option after the pointer goes down: original group membership must survive.
    visual.simulate_mouse_down(first_title, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(first_title.x + px(12.0), first_title.y),
        MouseButton::Left,
        option,
    );
    visual.simulate_mouse_move(landing, MouseButton::Left, option);
    visual.run_until_parked();
    visual.simulate_mouse_up(landing, MouseButton::Left, option);
    pump_until(&editor, cx, |view| {
        chain(view, DevicePanelOwner::Track(track)).len() == 5
    });
    editor.read_with(cx, |view, _| {
        let devices = view
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices();
        assert_eq!(
            [devices[0].id(), devices[1].id(), devices[4].id()],
            [first, second, third]
        );
        assert!(![first, second, third].contains(&devices[2].id()));
        assert!(![first, second, third].contains(&devices[3].id()));
        assert_eq!(devices[0].state(), devices[2].state());
        assert_eq!(devices[1].state(), devices[3].state());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        chain(view, DevicePanelOwner::Track(track)) == vec![first, second, third]
    });
}

#[gpui::test]
fn option_drag_copies_effects_on_return_and_main(cx: &mut TestAppContext) {
    let (service, state, track, return_id, return_device, _) = actor_return_utility_fixture();
    let utility = state.project.device(return_device).unwrap().state().clone();
    service
        .submit(Request::Edit(Command::InsertMainDevice {
            before: None,
            state: utility,
        }))
        .unwrap();
    let state = receive_service_event(&service).state;
    let main_device = state.project.main_devices()[0].id();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, cx| {
        view.focused_return = Some(return_id);
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(800.0)));
    visual.run_until_parked();
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    let owner = DevicePanelOwner::Return(return_id);
    let landing = drag_title(&mut visual, return_device, return_device, owner, false);
    visual.simulate_mouse_move(landing, MouseButton::Left, option);
    visual.simulate_mouse_up(landing, MouseButton::Left, option);
    pump_until(&editor, cx, |view| chain(view, owner).len() == 2);
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        chain(view, owner) == vec![return_device]
    });
    editor.update(cx, |view, cx| {
        view.focused_return = None;
        view.selected_track = None;
        view.device_panel.main_selected = Some(view.project_generation);
        cx.notify();
    });
    visual.run_until_parked();
    let landing = drag_title(
        &mut visual,
        main_device,
        main_device,
        DevicePanelOwner::Main,
        false,
    );
    visual.simulate_mouse_move(landing, MouseButton::Left, option);
    visual.simulate_mouse_up(landing, MouseButton::Left, option);
    pump_until(&editor, cx, |view| {
        chain(view, DevicePanelOwner::Main).len() == 2
    });
    editor.read_with(cx, |view, _| {
        assert_ne!(
            view.state.as_ref().unwrap().project.main_devices()[0].id(),
            main_device
        )
    });
}

#[gpui::test]
fn option_drag_instrument_into_occupied_track_refuses_without_changes(cx: &mut TestAppContext) {
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let instrument = state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let before = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let landing = drag_title(
        &mut visual,
        instrument,
        effect,
        DevicePanelOwner::Track(track),
        false,
    );
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_move(landing, MouseButton::Left, option);
    visual.simulate_mouse_up(landing, MouseButton::Left, option);
    pump_until(&editor, cx, |view| view.error.is_some());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, before);
    });
}

#[gpui::test]
fn option_drag_cancellation_keeps_the_chain_unchanged(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let before = state.project.clone();
    let revision = state.revision;
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let landing = drag_title(
        &mut visual,
        effect,
        effect,
        DevicePanelOwner::Track(track),
        false,
    );
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_move(landing, MouseButton::Left, option);
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(landing, MouseButton::Left, option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before);
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert!(view.active_native_device_drag.is_none());
    });
}

#[gpui::test]
fn option_drag_copy_applies_to_the_playing_callback(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    let _guard = eq_ui_tests::graph_layout_tests::status_fixture_guard();
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, effect, _, _) = actor_utility_fixture();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track,
            name: "copy drag ready".into()
        })))
    });
    let (mut output, mut backend) =
        eq_ui_tests::held_pointer_tests::attach_actual_output(&editor, cx, &mut visual);
    visual.simulate_keystrokes("space");
    let deadline = crate::test_wait::hang_deadline();
    while !backend.status().unwrap().playing.load(Acquire) {
        eq_ui_tests::held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "playback did not start"
        );
    }
    let landing = drag_title(
        &mut visual,
        effect,
        effect,
        DevicePanelOwner::Track(track),
        false,
    );
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_move(landing, MouseButton::Left, option);
    visual.simulate_mouse_up(landing, MouseButton::Left, option);
    loop {
        eq_ui_tests::held_pointer_tests::pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        let applied = editor.read_with(cx, |view, _| {
            view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .track(track)
                    .is_some_and(|track| track.devices().len() == 2)
                    && view.applied_project_revision() == Some(state.revision)
            })
        });
        if applied && backend.status().unwrap().playing.load(Acquire) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "copy drag did not reach playing callback: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
    }
}
