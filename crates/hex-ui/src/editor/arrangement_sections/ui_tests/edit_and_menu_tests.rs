use super::*;

#[gpui::test]
fn held_replacement_copy_allows_queued_duplicate_and_applied_play(cx: &mut TestAppContext) {
    held_recovery_allows_queued_duplicate(cx, 3);
}

#[gpui::test]
fn applied_pointer_section_copy_plays_from_the_selected_copy(cx: &mut TestAppContext) {
    pointer_section_copy_plays_from_the_selected_copy(cx, 0);
}

#[gpui::test]
fn appended_section_copy_with_gap_plays_from_the_selected_copy(cx: &mut TestAppContext) {
    pointer_section_copy_plays_from_the_selected_copy(cx, 1);
}

#[gpui::test]
fn replaced_section_copy_plays_from_the_selected_copy(cx: &mut TestAppContext) {
    pointer_section_copy_plays_from_the_selected_copy(cx, 2);
}

#[gpui::test]
fn applied_keyboard_section_copy_starts_before_its_terminal(cx: &mut TestAppContext) {
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
    let (source_revision, expected) = editor.update(&mut visual, |view, cx| {
        let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
        view.select_section_metadata(Some(section));
        view.interaction.play_start_at_beginning = false;
        let expected = view.playback_start_frame();
        let revision = view.state.as_ref().unwrap().revision;
        view.section_metadata(Command::DuplicateArrangementSection(section));
        view.toggle_play();
        assert!(
            view.test_probes.last_transport_command.is_none(),
            "unapplied source cannot start"
        );
        cx.notify();
        (revision, expected)
    });
    let deadline = crate::test_wait::hang_deadline();
    let observation = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(observation) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break observation;
        }
        assert!(Instant::now() < deadline, "committed section view");
        std::thread::yield_now();
    };
    let terminal = loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "section terminal");
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    let committed = observation.state.upgrade().unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_duplicate_view(observation));
        assert_eq!(view.state.as_ref().unwrap().revision, source_revision);
        cx.notify();
    });
    visual.run_until_parked();
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.section_copy_play_source().is_some()
        });
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "exact applied source and released fence"
        );
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().revision, source_revision);
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &committed
        ));
        let pending = view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap();
        pending.output_identity += 1;
        assert!(
            view.section_copy_play_source().is_none(),
            "wrong output must not start"
        );
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .output_identity -= 1;
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .context
            .project_generation += 1;
        assert!(
            view.section_copy_play_source().is_none(),
            "wrong generation must not start"
        );
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .context
            .project_generation -= 1;
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .context
            .editor_revision += 1;
        assert!(
            view.section_copy_play_source().is_none(),
            "wrong revision must not start"
        );
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .context
            .editor_revision -= 1;
        let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } = &mut view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .state
        else {
            panic!("resident metadata")
        };
        metadata.requires_preroll = true;
        assert!(
            view.section_copy_play_source().is_none(),
            "preroll must not be bypassed"
        );
        let crate::EditorPublicationState::ResidentSubmitted { metadata, .. } = &mut view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .state
        else {
            panic!("resident metadata")
        };
        metadata.requires_preroll = false;
        view.audio_status()
            .unwrap()
            .device_fault
            .store(true, std::sync::atomic::Ordering::Release);
        assert!(
            view.section_copy_play_source().is_none(),
            "faulted output must not start"
        );
        view.audio_status()
            .unwrap()
            .device_fault
            .store(false, std::sync::atomic::Ordering::Release);
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
    editor.update(&mut visual, |view, _| {
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .output_identity += 1;
        view.toggle_play();
        assert_eq!(
            view.test_probes.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::Pause)
        );
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .output_identity -= 1;
    });
    fixture.backend.render(&mut [[0.0; 2]; 512]).unwrap();
    editor.read_with(&visual, |view, _| {
        assert!(!view.audio_status().unwrap().playing.load(Acquire))
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
        cx.notify();
    });
    visual.run_until_parked();
    fixture.finish(cx);
}

