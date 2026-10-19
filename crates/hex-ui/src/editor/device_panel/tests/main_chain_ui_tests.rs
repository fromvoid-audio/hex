//! Actual Main header/add-menu input and Service ownership.
use super::*;
use crate::editor::test_environment::Environment;

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.state.is_some() && !view.edit_pending()
        });
        visual.run_until_parked();
        if done {
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::yield_now();
    }
}
#[gpui::test]
fn main_header_add_effect_uses_actual_main_chain_and_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 8_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.), px(850.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "ordinary".into()
        })))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| assert!(view.selected_track.is_some()));
    let main = visual.debug_bounds("main-track-header").unwrap();
    visual.simulate_click(
        point(main.left() + px(14.), main.center().y),
        Modifiers::default(),
    );
    editor.read_with(&visual, |view, _| {
        assert!(
            view.selected_track.is_none(),
            "Main selection must not retain the ordinary track target"
        );
        assert!(view.focused_return.is_none());
    });
    visual.run_until_parked();
    let add = visual
        .debug_bounds("device-add-toggle")
        .expect("Main device add control");
    visual.simulate_click(add.center(), Modifiers::default());
    visual.run_until_parked();
    let utility = visual
        .debug_bounds("add-main-effect-hex.utility")
        .expect("actual Main effect menu");
    visual.simulate_click(utility.center(), Modifiers::default());
    pump(&editor, &mut visual);
    let changed = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.main_devices().len(), 1);
        assert!(project.tracks()[0].devices().is_empty());
        assert_eq!(project.tracks().len(), 1);
        project.main_devices()[0].id()
    });
    for _ in 0..3 {
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |view, _| assert!(view.selected_track.is_none()));
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .main_devices()
                .is_empty()
        )
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.main_devices()[0].id(),
            changed
        )
    });
    let add = visual.debug_bounds("device-add-toggle").unwrap();
    visual.simulate_click(add.center(), Modifiers::default());
    visual.run_until_parked();
    let utility = visual.debug_bounds("add-main-effect-hex.utility").unwrap();
    visual.simulate_click(utility.center(), Modifiers::default());
    pump(&editor, &mut visual);
    let (first, second, before_drag) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.main_devices().len(), 2);
        (
            state.project.main_devices()[0].id(),
            state.project.main_devices()[1].id(),
            state.project.snapshot(),
        )
    });
    let landing = super::device_reorder_tests::drag_title(
        &mut visual,
        first,
        second,
        DevicePanelOwner::Main,
        true,
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before_drag)
    });
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let state = view.state.as_ref().unwrap();
        assert_eq!(
            state
                .project
                .main_devices()
                .iter()
                .map(|d| d.id())
                .collect::<Vec<_>>(),
            vec![second, first]
        );
        assert!(state.project.tracks()[0].devices().is_empty());
        assert!(view.active_native_device_drag.is_none());
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before_drag)
    });
}
