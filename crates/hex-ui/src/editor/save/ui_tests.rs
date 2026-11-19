//! Headless native-dialog flow; no application or physical-device interaction.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_session::service::Service;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
            Instant::now() < until,
            "save flow stalled: {:?}",
            editor.read_with(cx, |view, _| view.error.clone())
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn settled(view: &EditorView) -> bool {
    view.state.is_some()
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && !view.saving_project
}
fn fixture(
    cx: &mut TestAppContext,
) -> (
    Files,
    PathBuf,
    Vec<u8>,
    VisualTestContext,
    gpui::Entity<EditorView>,
) {
    let root = std::env::temp_dir().join(format!(
        "hex-upgrade-dialog-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("samples")).unwrap();
    let path = root.join("My Original.hex");
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let until = crate::test_wait::hang_deadline();
    while service.try_event().is_err() {
        assert!(
            std::time::Instant::now() < until,
            "fixture: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    service.submit(Request::SaveNew(path.clone())).unwrap();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none());
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "fixture: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // The historical schema36 tempo lineage has this same envelope without sections.
    let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // This empty fixture also predates directed Arrangement content views.
    assert_eq!(
        old.as_object_mut().unwrap().remove("content_views"),
        Some(serde_json::json!([]))
    );
    // Schema36 also predates the complete activation/window inventory.
    assert_eq!(
        old.as_object_mut().unwrap().remove("clip_activation"),
        Some(serde_json::json!([]))
    );
    // This empty schema36 fixture predates horizontal fade coordinates.
    assert_eq!(
        old.as_object_mut()
            .unwrap()
            .remove("midi_one_shot_boundaries"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("fade_curve_positions"),
        Some(serde_json::json!([]))
    );
    // Schema42 adds event fades; the historical empty envelope has none.
    assert_eq!(
        old.as_object_mut().unwrap().remove("audio_event_fades"),
        Some(serde_json::json!([]))
    );
    // Schema43 adds the complete grouping inventory; this fixture is empty.
    assert_eq!(
        old.as_object_mut().unwrap().remove("grouped_audio"),
        Some(serde_json::json!([]))
    );
    // Schema44 adds required hierarchy rows; this fixture has no tracks.
    assert_eq!(
        old.as_object_mut().unwrap().remove("track_hierarchy"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("native_sources"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("track_source_bindings"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("session_audio_cycles"),
        Some(serde_json::json!([]))
    );
    assert_eq!(
        old.as_object_mut().unwrap().remove("device_automation"),
        Some(serde_json::json!([]))
    );
    for key in [
        "alias_sources",
        "alias_members",
        "inactive_midi_notes",
        "inactive_audio_events",
        "nested_audio",
        "audio_event_gains",
        "midi_clip_gains",
        "automatic_track_names",
    ] {
        assert_eq!(
            old.as_object_mut().unwrap().remove(key),
            Some(serde_json::json!([]))
        );
    }
    // Schema53 host automation must be empty before comparing this old envelope.
    assert_eq!(
        old.get("host_automation"),
        Some(&serde_json::json!([])),
        "historical fixture cannot discard host automation"
    );
    old.as_object_mut().unwrap().remove("host_automation");
    old["version"] = 36.into();
    old.as_object_mut().unwrap().remove("arrangement_sections");
    let original = serde_json::to_vec(&old).unwrap();
    fs::write(&path, &original).unwrap();
    service.submit(Request::Open(path.clone())).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_project_preferences(
                    window,
                    cx,
                    service,
                    &root,
                    root.join("project-directory.json"),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    pump(&editor, &mut visual, |view| {
        settled(view) && view.document_format_upgrade_required
    });
    assert!(!cx.has_pending_prompt(), "opening never asks to overwrite");
    (Files(root), path, original, visual, editor)
}
fn save(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    editor.update(visual, |view, cx| view.save_project(cx));
    visual.run_until_parked();
    assert!(visual.has_pending_prompt());
    let (_, detail) = visual.pending_prompt().unwrap();
    assert!(detail.contains("older hex versions"));
    assert!(detail.contains("new or versioned name"));
}
fn wait_for_picker(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        editor.update(visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        if visual.pending_save_options().is_some() {
            return;
        }
        assert!(Instant::now() < until, "save as picker did not open");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui::test]
fn older_format_save_cancel_picker_cancel_and_failed_save_as_preserve_original(
    cx: &mut TestAppContext,
) {
    let (_files, path, original, mut visual, editor) = fixture(cx);
    save(&editor, &mut visual);
    visual.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!editor.read_with(&visual, |view, _| view.save_dialog_open
        || view.saving_project));

    save(&editor, &mut visual);
    visual.simulate_prompt_answer("save as…");
    wait_for_picker(&editor, &mut visual);
    assert_eq!(
        visual.pending_save_options().unwrap().1,
        Some("My Original-upgraded.hex".into())
    );
    visual.simulate_new_path_selection(|_| None);
    visual.run_until_parked();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!editor.read_with(&visual, |view, _| view.save_dialog_open));

    save(&editor, &mut visual);
    visual.simulate_prompt_answer("save as…");
    wait_for_picker(&editor, &mut visual);
    visual.simulate_new_path_selection(|_| Some(path.clone()));
    pump(&editor, &mut visual, |view| {
        settled(view) && view.error.is_some()
    });
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(editor.read_with(&visual, |view, _| view.document_format_upgrade_required));
    visual.update(|window, _| window.remove_window());
}

#[gpui::test]
fn older_format_explicit_overwrite_warns_once_and_save_as_keeps_original(cx: &mut TestAppContext) {
    let (files, path, original, mut visual, editor) = fixture(cx);
    save(&editor, &mut visual);
    visual.simulate_prompt_answer("save as…");
    wait_for_picker(&editor, &mut visual);
    let copy = files.0.join("My Original-v2/My Original-v2.hex");
    visual.simulate_new_path_selection(|_| Some(files.0.join("My Original-v2.hex")));
    pump(&editor, &mut visual, |view| {
        settled(view) && !view.document_format_upgrade_required
    });
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        editor.read_with(&visual, |view, _| view
            .state
            .as_ref()
            .unwrap()
            .document_path
            .clone()),
        Some(copy)
    );

    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Open(path.clone())));
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.document_format_upgrade_required
    });
    save(&editor, &mut visual);
    visual.simulate_prompt_answer("overwrite original");
    pump(&editor, &mut visual, |view| {
        settled(view) && !view.document_format_upgrade_required
    });
    assert_ne!(fs::read(&path).unwrap(), original);
    editor.update(&mut visual, |view, cx| view.save_project(cx));
    visual.run_until_parked();
    assert!(!visual.has_pending_prompt());
    pump(&editor, &mut visual, settled);
    visual.update(|window, _| window.remove_window());
}

#[gpui::test]
fn older_format_cancel_during_save_before_open_clears_the_open_intent(cx: &mut TestAppContext) {
    let (files, path, original, mut visual, editor) = fixture(cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(hex_project::Command::AddTrack {
            name: "unsaved track".into()
        })));
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.state.as_ref().unwrap().unsaved
    });
    let next = files.0.join("next.hex");
    fs::write(&next, &original).unwrap();
    editor.update(&mut visual, |view, cx| {
        view.pending_open_path = Some(next);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.has_pending_prompt());
    visual.simulate_prompt_answer("save");
    visual.run_until_parked();
    assert!(visual.has_pending_prompt());
    assert!(editor.read_with(&visual, |view, _| view.project_open.permits_save()));
    visual.simulate_prompt_answer("cancel");
    visual.run_until_parked();
    assert!(!editor.read_with(&visual, |view, _| view.project_open.busy()));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        editor.read_with(&visual, |view, _| view
            .state
            .as_ref()
            .unwrap()
            .document_path
            .clone()),
        Some(path)
    );
    visual.update(|window, _| window.remove_window());
}

