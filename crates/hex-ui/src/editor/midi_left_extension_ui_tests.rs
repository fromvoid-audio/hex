//! Fresh canonical MIDI creation → real left handle → preview/worker/history.
use super::super::{
    EditorView, musical_time::SnapGrid, test_environment::Environment, viewport::Viewport,
};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, Command};
use hex_session::service::{Event, Request, Service};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

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
#[gpui::test]
fn fresh_midi_left_extension_preserves_right_edge_notes_and_real_worker_history(
    cx: &mut TestAppContext,
) {
    cx.update(super::super::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "fresh left resize".into(),
        }))
        .unwrap();
    let event = next(&service);
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let event = next(&service);
    let clip = event.state.project.track(track).unwrap().midi_clips()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::ONE,
            pitch: 64,
            channel: 2,
            velocity: 97,
        }))
        .unwrap();
    let event = next(&service);
    let original = event.state.project.midi_clip(clip).unwrap().clone();
    assert_eq!(original.source_start(), BeatTime::ZERO);
    assert_eq!(original.source_length(), original.length());
    let before = event.state.project.snapshot();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&environment),
                )
            });
            editor.update(cx, |v, cx| {
                v.receive(event);
                v.viewport = Some(Viewport {
                    start: 0,
                    length: 384_000,
                });
                v.snap_grid = SnapGrid::Off;
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(Arc::ptr_eq(&environment, &v.test_environment))
    });
    let selector = Box::leak(format!("midi-clip-trim-left-{}", clip.value()).into_boxed_str());
    let down = visual
        .debug_bounds(selector)
        .expect("fresh clip left handle")
        .center();
    let target = point(down.x - px(90.0), down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x - px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let desired = editor.read_with(&visual, |v, _| {
        let drag = v.active_midi_trim_drag.as_ref().expect("actual left-handle capture");
        let preview = drag.preview.borrow().expect("canonical metadata candidate");
        eprintln!("fresh midi original={:?}..{:?} source={:?}, down={down:?} target={target:?}, preview={:?}..{:?}",
            original.start(), original.start().raw()+original.length().raw(), original.source_start(), preview.start(), preview.start().raw()+preview.length().raw());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(preview.start() < original.start(), "fresh MIDI left edge must expose earlier blank time");
        assert_eq!(preview.start().raw()+preview.length().raw(), original.start().raw()+original.length().raw());
        preview
    });
    let held = visual.debug_bounds("midi-trim-live-preview").unwrap();
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("midi-trim-live-preview"), Some(held));
    editor.read_with(&visual, |v, _| {
        assert!(v.active_midi_trim_drag.is_none());
        assert!(v.pending_trim.is_some());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.pending_trim.is_none());
        assert!(v.error.is_none(), "{:?}", v.error);
        let project = &v.state.as_ref().unwrap().project;
        let actual = project.midi_clip(clip).unwrap();
        assert_eq!(
            (actual.start(), actual.length()),
            (desired.start(), desired.length())
        );
        let note = &actual.notes()[0];
        assert_eq!(note.id(), original.notes()[0].id());
        assert_eq!(
            actual.start().raw() + note.start().raw() - actual.source_start().raw(),
            original.start().raw() + original.notes()[0].start().raw()
                - original.source_start().raw()
        );
        assert_eq!(
            (
                note.pitch(),
                note.channel(),
                note.velocity(),
                note.duration()
            ),
            (64, 2, 97, BeatTime::ONE)
        );
        project.snapshot()
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
