//! Real edge input, bounded note projection, worker terminal and history.
use super::super::{EditorView, musical_time::SnapGrid};
use super::midi_left_extension_ui_tests::{fixture, pump};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, ClipId, Command, TrimEdge};
use hex_session::service::Request;

fn capture(
    visual: &mut VisualTestContext,
    clip: ClipId,
    edge: TrimEdge,
    delta: f32,
    modifiers: Modifiers,
) -> gpui::Point<gpui::Pixels> {
    let name = match edge {
        TrimEdge::Left => "left",
        TrimEdge::Right => "right",
    };
    let selector = Box::leak(format!("midi-clip-trim-{name}-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
    visual.simulate_mouse_move(
        point(down.x + px(delta.signum() * 12.0), down.y),
        MouseButton::Left,
        modifiers,
    );
    visual.run_until_parked();
    let target = point(down.x + px(delta), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
    visual.run_until_parked();
    target
}
fn preview(
    editor: &Entity<EditorView>,
    visual: &VisualTestContext,
) -> hex_project::MidiClipStretch {
    editor.read_with(visual, |v, _| {
        let drag = v.active_midi_trim_drag.as_ref().unwrap();
        assert!(drag.stretch_modifier.get());
        let super::MidiEdgePreview::Stretch(preview) = drag.preview.borrow().unwrap() else {
            panic!("actual Shift gesture must use stretch projection")
        };
        preview
    })
}

#[gpui::test]
fn midi_shift_stretch_both_edges_projects_notes_through_release_ack_and_one_undo(
    cx: &mut TestAppContext,
) {
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        for delta in [-55.0, 55.0] {
            let (environment, handle, editor, clip) = fixture(cx);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let (before, original) = editor.read_with(&visual, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                (p.snapshot(), p.midi_clip(clip).unwrap().clone())
            });
            let shift = Modifiers {
                shift: true,
                ..Modifiers::default()
            };
            let target = capture(&mut visual, clip, edge, delta, shift);
            let desired = preview(&editor, &visual);
            assert_ne!(desired.length(), original.length());
            let held = visual.debug_bounds("midi-trim-live-preview").unwrap();
            let held_note = trim_notes(&mut visual);
            assert_eq!(held_note.len(), 1, "the stretched note is painted");
            let projected_duration = desired
                .scale(BeatTime::from_quarters(2).unwrap())
                .unwrap()
                .raw()
                - desired.scale(BeatTime::ONE).unwrap().raw();
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert_eq!(
                    v.active_midi_trim_drag
                        .as_ref()
                        .unwrap()
                        .clip
                        .source_identity(),
                    original.source_identity()
                );
                let shapes = super::super::tracks::midi_trim_note_shapes(
                    &v.state.as_ref().unwrap().project,
                    &original,
                    super::MidiEdgePreview::Stretch(desired),
                    0,
                    384_000,
                    0,
                );
                assert_eq!(shapes.len(), 1);
                let expected_frame_duration =
                    projected_duration as f64 / BeatTime::ONE.raw() as f64 * 24_000.0;
                assert!(
                    (f64::from(shapes[0].1) - expected_frame_duration / 384_000.0).abs() < 0.00001
                );
            });
            visual.simulate_mouse_up(target, MouseButton::Left, shift);
            // Completed repaints without consuming the worker terminal must
            // keep both the released extent and stretched note geometry.
            for _ in 0..3 {
                visual.run_until_parked();
                assert_eq!(visual.debug_bounds("midi-trim-live-preview"), Some(held));
                assert_eq!(trim_notes(&mut visual), held_note);
            }
            editor.read_with(&visual, |v, _| {
                assert!(v.active_midi_trim_drag.is_none());
                assert!(v.pending_trim.is_some());
                assert_eq!(v.worker_requests_pending, 1);
                assert!(matches!(v.last_enqueued_edit, Some(Command::StretchMidiClipEdge {clip:id,edge:which,..}) if id == clip && which == edge));
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
            pump(&editor, &mut visual);
            let actual = editor.read_with(&visual, |v, _| {
                assert!(v.error.is_none(), "{:?}", v.error);
                assert!(v.pending_trim.is_none());
                let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
                assert_eq!(
                    (c.start(), c.length(), c.source_start()),
                    (desired.start(), desired.length(), desired.source_start())
                );
                let n = &c.notes()[0];
                let old = &original.notes()[0];
                assert_eq!(
                    (n.id(), n.pitch(), n.channel(), n.velocity()),
                    (old.id(), old.pitch(), old.channel(), old.velocity())
                );
                assert_eq!(n.start(), desired.scale(old.start()).unwrap());
                assert_eq!(n.duration().raw(), projected_duration);
                let project = &v.state.as_ref().unwrap().project;
                let events = hex_engine::midi_compile::midi_events(project).unwrap();
                assert_eq!(events.len(), 2);
                let start = c.start().raw() + n.start().raw() - c.source_start().raw();
                let end = start + n.duration().raw();
                assert_eq!(
                    events[0].frame,
                    project
                        .tempo_map()
                        .beat_to_frame(BeatTime::from_raw(start))
                        .unwrap()
                );
                assert_eq!(
                    events[1].frame,
                    project
                        .tempo_map()
                        .beat_to_frame(BeatTime::from_raw(end))
                        .unwrap()
                );
                assert_eq!(
                    events[0].kind,
                    hex_engine::midi_compile::MidiEventKind::NoteOn
                );
                assert_eq!(
                    events[1].kind,
                    hex_engine::midi_compile::MidiEventKind::NoteOff
                );
                c.clone()
            });
            if edge == TrimEdge::Right && delta > 0.0 {
                let path = environment.root().join("stretched-midi.hex");
                let mut operation = PendingFileOperation {
                    environment: std::sync::Arc::clone(&environment),
                    acknowledged: false,
                };
                let expected =
                    editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::SaveNew(path.clone())))
                });
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
                assert!(std::fs::metadata(&path).unwrap().len() > 0);
                operation.acknowledged = true;
                let reopened = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
                next(&reopened);
                operation.acknowledged = false;
                reopened.submit(Request::Open(path)).unwrap();
                let loaded = next(&reopened);
                assert!(loaded.project_replaced);
                assert_eq!(loaded.state.project.snapshot(), expected);
                operation.acknowledged = true;
            }
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
            });
            visual.simulate_keystrokes("cmd-shift-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.state.as_ref().unwrap().project.midi_clip(clip),
                    Some(&actual)
                )
            });
        }
    }
}

