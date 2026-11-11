//! Actual pointer, keyboard and canonical worker diagnostics; no native-output claim.
use super::*;
use crate::editor::test_environment::Environment;

type Fixture = (
    gpui::AnyWindowHandle,
    gpui::Entity<EditorView>,
    Arc<Environment>,
    DeviceId,
);

fn held_node_window(cx: &mut TestAppContext) -> Fixture {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
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
    pump_until(&editor, cx, |view| view.state.is_some() && settled(view));
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "held eq".into()
        })))
    });
    pump_until(&editor, cx, |view| {
        settled(view) && view.state.as_ref().unwrap().project.tracks().len() == 1
    });
    let track = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::AddNativeDevice {
            track,
            class: NativeDeviceClass::Effect,
            type_id: Box::new(EQ_TYPE_ID),
            before: None,
        }))
    });
    pump_until(&editor, cx, |view| {
        settled(view)
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .devices()
                .is_empty()
    });
    let device = editor.read_with(cx, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .devices()[0]
            .id()
    });
    editor.update(cx, |view, cx| {
        assert!(Arc::ptr_eq(&environment, &view.test_environment));
        view.selected_track = Some(track);
        view.device_panel_active = true;
        view.clip_view_open = false;
        view.mode = crate::editor::EditorMode::Arrangement;
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    visual.run_until_parked();
    let toggle = visual
        .debug_bounds("device-panel-bottom-toggle")
        .expect("actual detail-pane toggle");
    visual.simulate_click(toggle.center(), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.clip_view_open && view.device_panel_active)
    });
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_click(start, Modifiers::default());
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device).nodes.len() == 1
    });
    (handle, editor, environment, device)
}

fn graph(
    editor: &gpui::Entity<EditorView>,
    cx: &TestAppContext,
    device: DeviceId,
) -> gpui::Entity<hex_widgets::ResponseGraph> {
    editor.read_with(cx, |view, _| eq_panel::graph_for_test(view, device))
}

#[gpui::test]
fn eq_held_node_position_survives_real_preview_ack_and_repeated_completed_frames(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| read(view, device));
    let node = before.nodes[0].id;
    let graph = graph(&editor, cx, device);
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    for (x, y) in [(0.62, 0.72), (0.7, 0.6), (0.58, 0.8), (0.65, 0.67)] {
        let pointer = plot(&mut visual, device, x, y);
        visual.simulate_mouse_move(pointer, Some(MouseButton::Left), Modifiers::default());
        for frame in 0..12 {
            editor.update(cx, |view, cx| {
                view.tick(cx);
                cx.notify();
            });
            visual.run_until_parked();
            graph.read_with(cx, |graph, _| {
                assert!(graph.is_dragging(), "owner lost at held frame {frame}");
                let actual = graph.node_position(node.value()).unwrap();
                assert!(
                    (actual[0] - x).abs() < 0.0001 && (actual[1] - y).abs() < 0.0001,
                    "held rendered model reverted at frame {frame}: {actual:?}, expected {x},{y}"
                );
            });
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    read(view, device),
                    before,
                    "preview cannot commit canonical state"
                )
            });
        }
    }
    let end = plot(&mut visual, device, 0.65, 0.67);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    // Release capture separately from display custody. Advance the real keyed
    // worker stream, but hold its exact Commit reply across completed frames.
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        let terminal = editor.update(&mut visual, |view, cx| {
            view.pump_device_gesture();
            let terminal = if let Ok(event) = view.service.try_event() {
                if event.gesture_ack.as_ref().is_some_and(|ack| {
                    ack.phase == hex_session::device_gesture::GesturePhase::Commit
                }) {
                    Some(event)
                } else {
                    view.receive(event);
                    None
                }
            } else {
                None
            };
            cx.notify();
            terminal
        });
        visual.run_until_parked();
        graph.read_with(&visual, |graph, _| {
            assert!(!graph.is_dragging());
            let actual = graph.node_position(node.value()).unwrap();
            assert!(
                (actual[0] - 0.65).abs() < 0.0001 && (actual[1] - 0.67).abs() < 0.0001,
                "released node reverted before its exact Commit: {actual:?}"
            );
        });
        if let Some(terminal) = terminal {
            break terminal;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "eq_held_node_position_survives_real_preview_ack_and_repeated_completed_frames: hang guard expired"
        );
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none());
    for _ in 0..4 {
        editor.update(&mut visual, |view, cx| {
            assert_eq!(read(view, device), before);
            cx.notify();
        });
        visual.run_until_parked();
        graph.read_with(&visual, |graph, _| {
            let actual = graph.node_position(node.value()).unwrap();
            assert!((actual[0] - 0.65).abs() < 0.0001 && (actual[1] - 0.67).abs() < 0.0001);
        });
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    for _ in 0..4 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        graph.read_with(&visual, |graph, _| {
            let actual = graph.node_position(node.value()).unwrap();
            assert!((actual[0] - 0.65).abs() < 0.0001 && (actual[1] - 0.67).abs() < 0.0001);
        });
    }
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) != before
    });
    let after = editor.read_with(cx, |view, _| read(view, device));
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == after
    });
}

