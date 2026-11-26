use super::*;

#[gpui::test]
fn section_duplicate_shortcut_copies_a_copy_at_a_rounded_sample_seam(cx: &mut TestAppContext) {
    use crate::editor::external_audio_drop_ui_test::BatchFiles;
    use hex_project::{Editor, Project, Tempo};

    let rate = 8_000;
    let tempo = Tempo::new(143_000_000).unwrap();
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let map = TempoMap::constant(rate, tempo).unwrap();
    let section_frames = map.beat_to_frame(beat(1)).unwrap();
    let files = BatchFiles::new();
    let wav = files.wav(
        "section-seam.wav",
        rate,
        u32::try_from(section_frames).unwrap(),
    );
    let path = wav.with_extension("hex");
    let mut project = Editor::new(Project::new(rate).unwrap(), 16).unwrap();
    for command in [
        Command::SetTempoPoint {
            beat: beat(0),
            tempo,
        },
        Command::AddArrangementSection {
            name: "source".into(),
            color: ElementColor::Blue,
            start: beat(0),
            end: beat(1),
        },
    ] {
        project.execute(command).unwrap();
    }
    let Created::Imported { clip, .. } = project
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: wav,
            sample_rate: rate,
            frames: section_frames,
        })
        .unwrap()
    else {
        panic!("audio import")
    };
    project
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let mut snapshot = project.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap()
        .view_end = beat(1).raw();
    hex_storage::save_new(&path, &Project::from_snapshot(snapshot).unwrap()).unwrap();
    let service = Service::new(rate, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Open(path)).unwrap();
    let opened = next(&service);
    assert!(
        opened.project_replaced && opened.open_complete && opened.error.is_none(),
        "{:?}",
        opened.error
    );
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor) = section_test_window(cx, service, Arc::clone(&opened.state));
    editor.update(cx, |view, cx| {
        view.receive(opened);
        view.viewport = Some(Viewport::fit(16_000));
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for index in 0..2 {
        let x = bounds(&mut visual, index).center().x;
        let y = editor.read_with(cx, |view, _| position(view, 0).y);
        let down = point(x, y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("cmd-d");
        let event = editor.update(cx, |view, _| next(&view.service));
        assert!(
            event.error.is_none(),
            "cmd-d on section {index}: {:?}",
            event.error
        );
        let project = &event.state.project;
        assert_eq!(project.arrangement_sections().len(), index + 2);
        let clips = project.tracks()[0].clips();
        assert_eq!(clips.len(), index + 2);
        assert!(
            clips
                .windows(2)
                .all(|pair| pair[0].start() + pair[0].duration() <= pair[1].start())
        );
        editor.update(cx, |view, cx| {
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let _ = bounds(&mut visual, index + 1);
    }
    for (key, count) in [("cmd-z", 2), ("cmd-shift-z", 3)] {
        visual.simulate_keystrokes(key);
        let event = editor.update(cx, |view, _| next(&view.service));
        assert!(event.error.is_none(), "{key}: {:?}", event.error);
        assert_eq!(event.state.project.arrangement_sections().len(), count);
        assert_eq!(event.state.project.tracks()[0].clips().len(), count);
        editor.update(cx, |view, cx| {
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("arrangement-section-2").is_some(),
            count == 3
        );
    }
}

#[gpui::test]
fn stationary_non_grid_content_edge_click_does_not_change_music(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(cx, |v, cx| {
        v.service
            .submit(Request::EditArrangementSection {
                project_generation: v.project_generation,
                revision: v.state.as_ref().unwrap().revision,
                command: Command::SetArrangementSectionRange {
                    section: ids[0],
                    start: BeatTime::ZERO,
                    end: BeatTime::from_ratio(19, 5).unwrap(),
                },
            })
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none());
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    let edge = bounds(&mut visual, 0);
    let down = point(
        edge.origin.x + edge.size.width - px(1.0),
        edge.origin.y + px(12.0),
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| {
        assert!(v.sections.pending.is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn stationary_click_near_snapped_end_does_not_move_song(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let a = bounds(&mut visual, 0);
    let down = point(a.origin.x + a.size.width - px(7.0), a.origin.y + px(12.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| {
        assert!(v.sections.pending.is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn option_command_move_changes_canonical_marker_without_moving_music(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let tracks = editor.read_with(cx, |v, _| {
        v.state.as_ref().unwrap().project.tracks().to_vec()
    });
    let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 3)));
    let modifiers = Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.tracks(), tracks);
        assert!(p.arrangement_section(ids[0]).unwrap().start() > BeatTime::ZERO);
        assert_eq!(
            p.arrangement_section(ids[0]).unwrap().end().raw()
                - p.arrangement_section(ids[0]).unwrap().start().raw(),
            BeatTime::from_quarters(4).unwrap().raw()
        );
    });
}

#[gpui::test]
fn reorder_pending_keeps_completed_frames_until_its_exact_terminal(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (down, to) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 0)));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-insertion-preview").is_none());
    let preview = bounds(&mut visual, 1);
    // Controlled FIFO seam: reserve two ordinary completion ordinals,
    // then deliver real earlier-work and refused section Service events.
    // The separate success test exercises real release/enqueue stamping.
    editor.update(cx, |v, _| {
        let g = v.sections.gesture.take().unwrap();
        let command = g.command.unwrap();
        assert!(matches!(command,
            Command::ReorderSectionBefore { section, before } if section == ids[1] && before == ids[0]
        ));
        let mut receipt = Receipt::new(v.project_generation);
        assert!(receipt.submitted(2));
        v.sections.pending = Some(Pending {
            selection_intent_epoch: v.interaction.selection_intent_epoch,
            generation: v.project_generation,
            revision: g.revision,
            command,
            display: g.display,
            layout: None,
            copied: None,
            cut_clipboard: None,
            cut_cursor: None,
            receipt,
            deferred_submission: false,
        });
        v.worker_requests_pending = 2;
    });
    visual.simulate_keystrokes("escape");
    editor.update(cx, |v, _| {
        v.toggle_play();
        assert!(format!("{:?}", v.error).contains("applying edit"));
        v.request_seek(12);
        assert!(format!("{:?}", v.error).contains("applying edit"));
        assert!(v.audio_pub.pending_audio_commands.is_empty());
    });
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, 1), preview);
        assert!(visual.debug_bounds("section-insertion-preview").is_none());
    }
    let earlier = editor.update(cx, |v, _| {
        v.service.submit(Request::Undo).unwrap();
        next(&v.service)
    });
    let terminal = editor.update(cx, |v, _| {
        let p = v.sections.pending.as_ref().unwrap();
        v.service
            .submit(Request::EditArrangementSection {
                project_generation: p.generation,
                revision: p.revision,
                command: p.command.clone(),
            })
            .unwrap();
        next(&v.service)
    });
    // Earlier completion keeps the frozen paint, even if it changes revision.
    editor.update(cx, |v, cx| {
        v.receive(earlier);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(bounds(&mut visual, 1), preview);
    // Undo made the stamp stale: refusal and canonical install retire together.
    assert!(terminal.error.is_some());
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-insertion-preview").is_none());
    editor.read_with(cx, |v, _| assert!(v.sections.pending.is_none()));
}

#[gpui::test]
fn pending_reorder_draws_final_clips_until_exact_terminal_and_refusal_restores(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let canonical = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
    let original = midi_clip_bounds(&mut visual, &canonical);
    let (down, to) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 2)));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(cx, |v, _| {
        assert!(v.sections.pending.as_ref().unwrap().layout.is_some());
        next(&v.service)
    });
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    let committed = std::sync::Arc::clone(&terminal.state.project);
    assert_ne!(*committed, *canonical, "reorder moves clip content");
    // Delayed ACK: every completed repaint already shows the final clips.
    let mut released = Vec::new();
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        released = midi_clip_bounds(&mut visual, &committed);
        assert_ne!(released, original);
    }
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        assert!(v.sections.pending.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(midi_clip_bounds(&mut visual, &committed), released);

    // A stale stamp previews the move, then its refusal restores canonical clips.
    let (down, to) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let renamed = editor.update(cx, |v, _| {
        v.service
            .submit(Request::Edit(Command::RenameTrack {
                track: committed.tracks()[0].id(),
                name: "renamed".into(),
            }))
            .unwrap();
        next(&v.service)
    });
    assert!(renamed.error.is_none(), "{:?}", renamed.error);
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let refused = editor.update(cx, |v, _| next(&v.service));
    assert!(refused.error.is_some(), "stale section stamp");
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    assert_ne!(midi_clip_bounds(&mut visual, &committed), released);
    editor.update(cx, |v, cx| {
        // The refusal carries the newer canonical state (the rename).
        v.receive(refused);
        assert!(v.sections.pending.is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(midi_clip_bounds(&mut visual, &committed), released);
}

#[gpui::test]
fn backspace_deletes_section_with_device_automation_and_undo_restores_it(cx: &mut TestAppContext) {
    use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId};
    use hex_project::{AutomationInterpolation, AutomationPoint, AutomationValue};

    let (handle, editor, [intro, chorus]) = fixture(cx);
    let beat = |n| BeatTime::from_quarters(n).unwrap();
    let automation_point = |at, value| AutomationPoint {
        beat: beat(at),
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Step,
    };
    let target = DeviceAutomationTarget::HostInputGain;
    let (device, mut before) = editor.update(cx, |v, cx| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        let state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap();
        v.service
            .submit(Request::Edit(Command::InsertDevice {
                track,
                before: None,
                state,
            }))
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        let Some(Created::Device(device)) = event.created else {
            panic!("device result")
        };
        v.receive(event);
        v.service
            .submit(Request::Edit(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: vec![
                    automation_point(0, 0.25),
                    automation_point(9, 0.5),
                    automation_point(12, 0.75),
                    automation_point(14, 1.0),
                ],
            }))
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        let before = event.state.project.snapshot();
        v.receive(event);
        cx.notify();
        (device, before)
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let _ = bounds(&mut visual, 1);
    let down = editor.read_with(cx, |v, _| position(v, 10));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("backspace");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(
        event.error.is_none(),
        "section Backspace: {:?}",
        event.error
    );
    let project = &event.state.project;
    assert!(project.arrangement_section(chorus).is_none());
    assert!(project.arrangement_section(intro).is_some());
    assert_eq!(
        project.tracks()[0]
            .midi_clips()
            .iter()
            .map(|clip| clip.start().raw() + clip.length().raw())
            .max(),
        Some(beat(12).raw()),
    );
    let lane = project
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    assert_eq!(
        lane.points,
        [
            automation_point(0, 0.25),
            automation_point(8, 0.75),
            automation_point(10, 1.0),
        ]
    );
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-section-1").is_none());

    visual.simulate_keystrokes("cmd-z");
    let event = editor.update(cx, |v, _| next(&v.service));
    assert!(event.error.is_none(), "{:?}", event.error);
    let restored = event.state.project.snapshot();
    assert!(restored.next_id >= before.next_id);
    before.next_id = restored.next_id;
    assert_eq!(restored, before);
    editor.update(cx, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    let _ = bounds(&mut visual, 1);
}

#[gpui::test]
fn section_name_palette_delete_and_text_focus_are_guarded(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let down = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(v.sections.naming());
        assert!(v.clip_name_edit.is_none());
    });
    visual.simulate_keystrokes("backspace");
    editor.read_with(cx, |v, cx| {
        assert!(
            v.sections
                .name
                .as_ref()
                .unwrap()
                .input
                .read(cx)
                .text()
                .is_empty()
        );
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_section(ids[0])
                .is_some()
        );
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            assert!(!v.finalize_section_name_for_open(window, cx));
            assert!(v.sections.naming());
            v.new_project(hex_session::DocumentAudioMode::Resident, window, cx);
            assert!(!v.persistence.project_open.busy());
            assert!(v.sections.naming());
            assert!(!v.persistence.opening_project);
        })
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.simulate_mouse_down(down, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let menu = v.context_menu.as_ref().unwrap();
        let choices: Vec<_> = menu
            .items
            .iter()
            .filter_map(|item| {
                if let super::super::super::context_menu::EditorContextAction::Section(
                    MenuAction {
                        kind: MenuKind::Color(color),
                        ..
                    },
                ) = item.action
                {
                    Some(color)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(choices, ElementColor::TRACK_CHOICES);
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-r");
    visual.simulate_input("Bridge X");
    visual.run_until_parked();
    editor.read_with(cx, |v, cx| {
        let input = v.sections.name.as_ref().unwrap().input.read(cx);
        assert_eq!(input.text(), "Bridge X");
        assert_eq!(input.painted_text(), Some("bridge x"));
    });
    visual.simulate_keystrokes("enter");
    let terminal = editor.update(cx, |v, _| next(&v.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_section(ids[0])
                .unwrap()
                .name(),
            "Bridge X"
        );
        cx.notify();
    });
    visual.run_until_parked();
    let original_fill = painted_fill(&mut visual, 0);
    visual.simulate_mouse_down(down, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    let green = editor.read_with(cx, |v, _| {
        v.context_menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|item| item.label == "green")
            .unwrap()
    });
    for _ in 0..green {
        visual.simulate_keystrokes("down");
    }
    visual.simulate_keystrokes("enter");
    let terminal = editor.update(cx, |v, _| next(&v.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_section(ids[0])
                .unwrap()
                .color(),
            ElementColor::Green
        );
        cx.notify();
    });
    visual.run_until_parked();
    assert_ne!(painted_fill(&mut visual, 0), original_fill);
    let tracks = editor.read_with(cx, |v, _| {
        v.state.as_ref().unwrap().project.tracks().to_vec()
    });
    visual.simulate_keystrokes("alt-cmd-backspace");
    let terminal = editor.update(cx, |v, _| next(&v.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_section(ids[0])
                .is_none()
        );
        assert_eq!(v.state.as_ref().unwrap().project.tracks(), tracks);
        cx.notify();
    });
    // A stale menu action cannot recolor any new canonical source.
    editor.update(cx, |v, _| {
        let state = v.state.as_ref().unwrap();
        let action = MenuAction {
            generation: v.project_generation,
            revision: state.revision,
            section: Some(ids[1]),
            kind: MenuKind::Color(ElementColor::Green),
        };
        v.project_generation += 1;
        assert!(v.section_menu_stale(&action));
        assert!(v.sections.pending.is_none());
    });
}

#[gpui::test]
fn painted_section_release_uses_stamped_queue_and_keeps_success_preview(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (start, end) = editor.read_with(cx, |v, _| (position(v, 5), position(v, 7)));
    visual.simulate_event(MouseDownEvent {
        position: start,
        button: MouseButton::Left,
        click_count: 2,
        modifiers: Modifiers::default(),
        first_mouse: false,
    });
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = bounds(&mut visual, 2);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(cx, |v, _| {
        assert!(!v.sections.held());
        let p = v
            .sections
            .pending
            .as_ref()
            .expect("released pending display");
        assert!(
            !p.receipt.is_queued(),
            "immediate enqueue stamps the exact receipt"
        );
        next(&v.service)
    });
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    for _ in 0..3 {
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        assert_eq!(bounds(&mut visual, 2), preview);
    }
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        assert!(v.sections.pending.is_none());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .len(),
            3
        );
        cx.notify();
    });
    visual.run_until_parked();
    // The ID becomes project owned and rows re-sort, preserving the final
    // geometry through the canonical installation in the same receive.
    assert_eq!(bounds(&mut visual, 1), preview);
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.clone());
    let (source, target) = editor.read_with(cx, |v, _| (position(v, 10), position(v, 0)));
    visual.simulate_mouse_down(source, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("section-insertion-preview").is_none());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let terminal = editor.update(cx, |v, _| next(&v.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        let sections = v.state.as_ref().unwrap().project.arrangement_sections();
        assert_eq!(sections[0].name(), "Chorus");
        assert_eq!(sections[1].name(), "Intro");
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-z");
    let terminal = editor.update(cx, |v, _| next(&v.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        let restored = v.state.as_ref().unwrap().project.snapshot();
        let mut expected = before.snapshot();
        assert!(
            restored.next_id >= expected.next_id,
            "undo never reuses project identities"
        );
        expected.next_id = restored.next_id;
        assert_eq!(
            restored, expected,
            "one undo restores every musical and structural field"
        );
        cx.notify();
    });
}

#[gpui::test]
fn empty_track_list_can_paint_sections_and_queue_refusal_retires_display(cx: &mut TestAppContext) {
    let (handle, editor, _) = fixture(cx);
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
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (start, end) = editor.read_with(cx, |v, _| (position(v, 5), position(v, 7)));
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
    // Held gesture then queue saturation between update and submission:
    // the immediate refusal must retire released paint synchronously.
    editor.update(cx, |v, cx| {
        let g = v.sections.gesture.take().unwrap();
        let command = g.command.unwrap();
        for _ in 0..super::super::super::UI_REQUEST_CAPACITY {
            v.pending_requests.push_back(Request::Undo);
        }
        v.submit_section(command, g.display, g.generation, g.revision);
        assert!(v.sections.pending.is_none());
        v.pending_requests.clear();
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("arrangement-section-2").is_none());
}

#[gpui::test]
fn ordinary_clip_and_text_focus_release_section_keyboard_ownership(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let section = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(section, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(section, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| assert_eq!(v.sections.selected, Some(ids[0])));
    let clip = editor.read_with(cx, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].midi_clips()[0].id()
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .expect("ordinary clip name");
    visual.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(v.sections.selected.is_none());
        assert_eq!(v.selected_clip, Some(clip));
    });
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert!(v.clip_name_edit.is_some());
        assert!(!v.sections.naming());
    });
    visual.simulate_keystrokes("escape backspace");
    let terminal = editor.update(cx, |v, _| next(&v.service));
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    editor.update(cx, |v, cx| {
        v.receive(terminal);
        let project = &v.state.as_ref().unwrap().project;
        assert!(project.midi_clip(clip).is_none());
        assert_eq!(project.arrangement_sections().len(), 2);
        cx.notify();
    });
    visual.run_until_parked();
    let section = editor.read_with(cx, |v, _| position(v, 2));
    visual.simulate_mouse_down(section, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(section, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |v, _| assert_eq!(v.sections.selected, Some(ids[0])));
    visual.update(|window, cx| {
        editor.read_with(cx, |v, cx| v.browser.search.read(cx).focus(window));
        window.refresh();
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.sections.selected.is_none()));
    visual.simulate_keystrokes("cmd-r backspace");
    editor.read_with(cx, |v, _| {
        assert!(!v.sections.naming());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .len(),
            2
        );
    });
    let return_track = editor.update(cx, |v, cx| {
        v.service
            .submit(Request::Edit(Command::AddReturnTrack {
                name: "return".into(),
            }))
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none());
        let Some(Created::ReturnTrack(id)) = event.created else {
            panic!("return result")
        };
        v.receive(event);
        cx.notify();
        id
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.focus.focus(window);
            v.select_section_metadata(Some(ids[0]));
            assert!(v.section_has_focus(window));
            // Keyboard/programmatic device focus preserves the ordinary track
            // selection and therefore must explicitly supersede Main ownership.
            assert!(v.focus_return_details(return_track, window, cx));
            assert!(!v.section_has_focus(window));
            assert!(!v.delete_section(window));
            assert!(!v.section_name_begin(window, cx));
            assert!(v.sections.pending.is_none());
            cx.notify();
        })
    });
    visual.run_until_parked();
    editor.read_with(cx, |v, _| assert!(v.sections.selected.is_none()));
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.select_section_metadata(Some(ids[0]));
            v.device_panel.main_selected = None;
            assert!(!v.section_has_focus(window));
            assert!(!v.delete_section(window));
            assert!(!v.section_name_begin(window, cx));
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_sections()
                    .len(),
                2
            );
            cx.notify();
        })
    });
}

