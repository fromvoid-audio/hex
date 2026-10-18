//! Actual multi-selection/header drag → worker candidate → one history step.
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{ClipId, Command, Created};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(10);
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
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    Vec<ClipId>,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("group.wav", 48_000, 96_000);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let source = &event.state.project.tracks()[0];
    let track = source.id();
    let original = source.clips()[0].id();
    let asset = source.clips()[0].asset();
    service
        .submit(Request::Edit(Command::RemoveClip(original)))
        .unwrap();
    next(&service);
    let mut clips = Vec::new();
    for i in 0..3 {
        service
            .submit(Request::Edit(Command::PlaceClip {
                track,
                asset,
                start: 48_000 + i * 48_000,
                duration: 24_000,
                source_offset: i * 24_000,
                source_length: 24_000,
                looping: false,
            }))
            .unwrap();
        let event = next(&service);
        let Created::Clip(clip) = event.created.unwrap() else {
            panic!("placed clip");
        };
        clips.push(clip);
    }
    // Consume the current worker state without making a synthetic project view.
    service
        .submit(Request::Edit(Command::Gain { track, gain: 1.0 }))
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
    (files, handle, editor, clips)
}
fn click(visual: &mut VisualTestContext, clip: ClipId, additive: bool) {
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let center = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(
        center,
        Modifiers {
            platform: additive,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
}
#[gpui::test]
fn dragging_nonprimary_selected_header_moves_all_three_clips_with_gaps_and_one_undo(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (i, clip) in clips.iter().enumerate() {
        click(&mut visual, *clip, i != 0);
    }
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip, clips.last().copied());
        assert_eq!(v.selected_clip_ids(), clips);
        v.state.as_ref().unwrap().project.snapshot()
    });
    let anchor = clips[1];
    let selector = Box::leak(format!("clip-drag-surface-{}", anchor.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(down.x + px(63.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.active_clip_drag.as_ref().unwrap().clip, anchor);
        assert_eq!(
            v.selected_clip_ids(),
            clips,
            "dragging a member must retain all selected IDs"
        );
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let moved_anchor = p.clip(anchor).unwrap().start();
        let delta = i128::from(moved_anchor) - 96_000;
        assert!(delta > 0, "actual dragged member moved");
        for (i, id) in clips.iter().enumerate() {
            let c = p
                .clip(*id)
                .expect("every selected ID survives group movement");
            let expected = 48_000 + i as i128 * 48_000 + delta;
            eprintln!(
                "group anchor={} clip={} delta={delta} actual={} expected={expected}",
                anchor.value(),
                id.value(),
                c.start()
            );
            assert_eq!(
                i128::from(c.start()),
                expected,
                "all selected clips use the anchor delta"
            );
            assert_eq!(c.source_offset(), i as u64 * 24_000);
            assert_eq!(c.source_length(), 24_000);
        }
        assert_eq!(v.selected_clip_ids(), clips);
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
