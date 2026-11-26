use super::*;

#[gpui::test]
fn lane_cursor_splits_only_markers_and_holds_preview_until_exact_ack(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let at = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-edit-cursor").is_some());
    assert!(visual.debug_bounds("section-insertion-preview").is_none());
    editor.read_with(cx, |v, _| {
        assert_eq!(
            v.sections.cursor.unwrap().beat,
            BeatTime::from_quarters(2).unwrap()
        );
        assert!(v.sections.pending.is_none());
    });
    visual.simulate_keystrokes("e");
    let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
    assert_eq!(preview[0].right(), preview[1].left());
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
        editor.read_with(cx, |v, _| assert!(v.sections.pending.is_some()));
    }
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
    editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.arrangement_sections().len(), 3);
        assert_eq!(p.arrangement_sections()[0].id(), ids[0]);
        assert_ne!(p.arrangement_sections()[1].id(), ids[0]);
        let mut after = p.snapshot();
        after.next_id = before.next_id;
        after.arrangement_sections = before.arrangement_sections.clone();
        assert_eq!(after, before);
        assert!(v.sections.pending.is_none());
    });
    visual.simulate_keystrokes("cmd-z");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none());
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    let mut undone = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    undone.next_id = before.next_id;
    assert_eq!(undone, before);
}

#[gpui::test]
fn lane_cursor_gap_endpoint_stale_and_text_context_refuse_without_clip_fallthrough(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for beat in [6, 4, 0] {
        let at = editor.read_with(cx, |v, _| position(v, beat));
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("section-edit-cursor").is_some());
        visual.simulate_keystrokes("e");
        editor.read_with(cx, |v, _| {
            assert!(v.sections.pending.is_none());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(
                v.error
                    .as_ref()
                    .is_some_and(|e| e.contains("strictly inside"))
            );
        });
    }
    let at = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    editor.update(cx, |v, _| v.sections.cursor.as_mut().unwrap().revision += 1);
    visual.simulate_keystrokes("e");
    editor.read_with(cx, |v, _| {
        assert!(v.sections.pending.is_none());
        assert!(
            v.error
                .as_ref()
                .is_some_and(|e| e.contains("click the lane again"))
        );
    });
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.sections.naming()));
    visual.simulate_keystrokes("cmd-a e");
    editor.read_with(cx, |v, cx| {
        assert!(v.sections.pending.is_none());
        assert_eq!(v.sections.name.as_ref().unwrap().input.read(cx).text(), "e");
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    // A regular-track point releases the lane and preserves clip splitting.
    let at = editor.read_with(cx, |v, _| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        let lane = v.timeline_caches.drag_lanes[&track];
        point(px(lane.left + lane.width * 0.1), px(lane.top + 18.0))
    });
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.sections.cursor.is_none()));
    visual.simulate_keystrokes("e");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(event.state.project.arrangement_sections().len(), 2);
    assert_eq!(event.state.project.tracks()[0].midi_clips().len(), 2);
}

#[gpui::test]
fn lane_cursor_snap_inversion_and_empty_project_clicks(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let mut beats = Vec::new();
    for grid in [SnapGrid::Quarter, SnapGrid::Off] {
        editor.update(cx, |v, _| v.snap_grid = grid);
        for free in [false, true] {
            let at = editor.read_with(cx, |v, _| {
                let a = position(v, 2);
                let b = position(v, 3);
                point(a.x + (b.x - a.x) * 0.37, a.y)
            });
            let modifiers = Modifiers {
                platform: free,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
            visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
            editor.read_with(cx, |v, _| {
                let beat = v.sections.cursor.unwrap().beat;
                beats.push(beat);
                assert!(v.sections.pending.is_none());
            });
        }
    }
    assert_eq!(beats[0], BeatTime::from_quarters(2).unwrap());
    assert!(beats[1] > beats[0] && beats[1] < BeatTime::from_quarters(3).unwrap());
    assert_eq!(
        beats[1], beats[2],
        "off and Command-inverted fixed grid are free"
    );
    assert_ne!(
        beats[3], beats[2],
        "Command with snap off enables adaptive snapping"
    );
    editor.update(cx, |v, cx| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        v.service
            .submit(Request::Edit(Command::RemoveTrack(track)))
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none());
        v.state = Some(event.state);
        cx.notify();
    });
    visual.run_until_parked();
    let at = editor.read_with(cx, |v, _| position(v, 6));
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-edit-cursor").is_some());
}