#[gpui::test]
fn eq_held_node_space_key_routes_transport_without_retiring_pointer_or_edit(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let graph = graph(&editor, cx, device);
    for moving in [false, true] {
        let start = plot(&mut visual, device, 0.5, 0.5);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let end = if moving {
            plot(&mut visual, device, 0.62, 0.7)
        } else {
            start
        };
        if moving {
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        }
        let key = editor.read_with(cx, |view, _| {
            assert!(view.last_transport_command.is_none());
            view.device_gesture.snapshot().map(|snapshot| snapshot.key)
        });
        visual.simulate_keystrokes("space");
        editor.read_with(cx, |view, _| {
            assert!(
                view.last_transport_command.is_some(),
                "actual Space binding did not reach transport while moving={moving}"
            );
            assert_eq!(
                view.device_gesture.snapshot().map(|snapshot| snapshot.key),
                key
            );
            assert!(
                view.audio.is_none(),
                "routing diagnostic only, no audible/native result"
            );
        });
        graph.read_with(cx, |graph, _| assert_eq!(graph.is_dragging(), moving));
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump_until(&editor, cx, settled);
        editor.update(cx, |view, _| view.last_transport_command = None);
    }
}

pub(in crate::editor) fn pump_output(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    output: &mut hex_audio::WorkerPreparedOutput,
    backend: &mut hex_audio::worker_fixture::HeadlessBackend,
) {
    editor.update(cx, |view, cx| {
        if view.tick(cx) {
            cx.notify();
        }
    });
    let control = output
        .publication
        .control()
        .expect("actual worker publication owner");
    control.reclaim();
    control.poll_resident_publication();
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.update(cx, |view, cx| {
        if view.tick(cx) {
            cx.notify();
        }
    });
    std::thread::yield_now();
}

pub(in crate::editor) fn attach_actual_output(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
) -> (
    hex_audio::WorkerPreparedOutput,
    hex_audio::worker_fixture::HeadlessBackend,
) {
    let (mut output, mut backend) = graph_layout_tests::headless_status_output();
    let client = output.take_client().unwrap();
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "actual guarded headless fixture".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        cx.notify();
    });
    let deadline = crate::test_wait::hang_deadline();
    loop {
        pump_output(editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| {
            view.audible_playback.is_some()
                && view.applied_project_revision() == Some(view.state.as_ref().unwrap().revision)
                && view.pending_playback.is_none()
                && view.pending_resident_publication.is_none()
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fixture never installed actual canonical output"
        );
    }
    (output, backend)
}

#[gpui::test]
fn eq_held_node_space_starts_applied_preview_without_seek_worker_or_release(
    cx: &mut TestAppContext,
) {
    use std::sync::atomic::Ordering::Acquire;
    let _output_guard = graph_layout_tests::status_fixture_guard();
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.current_seek_requires_preroll(),
            Some(true),
            "actual EQ path must use stateful seek"
        )
    });
    let graph = graph(&editor, cx, device);
    for moving in [false, true] {
        assert!(!backend.status().unwrap().playing.load(Acquire));
        let start = plot(&mut visual, device, 0.5, 0.5);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let end = if moving {
            plot(&mut visual, device, 0.62, 0.7)
        } else {
            start
        };
        if moving {
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        }
        // Admit the real preview before asking for transport, keeping the mouse held.
        let deadline = crate::test_wait::hang_deadline();
        pump_output(&editor, cx, &mut output, &mut backend);
        visual.run_until_parked();
        while editor.read_with(cx, |view, _| {
            let current_preview_applied = !moving
                || view.audible_playback.as_ref().is_some_and(|audible| {
                    audible.device_preview_identity.is_some_and(|identity| {
                        view.device_gesture.snapshot().is_some_and(|snapshot| {
                            identity.key == snapshot.key && identity.sequence == snapshot.sequence
                        }) && audible.audio_revision
                            == view
                                .audio
                                .as_ref()
                                .unwrap()
                                .control
                                .status()
                                .unwrap()
                                .applied_revision
                                .load(Acquire)
                    })
                });
            !current_preview_applied
                || view.worker_requests_pending != 0
                || !view.pending_requests.is_empty()
                || view.pending_playback.is_some()
                || view.pending_resident_publication.is_some()
                || !view.playback_candidates.is_empty()
        }) {
            pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "actual preview did not apply"
            );
        }
        let expected_source = editor.read_with(cx, |view, _| {
            EqState::decode(
                view.audible_playback
                    .as_ref()
                    .unwrap()
                    .state
                    .project
                    .device(device)
                    .unwrap()
                    .state(),
            )
            .unwrap()
        });
        let key = editor.read_with(cx, |view, _| {
            view.device_gesture.snapshot().map(|snapshot| snapshot.key)
        });
        visual.simulate_keystrokes("space");
        editor.read_with(cx, |view, _| {
            assert!(
                view.prepared_seek.is_none() && view.latched_seek_intent.is_none(),
                "Space must start the applied preview without a prepared seek worker"
            );
        });
        let deadline = crate::test_wait::hang_deadline();
        while !backend.status().unwrap().playing.load(Acquire) {
            pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            if std::time::Instant::now() >= deadline {
                let detail = editor.read_with(cx, |view, _| {
                    format!(
                        "error={:?} prepared={} latched={} key={:?}",
                        view.error,
                        view.prepared_seek.is_some(),
                        view.latched_seek_intent.is_some(),
                        view.device_gesture.snapshot().map(|snapshot| snapshot.key)
                    )
                });
                panic!("actual Space did not start callback while moving={moving}: {detail}");
            }
        }
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.device_gesture.snapshot().map(|snapshot| snapshot.key),
                key
            )
        });
        editor.read_with(cx, |view, _| {
            let audible = view.audible_playback.as_ref().unwrap();
            assert_eq!(
                EqState::decode(audible.state.project.device(device).unwrap().state()).unwrap(),
                expected_source,
                "seek must retain the actual preview source, not canonical parameter metadata"
            );
            assert_eq!(audible.device_preview_identity.is_some(), moving);
        });
        graph.read_with(cx, |graph, _| assert_eq!(graph.is_dragging(), moving));
        visual.simulate_keystrokes("space");
        let deadline = crate::test_wait::hang_deadline();
        while backend.status().unwrap().playing.load(Acquire) {
            pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "actual held Space did not pause"
            );
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let deadline = crate::test_wait::hang_deadline();
        loop {
            pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            if editor.read_with(cx, |view, _| settled(view)) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "eq_held_node_space_starts_applied_preview_without_seek_worker_or_release: hang guard expired"
            );
        }
    }
    editor.update(cx, |view, _| {
        let _ = view.audio.take();
    });
    backend.close();
    drop(output);
}

