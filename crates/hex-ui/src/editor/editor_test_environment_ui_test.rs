//! Constructor roots are checked before the deterministic executor runs startup I/O.
use super::*;

#[gpui::test]
fn browser_test_environment_ordinary_constructor_has_private_roots_before_startup(
    cx: &mut TestAppContext,
) {
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new(
                    window,
                    cx,
                    Service::new(48_000, 16, 8_000_000).unwrap(),
                    Err("offline sandbox test".into()),
                )
            })
        })
        .unwrap();
    // Do not park/drain until these assertions pass: the original constructor
    // queues ambient discovery. Failing here leaves those test-executor jobs unpolled.
    editor.read_with(cx, |view, _| {
        assert!(
            view.browser.home.starts_with(std::env::temp_dir()),
            "ordinary constructor queued ambient home discovery: {:?}",
            view.browser.home
        );
        assert_ne!(
            view.browser.home,
            std::env::temp_dir(),
            "the shared system temporary folder is not a private test home"
        );
        assert_eq!(view.browser.directory, view.browser.music);
        assert!(view.browser.music.starts_with(&view.browser.home));
        assert!(view.browser.music.is_dir());
        assert!(
            view.browser
                .places
                .iter()
                .all(|path| path.starts_with(&view.browser.home))
        );
        let owner = view.browser.home.parent().unwrap();
        assert!(view.browser.preferences_path.starts_with(owner));
        assert!(view.recent_projects.test_path().starts_with(owner));
        assert!(view.theme_settings.test_path().starts_with(owner));
    });
}

fn ordinary(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let handle = *cx.add_window(|_, _| Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new(
                    window,
                    cx,
                    Service::new(48_000, 16, 8_000_000).unwrap(),
                    Err("offline sandbox test".into()),
                )
            })
        })
        .unwrap();
    (handle, editor)
}

#[gpui::test]
fn browser_test_environment_unique_editors_complete_real_directory_and_catalog_startup(
    cx: &mut TestAppContext,
) {
    let mut roots = Vec::new();
    for _ in 0..2 {
        let (handle, editor) = ordinary(cx);
        let (root, home, database) = editor.read_with(cx, |view, _| {
            let root = view.test_environment.root().to_owned();
            assert!(view.browser.home.starts_with(&root));
            assert!(view.browser.music.starts_with(&root));
            assert_eq!(view.browser.directory, view.browser.music);
            assert!(
                view.browser
                    .places
                    .iter()
                    .all(|path| path.starts_with(&root))
            );
            assert!(
                view.sample_roots()
                    .iter()
                    .all(|path| path.starts_with(&root))
            );
            assert!(view.browser.preferences_path.starts_with(&root));
            assert!(view.recent_projects.test_path().starts_with(&root));
            assert!(view.theme_settings.test_path().starts_with(&root));
            (
                root,
                view.browser.home.clone(),
                view.browser
                    .preferences_path
                    .with_extension("samples-v1.db"),
            )
        });
        let visual = &mut VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.read_with(visual, |view, _| {
            assert!(!view.browser.preferences_loading);
            assert!(!view.browser.loading);
            assert_eq!(
                view.browser.listing.as_ref().unwrap().directory,
                home.join("Music")
            );
        });
        assert!(database.is_file(), "real catalog save completed");
        let catalog = hex_library::catalog::SampleCatalog::load(&database).unwrap();
        assert!(catalog.entries().is_empty());
        roots.push(root);
    }
    assert_ne!(
        roots[0], roots[1],
        "ordinary editors must not share configuration or discovery roots"
    );
}

#[gpui::test]
fn browser_test_environment_missing_music_falls_back_only_to_owned_home(cx: &mut TestAppContext) {
    let (handle, editor) = ordinary(cx);
    let home = editor.read_with(cx, |view, _| {
        assert!(view.browser.home.starts_with(view.test_environment.root()));
        std::fs::remove_dir(&view.browser.music).unwrap();
        view.browser.home.clone()
    });
    let wav = home.join("Fallback.WAV");
    std::fs::write(&wav, mono_pcm_wav(24)).unwrap();
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.directory, home);
        assert!(!view.browser.loading);
        assert!(view.browser.error.is_none());
        assert_eq!(view.browser.listing.as_ref().unwrap().directory, home);
        assert!(
            view.browser
                .listing
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.path == wav)
        );
        assert!(
            view.sample_roots()
                .iter()
                .all(|path| path.starts_with(&home))
        );
    });
}

