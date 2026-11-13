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
        assert_eq!(menu.items.len(), 4);
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
        assert_eq!(menu.items.len(), 4);
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
fn bound_source_picker_reassigns_unlinks_and_preserves_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let old = state.project.track(receiver).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }))
        .unwrap();
    let Created::Track(track) = receive_service_event(&service).created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track,
            source: Some(old),
        }))
        .unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "other source".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::NativeSource(other) = event.created.unwrap() else {
        panic!("source")
    };
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();

    let button = visual
        .debug_bounds("device-source-picker")
        .expect("bound source picker");
    assert!(visual.debug_bounds("device-source-summary").is_some());
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let (choice, index) = editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        let menu = view.context_menu.as_ref().unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("other source")
            })
            .unwrap();
        let EditorContextAction::SourcePicker(choice) = &menu.items[index].action else {
            panic!("choice")
        };
        (choice.clone(), index)
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track },
            ParameterValue::Float(1.0),
        )
        .unwrap();
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.clone().perform(view, window, cx);
            assert!(view.device_gesture.snapshot().is_some());
            assert_eq!(view.state.as_ref().unwrap().project, original);
        })
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(track).unwrap().source_id() == Some(other))
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.perform(view, window, cx);
            assert!(!view.edit_pending(), "stale choice must not reassign");
        })
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(track).unwrap().source_id() == Some(old))
    });

    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    == "unlink source"
            })
            .unwrap()
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(track).unwrap().source_id().is_none())
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(track).unwrap().source_id() == Some(old))
    });

    visual.run_until_parked();
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let jump = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("go to · receiver")
            })
            .unwrap()
    });
    for _ in 0..jump {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_track(), Some(receiver));
        assert_eq!(view.state.as_ref().unwrap().project, before.project);
        assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
    });
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    == "unlink source"
            })
            .unwrap()
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .error
                .as_deref()
                .is_some_and(|error| error.contains("choose another receiver"))
    });
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, before.project);
        assert_eq!(
            (state.revision, state.can_undo, state.can_redo),
            (before.revision, before.can_undo, before.can_redo)
        );
    });
}

#[gpui::test]
fn bound_source_picker_explicitly_changes_receiver_and_undoes(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }))
        .unwrap();
    let Created::Track(contributor) = receive_service_event(&service).created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, contributor);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    let (choice, index) = editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        let menu = view.context_menu.as_ref().unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("make receiver · contributor")
            })
            .unwrap();
        let EditorContextAction::SourcePicker(choice) = &menu.items[index].action else {
            panic!("choice")
        };
        (choice.clone(), index)
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: contributor },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.clone().perform(view, window, cx);
            assert!(view.device_gesture.snapshot().is_some());
            assert_eq!(view.state.as_ref().unwrap().project, original);
        })
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project.native_source(source).unwrap().receiver() == Some(contributor)
            })
    });
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(receiver).unwrap().source_id(), Some(source));
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
        assert_eq!(
            project.track(receiver).unwrap().devices(),
            original.track(receiver).unwrap().devices()
        );
        assert_eq!(
            project.track(contributor).unwrap().devices(),
            original.track(contributor).unwrap().devices()
        );
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.clone().perform(view, window, cx);
            assert!(!view.edit_pending());
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|error| error.contains("source receiver selection canceled"))
            );
        })
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project.native_source(source).unwrap().receiver() == Some(receiver)
            })
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project.native_source(source).unwrap().receiver() == Some(contributor)
            })
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveTrack(contributor))))
    });
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|s| s.project.track(contributor).is_none())
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            choice.perform(view, window, cx);
            assert!(!view.edit_pending());
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|error| error.contains("source receiver selection canceled"))
            );
        })
    });
}

#[gpui::test]
fn rebind_cancels_a_published_seek_before_callback(cx: &mut TestAppContext) {
    source_edit_cancels_a_published_seek(cx, SourceEdit::Rebind);
}

#[gpui::test]
fn receiver_choice_cancels_a_published_seek_before_callback(cx: &mut TestAppContext) {
    source_edit_cancels_a_published_seek(cx, SourceEdit::Receiver);
}

#[gpui::test]
fn source_deletion_cancels_a_published_seek_before_callback(cx: &mut TestAppContext) {
    source_edit_cancels_a_published_seek(cx, SourceEdit::Delete);
}

#[gpui::test]
fn source_creation_cancels_a_published_seek_before_callback(cx: &mut TestAppContext) {
    source_edit_cancels_a_published_seek(cx, SourceEdit::Create);
}

#[gpui::test]
fn source_rename_cancels_a_published_seek_before_callback(cx: &mut TestAppContext) {
    source_edit_cancels_a_published_seek(cx, SourceEdit::Rename);
}

enum SourceEdit {
    Rebind,
    Receiver,
    Delete,
    Create,
    Rename,
}