fn preview_is_applied(view: &EditorView) -> bool {
    use std::sync::atomic::Ordering::Acquire;
    let Some(snapshot) = view.device_gesture.snapshot() else {
        return false;
    };
    view.worker_requests_pending == 0
        && view.pending_requests.is_empty()
        && view.pending_playback.is_none()
        && view.pending_resident_publication.is_none()
        && view.playback_candidates.is_empty()
        && view.audible_playback.as_ref().is_some_and(|audible| {
            audible.device_preview_identity.is_some_and(|identity| {
                identity.key == snapshot.key && identity.sequence == snapshot.sequence
            }) && view
                .audio
                .as_ref()
                .unwrap()
                .control
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire)
                == audible.audio_revision
        })
}

fn pump_actual_until(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
    output: &mut hex_audio::WorkerPreparedOutput,
    backend: &mut hex_audio::worker_fixture::HeadlessBackend,
    ready: impl Fn(&EditorView) -> bool,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        pump_output(editor, cx, output, backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| ready(view)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual output/worker did not settle: {}",
            editor.read_with(cx, |view, _| format!("error={:?} generation={} requests={} deferred={} gesture={:?} seek={:?} latched={} pending_playback={} publication={} candidates={} audio={} tracks={}", view.error, view.project_generation, view.worker_requests_pending, view.pending_requests.len(), view.device_gesture.snapshot().map(|s|s.key), view.prepared_seek.as_ref().map(|s|(s.stage,s.cancelled)), view.latched_seek_intent.is_some(), view.pending_playback.is_some(), view.pending_resident_publication.is_some(), view.playback_candidates.len(), view.audio.is_some(), view.state.as_ref().map_or(0,|s|s.project.tracks().len())))
        );
    }
}

