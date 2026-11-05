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
            view.opening_project || view.project_open.busy(),
            "native New action was silently lost without a focused editor element"
        )
    });
}

#[gpui::test]
fn native_open_action_survives_editor_focus_loss(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    pump(&editor, cx, settled);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(!view.project_open.busy()));
}

fn pump(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    done: impl Fn(&EditorView) -> bool,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        cx.run_until_parked();
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
        && !view.audio_import_settings.is_loading()
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
    cx.update(super::super::bind_editor_keys);
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
        assert!(view.project_open.busy());
        assert!(
            !view.opening_project,
            "one click must not confirm its own replacement"
        );
        assert_eq!(view.state.as_ref().unwrap().project, before.0.project);
        assert_eq!(view.project_generation, before.1);
    });
    editor.update(cx, |_, cx| cx.notify());
    visual.run_until_parked();
    visual.update(|window, cx| editor.read(cx).focus.focus(window));
    visual.simulate_keystrokes("cmd-n");
    visual.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("don't save");
    visual.run_until_parked();
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
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_paths_selection(Some(vec![path.clone()]));
    visual.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "clean content opens without a second confirmation"
    );
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
        editor.update(cx, |view, _| view.error = None);
        visual.dispatch_action(OpenProject);
        cx.simulate_paths_selection(Some(vec![bad]));
        visual.run_until_parked();
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
    first.read_with(cx, |view, _| assert!(!view.project_open.busy()));
    second.read_with(cx, |view, _| assert!(view.project_open.busy()));
    let unrelated = *cx.add_window(|_, _| gpui::Empty);
    cx.update_window(unrelated, |_, window, _| window.activate_window())
        .unwrap();
    cx.update(|cx| cx.dispatch_action(&NewProject));
    second_visual.run_until_parked();
    second.read_with(cx, |view, _| assert!(!view.opening_project));
    first.read_with(cx, |view, _| assert!(!view.project_open.busy()));
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
    first.read_with(cx, |view, _| assert!(!view.project_open.busy()));
    second.read_with(cx, |view, _| assert!(!view.project_open.busy()));
    first_visual.update(|window, _| window.remove_window());
    cx.update_window(second_handle, |_, window, _| {
        window.activate_window();
        window.blur();
    })
    .unwrap();
    cx.update(|cx| cx.dispatch_action(&NewProject));
    cx.run_until_parked();
    second.read_with(cx, |view, _| assert!(view.project_open.busy()));
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
        assert!(view.project_open.busy());
        assert!(
            !view.opening_project,
            "focused handler and fallback must not both run"
        );
    });
    editor.update(cx, |view, _| {
        view.opening_project = true;
        view.cancel_open_after_save();
    });
    visual.update(|window, _| window.blur());
    visual.dispatch_action(NewProject);
    editor.read_with(cx, |view, _| assert!(!view.project_open.busy()));
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
    if editor.read_with(cx, |view, _| view.project_open.busy()) {
        visual.dispatch_action(super::super::NewStreamingProject);
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.error.as_deref(),
            Some("verified streaming is unavailable")
        );
        assert_eq!(view.state.as_ref().unwrap().project, before);
    });
    editor.update(cx, |view, _| view.error = None);
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

#[gpui::test]
fn one_open_action_offers_save_discard_cancel_after_selecting_a_file(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(OpenProject);
    assert!(
        cx.did_prompt_for_paths(),
        "one Open must request the picker"
    );
    cx.simulate_paths_selection(Some(vec![directory.0.join("chosen.hex")]));
    visual.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "one Open must continue to Save / Don't Save / Cancel, not wait for another Open action"
    );
    editor.update(cx, |view, cx| {
        assert!(!view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::AddTrack {
                name: "must refuse".into()
            }
        )));
        view.save_project(cx);
        assert!(
            !view.save_dialog_open,
            "confirmation cannot stack an unrelated Save As"
        );
    });
    cx.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, before.project);
        assert!(!view.opening_project);
    });
}