fn source_edit_cancels_a_published_seek(cx: &mut TestAppContext, kind: SourceEdit) {
    let _guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    use crate::editor::arrangement_seek::{PendingSeek, SeekIntent, SeekStage};
    use hex_audio::prepared::PublicationPolicy;
    use std::sync::atomic::Ordering::Acquire;
    let (service, initial, receiver, _, _, _) = actor_utility_fixture();
    let source = initial
        .project
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }))
        .unwrap();
    let Created::Track(track) = receive_service_event(&service).created.unwrap() else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track,
            source: Some(source),
        }))
        .unwrap();
    receive_service_event(&service);
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unused".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::NativeSource(unused) = event.created.unwrap() else {
        panic!("source")
    };
    let state = event.state;
    let revision = state.revision;
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &state.project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("private headless control: {error}"),
        }
    };
    let client = output.take_client().unwrap();
    let playback = || {
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
            .unwrap()
    };
    let active = output
        .publication
        .control()
        .unwrap()
        .publish(playback())
        .unwrap();
    let mut block = [[0.0; 2]; 128];
    backend.render(&mut block).unwrap();
    let fence = client.capture_fresh_runtime_fence().unwrap();
    let published = output
        .publication
        .control()
        .unwrap()
        .publish_with_policy(
            playback(),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let mut pending = PendingSeek::from_intent(
        91,
        &SeekIntent {
            kind: crate::editor::arrangement_seek::SeekKind::Fresh,
            recovery_generation: None,
            destination_frame: 128,
            start_playing: true,
            project_generation: 0,
            project_revision: revision,
            prepared_revision: Some(prepared),
            device_preview_identity: None,
        },
        fence,
    );
    pending.stage = SeekStage::Published {
        audio_revision: published,
    };
    let (_handle, editor) = actor_editor_window(cx, service, state, track);
    editor.update(cx, |view, _| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "private control".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.prepared_seek = Some(pending);
        let request = match kind {
            SourceEdit::Receiver => Request::ChooseNativeSourceReceiver {
                project_generation: 0,
                revision,
                source,
                expected_receiver: Some(receiver),
                track,
            },
            SourceEdit::Delete => Request::DeleteRetainedSource {
                project_generation: 0,
                revision,
                source: unused,
            },
            SourceEdit::Create => Request::CreateRetainedNativeSource {
                project_generation: 0,
                revision,
                type_id: Box::new(hex_project::default_instrument_state().unwrap().type_id()),
                name: "new source".into(),
            },
            SourceEdit::Rename => Request::RenameRetainedSource {
                project_generation: 0,
                revision,
                source: unused,
                name: "renamed".into(),
            },
            SourceEdit::Rebind => Request::RebindNativeSource {
                project_generation: 0,
                revision,
                track,
                expected_source: Some(source),
                source: None,
            },
        };
        assert!(view.enqueue(request));
        assert!(view.prepared_seek.as_ref().unwrap().cancelled);
    });
    backend.render(&mut block).unwrap();
    assert_eq!(
        backend.status().unwrap().applied_revision.load(Acquire),
        active
    );
    assert_eq!(
        backend.status().unwrap().rejected_revision(),
        Some((
            published,
            hex_engine::playback::PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    editor.update(cx, |view, _| view.audio = None);
    backend.close();
    drop(output);
}

#[gpui::test]
fn source_manager_lists_and_deletes_without_assigning(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let bound = state.project.track(receiver).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unused source".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::NativeSource(unused) = event.created.unwrap() else {
        panic!("source")
    };
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();

    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let manager_index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    == "manage sources"
            })
            .unwrap()
    });
    for _ in 0..manager_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let unused_index = editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        let menu = view.context_menu.as_ref().unwrap();
        assert!(menu.items.iter().any(|item| {
            theme::text(crate::editor::context_menu::context_item_label(item))
                .contains("1 midi · receiver")
        }));
        menu.items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("unused source · 0 midi · receiver none")
            })
            .unwrap()
    });
    for _ in 0..unused_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let action = editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.context_menu.as_ref().unwrap().items.len(), 2);
        let EditorContextAction::SourceManager(action) =
            &view.context_menu.as_ref().unwrap().items[0].action
        else {
            panic!("delete")
        };
        action.clone()
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: receiver },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            action.clone().perform(view, window, cx);
            assert!(view.device_gesture.snapshot().is_some());
            assert_eq!(view.state.as_ref().unwrap().project, original);
        })
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..manager_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    for _ in 0..unused_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.native_source(unused).is_none())
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.native_source(unused).is_some())
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            action.perform(view, window, cx);
            assert!(!view.edit_pending());
        })
    });

    editor.update(&mut visual, |view, cx| {
        view.selected_track = None;
        view.track_selection.replace(&[]);
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual
        .debug_bounds("source-manager-button")
        .expect("no-track manager");
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("shift-tab");
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(
                view.device_panel
                    .source_manager
                    .focus
                    .as_ref()
                    .unwrap()
                    .is_focused(window)
            );
        })
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.context_menu.is_none()));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let bound_index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("1 midi · receiver")
            })
            .unwrap()
    });
    for _ in 0..bound_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .error
                .as_deref()
                .is_some_and(|error| error.contains("still assigned"))
    });
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, before.project);
        assert_eq!(
            (state.revision, state.can_undo, state.can_redo),
            (before.revision, before.can_undo, before.can_redo)
        );
        assert!(state.project.native_source(bound).is_some());
    });
}

#[gpui::test]
fn source_manager_creates_an_unassigned_instrument_from_empty_project(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let catalog =
        hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Instrument)
            .unwrap();
    let chosen = catalog.last().unwrap();
    let chosen_index = catalog.len() - 1;
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
    editor.update(&mut visual, |view, cx| {
        view.selected_track = None;
        view.track_selection.replace(&[]);
        cx.notify();
    });
    visual.run_until_parked();
    let button = visual.debug_bounds("source-manager-button").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.context_menu.as_ref().unwrap().items.len(), 1);
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let action = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert!(!menu.items.is_empty());
        let EditorContextAction::SourceManager(action) = &menu.items[chosen_index].action else {
            panic!("instrument")
        };
        action.clone()
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.context_menu.is_none());
        view.begin_device_gesture(
            GestureTarget::TrackGain { track },
            ParameterValue::Float(1.0),
        )
        .unwrap();
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            action.clone().perform(view, window, cx);
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert!(view.device_gesture.snapshot().is_some());
        })
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    for _ in 0..chosen_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| !state.project.native_sources().is_empty())
    });
    let created = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let source = &project.native_sources()[0];
        assert_eq!(source.receiver(), None);
        assert_eq!(project.track(track).unwrap().source_id(), None);
        assert_eq!(source.name(), chosen.name);
        assert_eq!(source.instrument().state().type_id(), chosen.type_id);
        source.id()
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.native_source(created).is_none())
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            action.perform(view, window, cx);
            assert!(!view.edit_pending());
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|error| error.contains("project changed"))
            );
        })
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.native_source(created).is_some())
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .source_id(),
            None
        )
    });
}

