//! Arrangement left-edge leading buffer alignment and pointer interaction tests (Pulse #8947).
use super::arrangement::LEADING_BUFFER_WIDTH;
use super::viewport::Viewport;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{Command, Created, Editor, Project};
use std::sync::Arc;

fn fixture_with_clip_at_zero() -> (Project, hex_project::TrackId, hex_project::ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "lead".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio clip");
    };
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 96_000,
            enabled: true,
        })
        .unwrap();
    (editor.project().clone(), track, clip)
}

#[gpui::test]
fn arrangement_leading_buffer_aligns_all_rows_and_clamps_pointer_interactions(
    cx: &mut TestAppContext,
) {
    cx.update(super::bind_editor_keys);
    let (project, track, clip) = fixture_with_clip_at_zero();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    // 1. Verify visible leading buffer and pixel-exact frame-0 alignment across all rows.
    let surface = visual.debug_bounds("arrangement-surface").unwrap();
    let buffer = visual
        .debug_bounds("arrangement-leading-buffer")
        .expect("visible leading buffer must render");
    assert_eq!(buffer.left(), surface.left());
    assert_eq!(buffer.size.width, px(LEADING_BUFFER_WIDTH));
    assert_eq!(buffer.top(), surface.top());
    assert_eq!(buffer.bottom(), surface.bottom());

    let ruler = visual.debug_bounds("timeline-ruler").unwrap();
    let loop_rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    let lane_sel = Box::leak(format!("arrangement-lane-{}", track.value()).into_boxed_str());
    let lane = visual.debug_bounds(lane_sel).unwrap();
    let main_lane = visual.debug_bounds("main-track-lane").unwrap();
    let clip_sel = Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let clip_bounds = visual.debug_bounds(clip_sel).unwrap();
    let grid_zero = visual.debug_bounds("arrangement-grid-line-0").unwrap();
    let tick_zero = visual.debug_bounds("ruler-tick-0").unwrap();

    for (name, bounds) in [
        ("timeline-ruler", ruler),
        ("arrangement-loop-rail", loop_rail),
        ("track-lane", lane),
        ("main-track-lane", main_lane),
    ] {
        assert!(
            (bounds.left() - buffer.right()).abs() <= px(0.5),
            "{name} left {:?} must align with leading buffer right {:?}",
            bounds.left(),
            buffer.right()
        );
    }
    assert!(
        (clip_bounds.left() - lane.left()).abs() <= px(0.5),
        "clip at frame 0 must align with lane origin"
    );
    assert!(
        (grid_zero.left() - lane.left()).abs() <= px(1.0),
        "frame-0 gridline must align with lane origin"
    );
    assert!(
        (tick_zero.left() - ruler.left()).abs() <= px(1.0),
        "bar-1 ruler tick must align with ruler origin"
    );

    // 2. Clicking inside the leading buffer at ruler height targets frame 0 (no underflow).
    editor.update(&mut visual, |view, cx| {
        view.pending_seek_ticket = Some(1);
        cx.notify();
    });
    visual.run_until_parked();
    let ruler_buffer_pt = point(buffer.center().x, ruler.center().y);
    visual.simulate_mouse_down(ruler_buffer_pt, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(ruler_buffer_pt, MouseButton::Left, Modifiers::default());
    editor.update(&mut visual, |view, _| {
        assert!(
            view.scrub_active(),
            "clicking inside leading buffer at ruler height must begin ruler scrub"
        );
        view.pending_seek_ticket = None;
        let _ = view.cancel_scrub();
    });
    visual.run_until_parked();

    // 3. Starting a marquee time selection inside the leading buffer drags smoothly from frame 0.
    let drag_start = point(buffer.center().x, lane.center().y);
    let drag_end = point(lane.left() + lane.size.width * 0.25, lane.center().y);
    visual.simulate_mouse_down(drag_start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(drag_end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(drag_end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let selection = editor
        .read_with(&visual, |view, _| view.time_selection)
        .expect("marquee drag starting inside leading buffer must create a time selection");
    assert_eq!(
        selection.anchor_frame.min(selection.end_frame),
        0,
        "selection started in leading buffer must clamp cleanly to frame 0"
    );
    assert!(selection.anchor_frame.max(selection.end_frame) > 0);

    // 4. Clicking inside the leading buffer at track height places edit cursor at frame 0.
    visual.simulate_click(drag_start, Modifiers::default());
    visual.run_until_parked();
    let cursor = editor
        .read_with(&visual, |view, _| view.edit_cursor)
        .expect("clicking leading buffer at track height must place edit cursor");
    assert_eq!(cursor.track, Some(track));
    assert_eq!(cursor.frame, 0);

    // 5. Clicking inside the leading buffer at loop rail height starts a loop-start resize at frame 0.
    let loop_buffer_pt = point(buffer.right() - px(3.0), loop_rail.center().y);
    visual.simulate_mouse_down(loop_buffer_pt, MouseButton::Left, Modifiers::default());
    assert!(
        editor.read_with(&visual, |view, _| view.loop_rail_gesture.is_some()),
        "grabbing loop start handle from inside leading buffer must begin loop gesture"
    );
    visual.simulate_mouse_up(loop_buffer_pt, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    // 6. Panned rightward (viewport.start > 0): alignment holds and leading buffer clamps to viewport.start.
    editor.update(&mut visual, |view, cx| {
        view.viewport = Some(Viewport {
            start: 48_000,
            length: 192_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let buffer_panned = visual.debug_bounds("arrangement-leading-buffer").unwrap();
    let ruler_panned = visual.debug_bounds("timeline-ruler").unwrap();
    let lane_panned = visual.debug_bounds(lane_sel).unwrap();
    assert_eq!(buffer_panned.size.width, px(LEADING_BUFFER_WIDTH));
    assert!((ruler_panned.left() - buffer_panned.right()).abs() <= px(0.5));
    assert!((lane_panned.left() - buffer_panned.right()).abs() <= px(0.5));
    // Clicking the leading buffer at track height while panned places edit cursor at viewport.start (48_000).
    visual.simulate_click(
        point(buffer_panned.center().x, lane_panned.center().y),
        Modifiers::default(),
    );
    visual.run_until_parked();
    let panned_cursor = editor
        .read_with(&visual, |view, _| view.edit_cursor)
        .expect("clicking leading buffer while panned must place edit cursor");
    assert_eq!(
        panned_cursor.frame, 48_000,
        "clicking leading buffer while panned must clamp cleanly to viewport.start"
    );
    assert_eq!(
        super::seek::pointer_to_frame(
            f32::from(buffer_panned.center().x),
            f32::from(ruler_panned.left()),
            f32::from(ruler_panned.size.width),
            Viewport {
                start: 48_000,
                length: 192_000,
            },
        ),
        Some(48_000),
    );
}