fn saved_destination(
    directory: &Directory,
) -> (std::path::PathBuf, std::sync::Arc<hex_project::Project>) {
    use hex_session::service::{Event, Request};
    fn receive(service: &Service) -> Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "destination actor timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    receive(&service);
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "destination".into(),
        }))
        .unwrap();
    let project = receive(&service).state.project.clone();
    let path = directory.0.join("destination.hex");
    service.submit(Request::SaveNew(path.clone())).unwrap();
    assert!(receive(&service).save_complete);
    (path, project)
}

fn select_open(visual: &mut VisualTestContext, cx: &TestAppContext, path: std::path::PathBuf) {
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_paths_selection(Some(vec![path]));
    visual.run_until_parked();
}

fn assert_document_unchanged(
    editor: &gpui::Entity<EditorView>,
    cx: &TestAppContext,
    before: &hex_session::service::ViewState,
) {
    editor.read_with(cx, |view, _| {
        let after = view.state.as_ref().unwrap();
        assert_eq!(after.project, before.project);
        assert_eq!(after.document_path, before.document_path);
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            (after.can_undo, after.can_redo, after.unsaved),
            (before.can_undo, before.can_redo, before.unsaved)
        );
        assert!(!view.opening_project);
        assert!(!view.project_open.busy());
    });
}

#[gpui::test]
fn one_open_dont_save_replaces_once_and_clean_content_opens_directly(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (path, target) = saved_destination(&directory);
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, path.clone());
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("don't save");
    visual.run_until_parked();
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 1
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, target);
        assert_eq!(
            view.state.as_ref().unwrap().document_path.as_ref(),
            Some(&path)
        );
        assert!(!view.state.as_ref().unwrap().can_undo);
    });
    select_open(&mut visual, cx, path);
    assert!(
        !cx.has_pending_prompt(),
        "saved content is clean even when tracks exist"
    );
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 2
    });
    visual.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert!(!cx.has_pending_prompt());
}

#[gpui::test]
fn one_open_save_as_completes_original_selection_only_after_save_success(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (path, target) = saved_destination(&directory);
    let current_path = directory.0.join("preserved-current.hex");
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let current = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, path.clone());
    cx.simulate_prompt_answer("save");
    visual.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    assert!(!current_path.exists());
    editor.read_with(cx, |view, _| {
        assert_eq!(view.project_generation, generation)
    });
    visual.dispatch_action(OpenProject);
    assert!(
        !cx.did_prompt_for_paths(),
        "pending Save As cannot create a second Open"
    );
    cx.simulate_new_path_selection(|_| Some(current_path.clone()));
    visual.run_until_parked();
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 1
    });
    assert!(
        current_path.is_file(),
        "original current document must be saved before replacement"
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, target);
        assert_eq!(
            view.state.as_ref().unwrap().document_path.as_ref(),
            Some(&path)
        );
    });
    // Reopen the saved original through the same real worker path, independently
    // proving its contents rather than checking only that a file was created.
    select_open(&mut visual, cx, current_path);
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 2
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, current)
    });
}

#[gpui::test]
fn one_open_save_current_completes_original_selection_after_success(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (path, target) = saved_destination(&directory);
    let current_path = directory.0.join("named-current.hex");
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::SaveNew(current_path.clone())))
    });
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().document_path.is_some()
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::AddTrack {
                name: "saved before replacement".into(),
            }
        )))
    });
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().unsaved
    });
    let current = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, path.clone());
    cx.simulate_prompt_answer("save");
    visual.run_until_parked();
    assert!(
        !cx.did_prompt_for_new_path(),
        "named Save keeps its actual path"
    );
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 1
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, target);
        assert_eq!(
            view.state.as_ref().unwrap().document_path.as_ref(),
            Some(&path)
        );
    });
    select_open(&mut visual, cx, current_path);
    assert!(!cx.has_pending_prompt());
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == generation + 2
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, current)
    });
}

