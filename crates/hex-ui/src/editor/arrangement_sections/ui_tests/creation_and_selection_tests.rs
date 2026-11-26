use super::*;

#[gpui::test]
fn g_on_a_main_gap_leaves_underlying_clips_untouched(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::pump;
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    let (start, end, before) = editor.read_with(&visual, |view, _| {
        (
            position(view, 5).x,
            position(view, 7).x,
            view.state.as_ref().unwrap().project.clone(),
        )
    });
    let start = point(start, lane.center().y);
    let end = point(end, start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(
            project.tracks()[0].midi_clips(),
            before.tracks()[0].midi_clips()
        );
        let [intro, section, chorus] = project.arrangement_sections() else {
            panic!("three sections");
        };
        assert_eq!(
            (intro, chorus),
            (
                &before.arrangement_sections()[0],
                &before.arrangement_sections()[1]
            )
        );
        assert_eq!(
            (section.start(), section.end()),
            (
                BeatTime::from_quarters(5).unwrap(),
                BeatTime::from_quarters(7).unwrap()
            )
        );
    });
}

#[gpui::test]
fn dragging_main_gap_then_g_creates_a_section_even_without_tracks(cx: &mut TestAppContext) {
    use crate::editor::drag::group_drag_ui_test::pump;
    cx.update(super::super::super::bind_editor_keys);
    let service = Service::new(48_000, 8, 8_000_000).unwrap();
    let state = next(&service).state;
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline section ui test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.focus.focus(window);
                view.snap_grid = SnapGrid::Quarter;
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual.debug_bounds("main-track-lane").unwrap();
    let start = point(lane.origin.x + lane.size.width * 0.1, lane.center().y);
    let end = point(lane.origin.x + lane.size.width * 0.4, start.y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let (start, end) = editor.read_with(&visual, |view, _| {
        assert!(view.interaction.time_selection_project_scope);
        assert!(view.normalized_time_selection().is_none());
        let selection = view.interaction.time_selection.unwrap();
        assert!(selection.anchor_frame < selection.end_frame);
        let map = view.state.as_ref().unwrap().project.tempo_map();
        (
            map.frame_to_beat(selection.anchor_frame).unwrap(),
            map.frame_to_beat(selection.end_frame).unwrap(),
        )
    });
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.tracks().is_empty());
        let [section] = project.arrangement_sections() else {
            panic!("created section");
        };
        assert_eq!((section.start(), section.end()), (start, end));
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .is_empty()
        );
    });
}

