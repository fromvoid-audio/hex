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

fn pump_output(
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

fn attach_actual_output(
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        pump_output(&editor, cx, &mut output, &mut backend);
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
fn eq_held_node_space_reaches_real_prepared_output_and_callback_without_release(
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
        let key = editor.read_with(cx, |view, _| {
            view.device_gesture.snapshot().map(|snapshot| snapshot.key)
        });
        visual.simulate_keystrokes("space");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
        graph.read_with(cx, |graph, _| assert_eq!(graph.is_dragging(), moving));
        visual.simulate_keystrokes("space");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while backend.status().unwrap().playing.load(Acquire) {
            pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            assert!(
                std::time::Instant::now() < deadline,
                "actual held Space did not pause"
            );
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            pump_output(&editor, cx, &mut output, &mut backend);
            visual.run_until_parked();
            if editor.read_with(cx, |view, _| settled(view)) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        pump_output(editor, cx, output, backend);
        visual.run_until_parked();
        if editor.read_with(cx, |view, _| ready(view)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "actual output/worker did not settle: {}",
            editor.read_with(cx, |view, _| format!("{:?}", view.error))
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
        for action in ["sequence", "cancel", "commit", "project", "pause"] {
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
            visual.simulate_keystrokes("space");
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
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
                    assert_eq!(
                        read(view, device),
                        before,
                        "no held edit committed by Space"
                    );
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
                    settled(view)
                        && view.prepared_seek.is_none()
                        && view.latched_seek_intent.is_none()
                        && view.pending_playback.is_none()
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
                } else if action != "project" && action != "pause" {
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
