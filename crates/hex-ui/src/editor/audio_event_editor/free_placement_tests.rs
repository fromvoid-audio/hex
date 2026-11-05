//! Whole-clip placement must keep out-of-window material and released display custody.
use super::workflow_tests::{edit, open, pump};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};

#[gpui::test]
fn dragging_blank_space_left_of_the_root_cannot_move_its_first_event(cx: &mut TestAppContext) {
    let (_files, mut visual, editor, _) = open(cx);
    let before = editor.update(&mut visual, |v, cx| {
        for _ in 0..3 {
            v.event_pan(false);
        }
        cx.notify();
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.run_until_parked();
    let lane = visual.debug_bounds("audio-events-waveform").unwrap();
    assert!(visual.debug_bounds("audio-event-1").is_none());
    let down = point(lane.center().x, lane.top() + px(33.0));
    let moved = point(down.x + lane.size.width * 0.25, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(matches!(
            v.audio_event_editor.gesture.as_ref().unwrap().kind,
            Kind::Select
        ));
    });
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn frame_event_drag_beyond_clip_end_keeps_waveform_release_history_and_saved_material(
    cx: &mut TestAppContext,
) {
    let (files, mut visual, editor, clip) = open(cx);
    let duration = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .duration()
    });
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: duration / 4 },
    );
    let before = editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.grid = SnapGrid::Off;
        v.audio_event_editor.viewport = Some(Viewport {
            start: 0,
            length: duration * 3,
        });
        cx.notify();
        v.state.as_ref().unwrap().project.clone()
    });
    visual.run_until_parked();
    let lane = visual.debug_bounds("audio-events-waveform").unwrap();
    let original = visual.debug_bounds("audio-event-1").unwrap();
    let down = point(original.center().x, original.top() + px(8.0));
    let moved = point(down.x + lane.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-event-1").unwrap();
    assert!(
        (f32::from(held.left() - original.left() - lane.size.width * 0.5)).abs() < 2.0,
        "event must move past the unchanged Arrangement window: {original:?} -> {held:?}"
    );
    assert!(
        visual.debug_bounds("audio-event-waveform-1").is_some(),
        "held source peaks follow the event"
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(std::time::Instant::now() < deadline, "placement terminal");
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("audio-event-1").unwrap(),
            held,
            "release must retain the frozen whole-root geometry through completed repaints"
        );
        assert!(visual.debug_bounds("audio-event-waveform-1").is_some());
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_none());
        let project = &v.state.as_ref().unwrap().project;
        let c = project.clip(clip).unwrap();
        assert_eq!(
            (c.start(), c.duration()),
            (before.clip(clip).unwrap().start(), duration)
        );
        let events = c.audio_events();
        let event = events.events.iter().find(|e| e.id == 1).unwrap();
        assert!(event.start > duration);
        assert_eq!(event.source_start, 0);
        assert_eq!(event.length(), duration / 4);
        project.clone()
    });
    assert_eq!(visual.debug_bounds("audio-event-1").unwrap(), held);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, after)
    });
    let path = files.root().join("free-event.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        assert_eq!(v.state.as_ref().unwrap().project, after);
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        v.sync_audio_navigation();
        for _ in 0..32 {
            v.event_pan(true);
        }
        let viewport = v.event_viewport(&v.event_context().unwrap().1);
        assert!(
            viewport.start > duration * 4,
            "camera should not stop at four clip lengths"
        );
        assert!(viewport.end().unwrap() <= hex_project::MAX_AUDIO_DOMAIN_FRAMES);
        assert_eq!(v.state.as_ref().unwrap().project, after);
        cx.notify();
    });
}

