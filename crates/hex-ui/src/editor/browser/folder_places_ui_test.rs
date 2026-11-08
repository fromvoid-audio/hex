use super::*;
use gpui::{AppContext, Empty, Modifiers, TestAppContext, VisualTestContext, point};
use hex_session::service::Service;

fn settle(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        let ready = editor.update(visual, |view, cx| {
            view.tick(cx);
            view.state.is_some()
                && !view.browser.preferences_loading
                && !view.browser.loading
                && !view.browser.folder_bookmark_pending
                && !view.browser.places_save_in_flight
                && view.browser.pending_places_save.is_none()
        });
        if ready {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "browser workers did not settle"
        );
        std::thread::yield_now();
    }
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Arc<super::super::test_environment::Environment>,
    PathBuf,
) {
    cx.update(super::super::bind_editor_keys);
    let environment = super::super::test_environment::Environment::new().unwrap();
    let samples = environment.root().join("samples");
    std::fs::create_dir_all(samples.join("outer/nested")).unwrap();
    // A real small WAV exists before any startup/catalog/preview worker.
    let data = [0_u8; 48];
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(samples.join("file.wav"), wav).unwrap();
    let handle = *cx.add_window(|_, _| Empty);
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
    editor.read_with(cx, |view, _| {
        assert!(Arc::ptr_eq(&environment, &view.test_environment));
        assert_eq!(
            view.browser.preferences_path,
            environment.root().join("config/browser.json")
        );
        assert_eq!(view.browser.directory, samples);
    });
    (handle, editor, environment, samples)
}

