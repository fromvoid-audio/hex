use super::*;

#[gpui::test]
fn audio_origin_capture_uses_window_distance_at_close_zoom(cx: &mut TestAppContext) {
    assert_audio_origin_window(cx, 192_000);
}

#[gpui::test]
fn audio_origin_capture_uses_window_distance_at_wide_zoom(cx: &mut TestAppContext) {
    assert_audio_origin_window(cx, 384_000);
}

#[gpui::test]
fn origin_capture_still_applies_with_the_grid_off(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(
            cx,
            &[midi],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                grid: super::super::musical_time::SnapGrid::Off,
                ..Default::default()
            }),
        );
    }
}

#[gpui::test]
fn command_bypasses_audio_origin_and_both_grid_settings(cx: &mut TestAppContext) {
    for grid in [
        super::super::musical_time::SnapGrid::Quarter,
        super::super::musical_time::SnapGrid::Off,
    ] {
        assert_drag_handoff_at_grab(
            cx,
            &[false],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                grid,
                bypass: true,
                ..Default::default()
            }),
        );
    }
}

#[gpui::test]
fn command_bypasses_midi_origin_and_both_grid_settings(cx: &mut TestAppContext) {
    for grid in [
        super::super::musical_time::SnapGrid::Quarter,
        super::super::musical_time::SnapGrid::Off,
    ] {
        assert_drag_handoff_at_grab(
            cx,
            &[true],
            true,
            false,
            false,
            Some(1.0 / 3.0),
            Some(OriginProbe {
                grid,
                bypass: true,
                ..Default::default()
            }),
        );
    }
}

#[gpui::test]
fn outside_origin_capture_returns_to_the_edge_grid(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(
            cx,
            &[midi],
            true,
            false,
            false,
            Some(5.0 / 6.0),
            Some(OriginProbe {
                dx: 20.0,
                ..Default::default()
            }),
        );
    }
}

#[gpui::test]
fn origin_capture_keeps_one_delta_for_mixed_group_move(cx: &mut TestAppContext) {
    assert_drag_handoff_at_grab(
        cx,
        &[false, true],
        true,
        false,
        false,
        Some(5.0 / 6.0),
        Some(OriginProbe::default()),
    );
}

#[gpui::test]
fn origin_capture_keeps_mixed_copy_paint_through_a_queued_ack(cx: &mut TestAppContext) {
    assert_drag_handoff_at_grab(
        cx,
        &[false, true],
        true,
        true,
        true,
        Some(5.0 / 6.0),
        Some(OriginProbe::default()),
    );
}

#[gpui::test]
fn rendered_move_cancel_and_stale_controls_restore_source(cx: &mut TestAppContext) {
    for control in 0..4 {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let clip = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
        });
        let source = selector(format!("clip-visual-{}", clip.value()));
        let old = visual.debug_bounds(source).unwrap();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let (_, end) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 120.0);
        assert!(visual.debug_bounds("clip-move-guide-top").is_some());
        match control {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, cx| {
                assert!(cx.stop_active_drag(window));
            }),
            _ => {
                editor.update(&mut visual, |v, _| {
                    assert!(
                        v.enqueue(hex_session::service::Request::Edit(Command::RenameClip {
                            clip,
                            name: "independent edit".into()
                        }))
                    )
                });
                if control == 3 {
                    pump_move(&editor, &mut visual);
                }
            }
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        if control == 2 {
            editor.update(&mut visual, |v, cx| {
                assert_eq!(v.worker_requests_pending, 2);
                assert!(v.pending_clip_move.is_some());
                let renamed = upper_rail_next(&v.service);
                assert!(renamed.error.is_none());
                v.receive(renamed);
                assert!(v.pending_clip_move.is_some());
                let refused = upper_rail_terminal(&v.service);
                assert!(refused.error.as_ref().unwrap().contains("project changed"));
                v.receive(refused);
                assert!(v.pending_clip_move.is_none());
                cx.notify();
            });
        }
        editor.update(&mut visual, |v, _| {
            v.error = None;
        });
        pump_move(&editor, &mut visual);
        assert!(visual.debug_bounds("clip-move-guide-top").is_none());
        assert!(visual.debug_bounds("clip-move-guide-bottom").is_none());
        assert!(visual.debug_bounds("clip-move-guide-line").is_none());
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_clip_move.is_none() && v.interaction.active_clip_drag.is_none());
            if control < 2 {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            } else {
                let p = &v.state.as_ref().unwrap().project;
                assert_eq!(p.clip(clip).unwrap().start(), 0);
                assert_eq!(p.clip_name(clip), Some("independent edit"));
                if control == 3 {
                    assert!(matches!(
                        v.test_probes.last_enqueued_edit,
                        Some(Command::RenameClip { .. })
                    ));
                } else {
                    assert!(matches!(
                        v.test_probes.last_enqueued_edit,
                        Some(Command::MoveClipWithOverwrite { .. })
                    ));
                }
            }
        });
        assert!(visual.debug_bounds("clip-drop-preview").is_none());
        assert_eq!(visual.debug_bounds(source), Some(old));
        assert!(visual.rendered_background(source).is_some());
    }
}

