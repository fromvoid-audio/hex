use super::*;

#[gpui::test]
fn new_track_clip_release_keeps_preview_until_worker_accepts_or_refuses(cx: &mut TestAppContext) {
    use hex_session::service::Request;
    for midi in [false, true] {
        for copy in [false, true] {
            for refuse in [false, true] {
                let (_files, handle, editor) = upper_rail_owned_fixture(cx);
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
                visual.run_until_parked();
                editor.update(&mut visual, |view, _| {
                    view.settings
                        .track_defaults
                        .set_test_state(0.5, false, true)
                });
                let (source, anchor, audio) = editor.read_with(&visual, |view, _| {
                    let project = &view.state.as_ref().unwrap().project;
                    (
                        project.tracks()[0].id(),
                        project.tracks()[1].id(),
                        project.tracks()[0].clips()[0].id(),
                    )
                });
                let clip = if midi {
                    editor.update(&mut visual, |view, _| {
                        assert!(view.enqueue(Request::Edit(Command::RemoveClip(audio))))
                    });
                    pump_move(&editor, &mut visual);
                    editor.update(&mut visual, |view, _| {
                        assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                            track: source,
                            start: BeatTime::ZERO,
                            length: BeatTime::ONE,
                        })))
                    });
                    pump_move(&editor, &mut visual);
                    editor.read_with(&visual, |view, _| {
                        view.state
                            .as_ref()
                            .unwrap()
                            .project
                            .track(source)
                            .unwrap()
                            .midi_clips()[0]
                            .id()
                    })
                } else {
                    audio
                };
                let before = editor.read_with(&visual, |view, _| {
                    Arc::clone(&view.state.as_ref().unwrap().project)
                });
                if refuse {
                    // Advance only the real worker, leaving the displayed snapshot stale.
                    // The captured insertion anchor is then rejected by the worker.
                    editor.update(&mut visual, |view, _| {
                        view.service
                            .submit(Request::Edit(Command::RemoveTrack(anchor)))
                            .unwrap();
                        upper_rail_next(&view.service);
                    });
                }
                drag_source_to_inserted_track(
                    cx,
                    &mut visual,
                    &editor,
                    &before,
                    clip,
                    source,
                    Some(anchor),
                    copy,
                );
                let (old_cursor, drop_frame) = editor.read_with(&visual, |view, _| {
                    (
                        view.interaction
                            .edit_cursor
                            .map(|cursor| (cursor.track, cursor.frame)),
                        view.pending_clip_move
                            .as_ref()
                            .unwrap()
                            .drag
                            .cursor_position()
                            .unwrap()
                            .1,
                    )
                });
                editor.update(&mut visual, |view, _| {
                    view.settings
                        .track_defaults
                        .set_test_state(0.25, false, true)
                });
                if refuse {
                    editor.update(&mut visual, |view, cx| {
                        let event = upper_rail_terminal(&view.service);
                        assert!(event.error.is_some());
                        view.receive(event);
                        assert!(view.pending_clip_move.is_none());
                        assert_eq!(
                            view.interaction
                                .edit_cursor
                                .map(|cursor| (cursor.track, cursor.frame)),
                            old_cursor
                        );
                        assert!(view.state.as_ref().unwrap().project.track(anchor).is_none());
                        cx.notify();
                    });
                } else {
                    pump_move(&editor, &mut visual);
                    editor.read_with(&visual, |view, _| {
                        assert!(view.pending_clip_move.is_none());
                        assert_eq!(
                            view.state.as_ref().unwrap().project.tracks().len(),
                            before.tracks().len() + 1
                        );
                        let destination = &view.state.as_ref().unwrap().project.tracks()[1];
                        assert_eq!(destination.gain(), 0.5);
                        if !copy {
                            assert_eq!(view.selected_clip(), Some(clip));
                            assert_eq!(view.selected_track(), Some(destination.id()));
                        }
                        let cursor = view.interaction.edit_cursor.unwrap();
                        assert_eq!(
                            (cursor.track, cursor.frame),
                            (Some(destination.id()), drop_frame)
                        );
                    });
                }
                visual.run_until_parked();
                assert!(visual.debug_bounds("track-insert-preview").is_none());
                assert!(visual.debug_bounds("clip-move-guide-line").is_none());
            }
        }
    }
}

