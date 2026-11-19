//! Track-header destinations are independent of the selected device-panel owner.
use super::*;
use crate::editor::{EditorMode, browser::BrowserSection, test_environment::Environment};

fn idle(view: &EditorView) -> bool {
    view.worker_requests_pending == 0 && view.pending_requests.is_empty()
}
fn header_window(
    cx: &mut TestAppContext,
    service: Service,
    state: Arc<ViewState>,
    selected: TrackId,
    mode: EditorMode,
) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
    header_window_in(
        cx,
        service,
        state,
        selected,
        mode,
        Environment::new().unwrap(),
    )
}
fn header_window_in(
    cx: &mut TestAppContext,
    service: Service,
    state: Arc<ViewState>,
    selected: TrackId,
    mode: EditorMode,
    environment: Arc<Environment>,
) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(window, cx, service, environment)
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.selected_track = Some(selected);
                view.mode = mode;
                view.browser.section = BrowserSection::Instruments;
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor)
}
fn begin_device_drag(visual: &mut VisualTestContext, source: &'static str) {
    let source = visual.debug_bounds(source).expect("drag source").center();
    visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(source.x + px(12.), source.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
}

fn drag_to_header(
    visual: &mut VisualTestContext,
    target: TrackId,
    mode: EditorMode,
    source: &'static str,
) -> gpui::Background {
    begin_device_drag(visual, source);
    let prefix = if mode == EditorMode::Arrangement {
        "arrangement-track-header"
    } else {
        "session-track"
    };
    let header = visual
        .debug_bounds(Box::leak(
            format!("{prefix}-{}", target.value()).into_boxed_str(),
        ))
        .expect("target header");
    let landing = header.center();
    let before = visual.rendered_background(Box::leak(
        format!("{prefix}-{}", target.value()).into_boxed_str(),
    ));
    visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let selector = Box::leak(format!("{prefix}-{}", target.value()).into_boxed_str());
    assert_eq!(
        visual.debug_bounds(selector).unwrap(),
        header,
        "drag feedback keeps hit geometry"
    );
    assert_ne!(
        visual.rendered_background(selector),
        before,
        "header gives drag feedback"
    );
    let feedback = visual
        .rendered_background(selector)
        .expect("drag feedback paint");
    visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
    feedback
}

#[gpui::test]
fn header_effect_drop_appends_to_unselected_track_once_and_keeps_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        let (service, _, selected, _, _, _) = actor_utility_fixture();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "header target".into(),
            }))
            .unwrap();
        let event = receive_service_event(&service);
        let Created::Track(target) = event.created.unwrap() else {
            panic!("track")
        };
        let original = event.state.project.clone();
        let mut expected = Editor::new((*original).clone(), 8).unwrap();
        expected
            .execute(Command::InsertDevice {
                track: target,
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap();
        let (handle, editor) = header_window(cx, service, event.state, selected, mode);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1600.), px(700.)));
        visual.run_until_parked();
        drag_to_header(
            &mut visual,
            target,
            mode,
            "browser-device-effect-hex.utility",
        );
        editor.read_with(cx, |view, _| {
            let request = view
                .last_enqueued_native_device_drop
                .as_ref()
                .expect("header dispatch");
            assert_eq!(request.track, target);
            assert_eq!(request.before, None);
            assert_eq!(request.replace, None);
            assert_eq!(view.selected_track, Some(selected));
        });
        pump_until(&editor, cx, idle);
        let changed = editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(project.as_ref(), expected.project());
            assert_eq!(project.track(target).unwrap().devices().len(), 1);
            assert_eq!(project.track(selected), original.track(selected));
            assert_eq!(project.tracks().len(), original.tracks().len());
            assert_eq!(view.selected_track, Some(selected));
            project.clone()
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
        pump_until(&editor, cx, idle);
        assert!(expected.undo());
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.as_ref(),
                expected.project()
            )
        });
        editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
        pump_until(&editor, cx, idle);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, changed)
        });
    }
}