#[gpui::test]
fn one_open_cancel_save_as_clears_intent_so_later_save_does_not_open(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (path, _) = saved_destination(&directory);
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let generation = editor.read_with(cx, |view, _| view.project_generation);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, path);
    cx.simulate_prompt_answer("save");
    visual.run_until_parked();
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    assert_document_unchanged(&editor, cx, &before);
    let later = directory.0.join("later.hex");
    visual.dispatch_action(super::super::SaveProject);
    assert!(
        cx.did_prompt_for_new_path(),
        "cancel restores usable keyboard/action focus"
    );
    cx.simulate_new_path_selection(|_| Some(later.clone()));
    visual.run_until_parked();
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().document_path.as_ref() == Some(&later)
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(view.project_generation, generation);
        assert_eq!(view.state.as_ref().unwrap().project, before.project);
    });
}

#[gpui::test]
fn one_open_save_failure_keeps_document_and_does_not_resume(cx: &mut TestAppContext) {
    for named in [false, true] {
        let directory = Directory::new();
        let (path, _) = saved_destination(&directory);
        let (handle, editor) = directory.window(cx);
        add_track(&editor, cx);
        let current_path = directory.0.join("current.hex");
        if named {
            editor.update(cx, |view, _| {
                assert!(view.enqueue(hex_session::service::Request::SaveNew(current_path.clone())))
            });
            pump(&editor, cx, |view| {
                settled(view) && view.state.as_ref().unwrap().document_path.is_some()
            });
            editor.update(cx, |view, _| {
                assert!(view.enqueue(hex_session::service::Request::Edit(
                    hex_project::Command::AddTrack {
                        name: "unsaved second".into()
                    }
                )))
            });
            pump(&editor, cx, |view| {
                settled(view) && view.state.as_ref().unwrap().unsaved
            });
        }
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
        let generation = editor.read_with(cx, |view, _| view.project_generation);
        let preserved_bytes = b"external data must not be overwritten";
        std::fs::write(&current_path, preserved_bytes).unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        select_open(&mut visual, cx, path);
        cx.simulate_prompt_answer("save");
        visual.run_until_parked();
        if !named {
            cx.simulate_new_path_selection(|_| Some(current_path.clone()));
            visual.run_until_parked();
        }
        pump(&editor, cx, |view| settled(view) && view.error.is_some());
        assert_document_unchanged(&editor, cx, &before);
        assert_eq!(std::fs::read(&current_path).unwrap(), preserved_bytes);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.project_generation, generation)
        });
    }
}

#[gpui::test]
fn one_open_picker_cancel_and_failed_load_preserve_actual_imported_media(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let wav = directory.0.join("source.wav");
    let samples = [0_i16, 1000, -1000, 2000, -2000, 500, -500, 0];
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36_u32 + samples.len() as u32 * 2).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend(96_000_u32.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((samples.len() as u32 * 2).to_le_bytes());
    for sample in samples {
        bytes.extend(sample.to_le_bytes());
    }
    std::fs::write(&wav, &bytes).unwrap();
    let (handle, editor) = directory.window(cx);
    pump(&editor, cx, settled);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Import(wav.clone())))
    });
    pump(&editor, cx, |view| {
        settled(view)
            && view
                .state
                .as_ref()
                .unwrap()
                .project
                .assets()
                .next()
                .is_some()
    });
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(OpenProject);
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    assert_document_unchanged(&editor, cx, &before);
    for invalid in [
        directory.0.join("missing.hex"),
        directory.0.join("malformed.hex"),
    ] {
        if invalid.ends_with("malformed.hex") {
            std::fs::write(&invalid, b"not a hex project").unwrap();
        }
        select_open(&mut visual, cx, invalid);
        cx.simulate_prompt_answer("don't save");
        visual.run_until_parked();
        pump(&editor, cx, |view| settled(view) && view.error.is_some());
        assert_document_unchanged(&editor, cx, &before);
        assert_eq!(std::fs::read(&wav).unwrap(), bytes);
    }
}