#[gpui::test]
fn marker_only_paint_tracks_hover_latched_drag_and_exact_ack(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (from, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 6)));
    let marker = Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    };
    visual.update(|window, cx| {
        window.dispatch_input(
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: from,
                ..Default::default()
            }),
            cx,
        );
    });
    visual.simulate_modifiers_change(marker);
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-markers-only-0")
            .is_some()
    );
    let hover_fill = painted_fill_for(&mut visual, "arrangement-section-markers-only-0");
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-markers-only-0")
            .is_none()
    );
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-markers-only-0")
            .is_none()
    );
    visual.simulate_modifiers_change(marker);
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-markers-only-0")
            .is_some(),
        "released modifiers retain the actual latched marker-only mode"
    );
    assert_ne!(
        painted_fill_for(&mut visual, "arrangement-section-markers-only-0"),
        hover_fill,
        "selection remains visible within marker-only feedback",
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(cx, |view, _| next(&view.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-markers-only-0")
                .is_some()
        );
        assert!(painted_fill_for(&mut visual, "arrangement-section-markers-only-0").is_some());
    }
    editor.update(cx, |view, cx| {
        view.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("arrangement-section-markers-only-0")
            .is_none()
    );
}

#[gpui::test]
fn option_drag_copies_sections_with_live_modifiers_and_acknowledged_selection(
    cx: &mut TestAppContext,
) {
    for (late, select_elsewhere, empty) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let (handle, editor, ids) = fixture(cx);
        if empty {
            editor.update(cx, |view, cx| {
                let track = view.state.as_ref().unwrap().project.tracks()[0].id();
                assert!(view.enqueue(Request::Edit(Command::RemoveTrack(track))));
                let event = next(&view.service);
                assert!(event.error.is_none());
                view.receive(event);
                cx.notify();
            });
        }
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.viewport =
                Some(view.visible_viewport(view.state.as_ref().unwrap().timeline_frames));
            view.section_autoplay = true;
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let (from, to) = editor.read_with(cx, |view, _| (position(view, 2), position(view, 12)));
        let copy = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(from, MouseButton::Left, copy);
        visual.simulate_mouse_up(from, MouseButton::Left, copy);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before);
            assert!(view.sections.pending.is_none());
            assert!(
                view.audio_pub.pending_audio_commands.is_empty()
                    && view.audio_pub.prepared_seek.is_none()
            );
        });
        visual.simulate_mouse_down(
            from,
            MouseButton::Left,
            if late { Modifiers::default() } else { copy },
        );
        visual.simulate_mouse_move(
            to,
            MouseButton::Left,
            if late { Modifiers::default() } else { copy },
        );
        visual.simulate_modifiers_change(copy);
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::DragCopy)
        );
        editor.read_with(cx, |view, _| {
            assert!(matches!(view.sections.gesture.as_ref().unwrap().command,
                Some(Command::DuplicateArrangementSectionAt { section, target, after: true })
                if section == ids[0] && target == ids[1]));
            assert_eq!(view.state.as_ref().unwrap().project, before);
        });
        // Option is live copy intent, unlike latched marker-only Option-Command.
        visual.simulate_modifiers_change(Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ClosedHand)
        );
        editor.read_with(cx, |view, _| {
            assert!(matches!(
                view.sections.gesture.as_ref().unwrap().command,
                Some(Command::ReorderSectionAfter { .. })
            ))
        });
        visual.simulate_modifiers_change(copy);
        visual.run_until_parked();
        let frozen = [
            bounds(&mut visual, 0),
            bounds(&mut visual, 1),
            bounds(&mut visual, 2),
        ];
        let selected_fill = painted_fill(&mut visual, 2);
        let preview = before
            .preview_arrangement_section_edit(&Command::DuplicateArrangementSectionAt {
                section: ids[0],
                target: ids[1],
                after: true,
            })
            .unwrap();
        let copy_color = preview
            .sections()
            .iter()
            .find(|section| section.id().value() == 0)
            .unwrap()
            .color();
        assert_eq!(
            selected_fill,
            Some(rgb(theme::section_colors(copy_color, true, true).0).into()),
            "the captured copy retains its gesture-owned target feedback"
        );
        visual.simulate_mouse_up(to, MouseButton::Left, copy);
        let terminal = editor.update(cx, |view, _| {
            assert!(view.sections.gesture.is_none());
            assert!(view.sections.pending.is_some());
            next(&view.service)
        });
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        let Some(Created::ArrangementSection(created)) = terminal.created else {
            panic!("copy identity");
        };
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_ne!(
                visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::DragCopy),
                "pending preview must not retain a held copy cursor"
            );
            assert_eq!(
                [
                    bounds(&mut visual, 0),
                    bounds(&mut visual, 1),
                    bounds(&mut visual, 2)
                ],
                frozen
            );
            assert_eq!(painted_fill(&mut visual, 2), selected_fill);
        }
        editor.update(cx, |view, cx| {
            if select_elsewhere {
                view.select_section_metadata(Some(ids[1]));
            }
            view.receive(terminal);
            assert!(view.sections.pending.is_none());
            let expected = if select_elsewhere { ids[1] } else { created };
            assert_eq!(view.sections.selected, Some(expected));
            assert_eq!(view.sections.selection, vec![expected]);
            let project = &view.state.as_ref().unwrap().project;
            assert_eq!(
                project.arrangement_section(ids[0]),
                before.arrangement_section(ids[0])
            );
            let copied = project.arrangement_section(created).unwrap();
            assert_eq!(
                (copied.start(), copied.end()),
                (
                    BeatTime::from_quarters(12).unwrap(),
                    BeatTime::from_quarters(16).unwrap()
                )
            );
            assert!(
                view.audio_pub.pending_audio_commands.is_empty()
                    && view.audio_pub.prepared_seek.is_none()
            );
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            [
                bounds(&mut visual, 0),
                bounds(&mut visual, 1),
                bounds(&mut visual, 2)
            ],
            frozen
        );
        if !select_elsewhere {
            // At the exact insertion edge, the pointer is outside the new
            // copy's hover fill after ACK; pending target paint above stays frozen.
            assert_eq!(
                painted_fill(&mut visual, 2),
                Some(rgb(theme::section_colors(copy_color, true, false).0).into())
            );
        }
    }
}

