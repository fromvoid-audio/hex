//! Real bounded GPUI pointer routing and worker/history, without an OS backend.
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, ClipId, Command, Created, MidiOneShotBoundary};
use hex_session::service::{Event, Request, Service};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-loop-pointer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("samples")).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
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
            assert!(v.error.is_none(), "{:?}", v.error);
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(cx: &mut TestAppContext) -> (Files, gpui::AnyWindowHandle, Entity<EditorView>, ClipId) {
    cx.update(super::super::bind_editor_keys);
    let files = Files::new(); // All browser/preferences roots exist BEFORE workers start.
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "midi".into(),
        }))
        .unwrap();
    let event = next(&service);
    let Some(Created::Track(track)) = event.created else {
        panic!("track")
    };
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: beat(1),
            length: beat(8),
        }))
        .unwrap();
    let event = next(&service);
    let Some(Created::Clip(clip)) = event.created else {
        panic!("clip")
    };
    for (raw, pitch) in [(9_u64 << 30, 60), (6_u64 << 32, 61)] {
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_raw(raw),
                duration: BeatTime::from_raw(1 << 30),
                pitch,
                channel: 0,
                velocity: 100,
            }))
            .unwrap();
        next(&service);
    }
    service
        .submit(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(2),
            loop_start: beat(2),
            loop_end: beat(4),
            looping: false,
            length: beat(2),
        }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, &files.0)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, files.0.join("samples"));
                assert_eq!(v.browser.places, vec![files.0.join("samples")]);
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
fn midi_loop_corner_pointer_repeats_crop_commits_one_undo(cx: &mut TestAppContext) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let original_width = visual
        .debug_bounds(Box::leak(
            format!("midi-clip-visual-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap()
        .size
        .width;
    let down = visual
        .debug_bounds(Box::leak(
            format!("midi-clip-loop-outer-{}", clip.value()).into_boxed_str(),
        ))
        .expect("actual bottom-right loop handle")
        .center();
    println!("loop down={down:?} original_width={original_width:?}");
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        println!(
            "after down pointer={} selection={} loopcapture={}",
            v.pointer_down.is_some(),
            v.time_selection_gesture.is_some(),
            v.active_midi_trim_drag.is_some()
        )
    });
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.update(|_, cx| println!("after threshold gpui_drag={}", cx.has_active_drag()));
    editor.read_with(&visual, |v, _| {
        println!(
            "after threshold pointer={} selection={} loopcapture={}",
            v.pointer_down.is_some(),
            v.time_selection_gesture.is_some(),
            v.active_midi_trim_drag.is_some()
        )
    });
    let target = point(down.x + original_width * 2.0, down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        let drag = v
            .active_midi_trim_drag
            .as_ref()
            .expect("owned MIDI loop capture");
        let p = drag.preview.borrow().unwrap();
        assert!(p.looping());
        assert_eq!(
            (p.source_start(), p.loop_start(), p.loop_end()),
            (beat(2), beat(2), beat(4))
        );
        assert!(p.length().raw() > beat(5).raw());
        p
    });
    let preview_bounds = visual
        .debug_bounds("midi-trim-live-preview")
        .expect("actual held loop geometry");
    assert!(preview_bounds.size.width > original_width * 2.5);
    assert!(
        visual.debug_bounds("midi-trim-preview-note-61").is_none(),
        "hidden authored note stays outside the captured cycle"
    );
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let committed = editor.read_with(&visual, |v, _| {
        assert!(v.active_midi_trim_drag.is_none());
        let project = &v.state.as_ref().unwrap().project;
        let c = project.midi_clip(clip).unwrap();
        assert_eq!(c.source().one_shot_boundary(), MidiOneShotBoundary::LoopEnd);
        assert_eq!(
            c.source_playback().unwrap(),
            desired.source_playback().unwrap()
        );
        assert_eq!(
            c.notes().len(),
            2,
            "hidden authored material stays retained"
        );
        project.snapshot()
    });
    assert_ne!(committed, before);
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
        before
    );
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
        committed
    );
    visual.update(|window, _| window.remove_window());
}

