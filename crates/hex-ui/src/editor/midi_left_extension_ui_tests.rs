//! Fresh canonical MIDI creation → real left handle → preview/worker/history.
use super::super::{
    EditorView, musical_time::SnapGrid, test_environment::Environment, viewport::Viewport,
};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, Command};
use hex_session::service::{Event, Request, Service};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn next(service: &Service) -> Event {
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
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn fixture(
    cx: &mut TestAppContext,
) -> (
    Arc<Environment>,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    hex_project::ClipId,
) {
    fixture_with_note(cx, true)
}
fn fixture_with_note(
    cx: &mut TestAppContext,
    with_note: bool,
) -> (
    Arc<Environment>,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    hex_project::ClipId,
) {
    cx.update(super::super::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "fresh left resize".into(),
        }))
        .unwrap();
    let event = next(&service);
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let event = next(&service);
    let clip = event.state.project.track(track).unwrap().midi_clips()[0].id();
    let event = if with_note {
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ONE,
                duration: BeatTime::ONE,
                pitch: 64,
                channel: 2,
                velocity: 97,
            }))
            .unwrap();
        next(&service)
    } else {
        event
    };
    let original = event.state.project.midi_clip(clip).unwrap().clone();
    assert_eq!(original.source_start(), BeatTime::ZERO);
    assert_eq!(original.source_length(), original.length());
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&environment),
                )
            });
            editor.update(cx, |v, cx| {
                v.receive(event);
                v.viewport = Some(Viewport {
                    start: 0,
                    length: 384_000,
                });
                v.snap_grid = SnapGrid::Off;
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (environment, handle, editor, clip)
}
#[gpui::test]
fn fresh_midi_left_extension_preserves_right_edge_notes_and_real_worker_history(
    cx: &mut TestAppContext,
) {
    let (environment, handle, editor, clip) = fixture(cx);
    let (original, before, compiled_events) = editor.read_with(cx, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        (
            project.midi_clip(clip).unwrap().clone(),
            project.snapshot(),
            hex_engine::midi_compile::midi_events(project).unwrap(),
        )
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(Arc::ptr_eq(&environment, &v.test_environment))
    });
    let selector = Box::leak(format!("midi-clip-trim-left-{}", clip.value()).into_boxed_str());
    let down = visual
        .debug_bounds(selector)
        .expect("fresh clip left handle")
        .center();
    let target = point(down.x - px(90.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x - px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        let drag = v.active_midi_trim_drag.as_ref().expect("actual left-handle capture");
        let preview = drag.preview.borrow().expect("canonical metadata candidate");
        eprintln!("fresh midi original={:?}..{:?} source={:?}, down={down:?} target={target:?}, preview={:?}..{:?}",
            original.start(), original.start().raw()+original.length().raw(), original.source_start(), preview.start(), preview.start().raw()+preview.length().raw());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(preview.start() < original.start(), "fresh MIDI left edge must expose earlier blank time");
        assert_eq!(preview.start().raw()+preview.length().raw(), original.start().raw()+original.length().raw());
        let project = &v.state.as_ref().unwrap().project;
        let unchanged = original.preview_edge_trim(hex_project::TrimEdge::Left, original.start()).unwrap();
        let shapes = super::super::tracks::midi_trim_note_shapes(project, &original, unchanged, 0, 384_000, 0);
        assert_eq!(shapes.len(), 1);
        assert_eq!(
            super::super::tracks::midi_trim_note_shapes(project, &original, preview, 0, 384_000, 0), shapes,
            "the actual paint projection keeps the note at its old musical position",
        );
        preview
    });
    let held = visual.debug_bounds("midi-trim-live-preview").unwrap();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("midi-trim-live-preview"), Some(held));
    editor.read_with(&visual, |v, _| {
        assert!(v.active_midi_trim_drag.is_none());
        assert!(v.pending_trim.is_some());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.pending_trim.is_none());
        assert!(v.error.is_none(), "{:?}", v.error);
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(
            hex_engine::midi_compile::midi_events(project).unwrap(),
            compiled_events,
            "real compiler note-on/off frame ordering remains identical"
        );
        let actual = project.midi_clip(clip).unwrap();
        assert_eq!(
            (actual.start(), actual.length()),
            (desired.start(), desired.length())
        );
        let note = &actual.notes()[0];
        assert_eq!(note.id(), original.notes()[0].id());
        assert_eq!(
            actual.start().raw() + note.start().raw() - actual.source_start().raw(),
            original.start().raw() + original.notes()[0].start().raw()
                - original.source_start().raw()
        );
        assert_eq!(
            (
                note.pitch(),
                note.channel(),
                note.velocity(),
                note.duration()
            ),
            (64, 2, 97, BeatTime::ONE)
        );
        project.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    let path = environment.root().join("fresh-left.hex");
    let mut operation = PendingFileOperation {
        environment: Arc::clone(&environment),
        acknowledged: false,
    };
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    operation.acknowledged = true;
    let reopened = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&reopened);
    operation.acknowledged = false;
    reopened.submit(Request::Open(path.clone())).unwrap();
    let loaded = next(&reopened);
    assert!(loaded.project_replaced);
    assert_eq!(loaded.state.project.snapshot(), after);
    operation.acknowledged = true;
    operation.acknowledged = false;
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after);
    });
    operation.acknowledged = true;
}