#[gpui::test]
fn source_manager_renames_without_losing_draft_or_binding(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let source = state.project.track(track).unwrap().source_id().unwrap();
    let original_name = state
        .project
        .native_source(source)
        .unwrap()
        .name()
        .to_owned();
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    == "manage sources"
            })
            .unwrap()
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let rename = editor.read_with(&visual, |view, _| {
        let EditorContextAction::SourceManager(action) =
            &view.context_menu.as_ref().unwrap().items[1].action
        else {
            panic!("rename")
        };
        action.clone()
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            rename.perform(view, window, cx);
            assert!(!super::super::source_naming(view));
            assert!(view.device_gesture.snapshot().is_some());
        })
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(visual.debug_bounds("source-name-editor").is_some());
    visual.simulate_input("Shared Ω");
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.save_project(cx);
            assert!(super::super::source_naming(view));
            assert!(!view.saving_project);
            view.open_dialog(hex_session::DocumentAudioMode::Resident, window, cx);
            assert!(super::super::source_naming(view));
            view.request_quit(window, cx);
            assert!(super::super::source_naming(view));
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .native_source(source)
                    .unwrap()
                    .name(),
                original_name
            );
        })
    });
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project
                    .native_source(source)
                    .is_some_and(|s| s.name() == "Shared Ω")
            })
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .source_id(),
            Some(source)
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project
                    .native_source(source)
                    .is_some_and(|s| s.name() == original_name)
            })
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project
                    .native_source(source)
                    .is_some_and(|s| s.name() == "Shared Ω")
            })
    });
}

#[gpui::test]
fn unassigned_source_name_refuses_invalid_and_stale_drafts(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unused".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::NativeSource(source) = event.created.unwrap() else {
        panic!("source")
    };
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let manager = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    == "manage sources"
            })
            .unwrap()
    });
    for _ in 0..manager {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let unused = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("unused · 0 midi")
            })
            .unwrap()
    });
    for _ in 0..unused {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_input(" ");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(super::super::source_naming(view));
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .name(),
            "unused"
        );
        assert!(view.error.as_deref().is_some_and(|e| e.contains("invalid")));
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("new name");
    editor.update(&mut visual, |view, _| view.project_generation += 1);
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(super::super::source_naming(view));
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .name(),
            "unused"
        );
        assert!(
            view.error
                .as_deref()
                .is_some_and(|e| e.contains("project or edit changed"))
        );
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(!super::super::source_naming(view));
        view.project_generation -= 1;
    });
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..manager {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    for _ in 0..unused {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_input("new name");
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view.state.as_ref().is_some_and(|s| {
                s.project
                    .native_source(source)
                    .is_some_and(|s| s.name() == "new name")
            })
    });
    editor.read_with(&visual, |view, _| {
        let s = view
            .state
            .as_ref()
            .unwrap()
            .project
            .native_source(source)
            .unwrap();
        assert_eq!(s.receiver(), None);
    });
}

#[gpui::test]
fn source_picker_shows_multi_selection_and_keeps_manager_independent(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, receiver, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "unbound".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(unbound) = event.created.unwrap() else {
        panic!("track")
    };
    let before = event.state.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    for selected in [receiver, unbound] {
        editor.update(&mut visual, |view, cx| {
            view.selected_track = Some(selected);
            view.track_selection.replace(&[receiver, unbound]);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("source-picker-multiselect-hint")
                .is_some()
        );
        let picker = visual.debug_bounds("device-source-picker").unwrap();
        let selection = editor.read_with(&visual, |view, _| view.track_selection.clone());
        visual.simulate_click(picker.center(), Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| {
                view.device_panel
                    .source_picker
                    .focus
                    .as_ref()
                    .unwrap()
                    .focus(window);
            })
        });
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.context_menu.is_none());
            assert_eq!(view.track_selection, selection);
            assert_eq!(view.state.as_ref().unwrap().project, before.project);
        });
        assert!(visual.debug_bounds("source-manager-button").is_some());
        visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
        visual.simulate_keystrokes("shift-tab");
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| {
                assert!(
                    view.device_panel
                        .source_manager
                        .focus
                        .as_ref()
                        .unwrap()
                        .is_focused(window)
                );
            })
        });
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
        visual.simulate_keystrokes("enter"); // browse, never assign
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(view.selected_track, Some(selected));
            assert_eq!(view.track_selection, selection);
            assert_eq!(state.project, before.project);
            assert_eq!(
                (state.revision, state.can_undo, state.can_redo),
                (before.revision, before.can_undo, before.can_redo)
            );
        });
    }
    editor.update(&mut visual, |view, cx| {
        view.selected_track = Some(unbound);
        view.track_selection.replace(&[unbound]);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("source-picker-multiselect-hint")
            .is_none()
    );
    assert!(visual.debug_bounds("source-manager-button").is_none());
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
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

#[gpui::test]
fn source_picker_keeps_released_device_preview_until_terminal_ack(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, device) = actor_native_effect_fixture(
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    );
    let original_source = state.project.track(track).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "other source".into(),
            devices: vec![hex_project::default_instrument_state().unwrap()],
            instrument_index: 0,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::NativeSource(other) = event.created.unwrap() else {
        panic!("source")
    };
    // Do not tick the service: the released gesture's terminal remains in UI custody.
    let revision = event.state.revision;
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let choice = editor.read_with(&visual, |view, _| {
        let item = view
            .context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .find(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("other source")
            })
            .unwrap();
        let EditorContextAction::SourcePicker(choice) = &item.action else {
            panic!("source choice")
        };
        choice.clone()
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let selector =
        Box::leak(format!("device-{}-parameter-2-slider-thumb", device.value()).into_boxed_str());
    let initial = visual.debug_bounds(selector).unwrap();
    let start = initial.center();
    let end = point(start.x, start.y + px(24.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let preview = visual.debug_bounds(selector).unwrap();
    assert!(preview.top() > initial.top());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(matches!(
            view.device_gesture.terminal(),
            Some((hex_session::device_gesture::GesturePhase::Commit, _))
        ));
    });
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| editor.update(cx, |view, cx| choice.perform(view, window, cx)));
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector).unwrap(), preview);
        editor.read_with(&visual, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(view.device_gesture.snapshot().is_some());
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(track)
                    .unwrap()
                    .source_id(),
                Some(original_source)
            );
            assert_ne!(Some(original_source), Some(other));
        });
    }
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none()
            && !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision > revision)
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(selector).unwrap(), preview);
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("other source")
            })
            .unwrap()
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.project.track(track).unwrap().source_id() == Some(other))
    });
}