#[gpui::test]
fn new_track_clip_drop_refuses_unavailable_defaults_without_blocking_existing_tracks(
    cx: &mut TestAppContext,
) {
    for loading in [true, false] {
        let (project, _, anchor, clip) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            view.settings
                .track_defaults
                .set_test_state(0.5, loading, false)
        });
        let down = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap()
            .center();
        let lane = visual
            .debug_bounds(selector(format!("arrangement-lane-{}", anchor.value())))
            .unwrap();
        let target = point(
            lane.origin.x + lane.size.width * 0.7,
            lane.origin.y + px(1.0),
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(view.error.as_deref().unwrap().contains(if loading {
                "loading"
            } else {
                "open settings"
            }));
            assert!(view.test_probes.last_enqueued_edit.is_none());
            assert!(view.pending_clip_move.is_none());
        });
        editor.update(&mut visual, |view, _| view.error = None);
        let (_, target) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert!(matches!(
                view.test_probes.last_enqueued_edit,
                Some(Command::MoveClipWithOverwrite { .. })
            ));
        });
    }
}

#[gpui::test]
fn linked_clip_new_track_drop_captures_gain_and_retains_link(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let (_files, handle, editor) = upper_rail_owned_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(px(1280.0), px(800.0)));
        visual.run_until_parked();
        editor.update(&mut visual, |view, _| {
            view.settings
                .track_defaults
                .set_test_state(0.5, false, true)
        });
        let (source, anchor, audio) = editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            (
                p.tracks()[0].id(),
                p.tracks()[1].id(),
                p.tracks()[0].clips()[0].id(),
            )
        });
        let clip = if midi {
            editor.update(&mut visual, |view, _| {
                assert!(
                    view.enqueue(hex_session::service::Request::Edit(Command::RemoveClip(
                        audio
                    )))
                )
            });
            pump_move(&editor, &mut visual);
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(hex_session::service::Request::Edit(
                    Command::PlaceMidiClip {
                        track: source,
                        start: BeatTime::ZERO,
                        length: BeatTime::ONE
                    }
                )))
            });
            pump_move(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(source)
                    .unwrap()
                    .midi_clips()[0]
                    .id()
            })
        } else {
            audio
        };
        let source_bounds = visual
            .debug_bounds(selector(format!("clip-drag-surface-{}", clip.value())))
            .unwrap();
        let lane = visual
            .debug_bounds(selector(format!("arrangement-lane-{}", anchor.value())))
            .unwrap();
        let down = source_bounds.center();
        let target = point(
            lane.origin.x + lane.size.width * 0.7,
            lane.origin.y + px(1.0),
        );
        let modifiers = Modifiers {
            alt: true,
            platform: true,
            ..Default::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
        editor.read_with(&visual, |view, _| {
            assert!(matches!(view.test_probes.last_enqueued_edit.as_ref(), Some(Command::DuplicateAliasClipToNewTrack { initial_gain: Some(gain), .. }) if *gain == 0.5));
            assert!(view.pending_clip_move.is_some());
        });
        editor.update(&mut visual, |view, _| {
            view.settings
                .track_defaults
                .set_test_state(0.25, false, true)
        });
        pump_move(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            let target = &p.tracks()[1];
            assert_eq!(target.gain(), 0.5);
            let linked = if midi {
                target.midi_clips()[0].id()
            } else {
                target.clips()[0].id()
            };
            assert_eq!(p.clip_alias(clip), p.clip_alias(linked));
            assert!(view.pending_clip_move.is_none());
        });
    }
}

#[gpui::test]
fn rendered_clip_drop_highlights_only_occupied_audio_and_midi_spans(cx: &mut TestAppContext) {
    let (project, _, target, clip) = audio_project();
    let mut model = Editor::new(project, 8).expect("model");
    model
        .execute(Command::CopyClipTo {
            clip,
            track: target,
            start: 0,
        })
        .expect("audio victim");
    model
        .execute(Command::PlaceMidiClip {
            track: target,
            start: BeatTime::from_quarters(5).unwrap(),
            length: BeatTime::ONE,
        })
        .expect("midi victim");
    let project = model.project().clone();
    let media = BTreeMap::from([(
        project.clip(clip).unwrap().asset(),
        hex_engine::Audio::new(48_000, vec![[0.0; 2]; 96_000]).unwrap(),
    )]);
    let (_, midi) =
        hex_engine::compile::project_with_midi_preview(&project, &media).expect("compile previews");
    let (handle, editor) = editor_window(cx, project, Arc::new(midi));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .unwrap();
    let delta = f32::from(lane.size.width) * 48_000.0 / TIMELINE_FRAMES as f32;
    let (_, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, delta);
    let (start, end) = editor.read_with(cx, |view, _| {
        let preview = view
            .interaction
            .active_clip_drag
            .as_ref()
            .unwrap()
            .preview
            .get()
            .unwrap();
        (preview.start_frame, preview.end_frame)
    });
    for (index, (victim_start, victim_end)) in [(0_u64, 96_000_u64), (120_000, 144_000)]
        .into_iter()
        .enumerate()
    {
        let overlap = visual
            .debug_bounds(Box::leak(
                format!("clip-drop-overlap-{index}").into_boxed_str(),
            ))
            .unwrap_or_else(|| {
                panic!("occupied span {index} highlighted for landing {start}..{end}")
            });
        let expected_left = lane.origin.x
            + lane.size.width * (start.max(victim_start) as f32 / TIMELINE_FRAMES as f32);
        let expected_width = lane.size.width
            * ((end.min(victim_end) - start.max(victim_start)) as f32 / TIMELINE_FRAMES as f32);
        assert!((overlap.origin.x - expected_left).abs() <= px(1.0));
        assert!((overlap.size.width - expected_width).abs() <= px(1.0));
    }
    assert!(visual.debug_bounds("clip-drop-overlap-2").is_none());
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("clip-drop-overlap-0").is_none(),
        "release clears highlights"
    );
}