#[gpui::test]
fn section_keys_during_a_pending_edit_queue_one_follow_up_or_refuse_visibly(
    cx: &mut TestAppContext,
) {
    let sections = |editor: &Entity<EditorView>, cx: &mut TestAppContext| {
        editor.read_with(cx, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .iter()
                .map(|s| s.id())
                .collect::<Vec<_>>()
        })
    };
    // (keys after the pending cmd-d, expected section count, first kept)
    for (keys, count, kept) in [
        // Back to back: duplicate, then delete of the same section.
        ("backspace", 2, false),
        // Latest wins: the delete is replaced by a second duplicate.
        ("backspace cmd-d", 4, true),
        // Undo orders behind the duplicate and drops the queued delete.
        ("backspace cmd-z", 2, true),
    ] {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let a = editor.read_with(cx, |v, _| position(v, 2));
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-d");
        // The acknowledgement is delayed: nothing ticks the worker yet.
        visual.simulate_keystrokes(keys);
        editor.read_with(cx, |v, _| {
            assert!(v.sections.pending.is_some(), "{keys}");
            let queued = v.sections.queued.as_ref().map(|q| &q.command);
            match keys {
                "backspace" => assert!(
                    matches!(queued, Some(Command::DeleteArrangementSection(id)) if *id == ids[0])
                ),
                "backspace cmd-d" => assert!(
                    matches!(queued, Some(Command::DuplicateArrangementSection(id)) if *id == ids[0])
                ),
                _ => {
                    assert!(queued.is_none());
                    assert_eq!(&*v.status, "queued section edit cancelled by undo/redo");
                }
            }
            if queued.is_some() {
                assert_eq!(
                    &*v.status,
                    "section edit queued · applies after the current edit"
                );
            }
        });
        settle(&editor, &mut visual);
        let after = sections(&editor, cx);
        assert_eq!(after.len(), count, "{keys}: {after:?}");
        assert_eq!(after.contains(&ids[0]), kept, "{keys}");
        editor.read_with(cx, |v, _| {
            assert!(v.error.is_none(), "{keys}: {:?}", v.error)
        });
    }

    // g queues a merge behind another in-flight section edit.
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (a, b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(b, MouseButton::Left, shift);
    visual.simulate_mouse_up(b, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.update(cx, |v, _| {
        v.section_metadata(Command::SetArrangementSectionColor {
            section: ids[1],
            color: ElementColor::Green,
        });
        assert!(v.sections.pending.is_some());
    });
    visual.simulate_keystrokes("g");
    editor.read_with(cx, |v, _| {
        assert!(matches!(
            v.sections.queued.as_ref().map(|q| &q.command),
            Some(Command::MergeArrangementSections(_))
        ));
    });
    settle(&editor, &mut visual);
    assert_eq!(sections(&editor, cx), vec![ids[0]]);

    // Pointer and split input cannot wait for the edit; they refuse visibly.
    editor.update(cx, |v, _| {
        v.section_metadata(Command::SetArrangementSectionColor {
            section: ids[0],
            color: ElementColor::Blue,
        });
        assert!(v.sections.pending.is_some());
    });
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| {
        assert_eq!(v.error.as_deref(), Some(SECTION_BUSY));
        assert!(v.sections.gesture.is_none());
    });
    settle(&editor, &mut visual);
}

