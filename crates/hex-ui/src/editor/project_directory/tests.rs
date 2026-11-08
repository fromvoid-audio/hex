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
    let until = crate::test_wait::hang_deadline();
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
    let until = crate::test_wait::hang_deadline();
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
    let until = crate::test_wait::hang_deadline();
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
    let until = crate::test_wait::hang_deadline();
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
    let until = crate::test_wait::hang_deadline();
    while cx.pending_save_options().is_none() {
        visual.run_until_parked();
        assert!(
            std::time::Instant::now() < until,
            "project_directory_dirty_open_cancel_save_as_cancel_and_dont_save_failure_keep_folder: hang guard expired"
        );
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
        assert!(!unavailable.is_dir());
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
fn project_directory_existing_native_folder_preserves_case_and_bytes(cx: &mut TestAppContext) {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let owned = Directory::new();
    // The native macOS filesystem rejects invalid UTF-8 filenames (EILSEQ).
    // Exercise a real supported directory here; the codec and NSURL tests
    // independently preserve non-UTF-8 path bytes without creating that name.
    let name = if cfg!(target_os = "macos") {
        std::ffi::OsString::from("RawCase-Café")
    } else {
        std::ffi::OsString::from_vec(b"RawCase-\xff".to_vec())
    };
    let folder = owned.0.join(name);
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
    let gate = Arc::new(TestGate::default());
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
    let until = crate::test_wait::hang_deadline();
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

#[gpui::test]
fn project_directory_stale_preparation_clears_only_its_owned_open_intent(cx: &mut TestAppContext) {
    let owned = Directory::new();
    let folder = owned.folder("Remembered");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let (mut visual, editor) = owned.window(cx);
    let gate = Arc::new(TestGate::default());
    let generation = editor.read_with(&visual, |view, _| view.project_generation);
    editor.update(&mut visual, |view, _| {
        view.project_directory.directory_gate = Some(gate.clone())
    });
    visual.dispatch_action(OpenProject);
    pump(&editor, &mut visual, |_| {
        gate.reached.load(Ordering::Acquire)
    });
    assert!(cx.pending_open_directory().is_none());
    // The independent real actor replacement advances the actual UI generation.
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap()
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.project_generation == generation + 1
    });
    gate.release();
    pump(&editor, &mut visual, |view| !view.project_open.busy());
    assert!(cx.pending_open_directory().is_none());
    open(&mut visual, &editor);
    assert_eq!(cx.pending_open_directory(), Some(Some(folder)));
    cx.simulate_paths_selection(None);
    visual.run_until_parked();
    flush(&editor, &mut visual);
}

#[gpui::test]
fn project_directory_closed_window_does_not_receive_a_late_native_open(cx: &mut TestAppContext) {
    let owned = Directory::new();
    let folder = owned.folder("Remembered");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let original = std::fs::read(owned.preferences()).unwrap();
    let (mut visual, editor) = owned.window(cx);
    let gate = Arc::new(TestGate::default());
    editor.update(&mut visual, |view, _| {
        view.project_directory.directory_gate = Some(gate.clone())
    });
    visual.dispatch_action(OpenProject);
    pump(&editor, &mut visual, |_| {
        gate.reached.load(Ordering::Acquire)
    });
    visual.update(|window, _| window.remove_window());
    gate.release();
    pump(&editor, cx, |view| !view.project_open.busy());
    assert!(cx.pending_open_directory().is_none());
    assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
}

#[gpui::test]
fn project_directory_stale_save_response_cannot_save_into_the_replacement_document(
    cx: &mut TestAppContext,
) {
    let owned = Directory::new();
    let folder = owned.folder("Remembered");
    hex_preferences::save_project_directory(&owned.preferences(), &folder).unwrap();
    let original = std::fs::read(owned.preferences()).unwrap();
    let (mut visual, editor) = owned.window(cx);
    let generation = editor.read_with(&visual, |view, _| view.project_generation);
    save_as(&mut visual, &editor, false);
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap()
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.project_generation == generation + 1
    });
    let late = owned.0.join("Must Not Be Saved.hex");
    cx.simulate_new_path_selection(|_| Some(late.clone()));
    visual.run_until_parked();
    assert!(!late.exists());
    editor.read_with(&visual, |view, _| {
        assert!(!view.save_dialog_open);
        assert!(view.state.as_ref().unwrap().document_path.is_none());
    });
    save_as(&mut visual, &editor, false);
    cx.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    flush(&editor, &mut visual);
    assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
}