#[gpui::test]
fn group_track_keeps_project_source_manager_reachable(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, receiver, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[receiver]).unwrap(),
            name: "group".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(group) = event.created.unwrap() else {
        panic!("group")
    };
    let before = event.state.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, group);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let selection = editor.read_with(&visual, |view, _| view.track_selection.clone());
    assert!(visual.debug_bounds("device-source-picker").is_none());
    let manager = visual
        .debug_bounds("source-manager-button")
        .expect("group source manager");
    visual.simulate_click(manager.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("shift-tab");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(
                view.device_panel
                    .source_manager
                    .focus
                    .as_ref()
                    .unwrap()
                    .is_focused(window)
            );
        })
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(view.selected_track, Some(group));
        assert_eq!(view.track_selection, selection);
        assert_eq!(state.project, before.project);
        assert_eq!(
            (state.revision, state.can_undo, state.can_redo),
            (before.revision, before.can_undo, before.can_redo)
        );
    });
    // A selection change can precede the next repaint; do not focus the old
    // group header's manager while a different track is already selected.
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            assert_eq!(
                view.device_panel.owner,
                Some(DevicePanelOwner::Track(group))
            );
            view.selected_track = Some(receiver);
            assert!(!super::super::source_manager::focus_from_workspace(
                view, window
            ));
        });
    });
}

#[gpui::test]
fn bus_panels_keep_project_source_manager_reachable(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, track, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddReturnTrack {
            name: "return".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let return_id = event.state.project.return_tracks()[0].id();
    let before = event.state.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    for owner in [DevicePanelOwner::Main, DevicePanelOwner::Return(return_id)] {
        editor.update(&mut visual, |view, cx| {
            view.selected_track = None;
            view.focused_return = match owner {
                DevicePanelOwner::Return(id) => Some(id),
                _ => None,
            };
            view.device_panel.main_selected =
                (owner == DevicePanelOwner::Main).then_some(view.project_generation);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.device_panel.owner, Some(owner))
        });
        let selection = editor.read_with(&visual, |view, _| {
            (
                view.selected_track,
                view.focused_return,
                view.device_panel.main_selected,
                view.track_selection.clone(),
            )
        });
        let manager = visual
            .debug_bounds("source-manager-button")
            .expect("bus source manager");
        visual.simulate_click(manager.center(), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| {
                assert!(
                    view.device_panel
                        .source_manager
                        .focus
                        .as_ref()
                        .unwrap()
                        .is_focused(window)
                );
            })
        });
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some()));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(view.device_panel.owner, Some(owner));
            assert_eq!(
                (
                    view.selected_track,
                    view.focused_return,
                    view.device_panel.main_selected,
                    view.track_selection.clone()
                ),
                selection
            );
            assert_eq!(state.project, before.project);
            assert_eq!(
                (state.revision, state.can_undo, state.can_redo),
                (before.revision, before.can_undo, before.can_redo)
            );
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, _| {
                view.focused_return = None;
                view.device_panel.main_selected = None;
                view.selected_track = Some(track);
                assert!(!super::super::source_manager::focus_from_workspace(
                    view, window
                ));
            })
        });
    }
}