#[gpui::test]
fn eq_held_node_seek_sequence_terminal_and_project_races_preserve_actual_callback_custody(
    cx: &mut TestAppContext,
) {
    use crate::editor::arrangement_seek::SeekStage;
    use std::sync::atomic::Ordering::Acquire;
    let _output_guard = graph_layout_tests::status_fixture_guard();
    for published in [false, true] {
        for action in ["sequence", "cancel", "commit", "project", "device", "pause"] {
            eprintln!("ACTUAL_HELD_SEEK_RACE published={published} action={action}");
            let (handle, editor, _environment, device) = held_node_window(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
            let before = editor.read_with(cx, |view, _| read(view, device));
            let before_project = editor.read_with(cx, |view, _| {
                format!("{:?}", view.state.as_ref().unwrap().project)
            });
            let start = plot(&mut visual, device, 0.5, 0.5);
            let first = plot(&mut visual, device, 0.62, 0.7);
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(first, Some(MouseButton::Left), Modifiers::default());
            pump_actual_until(
                &editor,
                cx,
                &mut visual,
                &mut output,
                &mut backend,
                preview_is_applied,
            );
            request_worker_seek(&editor, cx);
            let (request_id, identity) = editor.read_with(cx, |view, _| {
                let pending = view.prepared_seek.as_ref().expect("actual seek admitted");
                assert_eq!(pending.stage, SeekStage::Preparing);
                (
                    pending.request_id,
                    pending.device_preview_identity.expect("preview retained"),
                )
            });
            if published {
                // Run the real preparation/publication workers but deliberately
                // withhold callback blocks until the exact revision is queued.
                let deadline = crate::test_wait::hang_deadline();
                loop {
                    editor.update(cx, |view, cx| {
                        if view.tick(cx) {
                            cx.notify();
                        }
                    });
                    let control = output.publication.control().unwrap();
                    control.reclaim();
                    control.poll_resident_publication();
                    if editor.read_with(cx, |view, _| {
                        view.prepared_seek
                            .as_ref()
                            .is_some_and(|seek| matches!(seek.stage, SeekStage::Published { .. }))
                    }) {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "actual seek publication did not complete"
                    );
                    std::thread::yield_now();
                }
                editor.read_with(cx, |view, _| {
                    let candidate = view
                        .playback_candidates
                        .iter()
                        .find(|candidate| candidate.seek_request_id == Some(request_id))
                        .unwrap();
                    assert_eq!(candidate.device_preview_identity, Some(identity));
                });
            }
            let generation = editor.read_with(cx, |view, _| view.project_generation);
            let held_output = editor.read_with(cx, |view, _| {
                view.audio
                    .as_ref()
                    .and_then(|a| a.control.output_identity())
            });
            match action {
                "sequence" => {
                    let next = plot(&mut visual, device, 0.71, 0.6);
                    visual.simulate_mouse_move(next, Some(MouseButton::Left), Modifiers::default());
                    editor.read_with(cx, |view, _| {
                        let snapshot = view.device_gesture.snapshot().unwrap();
                        assert_eq!(snapshot.key, identity.key);
                        assert!(snapshot.sequence > identity.sequence);
                    });
                }
                "cancel" => visual.simulate_keystrokes("escape"),
                "commit" => {
                    visual.simulate_mouse_up(first, MouseButton::Left, Modifiers::default())
                }
                "project" => editor.update(cx, |view, _| {
                    assert!(view.enqueue(Request::NewWithAudioMode {
                        mode: hex_session::DocumentAudioMode::Resident,
                        reservation: None
                    }));
                }),
                "device" => editor.update(cx, |view, _| {
                    assert!(view.enqueue(Request::Edit(Command::RemoveDevice(device))));
                }),
                "pause" => visual.simulate_keystrokes("space"),
                _ => unreachable!(),
            }
            if action == "sequence" {
                pump_actual_until(
                    &editor,
                    cx,
                    &mut visual,
                    &mut output,
                    &mut backend,
                    |view| {
                        view.audio
                            .as_ref()
                            .unwrap()
                            .control
                            .status()
                            .unwrap()
                            .playing
                            .load(Acquire)
                            && view.prepared_seek.is_none()
                            && view.latched_seek_intent.is_none()
                    },
                );
                editor.read_with(cx, |view, _| {
                    let audible = view.audible_playback.as_ref().unwrap();
                    let current = audible
                        .device_preview_identity
                        .expect("seek remains a preview");
                    let snapshot = view.device_gesture.snapshot().unwrap();
                    assert_eq!(
                        (current.key, current.sequence),
                        (snapshot.key, snapshot.sequence)
                    );
                    assert_ne!(current.sequence, identity.sequence);
                    assert_ne!(
                        EqState::decode(audible.state.project.device(device).unwrap().state())
                            .unwrap(),
                        before,
                        "preview source metadata must not become canonical"
                    );
                    assert_eq!(read(view, device), before, "no held edit committed by seek");
                });
                visual.simulate_keystrokes("space");
                visual.simulate_keystrokes("escape");
            }
            pump_actual_until(
                &editor,
                cx,
                &mut visual,
                &mut output,
                &mut backend,
                |view| {
                    (settled(view) || (action == "pause"
                        && view.worker_requests_pending == 0 && view.pending_requests.is_empty()))
                        && view.prepared_seek.is_none()
                        && view.latched_seek_intent.is_none()
                        // Replacement deliberately closes the old output. The
                        // new canonical graph may wait for a NEW output; it is
                        // not an old seek and must not be discarded by this test.
                        && (action == "project" || view.pending_playback.is_none())
                        && view.pending_resident_publication.is_none()
                        && view.playback_candidates.is_empty()
                        && (action != "project" || view.project_generation > generation)
                },
            );
            assert!(
                !backend.status().unwrap().playing.load(Acquire),
                "stale seek must not start after {action}"
            );
            editor.read_with(cx, |view, _| {
                assert!(
                    !view
                        .playback_candidates
                        .iter()
                        .any(|candidate| candidate.seek_request_id == Some(request_id))
                );
                assert!(
                    view.audible_playback
                        .as_ref()
                        .is_none_or(|audible| audible.seek_request_id != Some(request_id)),
                    "old seek must not become current"
                );
                if action == "commit" {
                    assert_ne!(read(view, device), before);
                } else if action == "project" {
                    // This fixture worker never reports OutputReleased, so the
                    // Resident endpoint stays held across the replacement.
                    assert_eq!(
                        view.audio
                            .as_ref()
                            .and_then(|a| a.control.output_identity()),
                        held_output
                    );
                    assert!(view.state.as_ref().unwrap().project.tracks().is_empty());
                    if let Some(pending) = view.pending_playback.as_ref() {
                        assert_eq!(pending.project_generation, view.project_generation);
                        assert!(pending.seek_request_id.is_none());
                        assert!(pending.device_preview_identity.is_none());
                    }
                } else if action == "device" {
                    assert!(
                        view.state
                            .as_ref()
                            .unwrap()
                            .project
                            .device(device)
                            .is_none()
                    );
                } else if action == "pause" {
                    assert_eq!(view.device_gesture.snapshot().unwrap().key, identity.key);
                    assert_eq!(read(view, device), before);
                } else {
                    assert_eq!(
                        format!("{:?}", view.state.as_ref().unwrap().project),
                        before_project
                    );
                }
            });
            if action == "commit" {
                editor.update(cx, |view, _| {
                    assert!(view.enqueue(Request::Undo));
                });
                pump_actual_until(&editor, cx, &mut visual, &mut output, &mut backend, settled);
                editor.read_with(cx, |view, _| assert_eq!(read(view, device), before));
            }
            if action == "pause" {
                visual.simulate_keystrokes("escape");
                pump_actual_until(&editor, cx, &mut visual, &mut output, &mut backend, settled);
            }
            editor.update(cx, |view, _| {
                let _ = view.audio.take();
            });
            backend.close();
            drop(output);
        }
    }
}

#[gpui::test]
fn eq_held_node_busy_seek_returns_exact_candidate_before_new_preview_replaces_it(
    cx: &mut TestAppContext,
) {
    use crate::editor::arrangement_seek::{LAST_SEEK_RETIREMENT, SeekStage};
    use hex_audio::prepared::PublicationPolicy;
    let _output_guard = graph_layout_tests::status_fixture_guard();
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
    let start = plot(&mut visual, device, 0.5, 0.5);
    let first = plot(&mut visual, device, 0.62, 0.7);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(first, Some(MouseButton::Left), Modifiers::default());
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        preview_is_applied,
    );
    request_worker_seek(&editor, cx);
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if editor.update(cx, |view, _| {
            view.tick_prepared_arrangement_seek();
            view.prepared_seek
                .as_ref()
                .is_some_and(|seek| seek.stage == SeekStage::CandidateReady)
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "eq_held_node_busy_seek_returns_exact_candidate_before_new_preview_replaces_it: hang guard expired"
        );
        std::thread::yield_now();
    }
    let (id, root, previous_identity, blocker) = editor.update(cx, |view, cx| {
        let pending = view.pending_playback.as_ref().unwrap();
        let seek = view.prepared_seek.as_ref().unwrap();
        let id = seek.request_id;
        let root = Arc::as_ptr(seek.prepared_revision.as_ref().unwrap()) as usize;
        let identity = seek.device_preview_identity.unwrap();
        assert_eq!(pending.seek_request_id, Some(id));
        // Occupy the REAL forwarding cell, without polling its worker. A second
        // publication must return the exact seek Playback with actual Busy.
        let blocker = hex_engine::playback::Playback::from_prepared(Arc::clone(
            seek.prepared_revision.as_ref().unwrap(),
        ));
        let token = view
            .audio
            .as_mut()
            .unwrap()
            .control
            .forward_resident_publication(
                blocker,
                PublicationPolicy::InheritRuntime,
                view.project_generation,
            )
            .unwrap();
        view.tick(cx);
        assert_eq!(
            view.prepared_seek.as_ref().unwrap().stage,
            SeekStage::CandidateReady
        );
        assert_eq!(
            view.pending_playback.as_ref().unwrap().seek_request_id,
            Some(id)
        );
        assert!(view.pending_resident_publication.is_none());
        (id, root, identity, token)
    });
    LAST_SEEK_RETIREMENT.set(None);
    let next = plot(&mut visual, device, 0.71, 0.6);
    visual.simulate_mouse_move(next, Some(MouseButton::Left), Modifiers::default());
    editor.update(cx, |view, _| {
        view.pump_device_gesture();
    });
    // Consume the ACTUAL Session preview event directly to observe receive's
    // replacement cutpoint before the later timer invalidation can mask it.
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let replaced = editor.update(cx, |view, _| {
            let Ok(event) = view.service.try_event() else {
                return false;
            };
            let replacement = event.device_preview.as_ref().is_some_and(|preview| {
                preview.identity.key == previous_identity.key
                    && preview.identity.sequence > previous_identity.sequence
            }) && event.playback.is_some();
            view.receive(event);
            replacement
        });
        if replaced {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual newer preview event missing"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        LAST_SEEK_RETIREMENT.get(),
        Some((id, root, true)),
        "exact CandidateReady must cross actual seek-worker retirement BEFORE preview replacement"
    );
    editor.read_with(cx, |view, _| {
        assert!(view.prepared_seek.is_none());
        let pending = view.pending_playback.as_ref().unwrap();
        assert!(pending.seek_request_id.is_none());
        let current = pending.device_preview_identity.unwrap();
        assert_eq!(current.key, previous_identity.key);
        assert!(current.sequence > previous_identity.sequence);
        assert!(view.latched_seek_intent.is_some());
    });
    // Finish the deliberately occupied real publication before pumping normal
    // input/callback reconciliation; no fabricated ACK/status or owner drop.
    output
        .publication
        .control()
        .unwrap()
        .poll_resident_publication();
    editor.update(cx, |view, _| {
        let response = view
            .audio
            .as_mut()
            .unwrap()
            .control
            .try_resident_publication_response()
            .unwrap();
        assert_eq!(response.token, blocker);
        assert!(response.result.is_ok());
    });
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| {
            view.prepared_seek.is_none()
                && view.latched_seek_intent.is_none()
                && view
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .status()
                    .unwrap()
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
        },
    );
    visual.simulate_keystrokes("space");
    visual.simulate_keystrokes("escape");
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| settled(view) && view.prepared_seek.is_none() && view.latched_seek_intent.is_none(),
    );
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn eq_held_node_retirement_fallback_blocks_admission_and_retries_exact_runtime(
    cx: &mut TestAppContext,
) {
    use crate::editor::arrangement_seek::{LAST_SEEK_RETIREMENT, SeekStage};
    let _output_guard = graph_layout_tests::status_fixture_guard();
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
    let start = plot(&mut visual, device, 0.5, 0.5);
    let first = plot(&mut visual, device, 0.62, 0.7);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(first, Some(MouseButton::Left), Modifiers::default());
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        preview_is_applied,
    );
    request_worker_seek(&editor, cx);
    let deadline = crate::test_wait::hang_deadline();
    while !editor.update(cx, |view, _| {
        view.tick_prepared_arrangement_seek();
        view.prepared_seek
            .as_ref()
            .is_some_and(|seek| seek.stage == SeekStage::CandidateReady)
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "eq_held_node_retirement_fallback_blocks_admission_and_retries_exact_runtime: hang guard expired"
        );
        std::thread::yield_now();
    }
    // Seed the existing UI fallback with the REAL completed runtime. This is
    // a component test of fallback ownership/admission, not fabricated worker
    // contention. Actual worker-full/shutdown/drop-thread tests live in Session.
    let (id, root) = editor.update(cx, |view, _| {
        let seek = view.prepared_seek.take().unwrap();
        let playback = view.pending_playback.take().unwrap();
        assert_eq!(playback.seek_request_id, Some(seek.request_id));
        let id = seek.request_id;
        let root = Arc::as_ptr(seek.prepared_revision.as_ref().unwrap()) as usize;
        view.retired_seek_result = Some(hex_session::arrangement_seek::ArrangementSeekResult {
            request_id: id,
            mode: seek.kind.mode(),
            generation: seek.project_generation,
            revision: seek.project_revision,
            prepared_revision: seek.prepared_revision.unwrap(),
            destination_frame: seek.destination_frame,
            work_budget_frames: seek.work_budget_frames,
            start_playing: seek.start_playing,
            result: Ok(playback.playback),
        });
        for destination in [16, 32, 64] {
            view.request_prepared_seek(destination, false);
            assert!(
                view.prepared_seek.is_none(),
                "fallback must block a second runtime owner"
            );
            let retained = view.retired_seek_result.as_ref().unwrap();
            assert_eq!(retained.request_id, id);
            assert_eq!(Arc::as_ptr(&retained.prepared_revision) as usize, root);
            assert!(retained.result.is_ok());
            assert_eq!(
                view.latched_seek_intent.as_ref().unwrap().destination_frame,
                destination
            );
        }
        (id, root)
    });
    LAST_SEEK_RETIREMENT.set(None);
    editor.update(cx, |view, _| {
        view.tick_prepared_arrangement_seek();
    });
    assert_eq!(
        LAST_SEEK_RETIREMENT.get(),
        Some((id, root, true)),
        "fallback retry must transfer the exact runtime to the actual worker"
    );
    visual.simulate_keystrokes("escape");
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| {
            settled(view)
                && view.prepared_seek.is_none()
                && view.latched_seek_intent.is_none()
                && view.retired_seek_result.is_none()
        },
    );
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}