#[gpui::test]
fn one_open_stale_picker_or_confirmation_cannot_replace_a_new_project(cx: &mut TestAppContext) {
    for picker in [true, false] {
        let directory = Directory::new();
        let (path, _) = saved_destination(&directory);
        let (handle, editor) = directory.window(cx);
        add_track(&editor, cx);
        let generation = editor.read_with(cx, |view, _| view.project_generation);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.update(|window, _| {
            window.activate_window();
            window.blur();
        });
        visual.dispatch_action(OpenProject);
        if !picker {
            cx.simulate_paths_selection(Some(vec![path.clone()]));
            visual.run_until_parked();
        }
        // An independent actor replacement is a real generation change, not a
        // manually edited UI generation counter or a fake completion event.
        editor.read_with(cx, |view, _| {
            view.service
                .submit(hex_session::service::Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap()
        });
        pump(&editor, cx, |view| {
            settled(view) && view.project_generation == generation + 1
        });
        if picker {
            cx.simulate_paths_selection(Some(vec![path]));
        } else {
            cx.simulate_prompt_answer("don't save");
        }
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.state.as_ref().unwrap().project.tracks().is_empty());
            assert!(!view.project_open.busy());
            assert_eq!(view.project_generation, generation + 1);
        });
    }
}

fn begin_tuning_preview(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    tuning: hex_tuning::ProjectTuning,
) {
    pump(editor, cx, settled);
    editor.update(cx, |view, _| {
        view.tuning.open = true;
        view.tuning
            .client
            .begin(
                view.project_generation,
                view.state.as_ref().unwrap().project.tuning(),
            )
            .unwrap();
        assert!(view.pump_tuning_gesture());
    });
    pump(editor, cx, |view| {
        settled(view) && !view.tuning.client.has_pending_requests()
    });
    editor.update(cx, |view, _| {
        view.tuning.client.change(tuning).unwrap();
        assert!(view.pump_tuning_gesture());
    });
    pump(editor, cx, |view| {
        settled(view) && !view.tuning.client.has_pending_requests()
    });
}

fn changed_tuning(reference_key: u8, hz: f64) -> hex_tuning::ProjectTuning {
    let original = hex_tuning::ProjectTuning::default();
    let mut scale = original.scale();
    scale.set_reference(reference_key, hz, 0).unwrap();
    hex_tuning::ProjectTuning::new(true, scale, original.audition()).unwrap()
}

#[gpui::test]
fn returned_open_failed_load_preserves_acknowledged_tuning_draft_and_history(
    cx: &mut TestAppContext,
) {
    for malformed in [false, true] {
        let directory = Directory::new();
        let (handle, editor) = directory.window(cx);
        add_track(&editor, cx);
        let draft = changed_tuning(60, 333.0);
        begin_tuning_preview(&editor, cx, draft);
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
        let snapshot = editor.read_with(cx, |view, _| view.tuning.client.snapshot().unwrap());
        let path = directory.0.join(if malformed {
            "malformed.hex"
        } else {
            "missing.hex"
        });
        if malformed {
            std::fs::write(&path, b"not a current project").unwrap();
        }
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        select_open(&mut visual, cx, path);
        cx.simulate_prompt_answer("don't save");
        visual.run_until_parked();
        pump(&editor, cx, |view| settled(view) && view.error.is_some());
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.tuning.client.snapshot(),
                Some(snapshot),
                "failed load must not retire or commit the user's tuning draft"
            );
            assert_eq!(view.tuning.client.snapshot().unwrap().tuning, draft);
        });
        assert_document_unchanged(&editor, cx, &before);
    }
}

#[gpui::test]
fn returned_open_refused_deferred_tuning_save_releases_intent_and_future_actions(
    cx: &mut TestAppContext,
) {
    use hex_project::{BeatTime, Command};
    use hex_session::service::Request;
    let directory = Directory::new();
    let (destination, _) = saved_destination(&directory);
    let (handle, editor) = directory.window(cx);
    pump(&editor, cx, settled);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::InsertTrackWithDevice {
            name: "notes".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })))
    });
    pump(&editor, cx, |view| {
        settled(view) && !view.state.as_ref().unwrap().project.tracks().is_empty()
    });
    let track = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE
        })))
    });
    pump(&editor, cx, settled);
    let clip = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .midi_clips()[0]
            .id()
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100
        })))
    });
    pump(&editor, cx, settled);
    let current_path = directory.0.join("current.hex");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::SaveNew(current_path.clone())))
    });
    pump(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().document_path.is_some()
    });
    let before_bytes = std::fs::read(&current_path).unwrap();
    begin_tuning_preview(&editor, cx, changed_tuning(0, 20_000.0));
    editor.read_with(cx, |view, _| {
        assert!(
            view.tuning.error.is_some(),
            "actual worker must reject the unplayable preview"
        )
    });
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, destination);
    cx.simulate_prompt_answer("save");
    visual.run_until_parked();
    pump(&editor, cx, |view| {
        settled(view) && !view.saving_project && view.tuning.error.is_some()
    });
    assert_document_unchanged(&editor, cx, &before);
    assert_eq!(std::fs::read(&current_path).unwrap(), before_bytes);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "usable after refusal".into()
        })))
    });
    pump(&editor, cx, settled);
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(!view.project_open.busy()));
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx));
            assert!(
                view.close_prompt_open,
                "close must reach its normal unsaved-work prompt"
            );
        });
    });
    cx.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    visual.dispatch_action(NewProject);
    editor.read_with(cx, |view, _| {
        assert!(
            view.project_open.busy(),
            "New remains reachable after refused Save"
        );
        assert!(!view.opening_project);
    });
}

