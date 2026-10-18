//! Actual Shift input → metadata preview → accepted queue → worker ack/history.
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{ClipId, Command, TrimEdge};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(10);
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
    let until = Instant::now() + Duration::from_secs(10);
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
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("resize-release.wav", 48_000, 96_000);
    let directory = wav.parent().unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let clip = event.state.project.tracks()[0].clips()[0].id();
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::MoveClip {
            clip,
            track,
            start: 96_000,
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                v.state = Some(event.state);
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, clip)
}
#[gpui::test]
fn shift_resize_preserves_visible_extent_through_release_and_real_worker_ack(
    cx: &mut TestAppContext,
) {
    for edge in [TrimEdge::Right, TrimEdge::Left] {
        let (_files, handle, editor, clip) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let name = if edge == TrimEdge::Right {
            "right"
        } else {
            "left"
        };
        let selector = Box::leak(format!("clip-trim-{name}-{}", clip.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_down(down, MouseButton::Left, shift);
        visual.simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, shift);
        visual.run_until_parked();
        let target = point(down.x + px(63.0), down.y);
        visual.simulate_mouse_move(target, MouseButton::Left, shift);
        visual.run_until_parked();
        let desired = editor.read_with(&visual, |v, _| {
            let drag = v.active_trim_drag.as_ref().expect("actual held Shift edge");
            assert!(drag.stretch_modifier.get());
            let p = drag.preview.borrow().as_ref().unwrap().clip.clone();
            assert!(p.audio_edit().unwrap().domain.warp_enabled);
            assert_ne!(p.duration(), 96_000);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            p
        });
        let held = visual
            .debug_bounds("trim-live-preview")
            .expect("actual held overlay");
        visual.simulate_mouse_up(target, MouseButton::Left, shift);
        visual.run_until_parked();
        let queued = editor.read_with(&visual, |v, _| {
            (v.worker_requests_pending, v.last_enqueued_edit.clone())
        });
        let released = visual
            .debug_bounds("trim-live-preview")
            .or_else(|| {
                visual.debug_bounds(Box::leak(
                    format!("clip-visual-{}", clip.value()).into_boxed_str(),
                ))
            })
            .expect("a visible released clip");
        pump(&editor, &mut visual);
        let actual = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .clone()
        });
        eprintln!(
            "release edge={edge:?} queued={} desired={}..{} releasedwidth={} heldwidth={} accepted={}..{} command={:?}",
            queued.0,
            desired.start(),
            desired.start() + desired.duration(),
            f32::from(released.size.width),
            f32::from(held.size.width),
            actual.start(),
            actual.start() + actual.duration(),
            queued.1
        );
        assert!(
            matches!(queued.1,Some(Command::StretchAudioClipEdge{clip:id,edge:requested,..})if id==clip&&requested==edge)
        );
        assert_eq!(
            actual, desired,
            "accepted Shift commit must retain exact warp/source metadata and extent"
        );
        assert!(
            (f32::from(released.left()) - f32::from(held.left())).abs() < 1.0
                && (f32::from(released.size.width) - f32::from(held.size.width)).abs() < 1.0,
            "accepted release must not expose old canonical extent while ack is pending"
        );
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.clip(clip), Some(&desired))
        });
    }
}