#[gpui::test]
fn source_manager_navigates_linked_tracks_without_assignment(cx: &mut TestAppContext) {
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
    let event = receive_service_event(&service);
    let original = event.state.project.clone();
    let revision = event.state.revision;
    let (handle, editor) = actor_editor_window(cx, service, event.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    editor.update(&mut visual, |view, cx| {
        view.selected_track = None;
        view.track_selection.replace(&[]);
        cx.notify();
    });
    visual.run_until_parked();
    let button = visual.debug_bounds("source-manager-button").unwrap();
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let (receiver_action, contributor_action) = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let find = |needle: &str| {
            let item = menu
                .items
                .iter()
                .find(|item| {
                    theme::text(crate::editor::context_menu::context_item_label(item))
                        .contains(needle)
                })
                .unwrap();
            let EditorContextAction::SourceManager(action) = &item.action else {
                panic!("manager navigation")
            };
            action.clone()
        };
        (find("go to · receiver"), find("go to · midi · Contributor"))
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: receiver },
            ParameterValue::Float(1.0),
        )
        .unwrap();
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            receiver_action.clone().perform(view, window, cx);
            assert_eq!(view.selected_track(), None);
            assert_eq!(view.state.as_ref().unwrap().project, original);
        })
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.project_generation += 1;
            receiver_action.clone().perform(view, window, cx);
            assert_eq!(view.selected_track(), None);
            view.project_generation -= 1;
        })
    });
    // Navigate through the displayed menu with the keyboard.
    visual.simulate_keystrokes("down down enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_track(), Some(receiver));
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(view.state.as_ref().unwrap().project, original);
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            contributor_action.perform(view, window, cx);
            assert_eq!(view.selected_track(), Some(contributor));
            assert_eq!(view.state.as_ref().unwrap().project, original);
            assert!(!view.edit_pending());
        })
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RenameNativeSource {
            source,
            name: "renamed source".into(),
        })));
    });
    settle(&editor, &mut visual, |view| {
        !view.edit_pending() && view.state.as_ref().unwrap().revision > revision
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let changed = view.state.as_ref().unwrap().project.clone();
            receiver_action.perform(view, window, cx);
            assert_eq!(view.selected_track(), Some(contributor));
            assert_eq!(view.state.as_ref().unwrap().project, changed);
        })
    });
}

#[gpui::test]
fn source_picker_adds_first_utility_from_receiver(cx: &mut TestAppContext) {
    source_picker_adds_first_utility(cx, false);
}

#[gpui::test]
fn source_picker_adds_first_utility_from_contributor(cx: &mut TestAppContext) {
    source_picker_adds_first_utility(cx, true);
}

fn source_picker_adds_first_utility(cx: &mut TestAppContext, from_contributor: bool) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "contributor".into(),
        }))
        .unwrap();
    let added = receive_service_event(&service);
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let bound = receive_service_event(&service);
    let local = bound
        .state
        .project
        .track(receiver)
        .unwrap()
        .devices()
        .to_vec();
    let target = if from_contributor {
        contributor
    } else {
        receiver
    };
    let (handle, editor) = actor_editor_window(cx, service, bound.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let (choice, index) = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("add utility to source")
            })
            .expect("source add action");
        let EditorContextAction::SourcePicker(choice) = &menu.items[index].action else {
            panic!("source action")
        };
        (choice.clone(), index)
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: target },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| choice.clone().perform(view, window, cx))
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_some());
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .is_empty()
        );
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 1
    });
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(receiver).unwrap().devices(), local);
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    visual.update(|window, cx| editor.update(cx, |view, cx| choice.perform(view, window, cx)));
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len(),
            1
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .is_empty()
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 1
    });
}

#[gpui::test]
fn source_picker_removes_sole_utility_from_receiver(cx: &mut TestAppContext) {
    source_picker_removes_sole_utility(cx, false);
}

#[gpui::test]
fn source_picker_removes_sole_utility_from_contributor(cx: &mut TestAppContext) {
    source_picker_removes_sole_utility(cx, true);
}

fn source_picker_removes_sole_utility(cx: &mut TestAppContext, from_contributor: bool) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    let instrument = state
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
    let added = receive_service_event(&service);
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let bound = receive_service_event(&service);
    service
        .submit(Request::AppendSourceUtility {
            project_generation: 0,
            revision: bound.state.revision,
            track: receiver,
            source,
            instrument,
        })
        .unwrap();
    let inserted = receive_service_event(&service);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let effect = inserted
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .id();
    let local = inserted
        .state
        .project
        .track(receiver)
        .unwrap()
        .devices()
        .to_vec();
    let target = if from_contributor {
        contributor
    } else {
        receiver
    };
    let (handle, editor) = actor_editor_window(cx, service, inserted.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let (choice, index) = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("remove utility from source")
            })
            .expect("source remove action");
        let EditorContextAction::SourcePicker(choice) = &menu.items[index].action else {
            panic!("source action")
        };
        (choice.clone(), index)
    });
    if !from_contributor {
        use crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::{
            headless_status_output, status_fixture_guard,
        };
        use std::sync::atomic::Ordering;
        let _guard = status_fixture_guard();
        let (mut output, _backend) = headless_status_output();
        let control = output.take_client().unwrap();
        control
            .status()
            .unwrap()
            .playing
            .store(true, Ordering::Release);
        editor.update(&mut visual, |view, _| {
            view.audio = Some(crate::editor_backend::EditorAudio {
                control,
                sample_rate: 48_000,
                label: "private control".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
        });
        editor.read_with(&visual, |view, _| assert!(!choice.current(view)));
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| choice.clone().perform(view, window, cx))
        });
        editor.read_with(&visual, |view, _| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .native_source(source)
                    .unwrap()
                    .post_instrument_effects()
                    .iter()
                    .any(|item| item.id() == effect)
            );
            assert!(!view.edit_pending());
        });
        editor.update(&mut visual, |view, _| {
            view.audio = None;
        });
    }
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: target },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| choice.clone().perform(view, window, cx))
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_some());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()[0]
                .id(),
            effect
        );
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .is_empty()
    });
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(receiver).unwrap().devices(), local);
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    visual.update(|window, cx| editor.update(cx, |view, cx| choice.perform(view, window, cx)));
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .is_empty()
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 1
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .is_empty()
    });
}

#[gpui::test]
fn source_picker_appends_utility_from_receiver(cx: &mut TestAppContext) {
    source_picker_appends_utility(cx, false);
}

#[gpui::test]
fn source_picker_appends_utility_from_contributor(cx: &mut TestAppContext) {
    source_picker_appends_utility(cx, true);
}