#[gpui::test]
fn header_instrument_drop_inserts_or_replaces_and_preserves_effects_and_history(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        let mut accepted_feedback = None;
        for chain in 0..3 {
            let (service, _, selected, _, _, _) = actor_utility_fixture();
            service
                .submit(Request::Edit(Command::AddTrack {
                    name: "instrument target".into(),
                }))
                .unwrap();
            let event = receive_service_event(&service);
            let Created::Track(empty) = event.created.unwrap() else {
                panic!("track")
            };
            let target = if chain == 1 { selected } else { empty };
            if chain == 2 {
                service
                    .submit(Request::AddNativeDevice {
                        track: target,
                        class: NativeDeviceClass::Effect,
                        type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                            "hex.utility",
                        )),
                        before: None,
                    })
                    .unwrap();
                receive_service_event(&service);
            }
            service
                .submit(Request::Edit(Command::PlaceMidiClip {
                    track: target,
                    start: hex_project::BeatTime::ONE,
                    length: hex_project::BeatTime::from_quarters(4).unwrap(),
                }))
                .unwrap();
            let state = receive_service_event(&service).state;
            let original = state.project.clone();
            let original_track = original.track(target).unwrap();
            let old_instrument = original_track.instrument().map(|device| device.id());
            let (handle, editor) = header_window(cx, service, state, selected, mode);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.simulate_resize(size(px(1600.), px(700.)));
            visual.run_until_parked();
            let feedback = drag_to_header(
                &mut visual,
                target,
                mode,
                "browser-device-instrument-hex.instrument.fm4",
            );
            if let Some(accepted) = accepted_feedback.as_ref() {
                assert_eq!(
                    &feedback, accepted,
                    "each valid target has accepted feedback"
                );
            } else {
                accepted_feedback = Some(feedback);
            }
            editor.read_with(cx, |view, _| {
                assert!(view.last_enqueued_native_device_drop.is_some())
            });
            pump_until(&editor, cx, idle);
            let changed = editor.read_with(cx, |view, _| {
                let project = &view.state.as_ref().unwrap().project;
                let track = project.track(target).unwrap();
                let instrument = track.instrument().unwrap();
                assert_eq!(
                    instrument.state().type_id().as_str().unwrap(),
                    "hex.instrument.fm4"
                );
                if let Some(old_id) = old_instrument {
                    assert_eq!(instrument.id(), old_id);
                }
                assert_eq!(track.devices(), original_track.devices());
                assert_eq!(track.midi_clips(), original_track.midi_clips());
                assert!(matches!(
                    track.ordered_device_chain().next(),
                    Some(hex_project::TrackDeviceRef::Instrument(_))
                ));
                if target != selected {
                    assert_eq!(project.track(selected), original.track(selected));
                }
                assert_eq!(view.selected_track, Some(selected));
                assert!(view.error.is_none(), "{:?}", view.error);
                assert!(view.active_native_device_drag.is_none());
                project.clone()
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
            pump_until(&editor, cx, idle);
            editor.read_with(cx, |view, _| {
                let project = &view.state.as_ref().unwrap().project;
                assert_eq!(project.track(target), original.track(target));
                assert_eq!(project.track(selected), original.track(selected));
            });
            editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
            pump_until(&editor, cx, idle);
            editor.read_with(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, changed)
            });
        }
    }
}

#[gpui::test]
fn header_effect_append_preserves_existing_instrument_effect_order_and_clip_selection(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, target, old_effect, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: target,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Clip(clip) = event.created.unwrap() else {
        panic!("clip")
    };
    let original = event.state.project.clone();
    let (handle, editor) = header_window(cx, service, event.state, target, EditorMode::Arrangement);
    editor.update(cx, |view, _| view.selected_clip = Some(clip));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.), px(700.)));
    visual.run_until_parked();
    drag_to_header(
        &mut visual,
        target,
        EditorMode::Arrangement,
        "browser-device-effect-hex.utility",
    );
    pump_until(&editor, cx, idle);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let track = project.track(target).unwrap();
        assert_eq!(track.devices().len(), 2);
        assert_eq!(track.devices()[0].id(), old_effect);
        assert_eq!(
            &track.devices()[0],
            &original.track(target).unwrap().devices()[0]
        );
        assert_ne!(track.devices()[1].id(), old_effect);
        assert_eq!(
            track.instrument(),
            original.track(target).unwrap().instrument()
        );
        assert_eq!(
            track.midi_clips(),
            original.track(target).unwrap().midi_clips()
        );
        assert_eq!(view.selected_clip, Some(clip));
        assert!(view.last_enqueued_arrangement_device_drop.is_none());
    });
}

