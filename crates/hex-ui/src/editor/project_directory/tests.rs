use super::*;
use crate::editor::{OpenProject, SaveAsProject, SaveProject};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use hex_session::service::{Request, Service};
use std::sync::atomic::{AtomicU64, Ordering};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-project-folder-ui-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("samples")).unwrap();
        Self(path)
    }
    fn preferences(&self) -> PathBuf {
        self.0.join("project-directory.json")
    }
    fn folder(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir(&path).unwrap();
        path
    }
    fn window(&self, cx: &mut TestAppContext) -> (VisualTestContext, gpui::Entity<EditorView>) {
        cx.update(crate::editor::dialogs::install_project_actions);
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let service = Service::new(48_000, 16, 8_000_000).unwrap();
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_project_preferences(
                        window,
                        cx,
                        service,
                        &self.0,
                        self.preferences(),
                    )
                })
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        pump(&editor, &mut visual, |view| {
            settled(view) && !view.project_directory.loading
        });
        visual.update(|window, _| window.activate_window());
        editor.update_in(&mut visual, |view, window, _| view.focus.focus(window));
        visual.run_until_parked();
        (visual, editor)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn settled(view: &EditorView) -> bool {
    view.state.is_some()
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && !view.opening_project
}
fn pump(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    done: impl Fn(&EditorView) -> bool,
) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        cx.run_until_parked();
        if editor.read_with(cx, |view, _| done(view)) {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "project-folder operation stalled: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn flush(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    pump(editor, cx, |view| {
        settled(view) && !view.project_directory.loading && {
            let writes = view.project_directory.writes.lock().unwrap();
            !writes.running && writes.pending.is_none()
        }
    });
}
fn open(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>) {
    editor.update_in(visual, |view, window, _| view.focus.focus(window));
    visual.dispatch_action(OpenProject);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        visual.run_until_parked();
        if visual.pending_open_directory().is_some() {
            break;
        }
        editor.update(visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        assert!(
            std::time::Instant::now() < until,
            "Open did not reach the actual platform request"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[gpui::test]
fn project_directory_saved_folder_reaches_actual_open_request(cx: &mut TestAppContext) {
    let owned = Directory::new();
    let folder = owned.folder("My Music Case");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let (mut visual, editor) = owned.window(cx);
    open(&mut visual, &editor);
    visual.run_until_parked();
    assert_eq!(cx.pending_open_directory(), Some(Some(folder)));
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    flush(&editor, &mut visual);
}

fn save_as(visual: &mut VisualTestContext, editor: &gpui::Entity<EditorView>, ordinary: bool) {
    editor.update_in(visual, |view, window, _| view.focus.focus(window));
    if ordinary {
        visual.dispatch_action(SaveProject);
    } else {
        visual.dispatch_action(SaveAsProject);
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        visual.run_until_parked();
        if visual.pending_save_options().is_some() {
            return;
        }
        editor.update(visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        assert!(
            std::time::Instant::now() < until,
            "Save did not reach its actual platform request"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn saved(
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    path: &std::path::Path,
) {
    pump(editor, visual, |view| {
        settled(view)
            && !view.saving_project
            && view.state.as_ref().unwrap().document_path.as_deref() == Some(path)
    });
}
fn service_state(service: &Service) -> Arc<hex_session::service::ViewState> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event.state;
        }
        assert!(std::time::Instant::now() < until, "fixture service stalled");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn saved_target(owned: &Directory, folder: &str) -> PathBuf {
    let directory = owned.folder(folder);
    let path = directory.join("Target Case.hex");
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    service_state(&service);
    service.submit(Request::SaveNew(path.clone())).unwrap();
    service_state(&service);
    path
}
fn close(mut visual: VisualTestContext, editor: gpui::Entity<EditorView>) {
    visual.update(|window, _| window.remove_window());
    drop(editor);
}

#[gpui::test]
fn project_directory_successful_save_open_and_restart_share_one_native_folder(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    let save_folder = owned.folder("Saved Takes");
    let target = saved_target(&owned, "Opened Takes");
    let (mut visual, editor) = owned.window(cx);
    save_as(&mut visual, &editor, true);
    assert_eq!(
        cx.pending_save_options(),
        Some((PathBuf::from("."), Some("untitled.hex".into())))
    );
    let first = save_folder.join("First Take.hex");
    cx.simulate_new_path_selection(|_| Some(save_folder.join("First Take")));
    saved(&mut visual, &editor, &first);
    flush(&editor, &mut visual);
    assert_eq!(
        hex_preferences::load_project_directory(&owned.preferences()).unwrap(),
        Some(save_folder.clone())
    );
    open(&mut visual, &editor);
    assert_eq!(cx.pending_open_directory(), Some(Some(save_folder)));
    cx.simulate_paths_selection(Some(vec![target.clone()]));
    pump(&editor, &mut visual, |view| {
        settled(view)
            && view.state.as_ref().unwrap().document_path.as_deref() == Some(target.as_path())
    });
    flush(&editor, &mut visual);
    let expected = target.parent().unwrap().to_owned();
    assert_eq!(
        hex_preferences::load_project_directory(&owned.preferences()).unwrap(),
        Some(expected.clone())
    );
    close(visual, editor);
    let (mut visual, editor) = owned.window(cx);
    open(&mut visual, &editor);
    assert_eq!(cx.pending_open_directory(), Some(Some(expected.clone())));
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    save_as(&mut visual, &editor, true);
    assert_eq!(
        cx.pending_save_options(),
        Some((expected, Some("untitled.hex".into())))
    );
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    flush(&editor, &mut visual);
}

#[gpui::test]
fn project_directory_cancel_platform_error_and_invalid_load_preserve_document_and_preference(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    let folder = owned.folder("Keep Case");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let original = std::fs::read(owned.preferences()).unwrap();
    let (mut visual, editor) = owned.window(cx);
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    open(&mut visual, &editor);
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    open(&mut visual, &editor);
    cx.simulate_paths_error("injected open platform refusal");
    visual.run_until_parked();
    let bad = owned.0.join("Broken.hex");
    std::fs::write(&bad, b"invalid current project").unwrap();
    open(&mut visual, &editor);
    cx.simulate_paths_selection(Some(vec![bad]));
    pump(&editor, &mut visual, |view| {
        settled(view) && view.error.is_some()
    });
    flush(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, before.project);
        assert_eq!(state.document_path, before.document_path);
        assert_eq!(
            (state.can_undo, state.can_redo),
            (before.can_undo, before.can_redo)
        );
        assert!(!view.project_open.busy());
    });
    assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
}

#[gpui::test]
fn project_directory_cancel_platform_error_and_save_failure_keep_original_files(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    let folder = owned.folder("Keep Saved Folder");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let original = std::fs::read(owned.preferences()).unwrap();
    let existing = owned.0.join("Existing.hex");
    std::fs::write(&existing, b"original user file bytes").unwrap();
    let (mut visual, editor) = owned.window(cx);
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    save_as(&mut visual, &editor, false);
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    save_as(&mut visual, &editor, false);
    cx.simulate_new_path_error("injected save platform refusal");
    visual.run_until_parked();
    save_as(&mut visual, &editor, false);
    cx.simulate_new_path_selection(|_| Some(existing.clone()));
    pump(&editor, &mut visual, |view| {
        settled(view) && !view.saving_project && view.error.is_some()
    });
    flush(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, before.project);
        assert_eq!(state.document_path, before.document_path);
        assert!(!view.save_dialog_open);
    });
    assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
    assert_eq!(
        std::fs::read(existing).unwrap(),
        b"original user file bytes"
    );
}

#[gpui::test]
fn project_directory_dirty_open_cancel_save_as_cancel_and_dont_save_failure_keep_folder(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    let folder = owned.folder("Unchanged");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let original = std::fs::read(owned.preferences()).unwrap();
    let target = saved_target(&owned, "Other Project");
    let (mut visual, editor) = owned.window(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "keep current".into()
        })))
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.state.as_ref().unwrap().unsaved
    });
    let before = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().clone());
    open(&mut visual, &editor);
    cx.simulate_paths_selection(Some(vec![target.clone()]));
    visual.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    open(&mut visual, &editor);
    cx.simulate_paths_selection(Some(vec![target]));
    visual.run_until_parked();
    cx.simulate_prompt_answer("save");
    pump(&editor, &mut visual, |view| view.save_dialog_open);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while cx.pending_save_options().is_none() {
        visual.run_until_parked();
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(cx.pending_save_options().unwrap().0, folder);
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    let bad = owned.0.join("Malformed.hex");
    std::fs::write(&bad, b"broken").unwrap();
    open(&mut visual, &editor);
    cx.simulate_paths_selection(Some(vec![bad]));
    visual.run_until_parked();
    cx.simulate_prompt_answer("don't save");
    pump(&editor, &mut visual, |view| {
        settled(view) && view.error.is_some()
    });
    flush(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project, before.project);
        assert_eq!(state.unsaved, before.unsaved);
        assert_eq!(state.can_undo, before.can_undo);
        assert!(!view.project_open.busy());
    });
    assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
}