#[gpui::test]
fn free_event_extreme_pointer_reversal_and_escape_leave_canonical_material_untouched(
    cx: &mut TestAppContext,
) {
    let (_files, mut visual, editor, clip) = open(cx);
    let duration = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .duration()
    });
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: duration / 4 },
    );
    let before = editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.grid = SnapGrid::Off;
        v.audio_event_editor.viewport = Some(Viewport {
            start: 0,
            length: duration * 3,
        });
        cx.notify();
        v.state.as_ref().unwrap().project.clone()
    });
    visual.run_until_parked();
    let bounds = visual.debug_bounds("audio-event-2").unwrap();
    let lane = visual.debug_bounds("audio-events-waveform").unwrap();
    let down = point(bounds.center().x, bounds.top() + px(8.0));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(px(f32::MAX), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.read_with(&visual, |v, _| {
        let query = v
            .audio_event_editor
            .gesture
            .as_ref()
            .unwrap()
            .placement
            .as_ref()
            .unwrap();
        assert_eq!(
            query.clip().audio_event_root().domain.clock.length(),
            hex_project::MAX_AUDIO_DOMAIN_FRAMES
        );
        assert_eq!(v.state.as_ref().unwrap().project, before);
        assert!(v.audio_event_editor.pending.is_none());
    });
    visual.simulate_mouse_move(
        point(down.x + lane.size.width * 0.5, down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let moved = visual.debug_bounds("audio-event-2").unwrap();
    assert!(
        (f32::from(moved.left() - bounds.left() - lane.size.width * 0.5)).abs() < 2.0,
        "reversal must derive from the original pointer and event"
    );
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before);
        assert!(v.audio_event_editor.gesture.is_none());
        assert!(v.audio_event_editor.pending.is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
    assert_eq!(visual.debug_bounds("audio-event-2").unwrap(), bounds);
}

#[gpui::test]
fn cropped_frame_source_keeps_camera_and_clip_window_when_moved_beyond_its_right_edge(
    cx: &mut TestAppContext,
) {
    use hex_project::{Command, TrimEdge};
    let (_files, mut visual, editor, clip) = open(cx);
    let duration = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .duration()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: duration / 4
        })))
    });
    pump(&editor, &mut visual);
    let before = editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.grid = SnapGrid::Off;
        v.audio_event_editor.viewport = Some(Viewport {
            start: 0,
            length: duration * 3,
        });
        cx.notify();
        v.state.as_ref().unwrap().project.clone()
    });
    visual.run_until_parked();
    let lane = visual.debug_bounds("audio-events-waveform").unwrap();
    let id = editor.read_with(&visual, |v, _| {
        let c = v.state.as_ref().unwrap().project.clip(clip).unwrap();
        c.editable_audio_events()
            .unwrap()
            .events
            .iter()
            .find(|e| e.start == c.audio_event_root().view_start)
            .unwrap()
            .id
    });
    let selector = Box::leak(format!("audio-event-{id}").into_boxed_str());
    let bounds = visual.debug_bounds(selector).unwrap();
    let down = point(bounds.center().x, bounds.top() + px(8.0));
    let moved = point(down.x + lane.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds(selector).unwrap();
    assert!((f32::from(held.left() - bounds.left() - lane.size.width * 0.5)).abs() < 2.0);
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(visual.debug_bounds(selector).unwrap(), held);
    editor.read_with(&visual, |v, _| {
        let c = v.state.as_ref().unwrap().project.clip(clip).unwrap();
        let old = before.clip(clip).unwrap();
        assert_eq!((c.start(), c.duration()), (old.start(), old.duration()));
        assert!(
            c.audio_events()
                .events
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .start
                > c.audio_event_root().view_end
        );
    });
}