#[gpui::test]
fn worker_discovered_reader_warning_opens_prompt_and_can_cancel_or_overwrite(
    cx: &mut TestAppContext,
) {
    let (_files, path, original, mut visual, editor) = fixture(cx);
    // The UI can have an older displayed compatibility flag, for example when
    // current-schema metadata grows beyond a legacy reader's byte ceiling.
    // Exercise the real backend refusal with this small older-schema fixture;
    // the Service test separately crosses the actual size threshold.
    for answer in ["cancel", "save as…", "overwrite original"] {
        editor.update(&mut visual, |view, cx| {
            view.document_format_upgrade_required = false;
            view.save_project(cx);
        });
        pump(&editor, &mut visual, |view| {
            settled(view) && view.save_dialog_open
        });
        assert!(visual.has_pending_prompt());
        assert_eq!(fs::read(&path).unwrap(), original);
        visual.simulate_prompt_answer(answer);
        visual.run_until_parked();
        if answer == "save as…" {
            wait_for_picker(&editor, &mut visual);
            visual.simulate_new_path_selection(|_| None);
            visual.run_until_parked();
        }
        pump(&editor, &mut visual, settled);
        assert!(!editor.read_with(&visual, |view, _| view.save_dialog_open));
    }
    assert_ne!(fs::read(&path).unwrap(), original);
    assert!(!visual.has_pending_prompt());
    visual.update(|window, _| window.remove_window());
}

#[gpui::test]
fn worker_discovered_warning_cannot_overwrite_a_replaced_document(cx: &mut TestAppContext) {
    let (files, path, original, mut visual, editor) = fixture(cx);
    editor.update(&mut visual, |view, cx| {
        view.document_format_upgrade_required = false;
        view.save_project(cx);
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.save_dialog_open
    });
    assert!(visual.has_pending_prompt());
    let other = files.0.join("other.hex");
    fs::write(&other, &original).unwrap();
    editor.update(&mut visual, |view, _| {
        view.service.submit(Request::Open(other.clone())).unwrap();
    });
    pump(&editor, &mut visual, |view| {
        settled(view) && view.project_generation == 2
    });
    visual.simulate_prompt_answer("overwrite original");
    visual.run_until_parked();
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(&other).unwrap(), original);
    assert!(!editor.read_with(&visual, |view, _| view.saving_project
        || view.save_dialog_open));
    visual.update(|window, _| window.remove_window());
}
