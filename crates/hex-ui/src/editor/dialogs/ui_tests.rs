use super::super::{EditorView, NewProject, OpenProject};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use hex_session::service::Service;

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-project-actions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("samples")).unwrap();
        Self(path)
    }
    fn window(&self, cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
        cx.update(super::install_project_actions);
        let service = Service::new(48_000, 8, 8_000_000).unwrap();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_browser_directory(window, cx, service, &self.0)
                })
            })
            .unwrap();
        (handle, editor)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[gpui::test]
fn native_new_action_survives_editor_focus_loss(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(NewProject);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.opening_project || view.pending_new_mode.is_some(),
            "native New action was silently lost without a focused editor element"
        )
    });
}

#[gpui::test]
fn native_open_confirmation_action_survives_editor_focus_loss(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Existing second-click confirmation path avoids invoking the platform picker
    // (unimplemented by GPUI's headless test platform), but uses real menu dispatch.
    editor.update(cx, |view, _| {
        view.pending_open_path = Some(directory.0.join("missing.hex"));
        view.pending_open_mode = Some(hex_session::DocumentAudioMode::Resident);
    });
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(OpenProject);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.pending_open_path.is_none(),
            "native Open action was silently lost without a focused editor element"
        )
    });
}

fn pump(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    done: impl Fn(&EditorView) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        if editor.read_with(cx, |view, _| done(view)) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "project action did not complete: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn settled(view: &EditorView) -> bool {
    view.state.is_some()
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && !view.opening_project
}
fn add_track(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    pump(editor, cx, settled);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::AddTrack {
                name: "keep this".into()
            }
        )))
    });
    pump(editor, cx, |view| {
        settled(view) && !view.state.as_ref().unwrap().project.tracks().is_empty()
    });
}

#[gpui::test]
fn project_actions_preserve_confirmation_and_dispatch_once_after_rerender(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    // Installing for another editor/window must not duplicate the fallback.
    cx.update(super::install_project_actions);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| {
        (
            view.state.as_ref().unwrap().clone(),
            view.project_generation,
        )
    });
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(NewProject);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.pending_new_mode,
            Some(hex_session::DocumentAudioMode::Resident)
        );
        assert!(
            !view.opening_project,
            "one click must not confirm its own replacement"
        );
        assert_eq!(view.state.as_ref().unwrap().project, before.0.project);
        assert_eq!(view.project_generation, before.1);
    });
    editor.update(cx, |_, cx| cx.notify());
    visual.run_until_parked();
    visual.dispatch_action(NewProject);
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation > before.1
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.project_generation, before.1 + 1);
        assert!(view.state.as_ref().unwrap().project.tracks().is_empty());
        assert!(!view.state.as_ref().unwrap().can_undo);
        assert_eq!(
            view.document_audio_mode,
            hex_session::DocumentAudioMode::Resident
        );
    });
}

#[gpui::test]
fn project_open_confirmation_loads_saved_state_and_failure_preserves_it(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let path = directory.0.join("saved.hex");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::SaveNew(path.clone())))
    });
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().document_path.as_ref() == Some(&path)
    });
    let saved = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(NewProject);
    visual.dispatch_action(NewProject);
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().project.tracks().is_empty()
    });
    // Native picker itself is a separately required platform check. Select its
    // already-supported confirmation path here; do not fake an OS picker pass.
    editor.update(cx, |view, _| {
        view.pending_open_path = Some(path.clone());
        view.pending_open_mode = Some(hex_session::DocumentAudioMode::Resident);
    });
    visual.dispatch_action(OpenProject);
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().project == saved
    });
    let before = editor.read_with(cx, |view, _| {
        (
            view.state.as_ref().unwrap().clone(),
            view.project_generation,
        )
    });
    for bad in [
        directory.0.join("missing.hex"),
        directory.0.join("malformed.hex"),
    ] {
        if bad.ends_with("malformed.hex") {
            std::fs::write(&bad, b"not a current hex project").unwrap();
        }
        editor.update(cx, |view, _| {
            view.pending_open_path = Some(bad);
            view.pending_open_mode = Some(hex_session::DocumentAudioMode::Resident);
            view.error = None;
        });
        visual.dispatch_action(OpenProject);
        pump(&editor, cx, |view| settled(view) && view.error.is_some());
        editor.read_with(cx, |view, _| {
            let after = view.state.as_ref().unwrap();
            assert_eq!(after.project, before.0.project);
            assert_eq!(after.document_path, before.0.document_path);
            assert_eq!(
                (after.can_undo, after.can_redo),
                (before.0.can_undo, before.0.can_redo)
            );
            assert_eq!(view.project_generation, before.1);
            assert!(!view.error.as_ref().unwrap().is_empty());
        });
    }
}

