use super::*;

#[gpui::test]
fn arrangement_scroll_rebuilds_drag_lane_eligibility_from_painted_rows(cx: &mut TestAppContext) {
    let mut project_editor =
        Editor::new(Project::new(48_000).expect("project"), 32).expect("editor model");
    for index in 0..24 {
        project_editor
            .execute(Command::AddTrack {
                name: format!("track {index}"),
            })
            .expect("add track");
    }
    let first_track = project_editor.project().tracks()[0].id();
    let last_track = project_editor.project().tracks()[23].id();
    let (handle, editor) = editor_window(cx, project_editor.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();

    let arrangement = visual
        .debug_bounds("arrangement-content")
        .expect("painted arrangement body");
    let pan_start = point(
        arrangement.origin.x + px(500.0),
        arrangement.origin.y + px(250.0),
    );
    visual.simulate_mouse_down(pan_start, gpui::MouseButton::Middle, Default::default());
    visual.simulate_mouse_move(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        gpui::MouseButton::Middle,
        Default::default(),
    );
    visual.simulate_mouse_up(
        point(pan_start.x, pan_start.y - px(1_100.0)),
        gpui::MouseButton::Middle,
        Default::default(),
    );
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        let generation = view.timeline_caches.drag_layout_generation;
        let painted = view
            .timeline_caches
            .drag_lane_epochs
            .iter()
            .filter_map(|(track, epoch)| (*epoch == generation).then_some(*track))
            .collect::<Vec<_>>();
        assert!(
            painted.contains(&last_track),
            "last row should be painted after scroll"
        );
        assert!(
            !painted.contains(&first_track),
            "offscreen first row must not be eligible in the new paint generation"
        );
        let target_lane = view.timeline_caches.drag_lanes[&last_track];
        let pointer_x = target_lane.left + target_lane.width / 2.0;
        let pointer_y = target_lane.top + target_lane.height / 2.0;
        assert!(view.timeline_caches.drag_lane_epochs[&first_track] < generation);
        // Recreate the dangerous stale-cache collision: the old offscreen track
        // still has bounds overlapping a currently painted row.
        view.timeline_caches
            .drag_lanes
            .insert(first_track, target_lane);
        assert_eq!(
            view.hovered_drag_target(pointer_x, pointer_y)
                .map(|(track, _)| track),
            Some(last_track),
            "stale offscreen lane geometry cannot win over the currently painted row"
        );
    });
}

#[gpui::test]
fn shift_edge_stretches_a_long_warped_clip_to_the_painted_endpoint(cx: &mut TestAppContext) {
    check_long_stretch_to_painted_endpoint(cx, Some(hex_project::WarpMode::Tones { grain_ms: 32 }));
}

#[gpui::test]
fn shift_edge_stretches_a_fresh_long_clip_to_the_painted_endpoint(cx: &mut TestAppContext) {
    check_long_stretch_to_painted_endpoint(cx, None);
}

#[gpui::test]
fn shift_edge_stretch_escape_and_focus_loss_cancel_without_release_edit(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    for cancel in [0, 1, 2] {
        let (project, _, _, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let selector = Box::leak(format!("clip-trim-right-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        let target = point(down.x - px(42.0), down.y);
        visual.simulate_mouse_move(point(down.x - px(12.0), down.y), MouseButton::Left, shift);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.interaction.active_trim_drag.is_some())
        });
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| {
                assert!(
                    view.focus.is_focused(window),
                    "root focus before cancellation {cancel}"
                )
            })
        });
        if cancel == 1 {
            visual.update(|window, cx| {
                editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
            });
        } else if cancel == 2 {
            visual.deactivate_window();
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                view.interaction.active_trim_drag.is_none(),
                "Escape/focus loss clears metadata preview: lose_focus={cancel}"
            )
        });
        // Inactive windows retain their last painted frame. Reactivate and
        // return focus before inspecting the repainted overlay and re-entry.
        visual.update(|window, cx| {
            window.activate_window();
            editor.read_with(cx, |view, _| view.focus.focus(window));
        });
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("trim-live-preview").is_none(),
            "repaint after cancel {cancel}"
        );
        visual.simulate_mouse_move(
            point(target.x - px(10.0), target.y),
            MouseButton::Left,
            shift,
        );
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(
                view.interaction.active_trim_drag.is_none(),
                "canceled payload cannot re-enter"
            )
        });
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        editor.read_with(cx, |view, _| {
            assert!(view.test_probes.last_enqueued_edit.is_none())
        });
    }
}