#[gpui::test]
fn returned_open_picker_admits_own_recovery_restore_without_admitting_edits(
    cx: &mut TestAppContext,
) {
    use hex_session::service::Request;
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    editor.update(cx, |view, _| view.recover_audio());
    let generation = editor.read_with(cx, |view, _| view.pending_recovery_generation.unwrap());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(OpenProject);
    assert!(cx.did_prompt_for_paths());
    editor.update(cx, |view, _| {
        assert!(view.project_open.busy());
        assert!(!view.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "must not be inserted".into()
        })));
        assert!(
            !view.enqueue(Request::RestorePlayback {
                generation: generation + 1
            }),
            "foreign recovery tokens remain blocked"
        );
        assert!(
            view.enqueue(Request::RestorePlayback { generation }),
            "picker must not reject its current internal audio-recovery continuation"
        );
    });
    // Receive the real restart and restoration completions explicitly. This
    // older source's separate restart-counter finding belongs to Source6;
    // a zero pending count alone must not masquerade as restoration proof.
    let deadline = crate::test_wait::hang_deadline();
    let mut restored = false;
    while !restored {
        if let Ok(event) = editor.read_with(cx, |view, _| view.service.try_event()) {
            if event.restore_complete == Some(generation) {
                assert!(event.error.is_none());
                assert!(event.playback.is_some());
                restored = true;
            }
            editor.update(cx, |view, _| {
                view.receive(event);
            });
        } else {
            assert!(
                std::time::Instant::now() < deadline,
                "actual restoration completion missing"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    pump(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert!(!view.project_open.busy());
        assert_eq!(view.state.as_ref().unwrap().project.tracks().len(), 1);
    });
}

#[gpui::test]
fn selected_open_path_survives_owned_restore_before_render_and_opens_once(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    use std::{cell::Cell, rc::Rc};
    let directory = Directory::new();
    let (path, target) = saved_destination(&directory);
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let old_generation = editor.read_with(cx, |view, _| view.project_generation);
    editor.update(cx, |view, _| view.recover_audio());
    let recovery = editor.read_with(cx, |view, _| view.pending_recovery_generation.unwrap());
    let injected = Rc::new(Cell::new(false));
    let seen = Rc::clone(&injected);
    let chosen = path.clone();
    // Notification runs after the real picker continuation chose the path and
    // before the next render reconciles it. Inject the actual internal request
    // at that production cutpoint, without a fake picker or direct Open call.
    let _subscription = cx.update(|cx| {
        cx.observe(&editor, move |editor, cx| {
            editor.update(cx, |view, _| {
                if view.pending_open_path.as_ref() != Some(&chosen) || seen.replace(true) {
                    return;
                }
                assert!(!view.enqueue(Request::Edit(hex_project::Command::AddTrack {
                    name: "must not be inserted".into(),
                })));
                assert!(!view.enqueue(Request::RestorePlayback {
                    generation: recovery + 1,
                }));
                assert!(view.enqueue(Request::RestorePlayback {
                    generation: recovery
                }));
                assert_eq!(
                    view.pending_open_path.as_ref(),
                    Some(&chosen),
                    "owned recovery must not consume the picker-selected Open intent"
                );
                assert_eq!(
                    view.pending_open_mode,
                    Some(hex_session::DocumentAudioMode::Resident)
                );
            });
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    select_open(&mut visual, cx, path.clone());
    assert!(
        injected.get(),
        "real picker callback must reach the tested ordering"
    );
    pump(&editor, cx, |view| {
        view.worker_requests_pending == 0
            && view.pending_requests.is_empty()
            && view.project_open.busy()
    });
    visual.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "the first Open continues after recovery"
    );
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, before.project);
        assert_eq!(state.revision, before.revision);
        assert_eq!(
            (state.can_undo, state.can_redo, state.unsaved),
            (before.can_undo, before.can_redo, before.unsaved)
        );
    });
    cx.simulate_prompt_answer("don't save");
    visual.run_until_parked();
    pump(&editor, cx, |view| {
        settled(view) && view.project_generation == old_generation + 1
    });
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, target);
        assert_eq!(state.document_path.as_ref(), Some(&path));
        assert_eq!(view.project_generation, old_generation + 1);
        assert!(!state.can_undo && !state.unsaved);
        assert!(view.pending_open_path.is_none() && !view.project_open.busy());
    });
}

