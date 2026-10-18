//! Actual unsupported Shift input must clamp to a prepared-capable boundary.
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
    let wav = files.wav("stretch-capability.wav", 48_000, 96_000);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
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
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
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
fn shift_right_preview_boundary_is_accepted_by_actual_worker(cx: &mut TestAppContext) {
    check_boundary(cx, TrimEdge::Right);
}
#[gpui::test]
fn shift_left_preview_boundary_is_accepted_by_actual_worker(cx: &mut TestAppContext) {
    check_boundary(cx, TrimEdge::Left);
}
fn check_boundary(cx: &mut TestAppContext, edge: TrimEdge) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
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
    visual.simulate_mouse_move(
        point(
            down.x + px(if edge == TrimEdge::Right { -12.0 } else { 12.0 }),
            down.y,
        ),
        MouseButton::Left,
        shift,
    );
    visual.run_until_parked();
    // Ask for a 1/16 transform, deliberately outside existing 1/8 admission.
    let target = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let c = state.project.clip(clip).unwrap();
        let track = state.project.tracks()[0].id();
        let lane = v.drag_lanes[&track];
        let viewport = v.visible_viewport(state.timeline_frames);
        let frame = if edge == TrimEdge::Right {
            c.start() + 6000
        } else {
            c.start() + c.duration() - 6000
        };
        point(
            px(lane.left
                + ((frame - viewport.start) as f64 / viewport.length as f64 * f64::from(lane.width))
                    as f32),
            down.y,
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, shift);
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        let d = v
            .active_trim_drag
            .as_ref()
            .expect("real held Shift capture");
        assert!(d.stretch_modifier.get());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        d.preview.borrow().as_ref().unwrap().clip.clone()
    });
    visual.simulate_mouse_up(target, MouseButton::Left, shift);
    visual.run_until_parked();
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        eprintln!(
            "capability edge={edge:?} desired={}..{} actual={:?} error={:?} submitted={:?}",
            desired.start(),
            desired.start() + desired.duration(),
            p.clip(clip).map(|c| (c.start(), c.duration())),
            v.error,
            v.last_enqueued_edit
        );
        assert!(
            v.error.is_none(),
            "visible preview must be prepared-capable: {:?}",
            v.error
        );
        assert_eq!(p.clip(clip), Some(&desired));
        assert!(
            desired.duration() >= 12000,
            "same prepared 1/8 bound, not a positive-only preview"
        );
        assert!(desired.duration() < 96000);
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
}