#[gpui::test]
fn midi_shift_stretch_live_modifiers_and_browser_release_keep_the_last_note_projection(
    cx: &mut TestAppContext,
) {
    let (_environment, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::Quarter;
        cx.notify();
    });
    visual.run_until_parked();
    let target = capture(
        &mut visual,
        clip,
        TrimEdge::Right,
        57.0,
        Modifiers::default(),
    );
    editor.read_with(&visual, |v, _| {
        assert!(matches!(
            v.active_midi_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .unwrap(),
            super::MidiEdgePreview::Trim(_)
        ))
    });
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(shift);
    visual.run_until_parked();
    let snapped = preview(&editor, &visual);
    assert_eq!(snapped.length().raw() % BeatTime::ONE.raw(), 0);
    let free = Modifiers {
        shift: true,
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(free);
    visual.run_until_parked();
    let desired = preview(&editor, &visual);
    assert_ne!(desired.length(), snapped.length());
    // Shift up reverts to trim without a mouse move; Shift down returns to
    // stretching the original immutable source, never a cumulative stretch.
    visual.simulate_modifiers_change(Modifiers {
        platform: true,
        ..Modifiers::default()
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(matches!(
            v.active_midi_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .unwrap(),
            super::MidiEdgePreview::Trim(_)
        ))
    });
    visual.simulate_modifiers_change(free);
    visual.run_until_parked();
    assert_eq!(preview(&editor, &visual), desired);
    let held = visual.debug_bounds("midi-trim-live-preview").unwrap();
    let browser = visual
        .debug_bounds("browser-section-samples")
        .unwrap()
        .center();
    visual.simulate_mouse_move(browser, MouseButton::Left, free);
    visual.run_until_parked();
    assert_eq!(preview(&editor, &visual), desired);
    visual.simulate_mouse_up(browser, MouseButton::Left, free);
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("midi-trim-live-preview"), Some(held));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_clip(clip)
                .unwrap()
                .length(),
            desired.length()
        );
    });
    let _ = target;
}