#[gpui::test]
fn section_cursor_zones_distinguish_inner_edges_and_shared_seam(cx: &mut TestAppContext) {
    let (handle, _editor, _) = touching_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let left = bounds(&mut visual, 0);
    let seam = left.right();
    for (offset, cursor) in [
        (-3.0, gpui::CursorStyle::ResizeRightBracket),
        (0.0, gpui::CursorStyle::ResizeLeftRight),
        (3.0, gpui::CursorStyle::ResizeLeftBracket),
    ] {
        visual.simulate_mouse_move(
            point(seam + px(offset), left.center().y),
            None,
            Modifiers::default(),
        );
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(cursor),
            "zone {offset}"
        );
    }
}

#[gpui::test]
fn command_snap_changes_live_without_entering_marker_mode(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let edge = bounds(&mut visual, 0);
    let down = point(edge.right() - px(1.0), edge.center().y);
    let to = editor.read_with(cx, |v, _| {
        let a = position(v, 6);
        let b = position(v, 7);
        point(a.x + (b.x - a.x) * 0.37, a.y)
    });
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    for free in [false, true, false, true] {
        visual.simulate_modifiers_change(Modifiers {
            platform: free,
            ..Modifiers::default()
        });
        editor.read_with(cx, |v, _| {
            let Some(Command::ResizeArrangementSectionEdge {
                section, boundary, ..
            }) = &v.sections.gesture.as_ref().unwrap().command
            else {
                panic!("command must invert snapping without selecting marker editing");
            };
            assert_eq!(*section, ids[0]);
            if free {
                assert!(*boundary > BeatTime::from_quarters(6).unwrap());
                assert!(*boundary < BeatTime::from_quarters(7).unwrap());
            } else {
                assert_eq!(*boundary, BeatTime::from_quarters(6).unwrap());
            }
        });
    }
    visual.simulate_keystrokes("escape");
    editor.read_with(cx, |v, _| assert!(v.sections.gesture.is_none()));
}