#[gpui::test]
fn upper_rail_fixture_same_root_survives_actual_catalog_job_and_view_drop(cx: &mut TestAppContext) {
    use std::{sync::atomic::Ordering, time::Instant};
    let (files, handle, editor) = upper_rail_owned_fixture(cx);
    let root = files.root().to_owned();
    let weak = Arc::downgrade(&files);
    let gate = files.gate_next_catalog_save();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        gate.reached.load(Ordering::Acquire),
        "actual catalog worker reached save gate"
    );
    assert!(!gate.finished.load(Ordering::Acquire));
    let catalog = editor.read_with(&visual, |view, _| {
        assert!(Arc::ptr_eq(&files, &view.test_environment));
        view.browser
            .test_preferences_path()
            .with_extension("samples-v1.db")
    });
    assert_eq!(catalog, root.join("config/browser.samples-v1.db"));
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| Empty);
    });
    let editor_weak = editor.downgrade();
    drop(editor);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(
        editor_weak.upgrade().is_none(),
        "real view retired before save continuation"
    );
    drop(files);
    assert_eq!(
        weak.strong_count(),
        1,
        "only actual paused filesystem job retains fixture root"
    );
    assert!(root.is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(weak.upgrade().is_none(), "no leaked fixture/view/job Arc");
    let until = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            Instant::now() < until,
            "owned background cleanup timed out: {root:?}"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(!root.exists(), "completed job cannot recreate retired root");
    eprintln!(
        "upper_rail_fixture_custody actual_save=true same_root=true all_four_preferences=true view_retired=true cleaned=true"
    );
}

#[test]
fn upper_rail_fixture_pre_ack_real_import_refusal_preserves_disk_without_arc_leak() {
    use hex_session::service::Request;
    use std::time::Duration;
    let files = super::super::test_environment::Environment::new().unwrap();
    let root = files.root().to_owned();
    let weak = Arc::downgrade(&files);
    let observed = std::cell::Cell::new(false);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut setup = UpperRailPendingSetup {
            environment: files,
            acknowledged: false,
        };
        let missing = root.join("missing.wav");
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        upper_rail_next(&service);
        service.submit(Request::Import(missing.clone())).unwrap();
        let until = crate::test_wait::hang_deadline();
        let event = loop {
            if let Ok(event) = service.try_event() {
                break event;
            }
            assert!(
                std::time::Instant::now() < until,
                "upper_rail_fixture_pre_ack_real_import_refusal_preserves_disk_without_arc_leak: hang guard expired"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(event.request_complete);
        assert!(event.error.is_some(), "actual missing-file import refused");
        observed.set(true);
        setup.acknowledge_import(&event, &missing); // Must unwind before disarming.
    }));
    assert!(caught.is_err());
    assert!(
        observed.get(),
        "actual response observed, not a timeout panic"
    );
    assert!(
        weak.upgrade().is_none(),
        "disk evidence retention does not leak Arc"
    );
    assert!(root.is_dir());
    eprintln!(
        "upper_rail_fixture_pre_ack actual_refusal=true disk_retained=true weak_expired=true root={root:?}"
    );
    // Only this artificial case proved its sole request finished, with no UI jobs.
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn rendered_move_audio_horizontal_handoff(cx: &mut TestAppContext) {
    assert_move_handoff(cx, &[false], false);
}

#[gpui::test]
fn rendered_move_midi_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[true], vertical);
    }
}

#[gpui::test]
fn rendered_move_audio_group_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[false, false], vertical);
    }
}

#[gpui::test]
fn rendered_move_midi_group_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[true, true], vertical);
    }
}

#[gpui::test]
fn rendered_move_mixed_group_handoff(cx: &mut TestAppContext) {
    for vertical in [false, true] {
        assert_move_handoff(cx, &[false, true], vertical);
    }
}

#[gpui::test]
fn rendered_copy_retains_destination_until_real_worker_ack(cx: &mut TestAppContext) {
    for midi in [&[false][..], &[true][..], &[false, true][..]] {
        assert_drag_handoff(cx, midi, true, true);
    }
}

#[gpui::test]
fn rendered_copy_survives_same_revision_worker_work(cx: &mut TestAppContext) {
    for midi in [&[false][..], &[true][..], &[false, true][..]] {
        assert_drag_handoff_with_pending_request(cx, midi, false, true, true);
    }
}

