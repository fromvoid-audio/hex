//! Native header-key routing through a disposable canonical worker/history.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_session::service::{Event, Service};
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
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            !view.edit_pending()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn header(visual: &mut VisualTestContext, track: TrackId, modifiers: Modifiers) {
    let b = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let p = gpui::point(b.right() - gpui::px(12.0), b.top() + gpui::px(1.0));
    visual.simulate_mouse_down(p, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(p, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
#[gpui::test]
fn header_backspace_removes_selected_tracks_and_restores_contents_in_one_undo(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let files = crate::editor::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("delete.wav", 48_000, 48_000);
    for multiple in [false, true] {
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        service.submit(Request::Import(wav.clone())).unwrap();
        let imported = next(&service);
        let audio = imported.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "midi".into(),
            }))
            .unwrap();
        let event = next(&service);
        let midi = event.state.project.tracks()[1].id();
        let before = event.state.project.snapshot();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline delete test".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(event.state);
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        header(&mut visual, audio, Modifiers::default());
        if multiple {
            header(
                &mut visual,
                midi,
                Modifiers {
                    platform: true,
                    ..Default::default()
                },
            );
        }
        visual.simulate_keystrokes("backspace");
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            assert!(
                p.track(audio).is_none(),
                "header Backspace removes the selected track"
            );
            assert_eq!(p.track(midi).is_none(), multiple);
            assert!(view.selected_track_ids().is_empty());
            p.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                before,
                "one Undo restores every removed track and its contents"
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}
