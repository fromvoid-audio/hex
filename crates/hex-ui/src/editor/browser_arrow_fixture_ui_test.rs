//! The arrow regressions share one root with their actual filesystem jobs.
use super::*;
use crate::editor::test_environment::Environment;

pub(super) fn arrow_row_fixture(
    cx: &mut TestAppContext,
    sample: bool,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Arc<Environment>,
    std::path::PathBuf,
) {
    let environment = Environment::new().expect("owned arrow fixture environment");
    with_environment(cx, sample, environment)
}

fn with_environment(
    cx: &mut TestAppContext,
    sample: bool,
    environment: Arc<Environment>,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Arc<Environment>,
    std::path::PathBuf,
) {
    cx.update(crate::editor::bind_editor_keys);
    let samples = environment.root().join("samples");
    std::fs::create_dir_all(&samples).unwrap();
    if !sample {
        std::fs::create_dir(samples.join("a")).unwrap();
        std::fs::write(samples.join("a/child.hex"), []).unwrap();
    }
    std::fs::write(samples.join("loop.wav"), mono_pcm_wav(24)).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
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
    editor.update(cx, |v, cx| {
        v.browser.auto_audition = false;
        v.refresh_sample_catalog(cx);
        v.request_browser_scan(samples.clone(), cx);
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let ready = editor.update(cx, |v, cx| {
            v.tick(cx);
            cx.notify();
            v.state.is_some() && v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real Service Init did not arrive"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    (handle, editor, environment, samples)
}

#[gpui::test]
fn browser_arrows_fixture_catalog_job_owns_root_after_caller_and_view_drop(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering;
    let environment = Environment::new().unwrap();
    let root = environment.root().to_owned();
    let weak = Arc::downgrade(&environment);
    let gate = environment.gate_next_catalog_save();
    let (handle, editor, owner, samples) = with_environment(cx, true, environment);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(gate.reached.load(Ordering::Acquire));
    assert!(!gate.finished.load(Ordering::Acquire));
    let paths = editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&owner, &view.test_environment));
        assert_eq!(view.test_environment.root(), root);
        [
            view.browser.preferences_path.clone(),
            view.recent_projects.test_path().to_owned(),
            view.theme_settings.test_path().to_owned(),
            view.project_directory.test_path().to_owned(),
        ]
    });
    for (path, name) in paths.iter().zip([
        "browser.json",
        "recent-projects.json",
        "theme.json",
        "project-directory.json",
    ]) {
        assert_eq!(*path, root.join("config").join(name));
        assert!(path.parent().unwrap().is_dir());
    }
    let database = paths[0].with_extension("samples-v1.db");
    assert_eq!(database, root.join("config/browser.samples-v1.db"));
    assert_eq!(samples, root.join("samples"));
    let wav = samples.join("loop.wav");
    assert_eq!(std::fs::read(&wav).unwrap(), mono_pcm_wav(24));
    assert!(
        !database.exists(),
        "held before the first real catalog save"
    );

    let editor_weak = editor.downgrade();
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    drop(editor);
    drop(owner);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(editor_weak.upgrade().is_none());
    assert_eq!(
        weak.strong_count(),
        1,
        "only the actual held job owns the root"
    );
    assert!(root.is_dir());
    assert_eq!(std::fs::read(&wav).unwrap(), mono_pcm_wav(24));
    for path in &paths {
        assert!(path.parent().unwrap().is_dir());
    }
    assert!(!gate.finished.load(Ordering::Acquire));

    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none());
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(std::time::Instant::now() < deadline, "owned root cleanup");
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(
        !root.exists(),
        "completed jobs do not recreate the retired root"
    );
}