mod recovery_name_tests;

#[gpui::test]
fn one_new_action_prompts_and_cancel_preserves_the_complete_current_document(
    cx: &mut TestAppContext,
) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(NewProject);
    visual.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "one New must present save / don't save / cancel"
    );
    cx.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    assert_document_unchanged(&editor, cx, &before);
}

fn select_new(visual: &mut VisualTestContext, cx: &TestAppContext) {
    visual.update(|window, _| {
        window.activate_window();
        window.blur();
    });
    visual.dispatch_action(NewProject);
    visual.run_until_parked();
    assert!(cx.has_pending_prompt());
}

#[gpui::test]
fn one_new_save_waits_for_current_or_new_save_and_replaces_once(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    for named in [false, true] {
        let directory = Directory::new();
        let (handle, editor) = directory.window(cx);
        add_track(&editor, cx);
        let path = directory.0.join("preserved.hex");
        if named {
            editor.update(cx, |view, _| {
                assert!(view.enqueue(Request::SaveNew(path.clone())))
            });
            pump(&editor, cx, |view| {
                settled(view) && view.state.as_ref().unwrap().document_path.is_some()
            });
            add_track(&editor, cx);
        }
        let (current, generation, saved_revision) = editor.read_with(cx, |view, _| {
            (
                view.state.as_ref().unwrap().project.clone(),
                view.project_generation,
                view.state.as_ref().unwrap().revision,
            )
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        select_new(&mut visual, cx);
        cx.simulate_prompt_answer("save");
        visual.run_until_parked();
        assert_eq!(cx.did_prompt_for_new_path(), !named);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.project_generation, generation)
        });
        // Further menu actions cannot replace this captured destination.
        visual.dispatch_action(NewProject);
        visual.dispatch_action(OpenProject);
        visual.run_until_parked();
        assert!(!cx.did_prompt_for_paths());
        if !named {
            cx.simulate_new_path_selection(|_| Some(path.clone()));
            visual.run_until_parked();
        }
        pump(&editor, cx, |view| {
            settled(view) && view.project_generation == generation + 1
        });
        editor.read_with(cx, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert!(state.project.tracks().is_empty() && !state.can_undo && !state.unsaved);
            assert!(state.document_path.is_none() && !view.project_open.busy());
            assert_eq!(
                view.document_audio_mode,
                hex_session::DocumentAudioMode::Resident
            );
        });
        // A duplicate/late save terminal cannot run the captured New again.
        editor.update(cx, |view, cx| {
            view.complete_open_save(true, saved_revision, cx);
            view.complete_open_save(false, saved_revision, cx);
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(view.project_generation, generation + 1);
        });
        select_open(&mut visual, cx, path);
        assert!(!cx.has_pending_prompt());
        pump(&editor, cx, |view| {
            settled(view) && view.project_generation == generation + 2
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, current)
        });
        // A clean saved document with content needs no discard prompt for New.
        visual.dispatch_action(NewProject);
        visual.run_until_parked();
        assert!(!cx.has_pending_prompt());
        pump(&editor, cx, |view| {
            settled(view) && view.project_generation == generation + 3
        });
        assert!(editor.read_with(cx, |view, _| {
            view.state.as_ref().unwrap().project.tracks().is_empty()
        }));
    }
}

