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
            assert!(!view.clip_paint_held());
            view.last_enqueued_edit.clone().expect("release edit")
        });
        assert!(
            matches!(command, Command::PlaceMidiClipWithOverwrite { track: id, start, length }
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
        assert!(visual.debug_bounds("clip-paint-preview").is_some());
    }
}

#[gpui::test]
fn clip_paint_cancels_and_rejects_explicit_handles_and_stale_state(cx: &mut TestAppContext) {
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
            (source.left + 1.0, source.top + 7.0),
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
        assert!(matches!(view.last_enqueued_edit, Some(Command::PlaceMidiClipWithOverwrite { length, .. }) if length == BeatTime::ONE));
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
            assert!(matches!(view.last_enqueued_edit, Some(Command::PlaceMidiClipWithOverwrite { start: actual, length, .. }) if actual == start.min(end) && length.raw() == end.raw().abs_diff(start.raw())));
        });
    }
}

#[gpui::test]
fn clip_headers_grab_and_double_clicking_audio_or_midi_selects(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let (project, track, _, clip) = if midi {
            super::clip_drag_ui_test::midi_project()
        } else {
            audio_project()
        };
        let original = project.clone();
        let preview = if midi {
            hex_engine::compile::project_with_midi_preview(&project, &Default::default())
                .expect("midi preview")
                .1
        } else {
            Default::default()
        };
        let (handle, editor) = editor_window(cx, project, Arc::new(preview));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
            ))
            .expect("clip header");
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .expect("lane");
        assert!((f32::from(header.size.height) - f32::from(lane.size.height) / 3.0).abs() < 1.0);
        let grab = header.center();
        visual.simulate_mouse_move(grab, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::OpenHand)
        );
        visual.simulate_mouse_down(grab, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(view.pointer_down.is_some());
            assert!(view.time_selection_gesture.is_none());
        });
        visual.simulate_mouse_up(grab, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| assert_eq!(view.selected_clip(), Some(clip)));
        visual.run_until_parked();
        let body = point(grab.x, lane.origin.y + lane.size.height * 0.75);
        let end = point(body.x + lane.size.width * 0.04, body.y);
        visual.simulate_mouse_move(body, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::Arrow)
        );
        visual.simulate_mouse_down(body, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| assert!(view.time_selection_gesture.is_some()));
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(view.normalized_time_selection().is_some());
            assert!(view.active_clip_drag.is_none());
            assert!(view.selected_clip().is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
        visual.run_until_parked();
        visual.simulate_event(MouseDownEvent {
            position: body,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-paint-preview").is_none());
        editor.read_with(cx, |view, _| {
            assert_eq!(view.selected_clip(), Some(clip));
            assert!(view.clip_paint_gesture.is_none());
            assert!(view.active_clip_drag.is_none());
            assert!(view.last_enqueued_edit.is_none());
            assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
        });
    }
}

#[gpui::test]
fn clip_review_header_selects_while_pending_and_when_leaving_rename(cx: &mut TestAppContext) {
    cx.update(super::bind_editor_keys);
    for renaming in [false, true] {
        let (project, source, target, clip) = audio_project();
        let mut model = Editor::new(project, 8).unwrap();
        let asset = model.project().clip(clip).unwrap().asset();
        let Created::Clip(other) = model
            .execute(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 96_000,
                source_offset: 0,
                source_length: 96_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let (handle, editor) = editor_window(cx, model.project().clone(), Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.set_selected_clip(source, clip, window, cx);
                if renaming {
                    view.begin_clip_name(window, cx);
                } else {
                    view.worker_requests_pending = 1;
                }
                cx.notify();
            })
        });
        if renaming {
            visual.simulate_input("new name");
        }
        visual.run_until_parked();
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", other.value()).into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.selected_clip(), Some(other), "one click selects even without a drag surface");
            assert!(view.active_clip_drag.is_none());
            if renaming {
                assert!(view.clip_name_edit.is_none());
                assert!(matches!(view.last_enqueued_edit, Some(Command::RenameClip { clip: id, .. }) if id == clip));
            }
        });
    }
}

#[gpui::test]
fn clip_review_visible_body_wins_over_occluded_edges(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let (project, track, _, clip) = if midi {
            super::clip_drag_ui_test::midi_project()
        } else {
            audio_project()
        };
        let mut model = Editor::new(project, 8).unwrap();
        let (edge, visible) = if midi {
            let Created::Clip(visible) = model
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::from_quarters(2).unwrap(),
                    length: BeatTime::from_quarters(4).unwrap(),
                })
                .unwrap()
            else {
                panic!("midi clip")
            };
            (120_000, visible)
        } else {
            let asset = model.project().clip(clip).unwrap().asset();
            let Created::Clip(visible) = model
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start: 48_000,
                    duration: 96_000,
                    source_offset: 0,
                    source_length: 96_000,
                    looping: false,
                })
                .unwrap()
            else {
                panic!("audio clip")
            };
            (96_000, visible)
        };
        let preview = if midi {
            hex_engine::compile::project_with_midi_preview(model.project(), &Default::default())
                .unwrap()
                .1
        } else {
            Default::default()
        };
        let (handle, editor) = editor_window(cx, model.project().clone(), Arc::new(preview));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .unwrap();
        // One pixel before the obscured earlier clip's right edge, inside the top body.
        let body = point(
            lane.origin.x + lane.size.width * (edge as f32 / 384_000.0) - px(1.0),
            lane.origin.y + lane.size.height * 0.75,
        );
        visual.simulate_mouse_down(body, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert!(
                view.time_selection_gesture.is_some(),
                "visible body accepts selection across hidden trim edge"
            )
        });
        visual.simulate_mouse_up(body, MouseButton::Left, Modifiers::default());
        visual.simulate_event(MouseDownEvent {
            position: body,
            button: MouseButton::Left,
            click_count: 2,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        visual.simulate_mouse_up(body, MouseButton::Left, Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert_eq!(
                view.selected_clip(),
                Some(visible),
                "the topmost painted clip is selected"
            );
            assert!(view.clip_paint_gesture.is_none());
            assert!(view.last_enqueued_edit.is_none());
        });
    }
}