fn source_picker_appends_utility(cx: &mut TestAppContext, from_contributor: bool) {
    use hex_lattice_contract::NativeDeviceMetadata;
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    let instrument = state
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
    let added = receive_service_event(&service);
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    receive_service_event(&service);
    let definition = &hex_devices::utility::UTILITY_DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::utility::parameter::BASS_MONO,
            ParameterValue::Bool(true),
        )
        .unwrap();
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: instrument,
            state,
        }))
        .unwrap();
    let inserted = receive_service_event(&service);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let first = inserted
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()[0]
        .clone();
    let local = inserted
        .state
        .project
        .track(receiver)
        .unwrap()
        .devices()
        .to_vec();
    let target = if from_contributor {
        contributor
    } else {
        receiver
    };
    let (handle, editor) = actor_editor_window(cx, service, inserted.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let (choice, index) = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains("add utility to source")
            })
            .expect("append action");
        let EditorContextAction::SourcePicker(choice) = &menu.items[index].action else {
            panic!("source action")
        };
        (choice.clone(), index)
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: target },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| choice.clone().perform(view, window, cx))
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_some());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects(),
            std::slice::from_ref(&first)
        );
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 2
    });
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            &project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()[0],
            &first
        );
        assert_eq!(project.track(receiver).unwrap().devices(), local);
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    visual.update(|window, cx| editor.update(cx, |view, cx| choice.perform(view, window, cx)));
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len(),
            2
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 1
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 2
    });
}

#[gpui::test]
fn source_picker_hides_utility_append_for_other_effect_types(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    let instrument = state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.auto-pan"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: instrument,
            state,
        }))
        .unwrap();
    let inserted = receive_service_event(&service);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let (handle, editor) = actor_editor_window(cx, service, inserted.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        assert!(menu.items.iter().all(|item| {
            !theme::text(crate::editor::context_menu::context_item_label(item))
                .contains("add utility to source")
        }));
    });
}

#[gpui::test]
fn source_picker_removes_first_shared_utility(cx: &mut TestAppContext) {
    source_picker_removes_shared_utility(cx, 0, false);
}
#[gpui::test]
fn source_picker_removes_middle_shared_utility(cx: &mut TestAppContext) {
    source_picker_removes_shared_utility(cx, 1, true);
}
#[gpui::test]
fn source_picker_removes_last_shared_utility(cx: &mut TestAppContext) {
    source_picker_removes_shared_utility(cx, 2, false);
}

fn source_picker_removes_shared_utility(
    cx: &mut TestAppContext,
    target_index: usize,
    from_contributor: bool,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    let instrument = state
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
    let added = receive_service_event(&service);
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let mut current = receive_service_event(&service);
    let utility = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    for _ in 0..3 {
        service
            .submit(Request::Edit(Command::InsertNestedDevice {
                parent: instrument,
                state: utility.clone(),
            }))
            .unwrap();
        current = receive_service_event(&service);
        assert!(current.error.is_none(), "{:?}", current.error);
    }
    let original = current
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()
        .to_vec();
    let effect = original[target_index].id();
    let expected: Vec<_> = original
        .iter()
        .filter(|item| item.id() != effect)
        .cloned()
        .collect();
    let local = current
        .state
        .project
        .track(receiver)
        .unwrap()
        .devices()
        .to_vec();
    let target = if from_contributor {
        contributor
    } else {
        receiver
    };
    let (handle, editor) = actor_editor_window(cx, service, current.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let label = format!("remove utility {} from source", target_index + 1);
    let index = editor.read_with(&visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item)).contains(&label)
            })
            .expect("target action")
    });
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                .len()
                == 2
    });
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project
                .native_source(source)
                .unwrap()
                .post_instrument_effects(),
            expected
        );
        assert_eq!(project.track(receiver).unwrap().devices(), local);
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                == original
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                == expected
    });
}

#[gpui::test]
fn source_picker_does_not_offer_subtree_removal(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    let instrument = state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let utility = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent: instrument,
            state: utility.clone(),
        }))
        .unwrap();
    let parent = receive_service_event(&service);
    let Created::Device(parent) = parent.created.unwrap() else {
        panic!("parent")
    };
    service
        .submit(Request::Edit(Command::InsertNestedDevice {
            parent,
            state: utility,
        }))
        .unwrap();
    let nested = receive_service_event(&service);
    assert!(nested.error.is_none(), "{:?}", nested.error);
    let (handle, editor) = actor_editor_window(cx, service, nested.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let items = &view.context_menu.as_ref().unwrap().items;
        let contains = |text: &str| {
            items.iter().any(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item)).contains(text)
            })
        };
        assert!(!contains("remove utility 1 from source"));
        assert!(contains("remove utility 2 from source"));
        assert!(!contains("move utility 1 later"));
        assert!(!contains("move utility 2 earlier"));
    });
}

#[gpui::test]
fn source_picker_moves_shared_utility_later_from_contributor(cx: &mut TestAppContext) {
    source_picker_moves_shared_utility(cx, false);
}
#[gpui::test]
fn source_picker_moves_shared_utility_earlier_from_receiver(cx: &mut TestAppContext) {
    source_picker_moves_shared_utility(cx, true);
}

