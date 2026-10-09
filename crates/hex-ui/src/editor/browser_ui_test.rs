use super::EditorView;
use gpui::{AppContext, Empty, TestAppContext};
use hex_project::{Editor, Project};
use hex_session::service::{Service, ViewState};
use std::sync::Arc;

#[gpui::test]
fn browser_search_owns_typing_and_blocks_editor_shortcuts(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
                .expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 0,
                    can_undo: false,
                    can_redo: false,
                    revision: 0,
                    unsaved: false,
                    document_path: None,
                }));
                view.browser
                    .search
                    .update(cx, |input, _| input.focus(window));
                cx.notify();
            });
            editor
        })
        .expect("editor window");

    cx.simulate_keystrokes(handle, "shift-a space cmd-shift-t");

    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).content, "a ");
        assert!(
            view.pending_requests.is_empty(),
            "search focus must suppress add-track"
        );
        assert!(
            view.pending_audio_commands.is_empty(),
            "search focus must suppress transport actions"
        );
        assert_eq!(
            view.state
                .as_ref()
                .map(|state| state.project.tracks().len()),
            Some(0)
        );
    });
}

#[gpui::test]
fn browser_project_open_preserves_dirty_work_until_same_item_is_confirmed(cx: &mut TestAppContext) {
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
                .expect("disposable test service");
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            });
            editor.update(cx, |view, cx| {
                let project = Editor::new(Project::new(48_000).expect("test project"), 4)
                    .expect("test editor")
                    .project()
                    .clone();
                view.state = Some(Arc::new(ViewState {
                    project: Arc::new(project),
                    waveforms: Default::default(),
                    midi_preview: Arc::new(Default::default()),
                    timeline_frames: 0,
                    can_undo: true,
                    can_redo: false,
                    revision: 1,
                    unsaved: true,
                    document_path: None,
                }));
                cx.notify();
            });
            editor
        })
        .expect("editor window");
    let first = std::path::PathBuf::from("/tmp/first.hex");
    let second = std::path::PathBuf::from("/tmp/second.hex");

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| view.open_browser_project(first.clone(), cx));
    })
    .expect("editor window");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.pending_open_path.as_ref(), Some(&first));
        assert!(view.pending_requests.is_empty());
    });

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| view.open_browser_project(second.clone(), cx));
    })
    .expect("editor window");
    editor.read_with(cx, |view, _| {
        assert_eq!(view.pending_open_path.as_ref(), Some(&second));
        assert!(view.pending_requests.is_empty());
    });

    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| view.open_browser_project(second.clone(), cx));
    })
    .expect("editor window");
    editor.read_with(cx, |view, _| {
        assert!(view.opening_project);
        assert!(view.pending_open_path.is_none());
    });
}

#[gpui::test]
fn browser_right_edge_drag_resizes_and_release_stops_resizing(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point, px};
    let bootstrap = cx.add_window(|_, _| Empty);
    let handle = *bootstrap;
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service =
                Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES).expect("test service");
            window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline test".into()))
            })
        })
        .expect("editor window");
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Default panel ends at x=280; grab its four-pixel separator midway down.
    visual.simulate_mouse_down(
        point(px(282.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| {
        assert!(view.browser.resizing, "separator must receive mouse down")
    });
    visual.simulate_mouse_move(
        point(px(310.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 310.0));
    visual.simulate_mouse_move(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(cx, |view, _| assert_eq!(view.browser.width, 240.0));
    visual.simulate_mouse_up(
        point(px(240.0), px(200.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(point(px(300.0), px(200.0)), None, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(!view.browser.resizing);
        assert_eq!(view.browser.width, 240.0);
    });
}
