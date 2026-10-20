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
fn command_t_uses_existing_add_track_and_one_canonical_undo(cx: &mut TestAppContext) {
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
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
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

fn instrument_enter_worker_window(
    cx: &mut TestAppContext,
    directory: &Arc<Environment>,
) -> (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Arc<hex_project::Project>,
) {
    use hex_session::service::{Request, Service};
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    let next = || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(event) = service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "instrument worker event"
            );
            std::thread::yield_now();
        }
    };
    next();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "existing".into(),
        }))
        .unwrap();
    let state = next().state;
    let original = Arc::clone(&state.project);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(directory),
                )
            });
            editor.update(cx, |view, cx| {
                view.selected_track = Some(state.project.tracks()[0].id());
                view.state = Some(state);
                view.browser.section = super::browser::BrowserSection::Instruments;
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, original)
}

fn pump_instrument_enter(
    editor: &gpui::Entity<EditorView>,
    visual: &mut VisualTestContext,
    tracks: usize,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        editor.update(visual, |view, cx| {
            view.tick(cx);
            cx.notify();
        });
        visual.run_until_parked();
        if editor.read_with(visual, |view, _| {
            view.worker_requests_pending == 0
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| state.project.tracks().len() == tracks)
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "instrument Enter actual worker completion"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[gpui::test]
fn browser_instrument_enter_uses_selected_registered_state_and_single_worker_undo(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let directory = TrackTestDirectory::new();
    let (handle, editor, original) = instrument_enter_worker_window(cx, &directory.0);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for kind in ["hex.instrument.fm4", "hex.instrument.sine"] {
        visual.run_until_parked();
        let selector = Box::leak(format!("browser-device-instrument-{kind}").into_boxed_str());
        let bounds = visual
            .debug_bounds(selector)
            .expect("actual registered instrument row");
        let point = gpui::point(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        visual.simulate_mouse_down(point, gpui::MouseButton::Left, gpui::Modifiers::default());
        visual.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
        visual.simulate_keystrokes("enter");
        let expected = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::new(kind).unwrap(),
        )
        .unwrap();
        editor.read_with(&visual, |view, _| {
            match view
                .last_enqueued_edit
                .as_ref()
                .expect("actual key dispatched edit")
            {
                Command::InsertTrackWithDevice {
                    before_track,
                    slot,
                    state,
                    ..
                } => {
                    assert_eq!(*before_track, None);
                    assert_eq!(*slot, hex_project::TrackDeviceSlot::Instrument);
                    assert_eq!(state, &expected);
                }
                other => panic!("selected instrument must be explicit, got {other:?}"),
            }
        });
        pump_instrument_enter(&editor, &mut visual, 2);
        let after = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(project.tracks()[0], original.tracks()[0]);
            let added = &project.tracks()[1];
            assert_eq!(added.name(), "track 2");
            assert_eq!(added.ordered_device_chain().count(), 1);
            assert_eq!(added.instrument().unwrap().state(), &expected);
            assert_eq!(
                view.selected_track,
                Some(added.id()),
                "retain existing created-track selection policy"
            );
            Arc::clone(project)
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(hex_session::service::Request::Undo))
        });
        pump_instrument_enter(&editor, &mut visual, 1);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.tracks(),
                original.tracks()
            )
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(hex_session::service::Request::Redo))
        });
        pump_instrument_enter(&editor, &mut visual, 2);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.as_ref(),
                after.as_ref()
            )
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(hex_session::service::Request::Undo))
        });
        pump_instrument_enter(&editor, &mut visual, 1);
    }
}

#[gpui::test]
fn browser_instrument_enter_refuses_absent_stale_and_invalid_selection_without_queue(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let directory = TrackTestDirectory::new();
    let (handle, editor, original) = instrument_enter_worker_window(cx, &directory.0);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for case in 0..6 {
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                let class = hex_engine::NativeDeviceClass::Instrument;
                let mut entry = hex_engine::native_device_catalog_for_class(class)
                    .unwrap()
                    .into_iter()
                    .next()
                    .unwrap();
                let mut selected_class = class;
                view.browser.results_focused = true;
                view.browser.selected_index = 0;
                view.last_enqueued_edit = None;
                view.error = None;
                match case {
                    0 => view.browser.results_focused = false,
                    1 => view.browser.selected_index = usize::MAX,
                    2 => {}
                    3 => selected_class = hex_engine::NativeDeviceClass::Effect,
                    4 => {
                        entry.type_id =
                            hex_lattice_contract::DeviceTypeId::from_static("missing.instrument")
                    }
                    5 => {
                        entry.type_id =
                            hex_lattice_contract::DeviceTypeId::from_static("hex.utility")
                    }
                    _ => unreachable!(),
                }
                view.browser.device_catalog = Some(if case == 2 {
                    Err("catalog unavailable".into())
                } else {
                    Ok(vec![(selected_class, entry)])
                });
                view.browser.results_focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        visual.simulate_keystrokes("enter");
        editor.read_with(&visual, |view, _| {
            assert!(
                view.last_enqueued_edit.is_none(),
                "case{case} must refuse before queue"
            );
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(
                view.state.as_ref().unwrap().project.as_ref(),
                original.as_ref()
            );
        });
        visual.run_until_parked();
    }
}