#[gpui::test]
fn frame_event_drag_before_clip_zero_preserves_camera_waveform_history_and_saved_material(
    cx: &mut TestAppContext,
) {
    let (files, mut visual, editor, clip) = open(cx);
    let duration = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .clip(clip)
            .unwrap()
            .duration()
    });
    edit(
        &mut visual,
        &editor,
        clip,
        AudioEventEdit::Split { at: duration / 4 },
    );
    let before = editor.update(&mut visual, |v, cx| {
        v.audio_event_editor.grid = SnapGrid::Off;
        v.audio_event_editor.viewport = Some(Viewport {
            start: 0,
            length: duration * 3,
        });
        v.audio_event_editor.cursor = duration / 2;
        v.event_pan(false);
        assert!(v.event_frame_window(&v.event_context().unwrap().1).start < 0);
        cx.notify();
        v.state.as_ref().unwrap().project.clone()
    });
    visual.run_until_parked();
    let lane = visual.debug_bounds("audio-events-waveform").unwrap();
    let original = visual.debug_bounds("audio-event-1").unwrap();
    let down = point(original.center().x, original.top() + px(8.0));
    let moved = point(down.x - lane.size.width * 0.5, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let cursor_before = visual.debug_bounds("audio-event-cursor").unwrap();
    let marker_before = visual.debug_bounds("audio-play-start-marker");
    let raw_cursor = editor.read_with(&visual, |v, _| v.audio_event_editor.cursor);
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds("audio-event-1").unwrap();
    assert!(
        (f32::from(held.left() - original.left() + lane.size.width * 0.5)).abs() < 2.0,
        "event must move past the unchanged Arrangement window: {original:?} -> {held:?}"
    );
    assert_eq!(
        visual.debug_bounds("audio-event-cursor").unwrap(),
        cursor_before
    );
    assert_eq!(
        visual.debug_bounds("audio-play-start-marker"),
        marker_before
    );
    assert!(
        visual.debug_bounds("audio-event-waveform-1").is_some(),
        "held source peaks follow the event"
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(std::time::Instant::now() < deadline, "placement terminal");
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none(), "{:?}", terminal.error);
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("audio-event-1").unwrap(),
            held,
            "release must retain the frozen whole-root geometry through completed repaints"
        );
        assert!(visual.debug_bounds("audio-event-waveform-1").is_some());
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.audio_event_editor.pending.is_none());
        let project = &v.state.as_ref().unwrap().project;
        let c = project.clip(clip).unwrap();
        assert_eq!(
            (c.start(), c.duration()),
            (before.clip(clip).unwrap().start(), duration)
        );
        let events = c.audio_events();
        let event = events.events.iter().find(|e| e.id == 1).unwrap();
        assert_eq!(event.start, 0);
        let lead = c.audio_event_root().view_start;
        assert!(lead > duration);
        assert_eq!(v.audio_event_editor.cursor, raw_cursor + lead);
        assert_eq!(
            c.audio_events()
                .events
                .iter()
                .find(|e| e.id == 2)
                .unwrap()
                .start,
            duration / 4 + lead
        );
        assert_eq!(event.source_start, 0);
        assert_eq!(event.length(), duration / 4);
        project.clone()
    });
    assert_eq!(visual.debug_bounds("audio-event-1").unwrap(), held);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, before)
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, after)
    });
    let path = files.root().join("negative-free-event.hex");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        assert_eq!(v.state.as_ref().unwrap().project, after);
        v.selected_clip = Some(clip);
        v.clip_view_open = true;
        v.device_panel_active = false;
        v.sync_audio_navigation();
        v.audio_fit_all();
        let root = v.event_context().unwrap().1;
        let window = v.event_root_window(&root).unwrap();
        assert!(window.contains(0));
        assert!(v.event_frame_window(&root).start < 0);
        for _ in 0..32 {
            v.event_pan(false);
        }
        assert!(v.event_frame_window(&root).start < -i128::from(duration * 4));
        let prior = v.event_frame_window(&root);
        v.audio_event_editor.range = Some((0, duration / 4));
        assert!(v.event_fit_selection(false));
        assert!(v.event_fit_selection(true));
        assert_eq!(v.event_frame_window(&root), prior);
        assert_eq!(v.state.as_ref().unwrap().project, after);
        cx.notify();
    });
}