#[gpui::test]
fn section_copy_cursor_cancels_without_late_release_edits(cx: &mut TestAppContext) {
    for loss in ["escape", "button", "window", "focus", "generation"] {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
        let (from, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 13)));
        let copy = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(from, MouseButton::Left, copy);
        visual.simulate_mouse_move(to, MouseButton::Left, copy);
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::DragCopy)
        );
        let outside = point(px(1.0), px(1.0));
        visual.simulate_mouse_move(outside, MouseButton::Left, copy);
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::DragCopy)
        );
        match loss {
            "escape" => visual.simulate_keystrokes("escape"),
            "button" => visual.simulate_mouse_move(outside, None, copy),
            "window" => visual.deactivate_window(),
            "focus" => visual.update(|w, cx| {
                editor.update(cx, |v, cx| v.search_browser(w, cx));
            }),
            _ => editor.update(cx, |v, cx| {
                v.project_generation += 1;
                cx.notify();
            }),
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(outside, MouseButton::Left, copy);
        visual.run_until_parked();
        assert_ne!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::DragCopy)
        );
        editor.read_with(cx, |v, _| {
            assert!(
                v.sections.gesture.is_none() && v.sections.pending.is_none(),
                "{loss}"
            );
            assert_eq!(v.state.as_ref().unwrap().project, before, "{loss}");
            assert_eq!(v.worker_requests_pending, 0);
        });
    }
}