#[gpui::test]
fn header_drop_refuses_busy_stale_and_escape_without_changing_target_or_selection(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        for refusal in 0..3 {
            let (service, _, selected, _, _, _) = actor_utility_fixture();
            service
                .submit(Request::Edit(Command::AddTrack {
                    name: "refused target".into(),
                }))
                .unwrap();
            let event = receive_service_event(&service);
            let Created::Track(target) = event.created.unwrap() else {
                panic!("track")
            };
            let original = event.state.project.clone();
            let (handle, editor) = header_window(cx, service, event.state, selected, mode);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.simulate_resize(size(px(1600.), px(700.)));
            visual.run_until_parked();
            begin_device_drag(&mut visual, "browser-device-effect-hex.utility");
            let prefix = if mode == EditorMode::Arrangement {
                "arrangement-track-header"
            } else {
                "session-track"
            };
            let landing = visual
                .debug_bounds(Box::leak(
                    format!("{prefix}-{}", target.value()).into_boxed_str(),
                ))
                .unwrap()
                .center();
            if refusal == 2 {
                visual.simulate_keystrokes("escape");
            } else {
                editor.update(cx, |view, _| {
                    if refusal == 0 {
                        view.worker_requests_pending = 1;
                    } else {
                        view.project_generation += 1;
                    }
                });
            }
            visual.simulate_mouse_move(landing, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            visual.simulate_mouse_up(landing, MouseButton::Left, Modifiers::default());
            editor.update(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, original);
                assert_eq!(view.selected_track, Some(selected));
                assert!(view.last_enqueued_native_device_drop.is_none());
                assert!(view.last_enqueued_arrangement_device_drop.is_none());
                assert!(view.pending_requests.is_empty());
                assert!(view.active_native_device_drag.is_none());
                if refusal == 0 {
                    view.worker_requests_pending = 0;
                }
            });
        }
    }
}

#[gpui::test]
fn header_effect_drop_on_real_imported_audio_keeps_media_and_clip_state(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, selected, _, _, _) = actor_utility_fixture();
    let environment = Environment::new().unwrap();
    let path = environment.root().join("header-target.wav");
    let count = 128_u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + count * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&48_000_u32.to_le_bytes());
    wav.extend_from_slice(&96_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(count * 2).to_le_bytes());
    for _ in 0..count {
        wav.extend_from_slice(&4096_i16.to_le_bytes());
    }
    std::fs::write(&path, &wav).unwrap();
    service.submit(Request::Import(path.clone())).unwrap();
    let event = receive_service_event(&service);
    assert!(event.error.is_none(), "{:?}", event.error);
    let original = event.state.project.clone();
    let (handle, editor) = header_window_in(
        cx,
        service,
        event.state,
        selected,
        EditorMode::Arrangement,
        environment,
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.), px(700.)));
    let target = original.tracks().last().unwrap().id();
    assert_ne!(target, selected);
    assert_eq!(original.track(target).unwrap().clips().len(), 1);
    editor.update(cx, |view, cx| {
        view.selected_track = Some(selected);
        cx.notify();
    });
    visual.run_until_parked();
    drag_to_header(
        &mut visual,
        target,
        EditorMode::Arrangement,
        "browser-device-effect-hex.utility",
    );
    pump_until(&editor, cx, idle);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(target).unwrap().devices().len(), 1);
        assert_eq!(
            project.track(target).unwrap().clips(),
            original.track(target).unwrap().clips()
        );
        assert_eq!(project.track(selected), original.track(selected));
        assert_eq!(view.selected_track, Some(selected));
        assert!(view.last_enqueued_arrangement_device_drop.is_none());
    });
    assert_eq!(
        std::fs::read(path).unwrap(),
        wav,
        "drop preserves original media bytes"
    );
}