#[gpui::test]
fn rendered_move_released_after_first_drag_motion_commits(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    for midi in [false, true] {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        let (track, audio) = editor.read_with(&visual, |v, _| {
            let track = &v.state.as_ref().unwrap().project.tracks()[0];
            (track.id(), track.clips()[0].id())
        });
        if midi {
            for command in [
                Command::RemoveClip(audio),
                Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::from_quarters(4).unwrap(),
                },
            ] {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(command)))
                });
                pump_move(&editor, &mut visual);
            }
        }
        let (clip, before) = editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            (
                if midi {
                    project.track(track).unwrap().midi_clips()[0].id()
                } else {
                    audio
                },
                project.clone(),
            )
        });
        editor.update(&mut visual, |view, cx| {
            // Exercise dragging a clip whose header has never selected it.
            view.selected_clip = None;
            view.reconcile_clip_selection();
            cx.notify();
        });
        visual.run_until_parked();
        let source = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap();
        let down = source.center();
        let end = point(down.x + px(36.0), down.y);
        assert!(
            source.contains(&end),
            "release still hits the painted source header"
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let continuations =
            editor.read_with(&visual, |view, _| view.test_probes.drag_move_event_count);
        // One coalesced motion crosses the native drag threshold. There need
        // not be a second mouse-move before release over the original header.
        let destination = visual.update(|window, app| {
            window.dispatch_input(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: end,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                app,
            );
            assert!(app.has_active_drag());
            let destination = {
                let view = editor.read(app);
                assert_eq!(
                    view.test_probes.drag_move_event_count, continuations,
                    "the first motion prepares the drop during typed admission"
                );
                let drag = view.interaction.active_clip_drag.as_ref().unwrap();
                let destination = drag
                    .preview
                    .get()
                    .expect("valid destination preview")
                    .position;
                assert_ne!(destination, drag.position);
                destination
            };
            // Dispatch against the same completed frame, as native events can
            // arrive between display refreshes.
            window.dispatch_input(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    position: end,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                app,
            );
            destination
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.worker_requests_pending, 1,
                "first-motion release must submit the move: {:?}",
                v.error
            );
        });
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_clip_ids(), vec![clip]);
            let project = &v.state.as_ref().unwrap().project;
            assert_ne!(project.snapshot(), before.snapshot());
            match destination {
                super::super::drag::ClipPosition::Midi { start, .. } => {
                    assert_eq!(project.midi_clip(clip).unwrap().start(), start)
                }
                super::super::drag::ClipPosition::Audio { start, .. } => {
                    assert_eq!(project.clip(clip).unwrap().start(), start)
                }
            }
        });
        let selected_edge = selector(format!(
            "{}clip-selected-trailing-{}",
            if midi { "midi-" } else { "" },
            clip.value()
        ));
        assert!(visual.debug_bounds(selected_edge).is_some());
        let moved = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("cmd-z");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before.snapshot()
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), moved)
        });
    }
}

#[gpui::test]
fn initial_left_third_keeps_the_move_start_on_grid(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(cx, &[midi], false, false, false, Some(1.0 / 3.0), None);
    }
}

#[gpui::test]
fn initial_two_thirds_boundary_snaps_the_move_end(cx: &mut TestAppContext) {
    for midi in [false, true] {
        assert_drag_handoff_at_grab(cx, &[midi], false, false, false, Some(2.0 / 3.0), None);
    }
}

#[gpui::test]
fn initial_right_third_retains_one_snap_delta_for_mixed_group_copy(cx: &mut TestAppContext) {
    assert_drag_handoff_at_grab(cx, &[false, true], true, true, false, Some(5.0 / 6.0), None);
}

#[gpui::test]
fn cropped_clip_move_chooses_from_the_full_audio_and_midi_span(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    for midi in [false, true] {
        let (project, source, target, clip) = if midi {
            midi_project()
        } else {
            audio_project()
        };
        let (start, end) = if midi {
            let clip = project.midi_clip(clip).unwrap();
            (
                project.tempo_map().beat_to_frame(clip.start()).unwrap(),
                project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(clip.start().raw() + clip.length().raw()))
                    .unwrap(),
            )
        } else {
            let clip = project.clip(clip).unwrap();
            (clip.start(), clip.start() + clip.duration())
        };
        let threshold = start + (end - start) * 2 / 3;
        let before = project.snapshot();
        let preview = if midi {
            Arc::new(
                hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                    .unwrap()
                    .1,
            )
        } else {
            Arc::default()
        };
        let (handle, editor) = editor_window(cx, project, preview);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(&mut visual, |view, cx| {
            view.viewport = Some(super::super::viewport::Viewport {
                start: threshold,
                length: TIMELINE_FRAMES - threshold,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let surface = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap();
        let down = point(
            surface.left() + surface.size.width / 4.0,
            surface.center().y,
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let lane = editor.read_with(&visual, |view, _| view.timeline_caches.drag_lanes[&target]);
        visual.simulate_mouse_move(
            point(down.x + px(80.0), px(lane.top + lane.height / 2.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            let drag = view.interaction.active_clip_drag.as_ref().unwrap();
            assert_eq!(drag.source_track, source);
            assert_eq!(
                drag.snap_edge.get(),
                Some(super::super::drag::ClipSnapEdge::Trailing),
                "the left quarter of this visible fragment is already in the real trailing third"
            );
        });
        visual.simulate_keystrokes("escape");
        editor.read_with(&visual, |view, _| {
            assert!(view.interaction.active_clip_drag.is_none());
            assert!(view.test_probes.last_enqueued_edit.is_none());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}
