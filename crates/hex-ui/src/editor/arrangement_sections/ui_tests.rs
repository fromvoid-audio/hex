use super::*;

use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point, px};

use hex_project::Created;

use hex_session::service::Service;

use std::time::{Duration, Instant};

fn painted_fill(visual: &mut VisualTestContext, index: usize) -> Option<gpui::Background> {
    let selector = Box::leak(format!("arrangement-section-{index}").into_boxed_str());
    painted_fill_for(visual, selector)
}

fn painted_fill_for(
    visual: &mut VisualTestContext,
    selector: &'static str,
) -> Option<gpui::Background> {
    let body = visual.debug_bounds(selector)?;
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    visual
        .rendered_filled_quads(selector)
        .into_iter()
        .filter(|(b, _)| {
            (b.top() / scale - f32::from(body.top())).abs() < 0.1
                && (b.size.width / scale - f32::from(body.size.width)).abs() < 0.1
                && b.size.height / scale > 2.0
        })
        .map(|(_, color)| color)
        .next_back()
}

#[track_caller]
pub(in crate::editor) fn next(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(Instant::now() < deadline, "service completion");
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(in crate::editor) fn fixture(
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, [SectionId; 2]) {
    cx.update(super::super::bind_editor_keys);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "track".into(),
        }))
        .unwrap();
    let state = next(&service).state;
    let track = state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16).unwrap(),
        }))
        .unwrap();
    let mut state = next(&service).state;
    let mut ids = Vec::new();
    for (start, end, name, color) in [
        (0, 4, "Intro", ElementColor::Blue),
        (8, 12, "Chorus", ElementColor::Rose),
    ] {
        service
            .submit(Request::EditArrangementSection {
                project_generation: 0,
                revision: state.revision,
                command: Command::AddArrangementSection {
                    name: name.into(),
                    color,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                },
            })
            .unwrap();
        let event = next(&service);
        assert!(event.error.is_none(), "{:?}", event.error);
        let Some(Created::ArrangementSection(id)) = event.created else {
            panic!("section result")
        };
        ids.push(id);
        state = event.state;
    }
    let (handle, editor) = section_test_window(cx, service, state);
    (handle, editor, [ids[0], ids[1]])
}

fn section_test_window(
    cx: &mut TestAppContext,
    service: Service,
    state: Arc<ViewState>,
) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
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
    (handle, editor)
}

pub(in crate::editor) fn position(view: &EditorView, quarters: u64) -> gpui::Point<gpui::Pixels> {
    let [left, top, width, _] = view.section_layout().unwrap();
    let state = view.state.as_ref().unwrap();
    let viewport = view.visible_viewport(state.timeline_frames);
    let frame = state
        .project
        .tempo_map()
        .beat_to_frame(BeatTime::from_quarters(quarters).unwrap())
        .unwrap();
    point(
        px(left + (frame - viewport.start) as f32 / viewport.length as f32 * width),
        px(top
            + super::super::time_selection::header_hit_height(
                super::super::tracks::TRACK_ROW_HEIGHT,
            ) * 0.5),
    )
}

fn bounds(visual: &mut VisualTestContext, index: usize) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("arrangement-section-{index}").into_boxed_str(),
        ))
        .expect("painted section")
}

fn midi_clip_bounds(
    visual: &mut VisualTestContext,
    project: &hex_project::Project,
) -> Vec<(u64, gpui::Bounds<gpui::Pixels>)> {
    project.tracks()[0]
        .midi_clips()
        .iter()
        .map(|clip| {
            let id = clip.id().value();
            let selector = Box::leak(format!("midi-clip-visual-{id}").into_boxed_str());
            (id, visual.debug_bounds(selector).expect("painted clip"))
        })
        .collect()
}

fn touching_fixture(
    cx: &mut TestAppContext,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, [SectionId; 2]) {
    let (handle, editor, ids) = fixture(cx);
    editor.update(cx, |v, cx| {
        v.service
            .submit(Request::EditArrangementSection {
                project_generation: v.project_generation,
                revision: v.state.as_ref().unwrap().revision,
                command: Command::SetArrangementSectionRange {
                    section: ids[1],
                    start: BeatTime::from_quarters(4).unwrap(),
                    end: BeatTime::from_quarters(8).unwrap(),
                },
            })
            .unwrap();
        let event = next(&v.service);
        assert!(event.error.is_none(), "{:?}", event.error);
        v.receive(event);
        cx.notify();
    });
    (handle, editor, ids)
}