#[gpui::test]
fn header_drop_preserves_invalid_track_gain_draft_without_admitting_an_edit(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, _, selected, _, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "draft target".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(target) = event.created.unwrap() else {
        panic!("track")
    };
    let original = event.state.project.clone();
    let (handle, editor) =
        header_window(cx, service, event.state, selected, EditorMode::Arrangement);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.), px(700.)));
    visual.run_until_parked();
    let field = visual
        .debug_bounds(Box::leak(
            format!("track-volume-{}-value", selected.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.simulate_keystrokes("enter");
    let gain = editor.update(cx, |view, cx| {
        crate::editor::track_gain::controls_for_finalize(view, cx)
            .into_iter()
            .find(|(target, _)| *target == GestureTarget::TrackGain { track: selected })
            .unwrap()
            .1
    });
    assert!(gain.read_with(cx, |control, _| control.is_editing()));
    drag_to_header(
        &mut visual,
        target,
        EditorMode::Arrangement,
        "browser-device-effect-hex.utility",
    );
    editor.read_with(cx, |view, _| {
        assert!(
            view.last_enqueued_native_device_drop.is_none(),
            "invalid gain draft must block header insertion"
        );
        assert!(idle(view));
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.selected_track, Some(selected));
    });
    gain.read_with(cx, |control, _| {
        assert!(control.is_editing());
        assert_eq!(control.display_text(), "invalid");
    });
}

#[gpui::test]
fn plain_drag_moves_a_device_to_the_end_of_another_track_header(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        for move_instrument in [false, true] {
            let (service, initial, source, effect, _, _) = actor_utility_fixture();
            let device = if move_instrument {
                initial
                    .project
                    .track(source)
                    .unwrap()
                    .instrument()
                    .unwrap()
                    .id()
            } else {
                effect
            };
            service
                .submit(Request::Edit(Command::AddTrack {
                    name: "destination".into(),
                }))
                .unwrap();
            let Created::Track(target) = receive_service_event(&service).created.unwrap() else {
                panic!("destination track");
            };
            service
                .submit(Request::AddNativeDevice {
                    track: target,
                    class: NativeDeviceClass::Effect,
                    type_id: Box::new(hex_devices::utility::UTILITY_TYPE_ID),
                    before: None,
                })
                .unwrap();
            let state = receive_service_event(&service).state;
            let original = state.project.clone();
            let first = original.track(target).unwrap().devices()[0].id();
            let (handle, editor) = header_window(cx, service, state, source, mode);
            editor.update(cx, |view, cx| {
                view.device_panel_active = true;
                view.clip_view_open = true;
                cx.notify();
            });
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.simulate_resize(size(px(1600.), px(800.)));
            visual.run_until_parked();
            drag_to_header(
                &mut visual,
                target,
                mode,
                Box::leak(format!("device-drag-title-{}", device.value()).into_boxed_str()),
            );
            pump_until(&editor, cx, idle);
            let changed = editor.read_with(cx, |view, _| {
                assert!(view.error.is_none(), "{:?}", view.error);
                let project = &view.state.as_ref().unwrap().project;
                let ids = |track: TrackId| {
                    project
                        .track(track)
                        .unwrap()
                        .ordered_device_chain()
                        .map(|entry| match entry {
                            hex_project::TrackDeviceRef::Effect(device)
                            | hex_project::TrackDeviceRef::Instrument(device) => device.id(),
                        })
                        .collect::<Vec<_>>()
                };
                assert_eq!(ids(target), [first, device]);
                assert!(!ids(source).contains(&device));
                assert_eq!(project.device(device), original.device(device));
                assert_eq!(view.selected_track, Some(source));
                project.clone()
            });
            for (request, expected) in [(Request::Undo, original), (Request::Redo, changed)] {
                editor.update(cx, |view, _| assert!(view.enqueue(request)));
                pump_until(&editor, cx, idle);
                editor.read_with(cx, |view, _| {
                    assert_eq!(view.state.as_ref().unwrap().project, expected);
                });
            }
        }
    }
}

#[gpui::test]
fn header_drag_moves_noncontiguous_selected_devices_in_chain_order(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    for mode in [EditorMode::Arrangement, EditorMode::Session] {
        let (service, _, source, first, _, _) = actor_utility_fixture();
        let add_effect = |track| {
            service
                .submit(Request::AddNativeDevice {
                    track,
                    class: NativeDeviceClass::Effect,
                    type_id: Box::new(hex_devices::utility::UTILITY_TYPE_ID),
                    before: None,
                })
                .unwrap();
            receive_service_event(&service).state
        };
        let middle = add_effect(source).project.track(source).unwrap().devices()[1].id();
        let last = add_effect(source).project.track(source).unwrap().devices()[2].id();
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "destination".into(),
            }))
            .unwrap();
        let Created::Track(target) = receive_service_event(&service).created.unwrap() else {
            panic!("destination track");
        };
        let state = add_effect(target);
        let original = state.project.clone();
        let existing = original.track(target).unwrap().devices()[0].id();
        let (handle, editor) = header_window(cx, service, state, source, mode);
        editor.update(cx, |view, cx| {
            view.device_panel_active = true;
            view.clip_view_open = true;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(1800.), px(800.)));
        visual.run_until_parked();
        let title = |device: DeviceId| -> &'static str {
            Box::leak(format!("device-drag-title-{}", device.value()).into_boxed_str())
        };
        let last_title = visual.debug_bounds(title(last)).unwrap().center();
        let first_title = visual.debug_bounds(title(first)).unwrap().center();
        visual.simulate_click(last_title, Modifiers::default());
        visual.simulate_click(
            first_title,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        editor.read_with(cx, |view, _| {
            assert!(view.device_panel.is_selected(first) && view.device_panel.is_selected(last));
            assert!(!view.device_panel.is_selected(middle));
        });
        drag_to_header(&mut visual, target, mode, title(last));
        pump_until(&editor, cx, idle);
        let moved = editor.read_with(cx, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            let project = &view.state.as_ref().unwrap().project;
            let effects = |track| {
                project
                    .track(track)
                    .unwrap()
                    .devices()
                    .iter()
                    .map(hex_project::Device::id)
                    .collect::<Vec<_>>()
            };
            assert_eq!(effects(source), [middle]);
            assert_eq!(effects(target), [existing, first, last]);
            for id in [existing, first, last, middle] {
                assert_eq!(project.device(id), original.device(id));
            }
            project.clone()
        });
        for (request, expected) in [(Request::Undo, original), (Request::Redo, moved)] {
            editor.update(cx, |view, _| assert!(view.enqueue(request)));
            pump_until(&editor, cx, idle);
            editor.read_with(cx, |view, _| {
                assert_eq!(view.state.as_ref().unwrap().project, expected);
            });
        }
    }
}