#[gpui::test]
fn shift_click_selects_section_blocks_in_header_and_body_without_time_ranges(
    cx: &mut TestAppContext,
) {
    for (focused, body) in [(false, false), (false, true), (true, false), (true, true)] {
        let (handle, editor, ids) = fixture(cx);
        editor.update(cx, |view, cx| {
            for command in [
                Command::AddTrack {
                    name: "hidden empty track".into(),
                },
                Command::SetTempoPoint {
                    beat: BeatTime::ONE,
                    tempo: hex_project::Tempo::new(180_000_000).unwrap(),
                },
                Command::SetArrangementSectionRange {
                    section: ids[0],
                    start: BeatTime::from_ratio(1, 7).unwrap(),
                    end: BeatTime::from_ratio(27, 7).unwrap(),
                },
            ] {
                let request = if matches!(command, Command::SetArrangementSectionRange { .. }) {
                    Request::EditArrangementSection {
                        project_generation: view.project_generation,
                        revision: view.state.as_ref().unwrap().revision,
                        command,
                    }
                } else {
                    Request::Edit(command)
                };
                assert!(view.enqueue(request));
                let event = next(&view.service);
                assert!(
                    event.error.is_none(),
                    "fixture edit refused: {:?}",
                    event.error
                );
                view.receive(event);
            }
            view.section_autoplay = false;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let header = editor.read_with(cx, |view, _| position(view, 10));
        visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if focused {
            visual.simulate_keystrokes("q");
            visual.run_until_parked();
        }
        editor.update(cx, |view, cx| {
            let first = view.state.as_ref().unwrap().project.tracks()[0].id();
            view.interaction.time_selection =
                Some(super::super::super::time_selection::TimeSelection {
                    anchor_track: Some(first),
                    end_track: Some(first),
                    anchor_frame: 111,
                    end_frame: 222,
                });
            view.interaction.time_selection_project_scope = false;
            view.section_autoplay = true;
            view.test_probes.last_enqueued_edit = None;
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().clone());
        let at = editor.read_with(cx, |view, _| {
            let [_, top, _, height] = view.section_layout().unwrap();
            point(
                position(view, 2).x,
                if body {
                    px(top + height * 0.75)
                } else {
                    header.y
                },
            )
        });
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at, MouseButton::Left, shift);
        visual.simulate_mouse_up(at, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.interaction.time_selection.is_none());
            assert!(!view.interaction.time_selection_project_scope);
            assert_eq!(view.sections.selection, vec![ids[0], ids[1]]);
            assert_eq!(view.sections.selected, Some(ids[0]));
            assert!(view.selected_clip_ids().is_empty());
            assert!(
                view.sections.gesture.is_none()
                    && view.sections.pending.is_none()
                    && view.sections.cursor.is_none()
            );
            assert!(view.interaction.time_selection_gesture.is_none());
            assert!(view.test_probes.last_enqueued_edit.is_none());
            assert!(
                view.audio_pub.pending_audio_commands.is_empty()
                    && view.audio_pub.prepared_seek.is_none()
                    && view.audio_pub.latched_seek_intent.is_none()
            );
            assert_eq!(view.worker_requests_pending, 0);
            assert_eq!(view.state.as_ref().unwrap().project, before.project);
            assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
            assert_eq!(
                (
                    view.state.as_ref().unwrap().can_undo,
                    view.state.as_ref().unwrap().can_redo
                ),
                (before.can_undo, before.can_redo)
            );
            assert_eq!(view.arrangement_focus.projection.enabled, focused);
        });
        let toggle = Modifiers {
            shift: true,
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(at, MouseButton::Left, toggle);
        visual.simulate_mouse_up(at, MouseButton::Left, toggle);
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.sections.selection, vec![ids[1]]);
            assert_eq!(view.sections.selected, Some(ids[1]));
            assert!(view.interaction.time_selection.is_none());
        });
    }
}