// Space starts the applied runtime directly. These lifecycle races deliberately
// request the existing guarded worker seek so candidate/publication custody is
// still exercised independently of the immediate keyboard transport path.
fn request_worker_seek(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    editor.update(cx, |view, _| {
        view.request_prepared_seek(0, true);
        assert!(
            view.prepared_seek.is_some(),
            "worker seek admission: {:?}",
            view.error
        );
    });
}

// Advance the real preparation worker without pumping a callback or the
// resident-publication worker. This preserves the otherwise short race window.
fn review_wait_seek_candidate(editor: &gpui::Entity<EditorView>, cx: &mut TestAppContext) {
    use crate::editor::arrangement_seek::SeekStage;
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if editor.update(cx, |view, _| {
            view.tick_prepared_arrangement_seek();
            view.prepared_seek
                .as_ref()
                .is_some_and(|s| s.stage == SeekStage::CandidateReady)
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual seek worker did not complete"
        );
        std::thread::yield_now();
    }
}

fn review_receive_preview(
    editor: &gpui::Entity<EditorView>,
    cx: &mut TestAppContext,
    sequence: u64,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if editor.update(cx, |view, _| {
            let Ok(event) = view.service.try_event() else {
                return false;
            };
            let found = event
                .device_preview
                .as_ref()
                .is_some_and(|p| p.identity.sequence == sequence)
                && event.playback.is_some();
            view.receive(event);
            found
        }) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual preview {sequence} missing"
        );
        std::thread::yield_now();
    }
}

