//! Real canonical worker + rendered loop-bar gestures, not user app interaction.
use super::super::{EditorView, musical_time::SnapGrid, viewport::Viewport};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, Command, LoopRegion, Tempo};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};
fn next(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
    start: u64,
    end: u64,
    viewport: Viewport,
) -> (gpui::AnyWindowHandle, Entity<EditorView>) {
    cx.update(super::super::bind_editor_keys);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "keep contents".into(),
        }))
        .unwrap();
    let event = next(&service);
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_raw(16 << 32),
        }))
        .unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::SetLoopRegion {
            start,
            end,
            enabled: false,
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline loop bar test".into()))
            });
            editor.update(cx, |v, cx| {
                v.state = Some(event.state);
                v.viewport = Some(viewport);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor)
}
fn at(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
    point(
        rail.left() + rail.size.width * fraction,
        rail.top() + rail.size.height / 2.0,
    )
}
fn region(editor: &Entity<EditorView>, visual: &VisualTestContext) -> LoopRegion {
    editor.read_with(visual, |v, _| {
        v.state.as_ref().unwrap().project.loop_region().unwrap()
    })
}
fn click(visual: &mut VisualTestContext, p: gpui::Point<gpui::Pixels>, modifiers: Modifiers) {
    visual.simulate_mouse_down(p, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(p, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
#[gpui::test]
fn command_click_toggles_both_ways_and_preserves_content_history(cx: &mut TestAppContext) {
    let (handle, editor) = fixture(
        cx,
        48_000,
        144_000,
        Viewport {
            start: 0,
            length: 384_000,
        },
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let p = at(&mut visual, 0.25);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    for enabled in [true, false, true] {
        click(&mut visual, p, command);
        pump(&editor, &mut visual);
        let r = region(&editor, &visual);
        assert_eq!(
            (r.start(), r.end(), r.enabled()),
            (48_000, 144_000, enabled)
        );
    }
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().project.snapshot(),
            before,
            "one Undo reverses one toggle and contents persist"
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let snapshot = v.state.as_ref().unwrap().project.snapshot();
        assert_eq!(
            hex_project::Project::from_snapshot(snapshot.clone())
                .unwrap()
                .snapshot(),
            snapshot
        );
    });
}
#[gpui::test]
fn body_drag_and_shift_steps_preserve_full_length_original_origin_and_one_undo(
    cx: &mut TestAppContext,
) {
    for viewport in [
        Viewport {
            start: 0,
            length: 384_000,
        },
        Viewport {
            start: 24_000,
            length: 192_000,
        },
    ] {
        let (handle, editor) = fixture(cx, 48_000, 96_000, viewport);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let fraction = (72_000 - viewport.start) as f32 / viewport.length as f32;
        let down = at(&mut visual, fraction);
        let rail = visual.debug_bounds("arrangement-loop-rail").unwrap();
        let step_px = rail.size.width * (48_000.0 / viewport.length as f32);
        let target = point(down.x + step_px * 1.2, down.y);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let gesture = v.loop_rail_gesture.expect("body owns whole-range drag");
            let p = gesture.preview.unwrap();
            assert_eq!((p.start(), p.end(), p.enabled()), (96_000, 144_000, false));
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        assert_eq!(region(&editor, &visual).start(), 96_000);
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        // Shift uses the original origin in the reverse direction, with a zero lower bound.
        let down = at(&mut visual, fraction);
        let target = point(down.x - step_px * 1.2, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        pump(&editor, &mut visual);
        let r = region(&editor, &visual);
        assert_eq!((r.start(), r.end()), (0, 48_000));
    }
}
#[gpui::test]
fn musical_binary_marker_supports_fractional_durations_and_ignores_view_width(
    cx: &mut TestAppContext,
) {
    for (raw, expected) in [
        (2 << 32, true),
        (4 << 32, true),
        (3 << 32, false),
        (1 << 31, true),
        (1 << 30, true),
    ] {
        let end = (raw as u128 * 24_000 / (1u128 << 32)) as u64;
        let (handle, editor) = fixture(
            cx,
            0,
            end,
            Viewport {
                start: 0,
                length: 384_000,
            },
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("loop-binary-length-marker").is_some(),
            expected,
            "rawbeatduration={raw}"
        );
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(Viewport {
                start: 0,
                length: 192_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("loop-binary-length-marker").is_some(),
            expected
        );
    }
}
