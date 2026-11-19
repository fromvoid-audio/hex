//! Real hovered-row wheel dispatch stays in transient layout state.
use super::*;
use gpui::{ScrollDelta, ScrollWheelEvent};
fn option() -> Modifiers {
    Modifiers {
        alt: true,
        ..Default::default()
    }
}
fn wheel(
    visual: &mut VisualTestContext,
    at: gpui::Point<Pixels>,
    delta: ScrollDelta,
    modifiers: Modifiers,
) {
    visual.simulate_event(ScrollWheelEvent {
        position: at,
        delta,
        modifiers,
        ..Default::default()
    });
    visual.run_until_parked();
}
fn line(y: f32) -> ScrollDelta {
    ScrollDelta::Lines(point(0., y))
}
#[gpui::test]
fn option_wheel_targets_hovered_header_and_lane_without_editing_or_zooming(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    // Select via the name: a resized header's center may be a mixer control.
    let first = bounds(&mut visual, "track-name", tracks[0]);
    visual.simulate_click(first.center(), Modifiers::default());
    let original = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
        )
    });
    let second = bounds(&mut visual, "track-name-area", tracks[1]);
    wheel(&mut visual, second.center(), line(1.), option());
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[0])
            .size
            .height,
        px(44.)
    );
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[1])
            .size
            .height,
        px(66.)
    );
    let lane = bounds(&mut visual, "arrangement-lane", tracks[1]);
    for height in [66., 66., 66., 44.] {
        wheel(
            &mut visual,
            lane.center(),
            ScrollDelta::Pixels(point(px(0.), px(-16.))),
            option(),
        );
        assert_eq!(
            bounds(&mut visual, "arrangement-track-header", tracks[1])
                .size
                .height,
            px(height)
        );
    }
    let header = bounds(&mut visual, "arrangement-track-header", tracks[1]);
    assert_eq!(header.size.height, px(44.));
    let lane = bounds(&mut visual, "arrangement-lane", tracks[1]);
    assert_eq!(header.top(), lane.top());
    // Natural-scroll inversion is undone exactly once for physical sizing.
    visual.simulate_event(ScrollWheelEvent {
        position: lane.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-64.))),
        modifiers: option(),
        direction_inverted_from_device: true,
        ..Default::default()
    });
    visual.run_until_parked();
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[1])
            .size
            .height,
        px(66.)
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_track, Some(tracks[0]));
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original.0);
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            original.1
        );
        assert_eq!(v.worker_requests_pending, 0);
    });
}
#[gpui::test]
fn option_wheel_caps_trackpad_steps_and_minimum_and_preserves_normal_wheel(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    let at = bounds(&mut visual, "track-name-area", tracks[0]).center();
    wheel(
        &mut visual,
        at,
        ScrollDelta::Pixels(point(px(0.), px(1_000_000.))),
        option(),
    );
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[0])
            .size
            .height,
        px(88.)
    );
    for _ in 0..3 {
        let at = bounds(&mut visual, "track-name-area", tracks[0]).center();
        wheel(&mut visual, at, line(-10.), option());
    }
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[0])
            .size
            .height,
        px(22.)
    );
    let at = bounds(&mut visual, "track-name-area", tracks[0]).center();
    for delta in [
        ScrollDelta::Pixels(point(px(0.), px(f32::NAN))),
        ScrollDelta::Pixels(point(px(0.), px(f32::INFINITY))),
        ScrollDelta::Pixels(point(px(80.), px(0.))),
    ] {
        wheel(&mut visual, at, delta, option());
    }
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[0])
            .size
            .height,
        px(22.)
    );
    let lane = bounds(&mut visual, "arrangement-lane", tracks[1]);
    let span = editor.read_with(&visual, |v, _| {
        v.visible_viewport(v.state.as_ref().unwrap().timeline_frames)
    });
    wheel(&mut visual, lane.center(), line(1.), Modifiers::default());
    assert_ne!(
        editor.read_with(&visual, |v, _| v
            .visible_viewport(v.state.as_ref().unwrap().timeline_frames)),
        span
    );
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[1])
            .size
            .height,
        px(44.)
    );
}
#[gpui::test]
fn option_wheel_main_and_overview_share_existing_heights_and_held_resize_excludes_wheel(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, tracks) = fixture(cx, 2);
    let main = visual.debug_bounds("pinned-main-track").unwrap();
    wheel(&mut visual, main.center(), line(1.), option());
    assert_eq!(
        visual
            .debug_bounds("pinned-main-track")
            .unwrap()
            .size
            .height,
        px(66.)
    );
    let name = bounds(&mut visual, "track-name-area", tracks[0]);
    visual.simulate_click(name.center(), Modifiers::default());
    visual.simulate_keystrokes("h");
    visual.run_until_parked();
    let name = bounds(&mut visual, "track-name-area", tracks[0]);
    wheel(&mut visual, name.center(), line(1.), option());
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[0])
            .size
            .height,
        px(44.)
    );
    visual.simulate_keystrokes("h");
    visual.run_until_parked();
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[0])
            .size
            .height,
        px(44.)
    );
    assert_eq!(
        visual
            .debug_bounds("pinned-main-track")
            .unwrap()
            .size
            .height,
        px(66.)
    );
    let edge = bounds(&mut visual, "track-height-resize", tracks[0]).center();
    visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    let before = bounds(&mut visual, "arrangement-track-header", tracks[1])
        .size
        .height;
    let at = bounds(&mut visual, "track-name-area", tracks[1]).center();
    wheel(&mut visual, at, line(1.), option());
    assert_eq!(
        bounds(&mut visual, "arrangement-track-header", tracks[1])
            .size
            .height,
        before
    );
    visual.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(!v.track_resize_active()));
}