fn drag_row(visual: &mut VisualTestContext, row: usize, target: gpui::Point<gpui::Pixels>) {
    let from = visual
        .debug_bounds(
            [
                "browser-entry-0",
                "browser-entry-1",
                "browser-entry-2",
                "browser-entry-3",
            ][row],
        )
        .unwrap()
        .center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        from + point(px(12.0), px(0.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn folders_drag_to_places_persists_without_import_or_navigation(cx: &mut TestAppContext) {
    let (handle, editor, environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    settle(&editor, visual);
    editor.update(visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, visual);
    let before = editor.read_with(visual, |view, _| {
        (
            format!("{:?}", view.state.as_ref().unwrap().project),
            view.state.as_ref().unwrap().revision,
            view.browser.playback_generation,
        )
    });
    let row = editor.read_with(visual, |view, cx| {
        view.browser_visible_entries(cx)
            .iter()
            .position(|entry| entry.path == samples.join("outer"))
            .unwrap()
    });
    let target = visual.debug_bounds("browser-place-0").unwrap().center();
    drag_row(visual, row, target);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        settle(&editor, visual);
        if editor.read_with(visual, |view, _| {
            view.browser.places.contains(&samples.join("outer"))
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "folder drag did not create a bookmark"
        );
        std::thread::yield_now();
    }
    settle(&editor, visual);
    let saved = hex_preferences::load(&environment.root().join("config/browser.json")).unwrap();
    assert!(saved.additional().contains(&samples.join("outer")));
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.section, BrowserSection::Place(samples.clone()));
        assert_eq!(view.browser.search.read(cx).text(), "");
        assert_eq!(
            before,
            (
                format!("{:?}", view.state.as_ref().unwrap().project),
                view.state.as_ref().unwrap().revision,
                view.browser.playback_generation
            )
        );
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

fn row_for(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
    path: &std::path::Path,
) -> usize {
    editor.read_with(visual, |view, cx| {
        view.browser_visible_entries(cx)
            .iter()
            .position(|entry| entry.path == path)
            .unwrap()
    })
}

fn begin_folder_drag(visual: &mut VisualTestContext, row: usize) {
    let from = visual
        .debug_bounds(
            [
                "browser-entry-0",
                "browser-entry-1",
                "browser-entry-2",
                "browser-entry-3",
            ][row],
        )
        .unwrap()
        .center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        from + point(px(12.0), px(0.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(visual.update(|_, cx| cx.has_active_drag()));
}

#[gpui::test]
fn nested_folder_duplicate_preserves_icon_order_and_reopens(cx: &mut TestAppContext) {
    let (handle, editor, environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    settle(&editor, visual);
    editor.update(visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, visual);
    let row = row_for(&editor, visual, &samples.join("outer"));
    let disclosure = visual
        .debug_bounds(["browser-disclosure-0", "browser-disclosure-1"][row])
        .unwrap()
        .center();
    visual.simulate_mouse_down(disclosure, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(disclosure, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        if editor.read_with(visual, |view, cx| {
            view.browser_visible_entries(cx)
                .iter()
                .any(|entry| entry.path == samples.join("outer/nested"))
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nested_folder_duplicate_preserves_icon_order_and_reopens: hang guard expired"
        );
        std::thread::yield_now();
    }
    let row = row_for(&editor, visual, &samples.join("outer/nested"));
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    drag_row(visual, row, target);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        settle(&editor, visual);
        if editor.read_with(visual, |view, _| {
            view.browser.places.contains(&samples.join("outer/nested"))
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nested_folder_duplicate_preserves_icon_order_and_reopens: hang guard expired"
        );
        std::thread::yield_now();
    }
    editor.update(visual, |view, cx| {
        view.set_browser_place_icon(samples.join("outer/nested"), Some("waveform"), cx)
    });
    settle(&editor, visual);
    let before = std::fs::read(environment.root().join("config/browser.json")).unwrap();
    let before_order = editor.read_with(visual, |view, _| view.browser.places.clone());
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    drag_row(visual, row, target);
    // Run the actual background directory check to completion, even for duplicates.
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        if editor.read_with(visual, |view, _| !view.browser.folder_bookmark_pending) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nested_folder_duplicate_preserves_icon_order_and_reopens: hang guard expired"
        );
        std::thread::yield_now();
    }
    settle(&editor, visual);
    assert_eq!(
        before,
        std::fs::read(environment.root().join("config/browser.json")).unwrap()
    );
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.places, before_order);
        assert_eq!(
            view.browser
                .place_icons
                .get(&samples.join("outer/nested"))
                .map(String::as_str),
            Some("waveform")
        );
    });
    let reopened_handle = *visual.add_window(|_, _| Empty);
    let reopened = visual
        .update_window(reopened_handle, |_, window, cx| {
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
    let reopened_visual = &mut VisualTestContext::from_window(reopened_handle, visual);
    settle(&reopened, reopened_visual);
    reopened.read_with(reopened_visual, |view, _| {
        assert!(Arc::ptr_eq(&environment, &view.test_environment));
        assert_eq!(view.browser.places, before_order);
        assert_eq!(
            view.browser
                .place_icons
                .get(&samples.join("outer/nested"))
                .map(String::as_str),
            Some("waveform")
        );
    });
}

#[gpui::test]
fn cancelled_outside_stale_and_missing_folder_do_not_bookmark(cx: &mut TestAppContext) {
    let (handle, editor, environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    settle(&editor, visual);
    editor.update(visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, visual);
    let before = editor.read_with(visual, |view, _| {
        (
            view.browser.places.clone(),
            format!("{:?}", view.state.as_ref().unwrap().project),
            view.state.as_ref().unwrap().revision,
        )
    });
    let row = row_for(&editor, visual, &samples.join("outer"));
    // Actual Escape retires the GPUI drag before a later Places release.
    begin_folder_drag(visual, row);
    visual.simulate_keystrokes("escape");
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    drag_row(visual, row, point(px(1000.0), px(5.0)));
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
    // A source change during a held drag invalidates the old visible-row payload.
    begin_folder_drag(visual, row);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    settle(&editor, visual);
    editor.update(visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, visual);
    let row = row_for(&editor, visual, &samples.join("outer"));
    begin_folder_drag(visual, row);
    std::fs::remove_dir_all(samples.join("outer")).unwrap();
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        if editor.read_with(visual, |view, _| {
            view.browser
                .places_status
                .as_deref()
                .is_some_and(|message| message.starts_with("could not bookmark folder"))
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cancelled_outside_stale_and_missing_folder_do_not_bookmark: hang guard expired"
        );
        std::thread::yield_now();
    }
    editor.read_with(visual, |view, _| {
        assert_eq!(
            before,
            (
                view.browser.places.clone(),
                format!("{:?}", view.state.as_ref().unwrap().project),
                view.state.as_ref().unwrap().revision
            )
        );
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
    assert!(!environment.root().join("config/browser.json").exists());
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
}

#[gpui::test]
fn file_drop_is_not_a_bookmark_and_malformed_preferences_remain_untouched(cx: &mut TestAppContext) {
    let (handle, editor, environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    settle(&editor, visual);
    editor.update(visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, visual);
    let before = editor.read_with(visual, |view, _| {
        (
            view.browser.places.clone(),
            format!("{:?}", view.state.as_ref().unwrap().project),
            view.state.as_ref().unwrap().revision,
        )
    });
    let row = row_for(&editor, visual, &samples.join("file.wav"));
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    drag_row(visual, row, target);
    settle(&editor, visual);
    editor.read_with(visual, |view, _| {
        assert_eq!(
            before,
            (
                view.browser.places.clone(),
                format!("{:?}", view.state.as_ref().unwrap().project),
                view.state.as_ref().unwrap().revision
            )
        );
        assert!(!view.browser.folder_bookmark_pending);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
    let path = environment.root().join("config/browser.json");
    std::fs::write(&path, b"malformed settings must survive").unwrap();
    editor.update(visual, |view, cx| view.start_browser_places_load(cx));
    // This reload uses the actual error completion, rather than editing writability.
    let deadline = crate::test_wait::hang_deadline();
    loop {
        visual.run_until_parked();
        if editor.read_with(visual, |view, _| !view.browser.preferences_writable) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "file_drop_is_not_a_bookmark_and_malformed_preferences_remain_untouched: hang guard expired"
        );
        std::thread::yield_now();
    }
    let row = row_for(&editor, visual, &samples.join("outer"));
    drag_row(visual, row, target);
    settle(&editor, visual);
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"malformed settings must survive"
    );
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.places, before.0);
        assert!(
            view.browser
                .places_status
                .as_ref()
                .unwrap()
                .starts_with("could not load browser places")
        );
    });
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
}

#[gpui::test]
fn bookmark_fixture_catalog_worker_retains_same_root_after_view_drop(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let (handle, editor, environment, samples) = fixture(cx);
    let root = environment.root().to_owned();
    let weak = Arc::downgrade(&environment);
    let gate = environment.gate_next_catalog_save();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(gate.reached.load(Ordering::Acquire));
    editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&environment, &view.test_environment));
        assert_eq!(view.browser.directory, samples);
        assert_eq!(
            view.browser
                .preferences_path
                .with_extension("samples-v1.db"),
            root.join("config/browser.samples-v1.db")
        );
    });
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    let editor_weak = editor.downgrade();
    drop(editor);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(editor_weak.upgrade().is_none());
    drop(environment);
    assert_eq!(
        weak.strong_count(),
        1,
        "only actual catalog job retains the fixture root"
    );
    assert!(root.is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none());
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "bookmark_fixture_catalog_worker_retains_same_root_after_view_drop: hang guard expired"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(!root.exists());
}

#[gpui::test]
fn folder_drag_feedback_and_geometry_are_stable_in_normal_and_compact_places(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    settle(&editor, visual);
    editor.update(visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, visual);
    for compact in [false, true] {
        if compact {
            let divider = visual
                .debug_bounds("browser-sidebar-resize")
                .unwrap()
                .center();
            visual.simulate_mouse_down(divider, MouseButton::Left, Modifiers::default());
            let minimum = divider - point(px(500.0), px(0.0));
            visual.simulate_mouse_move(minimum, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(minimum, MouseButton::Left, Modifiers::default());
        }
        let geometry = visual.debug_bounds("browser-place-0").unwrap();
        let row = row_for(&editor, visual, &samples.join("outer"));
        begin_folder_drag(visual, row);
        assert!(
            visual
                .debug_bounds("browser-folder-bookmark-ghost")
                .is_some()
        );
        let target = visual.debug_bounds("browser-places-drop").unwrap().center();
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        assert_eq!(visual.debug_bounds("browser-place-0"), Some(geometry));
        assert_eq!(
            visual.rendered_background("browser-places-drop"),
            Some(rgb(theme::color(theme::PaletteRole::Hover)).into())
        );
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(!visual.update(|_, cx| cx.has_active_drag()));
        assert_ne!(
            visual.rendered_background("browser-places-drop"),
            Some(rgb(theme::color(theme::PaletteRole::Hover)).into())
        );
        assert_eq!(visual.debug_bounds("browser-place-0"), Some(geometry));
        editor.read_with(visual, |view, _| {
            assert_eq!(view.browser.places, vec![samples.clone()]);
            assert!(view.pending_requests.is_empty());
        });
    }
}

#[gpui::test]
fn existing_audio_device_escape_precede_folder_fallback_and_idle_escape_clears_query(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    settle(&editor, visual);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("file");
    settle(&editor, visual);
    let before = editor.read_with(visual, |view, _| {
        (
            format!("{:?}", view.state.as_ref().unwrap().project),
            view.state.as_ref().unwrap().revision,
        )
    });
    let row = row_for(&editor, visual, &samples.join("file.wav"));
    begin_folder_drag(visual, row);
    editor.read_with(visual, |view, _| {
        assert!(view.active_browser_audio_drag.is_some())
    });
    visual.simulate_keystrokes("escape");
    editor.read_with(visual, |view, cx| {
        assert!(
            view.active_browser_audio_drag.is_none(),
            "existing audio cancellation branch must run"
        );
        assert_eq!(
            view.browser.search.read(cx).text(),
            "file",
            "audio cancellation precedes query clearing"
        );
    });
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("sine");
    settle(&editor, visual);
    let device = visual
        .debug_bounds("browser-device-instrument-hex.instrument.sine")
        .unwrap()
        .center();
    visual.simulate_mouse_down(device, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        device + point(px(12.0), px(0.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(visual, |view, _| {
        assert!(view.active_native_device_drag.is_some())
    });
    assert!(visual.update(|_, cx| cx.has_active_drag()));
    visual.simulate_keystrokes("escape");
    editor.read_with(visual, |view, cx| {
        assert!(
            view.active_native_device_drag.is_none(),
            "existing device cancellation branch must run"
        );
        assert_eq!(view.browser.search.read(cx).text(), "sine");
    });
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("query");
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
    visual.simulate_keystrokes("escape");
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "");
        assert_eq!(
            before,
            (
                format!("{:?}", view.state.as_ref().unwrap().project),
                view.state.as_ref().unwrap().revision
            )
        );
        assert_eq!(view.browser.places, vec![samples.clone()]);
        assert!(view.pending_requests.is_empty());
        assert!(view.pending_audio_commands.is_empty());
    });
}

#[gpui::test]
fn folder_validation_job_retains_root_after_view_and_caller_drop(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering;
    let (handle, editor, environment, samples) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    settle(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        view.select_browser_source(BrowserSection::Place(samples.clone()), cx)
    });
    settle(&editor, &mut visual);
    let root = environment.root().to_owned();
    let weak = Arc::downgrade(&environment);
    let gate = environment.gate_next_folder_validation();
    let row = row_for(&editor, &mut visual, &samples.join("outer"));
    let target = visual.debug_bounds("browser-places-drop").unwrap().center();
    drag_row(&mut visual, row, target);
    assert!(gate.reached.load(Ordering::Acquire));
    assert!(!gate.finished.load(Ordering::Acquire));
    let editor_weak = editor.downgrade();
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    drop(editor);
    drop(environment);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(editor_weak.upgrade().is_none());
    assert_eq!(
        weak.strong_count(),
        1,
        "only actual metadata job retains the root"
    );
    assert!(samples.join("outer").is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.validation_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_validation.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none());
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(std::time::Instant::now() < deadline, "owned root cleanup");
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(!root.exists());
}

fn settle_catalog(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        settle(editor, visual);
        if editor.read_with(visual, |view, _| !view.browser.samples.history_loading()) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sample catalog did not settle"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn moved_place_shows_missing_and_relocates_with_icon_order_and_samples(cx: &mut TestAppContext) {
    let (handle, editor, environment, samples) = fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    let old = environment.root().join("library/drums");
    let new = environment.root().join("moved/drums");
    std::fs::create_dir_all(old.join("kicks")).unwrap();
    std::fs::copy(samples.join("file.wav"), old.join("kicks/kick.wav")).unwrap();
    settle_catalog(&editor, visual);
    editor.update(visual, |view, cx| {
        view.browser.places.push(old.clone());
        view.queue_browser_places_save(cx);
        view.refresh_sample_catalog(cx);
    });
    settle_catalog(&editor, visual);
    editor.update(visual, |view, cx| {
        view.set_browser_place_icon(old.clone(), Some("waveform"), cx);
        view.select_browser_source(BrowserSection::Place(old.clone()), cx);
    });
    settle_catalog(&editor, visual);
    let index = editor.read_with(visual, |view, _| {
        assert!(!view.browser_place_missing(&old));
        assert!(
            view.browser
                .samples
                .catalog
                .record(&old.join("kicks/kick.wav"))
                .is_some_and(|record| record.fingerprint.is_some())
        );
        view.browser
            .places
            .iter()
            .position(|place| *place == old)
            .unwrap()
    });

    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::rename(&old, &new).unwrap();
    editor.update(visual, |view, cx| view.refresh_sample_catalog(cx));
    settle_catalog(&editor, visual);
    editor.read_with(visual, |view, _| assert!(view.browser_place_missing(&old)));
    let labels = visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.open_browser_place_icons(old.clone(), point(px(10.0), px(10.0)), window, cx);
            view.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .map(|item| item.label)
                .collect::<Vec<_>>()
        })
    });
    assert!(labels.contains(&"relocate…"));
    editor.update(visual, |view, cx| {
        view.context_menu = None;
        // Relocating onto an existing place is refused without changes.
        view.relocate_browser_place(old.clone(), samples.clone(), cx);
        assert_eq!(view.browser.places[index], old);
        view.relocate_browser_place(old.clone(), new.clone(), cx);
    });
    settle_catalog(&editor, visual);
    editor.read_with(visual, |view, _| {
        assert_eq!(view.browser.places[index], new);
        assert!(!view.browser.places.contains(&old));
        assert_eq!(
            view.browser.place_icons.get(&new).map(String::as_str),
            Some("waveform")
        );
        assert_eq!(view.browser.section, BrowserSection::Place(new.clone()));
        assert!(!view.browser_place_missing(&new) && !view.browser_place_missing(&old));
        let moved = new.join("kicks/kick.wav");
        assert!(view.browser.samples.catalog.record(&moved).is_some());
        assert!(
            view.browser
                .samples
                .catalog
                .entries()
                .iter()
                .all(|entry| !entry.path.starts_with(&old))
        );
    });
    let saved = hex_preferences::load(&environment.root().join("config/browser.json")).unwrap();
    assert!(saved.additional().contains(&new) && !saved.additional().contains(&old));
    assert_eq!(saved.icon(&new), Some("waveform"));
    assert!(
        new.join("kicks/kick.wav").is_file() && !old.exists(),
        "relocation never moves sample files"
    );
}