/// Tick the real worker pump until every edit and queued follow-up lands.
fn settle(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let idle = editor.update(visual, |v, cx| {
            v.tick(cx);
            !v.edit_pending() && v.sections.pending.is_none() && v.sections.queued.is_none()
        });
        if idle {
            break;
        }
        assert!(Instant::now() < deadline, "section edits settle");
        std::thread::sleep(Duration::from_millis(2));
    }
    visual.run_until_parked();
}

fn newer_selection_rejects_ahead_section_copy(cx: &mut TestAppContext, pointer_first: bool) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.select_section_metadata(Some(ids[0]));
        let first = if pointer_first {
            Command::DuplicateArrangementSectionAt {
                section: ids[0],
                target: ids[0],
                after: true,
            }
        } else {
            Command::DuplicateArrangementSection(ids[0])
        };
        view.section_metadata(first);
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
    let terminal = editor.read_with(&visual, |view, _| next(&view.service));
    editor.update(&mut visual, |view, _| {
        view.select_section_metadata(Some(ids[1]));
        assert!(view.observe_duplicate_view(first));
        assert!(
            view.sections.ahead.is_none(),
            "newer selection must not admit a successor"
        );
        view.receive(terminal);
        assert_eq!(view.sections.selected, Some(ids[1]));
        // The already requested command remains in the ordinary queue.
        assert!(view.sections.queued.is_some());
    });
}

// Capture the real worker receipt while recovery is held. Each test chooses
// when the UI receives it so a repaint or audio tick cannot drain it early.
fn section_copy_completion(service: &Service, revision: u64) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match service.try_event() {
            Ok(event) => {
                assert!(
                    event.request_complete && event.error.is_none(),
                    "{:?}",
                    event.error
                );
                assert_eq!(event.state.revision, revision);
                return event;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::yield_now()
            }
            Err(error) => panic!("section copy completion unavailable: {error}"),
        }
    }
}

