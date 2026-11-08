use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{AppContext, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_engine::NativeDeviceClass;
use hex_session::service::ArrangementDeviceDestination;
use std::sync::Arc;

#[gpui::test]
fn arrangement_catalog_pointer_drop_captures_target_and_preview_without_selection_change(
    cx: &mut TestAppContext,
) {
    for class in [NativeDeviceClass::Instrument, NativeDeviceClass::Effect] {
        for destination in 0..5 {
            let (project, first, target, _) = audio_project();
            let project = if destination == 2 {
                hex_project::Project::new(48_000).unwrap()
            } else {
                project
            };
            let original = project.clone();
            let (handle, editor) = editor_window(cx, project, Arc::default());
            editor.update(cx, |view, cx| {
                view.browser.section = match class {
                    NativeDeviceClass::Instrument => super::browser::BrowserSection::Instruments,
                    NativeDeviceClass::Effect => super::browser::BrowserSection::Instruments,
                };
                view.selected_track = if destination == 2 { None } else { Some(first) };
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let source = visual
                .debug_bounds(match class {
                    NativeDeviceClass::Instrument => "browser-device-instrument-hex.instrument.fm4",
                    NativeDeviceClass::Effect => "browser-device-effect-hex.utility",
                })
                .expect("catalog source");
            let down = point(
                source.origin.x + source.size.width * 0.5,
                source.origin.y + source.size.height * 0.5,
            );
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(
                point(down.x + px(12.0), down.y),
                MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
            let body = visual.debug_bounds("arrangement-content").expect("body");
            let landing = if destination == 2 {
                point(
                    body.origin.x + body.size.width * 0.4,
                    body.origin.y + body.size.height * 0.5,
                )
            } else {
                let lane_track = if destination == 3 { first } else { target };
                let lane = visual
                    .debug_bounds(Box::leak(
                        format!("arrangement-lane-{}", lane_track.value()).into_boxed_str(),
                    ))
                    .expect("target lane");
                point(
                    lane.origin.x + lane.size.width * 0.5,
                    match destination {
                        1 => lane.origin.y,
                        3 => lane.origin.y + px(1.0),
                        4 => lane.origin.y + lane.size.height + px(10.0),
                        _ => lane.origin.y + lane.size.height * 0.5,
                    },
                )
            };
            visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("arrangement-device-drop-preview")
                    .is_some()
            );
            editor.read_with(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
                assert!(view.last_enqueued_arrangement_device_drop.is_none());
            });
            visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
            editor.read_with(cx, |view, _| {
                let expected = match destination {
                    0 => ArrangementDeviceDestination::ExistingTrack {
                        track: target,
                        instrument: original
                            .track(target)
                            .unwrap()
                            .instrument()
                            .map(hex_project::Device::id),
                    },
                    1 => ArrangementDeviceDestination::NewTrack {
                        before_track: Some(target),
                    },
                    3 => ArrangementDeviceDestination::NewTrack {
                        before_track: Some(first),
                    },
                    _ => ArrangementDeviceDestination::NewTrack { before_track: None },
                };
                assert_eq!(view.last_enqueued_arrangement_device_drop, Some(expected));
                let expected_gain = if destination == 0 {
                    None
                } else {
                    Some(0.251_188_64)
                };
                assert_eq!(view.last_enqueued_device_track_gain, Some(expected_gain));
                assert_eq!(
                    view.selected_track,
                    if destination == 2 { None } else { Some(first) }
                );
                assert!(view.arrangement_device_preview.is_none());
            });
        }
    }
}

#[gpui::test]
fn arrangement_catalog_pointer_refuses_stale_busy_main_and_escape_without_edits(
    cx: &mut TestAppContext,
) {
    for refusal in 0..8 {
        let (project, first, target, _) = audio_project();
        let original = project.clone();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        editor.update(cx, |view, cx| {
            view.browser.section = super::browser::BrowserSection::Instruments;
            view.selected_track = Some(first);
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let source = visual
            .debug_bounds("browser-device-instrument-hex.instrument.fm4")
            .expect("catalog");
        let down = point(
            source.origin.x + source.size.width * 0.5,
            source.origin.y + source.size.height * 0.5,
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(if refusal == 0 {
                "main-track-lane"
            } else {
                Box::leak(format!("arrangement-lane-{}", target.value()).into_boxed_str())
            })
            .expect("landing");
        let landing = point(
            lane.origin.x + lane.size.width * 0.5,
            lane.origin.y + lane.size.height * 0.5,
        );
        visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if refusal == 5 {
            visual.simulate_keystrokes("escape");
        } else {
            editor.update(cx, |view, _| match refusal {
                1 => view.drag_layout_generation = view.drag_layout_generation.wrapping_add(1),
                2 => view.project_generation += 1,
                3 => {
                    let state = view.state.as_ref().unwrap();
                    view.state = Some(Arc::new(hex_session::service::ViewState {
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: state.project.clone(),
                        waveforms: state.waveforms.clone(),
                        source_overviews: state.source_overviews.clone(),
                        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
                        gain_preview: state.gain_preview.clone(),
                        midi_preview: state.midi_preview.clone(),
                        timeline_frames: state.timeline_frames,
                        can_undo: state.can_undo,
                        can_redo: state.can_redo,
                        revision: state.revision + 1,
                        unsaved: state.unsaved,
                        document_path: state.document_path.clone(),
                    }));
                }
                4 => view.worker_requests_pending = 1,
                6 => view.opening_project = true,
                _ => {}
            });
        }
        let release = if refusal == 7 {
            visual.simulate_mouse_move(down, MouseButton::Left, Modifiers::default());
            down
        } else {
            landing
        };
        visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(
                view.last_enqueued_arrangement_device_drop.is_none(),
                "refusal {refusal}"
            );
            assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
            assert_eq!(view.selected_track, Some(first));
            assert!(view.arrangement_device_preview.is_none());
            assert!(view.active_native_device_drag.is_none());
            if refusal != 5 && refusal != 7 {
                assert!(view.error.is_some(), "visible refusal {refusal}");
            }
        });
    }
}

#[gpui::test]
fn arrangement_catalog_worker_completion_and_undo_preserve_prior_selection(
    cx: &mut TestAppContext,
) {
    use hex_session::service::{Request, Service};
    fn next(service: &Service) -> hex_session::service::Event {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            if let Ok(event) = service.try_event() {
                return event;
            }
            assert!(std::time::Instant::now() < deadline, "worker event timeout");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn pump(editor: &gpui::Entity<super::EditorView>, cx: &mut TestAppContext, count: usize) {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
            if editor.read_with(cx, |view, _| {
                view.worker_requests_pending == 0
                    && view
                        .state
                        .as_ref()
                        .is_some_and(|state| state.project.tracks().len() == count)
            }) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker-backed drop/undo timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    let service = Service::new(48_000, 16, 128).unwrap();
    next(&service);
    for name in ["selected", "anchor"] {
        service
            .submit(Request::Edit(hex_project::Command::AddTrack {
                name: name.into(),
            }))
            .unwrap();
        if name == "selected" {
            next(&service);
        }
    }
    let state = next(&service).state;
    let original = state.project.clone();
    let selected = original.tracks()[0].id();
    let target = original.tracks()[1].id();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                super::EditorView::new(window, cx, service, Err("offline ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.selected_track = Some(selected);
                view.mode = super::EditorMode::Arrangement;
                view.browser.section = super::browser::BrowserSection::Instruments;
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .unwrap();
    let down = point(
        source.origin.x + source.size.width * 0.5,
        source.origin.y + source.size.height * 0.5,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .unwrap();
    let landing = point(lane.origin.x + lane.size.width * 0.5, lane.origin.y);
    visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    pump(&editor, cx, 3);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.tracks()[0], original.tracks()[0]);
        assert_eq!(project.tracks()[2], original.tracks()[1]);
        assert_eq!(project.tracks()[1].devices().len(), 1);
        assert_eq!(view.selected_track, Some(selected));
        assert!(view.error.is_none(), "{:?}", view.error);
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Undo));
    });
    pump(&editor, cx, 2);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.tracks(),
            original.tracks()
        );
        assert_eq!(view.selected_track, Some(selected));
    });
}