#[gpui::test]
fn shift_click_selects_every_section_between_anchor_and_click(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut all = vec![ids[0]];
    editor.update(cx, |view, cx| {
        for (start, end, name) in [(4, 8, "Verse"), (12, 16, "Bridge")] {
            assert!(view.enqueue(Request::EditArrangementSection {
                project_generation: view.project_generation,
                revision: view.state.as_ref().unwrap().revision,
                command: Command::AddArrangementSection {
                    name: name.into(),
                    color: ElementColor::Green,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                },
            }));
            let event = next(&view.service);
            assert!(event.error.is_none(), "{:?}", event.error);
            let Some(Created::ArrangementSection(id)) = event.created else {
                panic!("section result")
            };
            all.push(id);
            view.receive(event);
        }
        view.section_autoplay = false;
        cx.notify();
    });
    // Song order: intro, verse, chorus, bridge.
    let all = [all[0], all[1], ids[1], all[2]];
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let mut click = |quarters, modifiers| {
        let at = editor.read_with(&visual, |view, _| position(view, quarters));
        visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
        visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.interaction.time_selection.is_none());
            (
                view.sections.selected_ids().to_vec(),
                view.sections.selected,
            )
        })
    };
    click(2, Modifiers::default());
    assert_eq!(click(14, shift), (all.to_vec(), Some(all[3])));
    // Same anchor: a shorter range replaces the previous one.
    assert_eq!(click(10, shift), (all[..3].to_vec(), Some(all[2])));
    click(14, Modifiers::default());
    assert_eq!(click(2, shift), (all.to_vec(), Some(all[0])));
}

#[gpui::test]
fn menu_merge_click_over_the_lane_merges_and_undo_restores(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (a, b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    // One selected section: g explains instead of silently consuming input.
    visual.simulate_keystrokes("g");
    editor.read_with(cx, |v, _| {
        assert!(v.sections.pending.is_none());
        assert!(
            v.error
                .as_ref()
                .is_some_and(|e| e.contains("shift-click another section to merge a span"))
        );
    });
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(b, MouseButton::Left, shift);
    visual.simulate_mouse_up(b, MouseButton::Left, shift);
    visual.run_until_parked();
    let body = editor.read_with(cx, |v, _| {
        let [_, top, _, height] = v.section_layout().unwrap();
        point(a.x, px(top + height * 0.75))
    });
    visual.simulate_mouse_down(body, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(body, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(
            v.context_menu.as_ref().unwrap().items[0].label,
            "merge selected span · g"
        )
    });
    // The first row opens at the pointer, above the lane's own capture.
    let panel = visual.debug_bounds("context-menu-panel").unwrap();
    let row = point(panel.origin.x + px(20.0), panel.origin.y + px(14.0));
    visual.simulate_mouse_down(row, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let event = editor.update(cx, |v, _| {
        assert!(v.context_menu.is_none());
        assert!(matches!(
            v.sections.pending.as_ref().map(|p| &p.command),
            Some(Command::MergeArrangementSections(_))
        ));
        next(&v.service)
    });
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, cx| {
        v.receive(event);
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.arrangement_sections().len(), 1);
        assert_eq!(
            p.arrangement_section(ids[0]).unwrap().end(),
            BeatTime::from_quarters(12).unwrap()
        );
        assert_eq!(p.tracks(), before.tracks());
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-z");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, _| {
        v.receive(event);
        assert_eq!(v.state.as_ref().unwrap().project, before);
    });
}

