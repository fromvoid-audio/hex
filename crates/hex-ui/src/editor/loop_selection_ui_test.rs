//! Real worker + rendered shortcut: bounds, selection ownership, and canonical history.
use super::{EditorMode, EditorView, time_selection::TimeSelection};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipId, Command, Created};
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
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(cx: &mut TestAppContext) -> (gpui::AnyWindowHandle, Entity<EditorView>, ClipId, ClipId) {
    cx.update(super::bind_editor_keys);
    let files = super::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("loop.wav", 48_000, 48_000);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav)).unwrap();
    let event = next(&service);
    let audio = event.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "midi".into(),
        }))
        .unwrap();
    let event = next(&service);
    let midi_track = event.state.project.tracks()[1].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::from_raw(3 << 32),
            length: BeatTime::from_raw(2 << 32),
        }))
        .unwrap();
    let event = next(&service);
    let Created::Clip(midi) = event.created.unwrap() else {
        panic!("midi")
    };
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline loop test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(event.state);
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, audio, midi)
}
fn click(visual: &mut VisualTestContext, clip: ClipId, additive: bool) {
    let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let bounds = visual.debug_bounds(selector).expect("rendered header");
    let modifiers = Modifiers {
        platform: additive,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(bounds.center(), MouseButton::Left, modifiers);
    visual.simulate_mouse_up(bounds.center(), MouseButton::Left, modifiers);
    visual.run_until_parked();
}
#[gpui::test]
fn l_sets_audio_midi_and_multi_clip_bounds_preserving_contents_selection_and_history(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, midi) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (first, second, start, end) in [
        (audio, None, 0, 48_000),
        (midi, None, 72_000, 120_000),
        (audio, Some(midi), 0, 120_000),
    ] {
        click(&mut visual, first, false);
        if let Some(second) = second {
            click(&mut visual, second, true);
        }
        let (before, selection) = editor.read_with(&visual, |v, _| {
            (
                v.state.as_ref().unwrap().project.snapshot(),
                v.selected_clip_ids(),
            )
        });
        visual.simulate_keystrokes("l");
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let region = p.loop_region().expect("L creates selected loop");
            assert_eq!(
                (region.start(), region.end(), region.enabled()),
                (start, end, true)
            );
            let mut expected = before.clone();
            expected.loop_region = p.loop_region();
            assert_eq!(p.snapshot(), expected, "contents and IDs unchanged");
            assert_eq!(v.selected_clip_ids(), selection);
            p.snapshot()
        });
        visual.simulate_keystrokes("l");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                after,
                "reassert, never toggle"
            )
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        // A redundant canonical edit is a no-op in history.
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}
#[gpui::test]
fn l_time_bounds_win_including_reverse_and_main_proxy_with_stale_refusal(cx: &mut TestAppContext) {
    let (handle, editor, audio, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    for project_scope in [false, true] {
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 36_000,
                end_frame: 12_000,
            });
            v.time_selection_project_scope = project_scope;
        });
        visual.simulate_keystrokes("l");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let r = v.state.as_ref().unwrap().project.loop_region().unwrap();
            assert_eq!((r.start(), r.end()), (12_000, 36_000));
            assert_eq!(v.time_selection.unwrap().anchor_frame, 36_000);
        });
    }
    // A non-empty stale rectangle must not fall back to the valid selected audio.
    editor.update(&mut visual, |v, _| {
        v.time_selection_project_scope = false;
        let mut history =
            hex_project::Editor::new(v.state.as_ref().unwrap().project.as_ref().clone(), 4)
                .unwrap();
        let Created::Track(stale) = history
            .execute(Command::AddMidiTrack {
                name: "absent".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        v.time_selection.as_mut().unwrap().end_track = stale;
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, before);
        assert!(v.error.is_some());
    });
}

#[gpui::test]
fn l_focus_empty_stale_pending_and_musical_keyboard_guards(cx: &mut TestAppContext) {
    let (handle, editor, audio, midi) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // No selection must not create a loop or toggle an existing one.
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.state.as_ref().unwrap().project.loop_region().is_none())
    });
    click(&mut visual, audio, false);
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    // Browser search owns its input and enclosing browser key context.
    visual
        .update(|window, cx| editor.read_with(cx, |v, _| v.browser.search.read(cx).focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    // Actual numeric widget owns typing; Escape cancels its draft.
    let value = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    visual.simulate_keystrokes("escape");
    // Header rename is the canonical inline input, not a synthetic root-focus flag.
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let header_point = gpui::point(
        header.right() - gpui::px(12.0),
        header.top() + gpui::px(1.0),
    );
    visual.simulate_mouse_down(header_point, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(header_point, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.track_name.is_editing()));
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    visual.simulate_keystrokes("escape");
    click(&mut visual, midi, false);
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.focus.focus(window);
            v.computer_keyboard.enabled = true;
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "L remains a note"
        )
    });
    editor.update(&mut visual, |v, _| {
        v.computer_keyboard.enabled = false;
        v.worker_requests_pending = 1;
    });
    visual.simulate_keystrokes("l");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    editor.update(&mut visual, |v, _| {
        v.worker_requests_pending = 0;
        v.selected_clip = None;
        v.time_selection = None;
    });
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    // Session retains the existing global loop-toggle action.
    editor.update(&mut visual, |v, cx| {
        v.mode = EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            !v.state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        )
    });
}