#[gpui::test]
fn option_command_drag_replaces_overlapped_markers_and_retains_preview_until_actual_ack(
    cx: &mut TestAppContext,
) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let (from, to) = editor.read_with(cx, |view, _| (position(view, 2), position(view, 9)));
    let marker_modifier = Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    };
    // Start a structural copy, then switch to marker adjustment without
    // moving the pointer. Option-Command replaces the insertion-edge preview.
    let copy = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(from, MouseButton::Left, copy);
    visual.simulate_mouse_move(to, MouseButton::Left, copy);
    // Modifier intent also refreshes while captured outside the lane; an
    // old structural command must never survive the switch to marker mode.
    visual.simulate_mouse_move(point(px(1.0), px(1.0)), MouseButton::Left, copy);
    visual.simulate_modifiers_change(marker_modifier);
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::ClosedHand)
    );
    editor.read_with(cx, |view, _| {
        assert!(matches!(
            view.sections.gesture.as_ref().unwrap().command,
            Some(Command::MoveArrangementSectionMarker { .. })
        ));
        let display = &view.sections.gesture.as_ref().unwrap().display;
        let moved = display
            .markers
            .iter()
            .find(|m| m.id == Some(ids[0]))
            .unwrap();
        let remaining = display
            .markers
            .iter()
            .find(|m| m.id == Some(ids[1]))
            .unwrap();
        assert_eq!(
            (moved.start, moved.end),
            (
                BeatTime::from_quarters(7).unwrap(),
                BeatTime::from_quarters(11).unwrap()
            )
        );
        assert_eq!(
            (remaining.start, remaining.end),
            (
                BeatTime::from_quarters(11).unwrap(),
                BeatTime::from_quarters(12).unwrap()
            )
        );
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
    let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
    visual.simulate_mouse_up(to, MouseButton::Left, marker_modifier);
    let terminal = editor.update(cx, |view, _| {
        let pending = view
            .sections
            .pending
            .as_ref()
            .expect("released marker preview");
        assert!(
            !pending.receipt.is_queued(),
            "the exact move command was admitted"
        );
        next(&view.service)
    });
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
    }
    editor.update(cx, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        view.receive(terminal);
        assert!(view.sections.pending.is_none());
        let after = view.state.as_ref().unwrap().project.snapshot();
        let mut expected = before.clone();
        expected.arrangement_sections = after.arrangement_sections.clone();
        assert_eq!(after, expected, "only section marker geometry changes");
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
}

#[gpui::test]
fn option_command_resize_live_modifier_carves_both_edges_and_commits_outside_lane(
    cx: &mut TestAppContext,
) {
    for (left, late) in [(false, false), (false, true), (true, false), (true, true)] {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let edge = bounds(&mut visual, usize::from(left));
        let down = point(
            if left {
                edge.left() + px(1.0)
            } else {
                edge.right() - px(1.0)
            },
            edge.top() + px(12.0),
        );
        let (safe, overlap) = editor.read_with(cx, |v, _| {
            (position(v, 6), position(v, if left { 2 } else { 10 }))
        });
        let marker_modifier = Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(
            down,
            MouseButton::Left,
            if late {
                Modifiers::default()
            } else {
                marker_modifier
            },
        );
        visual.simulate_mouse_move(
            safe,
            MouseButton::Left,
            if late {
                Modifiers::default()
            } else {
                marker_modifier
            },
        );
        if late {
            visual.simulate_modifiers_change(marker_modifier);
        }
        visual.simulate_mouse_move(overlap, MouseButton::Left, marker_modifier);
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(matches!(
                v.sections.gesture.as_ref().unwrap().command,
                Some(Command::ResizeArrangementSectionMarker { .. })
            ))
        });
        let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
        let outside = point(px(1.0), px(1.0));
        visual.simulate_mouse_move(outside, MouseButton::Left, marker_modifier);
        visual.simulate_mouse_up(outside, MouseButton::Left, marker_modifier);
        let event = editor.update(cx, |v, _| {
            assert!(
                v.sections.pending.is_some(),
                "off-lane release must admit the last valid resize"
            );
            next(&v.service)
        });
        assert!(event.error.is_none(), "{:?}", event.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
        }
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
        let after = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        assert_eq!(
            after.arrangement_sections[0].end,
            BeatTime::from_quarters(if left { 2 } else { 10 }).unwrap()
        );
        assert_eq!(
            after.arrangement_sections[1].start,
            after.arrangement_sections[0].end
        );
        assert_eq!(after.arrangement_sections[0].id, ids[0].value());
        assert_eq!(after.arrangement_sections[1].id, ids[1].value());
        let mut music = after.clone();
        music.arrangement_sections = before.arrangement_sections.clone();
        assert_eq!(music, before);
        for (request, expected) in [(Request::Undo, &before), (Request::Redo, &after)] {
            editor.update(cx, |v, cx| {
                v.service.submit(request).unwrap();
                let event = next(&v.service);
                assert!(event.error.is_none());
                v.receive(event);
                assert_eq!(&v.state.as_ref().unwrap().project.snapshot(), expected);
                cx.notify();
            });
            visual.run_until_parked();
        }
    }
}