#[gpui::test]
fn section_copy_paints_committed_rows_before_the_terminal(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| position(view, 2));
    visual.simulate_click(source, Modifiers::default());
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-d");

    let terminal = editor.read_with(cx, |view, _| next(&view.service));
    let observed = editor.read_with(cx, |view, _| {
        view.service.try_arrangement_edit_view().unwrap()
    });
    let Some(Created::ArrangementSection(copy)) = terminal.created else {
        panic!("section copy");
    };
    assert!(matches!(
        observed.kind,
        hex_session::service::ArrangementEditViewKind::Duplicate(Created::ArrangementSection(id))
            if id == copy
    ));
    let committed = observed.state.upgrade().unwrap();
    let marker = committed.project.arrangement_section(copy).unwrap();
    let midi = committed.project.tracks()[0]
        .midi_clips()
        .iter()
        .find(|clip| clip.start() == marker.start())
        .unwrap()
        .id();
    let selector = Box::leak(format!("midi-clip-visual-{}", midi.value()).into_boxed_str());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(selector).is_none(),
        "the old focus cache has no copy"
    );

    editor.update(cx, |view, cx| {
        assert!(view.arrangement_focus.projection.enabled);
        assert_eq!(view.sections.selected, Some(ids[0]));
        for (generation, revision) in [
            (observed.project_generation + 1, observed.previous_revision),
            (observed.project_generation, observed.previous_revision + 1),
        ] {
            assert!(
                !view.observe_duplicate_view(ArrangementEditViewObservation {
                    project_generation: generation,
                    first_previous_revision: revision,
                    previous_revision: revision,
                    kind: observed.kind,
                    state: Arc::downgrade(&committed),
                })
            );
        }
        assert!(view.observe_duplicate_view(observed));
        assert!(view.section_install_pending());
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_section(copy)
                .is_none()
        );
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &committed
        ));
        cx.notify();
    });
    visual.run_until_parked();
    let before_ack = visual
        .debug_bounds(selector)
        .expect("committed copy is visible");
    let marker_bounds = visual.debug_bounds("arrangement-section-1").unwrap();
    editor.update(cx, |view, cx| {
        view.receive(terminal);
        assert!(view.committed_section_copy_view().is_none());
        assert!(!view.section_install_pending());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(selector), Some(before_ack));
    assert_eq!(
        visual.debug_bounds("arrangement-section-1"),
        Some(marker_bounds)
    );
}

#[gpui::test]
fn consecutive_keyboard_section_copies_keep_both_receipts_and_latest_rows(cx: &mut TestAppContext) {
    let (handle, editor, _ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let source = editor.read_with(cx, |view, _| position(view, 2));
    visual.simulate_click(source, Modifiers::default());
    visual.simulate_keystrokes("q");
    visual.run_until_parked();
    let initial = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    visual.simulate_keystrokes("cmd-d cmd-d");
    let deadline = crate::test_wait::hang_deadline();
    let first_view = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "first committed section view");
        std::thread::yield_now();
    };
    let mut first_terminal = editor.read_with(&visual, |view, _| next(&view.service));
    let first_state = first_view.state.upgrade().unwrap();
    editor.update(&mut visual, |view, cx| {
        // A refused, still-queued successor must not retire its predecessor.
        let primary = view.sections.pending.as_ref().unwrap();
        let command = primary.command.clone();
        view.sections.ahead = Some(Pending {
            generation: primary.generation,
            revision: initial + 1,
            command: command.clone(),
            display: primary.display.clone(),
            layout: None,
            copied: None,
            cut_clipboard: None,
            cut_cursor: None,
            receipt: Receipt::new(primary.generation),
            selection_intent_epoch: primary.selection_intent_epoch,
            deferred_submission: false,
        });
        assert!(view.refuse_section(&Request::EditArrangementSection {
            project_generation: view.project_generation,
            revision: initial + 1,
            command,
        }));
        assert!(view.sections.pending.is_some());
        assert!(view.sections.ahead.is_none());
        assert!(view.observe_duplicate_view(first_view));
        assert!(view.sections.pending.is_some());
        assert!(
            view.sections.ahead.is_some(),
            "successor admitted before first terminal"
        );
        cx.notify();
    });
    visual.run_until_parked();
    let second_view = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "second committed section view");
        std::thread::yield_now();
    };
    let second_terminal = editor.read_with(&visual, |view, _| next(&view.service));
    assert!(first_terminal.error.is_none());
    assert!(second_terminal.error.is_none());
    let second_state = second_view.state.upgrade().unwrap();
    assert_eq!(second_state.revision, initial + 2);
    let Some(Created::ArrangementSection(copy)) = second_terminal.created else {
        panic!("second section copy");
    };
    let copy_start = second_state
        .project
        .arrangement_section(copy)
        .unwrap()
        .start();
    let midi = second_state.project.tracks()[0]
        .midi_clips()
        .iter()
        .find(|clip| {
            clip.start() == copy_start && first_state.project.midi_clip(clip.id()).is_none()
        })
        .unwrap()
        .id();
    let selector = Box::leak(format!("midi-clip-visual-{}", midi.value()).into_boxed_str());
    editor.update(&mut visual, |view, cx| {
        for (generation, revision) in [
            (
                second_view.project_generation + 1,
                second_view.previous_revision,
            ),
            (
                second_view.project_generation,
                second_view.previous_revision + 1,
            ),
        ] {
            assert!(
                !view.observe_duplicate_view(ArrangementEditViewObservation {
                    project_generation: generation,
                    first_previous_revision: revision,
                    previous_revision: revision,
                    kind: second_view.kind,
                    state: Arc::downgrade(&second_state),
                })
            );
        }
        assert!(view.observe_duplicate_view(second_view));
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &second_state
        ));
        cx.notify();
    });
    visual.run_until_parked();
    let before = visual
        .debug_bounds(selector)
        .expect("second committed copy paints");
    let marker = visual.debug_bounds("arrangement-section-2").unwrap();
    editor.update(&mut visual, |view, cx| {
        view.history_action(Request::Undo, cx)
    });
    first_terminal.error = Some("automatic recovery: test warning".into());
    editor.update(&mut visual, |view, cx| {
        view.receive(first_terminal);
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &second_state
        ));
        assert!(
            view.sections.pending.is_some(),
            "warning cannot retire an admitted successor"
        );
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(selector), Some(before));
    assert_eq!(visual.debug_bounds("arrangement-section-2"), Some(marker));
    editor.update(&mut visual, |view, cx| {
        view.receive(second_terminal);
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &second_state));
        assert!(view.sections.pending.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(selector), Some(before));
    assert_eq!(visual.debug_bounds("arrangement-section-2"), Some(marker));
    settle(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial + 3);
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .len(),
            first_state.project.arrangement_sections().len(),
            "undo follows both admitted copies"
        );
    });
}