#[gpui::test]
fn project_directory_unset_missing_and_non_directory_fall_back_without_creating_paths(
    cx: &mut TestAppContext,
) {
    for kind in [0, 1, 2] {
        let owned = Directory::new();
        let unavailable = owned.0.join("Not A Directory");
        if kind != 0 {
            hex_preferences::save_project_directory(&owned.preferences(), &unavailable).unwrap();
        }
        if kind == 2 {
            std::fs::write(&unavailable, b"regular file").unwrap();
        }
        let original = std::fs::read(owned.preferences()).ok();
        let (mut visual, editor) = owned.window(cx);
        open(&mut visual, &editor);
        assert_eq!(cx.pending_open_directory(), Some(None));
        cx.simulate_paths_selection(None);
        visual.run_until_parked();
        save_as(&mut visual, &editor, false);
        assert_eq!(cx.pending_save_options().unwrap().0, PathBuf::from("."));
        cx.simulate_new_path_selection(|_| None);
        visual.run_until_parked();
        flush(&editor, &mut visual);
        assert_eq!(std::fs::read(owned.preferences()).ok(), original);
        assert_eq!(unavailable.is_dir(), false);
        if kind == 2 {
            assert_eq!(std::fs::read(&unavailable).unwrap(), b"regular file");
        }
        close(visual, editor);
    }
}