#[gpui::test]
fn option_command_move_latches_marker_mode_and_preserves_control_snap_inversion(
    cx: &mut TestAppContext,
) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for free in [false, true] {
        let (from, to) = editor.read_with(cx, |view, _| {
            let a = position(view, 6);
            let b = position(view, 7);
            (position(view, 2), point(a.x + (b.x - a.x) * 0.37, a.y))
        });
        visual.simulate_mouse_down(
            from,
            MouseButton::Left,
            Modifiers {
                platform: true,
                alt: true,
                ..Modifiers::default()
            },
        );
        // Releasing Option-Command must not turn this into a music edit.
        visual.simulate_mouse_move(
            to,
            MouseButton::Left,
            Modifiers {
                control: free,
                ..Modifiers::default()
            },
        );
        editor.read_with(cx, |view, _| {
            let Some(Command::MoveArrangementSectionMarker { section, start }) =
                &view.sections.gesture.as_ref().unwrap().command
            else {
                panic!("expected latched marker move")
            };
            assert_eq!(*section, ids[0]);
            let four = BeatTime::from_quarters(4).unwrap();
            if free {
                assert!(*start > four && *start < BeatTime::from_quarters(5).unwrap());
            } else {
                assert_eq!(*start, four);
            }
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.sections.gesture.is_none());
            assert!(view.sections.pending.is_none());
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}

#[gpui::test]
fn paint_empty_section_lane_at_every_height(cx: &mut TestAppContext) {
    for (fraction, reverse) in [(0.1, false), (0.5, true), (0.9, false)] {
        let (handle, editor, _) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let (from, to) = editor.read_with(cx, |v, _| {
            let [_, top, _, height] = v.section_layout().unwrap();
            let mut from = position(v, if reverse { 7 } else { 5 });
            let mut to = position(v, if reverse { 5 } else { 7 });
            from.y = px(top + height * fraction);
            to.y = from.y;
            (from, to)
        });
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(from, MouseButton::Left, Modifiers::default());
        visual.simulate_event(MouseDownEvent {
            position: from,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            assert!(
                matches!(v.sections.gesture.as_ref().and_then(|g| g.command.as_ref()),
                Some(Command::AddArrangementSection { start, end, .. })
                if *start == BeatTime::from_quarters(5).unwrap()
                    && *end == BeatTime::from_quarters(7).unwrap()),
                "height {fraction}"
            );
            assert!(v.interaction.time_selection_gesture.is_none());
        });
        let preview = bounds(&mut visual, 2);
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let terminal = editor.update(cx, |v, _| next(&v.service));
        assert!(terminal.error.is_none(), "{:?}", terminal.error);
        for _ in 0..2 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!(bounds(&mut visual, 2), preview);
        }
        editor.update(cx, |v, cx| {
            v.receive(terminal);
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .len(),
                3
            );
            v.service.submit(Request::Undo).unwrap();
            let terminal = next(&v.service);
            assert!(terminal.error.is_none());
            v.receive(terminal);
            let restored = v.state.as_ref().unwrap().project.snapshot();
            let mut expected = before.clone();
            assert!(restored.next_id >= expected.next_id);
            expected.next_id = restored.next_id;
            assert_eq!(restored, expected, "one undo restores all content");
            cx.notify();
        });
        visual.run_until_parked();
        let first = bounds(&mut visual, 0);
        let occupied = point(first.center().x, first.top() + first.size.height * fraction);
        visual.simulate_event(MouseDownEvent {
            position: occupied,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        editor.read_with(cx, |v, _| {
            if fraction < 0.3 {
                assert!(matches!(
                    v.sections.gesture.as_ref().map(|g| g.mode),
                    Some(Mode::Reorder(_))
                ));
            } else {
                assert!(v.sections.gesture.is_none());
                assert!(v.interaction.time_selection_gesture.is_some());
            }
        });
        visual.simulate_keystrokes("escape");
    }
}

#[gpui::test]
fn paint_and_resize_main_sections_preview_without_project_writes(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("main-track-lane").is_some());
    let (start, end) = editor.read_with(cx, |v, _| (position(v, 5), position(v, 7)));
    let original = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
    visual.simulate_event(MouseDownEvent {
        position: start,
        button: MouseButton::Left,
        click_count: 2,
        modifiers: Modifiers::default(),
        first_mouse: false,
    });
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(bounds(&mut visual, 2).size.width > px(1.0));
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, original);
        assert!(matches!(
            v.sections.gesture.as_ref().unwrap().command,
            Some(Command::AddArrangementSection { .. })
        ));
        assert!(v.interaction.time_selection_gesture.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-section-2").is_none());
    // The right edge edits marker geometry only, preserving every track.
    let edge = bounds(&mut visual, 0);
    let down = point(
        edge.origin.x + edge.size.width - px(1.0),
        edge.origin.y + px(12.0),
    );
    visual.simulate_mouse_down(
        down,
        MouseButton::Left,
        Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        },
    );
    let target = editor.read_with(cx, |v, _| position(v, 6));
    visual.simulate_mouse_move(
        target,
        MouseButton::Left,
        Modifiers {
            platform: true,
            alt: true,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(matches!(
            v.sections.gesture.as_ref().unwrap().command,
            Some(Command::ResizeArrangementSectionMarker { section, boundary: end, .. })
                if section == ids[0] && end == BeatTime::from_quarters(6).unwrap()
        ));
        assert_eq!(v.state.as_ref().unwrap().project, original);
    });
    assert!(bounds(&mut visual, 0).size.width > edge.size.width);
    // Revision/viewport changes invalidate a held operation before release.
    editor.update(cx, |v, cx| {
        v.snap_grid = SnapGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(!v.sections.held()));
    assert_eq!(bounds(&mut visual, 0).size.width, edge.size.width);
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.update(cx, |v, _| v.snap_grid = SnapGrid::Quarter);
    let (anchor, destination) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 3)));
    let alt = Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(anchor, MouseButton::Left, alt);
    visual.simulate_mouse_move(destination, MouseButton::Left, alt);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(matches!(
            v.sections.gesture.as_ref().unwrap().command,
            Some(Command::MoveArrangementSectionMarker { section, start })
                if section == ids[0] && start == BeatTime::ONE
        ));
        assert_eq!(v.state.as_ref().unwrap().project, original);
    });
    assert!(visual.debug_bounds("section-insertion-preview").is_none());
    visual.simulate_keystrokes("escape");
}

