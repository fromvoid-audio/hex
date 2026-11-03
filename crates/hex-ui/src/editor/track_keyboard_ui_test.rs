//! Native action dispatch and canonical history for track keyboard shortcuts.
use super::clip_drag_ui_test::audio_project;
use super::{EditorView, test_environment::Environment};
use gpui::AppContext;
use gpui::{TestAppContext, VisualTestContext};
use hex_project::{Command, Created, Editor};
use std::sync::Arc;

// Only this private fixture: the caller and real filesystem jobs share one owner.
struct TrackTestDirectory(Arc<Environment>);
impl TrackTestDirectory {
    fn new() -> Self {
        let environment = Environment::new().expect("owned empty-track environment");
        std::fs::create_dir(environment.root().join("samples")).unwrap();
        Self(environment)
    }
}
fn publish(view: &mut EditorView, project: hex_project::Project) {
    view.state = Some(Arc::new(hex_session::service::ViewState {
        canonical_audio_source: None,
        comparison_audio: view
            .state
            .as_ref()
            .and_then(|state| state.comparison_audio.clone()),
        project: Arc::new(project),
        waveforms: Default::default(),
        source_overviews: Default::default(),
        audio_clip_waveforms: Default::default(),
        midi_preview: Arc::default(),
        gain_preview: Ok(Arc::default()),
        timeline_frames: 384_000,
        can_undo: false,
        can_redo: false,
        revision: 1,
        unsaved: false,
        document_path: None,
    }));
}
fn editor_window(
    cx: &mut TestAppContext,
    project: hex_project::Project,
    directory: &Arc<Environment>,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(directory),
                )
            });
            editor.update(cx, |view, cx| {
                publish(view, project);
                assert!(Arc::ptr_eq(directory, &view.test_environment));
                assert_eq!(view.browser.directory, directory.root().join("samples"));
                assert!(view.audio.is_none());
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor)
}

#[gpui::test]
fn command_t_uses_existing_insert_track_and_one_canonical_undo(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let directory = TrackTestDirectory::new();
    let (project, _, _, _) = audio_project();
    let original = project.clone();
    let (handle, editor) = editor_window(cx, project, &directory.0);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-t");
    let command = editor.read_with(&visual, |view, _| {
        assert!(matches!(
            &view.last_enqueued_edit,
            Some(Command::InsertDefaultTrack { before_track: None })
        ));
        view.last_enqueued_edit.clone().unwrap()
    });
    let mut canonical = Editor::new(original.clone(), 8).unwrap();
    let Created::Track(id) = canonical.execute(command).unwrap() else {
        panic!("track result")
    };
    assert_eq!(canonical.project().tracks().len(), 3);
    assert_eq!(canonical.project().track(id).unwrap().name(), "track 3");
    assert!(canonical.project().track(id).unwrap().name_is_automatic());
    let created = canonical.project().track(id).unwrap();
    assert!(created.instrument().is_none());
    assert!(created.devices().is_empty());
    assert_eq!(created.ordered_device_chain().count(), 0);
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            publish(view, canonical.project().clone());
            view.focus_selected_track(id, window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("device-add-toggle").is_some(),
        "empty track keeps real add affordance"
    );
    let selector = Box::leak(format!("device-drop-end-{}", id.value()).into_boxed_str());
    assert!(
        visual.debug_bounds(selector).is_some(),
        "empty track keeps real device drop target"
    );
    let add = visual.debug_bounds("device-add-toggle").unwrap();
    let pos = gpui::point(
        add.origin.x + add.size.width / 2.0,
        add.origin.y + add.size.height / 2.0,
    );
    visual.simulate_mouse_down(pos, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(pos, gpui::MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("device-add-menu").is_some());
    assert!(
        visual
            .debug_bounds("add-native-instrument-hex.instrument.sine")
            .is_some(),
        "explicit instrument choice remains available"
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_track, Some(id));
        let empty = view.state.as_ref().unwrap().project.track(id).unwrap();
        assert!(empty.instrument().is_none());
        assert_eq!(empty.ordered_device_chain().count(), 0);
    });
    let high_water = canonical.project().snapshot().next_id;
    assert!(canonical.undo());
    let mut expected = original.snapshot();
    expected.next_id = high_water;
    assert_eq!(canonical.project().snapshot(), expected);
    assert!(!canonical.undo());
    assert!(canonical.redo());
    assert_eq!(canonical.project().track(id).unwrap().name(), "track 3");
}

#[gpui::test]
fn command_t_preserves_browser_and_clip_name_text_focus(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for clip_name in [false, true] {
        let directory = TrackTestDirectory::new();
        let (project, _, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, &directory.0);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                if clip_name {
                    view.selected_clip = Some(clip);
                    view.begin_clip_name(window, cx);
                } else {
                    view.browser.search.read(cx).focus(window);
                }
            })
        });
        visual.run_until_parked();
        let focus = visual.update(|window, cx| window.focused(cx).unwrap());
        visual.simulate_keystrokes("cmd-t");
        visual.update(|window, _| assert!(focus.is_focused(window)));
        editor.read_with(&visual, |view, _| {
            assert!(view.last_enqueued_edit.is_none());
            assert_eq!(view.worker_requests_pending, 0);
        });
    }
}

#[gpui::test]
fn empty_track_fixture_catalog_job_owns_same_root_after_view_and_caller_drop(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering;
    let directory = TrackTestDirectory::new();
    let root = directory.0.root().to_owned();
    let weak = Arc::downgrade(&directory.0);
    let gate = directory.0.gate_next_catalog_save();
    let (project, _, _, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, &directory.0);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        gate.reached.load(Ordering::Acquire),
        "actual catalog save reached"
    );
    assert!(!gate.finished.load(Ordering::Acquire));
    let paths = editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&directory.0, &view.test_environment));
        assert_eq!(view.browser.directory, root.join("samples"));
        [
            view.browser.test_preferences_path().to_owned(),
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
    assert!(!database.exists(), "held before real first catalog save");
    let editor_weak = editor.downgrade();
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| gpui::Empty);
    });
    drop(editor);
    drop(directory);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(editor_weak.upgrade().is_none());
    assert_eq!(
        weak.strong_count(),
        1,
        "only actual held catalog job owns root"
    );
    assert!(root.join("samples").is_dir());
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
    let until = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(std::time::Instant::now() < until, "owned root cleanup");
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(
        !root.exists(),
        "completed jobs cannot recreate retired root"
    );
    eprintln!(
        "empty_track_fixture SAME root, four preferences, actual catalog save, retired view/caller and final cleanup verified"
    );
}
