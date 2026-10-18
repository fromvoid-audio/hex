//! Native action dispatch and canonical history for track keyboard shortcuts.
use super::EditorView;
use super::clip_drag_ui_test::audio_project;
use gpui::AppContext;
use gpui::{TestAppContext, VisualTestContext};
use hex_project::{Command, Created, Editor};
use std::sync::Arc;

// Only this private fixture: bound all discovery before browser workers start.
struct TrackTestDirectory(std::path::PathBuf);
impl TrackTestDirectory {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "hex-empty-track-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("samples")).unwrap();
        Self(root)
    }
}
impl Drop for TrackTestDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("owned empty-track fixture only");
    }
}
fn publish(view: &mut EditorView, project: hex_project::Project) {
    view.state = Some(Arc::new(hex_session::service::ViewState {
        canonical_audio_source: None,
        project: Arc::new(project),
        waveforms: Default::default(),
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
    directory: &std::path::Path,
) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |view, cx| {
                publish(view, project);
                assert_eq!(view.browser.directory, directory.join("samples"));
                assert!(view.audio.is_none());
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor)
}

#[gpui::test]
fn command_t_uses_existing_add_track_and_one_canonical_undo(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, _, _, _) = audio_project();
    let original = project.clone();
    let directory = TrackTestDirectory::new();
    let (handle, editor) = editor_window(cx, project, &directory.0);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-t");
    let command = editor.read_with(&visual, |view, _| {
        assert!(
            matches!(&view.last_enqueued_edit, Some(Command::AddTrack {name}) if name == "audio 3")
        );
        view.last_enqueued_edit.clone().unwrap()
    });
    let mut canonical = Editor::new(original.clone(), 8).unwrap();
    let Created::Track(id) = canonical.execute(command).unwrap() else {
        panic!("track result")
    };
    assert_eq!(canonical.project().tracks().len(), 3);
    assert_eq!(canonical.project().track(id).unwrap().name(), "audio 3");
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
    assert_eq!(canonical.project().track(id).unwrap().name(), "audio 3");
}

#[gpui::test]
fn command_t_preserves_browser_and_clip_name_text_focus(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for clip_name in [false, true] {
        let (project, _, _, clip) = audio_project();
        let directory = TrackTestDirectory::new();
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