#[gpui::test]
fn first_section_can_be_inserted_after_last(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 12)));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(
            v.sections.gesture.as_ref().unwrap().command.is_some(),
            "after-last drop must move the song"
        );
    });
    assert!(visual.debug_bounds("section-insertion-preview").is_none());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let sections = v.state.as_ref().unwrap().project.arrangement_sections();
        assert_eq!(sections[0].name(), "Chorus");
        assert_eq!(sections[1].name(), "Intro");
        assert_eq!(sections[1].end(), BeatTime::from_quarters(12).unwrap());
        assert_eq!(
            v.state.as_ref().unwrap().project.meter_map(),
            &hex_project::MeterMap::common_time(),
            "dragging equal-meter sections must not create meter boundaries"
        );
    });
    assert!(
        visual
            .debug_bounds("arrangement-grid-line-192000")
            .is_some()
    );
    editor.read_with(cx, |v, _| {
        let state = v.state.as_ref().unwrap();
        let viewport = v.visible_viewport(state.timeline_frames);
        let width = v.timeline_caches.main_track_lane_bounds.unwrap().width;
        let line = super::super::super::musical_time::grid_lines(
            &state.project,
            viewport,
            width,
            v.snap_grid,
        )
        .into_iter()
        .find(|line| line.frame == 192_000)
        .unwrap();
        assert!(!line.meter_marker, "the moved seam is not a meter marker");
    });
}

