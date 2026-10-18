//! Workspace Tab dispatch uses an isolated browser before worker startup.
use super::{EditorMode, EditorView};
use gpui::{
    AppContext, Entity, Focusable, Modifiers, MouseButton, TestAppContext, VisualTestContext,
};
use hex_session::service::Service;
use std::time::{Duration, Instant};

fn fixture(
    cx: &mut TestAppContext,
) -> (
    super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
) {
    cx.update(super::bind_editor_keys);
    let files = super::external_audio_drop_ui_test::BatchFiles::new();
    let path = files.wav("fixture.wav", 48_000, 8);
    let directory = path.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let event = loop {
        if let Ok(event) = service.try_event() {
            break event;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor)
}

#[gpui::test]
fn workspace_tab_switches_both_directions_without_edit_or_view_state_loss(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(super::viewport::Viewport {
            start: 1000,
            length: 24_000,
        });
        v.session_track_start = 2;
        v.clip_view_open = false;
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrange view").is_some());
    visual.simulate_keystrokes("tab");
    visual.run_until_parked();
    assert!(visual.debug_bounds("console view").is_some());
    assert!(visual.debug_bounds("arrange view").is_none());
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.mode, EditorMode::Arrangement);
        assert_eq!(
            v.viewport,
            Some(super::viewport::Viewport {
                start: 1000,
                length: 24_000
            })
        );
        assert_eq!(v.session_track_start, 2);
        assert!(!v.clip_view_open);
        assert_eq!(v.state.as_ref().unwrap().project, before.project);
        assert_eq!(v.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(v.pending_requests.is_empty());
    });
}

#[gpui::test]
fn workspace_tab_leaves_browser_numeric_and_nested_focus_in_charge(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let browser = editor.read_with(&visual, |v, cx| v.browser.search.read(cx).focus_handle());
    visual.update(|window, _| browser.focus(window));
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    // The real numeric control owns its local draft and Tab routing.
    let bounds = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    // Both detail editors use their own focus, even though they descend from the workspace.
    for audio in [false, true] {
        visual.update(|window, cx| {
            editor.update(cx, |v, _| {
                if audio {
                    v.audio_event_focus.focus(window);
                } else {
                    v.midi_focus.focus(window);
                }
            })
        });
        visual.simulate_keystrokes("tab");
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    }
}

#[gpui::test]
fn workspace_tab_respects_modifiers_and_retained_root_modal_state(cx: &mut TestAppContext) {
    let (_files, handle, editor) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for key in ["shift-tab", "cmd-tab", "alt-tab", "ctrl-tab"] {
        visual.simulate_keystrokes(key);
        editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Arrangement));
    }
    for guard in 0..8 {
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.focus.focus(window);
                match guard {
                    0 => v.close_prompt_open = true,
                    1 => v.save_dialog_open = true,
                    2 => v.export_dialog_open = true,
                    3 => v.preset_dialog_open = true,
                    4 => v.opening_project = true,
                    5 => v.theme_settings.open = true,
                    6 => v.tuning.open = true,
                    _ => v.pending_open_path = Some(std::path::PathBuf::from("unused.hex")),
                }
                cx.notify();
            })
        });
        visual.simulate_keystrokes("tab");
        editor.update(&mut visual, |v, cx| {
            assert_eq!(v.mode, EditorMode::Arrangement, "guard {guard}");
            v.close_prompt_open = false;
            v.save_dialog_open = false;
            v.export_dialog_open = false;
            v.preset_dialog_open = false;
            v.opening_project = false;
            v.theme_settings.open = false;
            v.tuning.open = false;
            v.pending_open_path = None;
            assert_eq!(v.worker_requests_pending, 0);
            cx.notify();
        });
    }
    visual.simulate_keystrokes("tab");
    editor.read_with(&visual, |v, _| assert_eq!(v.mode, EditorMode::Session));
}
