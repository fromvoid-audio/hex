use super::*;
#[cfg(feature = "ui-test-support")]
use gpui::size;

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn native_scrollbar_tracks_wheel_outside_release_escape_and_preserves_selection_copy(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent};
    let dir = std::env::temp_dir().join(format!("hex-log-scroll-input-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session-600-1.log");
    let mut content = "session_started log_schema=3 segment=0\n".to_owned();
    for sequence in 1..=400 {
        let category = if sequence <= 3 { "audio" } else { "ui" };
        content.push_str(&format!("event level=info category={category} unix_nanos={sequence} elapsed_nanos={sequence} sequence={sequence} truncated=false message=\"line {sequence}\"\n"));
    }
    std::fs::write(&path, content).unwrap();
    let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
    cx.run_until_parked();
    let viewer = handle.root(cx).unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(*handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let last = visual.debug_bounds("log-row-400").unwrap();
    visual.simulate_click(last.center(), Modifiers::default());
    visual.run_until_parked();
    let selected = viewer.read_with(visual, |view, _| (view.anchor, view.selection));
    let initial = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    assert!(initial.maximum > 0.0);
    assert!((f32::from(initial.thumb.bottom() - initial.track.bottom())).abs() < 0.01);
    let pane = point(initial.track.left() - px(100.0), initial.track.center().y);
    visual.simulate_event(ScrollWheelEvent {
        position: pane,
        delta: ScrollDelta::Pixels(point(px(0.0), px(132.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let wheeled = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    assert!(
        wheeled.thumb.top() < initial.thumb.top(),
        "wheel and thumb share one offset"
    );
    let top = point(wheeled.track.center().x, wheeled.track.top() + px(1.0));
    visual.simulate_click(top, Modifiers::default());
    visual.run_until_parked();
    let at_top = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    assert_eq!(at_top.thumb.top(), at_top.track.top());
    let bottom_button = visual.debug_bounds("log-bottom").unwrap();
    visual.simulate_click(bottom_button.center(), Modifiers::default());
    visual.run_until_parked();
    let navigated = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    assert!((f32::from(navigated.thumb.bottom() - navigated.track.bottom())).abs() < 0.01);
    visual.simulate_click(top, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_down(
        at_top.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(viewer.read_with(visual, |view, _| view.scrollbar.drag.is_some()));
    let outside = point(px(-30.0), at_top.track.bottom() + px(400.0));
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let bottom = viewer.read_with(visual, |view, _| {
        assert!(view.scrollbar.drag.is_none());
        assert_eq!((view.anchor, view.selection), selected);
        view.scrollbar.geometry.get().unwrap()
    });
    assert!((f32::from(bottom.thumb.bottom() - bottom.track.bottom())).abs() < 0.01);
    // A final release with no intervening move uses its own position.
    visual.simulate_mouse_down(
        bottom.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(top, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let geometry = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    assert_eq!(geometry.thumb.top(), geometry.track.top());
    visual.simulate_mouse_down(
        geometry.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        geometry.thumb.center() + point(px(0.0), px(30.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(top, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    viewer.update(visual, |view, cx| {
        assert!(view.scrollbar.drag.is_none());
        assert_eq!((view.anchor, view.selection), selected);
        view.copy_selection(cx);
    });
    visual.run_until_parked();
    viewer.read_with(visual, |_, cx| {
        assert!(
            cx.read_from_clipboard()
                .unwrap()
                .text()
                .unwrap()
                .contains("message=\"line 400\"")
        )
    });
    let geometry = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    visual.simulate_mouse_down(
        geometry.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    viewer.update(visual, |view, cx| {
        view.category = super::super::CATEGORIES
            .iter()
            .position(|category| *category == "audio")
            .unwrap();
        view.filter(cx);
    });
    visual.run_until_parked();
    viewer.read_with(visual, |view, _| {
        assert!(view.scrollbar.drag.is_none());
        assert_eq!(view.rows.len(), 4);
        assert_eq!(view.scrollbar.geometry.get().unwrap().maximum, 0.0);
    });
    visual.simulate_mouse_up(top, MouseButton::Left, Modifiers::default());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn page_refresh_resize_and_deactivation_retire_old_scroll_captures(cx: &mut gpui::TestAppContext) {
    use gpui::Modifiers;
    let (dir, path) = super::super::tests::paged_fixture("scroll-pages");
    let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
    cx.run_until_parked();
    let viewer = handle.root(cx).unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(*handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    let initial = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    assert_eq!(initial.rows, 100_000);
    assert!(initial.maximum > 1_000_000.0);
    assert!(visual.debug_bounds("log-scrollbar").is_some());
    visual.simulate_mouse_down(
        initial.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    viewer.update(visual, |view, cx| view.shift_page(true, cx));
    visual.run_until_parked();
    viewer.read_with(visual, |view, _| {
        assert_eq!(view.page, 1);
        assert!(view.status.contains("filters apply to this page"));
        assert!(view.scrollbar.drag.is_none());
        let short = view.scrollbar.geometry.get().unwrap();
        assert_eq!(short.thumb, short.track);
    });
    visual.simulate_mouse_up(
        initial.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    viewer.update(visual, |view, cx| view.shift_page(false, cx));
    visual.run_until_parked();
    let before = viewer.read_with(visual, |view, _| view.scrollbar.geometry.get().unwrap());
    visual.simulate_mouse_down(
        before.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_resize(size(px(850.0), px(350.0)));
    visual.run_until_parked();
    let resized = viewer.read_with(visual, |view, _| {
        assert!(view.scrollbar.drag.is_none());
        view.scrollbar.geometry.get().unwrap()
    });
    assert!(resized.track.size.height < before.track.size.height);
    assert!(
        resized.thumb.top() >= resized.track.top()
            && resized.thumb.bottom() <= resized.track.bottom() + px(0.01)
    );
    visual.simulate_mouse_up(
        resized.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_down(
        resized.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.deactivate_window();
    visual.run_until_parked();
    viewer.read_with(visual, |view, _| assert!(view.scrollbar.drag.is_none()));
    std::fs::write(&path, "").unwrap();
    viewer.update(visual, |view, cx| view.reload(cx));
    visual.run_until_parked();
    viewer.read_with(visual, |view, _| {
        assert!(view.rows.is_empty());
        assert!(view.scrollbar.geometry.get().is_none());
    });
    std::fs::remove_dir_all(dir).unwrap();
}