#[gpui::test]
fn rendered_clip_drop_excludes_moved_source_but_highlights_copied_source(cx: &mut TestAppContext) {
    let (project, _, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 0, 0.0);
    assert!(visual.debug_bounds("clip-drop-overlap-0").is_none());
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(option);
    visual.run_until_parked();
    let overlap = visual
        .debug_bounds("clip-drop-overlap-0")
        .expect("copy intersects original");
    let preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert!((overlap.origin.x - preview.origin.x).abs() <= px(1.0));
    assert!((overlap.size.width - preview.size.width).abs() <= px(1.0));
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-drop-overlap-0").is_none());
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn rendered_audio_clip_drag_starts_previews_and_commits_a_move(cx: &mut TestAppContext) {
    let (project, source, target, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_preview, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(
            matches!(
                view.test_probes.last_enqueued_edit.as_ref(),
                Some(Command::MoveClipWithOverwrite {
                    clip: requested_clip,
                    track: requested_track,
                    ..
                }) if *requested_clip == clip && *requested_track == target
            ),
            "audio release should enqueue MoveClip for source {source:?}; error={:?}",
            view.error
        );
    });
    visual.run_until_parked();
}

#[gpui::test]
fn option_drag_copies_audio_clip_and_modifier_changes_apply_without_pointer_motion(
    cx: &mut TestAppContext,
) {
    let (project, _source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("audio drag surface");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("destination lane");
    let down = point(
        surface.origin.x + surface.size.width / 2.0,
        surface.origin.y + surface.size.height / 2.0,
    );
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, option);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, option);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    editor.read_with(cx, |view, _| {
        assert!(
            view.interaction
                .active_clip_drag
                .as_ref()
                .is_some_and(|drag| drag.copy_modifier.get())
        );
    });
    let target = point(
        target_lane.origin.x + target_lane.size.width * 0.57,
        target_lane.origin.y + target_lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, option);
    visual.run_until_parked();
    let first_preview = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active drag");
        assert!(drag.copy_modifier.get());
        drag.preview.get().expect("copy preview").position
    });

    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active drag");
        assert!(!drag.copy_modifier.get(), "option release updates in place");
    });
    visual.simulate_modifiers_change(option);
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active drag");
        assert!(drag.copy_modifier.get(), "option press updates in place");
        assert_eq!(
            drag.preview.get().expect("updated preview").position,
            first_preview
        );
    });

    visual.simulate_mouse_up(target, MouseButton::Left, option);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::CopyClipTo { clip: copied, track, .. })
                if *copied == clip && *track == target_track
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn audio_clip_drop_uses_option_state_at_release_even_without_modifier_event(
    cx: &mut TestAppContext,
) {
    let (project, _source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_preview, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 90.0);

    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_up(release, MouseButton::Left, option);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::CopyClipTo { clip: copied, track, .. })
                if *copied == clip && *track == target_track
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn command_modifier_bypasses_adaptive_snap_while_drag_is_held_and_on_release(
    cx: &mut TestAppContext,
) {
    let (project, source, target_track, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_bounds = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("rendered audio move surface");
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("rendered source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("rendered target lane");
    let grid_before = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("adaptive grid visible before the modifier gesture");
    let down = point(
        source_bounds.origin.x + source_bounds.size.width / 2.0,
        source_bounds.origin.y + source_bounds.size.height / 2.0,
    );
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, command);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let target = point(
        source_lane.origin.x + source_lane.size.width * 0.57,
        target_lane.origin.y + target_lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    let grid_held = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("command bypass changes snapping without hiding grid");
    assert_eq!(grid_held.origin, grid_before.origin);
    let free_start = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active audio drag");
        assert!(drag.snap_modifier.get());
        match drag.preview.get().expect("free preview").position {
            super::super::drag::ClipPosition::Audio { start, .. } => start,
            _ => panic!("audio drag stays frame anchored"),
        }
    });

    let free_preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert_clip_move_guides(&mut visual, free_preview.left());
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let snapped_preview = visual.debug_bounds("clip-drop-preview").unwrap();
    assert_clip_move_guides(&mut visual, snapped_preview.left());
    let grid_released = visual
        .debug_bounds("arrangement-grid-line-0")
        .expect("grid remains visible after releasing command");
    assert_eq!(grid_released.origin, grid_before.origin);
    let snapped_start = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active audio drag");
        assert!(!drag.snap_modifier.get());
        match drag
            .preview
            .get()
            .expect("adaptive preview after key release")
            .position
        {
            super::super::drag::ClipPosition::Audio { start, .. } => start,
            _ => panic!("audio drag stays frame anchored"),
        }
    });
    assert_ne!(
        snapped_start, free_start,
        "stationary modifier release restores snapping"
    );

    // The release event's modifier wins even when no final mouse move arrives.
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::MoveClipWithOverwrite { clip: moved, track, start })
                if *moved == clip && *track == target_track && *start == free_start
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn command_modifier_temporarily_bypasses_beat_anchored_midi_drag_snap(cx: &mut TestAppContext) {
    let (project, source, target_track, clip) = midi_project();
    let (_, midi_preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(midi_preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_bounds = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("rendered midi move surface");
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("rendered source lane");
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target_track.value()).into_boxed_str(),
        ))
        .expect("rendered target lane");
    let down = point(
        source_bounds.origin.x + source_bounds.size.width / 2.0,
        source_bounds.origin.y + source_bounds.size.height / 2.0,
    );
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, command);
    visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, command);
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));
    let target = point(
        source_lane.origin.x + source_lane.size.width * 0.57,
        target_lane.origin.y + target_lane.size.height / 2.0,
    );
    visual.simulate_mouse_move(target, MouseButton::Left, command);
    visual.run_until_parked();
    let free_start = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active midi drag");
        assert!(drag.snap_modifier.get());
        match drag.preview.get().expect("free preview").position {
            super::super::drag::ClipPosition::Midi { start, .. } => start,
            _ => panic!("midi drag stays beat anchored"),
        }
    });
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let snapped_start = editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("active midi drag");
        assert!(!drag.snap_modifier.get());
        match drag
            .preview
            .get()
            .expect("adaptive preview after key release")
            .position
        {
            super::super::drag::ClipPosition::Midi { start, .. } => start,
            _ => panic!("midi drag stays beat anchored"),
        }
    });
    assert_ne!(
        snapped_start, free_start,
        "stationary modifier release restores snapping"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::MoveMidiClipWithOverwrite { clip: moved, track, start })
                if *moved == clip && *track == target_track && *start == free_start
        ));
    });
    visual.run_until_parked();
}

