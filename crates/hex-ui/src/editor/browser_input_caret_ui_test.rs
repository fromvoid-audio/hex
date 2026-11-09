//! Idle cost of a focused search caret: a stepped blink, not a frame loop.
use super::super::{EditorView, navigation_metrics, test_environment::Environment};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use hex_session::service::Service;
use std::{sync::Arc, time::Duration};

fn editor_renders() -> u64 {
    navigation_metrics::snapshot().calls[navigation_metrics::Stage::Editor as usize]
}

#[gpui::test]
fn focused_search_caret_renders_editor_at_most_three_times_per_idle_second(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.search.read(cx).focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    // Settle startup work so only the idle caret remains.
    for _ in 0..4 {
        visual.executor().advance_clock(Duration::from_millis(250));
        visual.run_until_parked();
    }
    assert!(visual.update(|window, cx| {
        editor
            .read(cx)
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
    }));
    navigation_metrics::reset(true);
    for _ in 0..100 {
        visual.executor().advance_clock(Duration::from_millis(10));
        visual.run_until_parked();
    }
    let renders = editor_renders();
    navigation_metrics::reset(false);
    assert!(
        renders <= 3,
        "a focused idle caret rendered the editor {renders} times in one second"
    );
    assert!(
        renders >= 1,
        "the caret must still blink while focused, got {renders} renders"
    );
    // Losing focus stops painting the caret, which drops its pending timer.
    visual.update(|window, _| window.blur());
    visual.run_until_parked();
    navigation_metrics::reset(true);
    for _ in 0..100 {
        visual.executor().advance_clock(Duration::from_millis(10));
        visual.run_until_parked();
    }
    let unfocused = editor_renders();
    navigation_metrics::reset(false);
    assert_eq!(
        unfocused, 0,
        "an unfocused field must not keep waking the editor"
    );
}