#[gpui::test]
fn option_drag_copies_device_to_another_track_header(cx: &mut TestAppContext) {
    let (service, _, source, device, _, _) = actor_utility_fixture();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "destination".into(),
        }))
        .unwrap();
    let event = receive_service_event(&service);
    let Created::Track(target) = event.created.unwrap() else {
        panic!("target");
    };
    let (handle, editor) = actor_editor_window(cx, service, event.state, source);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.0), px(800.0)));
    visual.run_until_parked();
    let title = visual
        .debug_bounds(Box::leak(
            format!("device-drag-title-{}", device.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let header = visual
        .debug_bounds(Box::leak(
            format!("session-track-{}", target.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let option = Modifiers {
        alt: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(title, MouseButton::Left, option);
    visual.simulate_mouse_move(
        point(title.x + px(12.0), title.y),
        MouseButton::Left,
        option,
    );
    visual.simulate_mouse_move(header, MouseButton::Left, option);
    visual.run_until_parked();
    visual.simulate_mouse_up(header, MouseButton::Left, option);
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(target)
            .unwrap()
            .devices()
            .len()
            == 1
    });
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let copy = &project.track(target).unwrap().devices()[0];
        assert_ne!(copy.id(), device);
        assert_eq!(copy.state(), project.device(device).unwrap().state());
        assert_eq!(view.selected_track, Some(source));
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(target)
            .unwrap()
            .devices()
            .is_empty()
    });
}

#[gpui::test]
fn browser_device_double_click_appends_once_and_preserves_history(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = header_window(cx, service, state, track, EditorMode::Arrangement);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.), px(700.)));
    visual.run_until_parked();
    let at = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .unwrap()
        .center();
    for count in [1, 2] {
        visual.simulate_event(gpui::MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            click_count: count,
            first_mouse: false,
            modifiers: Modifiers::default(),
        });
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    }
    pump_until(&editor, cx, idle);
    let changed = editor.read_with(cx, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(track).unwrap().devices().len(), 2);
        assert_eq!(
            project.track(track).unwrap().devices()[0],
            original.track(track).unwrap().devices()[0]
        );
        project.clone()
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, idle);
    editor.read_with(cx, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.track(track), original.track(track));
        assert_eq!(project.tracks().len(), original.tracks().len());
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, idle);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project, changed)
    });
}

#[gpui::test]
fn browser_device_double_click_without_selected_track_is_refused(cx: &mut TestAppContext) {
    let (service, state, track, _, _, _) = actor_utility_fixture();
    let original = state.project.clone();
    let (handle, editor) = header_window(cx, service, state, track, EditorMode::Arrangement);
    editor.update(cx, |view, _| view.selected_track = None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(1600.), px(700.)));
    visual.run_until_parked();
    let at = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .unwrap()
        .center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
        modifiers: Modifiers::default(),
    });
    editor.read_with(cx, |view, _| {
        assert!(view.error.is_some());
        assert_eq!(view.state.as_ref().unwrap().project, original);
        assert_eq!(view.worker_requests_pending, 0);
    });
}
