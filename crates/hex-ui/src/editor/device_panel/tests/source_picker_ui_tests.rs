//! Explicit source choice remains a worker-owned edit.
use super::*;
use crate::editor::context_menu::EditorContextAction;

fn settle(editor: &Entity<EditorView>, cx: &mut TestAppContext, predicate: impl Fn(&EditorView) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        editor.update(cx, |view, cx| { view.tick(cx); cx.notify(); });
        if editor.read_with(cx, |view, _| predicate(view)) { return; }
        assert!(std::time::Instant::now() < deadline, "{}", editor.read_with(cx, |view, _| format!("revision={:?} pending={} error={:?} menu={}", view.state.as_ref().map(|s| s.revision), view.edit_pending(), view.error, view.context_menu.is_some())));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn source_picker_assigns_only_after_explicit_choice_and_undoes(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, receiver, _, _, _) = actor_utility_fixture();
    let source = state.project.track(receiver).unwrap().source_id().unwrap();
    service.submit(Request::Edit(Command::RenameNativeSource { source, name: "Bright Source".into() })).unwrap();
    receive_service_event(&service);
    service.submit(Request::Edit(Command::CreateNativeSource {
        name: "Unused Source".into(),
        devices: vec![hex_project::default_instrument_state().unwrap()],
        instrument_index: 0,
    })).unwrap();
    let unused_event = receive_service_event(&service);
    let Created::NativeSource(unused) = unused_event.created.unwrap() else { panic!("source") };
    service.submit(Request::Edit(Command::AddMidiTrack { name: "Target".into() })).unwrap();
    let event = receive_service_event(&service);
    let Created::Track(target) = event.created.unwrap() else { panic!("target") };
    let original = event.state.project.clone();
    let (handle, editor) = actor_editor_window(cx, service, event.state, target);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(900.0), px(700.0)));
    visual.run_until_parked();
    let button = visual.debug_bounds("device-source-picker").expect("source picker");
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let menu = view.context_menu.as_ref().expect("source choices");
        assert_eq!(menu.items.len(), 2);
        let EditorContextAction::SourcePicker(choice) = &menu.items[0].action else { panic!("choice") };
        let label = crate::editor::context_menu::context_item_label(&menu.items[0]);
        assert!(theme::text(label).contains("Bright Source · 1 midi"));
        let unused_label = crate::editor::context_menu::context_item_label(&menu.items[1]);
        assert!(theme::text(unused_label).contains("Unused Source · 0 midi · receiver none"));
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
    visual.simulate_click(menu.origin + point(px(20.0), px(15.0)), Modifiers::default());
    settle(&editor, &mut visual, |view| {
        !view.edit_pending() && view.state.as_ref().is_some_and(|s| s.project.track(target).unwrap().source_id() == Some(source))
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    settle(&editor, &mut visual, |view| {
        !view.edit_pending() && view.state.as_ref().is_some_and(|s| s.project.track(target).unwrap().source_id().is_none())
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, _| {
            assert!(view.device_panel.source_picker.bounds.get().is_some(), "picker anchor missing");
            window.focus(view.device_panel.source_picker.focus.as_ref().unwrap());
            assert!(view.device_panel.source_picker.focus.as_ref().unwrap().is_focused(window));
        });
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| assert!(view.context_menu.is_some(), "keyboard did not open source picker"));
    visual.simulate_keystrokes("down");
    visual.simulate_keystrokes("enter");
    settle(&editor, &mut visual, |view| {
        !view.edit_pending() && view.state.as_ref().is_some_and(|s| s.project.track(target).unwrap().source_id() == Some(unused))
    });
}
