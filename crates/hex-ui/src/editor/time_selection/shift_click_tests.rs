//! Rendered pointer selection must retain the cursor anchor across Shift-clicks.
use crate::editor::{
    clip_drag_ui_test::{audio_project, editor_window},
    musical_time::SnapGrid,
};
use gpui::{Modifiers, TestAppContext, VisualTestContext, point};
use std::sync::Arc;

#[gpui::test]
fn shift_click_extends_from_cursor_forward_backward_and_plain_click_resets(
    cx: &mut TestAppContext,
) {
    for main in [false, true] {
        for off in [false, true] {
            let (project, source, target, _) = audio_project();
            let before = project.snapshot();
            let (handle, editor) = editor_window(cx, project, Arc::default());
            let mut visual = VisualTestContext::from_window(handle, cx);
            editor.update(&mut visual, |view, cx| {
                view.snap_grid = if off {
                    SnapGrid::Off
                } else {
                    SnapGrid::Adaptive
                };
                cx.notify();
            });
            visual.run_until_parked();
            let lane = visual
                .debug_bounds(if main {
                    "main-track-lane"
                } else {
                    Box::leak(format!("arrangement-lane-{}", target.value()).into_boxed_str())
                })
                .unwrap();
            let at = |fraction| {
                point(
                    lane.origin.x + lane.size.width * fraction,
                    lane.origin.y + lane.size.height * 0.75,
                )
            };
            visual.simulate_click(at(0.5), Modifiers::default());
            visual.run_until_parked();
            let anchor = editor.read_with(&visual, |view, _| view.edit_cursor.unwrap().frame);
            for fraction in [0.8, 0.2, 0.65] {
                let expected = editor.read_with(&visual, |view, _| {
                    let state = view.state.as_ref().unwrap();
                    let viewport = view.visible_viewport(state.timeline_frames);
                    let measured = if main {
                        view.main_track_lane_bounds.unwrap()
                    } else {
                        view.drag_lanes[&target]
                    };
                    let frame = crate::editor::seek::pointer_to_frame(
                        f32::from(at(fraction).x),
                        measured.left,
                        measured.width,
                        viewport,
                    )
                    .unwrap();
                    view.snap_grid
                        .snap_frame(&state.project, frame, viewport, measured.width)
                        .unwrap()
                });
                visual.simulate_click(
                    at(fraction),
                    Modifiers {
                        shift: true,
                        ..Modifiers::default()
                    },
                );
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    let range = view
                        .normalized_time_selection()
                        .expect("Shift-click extends a range");
                    assert_eq!(
                        (range.start(), range.end()),
                        (anchor.min(expected), anchor.max(expected))
                    );
                    let tracks = if main {
                        vec![source, target]
                    } else {
                        vec![target]
                    };
                    assert_eq!(range.tracks(), tracks);
                    assert_eq!(view.edit_cursor.unwrap().frame, anchor);
                    assert_eq!(view.time_selection_project_scope, main);
                    assert!(
                        view.time_selection_gesture.is_none()
                            && view.time_range_move_gesture.is_none()
                    );
                    assert!(
                        view.last_enqueued_edit.is_none() && view.last_transport_command.is_none()
                    );
                    assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
                });
                let selector = if main {
                    "main-project-time-selection"
                } else {
                    Box::leak(format!("edit-time-selection-{}", target.value()).into_boxed_str())
                };
                assert!(visual.debug_bounds(selector).is_some());
            }
            visual.simulate_click(at(0.4), Modifiers::default());
            visual.run_until_parked();
            let replacement = editor.read_with(&visual, |view, _| {
                assert!(view.normalized_time_selection().is_none());
                view.edit_cursor.unwrap().frame
            });
            assert_ne!(replacement, anchor);
            visual.simulate_click(
                at(0.7),
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            );
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.time_selection.unwrap().anchor_frame, replacement)
            });
            editor.update(&mut visual, |view, _| view.project_generation += 1);
            visual.simulate_click(
                at(0.9),
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            );
            visual.run_until_parked();
            editor.read_with(&visual, |view, _| {
                assert!(
                    view.normalized_time_selection().is_none(),
                    "old-document cursor cannot anchor a new range"
                );
                assert_eq!(
                    view.edit_cursor.unwrap().generation,
                    view.project_generation
                );
            });
        }
    }
}

#[gpui::test]
fn shift_click_without_cursor_places_one_and_clip_headers_keep_selection_precedence(
    cx: &mut TestAppContext,
) {
    let (project, source, target, clip) = audio_project();
    let before = project.snapshot();
    let (handle, editor) = editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let target_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", target.value()).into_boxed_str(),
        ))
        .unwrap();
    let source_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .unwrap();
    let at = |lane: gpui::Bounds<gpui::Pixels>, fraction| {
        point(
            lane.origin.x + lane.size.width * fraction,
            lane.origin.y + lane.size.height * 0.75,
        )
    };
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    assert!(editor.read_with(&visual, |view, _| view.edit_cursor.is_none()));
    visual.simulate_click(at(target_lane, 0.05), shift);
    visual.run_until_parked();
    let anchor = editor.read_with(&visual, |view, _| {
        assert!(view.normalized_time_selection().is_none());
        view.edit_cursor.unwrap().frame
    });
    visual.simulate_click(at(source_lane, 0.8), shift);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let range = view.normalized_time_selection().unwrap();
        assert_eq!(range.tracks(), [source, target]);
        assert_eq!(range.start(), anchor);
    });
    // Native repeated Shift-click may carry a double-click count. It extends the
    // range without entering double-click clip creation.
    let end = at(source_lane, 0.7);
    visual.simulate_event(gpui::MouseDownEvent {
        position: end,
        button: gpui::MouseButton::Left,
        click_count: 2,
        modifiers: shift,
        first_mouse: false,
    });
    visual.simulate_mouse_up(end, gpui::MouseButton::Left, shift);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.time_selection.unwrap().anchor_frame, anchor);
        assert!(view.last_enqueued_edit.is_none());
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(header.center(), shift);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_clip_ids(), vec![clip]);
        assert!(view.normalized_time_selection().is_none());
        assert!(view.time_selection_gesture.is_none() && view.time_range_move_gesture.is_none());
        assert!(view.last_enqueued_edit.is_none());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
}