#[gpui::test]
fn resize_keeps_one_selected_surface_every_frame_through_delayed_ack(cx: &mut TestAppContext) {
    let platform = Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    };
    let alt = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    // (touching seam, modifiers, release quarter)
    for (seam, modifiers, end) in [
        (false, Modifiers::default(), 7),
        (false, platform, 7),
        (false, alt, 7),
        (true, Modifiers::default(), 6),
        (true, platform, 6),
    ] {
        let (handle, editor, ids) = if seam {
            touching_fixture(cx)
        } else {
            fixture(cx)
        };
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let edge = bounds(&mut visual, 0);
        let down = point(
            if seam {
                edge.right()
            } else {
                edge.right() - px(1.0)
            },
            edge.origin.y + px(12.0),
        );
        let to = editor.read_with(cx, |v, _| position(v, end));
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        let mut frames = Vec::new();
        // Sub-grid steps cross snap points, so the pointer alternates
        // between inside and outside the snapped preview edge.
        for step in 1..=8 {
            let x = down.x + (to.x - down.x) * (step as f32 / 8.0);
            visual.simulate_mouse_move(point(x, down.y), MouseButton::Left, modifiers);
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("arrangement-section-selected-trailing-0")
                    .is_some(),
                "seam {seam} {modifiers:?} step {step}: section lost its selected edge"
            );
            editor.read_with(cx, |v, _| {
                assert_eq!(v.sections.selected, Some(ids[0]));
                assert!(v.sections.gesture.is_some());
            });
            frames.push((
                painted_fill(&mut visual, 0),
                painted_fill_for(&mut visual, "arrangement-section-header-0"),
            ));
        }
        visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        // Delayed acknowledgement: completed repaints keep the frozen look.
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert!(
                visual
                    .debug_bounds("arrangement-section-selected-trailing-0")
                    .is_some()
            );
            frames.push((
                painted_fill(&mut visual, 0),
                painted_fill_for(&mut visual, "arrangement-section-header-0"),
            ));
        }
        assert!(frames[0].0.is_some() && frames[0].1.is_some(), "{frames:?}");
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame, &frames[0],
                "seam {seam} {modifiers:?} frame {index}: selected surface changed"
            );
        }
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("arrangement-section-selected-trailing-0")
                .is_some()
        );
        editor.read_with(cx, |v, _| {
            assert_eq!(v.sections.selected, Some(ids[0]));
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[0])
                    .unwrap()
                    .end(),
                BeatTime::from_quarters(end).unwrap()
            );
        });
    }
}

#[gpui::test]
fn g_merges_a_click_and_shift_click_section_span(cx: &mut TestAppContext) {
    g_merge_case(cx, false, false);
}

#[gpui::test]
fn g_merges_a_leftward_shift_click_span(cx: &mut TestAppContext) {
    g_merge_case(cx, false, true);
}

#[gpui::test]
fn g_merges_a_leftward_shift_click_span_under_section_focus(cx: &mut TestAppContext) {
    g_merge_case(cx, true, true);
}

#[gpui::test]
fn g_merges_a_shift_click_span_under_section_focus(cx: &mut TestAppContext) {
    g_merge_case(cx, true, false);
}

#[gpui::test]
fn g_merges_a_selected_span_outside_strict_section_focus_and_explains_empty(
    cx: &mut TestAppContext,
) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // Nothing selected: g explains instead of silently doing nothing.
    visual.simulate_keystrokes("g");
    editor.read_with(cx, |v, _| {
        assert!(
            v.error
                .as_ref()
                .is_some_and(|e| e.contains("select a clip, a time range or two sections")),
            "{:?}",
            v.error
        );
    });
    editor.update(cx, |v, _| {
        v.error = None;
        v.sections.selected = Some(ids[0]);
        v.sections.selection = ids.to_vec();
        // Main no longer owns the device view, so the strict section-focus
        // test fails, yet the section span is the only grouping selection.
        v.device_panel.main_selected = None;
    });
    visual.simulate_keystrokes("g");
    editor.read_with(cx, |v, _| {
        assert!(
            matches!(
                v.sections.pending.as_ref().map(|p| &p.command),
                Some(Command::MergeArrangementSections(_))
            ),
            "{:?}",
            v.error
        );
    });
}