#[gpui::test]
fn one_new_cancelled_or_failed_save_aborts_continuation_before_a_later_save(
    cx: &mut TestAppContext,
) {
    for failure in [false, true] {
        let directory = Directory::new();
        let (handle, editor) = directory.window(cx);
        add_track(&editor, cx);
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
        let generation = editor.read_with(cx, |view, _| view.project_generation);
        let collision = directory.0.join("external.hex");
        std::fs::write(&collision, b"preserve external bytes").unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        select_new(&mut visual, cx);
        cx.simulate_prompt_answer("save");
        visual.run_until_parked();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| failure.then_some(collision.clone()));
        visual.run_until_parked();
        pump(&editor, cx, |view| {
            settled(view) && !view.save_dialog_open && !view.saving_project
        });
        assert_document_unchanged(&editor, cx, &before);
        assert_eq!(
            std::fs::read(&collision).unwrap(),
            b"preserve external bytes"
        );
        let saved = directory.0.join("later.hex");
        editor.update(cx, |view, cx| view.save_project(cx));
        visual.run_until_parked();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(saved.clone()));
        visual.run_until_parked();
        pump(&editor, cx, |view| {
            settled(view)
                && !view.saving_project
                && view.state.as_ref().unwrap().document_path.is_some()
        });
        editor.read_with(cx, |view, _| {
            assert_eq!(view.project_generation, generation);
            assert_eq!(view.state.as_ref().unwrap().project, before.project);
            assert!(!view.project_open.busy());
        });
        assert!(saved.is_file());
    }
}

#[gpui::test]
fn one_new_waits_for_admitted_edits_and_stale_answers_cannot_replace_again(
    cx: &mut TestAppContext,
) {
    use hex_session::service::Request;
    for answer in ["save", "don't save"] {
        let directory = Directory::new();
        let (handle, editor) = directory.window(cx);
        pump(&editor, cx, settled);
        let generation = editor.read_with(cx, |view, _| view.project_generation);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(cx, |view, _| {
            assert!(view.enqueue(Request::Edit(hex_project::Command::AddTrack {
                name: "pending before new".into()
            })))
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
                view.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
                assert!(view.project_open.busy() && !view.opening_project);
            })
        });
        pump(&editor, cx, settled);
        visual.run_until_parked();
        assert!(
            cx.has_pending_prompt(),
            "dirty acknowledgement must precede the prompt"
        );
        assert_eq!(
            editor.read_with(cx, |view, _| view
                .state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .len()),
            1
        );
        // An independent actor really replaces the source while the modal is pending.
        editor.read_with(cx, |view, _| {
            view.service
                .submit(Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                })
                .unwrap()
        });
        pump(&editor, cx, |view| {
            settled(view) && view.project_generation == generation + 1
        });
        cx.simulate_prompt_answer(answer);
        visual.run_until_parked();
        pump(&editor, cx, settled);
        assert!(!cx.did_prompt_for_new_path());
        editor.read_with(cx, |view, _| {
            assert_eq!(view.project_generation, generation + 1);
            assert!(view.state.as_ref().unwrap().project.tracks().is_empty());
            assert!(!view.project_open.busy());
        });
    }
}

#[gpui::test]
fn one_new_aborts_when_an_admitted_edit_is_refused_before_confirmation(cx: &mut TestAppContext) {
    let directory = Directory::new();
    let (handle, editor) = directory.window(cx);
    add_track(&editor, cx);
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::AddTrack {
                name: String::new()
            }
        )))
    });
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.new_project(hex_session::DocumentAudioMode::Resident, window, cx)
        })
    });
    pump(&editor, cx, |view| settled(view) && view.error.is_some());
    visual.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "failed preparation must abort New instead of offering to discard work"
    );
    assert_document_unchanged(&editor, cx, &before);
}