#[gpui::test]
fn unrelated_worker_completion_preserves_existing_status(cx: &mut TestAppContext) {
    let (_handle, editor, _ids) = fixture(cx);
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddTrack {
            name: "other".into()
        })));
    });
    let mut event = editor.read_with(cx, |view, _| next(&view.service));
    assert!(event.error.is_none());
    event.status = None;
    editor.update(cx, |view, _| {
        view.status = "existing status".into();
        view.receive(event);
        assert_eq!(&*view.status, "existing status");
    });
}

#[gpui::test]
fn newer_selection_does_not_admit_an_ahead_section_copy(cx: &mut TestAppContext) {
    newer_selection_rejects_ahead_section_copy(cx, false);
}

#[gpui::test]
fn newer_selection_does_not_admit_a_duplicate_after_pointer_copy(cx: &mut TestAppContext) {
    newer_selection_rejects_ahead_section_copy(cx, true);
}

#[gpui::test]
fn pointer_copy_keeps_a_different_queued_duplicate_ordered(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.select_section_metadata(Some(ids[0]));
        view.section_metadata(Command::DuplicateArrangementSectionAt {
            section: ids[0],
            target: ids[0],
            after: true,
        });
        view.section_metadata(Command::DuplicateArrangementSection(ids[1]));
    });
    let deadline = crate::test_wait::hang_deadline();
    let first = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "first section view");
        std::thread::yield_now();
    };
    let terminal = editor.read_with(&visual, |view, _| next(&view.service));
    editor.update(&mut visual, |view, _| {
        assert!(view.observe_duplicate_view(first));
        assert!(
            view.sections.ahead.is_none(),
            "different source must not be admitted ahead"
        );
        assert!(view.sections.queued.is_some());
        view.receive(terminal);
    });
}