#[gpui::test]
fn project_directory_invalid_preferences_still_open_default_and_preserve_bytes(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    std::fs::write(owned.preferences(), b"foreign malformed settings").unwrap();
    let (mut visual, editor) = owned.window(cx);
    open(&mut visual, &editor);
    assert_eq!(cx.pending_open_directory(), Some(None));
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    let destination = owned.folder("Actual Successful Save").join("Take.hex");
    save_as(&mut visual, &editor, true);
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    saved(&mut visual, &editor, &destination);
    flush(&editor, &mut visual);
    assert_eq!(
        std::fs::read(owned.preferences()).unwrap(),
        b"foreign malformed settings"
    );
    editor.read_with(&visual, |view, _| {
        assert!(
            view.error
                .as_deref()
                .is_some_and(|s| s.contains("original invalid preferences preserved"))
        )
    });
}

#[cfg(unix)]
#[gpui::test]
fn project_directory_native_non_utf8_folder_is_not_lowercased_or_lossily_converted(
    cx: &mut TestAppContext,
) {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let owned = Directory::new();
    let folder = owned
        .0
        .join(std::ffi::OsString::from_vec(b"RawCase-\xff".to_vec()));
    std::fs::create_dir(&folder).unwrap();
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let (mut visual, editor) = owned.window(cx);
    open(&mut visual, &editor);
    assert_eq!(
        cx.pending_open_directory()
            .unwrap()
            .unwrap()
            .as_os_str()
            .as_bytes(),
        folder.as_os_str().as_bytes()
    );
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    save_as(&mut visual, &editor, false);
    assert_eq!(
        cx.pending_save_options().unwrap().0.as_os_str().as_bytes(),
        folder.as_os_str().as_bytes()
    );
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    flush(&editor, &mut visual);
}