#[gpui::test]
fn browser_test_environment_explicit_missing_corrupt_and_populated_preferences_keep_native_paths(
    cx: &mut TestAppContext,
) {
    for mode in 0..3 {
        let directory = PreferencesTestDirectory::new();
        let prefs = directory.0.join("Custom Preferences.json");
        let recent = directory.0.join("Recent Projects.json");
        let place = directory.0.join("Original Case Samples");
        std::fs::create_dir(&place).unwrap();
        match mode {
            0 => {}
            1 => std::fs::write(&prefs, b"malformed preferences preserved").unwrap(),
            2 => hex_preferences::save(
                &prefs,
                &hex_preferences::BrowserPlaces::new([place.clone()]).unwrap(),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let original = std::fs::read(&prefs).ok();
        let handle = *cx.add_window(|_, _| Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test(
                        window,
                        cx,
                        Service::new(48_000, 16, 8_000_000).unwrap(),
                        Err("offline explicit preference test".into()),
                        prefs.clone(),
                        recent.clone(),
                    )
                })
            })
            .unwrap();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.browser.preferences_path, prefs);
            assert_eq!(view.recent_projects.test_path(), recent);
            assert!(
                view.theme_settings
                    .test_path()
                    .starts_with(view.test_environment.root())
            );
            assert!(view.browser.home.starts_with(view.test_environment.root()));
        });
        let visual = &mut VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.read_with(visual, |view, _| {
            assert!(!view.browser.preferences_loading);
            if mode == 1 {
                assert!(!view.browser.preferences_writable);
                assert!(view.browser.places_status.is_some());
            }
            if mode == 2 {
                assert!(view.browser.places.contains(&place));
            }
        });
        assert_eq!(
            std::fs::read(&prefs).ok(),
            original,
            "explicit missing/corrupt/populated file must not be replaced by sandbox defaults"
        );
        visual.update(|window, cx| {
            let _ = window.replace_root(cx, |_, _| Empty);
        });
        drop(editor);
        assert!(
            directory.0.is_dir(),
            "caller-owned directory must never be adopted/deleted"
        );
        assert!(place.is_dir());
        assert_eq!(std::fs::read(&prefs).ok(), original);
    }
}

#[gpui::test]
fn browser_test_environment_actual_catalog_save_keeps_root_after_view_drop_then_cleans(
    cx: &mut TestAppContext,
) {
    use std::sync::{Arc, atomic::Ordering};
    let (handle, editor) = ordinary(cx);
    let (root, weak, gate) = editor.read_with(cx, |view, _| {
        (
            view.test_environment.root().to_owned(),
            Arc::downgrade(&view.test_environment),
            view.test_environment.gate_next_catalog_save(),
        )
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        gate.reached.load(Ordering::Acquire),
        "held inside the actual catalog scan/save worker"
    );
    assert!(!gate.finished.load(Ordering::Acquire));
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    let editor_weak = editor.downgrade();
    drop(editor);
    // GPUI defers Entity value destruction until an App update flushes releases.
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(
        editor_weak.upgrade().is_none(),
        "the actual editor owner was released before the job resumes"
    );
    assert!(
        weak.upgrade().is_some(),
        "actual filesystem job owns the environment after view drop"
    );
    assert_eq!(
        weak.strong_count(),
        1,
        "only the actual held filesystem job retains the environment"
    );
    assert!(root.is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(
        gate.finished.load(Ordering::Acquire),
        "actual catalog save ran after release, not a stub"
    );
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(
        weak.upgrade().is_none(),
        "no editor/FS-job guards left after actual save"
    );
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "background-owned root cleanup timed out"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(
        !root.exists(),
        "drained filesystem work must not recreate the deleted root"
    );
}