#[gpui::test]
fn midi_shift_stretch_escape_focus_and_real_document_replacement_retire_without_edit(
    cx: &mut TestAppContext,
) {
    for terminal in 0..3 {
        let (_environment, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let target = capture(&mut visual, clip, TrimEdge::Right, 55.0, shift);
        preview(&editor, &visual);
        assert!(visual.debug_bounds("midi-trim-live-preview").is_some());
        editor.update(&mut visual, |v, _| v.last_enqueued_edit = None);
        match terminal {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window)),
            _ => {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::NewWithAudioMode {
                        mode: hex_session::DocumentAudioMode::Resident,
                        reservation: None
                    }))
                });
                pump(&editor, &mut visual);
            }
        }
        visual.run_until_parked();
        assert!(visual.debug_bounds("midi-trim-live-preview").is_none());
        let expected =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        if terminal < 2 {
            assert_eq!(expected, before);
        }
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.active_midi_trim_drag.is_none() && v.pending_trim.is_none());
            assert!(!matches!(
                v.last_enqueued_edit,
                Some(Command::StretchMidiClipEdge { .. })
            ));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
        });
    }
}

fn next(service: &hex_session::service::Service) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "next: hang guard expired"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
struct PendingFileOperation {
    environment: std::sync::Arc<super::super::test_environment::Environment>,
    acknowledged: bool,
}
impl Drop for PendingFileOperation {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.environment.retain_for_unacknowledged_setup_failure();
        }
    }
}

#[gpui::test]
fn midi_shift_stretch_pending_receipt_ignores_earlier_ack_and_retires_real_worker_refusal(
    cx: &mut TestAppContext,
) {
    let (_environment, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    capture(&mut visual, clip, TrimEdge::Right, 55.0, shift);
    let candidate = preview(&editor, &visual);
    editor.update(&mut visual, |v, cx| {
        // Controlled queue seam after a real capture: an earlier canonical
        // deletion makes this admitted resize fail in the actual worker.
        // Both events are consumed individually to verify exact FIFO custody.
        let drag = v.active_midi_trim_drag.as_ref().unwrap().clone();
        let state = v.state.as_ref().unwrap();
        let request = Request::Edit(Command::StretchMidiClipEdge {
            clip,
            edge: TrimEdge::Right,
            beat: BeatTime::from_raw(candidate.start().raw() + candidate.length().raw()),
        });
        v.pending_trim = Some(super::PendingTrim {
            generation: v.project_generation,
            revision: state.revision,
            track: drag.source_track,
            command: super::ResizeCommand::from_request(&request).unwrap(),
            remaining: None,
            scroll_viewport: None,
            display: super::PendingTrimDisplay::Midi {
                original: std::rc::Rc::clone(&drag.clip),
                prospective: super::MidiEdgePreview::Stretch(candidate),
            },
        });
        assert!(v.enqueue(Request::Edit(Command::RemoveMidiClip(clip))));
        assert!(v.pending_trim.as_ref().unwrap().remaining.is_none());
        assert!(v.enqueue(request));
        assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(2));
        v.retire_trim_gesture(false, cx);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-trim-live-preview").is_some());
    editor.update(&mut visual, |v, cx| {
        let first = next(&v.service);
        assert!(first.request_complete);
        v.receive(first);
        assert_eq!(v.pending_trim.as_ref().unwrap().remaining, Some(1));
        let second = event(&v.service);
        assert!(second.request_complete);
        assert!(
            second
                .error
                .as_deref()
                .is_some_and(|error| error.contains("no longer exists")),
            "{:?}",
            second.error
        );
        v.receive(second);
        assert!(v.pending_trim.is_none());
        assert!(v.state.as_ref().unwrap().project.midi_clip(clip).is_none());
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("midi-trim-live-preview").is_none());
}
fn event(service: &hex_session::service::Service) -> hex_session::service::Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "event: hang guard expired"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Painted trim-preview note rectangles, in logical pixels.
fn trim_notes(visual: &mut gpui::VisualTestContext) -> Vec<gpui::Bounds<gpui::Pixels>> {
    crate::editor::midi_thumbnail::rendered_note_layer(visual, "midi-trim-preview-notes")
        .into_iter()
        .map(|(bounds, _)| bounds)
        .collect()
}