#[gpui::test]
fn control_alias_inverts_snap_and_option_command_selects_marker_resize(cx: &mut TestAppContext) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for marker_only in [false, true] {
        for free in [false, true] {
            let edge = bounds(&mut visual, 0);
            let down = point(edge.right() - px(1.0), edge.center().y);
            let to = editor.read_with(cx, |v, _| {
                let a = position(v, 6);
                let b = position(v, 7);
                point(a.x + (b.x - a.x) * 0.37, a.y)
            });
            let modifiers = Modifiers {
                platform: marker_only,
                alt: marker_only,
                control: free,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
            editor.read_with(cx, |v, _| {
                let command = v
                    .sections
                    .gesture
                    .as_ref()
                    .unwrap()
                    .command
                    .as_ref()
                    .unwrap();
                let value = match command {
                    Command::ResizeArrangementSectionMarker {
                        section,
                        boundary: end,
                        ..
                    } if marker_only && *section == ids[0] => *end,
                    Command::ResizeArrangementSectionEdge {
                        section, boundary, ..
                    } if !marker_only && *section == ids[0] => *boundary,
                    other => panic!("wrong modifier mode: {other:?}"),
                };
                if free {
                    assert!(
                        value > BeatTime::from_quarters(6).unwrap()
                            && value < BeatTime::from_quarters(7).unwrap()
                    );
                } else {
                    assert_eq!(value, BeatTime::from_quarters(6).unwrap());
                }
            });
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
        }
    }
}