#[gpui::test]
fn alias_drag_preview_and_release_follow_live_modifiers_for_audio_and_midi(
    cx: &mut TestAppContext,
) {
    for midi in [false, true] {
        let (project, _, target, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let midi_preview = if midi {
            let (_, preview) =
                hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                    .unwrap();
            Arc::new(preview)
        } else {
            Arc::default()
        };
        let (handle, editor) = editor_window(cx, project, midi_preview);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let (_, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
        assert!(visual.debug_bounds("clip-alias-drag-indicator").is_none());
        let alias = Modifiers {
            alt: true,
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_modifiers_change(alias);
        visual.run_until_parked();
        let badge = visual.debug_bounds("clip-alias-drag-indicator").unwrap();
        let preview = visual.debug_bounds("clip-drop-preview").unwrap();
        assert!(
            badge.right() <= preview.right() - px(5.0),
            "alias badge clears the clip corner cut"
        );
        visual.simulate_modifiers_change(Modifiers {
            alt: true,
            ..Modifiers::default()
        });
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-alias-drag-indicator").is_none());
        visual.simulate_modifiers_change(alias);
        visual.run_until_parked();
        visual.simulate_mouse_up(release, MouseButton::Left, alias);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.test_probes.last_enqueued_edit.as_ref(), Some(Command::DuplicateAliasClipsAt { clips, anchor, track, .. }) if clips == &[clip] && *anchor == clip && *track == target));
            assert!(view.pending_clip_move.is_some(), "preview remains in custody until the worker reply");
        });
        assert!(visual.debug_bounds("clip-alias-drag-indicator").is_some());
    }
}

#[gpui::test]
fn drag_guides_preserve_locked_play_start_through_cancel_and_ack(cx: &mut TestAppContext) {
    for cancel in [true, false] {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let clip = editor.update(&mut visual, |view, cx| {
            let track = &view.state.as_ref().unwrap().project.tracks()[0];
            let clip = track.clips()[0].id();
            let cursor = super::super::edit_cursor::EditCursor {
                track: Some(track.id()),
                frame: 24_000,
                generation: view.project_generation,
                pulse_color: crate::theme::time_position_marker_color(),
                pulse_started: std::time::Instant::now(),
            };
            view.interaction.edit_cursor = Some(cursor);
            view.interaction.locked_play_start = Some(cursor);
            cx.notify();
            clip
        });
        visual.run_until_parked();
        let (_, end) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 120.0);
        let destination = editor.read_with(&visual, |view, _| {
            view.interaction
                .active_clip_drag
                .as_ref()
                .unwrap()
                .cursor_position()
                .unwrap()
        });
        assert!(visual.debug_bounds("clip-move-guide-top").is_some());
        assert!(visual.debug_bounds("clip-move-guide-line").is_some());
        assert!(visual.debug_bounds("locked-play-start-top").is_some());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.playback_start_frame(), 24_000)
        });
        if cancel {
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            assert!(visual.debug_bounds("clip-move-guide-line").is_none());
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        if !cancel {
            visual.run_until_parked();
            assert!(visual.debug_bounds("clip-move-guide-top").is_some());
            pump_move(&editor, &mut visual);
        }
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-move-guide-top").is_none());
        assert!(visual.debug_bounds("clip-move-guide-bottom").is_none());
        assert!(visual.debug_bounds("clip-move-guide-line").is_none());
        assert!(visual.debug_bounds("locked-play-start-top").is_some());
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.valid_locked_play_start().unwrap().frame, 24_000);
            assert_eq!(view.playback_start_frame(), 24_000);
            let cursor = view.interaction.edit_cursor.unwrap();
            if cancel {
                assert_eq!(cursor.frame, 24_000);
            } else {
                assert_eq!((cursor.track, cursor.frame), destination);
            }
            assert!(view.test_probes.last_transport_command.is_none());
        });
    }
}