fn held_recovery_allows_queued_duplicate(cx: &mut TestAppContext, copy_kind: u8) {
    use std::sync::atomic::Ordering::Acquire;

    let environment = super::super::test_environment::Environment::new().unwrap();
    let recovery = environment.root().join("held-section.hex-journal");
    let mut commands = vec![
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
    if copy_kind == 3 {
        commands.push(Command::AddArrangementSection {
            name: "target".into(),
            color: ElementColor::Blue,
            start: BeatTime::from_quarters(12).unwrap(),
            end: BeatTime::from_quarters(16).unwrap(),
        });
    }
    let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
        cx,
        commands,
        false,
        true,
        Some(recovery.clone()),
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.run_until_parked();
    let mut gate = hex_session::service::hold_next_checkpoint(recovery).unwrap();
    let (revision, expected) = editor.update(&mut visual, |view, _| {
        let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
        view.select_section_metadata(Some(section));
        view.interaction.play_start_at_beginning = false;
        let expected = view.playback_start_frame();
        let revision = view.state.as_ref().unwrap().revision;
        let first = if copy_kind == 1 {
            Command::DuplicateArrangementSectionAt {
                section,
                target: section,
                after: true,
            }
        } else if copy_kind == 2 {
            Command::AppendArrangementSectionCopy {
                section,
                start: BeatTime::from_quarters(16).unwrap(),
                gap_name: "gap".into(),
                gap_color: ElementColor::Blue,
            }
        } else if copy_kind == 3 {
            let target = view.state.as_ref().unwrap().project.arrangement_sections()[1].id();
            Command::ReplaceArrangementSection {
                source: section,
                target,
                copy: true,
            }
        } else {
            Command::DuplicateArrangementSection(section)
        };
        view.section_metadata(first);
        view.section_metadata(Command::DuplicateArrangementSection(section));
        (revision, expected)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut entered_at = None;
    let mut first = None;
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        if entered_at.is_none() {
            match gate.try_entered() {
                Ok(()) => entered_at = Some(Instant::now()),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("checkpoint gate failed: {error}"),
            }
        }
        if first.is_none() {
            first = editor.read_with(&visual, |view, _| {
                view.service.try_arrangement_edit_view().ok()
            });
        }
        let applied = editor.read_with(&visual, |view, _| {
            view.audio_status().is_some_and(|status| {
                !status.stopped_edit_pending()
                    && status
                        .recording_source()
                        .is_some_and(|source| source.context.editor_revision == revision + 1)
            })
        });
        if entered_at.is_some() && first.is_some() && applied {
            break;
        }
        assert!(
            entered_at.is_none_or(|time| time.elapsed() < Duration::from_millis(1500)),
            "first source did not arrive within the held checkpoint"
        );
        assert!(
            Instant::now() < deadline,
            "held first checkpoint and applied source"
        );
        std::thread::yield_now();
    }
    let first_copy = match &first.as_ref().unwrap().kind {
        hex_session::service::ArrangementEditViewKind::Duplicate(
            hex_project::Created::ArrangementSection(id),
        ) => *id,
        _ => panic!("expected first copied section"),
    };
    let first_terminal = editor.read_with(&visual, |view, _| {
        section_copy_completion(&view.service, revision + 1)
    });
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_duplicate_view(first.take().unwrap()));
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
        assert!(
            entered_at.is_some_and(|time| time.elapsed() < Duration::from_millis(1500)),
            "second copy missed the held checkpoint"
        );
        assert!(
            Instant::now() < deadline,
            "second copy while recovery is held"
        );
        std::thread::yield_now();
    };
    let committed = second.state.upgrade().unwrap();
    assert_eq!(committed.revision, revision + 2);
    let first_copy_start = committed
        .project
        .arrangement_section(first_copy)
        .unwrap()
        .start();
    assert_eq!(
        first_copy_start,
        BeatTime::from_quarters(if copy_kind == 2 { 20 } else { 16 }).unwrap(),
        "the queued duplicate must target the original section"
    );
    let expected = if copy_kind != 0 {
        committed
            .project
            .tempo_map()
            .beat_to_frame(first_copy_start)
            .unwrap()
    } else {
        expected
    };
    let second_terminal = editor.read_with(&visual, |view, _| {
        section_copy_completion(&view.service, revision + 2)
    });
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_duplicate_view(second));
        cx.notify();
    });
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
        assert!(
            entered_at.is_some_and(|time| time.elapsed() < Duration::from_millis(1500)),
            "applied source missed the held checkpoint"
        );
        assert!(
            Instant::now() < deadline,
            "applied second source while recovery is held"
        );
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
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
    visual.run_until_parked();
    let marker = visual.debug_bounds("arrangement-section-1").unwrap();
    gate.release()
        .expect("checkpoint must still be held after the second copy and Play");
    editor.update(&mut visual, |view, cx| {
        view.receive(first_terminal);
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &committed
        ));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("arrangement-section-1"), Some(marker));
    editor.update(&mut visual, |view, cx| {
        view.receive(second_terminal);
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
        view.toggle_play();
        cx.notify();
    });
    fixture.backend.render(&mut [[0.0; 2]; 512]).unwrap();
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("arrangement-section-1"), Some(marker));
    editor.update(&mut visual, |view, cx| {
        view.history_action(Request::Undo, cx)
    });
    // A user can press Redo as soon as Undo is visible, before the callback
    // installs its graph. Render the Undo lease once, then hold that installation.
    loop {
        let requested = editor.read_with(&visual, |view, _| {
            view.audio_status().unwrap().stopped_edit_pending()
        });
        if requested {
            break;
        }
        assert!(Instant::now() < deadline, "undo lease request");
        std::thread::yield_now();
    }
    fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
    loop {
        let undone = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision == revision + 3)
        });
        if undone {
            break;
        }
        assert!(Instant::now() < deadline, "undo after durable copies");
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .arrangement_sections()
                .len(),
            if copy_kind == 2 { 3 } else { 2 }
        )
    });
    let previous_audio_revision = editor.update(&mut visual, |view, cx| {
        let source = view.audio_status().unwrap().recording_source().unwrap();
        assert_eq!(source.context.editor_revision, revision + 2);
        view.history_action(Request::Redo, cx);
        source.audio_revision
    });
    let observed = fixture.section_wait_observed.as_ref().unwrap();
    while observed.load(Acquire) <= previous_audio_revision {
        assert!(
            Instant::now() < deadline,
            "Redo admission behind the unapplied Undo graph"
        );
        std::thread::yield_now();
    }
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let redone = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            assert!(view.error.is_none(), "redo failed: {:?}", view.error);
            view.state
                .as_ref()
                .is_some_and(|state| state.revision == revision + 4)
                && view
                    .audio_status()
                    .unwrap()
                    .recording_source()
                    .is_some_and(|source| source.context.editor_revision == revision + 4)
        });
        if redone {
            break;
        }
        assert!(Instant::now() < deadline, "redo after durable copies");
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(*view.state.as_ref().unwrap().project, *committed.project);
    });
    fixture.finish(cx);
    drop(environment);
}

