//! Whole-clip placement must keep out-of-window material and released display custody.
use super::workflow_tests::{edit, open, pump};
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};

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