#[gpui::test]
fn midi_loop_first_marker_pointer_keeps_outer_duration_and_undo(cx: &mut TestAppContext) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, _| {
        let command = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_clip(clip)
            .unwrap()
            .loop_outer_resize_command(beat(7))
            .unwrap();
        assert!(v.enqueue(Request::Edit(command)));
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let marker = visual
        .debug_bounds(Box::leak(
            format!("midi-clip-loop-boundary-{}", clip.value()).into_boxed_str(),
        ))
        .expect("actual first-repeat marker");
    let down = marker.center();
    let body = visual
        .debug_bounds(Box::leak(
            format!("midi-clip-visual-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    println!("marker={marker:?} clip={body:?}");
    assert!(
        (f32::from(marker.center().x - body.origin.x) - f32::from(body.size.width) / 3.0).abs()
            < 0.01,
        "actual marker is at the first projected repeat boundary"
    );
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(down.x + body.size.width / 12.0, down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        let p = v
            .active_midi_trim_drag
            .as_ref()
            .expect("marker capture")
            .preview
            .borrow()
            .unwrap();
        assert_eq!(p.length(), beat(6));
        assert_ne!(p.loop_end(), beat(4));
        assert_eq!(p.source_start(), beat(2));
        p
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
        assert_eq!(c.length(), beat(6));
        assert_eq!(
            c.source_playback().unwrap(),
            desired.source_playback().unwrap()
        );
        assert_eq!(c.notes().len(), 2);
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    assert_eq!(
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
        before
    );
    visual.update(|window, _| window.remove_window());
}

#[gpui::test]
fn midi_loop_corner_stationary_snap_modifier_and_escape_preserve_project(cx: &mut TestAppContext) {
    let (_files, handle, editor, clip) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::default();
        cx.notify();
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let bounds = visual
        .debug_bounds(Box::leak(
            format!("midi-clip-loop-outer-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let clip_bounds = visual
        .debug_bounds(Box::leak(
            format!("midi-clip-visual-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    println!("corner={bounds:?} clip={clip_bounds:?}");
    assert!(bounds.origin.x >= clip_bounds.origin.x);
    assert!(
        bounds.origin.y + bounds.size.height
            <= clip_bounds.origin.y + clip_bounds.size.height - px(1.0)
    );
    for dx in [0.25, 7.75] {
        for dy in [0.25, 7.75] {
            editor.read_with(&visual, |v, _| {
                assert!(
                    v.pointer_hits_midi_loop_control(
                        f32::from(bounds.origin.x) + dx,
                        f32::from(bounds.origin.y) + dy
                    ),
                    "all actual loop-handle corners reserve capture"
                )
            });
        }
    }
    let down = bounds.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.active_midi_trim_drag.is_none());
        assert!(v.pointer_down.is_none());
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(down.x + clip_bounds.size.width * 1.37, down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let snapped = editor.read_with(&visual, |v, _| {
        v.active_midi_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow()
            .unwrap()
    });
    visual.simulate_modifiers_change(Modifiers {
        platform: true,
        ..Modifiers::default()
    });
    let free = editor.read_with(&visual, |v, _| {
        v.active_midi_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow()
            .unwrap()
    });
    assert_ne!(
        snapped.length(),
        free.length(),
        "modifier refreshes the stationary loop preview"
    );
    visual.simulate_modifiers_change(Modifiers::default());
    assert_eq!(
        editor.read_with(&visual, |v, _| v
            .active_midi_trim_drag
            .as_ref()
            .unwrap()
            .preview
            .borrow()
            .unwrap()),
        snapped
    );
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.active_midi_trim_drag.is_none());
        assert!(v.pending_trim.is_none());
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    // A new gesture uses the actual mouse-up modifier even when the pointer
    // stays still after the previous free preview.
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_modifiers_change(Modifiers {
        platform: true,
        ..Modifiers::default()
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let c = v.state.as_ref().unwrap().project.midi_clip(clip).unwrap();
        assert_eq!(
            c.source_playback().unwrap(),
            snapped.source_playback().unwrap(),
            "release recomputes the current snap modifier"
        );
        assert!(v.pending_trim.is_none());
    });
    visual.update(|window, _| window.remove_window());
}