#[gpui::test]
fn project_directory_ordinary_constructor_resolves_fourth_path_before_any_job(
    cx: &mut TestAppContext,
) {
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline owned environment".into()))
            })
        })
        .unwrap();
    pump(&editor, cx, |view| {
        settled(view) && !view.project_directory.loading
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.project_directory.path,
            view.test_environment
                .root()
                .join("config/project-directory.json")
        );
        assert!(view.project_directory.directory.is_none());
        assert!(
            !view.project_directory.path.exists(),
            "merely opening a picker/starting up must not create preference data"
        );
    });
    cx.update_window(handle, |_, window, _| window.remove_window())
        .unwrap();
    drop(editor);
}

fn startup_named_document(
    cx: &mut TestAppContext,
    preferences: u8,
) -> (
    Directory,
    VisualTestContext,
    gpui::Entity<EditorView>,
    Arc<TestGate>,
    Arc<TestGate>,
    PathBuf,
) {
    let owned = Directory::new();
    let current = owned
        .folder("Named Save Before Preferences")
        .join("Current.hex");
    let remembered = owned.folder("Old Remembered Folder");
    hex_preferences::save_project_directory(&owned.preferences(), &remembered).unwrap();
    match preferences {
        0 => {}
        1 => std::fs::write(owned.preferences(), b"malformed original preferences").unwrap(),
        2 => {
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(owned.preferences()).unwrap()).unwrap();
            value["version"] = serde_json::json!(u32::MAX);
            std::fs::write(owned.preferences(), serde_json::to_vec(&value).unwrap()).unwrap();
        }
        _ => unreachable!(),
    }
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    service_state(&service);
    service.submit(Request::SaveNew(current.clone())).unwrap();
    let initial = service_state(&service);
    let gate = Arc::new(TestGate::default());
    INITIAL_LOAD_GATE.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(gate.clone());
    });
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
            editor.update(cx, |view, cx| {
                view.state = Some(initial);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    let save_gate = Arc::new(TestGate::default());
    editor.update(&mut visual, |view, _| {
        view.project_directory.save_gate = Some(save_gate.clone())
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && gate.reached.load(Ordering::Acquire)
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "actual startup save".into()
        })))
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.state.as_ref().unwrap().unsaved
    });
    editor.update_in(&mut visual, |view, window, _| view.focus.focus(window));
    visual.run_until_parked();
    (owned, visual, editor, gate, save_gate, current)
}
fn named_startup_success(
    visual: &mut VisualTestContext,
    editor: &gpui::Entity<EditorView>,
    current: &std::path::Path,
) {
    let before = std::fs::read(current).unwrap();
    visual.dispatch_action(SaveProject);
    pump(editor, visual, |view| {
        settled(view) && !view.saving_project && !view.state.as_ref().unwrap().unsaved
    });
    assert!(
        visual.pending_save_options().is_none(),
        "named Save has no chooser"
    );
    assert_ne!(
        std::fs::read(current).unwrap(),
        before,
        "canonical named Save changed actual project bytes"
    );
    editor.read_with(visual, |view, _| {
        assert!(
            view.project_directory.loading,
            "startup load remains held through actual success"
        );
        assert_eq!(
            view.project_directory
                .writes
                .lock()
                .unwrap()
                .pending
                .as_deref(),
            current.parent()
        );
    });
}
fn finish_startup_jobs(cx: &mut TestAppContext, gate: &TestGate, writes: &Arc<Mutex<Writes>>) {
    let until = crate::test_wait::hang_deadline();
    while !gate.finished.load(Ordering::Acquire) || writes.lock().unwrap().running {
        cx.run_until_parked();
        assert!(
            std::time::Instant::now() < until,
            "owned startup jobs stalled"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    cx.run_until_parked();
}

#[gpui::test]
fn project_directory_named_save_during_initial_load_persists_after_view_drop(
    cx: &mut TestAppContext,
) {
    let (owned, mut visual, editor, gate, save_gate, current) = startup_named_document(cx, 0);
    named_startup_success(&mut visual, &editor, &current);
    pump(&editor, &mut visual, |_| {
        save_gate.reached.load(Ordering::Acquire)
    });
    let writes = editor.read_with(&visual, |view, _| {
        Arc::clone(&view.project_directory.writes)
    });
    let weak = editor.downgrade();
    close(visual, editor);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "owned writer must not pin view");
    gate.release();
    save_gate.release();
    finish_startup_jobs(cx, &gate, &writes);
    assert_eq!(
        hex_preferences::load_project_directory(&owned.preferences()).unwrap(),
        Some(current.parent().unwrap().to_owned())
    );
}

#[gpui::test]
fn project_directory_startup_malformed_and_unsupported_stores_survive_save_and_view_drop(
    cx: &mut TestAppContext,
) {
    for format in [1, 2] {
        for drop_first in [false, true] {
            let (owned, mut visual, editor, gate, save_gate, current) =
                startup_named_document(cx, format);
            let original = std::fs::read(owned.preferences()).unwrap();
            named_startup_success(&mut visual, &editor, &current);
            pump(&editor, &mut visual, |_| {
                save_gate.reached.load(Ordering::Acquire)
            });
            let writes = editor.read_with(&visual, |view, _| {
                Arc::clone(&view.project_directory.writes)
            });
            if !drop_first {
                save_gate.release();
                pump(&editor, &mut visual, |view| {
                    !view.project_directory.writes.lock().unwrap().running
                        && view
                            .error
                            .as_deref()
                            .is_some_and(|e| e.contains("could not save project folder"))
                });
                editor.read_with(&visual, |view, _| {
                    assert!(
                        !view.state.as_ref().unwrap().unsaved,
                        "preference failure does not relabel successful project Save"
                    )
                });
                assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
            }
            let weak = editor.downgrade();
            close(visual, editor);
            cx.run_until_parked();
            assert!(weak.upgrade().is_none());
            gate.release();
            save_gate.release();
            finish_startup_jobs(cx, &gate, &writes);
            assert_eq!(
                std::fs::read(owned.preferences()).unwrap(),
                original,
                "malformed/unsupported bytes must not be replaced after view drop"
            );
        }
    }
}

#[gpui::test]
fn project_directory_startup_latest_canonical_success_and_old_load_cannot_replace_folder(
    cx: &mut TestAppContext,
) {
    let (owned, mut visual, editor, gate, save_gate, current) = startup_named_document(cx, 0);
    named_startup_success(&mut visual, &editor, &current);
    pump(&editor, &mut visual, |_| {
        save_gate.reached.load(Ordering::Acquire)
    });
    let latest = owned.folder("Latest Canonical Success").join("Latest.hex");
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::SaveNew(latest.clone())))
    });
    pump(&editor, &mut visual, |view| {
        settled(view)
            && view.state.as_ref().unwrap().document_path.as_deref() == Some(latest.as_path())
    });
    assert!(
        latest.is_file(),
        "latest path is actual canonical SaveNew completion, not picker selection"
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.project_directory
                .writes
                .lock()
                .unwrap()
                .pending
                .as_deref(),
            latest.parent()
        )
    });
    // Complete the old startup load while the live view still holds newer success.
    gate.release();
    pump(&editor, &mut visual, |view| !view.project_directory.loading);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.project_directory.directory.as_deref(), latest.parent())
    });
    let writes = editor.read_with(&visual, |view, _| {
        Arc::clone(&view.project_directory.writes)
    });
    close(visual, editor);
    save_gate.release();
    finish_startup_jobs(cx, &gate, &writes);
    assert_eq!(
        hex_preferences::load_project_directory(&owned.preferences()).unwrap(),
        Some(latest.parent().unwrap().to_owned())
    );
}

