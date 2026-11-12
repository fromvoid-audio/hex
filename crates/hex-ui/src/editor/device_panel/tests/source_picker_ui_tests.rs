//! Explicit source choice remains a worker-owned edit.
use super::*;
use crate::editor::context_menu::EditorContextAction;

fn settle(
    editor: &Entity<EditorView>,
    cx: &mut TestAppContext,
    predicate: impl Fn(&EditorView) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(cx, |view, _| predicate(view)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{}",
            editor.read_with(cx, |view, _| format!(
                "revision={:?} pending={} error={:?} menu={}",
                view.state.as_ref().map(|s| s.revision),
                view.edit_pending(),
                view.error,
                view.context_menu.is_some()
            ))
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn source_picker_assigns_only_after_explicit_choice_and_undoes(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::RenameNativeSource {
            source,
            name: "Bright Source".into(),
        }))
        .unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "Unused Source".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }))
        .unwrap();
    let unused_event = receive_service_event(&service);
    let Created::NativeSource(unused) = unused_event.created.unwrap() else {
        panic!("source")
    };
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "Target".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(target) = event.created.unwrap() else {
        panic!("target")
    };
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let button = visual
        .debug_bounds("device-source-picker")
        .expect("source picker");
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().expect("source choices");
        assert_eq!(menu.items.len(), 3);
        let EditorContextAction::SourcePicker(choice) = &menu.items[0].action else {
            panic!("choice")
        };
        let label = crate::editor::context_menu::context_item_label(&menu.items[0]);
        assert!(theme::text(label).contains("1 midi · Bright Source"));
        let unused_label = crate::editor::context_menu::context_item_label(&menu.items[2]);
        assert!(theme::text(unused_label).contains("0 midi · Unused Source · receiver none"));
        assert!(choice.current(view));
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.context_menu.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let menu = visual.debug_bounds("context-menu-panel").unwrap();
    visual.simulate_click(
        menu.origin + point(px(20.0), px(15.0)),
        Modifiers::default(),
    );
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(target).unwrap().source_id() == Some(source))
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(target).unwrap().source_id().is_none())
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| view.focus.focus(window));
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("shift-tab");
    visual.run_until_parked();
    let picker_focused = visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            view.device_panel
                .source_picker
                .focus
                .as_ref()
                .unwrap()
                .is_focused(window)
        })
    });
    assert!(
        picker_focused,
        "tab navigation did not reach the source picker"
    );
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(
            view.context_menu.is_some(),
            "keyboard did not open source picker"
        )
    });
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(target).unwrap().source_id() == Some(unused))
    });
}

#[gpui::test]
fn source_picker_refuses_pending_gesture_and_stale_choice(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, receiver, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "target".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(target) = event.created.unwrap() else {
        panic!("target")
    };
    let original = event.state.project.clone();
    let source = original.track(receiver).unwrap().source_id().unwrap();
    let (handle, editor) = actor_editor_window(cx, service, event.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    editor.update(&mut visual, |view, cx| {
        view.track_selection.replace(&[receiver, target]);
        cx.notify();
    });
    visual.simulate_click(button.center(), Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(view.context_menu.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    editor.update(&mut visual, |view, cx| {
        view.track_selection.replace(&[target]);
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: target },
            ParameterValue::Float(1.0),
        )
        .unwrap();
    });
    visual.simulate_keystrokes("enter");
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_some());
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    editor.update(&mut visual, |view, _| {
        view.cancel_device_gesture().unwrap();
    });
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    visual.run_until_parked();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let (stale, stale_jump) = editor.read_with(&visual, |view, _| {
        let EditorContextAction::SourcePicker(choice) =
            &view.context_menu.as_ref().unwrap().items[0].action
        else {
            panic!("choice")
        };
        let EditorContextAction::SourcePicker(jump) =
            &view.context_menu.as_ref().unwrap().items[1].action
        else {
            panic!("jump")
        };
        (choice.clone(), jump.clone())
    });
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameTrack {
            track: target,
            name: "changed".into()
        })))
    });
    settle(&editor, &mut visual, |view| {
        !view.edit_pending() && view.state.as_ref().unwrap().revision > before
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(view.context_menu.is_none());
            let revision = view.state.as_ref().unwrap().revision;
            stale.perform(view, window, cx);
            stale_jump.perform(view, window, cx);
            assert_eq!(view.selected_track(), Some(target));
            assert_eq!(view.state.as_ref().unwrap().revision, revision);
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(target)
                    .unwrap()
                    .source_id()
                    .is_none()
            );
            assert!(!view.edit_pending());
        });
    });
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let deleted_jump = editor.read_with(&visual, |view, _| {
        let EditorContextAction::SourcePicker(jump) =
            &view.context_menu.as_ref().unwrap().items[1].action
        else {
            panic!("jump")
        };
        jump.clone()
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveTrack(receiver))))
    });
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(receiver)
                .is_none()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            deleted_jump.clone().perform(view, window, cx);
            assert_eq!(view.selected_track(), Some(target));
            assert!(!view.edit_pending());
        })
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveNativeSource(source))))
    });
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .is_none()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            deleted_jump.perform(view, window, cx);
            assert_eq!(view.selected_track(), Some(target));
            assert!(!view.edit_pending());
        })
    });
}

#[gpui::test]
fn source_picker_navigates_to_receiver_and_contributor_without_assigning(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "Contributor".into(),
        }))
        .unwrap();
    let Created::Track(contributor) = receive_service_event(&service).created.unwrap() else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "Target".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(target) = event.created.unwrap() else {
        panic!("target")
    };
    let original = event.state.project.clone();
    let revision = event.state.revision;
    let (handle, editor) = actor_editor_window(cx, service, event.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert_eq!(menu.items.len(), 3);
        assert!(
            theme::text(crate::editor::context_menu::context_item_label(
                &menu.items[1]
            ))
            .contains("receiver")
        );
        assert!(
            theme::text(crate::editor::context_menu::context_item_label(
                &menu.items[2]
            ))
            .contains("midi · Contributor")
        );
    });
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.is_focused(window)))
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_track(), Some(contributor));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(!view.edit_pending());
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.select_track(target, window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let panel = visual.debug_bounds("context-menu-panel").unwrap();
    visual.simulate_click(
        panel.origin + point(px(20.0), px(39.0)),
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.is_focused(window)))
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_track(), Some(receiver));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(!view.edit_pending());
    });
}

#[gpui::test]
fn source_picker_without_sources_is_an_inert_empty_state(cx: &mut TestAppContext) {
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "target".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(track) = event.created.unwrap() else {
        panic!("track")
    };
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.context_menu.is_none());
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert!(!view.edit_pending());
    });
}