#[gpui::test]
fn project_fallback_targets_only_active_editor_not_another_window(cx: &mut TestAppContext) {
    let first_directory = Directory::new();
    let second_directory = Directory::new();
    let (first_handle, first) = first_directory.window(cx);
    let (second_handle, second) = second_directory.window(cx);
    add_track(&first, cx);
    add_track(&second, cx);
    let first_visual = VisualTestContext::from_window(first_handle, cx);
    let mut second_visual = VisualTestContext::from_window(second_handle, cx);
    first_visual.run_until_parked();
    second_visual.run_until_parked();
    second_visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    cx.update(|cx| cx.dispatch_action(&NewProject));
    second_visual.run_until_parked();
    first.read_with(cx, |view, _| assert!(view.pending_new_mode.is_none()));
    second.read_with(cx, |view, _| {
        assert_eq!(
            view.pending_new_mode,
            Some(hex_session::DocumentAudioMode::Resident)
        )
    });
    let unrelated = *cx.add_window(|_, _| gpui::Empty);
    cx.update_window(unrelated, |_, window, _| window.activate_window())
        .unwrap();
    cx.update(|cx| cx.dispatch_action(&NewProject));
    second_visual.run_until_parked();
    second.read_with(cx, |view, _| assert!(!view.opening_project));
    first.read_with(cx, |view, _| assert!(view.pending_new_mode.is_none()));
}

#[gpui::test]
fn project_menu_without_active_or_after_closed_window_cannot_touch_another_editor(
    cx: &mut TestAppContext,
) {
    let first_directory = Directory::new();
    let second_directory = Directory::new();
    let (first_handle, first) = first_directory.window(cx);
    let (second_handle, second) = second_directory.window(cx);
    add_track(&first, cx);
    add_track(&second, cx);
    let mut first_visual = VisualTestContext::from_window(first_handle, cx);
    first_visual.update(|window, _| window.activate_window());
    first_visual.deactivate_window();
    cx.update(|cx| cx.dispatch_action(&NewProject));
    first_visual.run_until_parked();
    first.read_with(cx, |view, _| assert!(view.pending_new_mode.is_none()));
    second.read_with(cx, |view, _| assert!(view.pending_new_mode.is_none()));
    first_visual.update(|window, _| window.remove_window());
    cx.update_window(second_handle, |_, window, _| {
        window.activate_window();
        window.blur();
    })
    .unwrap();
    cx.update(|cx| cx.dispatch_action(&NewProject));
    cx.run_until_parked();
    second.read_with(cx, |view, _| {
        assert_eq!(
            view.pending_new_mode,
            Some(hex_session::DocumentAudioMode::Resident)
        )
    });
}

#[gpui::test]
fn focused_file_action_keeps_one_confirmation_and_pending_project_action_refuses(
    cx: &mut TestAppContext,
) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.activate_window();
        editor.read(cx).focus.focus(window);
    });
    visual.dispatch_action(NewProject);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.pending_new_mode,
            Some(hex_session::DocumentAudioMode::Resident)
        );
        assert!(
            !view.opening_project,
            "focused handler and fallback must not both run"
        );
    });
    editor.update(cx, |view, _| {
        view.opening_project = true;
        view.pending_new_mode = None;
    });
    visual.update(|window, _| window.blur());
    visual.dispatch_action(NewProject);
    editor.read_with(cx, |view, _| assert!(view.pending_new_mode.is_none()));
}

#[gpui::test]
fn unfocused_streaming_file_actions_preserve_explicit_mode_refusal(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    pump(&editor, cx, settled);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    visual.dispatch_action(super::super::NewStreamingProject);
    // If content requires confirmation, the second invocation confirms only this mode.
    if editor.read_with(cx, |view, _| view.pending_new_mode.is_some()) {
        visual.dispatch_action(super::super::NewStreamingProject);
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.error.as_deref(),
            Some("verified streaming is unavailable")
        );
        assert_eq!(view.state.as_ref().unwrap().project, before);
    });
    editor.update(cx, |view, _| {
        view.error = None;
        view.pending_open_path = Some(directory.0.join("saved.hex"));
        view.pending_open_mode = Some(hex_session::DocumentAudioMode::VerifiedStreaming);
    });
    visual.dispatch_action(super::super::OpenStreamingProject);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.error.as_deref(),
            Some("verified streaming is unavailable")
        );
        assert_eq!(view.state.as_ref().unwrap().project, before);
        assert_eq!(
            view.document_audio_mode,
            hex_session::DocumentAudioMode::Resident
        );
        assert!(!view.opening_project);
    });
}