fn pointer_section_copy_plays_from_the_selected_copy(cx: &mut TestAppContext, copy_kind: u8) {
    let environment = super::super::test_environment::Environment::new().unwrap();
    let recovery = environment.root().join("held-pointer-section.hex-journal");
    let mut commands = vec![
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
    if copy_kind == 2 {
        commands.push(Command::AddArrangementSection {
            name: "target".into(),
            color: ElementColor::Blue,
            start: BeatTime::from_quarters(12).unwrap(),
            end: BeatTime::from_quarters(16).unwrap(),
        });
    }
    let mut fixture = super::super::worker_ui_fixture::open_with_section_and_recovery(
        cx,
        commands,
        false,
        true,
        Some(recovery.clone()),
    );
    let editor = fixture.editor.clone();
    let mut visual = VisualTestContext::from_window(fixture.handle, cx);
    visual.run_until_parked();
    let mut gate = hex_session::service::hold_next_checkpoint(recovery).unwrap();
    let original_revision = editor.update(&mut visual, |view, _| {
        let section = view.state.as_ref().unwrap().project.arrangement_sections()[0].id();
        view.select_section_metadata(Some(section));
        view.interaction.play_start_at_beginning = false;
        let revision = view.state.as_ref().unwrap().revision;
        let command = if copy_kind == 1 {
            Command::AppendArrangementSectionCopy {
                section,
                start: BeatTime::from_quarters(16).unwrap(),
                gap_name: "gap".into(),
                gap_color: ElementColor::Blue,
            }
        } else if copy_kind == 2 {
            let target = view.state.as_ref().unwrap().project.arrangement_sections()[1].id();
            Command::ReplaceArrangementSection {
                source: section,
                target,
                copy: true,
            }
        } else {
            Command::DuplicateArrangementSectionAt {
                section,
                target: section,
                after: true,
            }
        };
        view.section_metadata(command);
        revision
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
    let committed = observation.state.upgrade().unwrap();
    loop {
        match gate.try_entered() {
            Ok(()) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline, "checkpoint was not held");
                std::thread::yield_now();
            }
            Err(error) => panic!("checkpoint gate failed: {error}"),
        }
    }
    let terminal = editor.read_with(&visual, |view, _| {
        section_copy_completion(&view.service, original_revision + 1)
    });
    let copied = match observation.kind {
        hex_session::service::ArrangementEditViewKind::Duplicate(
            hex_project::Created::ArrangementSection(id),
        ) => id,
        _ => panic!("unexpected committed view"),
    };
    let expected = committed
        .project
        .tempo_map()
        .beat_to_frame(
            committed
                .project
                .arrangement_section(copied)
                .unwrap()
                .start(),
        )
        .unwrap();
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_duplicate_view(observation));
        assert_eq!(view.state.as_ref().unwrap().revision, original_revision);
        cx.notify();
    });
    // Apply the copied graph before the editor consumes its publication.
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let applied = editor.read_with(&visual, |view, _| {
            view.audio_status()
                .unwrap()
                .recording_source()
                .is_some_and(|source| source.context.editor_revision == original_revision + 1)
        });
        if applied {
            break;
        }
        assert!(Instant::now() < deadline, "applied copied section source");
        std::thread::yield_now();
    }
    editor.read_with(&visual, |view, _| {
        assert!(
            view.section_copy_play_source().is_none(),
            "callback application alone must leave play fenced"
        );
    });
    loop {
        fixture.backend.render(&mut [[0.0; 2]; 64]).unwrap();
        let ready = editor.update(&mut visual, |view, cx| {
            view.tick(cx);
            view.section_copy_play_source()
                .is_some_and(|source| source.context.editor_revision == original_revision + 1)
        });
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "copied section ready for play");
        std::thread::yield_now();
    }
    editor.update(&mut visual, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().revision, original_revision);
        assert!(
            view.service.try_event().is_err(),
            "terminal must still be held"
        );
        let observation = view
            .audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap();
        observation.output_identity += 1;
        assert!(
            view.section_copy_play_source().is_none(),
            "wrong output must refuse"
        );
        view.audio_pub
            .pending_publication_observation
            .as_mut()
            .unwrap()
            .output_identity -= 1;
        let epoch = view.interaction.selection_intent_epoch;
        view.advance_selection_intent();
        assert!(
            view.section_copy_play_source().is_none(),
            "changed selection must refuse"
        );
        view.interaction.selection_intent_epoch = epoch;
        view.interaction.play_start_at_beginning = true;
        assert_eq!(view.section_copy_play_start(), Some(0));
        view.interaction.play_start_at_beginning = false;
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
        assert!(
            view.audio_status()
                .unwrap()
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
    });
    gate.release()
        .expect("checkpoint remains held until after Play");
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        assert_eq!(view.playback_start_frame(), expected);
        cx.notify();
    });
    fixture.finish(cx);
}