// A failed or timed-out real file request must retain its input root as evidence,
// rather than remove it while the existing Service may still be using the path.
struct PendingFileOperation {
    environment: Arc<Environment>,
    acknowledged: bool,
}
impl Drop for PendingFileOperation {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.environment.retain_for_unacknowledged_setup_failure();
        }
    }
}
fn capture_left(
    visual: &mut VisualTestContext,
    clip: hex_project::ClipId,
    delta: f32,
) -> gpui::Point<gpui::Pixels> {
    let selector = Box::leak(format!("midi-clip-trim-left-{}", clip.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x - px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let target = point(down.x + px(delta), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    target
}
#[gpui::test]
fn fresh_midi_left_extension_grid_modifier_release_keeps_the_same_canonical_candidate(
    cx: &mut TestAppContext,
) {
    let (_environment, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::Quarter;
        cx.notify();
    });
    visual.run_until_parked();
    let target = capture_left(&mut visual, clip, -90.0);
    let snapped = editor.read_with(&visual, |v, _| {
        v.active_midi_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow()
            .unwrap()
    });
    assert_eq!(snapped.start().raw() % BeatTime::ONE.raw(), 0);
    let free = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(free);
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        v.active_midi_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow()
            .unwrap()
    });
    assert_ne!(
        desired.start(),
        snapped.start(),
        "command modifier recomputes free ordinary MIDI trim edge"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, free);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
        assert_eq!((c.start(), c.length()), (desired.start(), desired.length()));
        assert_eq!(
            c.start().raw() + c.length().raw(),
            BeatTime::from_quarters(8).unwrap().raw()
        );
    });
}
#[gpui::test]
fn fresh_midi_left_extension_escape_and_actual_project_replacement_never_commit_prefix(
    cx: &mut TestAppContext,
) {
    for replacement in [false, true] {
        let (_environment, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let target = capture_left(&mut visual, clip, -90.0);
        editor.read_with(&visual, |v, _| {
            assert!(
                v.active_midi_trim_drag
                    .as_ref()
                    .unwrap()
                    .preview
                    .borrow()
                    .and_then(|preview| match preview {
                        super::MidiEdgePreview::Trim(trim) => Some(trim.source_prefix()),
                        super::MidiEdgePreview::Stretch(_) | super::MidiEdgePreview::Loop(_) =>
                            None,
                    })
                    .is_some_and(|prefix| prefix > BeatTime::ZERO)
            )
        });
        editor.update(&mut visual, |v, _| v.last_enqueued_edit = None);
        if replacement {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::NewWithAudioMode {
                    mode: hex_session::DocumentAudioMode::Resident,
                    reservation: None,
                }))
            });
            pump(&editor, &mut visual);
        } else {
            visual.simulate_keystrokes("escape");
        }
        let expected =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        if !replacement {
            assert_eq!(expected, before);
        }
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
            assert!(v.active_midi_trim_drag.is_none());
            assert!(v.pending_trim.is_none());
            assert!(
                v.last_enqueued_edit.is_none(),
                "stale/cancelled capture cannot author its prefix"
            );
        });
    }
}
#[gpui::test]
fn fresh_midi_left_extension_edge_pan_stops_interior_and_releases_exact_geometry(
    cx: &mut TestAppContext,
) {
    let (_environment, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(Viewport {
            start: 48_000,
            length: 96_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let (track, lane) = editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            v.viewport.unwrap(),
            "fixture must start inside canonical viewport bounds, not merely normalize on tick"
        );
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        (track, v.drag_lanes[&track])
    });
    let down = capture_left(&mut visual, clip, -12.0);
    let edge = point(px(lane.left + 1.0), down.y);
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    let now = Instant::now();
    editor.update(&mut visual, |v, cx| {
        v.tick_clip_gesture_autoscroll(now, cx);
        v.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.viewport.unwrap().start < 48_000));
    let interior = point(px(lane.left + lane.width * 0.35), down.y);
    visual.simulate_mouse_move(interior, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let (stopped, desired) = editor.read_with(&visual, |v, _| {
        (
            v.viewport,
            v.active_midi_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .unwrap(),
        )
    });
    editor.update(&mut visual, |v, cx| {
        v.tick_clip_gesture_autoscroll(now + Duration::from_millis(100), cx);
        v.tick_clip_gesture_autoscroll(now + Duration::from_millis(150), cx);
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.viewport, stopped, "interior stops edge pan")
    });
    visual.simulate_mouse_up(interior, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let c = &v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .midi_clips()[0];
        assert_eq!((c.start(), c.length()), (desired.start(), desired.length()));
        assert_eq!(
            c.start().raw() + c.length().raw(),
            BeatTime::from_quarters(8).unwrap().raw()
        );
    });
}

#[gpui::test]
fn fresh_midi_left_extension_empty_creation_reaches_zero_without_negative_source(
    cx: &mut TestAppContext,
) {
    let (_environment, handle, editor, clip) = fixture_with_note(cx, false);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = editor.read_with(&visual, |v, _| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        v.drag_lanes[&track]
    });
    let down = capture_left(&mut visual, clip, -12.0);
    let target = point(px(lane.left + 0.5), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let desired = v
            .active_midi_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow()
            .unwrap();
        assert_eq!(desired.start(), BeatTime::ZERO);
        assert_eq!(desired.source_start(), BeatTime::ZERO);
        assert_eq!(desired.length(), BeatTime::from_quarters(8).unwrap());
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
        assert_eq!(
            (c.start(), c.source_start()),
            (BeatTime::ZERO, BeatTime::ZERO)
        );
        assert_eq!(
            (c.length(), c.source_length()),
            (
                BeatTime::from_quarters(8).unwrap(),
                BeatTime::from_quarters(8).unwrap()
            )
        );
        assert!(c.notes().is_empty());
    });
}