#[gpui::test]
fn project_directory_failed_named_save_during_startup_does_not_remember(cx: &mut TestAppContext) {
    let (owned, mut visual, editor, gate, save_gate, current) = startup_named_document(cx, 0);
    let original = std::fs::read(owned.preferences()).unwrap();
    // Deliberately block only this private fixture's named document destination.
    std::fs::remove_file(&current).unwrap();
    std::fs::create_dir(&current).unwrap();
    visual.dispatch_action(SaveProject);
    pump(&editor, &mut visual, |view| {
        settled(view) && !view.saving_project && view.error.is_some()
    });
    editor.read_with(&visual, |view, _| {
        assert!(view.state.as_ref().unwrap().unsaved);
        assert!(
            view.project_directory
                .writes
                .lock()
                .unwrap()
                .pending
                .is_none()
        );
        assert!(!view.project_directory.writes.lock().unwrap().running);
    });
    assert!(
        !save_gate.reached.load(Ordering::Acquire),
        "failed Save cannot start folder writer"
    );
    let writes = editor.read_with(&visual, |view, _| {
        Arc::clone(&view.project_directory.writes)
    });
    close(visual, editor);
    gate.release();
    save_gate.release();
    finish_startup_jobs(cx, &gate, &writes);
    assert_eq!(std::fs::read(owned.preferences()).unwrap(), original);
}
