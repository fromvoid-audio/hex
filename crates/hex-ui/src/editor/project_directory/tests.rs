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