#[gpui::test]
fn shared_seam_drags_publish_both_edges_atomically_and_option_command_preserves_music(
    cx: &mut TestAppContext,
) {
    for marker_only in [false, true] {
        let (handle, editor, ids) = touching_fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let left = bounds(&mut visual, 0);
        let down = point(left.right(), left.center().y);
        let to = editor.read_with(cx, |v, _| position(v, 5));
        let modifiers = Modifiers {
            platform: marker_only,
            alt: marker_only,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
        visual.run_until_parked();
        editor.read_with(cx, |v, _| {
            let command = v
                .sections
                .gesture
                .as_ref()
                .unwrap()
                .command
                .as_ref()
                .unwrap();
            match command {
                Command::ResizeArrangementSectionMarker {
                    section,
                    edge: hex_project::TrimEdge::Right,
                    boundary,
                } if marker_only
                    && *section == ids[0]
                    && *boundary == BeatTime::from_quarters(5).unwrap() => {}
                Command::ResizeArrangementSectionBoundary {
                    left,
                    right,
                    boundary,
                    marker_only: false,
                } if !marker_only
                    && *left == ids[0]
                    && *right == ids[1]
                    && *boundary == BeatTime::from_quarters(5).unwrap() => {}
                other => panic!("wrong seam command {other:?}"),
            }
        });
        let preview = [bounds(&mut visual, 0), bounds(&mut visual, 1)];
        visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
        let event = editor.update(cx, |v, _| next(&v.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        for _ in 0..3 {
            visual.update(|window, _| window.refresh());
            visual.run_until_parked();
            assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
            assert!(visual.debug_bounds("section-insertion-preview").is_none());
        }
        editor.update(cx, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!([bounds(&mut visual, 0), bounds(&mut visual, 1)], preview);
        editor.read_with(cx, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(
                p.arrangement_section(ids[0]).unwrap().start(),
                BeatTime::ZERO
            );
            assert_eq!(
                p.arrangement_section(ids[0]).unwrap().end(),
                BeatTime::from_quarters(5).unwrap()
            );
            assert_eq!(
                p.arrangement_section(ids[1]).unwrap().start(),
                BeatTime::from_quarters(5).unwrap()
            );
            assert_eq!(
                p.arrangement_section(ids[1]).unwrap().end(),
                BeatTime::from_quarters(8).unwrap()
            );
            assert_eq!(p.snapshot().tracks == before.tracks, marker_only);
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
}

#[gpui::test]
fn option_command_after_shared_seam_capture_can_resize_past_the_neighbor(cx: &mut TestAppContext) {
    let (handle, editor, ids) = touching_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let left = bounds(&mut visual, 0);
    let down = point(left.right(), left.center().y);
    let to = editor.read_with(cx, |v, _| position(v, 10));
    let marker_modifier = Modifiers {
        platform: true,
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_modifiers_change(marker_modifier);
    visual.simulate_mouse_move(to, MouseButton::Left, marker_modifier);
    visual.simulate_mouse_up(to, MouseButton::Left, marker_modifier);
    editor.update(cx, |v, cx| {
        let event = next(&v.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        v.receive(event);
        let after = v.state.as_ref().unwrap().project.snapshot();
        assert_eq!(after.arrangement_sections.len(), 1);
        assert_eq!(after.arrangement_sections[0].id, ids[0].value());
        assert_eq!(
            after.arrangement_sections[0].end,
            BeatTime::from_quarters(10).unwrap()
        );
        let mut music = after;
        music.arrangement_sections = before.arrangement_sections.clone();
        assert_eq!(music, before);
        cx.notify();
    });
}

#[gpui::test]
fn inner_zones_resize_only_the_touched_edge_and_leave_the_neighbor_in_place(
    cx: &mut TestAppContext,
) {
    for (offset, target, index) in [(-3.0, 3, 0usize), (3.0, 5, 1usize)] {
        for marker_only in [false, true] {
            let (handle, editor, ids) = touching_fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let left = bounds(&mut visual, 0);
            let neighbor = editor.read_with(cx, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(ids[1 - index])
                    .unwrap()
                    .clone()
            });
            let down = point(left.right() + px(offset), left.center().y);
            let to = editor.read_with(cx, |v, _| position(v, target));
            let modifiers = Modifiers {
                platform: marker_only,
                alt: marker_only,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
            visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
            let event = editor.update(cx, |v, _| next(&v.service));
            assert!(event.error.is_none(), "{:?}", event.error);
            editor.update(cx, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            editor.read_with(cx, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                assert_eq!(p.arrangement_section(ids[1 - index]), Some(&neighbor));
                let section = p.arrangement_section(ids[index]).unwrap();
                assert_eq!(
                    if index == 0 {
                        section.end()
                    } else {
                        section.start()
                    },
                    BeatTime::from_quarters(target).unwrap()
                );
            });
        }
    }
}