#[gpui::test]
fn off_target_clip_release_does_not_block_section_loop_scope(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let clip = editor.read_with(cx, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].midi_clips()[0].id()
    });
    let trim_selector =
        Box::leak(format!("midi-clip-trim-right-{}", clip.value()).into_boxed_str());
    let trim_point = visual.debug_bounds(trim_selector).unwrap().center();

    // 1. Clicking a trim handle on the lane canvas must retire pointer_down.
    visual.simulate_mouse_down(trim_point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| assert!(v.interaction.pointer_down.is_some()));
    visual.simulate_mouse_up(trim_point, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.interaction.pointer_down.is_none()));

    // 2. Pressing a clip header and releasing off the clip must retire pointer_down
    //    so selecting a section and pressing L sets the loop scope cleanly.
    let header_selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let header_point = visual.debug_bounds(header_selector).unwrap().center();
    visual.simulate_mouse_down(header_point, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| assert!(v.interaction.pointer_down.is_some()));
    visual.simulate_mouse_up(
        point(px(2.0), px(2.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.interaction.pointer_down.is_none()));

    // 3. Even if a stale pointer_down survived a lost release, a fresh section
    //    click clears it so L immediately sets the section loop scope.
    editor.update(cx, |v, _| {
        v.remember_pointer_down(v.project_generation, clip, None, 10.0);
        assert!(v.interaction.pointer_down.is_some());
    });
    let chorus = editor.read_with(cx, |v, _| position(v, 10));
    visual.simulate_mouse_down(chorus, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(chorus, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.interaction.pointer_down.is_none()));

    crate::editor::section_launch::test_tap_loop_key(&mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(v.error, None);
        assert!(matches!(
            v.test_probes.last_enqueued_edit,
            Some(Command::SetLoopRegion { .. })
        ));
    });
}

#[gpui::test]
fn section_copy_then_immediate_pointer_edit_succeeds_without_refusal_or_flicker(
    cx: &mut TestAppContext,
) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    // Select section 0 (beats 0..4) and trigger Cmd+D section copy without settling yet.
    let section_a = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(section_a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(section_a, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-d");

    editor.read_with(cx, |v, _| {
        assert!(v.sections.pending.is_some(), "section copy is in flight");
        assert_eq!(v.worker_requests_pending, 1);
        assert!(
            v.section_pointer_available(),
            "section pointer gestures must stay available during section copy"
        );
        assert!(
            v.drag_available(),
            "clip dragging must stay available during section copy"
        );
        assert!(
            !v.track_gain_edit_blocked(),
            "track volume must stay available during section copy"
        );
        assert_eq!(v.error, None);
    });

    // Immediately drag section 0's trailing edge (beat 4 -> beat 6) with Option+Cmd
    // (marker-only resize) BEFORE the section copy terminal arrives.
    let edge = editor.read_with(cx, |v, _| {
        let p = position(v, 4);
        point(p.x - px(2.0), p.y)
    });
    let target = editor.read_with(cx, |v, _| position(v, 6));
    let marker_only = Modifiers {
        alt: true,
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(edge, MouseButton::Left, marker_only);
    visual.simulate_mouse_move(target, Some(MouseButton::Left), marker_only);
    visual.run_until_parked();
    let held_end = editor.read_with(cx, |v, _| {
        v.sections
            .gesture
            .as_ref()
            .and_then(|g| g.display.markers.iter().find(|m| m.id == Some(ids[0])))
            .map(|m| m.end)
    });
    assert_eq!(
        held_end,
        Some(BeatTime::from_quarters(6).unwrap()),
        "pointer drag during pending section copy must preview new edge"
    );

    visual.simulate_mouse_up(target, MouseButton::Left, marker_only);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(
            v.error, None,
            "section pointer edit during section copy must not show SECTION_BUSY refusal"
        );
        assert!(
            v.sections.queued.is_some(),
            "released section edit must queue behind the in-flight copy"
        );
        let queued_end = v
            .sections
            .queued
            .as_ref()
            .and_then(|q| q.display.as_ref())
            .and_then(|d| d.markers.iter().find(|m| m.id == Some(ids[0])))
            .map(|m| m.end);
        assert_eq!(
            queued_end,
            Some(BeatTime::from_quarters(6).unwrap()),
            "queued pointer edit must keep its frozen display without release flicker"
        );
    });

    // Settle both the section copy and the queued resize edit.
    settle(&editor, &mut visual);
    editor.read_with(cx, |v, _| {
        assert_eq!(v.error, None);
        assert!(v.sections.pending.is_none());
        assert!(v.sections.queued.is_none());
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(
            project.arrangement_sections().len(),
            3,
            "section copy must commit canonically"
        );
        let first = project.arrangement_section(ids[0]).unwrap();
        assert_eq!(
            first.end(),
            BeatTime::from_quarters(6).unwrap(),
            "immediate follow-up resize must commit canonically"
        );
    });
}