fn source_picker_moves_shared_utility(cx: &mut TestAppContext, earlier: bool) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    let instrument = state
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
    let added = receive_service_event(&service);
    let Created::Track(contributor) = added.created.unwrap() else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let mut current = receive_service_event(&service);
    let utility = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    for _ in 0..3 {
        service
            .submit(Request::Edit(Command::InsertNestedDevice {
                parent: instrument,
                state: utility.clone(),
            }))
            .unwrap();
        current = receive_service_event(&service);
        assert!(current.error.is_none(), "{:?}", current.error);
    }
    let original = current
        .state
        .project
        .native_source(source)
        .unwrap()
        .post_instrument_effects()
        .to_vec();
    let expected = vec![
        original[1].clone(),
        original[0].clone(),
        original[2].clone(),
    ];
    let local = current
        .state
        .project
        .track(receiver)
        .unwrap()
        .devices()
        .to_vec();
    let target = if earlier { receiver } else { contributor };
    let (handle, editor) = actor_editor_window(cx, service, current.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let label = if earlier {
        "move utility 2 earlier"
    } else {
        "move utility 1 later"
    };
    let (choice, index) = editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().unwrap();
        let index = menu
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item)).contains(label)
            })
            .expect("move action");
        let EditorContextAction::SourcePicker(choice) = &menu.items[index].action else {
            panic!("source action")
        };
        (choice.clone(), index)
    });
    editor.update(&mut visual, |view, _| {
        view.begin_device_gesture(
            GestureTarget::TrackGain { track: target },
            ParameterValue::Float(1.0),
        )
        .unwrap()
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| choice.clone().perform(view, window, cx))
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects(),
            original
        )
    });
    editor.update(&mut visual, |view, _| view.cancel_device_gesture().unwrap());
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none() && !view.edit_pending()
    });
    editor.update(&mut visual, |view, cx| {
        view.context_menu = None;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    for _ in 0..index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                == expected
    });
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(receiver).unwrap().devices(), local);
        assert_eq!(
            project.track(contributor).unwrap().source_id(),
            Some(source)
        );
    });
    visual.update(|window, cx| editor.update(cx, |view, cx| choice.perform(view, window, cx)));
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects(),
            expected
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                == original
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending()
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .post_instrument_effects()
                == expected
    });
}

fn open_unassigned_fm(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    source_label: &str,
) -> super::super::SourceManagerAction {
    let picker = visual.debug_bounds("device-source-picker").unwrap();
    visual.simulate_click(picker.center(), Modifiers::default());
    visual.run_until_parked();
    let manager = editor.read_with(&*visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    == "manage sources"
            })
            .unwrap()
    });
    for _ in 0..manager {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let source_index = editor.read_with(&*visual, |view, _| {
        view.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item))
                    .contains(source_label)
            })
            .unwrap()
    });
    for _ in 0..source_index {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let edit = editor.read_with(&*visual, |view, _| {
        let item = view
            .context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .find(|item| {
                theme::text(crate::editor::context_menu::context_item_label(item)) == "edit source"
            })
            .unwrap();
        let EditorContextAction::SourceManager(action) = &item.action else {
            panic!("edit source")
        };
        action.clone()
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.update(&mut *visual, |view, _| {
        let tuning = view.state.as_ref().unwrap().project.tuning();
        view.tuning
            .client
            .begin(view.project_generation, tuning)
            .unwrap();
    });
    visual
        .update(|window, cx| editor.update(cx, |view, cx| edit.clone().perform(view, window, cx)));
    editor.update(&mut *visual, |view, _| {
        assert!(view.device_panel.source_manager.source_view.is_none());
        assert!(view.tuning.client.snapshot().is_some());
        assert!(view.error.is_some());
        view.tuning.client.reset();
        view.error = None;
    });
    visual
        .update(|window, cx| editor.update(cx, |view, cx| edit.clone().perform(view, window, cx)));
    visual.run_until_parked();
    edit
}

#[gpui::test]
fn source_manager_edits_unassigned_fm_without_track_authority(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let original_track = state.project.track(receiver).unwrap().clone();
    let fm = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.fm4"),
    )
    .unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "unassigned fm".into(),
            devices: vec![fm],
            instrument_index: 0,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Some(Created::NativeSource(source)) = event.created else {
        panic!("source")
    };
    let device = event
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "later contributor".into(),
        }))
        .unwrap();
    let before_bind = receive_service_event(&service);
    let Some(Created::Track(contributor)) = before_bind.created else {
        panic!("contributor")
    };
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let bound_event = receive_service_event(&service);
    let (handle, editor) = actor_editor_window(cx, service, before_bind.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    let edit = open_unassigned_fm(&mut visual, &editor, "unassigned fm · 0 midi");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Source(source))
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(receiver)
                .unwrap(),
            &original_track
        );
    });
    let parameter = hex_lattice_contract::ParameterId::from_static(1);
    let control = editor.read_with(&visual, |view, _| {
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("source parameter")
        };
        entity.clone()
    });
    // A queued input must not use the old source panel after selection changes,
    // even before the next repaint installs the newly selected owner.
    editor.update(&mut visual, |view, _| view.selected_track = None);
    control.update(&mut visual, |_, cx| {
        cx.emit(NumericControlEvent {
            phase: NumericEventPhase::Begin,
            value: 2.0,
            gesture: None,
        })
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_none())
    });
    editor.update(&mut visual, |view, _| view.selected_track = Some(receiver));
    let generation = editor.read_with(&visual, |view, _| view.project_generation);
    editor.update(&mut visual, |view, _| {
        view.project_generation = generation + 1
    });
    control.update(&mut visual, |_, cx| {
        cx.emit(NumericControlEvent {
            phase: NumericEventPhase::Begin,
            value: 2.0,
            gesture: None,
        })
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_none())
    });
    editor.update(&mut visual, |view, _| view.project_generation = generation);
    enter_typed_value(&mut visual, device, parameter, "2");
    editor.update(&mut visual, |view, cx| {
        assert!(super::super::finalize_controls(view, cx))
    });
    editor.update(&mut visual, |view, cx| {
        view.receive(bound_event);
        cx.notify();
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(
                view.device_panel.owner,
                Some(DevicePanelOwner::Source(source))
            );
            assert!(view.device_gesture.has_pending_terminal());
            let Some(ParameterControl::Float { entity, .. }) = view
                .device_panel
                .controls
                .get(&ParameterKey { device, parameter })
            else {
                panic!("pending source parameter")
            };
            assert!((entity.read(cx).value() - 2.0).abs() < 0.00001);
        });
    }
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            view.device_panel
                .source_manager
                .close_focus
                .as_ref()
                .unwrap()
                .focus(window);
        })
    });
    visual.simulate_keystrokes("enter");
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(
                view.device_panel.owner,
                Some(DevicePanelOwner::Source(source))
            );
            assert!(view.device_gesture.snapshot().is_some());
            let Some(ParameterControl::Float { entity, .. }) = view
                .device_panel
                .controls
                .get(&ParameterKey { device, parameter })
            else {
                panic!("source parameter")
            };
            assert!((entity.read(cx).value() - 2.0).abs() < 0.00001);
        });
    }
    settle(&editor, &mut visual, |view| {
        view.device_gesture.snapshot().is_none()
    });
    visual.run_until_parked();
    let value = |view: &EditorView| {
        hex_engine::native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .instrument()
                .state(),
            parameter,
        )
        .unwrap()
    };
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Track(receiver))
        );
        assert_eq!(value(view), ParameterValue::Float(2.0));
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(receiver)
                .unwrap(),
            &original_track
        );
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        value(view) == ParameterValue::Float(1.0)
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    settle(&editor, &mut visual, |view| {
        value(view) == ParameterValue::Float(2.0)
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: None
        })))
    });
    settle(&editor, &mut visual, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(contributor)
            .unwrap()
            .source_id()
            .is_none()
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveNativeSource(source))))
    });
    settle(&editor, &mut visual, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .native_source(source)
            .is_none()
    });
    visual.update(|window, cx| editor.update(cx, |view, cx| edit.perform(view, window, cx)));
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_some());
        assert_ne!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Source(source))
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(receiver)
                .unwrap(),
            &original_track
        );
    });
}