#[gpui::test]
fn rendered_audio_clip_drop_between_tracks_inserts_and_preserves_pointer_grab(
    cx: &mut TestAppContext,
) {
    let (project, source, anchor, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        Some(anchor),
        false,
    );
}

#[gpui::test]
fn rendered_audio_clip_drop_below_last_track_inserts_without_workspace_jump(
    cx: &mut TestAppContext,
) {
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        None,
        false,
    );
}

#[gpui::test]
fn option_drag_copies_audio_clip_into_an_appended_track(cx: &mut TestAppContext) {
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(cx, &mut visual, &editor, &project, clip, source, None, true);
}

#[gpui::test]
fn rendered_midi_clip_drop_between_tracks_inserts_and_preserves_beat_anchor(
    cx: &mut TestAppContext,
) {
    let (project, source, anchor, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        Some(anchor),
        false,
    );
}

#[gpui::test]
fn option_drag_copies_midi_clip_into_a_track_between_rows(cx: &mut TestAppContext) {
    let (project, source, anchor, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        Some(anchor),
        true,
    );
}

#[gpui::test]
fn rendered_midi_clip_drop_below_last_track_inserts_without_workspace_jump(
    cx: &mut TestAppContext,
) {
    let (project, source, _, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project.clone(), Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    drag_source_to_inserted_track(
        cx,
        &mut visual,
        &editor,
        &project,
        clip,
        source,
        None,
        false,
    );
}

#[gpui::test]
fn dropping_clip_on_pinned_main_cancels_instead_of_appending_track(cx: &mut TestAppContext) {
    let (project, source, _, clip) = audio_project();
    let track_count = project.tracks().len();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source_surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
        ))
        .expect("source clip surface");
    let main = visual
        .debug_bounds("pinned-main-track")
        .expect("pinned main row");
    let down = point(
        source_surface.origin.x + source_surface.size.width / 2.0,
        source_surface.origin.y + source_surface.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(cx.update(|app| app.has_active_drag()));

    let target = point(main.origin.x + px(20.0), main.center().y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        let drag = view
            .interaction
            .active_clip_drag
            .as_ref()
            .expect("drag stays captured");
        assert!(drag.destination.get().is_none());
        assert!(drag.preview.get().is_none());
        assert_eq!(
            view.timeline_caches.arrangement_bounds_epoch,
            view.timeline_caches.drag_layout_generation
        );
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().project.tracks().len(),
            track_count
        );
        assert!(view.test_probes.last_enqueued_edit.is_none());
        assert!(view.interaction.active_clip_drag.is_none());
        assert!(view.state.as_ref().unwrap().project.track(source).is_some());
    });
}

