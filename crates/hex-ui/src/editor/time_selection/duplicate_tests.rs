//! Real worker and rendered shortcut regressions; no user project/audio output.
use super::*;
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Created};
use hex_session::service::{Event, Service};
use std::time::Duration;

fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
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
        assert!(
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[gpui::test]
fn command_d_duplicates_bounded_audio_midi_gaps_and_advances_scope_atomically(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let files = crate::editor::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("duplicate.wav", 48_000, 48_000);
    for scope in 0..3 {
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        service.submit(Request::Import(wav.clone())).unwrap();
        let audio_event = next(&service);
        let audio = audio_event.state.project.tracks()[0].id();
        let original_audio = audio_event.state.project.tracks()[0].clips()[0].clone();
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "midi".into(),
            }))
            .unwrap();
        let added = next(&service);
        let midi = added.state.project.tracks()[1].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: midi,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(4 << 32),
            }))
            .unwrap();
        let placed = next(&service);
        let Created::Clip(clip) = placed.created.unwrap() else {
            panic!("midi clip")
        };
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(2 << 32),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .unwrap();
        let event = next(&service);
        let before = event.state.project.snapshot();
        let original_midi = event.state.project.track(midi).unwrap().clone();
        let handle = *cx.add_window(|_, _| gpui::Empty);
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(window, cx, service, Err("offline duplicate test".into()))
                });
                editor.update(cx, |view, cx| {
                    view.state = Some(event.state);
                    view.selected_clip = Some(clip); // Time range must beat whole MIDI duplication.
                    view.time_selection = Some(TimeSelection {
                        anchor_track: Some(audio),
                        end_track: Some(if scope == 0 { audio } else { midi }),
                        anchor_frame: 12_000,
                        end_frame: 72_000,
                    });
                    view.time_selection_project_scope = scope == 2;
                    view.edit_cursor = Some(crate::editor::edit_cursor::EditCursor {
                        track: Some(audio),
                        frame: 12_000,
                        generation: view.project_generation,
                        pulse_color: 0,
                        pulse_started: Instant::now(),
                    });
                    view.focus.focus(window);
                    cx.notify();
                });
                editor
            })
            .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            let copied = p
                .track(audio)
                .unwrap()
                .clips()
                .iter()
                .find(|c| c.start() == 72_000)
                .expect("bounded audio copy");
            assert_eq!(copied.duration(), 36_000);
            assert_eq!(copied.source_offset(), 12_000);
            assert_eq!(copied.asset(), original_audio.asset());
            assert_ne!(copied.id(), original_audio.id());
            assert_eq!(
                p.track(audio)
                    .unwrap()
                    .clips()
                    .iter()
                    .find(|c| c.id() == original_audio.id())
                    .unwrap(),
                &original_audio
            );
            if scope == 0 {
                assert_eq!(p.track(midi).unwrap(), &original_midi);
            } else {
                let fragment = p
                    .track(midi)
                    .unwrap()
                    .midi_clips()
                    .iter()
                    .find(|c| p.tempo_map().beat_to_frame(c.start()).unwrap() == 72_000)
                    .expect("partial MIDI copy");
                assert_ne!(fragment.id(), clip);
                assert_eq!(
                    p.tempo_map().beat_to_frame(fragment.length()).unwrap(),
                    60_000
                );
                assert_eq!(fragment.notes().len(), 1);
                assert_eq!(fragment.notes()[0].start(), BeatTime::ZERO);
                assert_eq!(
                    p.tempo_map()
                        .beat_to_frame(fragment.notes()[0].duration())
                        .unwrap(),
                    36_000
                );
                assert_eq!(fragment.notes()[0].pitch(), 60);
                assert_ne!(
                    fragment.notes()[0].id(),
                    original_midi.midi_clips()[0].notes()[0].id()
                );
            }
            let range = view.normalized_time_selection().unwrap();
            assert_eq!((range.start(), range.end()), (72_000, 132_000));
            assert_eq!(view.time_selection_project_scope, scope == 2);
            p.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let mut expected = before.clone();
            expected.next_id = after.next_id;
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                expected,
                "single Undo restores all source/destination rows"
            );
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
        });
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let range = view.normalized_time_selection().unwrap();
            assert_eq!((range.start(), range.end()), (132_000, 192_000));
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .track(audio)
                    .unwrap()
                    .clips()
                    .iter()
                    .any(|c| c.start() == 132_000 && c.duration() == 36_000)
            );
        });
        visual.update(|window, cx| {
            editor.read_with(cx, |view, _| view.browser.search.read(cx).focus(window))
        });
        visual.run_until_parked();
        let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().revision,
                revision,
                "browser text owns focus"
            )
        });
    }
}

#[gpui::test]
fn command_d_refuses_overflow_and_pending_time_edits(cx: &mut TestAppContext) {
    cx.update(crate::editor::bind_editor_keys);
    let (project, audio, _, _clip) = crate::editor::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, std::sync::Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.run_until_parked();
    for pending in [false, true] {
        visual.update(|window, cx| {
            editor.update(cx, |view, _| {
                view.time_selection = Some(TimeSelection {
                    anchor_track: Some(audio),
                    end_track: Some(audio),
                    anchor_frame: if pending { 12_000 } else { u64::MAX - 1 },
                    end_frame: if pending { 24_000 } else { u64::MAX },
                });
                view.worker_requests_pending = usize::from(pending);
                assert!(view.duplicate_time_selection(window));
                assert!(view.last_enqueued_edit.is_none());
                assert!(view.pending_time_range_move.is_none());
                assert!(view.error.is_some());
                view.worker_requests_pending = 0;
            })
        });
    }
}
