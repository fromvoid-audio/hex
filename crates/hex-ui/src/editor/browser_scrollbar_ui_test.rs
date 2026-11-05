//! Rendered virtual-list geometry and the actual Window pointer lease.
use super::*;
use gpui::{ScrollDelta, ScrollWheelEvent};

#[gpui::test]
fn browser_thumb_tracks_wheel_and_drag_without_selection_or_audition(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.listing.as_mut().unwrap().entries = (0..1000)
            .map(|i| LibraryEntry {
                name: format!("sample-{i:04}.wav"),
                path: directory.0.join(format!("sample-{i:04}.wav")),
                kind: EntryKind::WaveAudio,
            })
            .collect();
        view.browser.samples.invalidate_listing();
        view.browser.reset_results();
        cx.notify();
    });
    visual.run_until_parked();
    let first = visual.debug_bounds("browser-entry-0").unwrap();
    visual.simulate_click(first.center(), Modifiers::default());
    visual.run_until_parked();
    let before = editor.read_with(visual, |view, _| {
        (
            view.browser.selected_index,
            view.browser.playback_generation,
            view.browser.preview_preparations,
            view.pending_requests.len(),
            view.pending_audio_commands.len(),
        )
    });
    let initial = editor.read_with(visual, |view, _| {
        view.browser.scrollbar.geometry.get().unwrap()
    });
    assert!(initial.maximum > 0.0);
    assert_eq!(initial.thumb.top(), initial.track.top());
    visual.simulate_event(ScrollWheelEvent {
        position: first.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-300.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    let wheeled = editor.read_with(visual, |view, _| {
        view.browser.scrollbar.geometry.get().unwrap()
    });
    assert!(wheeled.thumb.top() > initial.thumb.top());
    visual.simulate_mouse_down(
        wheeled.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(editor.read_with(visual, |view, _| view.browser.scrollbar.drag.is_some()));
    let outside = point(px(-30.0), wheeled.track.bottom() + px(400.0));
    visual.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let bottom = editor.read_with(visual, |view, _| {
        assert!(view.browser.scrollbar.drag.is_none());
        assert_eq!(
            (
                view.browser.selected_index,
                view.browser.playback_generation,
                view.browser.preview_preparations,
                view.pending_requests.len(),
                view.pending_audio_commands.len()
            ),
            before
        );
        view.browser.scrollbar.geometry.get().unwrap()
    });
    assert!((f32::from(bottom.thumb.bottom() - bottom.track.bottom())).abs() < 0.01);
    assert!(visual.debug_bounds("browser-entry-999").is_some());
    visual.simulate_mouse_down(
        bottom.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    // Release alone must use its own coordinate, including outside the thumb.
    visual.simulate_mouse_up(
        point(bottom.track.center().x, bottom.track.top() - px(40.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let top = editor.read_with(visual, |view, _| {
        view.browser.scrollbar.geometry.get().unwrap()
    });
    assert_eq!(top.thumb.top(), top.track.top());
    assert!(visual.debug_bounds("browser-entry-0").is_some());
    visual.simulate_mouse_down(top.thumb.center(), MouseButton::Left, Modifiers::default());
    let moved = top.thumb.center() + point(px(0.0), px(20.0));
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    // Ordinary completion preserves result keyboard navigation. Escape also
    // reaches the existing editor key handler and has its own focus policy.
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert!(visual.debug_bounds("browser-entry-1").is_some());
    assert_eq!(
        editor.read_with(visual, |view, _| view.browser.selected_index),
        1
    );
    let navigation = editor.read_with(visual, |view, _| {
        view.browser.scrollbar.geometry.get().unwrap()
    });
    visual.simulate_mouse_down(
        navigation.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(
        navigation.thumb.center() + point(px(0.0), px(20.0)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(editor.read_with(visual, |view, _| view.browser.scrollbar.drag.is_none()));
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn browser_thumb_recomputes_after_filter_and_resize_and_retires_capture(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser.auto_audition = false;
        view.browser.listing.as_mut().unwrap().entries = (0..1000)
            .map(|i| LibraryEntry {
                name: format!("sample-{i:04}.wav"),
                path: directory.0.join(format!("sample-{i:04}.wav")),
                kind: EntryKind::WaveAudio,
            })
            .collect();
        view.browser.samples.invalidate_listing();
        cx.notify();
    });
    visual.run_until_parked();
    let full = editor.read_with(visual, |view, _| {
        view.browser.scrollbar.geometry.get().unwrap()
    });
    visual.simulate_mouse_down(full.thumb.center(), MouseButton::Left, Modifiers::default());
    editor.update(visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("sample-00", cx));
    });
    visual.run_until_parked();
    let filtered = editor.read_with(visual, |view, _| {
        assert!(view.browser.scrollbar.drag.is_none());
        view.browser.scrollbar.geometry.get().unwrap()
    });
    assert!(filtered.thumb.size.height > full.thumb.size.height);
    visual.simulate_mouse_up(
        filtered.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_down(
        filtered.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_resize(gpui::size(px(880.0), px(540.0)));
    visual.run_until_parked();
    let resized = editor.read_with(visual, |view, _| {
        assert!(view.browser.scrollbar.drag.is_none());
        view.browser.scrollbar.geometry.get().unwrap()
    });
    visual.simulate_mouse_up(
        resized.thumb.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert_ne!(filtered.track, resized.track);
    assert!(resized.maximum > 0.0);
    editor.update(visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("sample-0000", cx));
    });
    visual.run_until_parked();
    assert_eq!(
        editor.read_with(visual, |view, _| view
            .browser
            .scrollbar
            .geometry
            .get()
            .unwrap()
            .maximum),
        0.0
    );
    editor.update(visual, |view, cx| {
        view.browser
            .search
            .update(cx, |input, cx| input.set_text("missing", cx));
    });
    visual.run_until_parked();
    assert!(editor.read_with(visual, |view, _| {
        view.browser.scrollbar.geometry.get().is_none()
    }));
}