#[gpui::test]
fn parameter_observations_preserve_audio_and_midi_resize_custody(cx: &mut TestAppContext) {
    use hex_session::service::{PluginParameterQuery, Request};
    for midi in [false, true] {
        let (project, _, _, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let mut model = Editor::new(project, 8).unwrap();
        let Created::Device(device) = model
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let preview = if midi {
            hex_engine::compile::project_with_midi_preview(model.project(), &Default::default())
                .unwrap()
                .1
        } else {
            Default::default()
        };
        let (handle, editor) = editor_window(cx, model.project().clone(), Arc::new(preview));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let prefix = if midi {
            "midi-clip-trim-right"
        } else {
            "clip-trim-right"
        };
        let selector = Box::leak(format!("{prefix}-{}", clip.value()).into_boxed_str());
        let down = trim_grab_point(visual.debug_bounds(selector).unwrap());
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.update(cx, |view, _| {
            let query = PluginParameterQuery {
                context: hex_engine::AudioSourceContext {
                    project_generation: view.project_generation,
                    editor_revision: view.state.as_ref().unwrap().revision,
                },
                device,
                output_identity: 1,
                request_id: 1,
                page_first: Some(0),
            };
            assert!(view.enqueue(Request::QueryPluginParameters(query)));
        });
        visual.run_until_parked();
        let target = point(down.x - px(28.0), down.y);
        visual.simulate_mouse_move(
            target,
            MouseButton::Left,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                if midi {
                    view.midi_editor
                        .active_trim_drag
                        .as_ref()
                        .is_some_and(|drag| drag.preview.borrow().is_some())
                } else {
                    view.interaction
                        .active_trim_drag
                        .as_ref()
                        .is_some_and(|drag| drag.preview.borrow().is_some())
                },
                "a completed repaint retains the held resize preview"
            );
            assert_eq!(
                view.worker_requests_pending, 0,
                "a parameter read cannot add a pending-edit badge"
            );
        });
        visual.simulate_mouse_up(
            target,
            MouseButton::Left,
            Modifiers {
                platform: true,
                ..Default::default()
            },
        );
        visual.run_until_parked();
        editor.update(cx, |view, _| {
            assert!(
                view.test_probes.last_enqueued_edit.is_some(),
                "resize release commits the preview"
            );
            assert!(view.interaction.pending_trim.is_some());
            let pending = view.worker_requests_pending;
            let state = Arc::clone(view.state.as_ref().unwrap());
            let mut observation =
                super::super::arrangement_main_track_ui_test::completed_event(&state);
            observation.state = state;
            observation.plugin_parameter_observation = true;
            view.receive(observation);
            assert_eq!(
                view.worker_requests_pending, pending,
                "read completion cannot acknowledge the resize edit"
            );
            assert!(
                view.interaction.pending_trim.is_some(),
                "read completion retains the pending resize display"
            );
            let state = Arc::clone(view.state.as_ref().unwrap());
            let mut refused_edit =
                super::super::arrangement_main_track_ui_test::completed_event(&state);
            refused_edit.state = state;
            refused_edit.error = Some("fixture edit refused".into());
            view.receive(refused_edit);
            assert_eq!(view.worker_requests_pending, 0);
            assert!(
                view.interaction.pending_trim.is_none(),
                "a real edit terminal still retires resize custody"
            );
        });
        visual.update(|window, _| window.remove_window());
        visual.run_until_parked();
    }
}