#[gpui::test]
fn option_wheel_centers_hovered_track_as_it_resizes(cx: &mut TestAppContext) {
    let (mut visual, editor, tracks) = fixture(cx, 40);
    editor.update(&mut visual, |view, cx| {
        view.track_scroll
            .scroll_to_item(10, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let track = tracks[12];
    for delta in [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, -1.0, -1.0] {
        let row = bounds(&mut visual, "arrangement-track-header", track);
        wheel(&mut visual, row.center(), line(delta), option());
        let row = bounds(&mut visual, "arrangement-track-header", track);
        let viewport =
            editor.read_with(&visual, |view, _| view.track_scroll.list.viewport_bounds());
        assert!(
            (f32::from(row.center().y - viewport.center().y)).abs() <= 1.0,
            "resized row center {:?} must stay at viewport center {:?}",
            row,
            viewport
        );
    }
    let row = bounds(&mut visual, "arrangement-track-header", track);
    wheel(
        &mut visual,
        row.center(),
        ScrollDelta::Pixels(point(px(0.0), px(-16.0))),
        option(),
    );
    assert_eq!(bounds(&mut visual, "arrangement-track-header", track), row);
}

#[gpui::test]
fn option_wheel_centering_respects_scroll_endpoints(cx: &mut TestAppContext) {
    let (mut visual, editor, tracks) = fixture(cx, 40);
    for (track, strategy) in [
        (tracks[0], gpui::ScrollStrategy::Top),
        (tracks[39], gpui::ScrollStrategy::Bottom),
    ] {
        editor.update(&mut visual, |view, cx| {
            let index = if track == tracks[0] {
                0
            } else {
                tracks.len() - 1
            };
            view.track_scroll.scroll_to_item(index, strategy);
            cx.notify();
        });
        visual.run_until_parked();
        for delta in [1.0, 1.0, -1.0] {
            let row = bounds(&mut visual, "arrangement-track-header", track);
            wheel(&mut visual, row.center(), line(delta), option());
            let row = bounds(&mut visual, "arrangement-track-header", track);
            editor.read_with(&visual, |view, _| {
                let viewport = view.track_scroll.list.viewport_bounds();
                let offset = -f32::from(view.track_scroll.offset().y);
                let maximum = f32::from(view.track_scroll.max_offset().height);
                assert!(
                    offset >= -1.0 && offset <= maximum + 1.0,
                    "offset={offset}, max={maximum}"
                );
                assert!(row.bottom() > viewport.top() && row.top() < viewport.bottom());
            });
        }
    }
}

#[gpui::test]
fn option_wheel_centers_track_taller_than_viewport(cx: &mut TestAppContext) {
    let (mut visual, editor, tracks) = fixture(cx, 40);
    editor.update(&mut visual, |view, cx| {
        view.track_scroll
            .scroll_to_item(10, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let track = tracks[12];
    for _ in 0..8 {
        let at = bounds(&mut visual, "arrangement-track-header", track).center();
        wheel(&mut visual, at, line(1.0), option());
    }
    visual.simulate_resize(size(px(1_280.0), px(620.0)));
    visual.run_until_parked();
    let viewport = editor.read_with(&visual, |view, _| view.track_scroll.list.viewport_bounds());
    let row = bounds(&mut visual, "arrangement-track-header", track);
    assert!(
        row.size.height > viewport.size.height,
        "fixture needs a taller row: {row:?} {viewport:?}"
    );
    let previous_height = row.size.height;
    // The full-height name plate can extend under an adjacent pane after a
    // window shrink. Use the visible title's hit position for the wheel input.
    let at = bounds(&mut visual, "track-name", track).center();
    assert!(
        visual
            .debug_bounds("arrangement-content")
            .unwrap()
            .contains(&at)
    );
    wheel(&mut visual, at, line(-1.0), option());
    let viewport = editor.read_with(&visual, |view, _| view.track_scroll.list.viewport_bounds());
    let row = bounds(&mut visual, "arrangement-track-header", track);
    // The existing height limit can clamp the row to the smaller viewport.
    assert!(
        row.size.height < previous_height,
        "row {row:?} did not shrink from {previous_height:?} at {at:?}"
    );
    assert!(
        (f32::from(row.center().y - viewport.center().y)).abs() <= 1.0,
        "row {row:?}, viewport {viewport:?}"
    );
}