#[gpui::test]
fn project_directory_owned_writer_persists_latest_success_after_view_drop(cx: &mut TestAppContext) {
    let owned = Directory::new();
    let first = owned.folder("First").join("One.hex");
    let latest = owned.folder("Latest").join("Two.hex");
    let (mut visual, editor) = owned.window(cx);
    let gate = Arc::new(SaveGate::default());
    editor.update(&mut visual, |view, _| {
        view.project_directory.save_gate = Some(gate.clone())
    });
    save_as(&mut visual, &editor, true);
    cx.simulate_new_path_selection(|_| Some(first.clone()));
    saved(&mut visual, &editor, &first);
    pump(&editor, &mut visual, |_| {
        gate.reached.load(Ordering::Acquire)
    });
    save_as(&mut visual, &editor, false);
    cx.simulate_new_path_selection(|_| Some(latest.clone()));
    saved(&mut visual, &editor, &latest);
    let writes = editor.read_with(&visual, |view, _| {
        Arc::clone(&view.project_directory.writes)
    });
    close(visual, editor);
    gate.release();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if !writes.lock().unwrap().running {
            break;
        }
        assert!(std::time::Instant::now() < until, "owned writer stalled");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        hex_preferences::load_project_directory(&owned.preferences()).unwrap(),
        Some(latest.parent().unwrap().to_owned())
    );
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn project_directory_macos_url_roundtrip_preserves_raw_bytes_and_refuses_nul(
    cx: &mut TestAppContext,
) {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    for bytes in [
        b"/tmp/Project Case".as_slice(),
        b"/tmp/Native-\xff".as_slice(),
    ] {
        let p = PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec()));
        assert_eq!(
            cx.picker_directory_url_roundtrip(&p)
                .unwrap()
                .as_os_str()
                .as_bytes(),
            bytes
        );
    }
    let nul = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/bad\0path".to_vec()));
    assert!(cx.picker_directory_url_roundtrip(&nul).is_err());
}

#[gpui::test]
fn project_directory_named_save_uses_actual_document_path_without_a_chooser(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    let current = owned.folder("Actual Document").join("Named Take.hex");
    let remembered = owned.folder("Different Remembered Folder");
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    service_state(&service);
    service.submit(Request::SaveNew(current.clone())).unwrap();
    let initial = service_state(&service);
    let original = std::fs::read(&current).unwrap();
    hex_preferences::save_project_directory(&owned.preferences(), &remembered).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_project_preferences(
                    window,
                    cx,
                    service,
                    &owned.0,
                    owned.preferences(),
                )
            });
            // The initial ViewState is the actual already-loaded actor's saved receipt.
            editor.update(cx, |view, cx| {
                view.state = Some(initial);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual, |view| {
        settled(view) && !view.project_directory.loading
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "named save content".into()
        })))
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.state.as_ref().unwrap().unsaved
    });
    editor.update_in(&mut visual, |view, window, _| view.focus.focus(window));
    visual.run_until_parked();
    visual.dispatch_action(SaveProject);
    saved(&mut visual, &editor, &current);
    pump(&editor, &mut visual, |view| {
        settled(view) && !view.saving_project && !view.state.as_ref().unwrap().unsaved
    });
    flush(&editor, &mut visual);
    assert!(cx.pending_save_options().is_none());
    assert_ne!(std::fs::read(&current).unwrap(), original);
    assert!(!remembered.join("Named Take.hex").exists());
    assert!(!remembered.join("untitled.hex").exists());
    assert_eq!(
        hex_preferences::load_project_directory(&owned.preferences()).unwrap(),
        Some(current.parent().unwrap().to_owned())
    );
}