#[gpui::test]
fn project_replacement_retires_both_section_receipts(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.select_section_metadata(Some(ids[0]));
        view.section_metadata(Command::DuplicateArrangementSection(ids[0]));
        view.section_metadata(Command::DuplicateArrangementSection(ids[0]));
    });
    let deadline = crate::test_wait::hang_deadline();
    let first = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "first section view");
        std::thread::yield_now();
    };
    let _first_terminal = editor.read_with(&visual, |view, _| next(&view.service));
    editor.update(&mut visual, |view, _| {
        assert!(view.observe_duplicate_view(first))
    });
    let second = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "second section view");
        std::thread::yield_now();
    };
    let _second_terminal = editor.read_with(&visual, |view, _| next(&view.service));
    let stale_state = second.state.upgrade().unwrap();
    let stale_kind = second.kind;
    let stale_generation = second.project_generation;
    let stale_revision = second.previous_revision;
    editor.update(&mut visual, |view, _| {
        assert!(view.observe_duplicate_view(second));
        assert!(view.sections.pending.is_some() && view.sections.ahead.is_some());
        view.service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap();
    });
    let replacement = editor.read_with(&visual, |view, _| next(&view.service));
    assert!(replacement.project_replaced && replacement.error.is_none());
    editor.update(&mut visual, |view, _| {
        view.receive(replacement);
        assert!(view.sections.pending.is_none() && view.sections.ahead.is_none());
        assert!(
            !view.observe_duplicate_view(ArrangementEditViewObservation {
                project_generation: stale_generation,
                first_previous_revision: stale_revision,
                previous_revision: stale_revision,
                kind: stale_kind,
                state: Arc::downgrade(&stale_state),
            })
        );
    });
}

#[gpui::test]
fn play_uses_applied_successor_while_both_section_receipts_wait(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::Acquire;

    let commands = vec![
        Command::AddTrack {
            name: "track".into(),
        },
        Command::AddArrangementSection {
            name: "section".into(),
            color: ElementColor::Blue,
            start: BeatTime::from_quarters(8).unwrap(),
            end: BeatTime::from_quarters(12).unwrap(),
        },
    ];
    let mut fixture = super::super::super::worker_ui_fixture::open_with_section_support(
        cx, commands, false, true,
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.run_until_parked();
    let (revision, expected) = editor.update(&mut visual, |view, _| {
        let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
        view.select_section_metadata(Some(section));
        view.interaction.play_start_at_beginning = false;
        let expected = view.playback_start_frame();
        let revision = view.state.as_ref().unwrap().revision;
        view.section_metadata(Command::DuplicateArrangementSection(section));
        view.section_metadata(Command::DuplicateArrangementSection(section));
        (revision, expected)
    });
    let deadline = crate::test_wait::hang_deadline();
    let first = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "first section view");
        std::thread::yield_now();
    };
    let first_terminal = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "first section terminal");
        std::thread::yield_now();
    };
    assert!(first_terminal.error.is_none(), "{:?}", first_terminal.error);
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.read_with(&visual, |view, _| {
            view.audio_status().is_some_and(|status| {
                !status.stopped_edit_pending()
                    && status
                        .recording_source()
                        .is_some_and(|source| source.context.editor_revision == revision + 1)
            })
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "first source and released fence");
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_duplicate_view(first));
        assert!(view.sections.ahead.is_some());
        cx.notify();
    });
    let second = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "second section view");
        std::thread::yield_now();
    };
    let second_terminal = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "second section terminal");
        std::thread::yield_now();
    };
    assert!(
        second_terminal.error.is_none(),
        "{:?}",
        second_terminal.error
    );
    let committed = second.state.upgrade().unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_duplicate_view(second));
        cx.notify();
    });
    visual.run_until_parked();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.section_copy_play_source()
                .is_some_and(|source| source.context.editor_revision == revision + 2)
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "applied successor source");
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &committed
        ));
        view.toggle_play();
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(
                hex_engine::playback::PlaybackCommand::PlayFromWithNoteChase {
                    frame: expected,
                    note_chase: hex_engine::NoteChase::On,
                }
            )
        );
        cx.notify();
    });
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    editor.read_with(&visual, |view, _| {
        assert!(view.audio_status().unwrap().playing.load(Acquire))
    });
    editor.update(&mut visual, |view, cx| {
        view.receive(first_terminal);
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &committed
        ));
        view.receive(second_terminal);
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
        cx.notify();
    });
    fixture.finish(cx);
}

#[gpui::test]
fn held_recovery_allows_next_section_copy_and_applied_play(cx: &mut TestAppContext) {
    held_recovery_allows_queued_duplicate(cx, 0);
}

#[gpui::test]
fn held_pointer_copy_allows_queued_duplicate_and_applied_play(cx: &mut TestAppContext) {
    held_recovery_allows_queued_duplicate(cx, 1);
}

#[gpui::test]
fn held_append_copy_allows_queued_duplicate_and_applied_play(cx: &mut TestAppContext) {
    held_recovery_allows_queued_duplicate(cx, 2);
}