fn g_merge_case(cx: &mut TestAppContext, focused: bool, leftward: bool) {
    let (handle, editor, ids) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (mut a, mut b) = editor.read_with(cx, |v, _| (position(v, 2), position(v, 10)));
    if leftward {
        std::mem::swap(&mut a, &mut b);
    }
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    if focused {
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
    visual.simulate_mouse_down(b, MouseButton::Left, shift);
    visual.simulate_mouse_up(b, MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        let mut selection = v.sections.selection.clone();
        selection.sort();
        let mut expected = ids.to_vec();
        expected.sort();
        assert_eq!(selection, expected, "error={:?}", v.error);
    });
    visual.simulate_keystrokes("g");
    editor.read_with(cx, |v, _| {
        assert!(
            matches!(
                v.sections.pending.as_ref().map(|p| &p.command),
                Some(Command::MergeArrangementSections(_))
            ),
            "error={:?} status={:?} track={:?} clip={:?} time={:?} main={:?}",
            v.error,
            v.status,
            v.selected_track,
            v.selected_clip,
            v.interaction
                .time_selection
                .map(|t| (t.anchor_frame, t.end_frame)),
            v.device_panel.main_selected,
        );
    });
}

#[gpui::test]
fn launch_action_geometry_matches_raw_exclusion_without_enlarging_tiny_sections(
    cx: &mut TestAppContext,
) {
    for empty in [false, true] {
        let (handle, editor, ids) = fixture(cx);
        if empty {
            editor.update(cx, |view, cx| {
                let track = view.state.as_ref().unwrap().project.tracks()[0].id();
                view.service
                    .submit(Request::Edit(Command::RemoveTrack(track)))
                    .unwrap();
                let event = next(&view.service);
                assert!(event.error.is_none());
                view.receive(event);
                cx.notify();
            });
        }
        let mut visual = VisualTestContext::from_window(handle, cx);
        for width in [880.0, 1440.0] {
            visual.simulate_resize(gpui::size(px(width), px(700.0)));
            for start in [0, 72_000] {
                editor.update(cx, |view, cx| {
                    view.viewport = Some(Viewport {
                        start,
                        length: 384_000,
                    });
                    cx.notify();
                });
                visual.run_until_parked();
                for (index, id) in ids.iter().enumerate() {
                    let header = visual
                        .debug_bounds(Box::leak(
                            format!("arrangement-section-header-{index}").into_boxed_str(),
                        ))
                        .unwrap();
                    let selector =
                        Box::leak(format!("section-launch-{}", id.value()).into_boxed_str());
                    let expected = super::super::section_launch::action_bounds(
                        f32::from(header.left()),
                        f32::from(header.top()),
                        f32::from(header.size.width),
                    );
                    assert_eq!(visual.debug_bounds(selector), expected);
                    if let Some(bounds) = expected {
                        let at = bounds.center();
                        editor.read_with(cx, |view, _| {
                            assert!(
                                view.pointer_hits_section_launch(f32::from(at.x), f32::from(at.y))
                            );
                            assert!(
                                !view.pointer_hits_section_launch(
                                    f32::from(header.left()) + 1.0,
                                    f32::from(at.y)
                                ),
                                "leading resize edge remains available"
                            );
                        });
                    }
                }
            }
        }
        let play = visual
            .debug_bounds(Box::leak(
                format!("section-launch-{}", ids[1].value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        let before = editor.read_with(cx, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for range_key in [false, true] {
            editor.update(cx, |view, _| {
                view.error = None;
                view.interaction.time_selection_key_held = range_key;
            });
            visual.simulate_mouse_down(play, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(play, MouseButton::Left, Modifiers::default());
            editor.read_with(cx, |view, _| {
                assert_eq!(
                    view.error.as_deref(),
                    Some("section playback source is unavailable")
                );
                assert!(!view.sections.held());
                assert!(view.interaction.time_selection_gesture.is_none());
                assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
        editor.update(cx, |view, _| {
            view.interaction.time_selection_key_held = false
        });
        let tiny = editor.update(cx, |view, cx| {
            let start = BeatTime::from_quarters(6).unwrap();
            view.service
                .submit(Request::EditArrangementSection {
                    project_generation: view.project_generation,
                    revision: view.state.as_ref().unwrap().revision,
                    command: Command::AddArrangementSection {
                        name: "tiny".into(),
                        color: ElementColor::Red,
                        start,
                        end: BeatTime::from_raw(start.raw() + 1),
                    },
                })
                .unwrap();
            let event = next(&view.service);
            assert!(event.error.is_none());
            let Some(Created::ArrangementSection(id)) = event.created else {
                panic!("tiny section");
            };
            view.receive(event);
            cx.notify();
            id
        });
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("section-launch-{}", tiny.value()).into_boxed_str()
                ))
                .is_none()
        );
        editor.update(cx, |view, cx| {
            view.service
                .submit(Request::EditArrangementSection {
                    project_generation: view.project_generation,
                    revision: view.state.as_ref().unwrap().revision,
                    command: Command::RemoveArrangementSection(ids[1]),
                })
                .unwrap();
            let event = next(&view.service);
            assert!(event.error.is_none());
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            let view = editor.read(cx);
            assert!(!view.sections.launch_focus.borrow().contains_key(&ids[1]));
            assert!(view.focus.is_focused(window));
        });
    }
}

#[gpui::test]
fn section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels(
    cx: &mut TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    use std::sync::Arc;
    let (handle, editor, ids) = fixture(cx);
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let region = hex_project::LoopRegion::new(0, 96_000, true).unwrap();
    editor.update(cx, |view, _| {
        view.service
            .submit(Request::Edit(Command::SetLoopRegion {
                start: region.start(),
                end: region.end(),
                enabled: true,
            }))
            .unwrap();
        let event = next(&view.service);
        assert!(event.error.is_none());
        view.receive(event);
        view.audio_pub.pending_playback = None;
    });
    let (source, project_generation, editor_revision) = editor.read_with(cx, |view, _| {
        let state = view.state.as_ref().unwrap();
        (
            state.project.clone(),
            view.project_generation,
            state.revision,
        )
    });
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &source,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    assert!(!prepared.arrangement_seek_requires_preroll());
    let control = output.publication.control().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 32_000_000,
                max_construction_peak_bytes: 32_000_000,
            },
        )
        .unwrap();
    let audio_revision = control
        .publish_with_admission_and_source_on_control(
            hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&prepared))
                .unwrap(),
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation,
                editor_revision,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    backend.render(&mut [[0.0; 2]; 1]).unwrap();
    let client = output.take_client().unwrap();
    editor.update(cx, |view, cx| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test audio".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        let state = Arc::clone(view.state.as_ref().unwrap());
        view.audio_pub.applied_project_revision = Some(state.revision);
        view.audio_pub.audible_playback = Some(super::super::PlaybackCandidate {
            clip_preview_identity: None,
            worker_requires_preroll: Some(false),
            observation: None,
            audio_revision,
            project_generation: view.project_generation,
            state,
            prepared_revision: Some(prepared),
            seek_request_id: None,
            device_preview_identity: None,
            tuning_preview_identity: None,
        });
        view.session_grid.quantization = hex_engine::LaunchQuantization::QuarterTriplet;
        view.toggle_section_autoplay();
        cx.notify();
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let clicked = editor.read_with(cx, |view, _| position(view, 1));
    visual.simulate_mouse_down(clicked, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(clicked, MouseButton::Left, Modifiers::default());
    editor.update(cx, |view, _| {
        view.tick_section_launch();
        let pending = view
            .audio_pub
            .prepared_seek
            .as_ref()
            .expect("stationary section click prepares a seek even without stateful devices");
        let section = pending.kind.section().unwrap();
        assert_eq!(section.region, region);
        assert_eq!(
            section.quantization,
            hex_engine::LaunchQuantization::QuarterTriplet
        );
        assert_eq!(view.sections.selected, Some(ids[0]));
        assert_eq!(
            view.state.as_ref().unwrap().project,
            source,
            "an already-matching loop needs no new history edit"
        );
        let cancellation = Arc::clone(&pending.worker_cancel);
        view.toggle_section_autoplay();
        assert!(cancellation.load(std::sync::atomic::Ordering::Acquire));
        assert!(view.audio_pub.latched_seek_intent.is_none());
        assert!(!view.section_autoplay);
    });
    // Turning autoplay off must still leave ordinary section dragging
    // available on an actually playing output.
    editor.update(cx, |view, _| {
        view.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::PlaybackCommand::Play)
            .unwrap();
    });
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let end = editor.read_with(cx, |view, _| position(view, 12));
    visual.simulate_mouse_down(clicked, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.audio_status()
                .unwrap()
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
        let gesture = view
            .sections
            .gesture
            .as_ref()
            .expect("live reorder capture");
        assert!(matches!(
            gesture.command,
            Some(Command::ReorderSectionAfter { .. }) | Some(Command::ReorderSectionBefore { .. })
        ));
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_eq!(view.state.as_ref().unwrap().project, source);
    });
    editor.update(cx, |view, _| {
        view.cancel_section_pointer();
    });
    visual.update(|window, cx| {
        window.cancel_pointer_capture_for(editor.entity_id(), cx);
    });
    visual.run_until_parked();
    let selector = Box::leak(format!("section-launch-{}", ids[0].value()).into_boxed_str());
    let target = visual
        .debug_bounds(selector)
        .expect("explicit section launch")
        .center();
    let selected = editor.read_with(cx, |view, _| view.sections.selected);
    visual
        .update(|window, cx| editor.read(cx).sections.launch_focus.borrow()[&ids[0]].focus(window));
    visual.simulate_keystrokes("enter");
    editor.read_with(cx, |view, _| {
        let pending = view
            .pending_loop_region()
            .expect("enter disables the old loop");
        assert_eq!(
            pending,
            hex_project::LoopRegion::new(0, 96_000, false).unwrap()
        );
        assert!(!view.section_autoplay);
        assert!(!view.sections.held());
        assert_eq!(view.sections.selected, selected);
        assert_eq!(view.state.as_ref().unwrap().project, source);
    });
    editor.update(cx, |view, _| {
        view.cancel_section_launch_wait();
        view.invalidate_prepared_seek();
        view.persistence.save_dialog_open = true;
    });
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(!view.section_launch_waiting());
        assert!(view.audio_pub.latched_seek_intent.is_none());
        assert!(
            view.audio_pub
                .prepared_seek
                .as_ref()
                .is_none_or(|pending| pending.cancelled)
        );
    });
    editor.update(cx, |view, _| {
        view.persistence.save_dialog_open = false;
        view.audio = None;
    });
}