#[gpui::test]
fn rendered_midi_drag_moves_between_rows_and_remains_pickable(cx: &mut TestAppContext) {
    let (project, source, target, clip) = midi_project();
    let original_project = project.clone();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("compile midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (_drop_preview, release) = drag_source_to_target(cx, &mut visual, &editor, clip, 1, 320.0);
    let destination_start = editor
        .read_with(cx, |view, _| {
            let preview = view.interaction.active_clip_drag.as_ref()?.preview.get()?;
            let super::super::drag::ClipPosition::Midi { start, .. } = preview.position else {
                return None;
            };
            Some(start)
        })
        .expect("beat-anchored destination");
    visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.test_probes.last_enqueued_edit.as_ref(),
            Some(Command::MoveMidiClipWithOverwrite {
                clip: requested_clip,
                track: requested_track,
                start,
            }) if *requested_clip == clip
                && *requested_track == target
                && *start == destination_start
        ), "midi release should enqueue beat-anchored MoveMidiClip; request={:?}, destination={destination_start:?}, source={source:?}, error={:?}", view.test_probes.last_enqueued_edit, view.error);
    });
    visual.run_until_parked();

    // Publish the validated model command as the worker would, then verify the
    // moved clip remains pickable at its new owner after a revision change.
    let mut moved_project = Editor::new(original_project, 8).expect("editor model");
    moved_project
        .execute(Command::MoveMidiClip {
            clip,
            track: target,
            start: destination_start,
        })
        .expect("validated midi move");
    assert_eq!(
        moved_project
            .project()
            .tracks()
            .iter()
            .find(|track| track.midi_clips().iter().any(|item| item.id() == clip))
            .map(|track| track.id()),
        Some(target),
    );
    let (_, moved_preview) = hex_engine::compile::project_with_midi_preview(
        moved_project.project(),
        &Default::default(),
    )
    .expect("moved midi preview");
    editor.update(cx, |view, cx| {
        let previous = view.state.as_ref().expect("project state");
        view.state = Some(Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(moved_project.project().clone()),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: Default::default(),
            midi_preview: Arc::new(moved_preview),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: TIMELINE_FRAMES,
            can_undo: true,
            can_redo: false,
            revision: previous.revision + 1,
            unsaved: true,
            document_path: None,
        }));
        view.pending_requests.clear();
        view.worker_requests_pending = 0;
        cx.notify();
    });
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let moved = visual
        .debug_bounds(selector)
        .expect("moved midi clip has a new drag surface");
    let down = point(
        moved.origin.x + moved.size.width / 2.0,
        moved.origin.y + moved.size.height / 2.0,
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.interaction.active_clip_drag.is_some(),
            "moved midi clip can be picked up again; pointer_down={:?}, pending={}, worker_pending={}, lanes={:?}, revision={:?}, owners={:?}, moved_bounds={moved:?}, down={down:?}",
            view.interaction.pointer_down,
            view.pending_requests.len(),
            view.worker_requests_pending,
            view.timeline_caches.drag_lanes
                .iter()
                .map(|(id, lane)| (*id, lane.left, lane.width, lane.top, lane.height))
                .collect::<Vec<_>>(),
            view.state.as_ref().map(|state| state.revision),
            view.state.as_ref().map(|state| state.project.tracks().iter().filter(|track| !track.midi_clips().is_empty()).map(|track| (track.id(), track.midi_clips().iter().map(|clip| clip.id()).collect::<Vec<_>>())).collect::<Vec<_>>()),
        )
    });
}