#[gpui::test]
fn selected_sections_l_uses_outer_bounds_and_one_history_edit(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Use the real marker click, including the gap between these sections.
    let chorus = editor.read_with(cx, |v, _| position(v, 10));
    visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    for (key, start, end) in [("l", 192_000, 288_000), ("w", 0, 288_000)] {
        if start == 0 {
            let intro = editor.read_with(cx, |v, _| position(v, 2));
            let shift = Modifiers {
                shift: true,
                platform: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(intro, MouseButton::Left, shift);
            visual.simulate_mouse_up(intro, MouseButton::Left, shift);
            visual.run_until_parked();
        }
        let before = editor.read_with(cx, |v, _| {
            assert_eq!(v.sections.selection.len(), if start == 0 { 2 } else { 1 });
            v.state.as_ref().unwrap().project.clone()
        });
        if key == "l" {
            crate::editor::section_launch::test_tap_loop_key(&mut visual);
        } else {
            visual.simulate_keystrokes(key);
        }
        editor.read_with(cx, |v, _| {
            assert!(
                matches!(v.test_probes.last_enqueued_edit,
            Some(Command::SetLoopRegion { start: a, end: b, enabled: true }) if (a,b)==(start,end)),
                "selected sections must dispatch a loop request"
            )
        });
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        let after = event.state.project.clone();
        let region = after.loop_region().unwrap();
        assert_eq!(
            (region.start(), region.end(), region.enabled()),
            (start, end, true)
        );
        assert_eq!(after.tracks(), before.tracks());
        assert_eq!(after.arrangement_sections(), before.arrangement_sections());
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        for (key, expected) in [("cmd-z", &before), ("cmd-shift-z", &after)] {
            visual.simulate_keystrokes(key);
            let event = editor.update(cx, |v, _| next(&v.service));
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_eq!(&event.state.project, expected);
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.run_until_parked();
        }
        editor.read_with(cx, |v, _| {
            assert!(v.sections.selection.iter().all(|id| ids.contains(id)))
        });
    }
}

#[gpui::test]
fn lost_section_button_records_cancel_and_allows_loop_before_late_release(cx: &mut TestAppContext) {
    let directory = std::env::temp_dir().join(format!(
        "hex-section-loop-diagnostic-test-{}",
        std::process::id()
    ));
    let mut log = hex_diagnostics::SessionLog::start(directory).unwrap();
    cx.update(|cx| {
        crate::diagnostics::install_session_log(cx, Ok(log.handle()));
        crate::diagnostics::install_action_diagnostics(cx, true);
    });
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let from = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    crate::editor::section_launch::test_tap_loop_key(&mut visual);
    editor.read_with(cx, |v, _| {
        assert!(v.sections.held());
        assert!(
            v.test_probes.last_enqueued_edit.is_none(),
            "a real held gesture still blocks loop creation"
        );
    });
    visual.simulate_mouse_move(point(px(1.0), px(1.0)), None, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(
            !v.sections.held(),
            "button loss cancels before any late release"
        )
    });
    crate::editor::section_launch::test_tap_loop_key(&mut visual);
    editor.read_with(cx, |v, _| {
        assert!(matches!(
            v.test_probes.last_enqueued_edit,
            Some(Command::SetLoopRegion { .. })
        ))
    });
    editor.update(cx, |view, cx| {
        let event = next(&view.service);
        assert!(event.error.is_none());
        view.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    let point = editor.read_with(cx, |view, _| position(view, 10));
    visual.simulate_mouse_down(point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    log.shutdown().unwrap();
    let contents = std::fs::read_to_string(log.path()).unwrap();
    assert!(contents.contains("section_held"));
    assert!(contents.contains("stage=section_capture outcome=cancel"));
    assert!(contents.contains("stage=section_capture outcome=release"));
}

#[gpui::test]
fn selected_sections_loop_resolves_current_tempo_and_respects_input_custody(
    cx: &mut TestAppContext,
) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |v, cx| {
        v.service
            .submit(Request::Edit(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(4).unwrap(),
                tempo: hex_project::Tempo::new(60_000_000).unwrap(),
            }))
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none());
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    let chorus = editor.read_with(cx, |v, _| position(v, 10));
    visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(cx, |v, _| {
        assert_eq!(v.section_loop_bounds(), Ok(Some((288_000, 480_000))));
        v.test_probes.last_enqueued_edit = None;
        v.computer_keyboard.enabled = true;
    });
    crate::editor::section_launch::test_tap_loop_key(&mut visual);
    editor.update(cx, |v, _| {
        assert!(
            v.test_probes.last_enqueued_edit.is_none(),
            "L retains computer MIDI priority"
        );
        v.computer_keyboard.enabled = false;
    });
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.sections.naming()));
    visual.simulate_keystrokes("l cmd-l");
    editor.read_with(cx, |v, _| {
        assert!(v.test_probes.last_enqueued_edit.is_none())
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let chorus = editor.read_with(cx, |v, _| position(v, 10));
    visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
    crate::editor::section_launch::test_tap_loop_key(&mut visual);
    editor.read_with(cx, |v, _| {
        assert!(v.sections.held());
        assert!(
            v.test_probes.last_enqueued_edit.is_none(),
            "held section capture cannot set a loop"
        );
    });
    visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
    editor.update(cx, |v, _| {
        // Simulate an unreconciled stale selection at the read-only boundary.
        v.service
            .submit(Request::EditArrangementSection {
                project_generation: v.project_generation,
                revision: v.state.as_ref().unwrap().revision,
                command: Command::RemoveArrangementSection(ids[0]),
            })
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none());
        v.receive(event);
        v.sections.selection = ids.to_vec();
        v.sections.selected = Some(ids[1]);
        assert_eq!(
            v.section_loop_bounds(),
            Err("selected section no longer belongs to the arrangement")
        );
        v.sections.selection = vec![ids[1]];
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        // A retained range from another context must not win over Main sections.
        v.interaction.time_selection = Some(super::super::super::time_selection::TimeSelection {
            anchor_track: Some(track),
            end_track: Some(track),
            anchor_frame: 1,
            end_frame: 2,
        });
    });
    crate::editor::section_launch::test_tap_loop_key(&mut visual);
    editor.read_with(cx, |v, _| {
        assert!(matches!(
            v.test_probes.last_enqueued_edit,
            Some(Command::SetLoopRegion {
                start: 288_000,
                end: 480_000,
                enabled: true
            })
        ))
    });
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none());
    let region = event.state.project.loop_region().unwrap();
    assert_eq!((region.start(), region.end()), (288_000, 480_000));
}