#[gpui::test]
fn section_lower_body_paints_all_tracks_without_section_edit_even_when_focused(
    cx: &mut TestAppContext,
) {
    for focused in [false, true] {
        let (handle, editor, ids) = fixture(cx);
        editor.update(cx, |view, cx| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: "hidden empty track".into()
            })));
            let event = next(&view.service);
            assert!(event.error.is_none());
            view.receive(event);
            view.section_autoplay = false;
            view.test_probes.last_enqueued_edit = None;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (header, start, end) = editor.read_with(cx, |view, _| {
            let [_, top, _, height] = view.section_layout().unwrap();
            let a = position(view, 1);
            let b = position(view, 3);
            (
                point(a.x, px(top + 6.0)),
                point(a.x, px(top + height * 0.75)),
                point(b.x, px(top + height * 0.75)),
            )
        });
        visual.simulate_mouse_down(header, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.sections.selected, Some(ids[0]))
        });
        if focused {
            visual.simulate_keystrokes("q");
            visual.run_until_parked();
        }
        let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
        let hidden = before.tracks()[1].id();
        if focused {
            assert!(
                visual
                    .debug_bounds(Box::leak(
                        format!("arrangement-lane-{}", hidden.value()).into_boxed_str()
                    ))
                    .is_none()
            );
        }
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(
                view.sections.gesture.is_none(),
                "lower section body must delegate to the time-selection owner"
            );
            assert!(view.interaction.time_selection_gesture.is_some());
            assert!(
                view.sections.cursor.is_none(),
                "body range must not retain a second section edit cursor"
            );
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("main-project-time-selection").is_some());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            let range = view
                .normalized_time_selection()
                .expect("completed main range");
            let map = before.tempo_map();
            assert_eq!(
                (range.start(), range.end()),
                (
                    map.beat_to_frame(BeatTime::ONE).unwrap(),
                    map.beat_to_frame(BeatTime::from_quarters(3).unwrap())
                        .unwrap()
                )
            );
            assert_eq!(
                range.tracks(),
                before
                    .tracks()
                    .iter()
                    .map(|track| track.id())
                    .collect::<Vec<_>>()
            );
            assert!(view.interaction.time_selection_project_scope);
            assert!(view.sections.pending.is_none());
            assert!(view.test_probes.last_enqueued_edit.is_none());
            assert_eq!(view.state.as_ref().unwrap().project, before);
            assert_eq!(view.arrangement_focus.projection.enabled, focused);
            assert!(
                !focused || view.sections.selected == Some(ids[0]),
                "focus retains its section driver while range scope includes hidden tracks"
            );
        });
        assert!(
            !visual.update(|window, app| editor.read(app).section_has_focus(window)),
            "range owns keyboard actions while Q keeps its display driver"
        );
        visual.simulate_keystrokes("cmd-c");
        editor.read_with(cx, |view, _| {
            assert_eq!(view.test_probes.last_time_request, Some(("copy", None)))
        });
    }
}