#[gpui::test]
fn held_midi_move_submits_and_observes_exact_worker_audition(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    use std::time::{Duration, Instant};

    let mut project_editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut commands = Vec::new();
    let mut edit = |command: Command| {
        let result = project_editor.execute(command.clone()).unwrap();
        commands.push(command);
        result
    };
    let Created::Track(source) = edit(Command::AddTrack {
        name: "source".into(),
    }) else {
        panic!("track")
    };
    edit(Command::AddTrack {
        name: "target".into(),
    });
    let Created::Clip(clip) = edit(Command::PlaceMidiClip {
        track: source,
        start: BeatTime::ONE,
        length: BeatTime::from_quarters(2).unwrap(),
    }) else {
        panic!("clip")
    };
    edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 100,
    });
    let mut fixture =
        super::super::worker_ui_fixture::open_with_audition_readiness(cx, commands, true);
    let editor = fixture.editor.clone();
    let original = editor.read_with(cx, |view, _| {
        Arc::clone(&view.state.as_ref().unwrap().project)
    });
    editor.update(cx, |view, _| {
        view.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::PlaybackCommand::Play)
            .unwrap();
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let (_, destination) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
    let identity = editor.read_with(&visual, |view, _| {
        let identity = view.expected_clip_preview.expect("held target submitted");
        assert!(
            view.applied_clip_preview.is_none(),
            "submission is not a callback ACK"
        );
        identity
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.applied_clip_preview
                .is_some_and(|(revision, current)| {
                    current == identity
                        && view.audio_status().unwrap().applied_revision.load(Acquire) == revision
                })
        });
        if applied {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "held move did not reach the callback"
        );
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        assert!(view.interaction.active_clip_drag.is_some());
        assert_eq!(
            view.state.as_ref().unwrap().project.as_ref(),
            original.as_ref()
        );
        assert!(view.audio_pub.applied_project_revision.is_none());
    });
    // Change the prospective position while withholding callbacks. The
    // marker remains at the actual anchor and on its actual track.
    visual.simulate_mouse_move(
        point(destination.x + px(50.0), destination.y - px(44.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let (next_identity, marker_fraction) = editor.update(&mut visual, |view, cx| {
        let expected = view.expected_clip_preview.unwrap();
        assert_ne!(expected.position, identity.position);
        assert_ne!(expected.destination, identity.destination);
        let (track, frame) = view
            .audible_clip_move_anchor()
            .expect("actual A while B prepares");
        assert_eq!(Some(track), identity.destination.existing_track());
        let saved = view.applied_clip_preview;
        view.applied_clip_preview = saved.map(|(revision, actual)| (revision + 1, actual));
        assert!(
            view.audible_clip_move_anchor().is_none(),
            "another output revision is not audible evidence"
        );
        view.applied_clip_preview = saved.map(|(revision, mut actual)| {
            actual.context.project_generation += 1;
            (revision, actual)
        });
        assert!(
            view.audible_clip_move_anchor().is_none(),
            "another project cannot be displayed"
        );
        view.applied_clip_preview = saved;
        let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
        let fraction = viewport.fraction(frame).unwrap();
        cx.notify();
        (expected, fraction)
    });
    visual.run_until_parked();
    let marker = visual
        .debug_bounds("clip-audition-audible-marker")
        .expect("actual placement marker");
    assert!(visual.debug_bounds("clip-audition-audible-label").is_some());
    editor.read_with(&visual, |view, _| {
        let lane = view
            .timeline_caches
            .drag_lanes
            .get(&identity.destination.existing_track().unwrap())
            .unwrap();
        let expected_x = lane.left + lane.width * marker_fraction;
        assert!((f32::from(marker.left()) - expected_x).abs() < 2.0);
        assert!((f32::from(marker.top()) - lane.top).abs() < 2.0);
    });
    let saved_actual = editor.update(&mut visual, |view, cx| {
        let saved = view.applied_clip_preview.unwrap();
        let mut offscreen = saved.1;
        offscreen.position = hex_project::ClipGroupPosition::Audio(u64::MAX);
        view.applied_clip_preview = Some((saved.0, offscreen));
        cx.notify();
        saved
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("clip-audition-audible-marker")
            .is_none()
    );
    editor.update(&mut visual, |view, cx| {
        view.applied_clip_preview = Some(saved_actual);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("clip-audition-audible-marker")
            .is_some()
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = editor.update(&mut visual, |view, cx| {
            let changed = view.tick(cx);
            if changed {
                cx.notify();
            }
            view.applied_clip_preview
                .is_some_and(|(_, actual)| actual == next_identity)
        });
        if applied {
            break;
        }
        if Instant::now() >= deadline {
            editor.read_with(&visual, |view, _| {
                panic!("new placement did not reach callback: desired={:?}, expected={:?}, actual={:?}, status={:?}, error={:?}", next_identity, view.expected_clip_preview, view.applied_clip_preview, view.audio_status().map(|status| (status.applied_revision.load(Acquire), status.rejected_revision())), view.error);
            });
        }
        std::thread::yield_now();
    }
    visual.run_until_parked();
    let marker = visual
        .debug_bounds("clip-audition-audible-marker")
        .expect("applied B marker");
    editor.read_with(&visual, |view, _| {
        let (track, frame) = view.audible_clip_move_anchor().unwrap();
        assert_eq!(Some(track), next_identity.destination.existing_track());
        let lane = view.timeline_caches.drag_lanes.get(&track).unwrap();
        let viewport = view.visible_viewport(view.state.as_ref().unwrap().timeline_frames);
        let expected_x = lane.left + lane.width * viewport.fraction(frame).unwrap();
        assert!((f32::from(marker.left()) - expected_x).abs() < 2.0);
        assert!((f32::from(marker.top()) - lane.top).abs() < 2.0);
    });
    let identity = next_identity;
    editor.update(&mut visual, |view, cx| {
        let drag = view.interaction.active_clip_drag.as_ref().unwrap().clone();
        drag.copy_modifier.set(true);
        view.sync_held_clip_move_audition();
        let copy = view.expected_clip_preview.unwrap();
        assert_eq!(
            view.applied_clip_preview.map(|(_, applied)| applied),
            Some(identity),
            "the prior callback remains audible while the copy prepares"
        );
        assert_eq!(copy.gesture, identity.gesture);
        assert!(copy.sequence > identity.sequence);
        assert_eq!(
            view.interaction
                .clip_move_audition
                .as_ref()
                .unwrap()
                .target
                .as_ref()
                .unwrap()
                .mode,
            hex_session::service::ArrangementMoveMode::Copy
        );
        view.sync_held_clip_move_audition();
        assert_eq!(
            view.expected_clip_preview,
            Some(copy),
            "unchanged target coalesces"
        );
        drag.snap_modifier.set(true);
        view.sync_held_clip_move_audition();
        let alias = view.expected_clip_preview.unwrap();
        assert!(alias.sequence > copy.sequence);
        assert_eq!(
            view.interaction
                .clip_move_audition
                .as_ref()
                .unwrap()
                .target
                .as_ref()
                .unwrap()
                .mode,
            hex_session::service::ArrangementMoveMode::AliasCopy
        );
        drag.destination.set(None);
        assert!(
            view.sync_held_clip_move_audition(),
            "cancellation requests a repaint"
        );
        assert!(view.expected_clip_preview.is_none());
        assert!(view.applied_clip_preview.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("clip-audition-audible-marker")
            .is_none()
    );
    // Restore the real pointer location before releasing; the synthetic
    // destination cancellation above only changed the drag payload.
    visual.simulate_mouse_move(destination, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(destination, MouseButton::Left, Modifiers::default());
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let settled = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.pending_clip_move.is_none() && view.worker_requests_pending == 0
        });
        if settled {
            break;
        }
        assert!(Instant::now() < deadline, "move release did not settle");
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        assert_ne!(
            view.state.as_ref().unwrap().project.as_ref(),
            original.as_ref()
        );
        assert!(view.expected_clip_preview.is_none());
    });
    drop(visual);
    fixture.finish(cx);
}

#[gpui::test]
fn held_group_audition_reuses_all_selected_members(cx: &mut TestAppContext) {
    let mut project_editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut commands = Vec::new();
    let mut edit = |command: Command| {
        let result = project_editor.execute(command.clone()).unwrap();
        commands.push(command);
        result
    };
    let Created::Track(source) = edit(Command::AddTrack {
        name: "source".into(),
    }) else {
        panic!("track")
    };
    edit(Command::AddTrack {
        name: "target".into(),
    });
    let mut clips = Vec::new();
    for beat in [1, 4] {
        let Created::Clip(clip) = edit(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::from_quarters(beat).unwrap(),
            length: BeatTime::ONE,
        }) else {
            panic!("clip")
        };
        clips.push(clip);
    }
    let mut fixture =
        super::super::worker_ui_fixture::open_with_audition_readiness(cx, commands, true);
    let editor = fixture.editor.clone();
    editor.update(cx, |view, _| {
        view.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::PlaybackCommand::Play)
            .unwrap();
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    super::super::drag::group_drag_ui_test::click(&mut visual, clips[0], false);
    super::super::drag::group_drag_ui_test::click(&mut visual, clips[1], true);
    let selector = Box::leak(format!("clip-drag-surface-{}", clips[0].value()).into_boxed_str());
    let source_bounds = visual.debug_bounds(selector).unwrap();
    let down = point(
        source_bounds.center().x,
        source_bounds.top() + source_bounds.size.height / 4.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        point(
            down.x + px(90.0),
            source_bounds.origin.y + px(22.0) + px(44.0),
        ),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        view.sync_held_clip_move_audition();
        let previous = view
            .interaction
            .clip_move_audition
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(previous.clips.as_ref(), clips.as_slice());
        assert_eq!(previous.anchor, clips[0]);
        let drag = view.interaction.active_clip_drag.as_ref().unwrap().clone();
        drag.copy_modifier.set(true);
        view.sync_held_clip_move_audition();
        let next = view
            .interaction
            .clip_move_audition
            .as_ref()
            .unwrap()
            .target
            .as_ref()
            .unwrap();
        assert!(Arc::ptr_eq(&previous.clips, &next.clips));
        assert!(next.sequence > previous.sequence);
        assert_eq!(next.mode, hex_session::service::ArrangementMoveMode::Copy);
        assert!(view.retire_active_clip_drag());
        assert!(view.expected_clip_preview.is_none());
    });
    drop(visual);
    fixture.finish(cx);
}

/// Opt-in headless measurement of the real pointer-to-worker-to-callback path.
/// Callback buffers are pumped by the test, not by a physical audio device.
#[gpui::test]
#[ignore = "opt-in optimized held clip audition admission latency probe"]
fn held_clip_audition_admission_latency(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;
    use std::time::{Duration, Instant};

    let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut commands = Vec::new();
    let mut edit = |command: Command| {
        let result = model.execute(command.clone()).unwrap();
        commands.push(command);
        result
    };
    let device = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let Created::Track(track) = edit(Command::InsertTrackWithDevice {
        name: "instrument".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: device,
    }) else {
        panic!("track")
    };
    let Created::Clip(clip) = edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ONE,
        length: BeatTime::from_quarters(2).unwrap(),
    }) else {
        panic!("clip")
    };
    edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 100,
    });
    let mut fixture =
        super::super::worker_ui_fixture::open_with_audition_readiness(cx, commands, true);
    let editor = fixture.editor.clone();
    let original = editor.read_with(cx, |view, _| {
        Arc::clone(&view.state.as_ref().unwrap().project)
    });
    editor.update(cx, |view, _| {
        view.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::PlaybackCommand::Play)
            .unwrap();
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
    visual.run_until_parked();
    let (_, destination) = drag_source_to_target(cx, &mut visual, &editor, clip, 0, 90.0);
    let mut samples = Vec::new();
    let mut prior = editor.read_with(&visual, |view, _| view.expected_clip_preview.unwrap());
    for iteration in 0..16 {
        let x = destination.x + px(if iteration % 2 == 0 { 50.0 } else { 110.0 });
        let started = Instant::now();
        visual.simulate_mouse_move(
            point(x, destination.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let target = editor.read_with(&visual, |view, _| view.expected_clip_preview.unwrap());
        assert_eq!(target.gesture, prior.gesture);
        assert!(
            target.sequence > prior.sequence,
            "each pointer position must change the target"
        );
        let deadline = started + Duration::from_secs(3);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            let applied = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.applied_clip_preview.is_some_and(|(revision, actual)| {
                    actual == target
                        && view.audio_status().unwrap().applied_revision.load(Acquire) == revision
                })
            });
            if applied {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "target did not reach the callback: {target:?}"
            );
            std::thread::yield_now();
        }
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if iteration >= 4 {
            samples.push(elapsed);
        }
        prior = target;
    }
    editor.read_with(&visual, |view, _| {
        assert!(view.interaction.active_clip_drag.is_some());
        assert_eq!(
            view.state.as_ref().unwrap().project.as_ref(),
            original.as_ref()
        );
    });
    let mut ordered = samples.clone();
    ordered.sort_by(f64::total_cmp);
    println!(
        "held_clip_audition_ms samples={samples:?} median={} p95_nearest_rank={} max={} warmups=4 measured=12 sample_rate=48000 block_frames=64 tracks=1 midi_clips=1 notes=1 window=1280x800 backend=headless_pumped",
        (ordered[ordered.len() / 2 - 1] + ordered[ordered.len() / 2]) / 2.0,
        ordered[(ordered.len() * 95).div_ceil(100) - 1],
        ordered[ordered.len() - 1]
    );
    editor.update(&mut visual, |view, _| {
        view.retire_active_clip_drag();
    });
    drop(visual);
    fixture.finish(cx);
}

