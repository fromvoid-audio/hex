//! Actual held geometry and tiny-bar input, independent of terminal-only checks.
use super::{at, fixture, pump, region};
use crate::editor::{musical_time::SnapGrid, viewport::Viewport};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{Command, LoopRegion};
use hex_session::service::Request;

fn painted(visual: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
    let bounds = visual.debug_bounds("loop-rail-range").unwrap();
    assert!(
        !visual
            .rendered_quad_paint_layers("loop-rail-range", false)
            .is_empty(),
        "actual completed range quad"
    );
    bounds
}
#[gpui::test]
fn off_grid_pure_y_hold_and_modifier_refresh_keep_original_preview_and_scene(
    cx: &mut TestAppContext,
) {
    let (handle, editor) = fixture(
        cx,
        50_123,
        146_123,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::Quarter;
        cx.notify();
    });
    visual.run_until_parked();
    let original = region(&editor, &visual);
    let original_scene = painted(&mut visual);
    let down = at(&mut visual, 0.25);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x, down.y + px(80.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let held = editor.read_with(&visual, |v, _| {
        let g = v.loop_rail_gesture.unwrap();
        assert!(!g.moved && g.click_disqualified);
        g.preview.unwrap()
    });
    let held_scene = painted(&mut visual);
    visual.simulate_modifiers_change(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    visual.simulate_modifiers_change(Modifiers::default());
    visual.run_until_parked();
    let refreshed = editor.read_with(&visual, |v, _| {
        v.loop_rail_gesture.unwrap().preview.unwrap()
    });
    let refreshed_scene = painted(&mut visual);
    eprintln!(
        "OFFGRID_HELD original={original:?} held={held:?} refreshed={refreshed:?} scenes={original_scene:?}/{held_scene:?}/{refreshed_scene:?}"
    );
    // Observe all held discriminators before the failure; terminal state alone
    // correctly stayed unmodified on the earlier accepted source.
    assert_eq!(
        (held, held_scene, refreshed, refreshed_scene),
        (original, original_scene, original, original_scene)
    );
}
#[gpui::test]
fn narrow_loop_center_drag_preserves_whole_range_at_two_zoom_levels(cx: &mut TestAppContext) {
    let mut failures = Vec::new();
    for length in [384_000_u64, 192_000] {
        for requested_width in [12.0_f32, 8.0, 1.0] {
            let viewport = Viewport { start: 0, length };
            let (handle, editor) = fixture(cx, 48_000, 96_000, viewport);
            let mut visual = VisualTestContext::from_window(handle, cx);
            visual.run_until_parked();
            let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
            let frames = (f64::from(requested_width) * length as f64
                / f64::from(f32::from(rail.size.width)))
            .round()
            .max(1.0) as u64;
            let original = LoopRegion::new(length / 2, length / 2 + frames, false).unwrap();
            editor.update(&mut visual, |v, _| {
                v.snap_grid = SnapGrid::Off;
                assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
                    start: original.start(),
                    end: original.end(),
                    enabled: false
                })));
            });
            pump(&editor, &mut visual);
            let before = painted(&mut visual);
            assert!((f32::from(before.size.width) - requested_width).abs() < 0.1);
            let down = point(before.center().x, rail.center().y);
            let target = point(down.x + px(18.0), down.y);
            editor.read_with(&visual, |v, _| {
                assert!(matches!(
                    v.loop_rail_hover(f32::from(down.x), viewport, rail),
                    super::super::Hover::Move
                ));
            });
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            let (mode, whole) = editor.read_with(&visual, |v, _| {
                let g = v.loop_rail_gesture.expect("actual tiny-bar capture");
                (
                    format!("{:?}", g.mode),
                    matches!(g.mode, super::super::EditMode::Move { .. }),
                )
            });
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let preview = editor.read_with(&visual, |v, _| {
                v.loop_rail_gesture.unwrap().preview.unwrap()
            });
            let held_scene = visual.debug_bounds("loop-rail-range").unwrap();
            let held_quads = visual
                .rendered_quad_paint_layers("loop-rail-range", false)
                .len();
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
            let actual = region(&editor, &visual);
            let passed = whole
                && held_quads > 0
                && actual == preview
                && actual.start() > original.start()
                && actual.end() - actual.start() == original.end() - original.start();
            eprintln!(
                "NARROW_CENTER viewport={length} requested_px={requested_width} actual_bounds={before:?} down={down:?} target={target:?} mode={mode} original={original:?} preview={preview:?} held_scene={held_scene:?} held_quads={held_quads} committed={actual:?} whole_pass={passed}"
            );
            if !passed {
                failures.push((length, requested_width, mode, original, actual));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "center must own whole-loop Move at every measured size: {failures:?}"
    );
}