#[gpui::test]
fn section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts(
    cx: &mut TestAppContext,
) {
    struct RestoreTheme(hex_preferences::ThemePreferences);
    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            theme::apply_preferences(self.0);
        }
    }
    let _restore = RestoreTheme(theme::preferences());
    let (handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().project.clone());
    for (width, palette) in [
        (880.0, hex_preferences::ThemePreferences::default()),
        (
            1280.0,
            hex_preferences::ThemePreferences {
                background: hex_preferences::RgbColor::new(225, 229, 238),
                text: hex_preferences::RgbColor::new(24, 27, 34),
                primary: hex_preferences::RgbColor::new(145, 25, 45),
                secondary: hex_preferences::RgbColor::new(16, 70, 145),
            },
        ),
    ] {
        theme::apply_preferences(palette);
        editor.update(cx, |view, cx| {
            view.viewport = Some(Viewport {
                start: 24_000,
                length: 240_000,
            });
            cx.notify();
        });
        visual.simulate_resize(gpui::size(px(width), px(600.0)));
        visual.simulate_mouse_move(point(px(4.0), px(4.0)), None, Modifiers::default());
        visual.run_until_parked();
        let body = bounds(&mut visual, 0);
        let header = visual
            .debug_bounds("arrangement-section-header-0")
            .expect("actual section header");
        assert!((f32::from(header.top() - body.top())).abs() <= 1.0);
        assert!(
            (f32::from(header.size.height) - super::super::clip_name::CLIP_NAME_HEIGHT).abs()
                <= 0.1,
            "header remains a fixed compact height"
        );
        assert!(header.right() <= body.right() + px(1.0));
        let (fill, _, _) = theme::section_colors(ElementColor::Blue, false, false);
        assert_eq!(
            painted_fill(&mut visual, 0),
            Some(rgb(fill).into()),
            "opaque tier fill"
        );
        let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
        let fills: Vec<_> = visual
            .rendered_filled_quads("arrangement-section-0")
            .into_iter()
            .filter(|(_, c)| *c == gpui::Background::from(rgb(fill)))
            .collect();
        assert!(
            fills.iter().any(
                |(b, _)| (b.bottom() / scale - f32::from(body.bottom())).abs() < 0.1
                    && b.right() / scale < f32::from(body.right()) - 1.0
            ),
            "cut leaves the bottom-right corner unfilled"
        );
        for (at, cursor) in [
            (header.center(), gpui::CursorStyle::OpenHand),
            (
                point(body.center().x, header.bottom() + px(1.0)),
                gpui::CursorStyle::Arrow,
            ),
            (
                point(body.right() - px(1.0), body.top() + body.size.height * 0.75),
                gpui::CursorStyle::ResizeRightBracket,
            ),
        ] {
            visual.simulate_mouse_move(at, None, Modifiers::default());
            visual.run_until_parked();
            assert_eq!(visual.rendered_cursor_style(), Some(cursor));
        }
        editor.read_with(cx, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project, before)
        });
    }
}

#[path = "ui_tests/creation_and_selection_tests.rs"]
mod creation_and_selection_tests;
#[path = "ui_tests/drag_and_copy_tests.rs"]
mod drag_and_copy_tests;
#[path = "ui_tests/edit_and_menu_tests.rs"]
mod edit_and_menu_tests;
#[path = "ui_tests/playback_and_loop_tests.rs"]
mod playback_and_loop_tests;