#[gpui::test]
fn held_new_track_audition_uses_captured_defaults_for_release_and_undo(cx: &mut TestAppContext) {
    use std::time::{Duration, Instant};

    for (alt, platform) in [(false, false), (true, false), (true, true)] {
        let mut project = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut commands = Vec::new();
        let mut edit = |command: Command| {
            let result = project.execute(command.clone()).unwrap();
            commands.push(command);
            result
        };
        let Created::Track(source) = edit(Command::AddTrack {
            name: "source".into(),
        }) else {
            panic!("track")
        };
        let Created::Track(second) = edit(Command::AddTrack {
            name: "second".into(),
        }) else {
            panic!("track")
        };
        let Created::Clip(clip) = edit(Command::PlaceMidiClip {
            track: source,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(2).unwrap(),
        }) else {
            panic!("clip")
        };
        edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        });
        let mut fixture =
            super::super::worker_ui_fixture::open_with_audition_readiness(cx, commands, true);
        let editor = fixture.editor.clone();
        editor.update(cx, |view, _| {
            view.settings
                .track_defaults
                .set_test_state(0.5, false, true);
            view.audio
                .as_mut()
                .unwrap()
                .control
                .command(hex_engine::PlaybackCommand::Play)
                .unwrap();
        });
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let mut visual = VisualTestContext::from_window(fixture.handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            view.settings
                .track_defaults
                .set_test_state(0.5, false, true);
        });
        let original = editor.read_with(&visual, |view, _| {
            Arc::clone(&view.state.as_ref().unwrap().project)
        });
        let surface = visual
            .debug_bounds(Box::leak(
                format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", source.value()).into_boxed_str(),
            ))
            .unwrap();
        let down = surface.center();
        let modifiers = Modifiers {
            alt,
            platform,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.run_until_parked();
        let bounds = editor.read_with(&visual, |view, _| {
            view.timeline_caches.arrangement_bounds.unwrap()
        });
        let mut target = point(
            lane.origin.x + lane.size.width * 0.73,
            px(bounds.top + bounds.height - 8.0),
        );
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let identity = editor.update(&mut visual, |view, _| {
            view.sync_held_clip_move_audition();
            let identity = view.expected_clip_preview.expect("new-track target");
            assert!(matches!(
                identity.destination,
                hex_session::service::ClipMovePreviewDestination::NewTrack { before_track: None }
            ));
            let expected_mode = match (alt, platform) {
                (true, true) => hex_session::service::ArrangementMoveMode::AliasCopy,
                (true, false) => hex_session::service::ArrangementMoveMode::Copy,
                _ => hex_session::service::ArrangementMoveMode::Move,
            };
            assert_eq!(identity.mode, expected_mode);
            assert_eq!(
                view.state.as_ref().unwrap().project.as_ref(),
                original.as_ref()
            );
            let drag = view.interaction.active_clip_drag.as_ref().unwrap().clone();
            let destination = drag.destination.get();
            drag.destination.set(None);
            assert!(view.sync_held_clip_move_audition());
            assert!(view.expected_clip_preview.is_none());
            drag.destination.set(destination);
            assert!(view.sync_held_clip_move_audition());
            let identity = view.expected_clip_preview.unwrap();
            view.settings
                .track_defaults
                .set_test_state(0.25, false, true);
            identity
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("new-track-audition-audible-marker")
                .is_none()
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if editor.update(&mut visual, |view, cx| {
                if view.tick(cx) {
                    cx.notify();
                }
                view.applied_clip_preview
                    .is_some_and(|(_, actual)| actual == identity)
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "new-track preview did not reach callback"
            );
            std::thread::yield_now();
        }
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("new-track-audition-audible-marker")
                .is_some()
        );
        let actual_a = editor.read_with(&visual, |view, _| {
            view.new_track_audible_marker()
                .expect("callback-applied new-track marker")
        });
        let saved = editor.update(&mut visual, |view, cx| {
            let saved = view.applied_clip_preview.unwrap();
            view.applied_clip_preview = Some((saved.0 + 1, saved.1));
            cx.notify();
            saved
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("new-track-audition-audible-marker")
                .is_none()
        );
        editor.update(&mut visual, |view, cx| {
            let mut wrong_source = saved.1;
            wrong_source.context.project_generation += 1;
            view.applied_clip_preview = Some((saved.0, wrong_source));
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("new-track-audition-audible-marker")
                .is_none()
        );
        editor.update(&mut visual, |view, cx| {
            view.applied_clip_preview = Some(saved);
            cx.notify();
        });
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            let epoch = view
                .timeline_caches
                .drag_lane_epochs
                .remove(&second)
                .unwrap();
            assert!(view.new_track_audible_marker().is_none());
            view.timeline_caches.drag_lane_epochs.insert(second, epoch);
        });
        let second_lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", second.value()).into_boxed_str(),
            ))
            .unwrap();
        target = point(target.x + px(40.0), second_lane.origin.y);
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let next = editor.update(&mut visual, |view, cx| {
            view.sync_held_clip_move_audition();
            let next = view.expected_clip_preview.unwrap();
            assert_ne!(next, identity);
            assert!(matches!(next.destination, hex_session::service::ClipMovePreviewDestination::NewTrack { before_track: Some(track) } if track == second));
            cx.notify();
            next
        });
        visual.run_until_parked();
        assert_eq!(
            editor.read_with(&visual, |view, _| view.new_track_audible_marker()),
            Some(actual_a)
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if editor.update(&mut visual, |view, cx| {
                if view.tick(cx) {
                    cx.notify();
                }
                view.applied_clip_preview
                    .is_some_and(|(_, actual)| actual == next)
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "second new-track preview did not reach callback"
            );
            std::thread::yield_now();
        }
        visual.run_until_parked();
        let actual_b = editor.read_with(&visual, |view, _| {
            view.new_track_audible_marker()
                .expect("updated callback-applied marker")
        });
        assert_ne!(actual_a, actual_b);
        let destination = editor.update(&mut visual, |view, cx| {
            let drag = view.interaction.active_clip_drag.as_ref().unwrap().clone();
            let destination = drag.destination.get();
            drag.destination.set(None);
            assert!(view.sync_held_clip_move_audition());
            cx.notify();
            destination
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("new-track-audition-audible-marker")
                .is_none()
        );
        editor.update(&mut visual, |view, cx| {
            view.interaction
                .active_clip_drag
                .as_ref()
                .unwrap()
                .destination
                .set(destination);
            assert!(view.sync_held_clip_move_audition());
            cx.notify();
        });
        visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.pending_clip_move.is_none() && view.worker_requests_pending == 0
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "new-track release did not settle"
            );
            std::thread::yield_now();
        }
        editor.update(&mut visual, |view, _| {
            let result = &view.state.as_ref().unwrap().project;
            assert_eq!(
                result.tracks().len(),
                original.tracks().len() + 1,
                "modifiers={modifiers:?}, error={:?}, last edit={:?}, pending={}, queued={}",
                view.error,
                view.test_probes.last_enqueued_edit,
                view.worker_requests_pending,
                view.pending_requests.len()
            );
            assert_eq!(result.tracks()[1].gain(), 0.5);
            assert!(view.enqueue(hex_session::service::Request::Undo));
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
            if editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.worker_requests_pending == 0
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "undo did not settle");
            std::thread::yield_now();
        }
        editor.read_with(&visual, |view, _| {
            let result = &view.state.as_ref().unwrap().project;
            assert_eq!(result.tracks(), original.tracks());
        });
        drop(visual);
        fixture.finish(cx);
    }
}