#[gpui::test]
fn multiselection_hover_and_g_merge_keep_music_and_focus_guards(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (a, b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
    let base = painted_fill(&mut visual, 0);
    visual.simulate_event(gpui::MouseMoveEvent {
        position: a,
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    visual.run_until_parked();
    assert_ne!(
        base,
        painted_fill(&mut visual, 0),
        "hover changes section surface"
    );
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let selected = painted_fill(&mut visual, 0);
    assert_ne!(base, selected, "selection changes entire section surface");
    let shift = Modifiers {
        shift: true,
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(b, MouseButton::Left, shift);
    visual.simulate_mouse_up(b, MouseButton::Left, shift);
    visual.run_until_parked();
    let tracks = editor.read_with(cx, |v, _| {
        assert_eq!(v.sections.selection.len(), 2);
        assert!(v.sections.pending.is_none());
        v.state.as_ref().unwrap().project.tracks().to_vec()
    });
    visual.simulate_keystrokes("g");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.arrangement_sections().len(), 1);
        assert_eq!(
            p.arrangement_section(ids[0]).unwrap().end(),
            BeatTime::from_quarters(12).unwrap()
        );
        assert_eq!(p.tracks(), tracks);
        assert_eq!(v.sections.selected, Some(ids[0]));
        assert_eq!(v.sections.selection, vec![ids[0]]);
    });
    // Text editing owns printable g instead of merging a section.
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.simulate_input("g");
    editor.read_with(cx, |v, cx| {
        assert!(v.sections.naming());
        assert_eq!(v.sections.name.as_ref().unwrap().input.read(cx).text(), "g");
        assert!(v.sections.pending.is_none());
    });
}

#[gpui::test]
fn content_resize_release_retains_preview_and_command_resize_only_moves_marker(
    cx: &mut TestAppContext,
) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let edge = bounds(&mut visual, 0);
    let down = point(
        edge.origin.x + edge.size.width - px(1.0),
        edge.origin.y + px(12.0),
    );
    let to = editor.read_with(cx, |v, _| position(v, 6));
    let before = editor.read_with(cx, |v, _| {
        v.state.as_ref().unwrap().project.tracks().to_vec()
    });
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(matches!(
            v.sections.gesture.as_ref().unwrap().command,
            Some(Command::ResizeArrangementSectionEdge { .. })
        ))
    });
    let preview = bounds(&mut visual, 0);
    let outside = point(px(1.0), px(1.0));
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, 0), preview);
    }
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(bounds(&mut visual, 0), preview);
    editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(
            p.arrangement_section(ids[0]).unwrap().end(),
            BeatTime::from_quarters(6).unwrap()
        );
        assert_ne!(p.tracks(), before);
    });
    // Normal deletion cuts song time; command deletion is covered separately.
    visual.simulate_keystrokes("backspace");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_section(ids[0])
                .is_none()
        )
    });
}

#[gpui::test]
fn section_duplicate_shortcut_copies_song_content_and_is_one_history_step(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let down = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    visual.simulate_keystrokes("cmd-d");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.arrangement_sections().len(), 3);
        assert_eq!(
            p.arrangement_sections()[1].start(),
            BeatTime::from_quarters(4).unwrap()
        );
        assert!(p.tracks()[0].midi_clips().len() > 1);
    });
    visual.simulate_keystrokes("cmd-z");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none());
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    editor.read_with(cx, |v, _| {
        let mut actual = v.state.as_ref().unwrap().project.snapshot();
        actual.next_id = before.next_id;
        assert_eq!(actual, before);
    });
}
