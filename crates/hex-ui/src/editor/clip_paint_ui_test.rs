use super::clip_drag_ui_test::{audio_project, editor_window};
use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, Command, Created, Editor};
use std::sync::Arc;

#[gpui::test]
fn clip_paint_double_click_hold_previews_then_commits_once(cx: &mut TestAppContext) {
    for left in [false, true] {
        let (project, _, track, _) = audio_project();
        let mut canonical = Editor::new(project.clone(), 8).expect("editor");
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .expect("lane");
        let start = point(
            lane.origin.x + lane.size.width * if left { 0.5 } else { 0.25 },
            lane.origin.y + px(22.0),
        );
        let end = point(
            lane.origin.x + lane.size.width * if left { 0.25 } else { 0.5 },
            start.y,
        );
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
        visual.simulate_event(MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        // Cross the original anchor before settling on the final direction.
        let opposite = point(
            lane.origin.x + lane.size.width * if left { 0.75 } else { 0.125 },
            start.y,
        );
        visual.simulate_mouse_move(opposite, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = visual.debug_bounds("clip-paint-preview").expect("preview");
        assert!(
            (f32::from(preview.origin.x - lane.origin.x) - f32::from(lane.size.width) * 0.25).abs()
                < 1.0
        );
        assert!((f32::from(preview.size.width) - f32::from(lane.size.width) * 0.25).abs() < 1.0);
        editor.read_with(cx, |view, _| {
            assert!(view.clip_paint_gesture.is_some());
            assert!(view.time_selection_gesture.is_none());
            assert!(
                view.last_enqueued_edit.is_none(),
                "preview must not edit history"
            );
        });
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let command = editor.read_with(cx, |view, _| {
            assert!(view.clip_paint_gesture.is_none());
            view.last_enqueued_edit.clone().expect("release edit")
        });
        assert!(
            matches!(command, Command::PlaceMidiClip { track: id, start, length }
            if id == track && start == BeatTime::from_quarters(4).unwrap()
                && length == BeatTime::from_quarters(4).unwrap())
        );
        let Created::Clip(clip) = canonical.execute(command).expect("apply") else {
            panic!("clip");
        };
        assert!(canonical.project().midi_clip(clip).is_some());
        assert!(canonical.undo());
        assert!(canonical.project().midi_clip(clip).is_none());
        assert!(canonical.redo());
        assert!(canonical.project().midi_clip(clip).is_some());
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-paint-preview").is_none());
    }
}

#[gpui::test]
fn clip_paint_cancels_and_rejects_existing_content_and_stale_state(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let (start, end, occupied) = editor.read_with(cx, |view, _| {
        let lane = view.drag_lanes[&target];
        let source = view.drag_lanes[&source];
        (
            (lane.left + lane.width * 0.25, lane.top + 22.0),
            (lane.left + lane.width * 0.5, lane.top + 22.0),
            (source.left + source.width * 0.125, source.top + 22.0),
        )
    });
    editor.update(cx, |view, _| {
        assert!(!view.begin_clip_paint(occupied.0, occupied.1, false));
        assert!(view.begin_clip_paint(start.0, start.1, false));
    });
    visual.simulate_keystrokes("escape");
    editor.update(cx, |view, _| {
        assert!(view.clip_paint_gesture.is_none());
        assert!(!view.finish_clip_paint(end.0, end.1, false));
        assert!(view.last_enqueued_edit.is_none());
        assert!(view.begin_clip_paint(start.0, start.1, false));
        view.project_generation += 1;
        assert!(view.finish_clip_paint(end.0, end.1, false));
        assert!(view.last_enqueued_edit.is_none());
        assert!(view.begin_clip_paint(start.0, start.1, false));
        assert!(view.finish_clip_paint(end.0, -10.0, false));
        assert!(view.last_enqueued_edit.is_none());
    });
}

#[gpui::test]
fn clip_paint_minimum_length(cx: &mut TestAppContext) {
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(cx, |view, _| {
        view.snap_grid = super::musical_time::SnapGrid::Quarter;
        let lane = view.drag_lanes[&target];
        let x = lane.left + lane.width * 0.25;
        let y = lane.top + 22.0;
        assert!(view.begin_clip_paint(x, y, false));
        assert!(view.finish_clip_paint(x, y, false));
        assert!(matches!(view.last_enqueued_edit, Some(Command::PlaceMidiClip { length, .. }) if length == BeatTime::ONE));
    });
}

#[gpui::test]
fn clip_paint_free_drag_uses_pointer_beats(cx: &mut TestAppContext) {
    for left in [false, true] {
        let (project, _, target, _) = audio_project();
        let (handle, editor) = editor_window(cx, project, Arc::default());
        let visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(cx, |view, _| {
            view.snap_grid = super::musical_time::SnapGrid::Off;
            let lane = view.drag_lanes[&target];
            let x = lane.left + lane.width * if left { 0.437 } else { 0.173 };
            let end_x = lane.left + lane.width * if left { 0.173 } else { 0.437 };
            let y = lane.top + 22.0;
            let state = view.state.as_ref().unwrap();
            let viewport = view.visible_viewport(state.timeline_frames);
            let start = state.project.tempo_map().frame_to_beat(super::seek::pointer_to_frame(x, lane.left, lane.width, viewport).unwrap()).unwrap();
            let end = state.project.tempo_map().frame_to_beat(super::seek::pointer_to_frame(end_x, lane.left, lane.width, viewport).unwrap()).unwrap();
            assert!(view.begin_clip_paint(x, y, false));
            assert!(view.finish_clip_paint(end_x, y, false));
            assert!(matches!(view.last_enqueued_edit, Some(Command::PlaceMidiClip { start: actual, length, .. }) if actual == start.min(end) && length.raw() == end.raw().abs_diff(start.raw())));
        });
    }
}