#[gpui::test]
fn stationary_command_endpoints_toggle_and_command_edge_drags_remain_free(cx: &mut TestAppContext) {
    for length in [384_000_u64, 192_000] {
        for requested_width in [12.0_f32, 8.0, 1.0] {
            for left in [true, false] {
                let (handle, editor) = fixture(cx, 48_000, 96_000, Viewport { start: 0, length });
                let mut visual = VisualTestContext::from_window(handle, cx);
                visual.run_until_parked();
                let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
                let frames = (f64::from(requested_width) * length as f64
                    / f64::from(f32::from(rail.size.width)))
                .round()
                .max(1.0) as u64;
                let original = LoopRegion::new(length / 2, length / 2 + frames, false).unwrap();
                editor.update(&mut visual, |v, _| {
                    v.snap_grid = SnapGrid::Quarter;
                    assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
                        start: original.start(),
                        end: original.end(),
                        enabled: false
                    })));
                });
                pump(&editor, &mut visual);
                let bounds = painted(&mut visual);
                let command = Modifiers {
                    platform: true,
                    ..Modifiers::default()
                };
                let down = point(
                    if left { bounds.left() } else { bounds.right() },
                    rail.center().y,
                );
                // The user's law is bar-wide; endpoint hit regions are included.
                visual.simulate_mouse_down(down, MouseButton::Left, command);
                visual.simulate_mouse_up(down, MouseButton::Left, command);
                pump(&editor, &mut visual);
                let toggled = region(&editor, &visual);
                assert_eq!(
                    (toggled.start(), toggled.end(), toggled.enabled()),
                    (original.start(), original.end(), true)
                );
                visual.simulate_keystrokes("cmd-z");
                pump(&editor, &mut visual);
                assert_eq!(region(&editor, &visual), original);
                visual.simulate_mouse_down(down, MouseButton::Left, command);
                editor.read_with(&visual, |v, _| {
                    let mode = v.loop_rail_gesture.unwrap().mode;
                    assert!(if left {
                        matches!(mode, super::super::EditMode::ResizeStart { .. })
                    } else {
                        matches!(mode, super::super::EditMode::ResizeEnd { .. })
                    });
                });
                let target = point(down.x + px(if left { -18.0 } else { 18.0 }), down.y);
                visual.simulate_mouse_move(target, MouseButton::Left, command);
                visual.run_until_parked();
                let desired = editor.read_with(&visual, |v, _| {
                    v.loop_rail_gesture.unwrap().preview.unwrap()
                });
                painted(&mut visual);
                visual.simulate_mouse_up(target, MouseButton::Left, command);
                pump(&editor, &mut visual);
                assert_eq!(region(&editor, &visual), desired);
                assert!(!desired.enabled(), "a real edge drag cannot toggle");
                if left {
                    assert!(desired.start() < original.start());
                    assert_eq!(desired.end(), original.end());
                } else {
                    assert_eq!(desired.start(), original.start());
                    assert!(desired.end() > original.end());
                }
                assert_ne!(
                    if left { desired.start() } else { desired.end() } % 24_000,
                    0,
                    "Command bypasses Quarter grid during edge resize"
                );
                eprintln!(
                    "ENDPOINT_CONTROL viewport={length} px={requested_width} left={left} original={original:?} resized={desired:?}"
                );
            }
        }
    }
}

#[gpui::test]
fn minimum_frame_bar_body_keeps_move_even_when_pointer_rounds_to_end(cx: &mut TestAppContext) {
    let viewport = Viewport {
        start: 0,
        length: 300,
    };
    let (handle, editor) = fixture(cx, 100, 101, viewport);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::Off;
        cx.notify();
    });
    visual.run_until_parked();
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    let bar = painted(&mut visual);
    let down = point(bar.left() + bar.size.width * 0.6, rail.center().y);
    let frame = crate::editor::seek::pointer_to_frame(
        f32::from(down.x),
        f32::from(rail.left()),
        f32::from(rail.size.width),
        viewport,
    )
    .unwrap();
    assert_eq!(frame, 101, "real body pointer rounds to the exclusive end");
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(matches!(
            v.loop_rail_gesture.unwrap().mode,
            super::super::EditMode::Move { .. }
        ))
    });
    let target = point(down.x + px(18.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = editor.read_with(&visual, |v, _| {
        v.loop_rail_gesture.unwrap().preview.unwrap()
    });
    painted(&mut visual);
    assert!(preview.start() > 100);
    assert_eq!(preview.end() - preview.start(), 1);
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert_eq!(region(&editor, &visual), preview);
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    assert_eq!(
        region(&editor, &visual),
        LoopRegion::new(100, 101, false).unwrap()
    );
}

#[gpui::test]
fn viewport_edge_caps_resize_the_existing_loop_instead_of_painting_a_new_range(
    cx: &mut TestAppContext,
) {
    for (start, end, marker, target) in [
        (50, 100, "arrangement-loop-end-marker", 0.25),
        (200, 250, "arrangement-loop-start-marker", 0.75),
    ] {
        let cx = &mut cx.new_app();
        let (handle, editor) = fixture(
            cx,
            start,
            end,
            Viewport {
                start: 100,
                length: 100,
            },
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, cx| {
            v.snap_grid = SnapGrid::Off;
            cx.notify();
        });
        visual.run_until_parked();
        let cap = visual.debug_bounds(marker).unwrap();
        assert!(!visual.rendered_quad_paint_layers(marker, false).is_empty());
        let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
        let down = cap.center();
        let up = point(rail.left() + rail.size.width * target, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        let changed = region(&editor, &visual);
        if start == 50 {
            assert_eq!(changed.start(), start);
            assert!(changed.end() > end && changed.end() < 150);
        } else {
            assert_eq!(changed.end(), end);
            assert!(changed.start() < start && changed.start() > 150);
        }
        editor.read_with(&visual, |v, _| assert!(v.error.is_none(), "{:?}", v.error));
    }
}