#[gpui::test]
fn typed_drag_factory_uses_original_press_and_refuses_gaps(cx: &mut TestAppContext) {
    use gpui::{Context, IntoElement, Pixels, Point, Render, Window, div, prelude::*};
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Source {
        clip: u64,
        pressed_at: Point<Pixels>,
    }

    struct Ghost;
    impl Render for Ghost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size(px(1.0))
        }
    }

    struct Fixture {
        created: Rc<Cell<Option<Source>>>,
        dropped: Rc<Cell<Option<Source>>>,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let created = self.created.clone();
            let dropped = self.dropped.clone();
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .top(px(10.0))
                        .left(px(10.0))
                        .w(px(220.0))
                        .h(px(14.0))
                        .id("drag-lane")
                        .on_drag_from_press(move |press, _, _, cx| {
                            if press.position.x < px(10.0) || press.position.x >= px(30.0) {
                                return None;
                            }
                            let source = Source {
                                clip: 27,
                                pressed_at: press.position,
                            };
                            created.set(Some(source));
                            Some((source, cx.new(|_| Ghost)))
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(160.0))
                        .top(px(30.0))
                        .w(px(90.0))
                        .h(px(34.0))
                        .on_drop(move |source: &Source, _, cx| {
                            dropped.set(Some(*source));
                            cx.stop_propagation();
                        }),
                )
        }
    }

    let created = Rc::new(Cell::new(None));
    let dropped = Rc::new(Cell::new(None));
    let window = *cx.add_window(|_, _| Fixture {
        created: created.clone(),
        dropped: dropped.clone(),
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.simulate_resize(gpui::size(px(300.0), px(100.0)));
    visual.run_until_parked();

    let clip = point(px(19.0), px(15.0));
    let gap = point(px(121.0), px(15.0));
    let drop_at = point(px(201.0), px(43.0));
    for (start, expected) in [
        (gap, None),
        (
            clip,
            Some(Source {
                clip: 27,
                pressed_at: clip,
            }),
        ),
    ] {
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(start.x + px(0.5), start.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        assert_eq!(created.get(), None);
        visual.simulate_mouse_move(drop_at, MouseButton::Left, Modifiers::default());
        assert_eq!(created.get(), expected);
        assert_eq!(
            visual.update(|_, cx| cx.has_active_drag()),
            expected.is_some()
        );

        // The receiver handles release before an intervening repaint.
        visual.simulate_mouse_up(drop_at, MouseButton::Left, Modifiers::default());
        assert_eq!(dropped.get(), expected);
        assert!(!visual.update(|_, cx| cx.has_active_drag()));
        visual.run_until_parked();
    }
}
