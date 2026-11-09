//! Actual instrument browser key dispatch and canonical Service completion.
use super::*;
use crate::editor::test_environment::Environment;

fn instrument_environment() -> Arc<Environment> {
    let environment = Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    environment
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
        let deadline = crate::test_wait::hang_deadline();
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
                view.browser.section = BrowserSection::Instruments;
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
    let deadline = crate::test_wait::hang_deadline();
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
    cx.update(crate::editor::bind_editor_keys);
    let directory = instrument_environment();
    hex_preferences::save_track_defaults(
        &directory.root().join("config/track-defaults.json"),
        hex_preferences::TrackDefaults::new(0.5).unwrap(),
    )
    .unwrap();
    let (handle, editor, original) = instrument_enter_worker_window(cx, &directory);
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
            assert!(
                view.last_enqueued_edit.is_none(),
                "UI must send identity, not construct a device edit"
            );
            assert_eq!(
                view.worker_requests_pending, 1,
                "actual key queued one typed worker request"
            );
        });
        pump_instrument_enter(&editor, &mut visual, 2);
        let after = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(project.tracks()[0], original.tracks()[0]);
            let added = &project.tracks()[1];
            assert_eq!(added.name(), kind.trim_start_matches("hex.instrument."));
            assert!(added.name_is_automatic());
            assert_eq!(added.gain(), 0.5);
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
    cx.update(crate::editor::bind_editor_keys);
    let directory = instrument_environment();
    let (handle, editor, original) = instrument_enter_worker_window(cx, &directory);
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