#[gpui::test]
fn source_manager_retires_preview_with_replaced_instrument_event(cx: &mut TestAppContext) {
    use hex_session::device_gesture::{GestureKey, GestureRequest};
    cx.update(crate::editor::bind_editor_keys);
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let original_track = state.project.track(receiver).unwrap().clone();
    let kind = hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.fm4");
    let fm = hex_engine::default_native_device_state_for_class(NativeDeviceClass::Instrument, kind)
        .unwrap();
    service
        .submit(Request::Edit(Command::CreateNativeSource {
            name: "pending fm".into(),
            devices: vec![fm.clone()],
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
            name: "later contributor".into(),
        }))
        .unwrap();
    let before_bind = receive_service_event(&service);
    let Some(Created::Track(contributor)) = before_bind.created else {
        panic!("contributor")
    };
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    let parameter = hex_lattice_contract::ParameterId::from_static(1);
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::DeviceParameter { device, parameter },
        }))
        .unwrap();
    assert!(
        receive_service_event(&service)
            .gesture_ack
            .unwrap()
            .accepted
    );
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(2.0),
        }))
        .unwrap();
    assert!(
        receive_service_event(&service)
            .active_device_gesture
            .is_some()
    );
    service
        .submit(Request::Edit(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        }))
        .unwrap();
    let bound = receive_service_event(&service);
    assert!(bound.active_device_gesture.is_some());
    service
        .submit(Request::Edit(Command::ReplaceTrackInstrument {
            track: contributor,
            replacing: device,
            state: fm,
        }))
        .unwrap();
    let replaced = receive_service_event(&service);
    let new_device = replaced
        .state
        .project
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    assert_ne!(device, new_device);
    assert!(
        replaced
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.key == key && ack.reason.is_some())
    );
    let (handle, editor) = actor_editor_window(cx, service, before_bind.state, receiver);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1200.0), px(700.0)));
    visual.run_until_parked();
    open_unassigned_fm(&mut visual, &editor, "pending fm · 0 midi");
    enter_typed_value(&mut visual, device, parameter, "2");
    editor.update(&mut visual, |view, cx| {
        assert!(super::super::finalize_controls(view, cx))
    });
    let old_control = editor.read_with(&visual, |view, _| {
        assert_eq!(view.device_gesture.snapshot().unwrap().key, key);
        let Some(ParameterControl::Float { entity, .. }) = view
            .device_panel
            .controls
            .get(&ParameterKey { device, parameter })
        else {
            panic!("source control")
        };
        entity.clone()
    });
    editor.update(&mut visual, |view, cx| {
        view.receive(bound);
        cx.notify();
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, cx| {
            assert_eq!(
                view.device_panel.owner,
                Some(DevicePanelOwner::Source(source))
            );
            assert!(view.device_gesture.has_pending_terminal());
            let Some(ParameterControl::Float { entity, .. }) = view
                .device_panel
                .controls
                .get(&ParameterKey { device, parameter })
            else {
                panic!("pending source control")
            };
            assert!((entity.read(cx).value() - 2.0).abs() < 0.00001);
        });
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(replaced);
        cx.notify();
    });
    visual.run_until_parked();
    old_control.update(&mut visual, |_, cx| {
        cx.emit(NumericControlEvent {
            phase: NumericEventPhase::Begin,
            value: 3.0,
            gesture: None,
        })
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.device_gesture.snapshot().is_none());
        assert_eq!(
            view.device_panel.owner,
            Some(DevicePanelOwner::Track(receiver))
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .native_source(source)
                .unwrap()
                .instrument()
                .id(),
            new_device
        );
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(receiver)
                .unwrap(),
            &original_track
        );
    });
}
