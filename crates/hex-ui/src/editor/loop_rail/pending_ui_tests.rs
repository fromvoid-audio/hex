//! Real pointer scenes across a held, genuine Service response.
use super::*;
use crate::editor::test_environment::Environment;
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext, point, size};
use hex_session::service::{Event, Service};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ready = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.state.is_some() && !v.edit_pending()
        });
        visual.run_until_parked();
        if ready {
            return;
        }
        assert!(Instant::now() < deadline, "actual Service completion");
        std::thread::yield_now();
    }
}
fn fixture(
    cx: &mut TestAppContext,
    old: LoopRegion,
) -> (Arc<Environment>, Entity<EditorView>, VisualTestContext) {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    Service::new(48_000, 32, 4_000_000).unwrap(),
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1400.), px(800.)));
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "source".into()
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(Viewport {
            start: 0,
            length: 384_000,
        });
        assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
            start: old.start(),
            end: old.end(),
            enabled: old.enabled()
        })));
        cx.notify();
    });
    pump(&editor, &mut visual);
    (environment, editor, visual)
}
fn rail_point(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    point(rail.left() + rail.size.width * fraction, rail.center().y)
}
fn hold_real_response(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = editor.update(&mut *visual, |v, _| v.service.try_event().ok()) {
            return event;
        }
        assert!(Instant::now() < deadline, "real worker response");
        std::thread::yield_now();
    }
}
fn painted_range(visual: &mut VisualTestContext) -> Bounds<gpui::Pixels> {
    let bounds = visual
        .debug_bounds("loop-rail-range")
        .expect("rendered loop geometry");
    assert!(
        !visual
            .rendered_quad_paint_layers("loop-rail-range", false)
            .is_empty(),
        "completed scene must contain the actual filled range quad"
    );
    bounds
}
#[gpui::test]
fn replacement_loop_stays_painted_until_its_real_response_installs(cx: &mut TestAppContext) {
    let old = LoopRegion::new(24_000, 48_000, true).unwrap();
    let (_environment, editor, mut visual) = fixture(cx, old);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let old_paint = painted_range(&mut visual);
    let start = rail_point(&mut visual, 0.25);
    let end = rail_point(&mut visual, 0.5);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let held = painted_range(&mut visual);
    assert_ne!(held, old_paint);
    let expected = editor.read_with(&visual, |v, _| {
        v.loop_rail_gesture.unwrap().preview.unwrap()
    });
    assert_eq!(expected, LoopRegion::new(96_000, 192_000, false).unwrap());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    // Delay UI delivery of a real worker Event, without faking acceptance or state.
    let response = hold_real_response(&editor, &mut visual);
    assert!(
        response.request_complete && response.error.is_none(),
        "{:?}",
        response.error
    );
    assert_eq!(response.state.project.loop_region(), Some(expected));
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.run_until_parked();
    assert_eq!(
        painted_range(&mut visual),
        held,
        "accepted replacement must not flash the old canonical bar before its actual response"
    );
    editor.update(&mut visual, |v, cx| {
        v.receive(response);
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(painted_range(&mut visual), held);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.loop_region(),
            Some(expected)
        );
        assert!(v.loop_rail_gesture.is_none());
        assert_eq!(v.worker_requests_pending, 0);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    assert_eq!(painted_range(&mut visual), old_paint);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    assert_eq!(painted_range(&mut visual), held);
}