#[gpui::test]
fn eq_review_overtaken_preview_preserves_held_seek_intent(cx: &mut TestAppContext) {
    let _guard = graph_layout_tests::status_fixture_guard();
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let first = plot(&mut visual, device, 0.62, 0.7);
    visual.simulate_mouse_move(first, Some(MouseButton::Left), Modifiers::default());
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        preview_is_applied,
    );
    request_worker_seek(&editor, cx);
    review_wait_seek_candidate(&editor, cx);
    let second = plot(&mut visual, device, 0.68, 0.65);
    visual.simulate_mouse_move(second, Some(MouseButton::Left), Modifiers::default());
    let submitted = editor.update(cx, |view, _| {
        view.pump_device_gesture();
        view.device_gesture.snapshot().unwrap().sequence
    });
    let third = plot(&mut visual, device, 0.73, 0.6);
    visual.simulate_mouse_move(third, Some(MouseButton::Left), Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.device_gesture.snapshot().unwrap().sequence > submitted)
    });
    review_receive_preview(&editor, cx, submitted);
    editor.read_with(cx, |view, _| {
        assert!(view.prepared_seek.is_none());
        assert!(
            view.latched_seek_intent
                .as_ref()
                .is_some_and(|i| i.start_playing),
            "intermediate same-owner preview must not erase the held seek"
        );
    });
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| {
            view.prepared_seek.is_none()
                && view.latched_seek_intent.is_none()
                && view
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .status()
                    .unwrap()
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
        },
    );
    visual.simulate_keystrokes("space");
    visual.simulate_keystrokes("escape");
    pump_actual_until(&editor, cx, &mut visual, &mut output, &mut backend, settled);
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn eq_review_publication_retirement_preserves_newer_held_seek_intent(cx: &mut TestAppContext) {
    use crate::editor::arrangement_seek::SeekStage;
    let _guard = graph_layout_tests::status_fixture_guard();
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let first = plot(&mut visual, device, 0.62, 0.7);
    visual.simulate_mouse_move(first, Some(MouseButton::Left), Modifiers::default());
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        preview_is_applied,
    );
    request_worker_seek(&editor, cx);
    review_wait_seek_candidate(&editor, cx);
    editor.update(cx, |view, cx| {
        view.tick(cx);
        assert!(view.pending_resident_publication.is_some());
        assert_eq!(
            view.prepared_seek.as_ref().unwrap().stage,
            SeekStage::CandidateReady
        );
    });
    let next = plot(&mut visual, device, 0.71, 0.6);
    visual.simulate_mouse_move(next, Some(MouseButton::Left), Modifiers::default());
    editor.update(cx, |view, _| {
        view.tick_prepared_arrangement_seek();
        assert!(view.prepared_seek.as_ref().unwrap().cancelled);
        assert!(view.latched_seek_intent.is_some());
    });
    let control = output.publication.control().unwrap();
    control.poll_resident_publication();
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if editor.update(cx, |view, cx| {
            view.tick(cx);
            matches!(
                view.prepared_seek.as_ref().map(|s| s.stage),
                Some(SeekStage::Published { .. })
            )
        }) {
            break;
        }
        control.poll_resident_publication();
        assert!(
            std::time::Instant::now() < deadline,
            "eq_review_publication_retirement_preserves_newer_held_seek_intent: hang guard expired"
        );
        std::thread::yield_now();
    }
    editor.read_with(cx, |view, _| {
        assert!(
            view.latched_seek_intent
                .as_ref()
                .is_some_and(|i| i.start_playing),
            "publication response must preserve the newer held intent during cancellation"
        )
    });
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| {
            view.prepared_seek.is_none()
                && view.latched_seek_intent.is_none()
                && view
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .status()
                    .unwrap()
                    .playing
                    .load(std::sync::atomic::Ordering::Acquire)
        },
    );
    visual.simulate_keystrokes("space");
    visual.simulate_keystrokes("escape");
    pump_actual_until(&editor, cx, &mut visual, &mut output, &mut backend, settled);
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn eq_review_completed_seek_cancel_reconciles_actual_callback_revision(cx: &mut TestAppContext) {
    use crate::editor::arrangement_seek::SeekStage;
    use std::sync::atomic::Ordering::Acquire;
    let _guard = graph_layout_tests::status_fixture_guard();
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (mut output, mut backend) = attach_actual_output(&editor, cx, &mut visual);
    let start = plot(&mut visual, device, 0.5, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    let first = plot(&mut visual, device, 0.62, 0.7);
    visual.simulate_mouse_move(first, Some(MouseButton::Left), Modifiers::default());
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        preview_is_applied,
    );
    request_worker_seek(&editor, cx);
    review_wait_seek_candidate(&editor, cx);
    let deadline = crate::test_wait::hang_deadline();
    let revision = loop {
        editor.update(cx, |view, cx| {
            view.tick(cx);
        });
        output
            .publication
            .control()
            .unwrap()
            .poll_resident_publication();
        if let Some(revision) = editor.read_with(cx, |view, _| {
            match view.prepared_seek.as_ref().unwrap().stage {
                SeekStage::Published { audio_revision } => Some(audio_revision),
                _ => None,
            }
        }) {
            break revision;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "eq_review_completed_seek_cancel_reconciles_actual_callback_revision: hang guard expired"
        );
        std::thread::yield_now();
    };
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        backend.status().unwrap().applied_revision.load(Acquire),
        revision
    );
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.audio
                .as_ref()
                .unwrap()
                .control
                .cancel_fresh_runtime_publication(revision),
            hex_audio::prepared::FreshRuntimeCancellationResult::Complete
        );
    });
    // Callback won, but UI has not consumed its applied ACK. Pause is a real
    // action and must not make the now-installed source disappear from metadata.
    visual.simulate_keystrokes("space");
    editor.update(cx, |view, cx| {
        view.tick(cx);
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            revision,
            "late cancellation cannot discard the installed seek metadata"
        );
        assert!(view.prepared_seek.is_none());
    });
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| {
            !view
                .audio
                .as_ref()
                .unwrap()
                .control
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        },
    );
    visual.simulate_keystrokes("space");
    pump_actual_until(
        &editor,
        cx,
        &mut visual,
        &mut output,
        &mut backend,
        |view| {
            view.prepared_seek.is_none()
                && view
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .status()
                    .unwrap()
                    .playing
                    .load(Acquire)
        },
    );
    visual.simulate_keystrokes("space");
    visual.simulate_keystrokes("escape");
    pump_actual_until(&editor, cx, &mut visual, &mut output, &mut backend, settled);
    editor.update(cx, |view, _| {
        view.audio.take();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn eq_created_node_retains_released_position_through_exact_worker_terminal(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let graph = graph(&editor, cx, device);
    let before = editor.read_with(&visual, |view, _| read(view, device));
    let id = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .next_identity()
            .max(u64::from(before.next_node_id)) as u32
    });
    let start = plot(&mut visual, device, 0.8, 0.8);
    let end = plot(&mut visual, device, 0.72, 0.75);
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let held = graph.read_with(&visual, |graph, _| {
        graph.node_position(id).expect("new held node")
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    // Advance the real keyed stream and hold only its exact Commit reply.
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        let terminal = editor.update(&mut visual, |view, cx| {
            view.pump_device_gesture();
            let terminal = if let Ok(event) = view.service.try_event() {
                if event.gesture_ack.as_ref().is_some_and(|ack| {
                    ack.phase == hex_session::device_gesture::GesturePhase::Commit
                }) {
                    Some(event)
                } else {
                    view.receive(event);
                    None
                }
            } else {
                None
            };
            cx.notify();
            terminal
        });
        visual.run_until_parked();
        assert_eq!(
            graph.read_with(&visual, |graph, _| graph.node_position(id)),
            Some(held)
        );
        if let Some(event) = terminal {
            break event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "eq_created_node_retains_released_position_through_exact_worker_terminal: hang guard expired"
        );
        std::thread::yield_now();
    };
    assert!(terminal.gesture_ack.as_ref().unwrap().accepted);
    for _ in 0..4 {
        editor.update(&mut visual, |view, cx| {
            assert_eq!(read(view, device), before);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            graph.read_with(&visual, |graph, _| graph.node_position(id)),
            Some(held),
            "released creation must not vanish before its exact keyed terminal"
        );
    }
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    for _ in 0..4 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            graph.read_with(&visual, |graph, _| graph.node_position(id)),
            Some(held)
        );
    }
    let after = editor.read_with(&visual, |view, _| read(view, device));
    assert_eq!(after.nodes.len(), before.nodes.len() + 1);
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == before
    });
    editor.update(cx, |view, _| assert!(view.enqueue(Request::Redo)));
    pump_until(&editor, cx, |view| {
        settled(view) && read(view, device) == after
    });
}

#[gpui::test]
fn eq_new_node_reaches_worker_playback_preview_while_pointer_is_held(cx: &mut TestAppContext) {
    let (handle, editor, _environment, device) = held_node_window(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let start = plot(&mut visual, device, 0.78, 0.8);
    let end = plot(&mut visual, device, 0.66, 0.75);
    visual.simulate_mouse_down(start, MouseButton::Left, OPTION_CREATE);
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    pump_until(&editor, cx, |view| {
        view.resident_device_preview
            .as_ref()
            .is_some_and(|preview| {
                EqState::decode(preview.state.project.device(device).unwrap().state())
                    .is_ok_and(|state| state.nodes.len() == 2)
            })
    });
    for _ in 0..3 {
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            assert!(view.resident_device_preview.is_some());
        });
    }
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    pump_until(&editor, cx, settled);
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
}
