use super::clip_drag_ui_test::{audio_project, editor_window, midi_project};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::Command;
use std::sync::Arc;

#[gpui::test]
fn clicked_audio_cursor_is_visible_and_command_e_uses_it_offline(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, _, clip) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("lane");
    let click = point(
        lane.origin.x + lane.size.width * 0.125,
        lane.origin.y + px(22.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let frame = editor.read_with(cx, |view, _| {
        let cursor = view.edit_cursor.expect("click places cursor");
        assert_eq!(cursor.track, source);
        assert!(cursor.frame > 0 && cursor.frame < 96_000);
        assert_eq!(
            view.cursor_position(),
            0,
            "edit click does not seek the audio playhead"
        );
        cursor.frame
    });
    let line = visual
        .debug_bounds("edit-cursor-line")
        .expect("rendered line");
    let layer = visual
        .debug_bounds("edit-cursor-layer")
        .expect("cursor extent");
    assert_eq!(line.size.height, layer.size.height);
    assert!(f32::from(line.size.width) <= 1.0);
    assert!(
        (f32::from(line.origin.x - layer.origin.x) / f32::from(layer.size.width)
            - frame as f32 / 384_000.0)
            .abs()
            < 0.002
    );
    editor.update(cx, |view, cx| {
        view.viewport = Some(super::viewport::Viewport {
            start: 10_000,
            length: 128_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let moved = visual
        .debug_bounds("edit-cursor-line")
        .expect("line follows viewport");
    let extent = visual
        .debug_bounds("edit-cursor-layer")
        .expect("cursor extent");
    let expected = (frame - 10_000) as f32 / 128_000.0;
    assert!(
        (f32::from(moved.origin.x - extent.origin.x) / f32::from(extent.size.width) - expected)
            .abs()
            < 0.002
    );
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitClip { clip: id, frame: position }) if id == clip && position == frame));
    });
}

#[gpui::test]
fn command_click_is_free_and_command_e_routes_midi_split(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, source, _, clip) = midi_project();
    let (_, preview) =
        hex_engine::compile::project_with_midi_preview(&project, &Default::default())
            .expect("midi preview");
    let (handle, editor) = editor_window(cx, project, Arc::new(preview));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .expect("lane");
    let click = point(
        lane.origin.x + lane.size.width * 0.17,
        lane.origin.y + px(22.0),
    );
    let modifiers = Modifiers {
        platform: true,
        ..Default::default()
    };
    visual.simulate_mouse_down(click, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(click, MouseButton::Left, modifiers);
    visual.run_until_parked();
    let beat = editor.read_with(cx, |view, _| {
        let frame = view.edit_cursor.expect("free click cursor").frame;
        let geometry = view
            .drag_lanes
            .get(&source)
            .expect("actual timeline geometry");
        let expected = ((f32::from(click.x) - geometry.left) as f64 / f64::from(geometry.width)
            * 384_000.0)
            .round() as u64;
        assert_eq!(frame, expected, "free placement keeps pointer precision");
        assert_ne!(
            frame % 24_000,
            0,
            "command click must remain off the quarter grid"
        );
        view.state
            .as_ref()
            .expect("state")
            .project
            .tempo_map()
            .frame_to_beat(frame)
            .expect("beat")
    });
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| {
        assert!(matches!(view.last_enqueued_edit, Some(Command::SplitMidiClip { clip: id, beat: position }) if id == clip && position == beat));
    });
}

#[gpui::test]
fn empty_position_and_missing_cursor_do_not_dispatch_a_split(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    let (project, _, target, _) = audio_project();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| assert!(view.last_enqueued_edit.is_none()));
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .expect("empty target lane");
    let click = point(
        lane.origin.x + lane.size.width / 2.0,
        lane.origin.y + px(22.0),
    );
    visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-e");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.edit_cursor.expect("cursor on empty track").track,
            target
        );
        assert!(view.last_enqueued_edit.is_none());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("inside a clip"))
        );
    });
}
