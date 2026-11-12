//! Real worker + rendered shortcut: bounds, selection ownership, and canonical history.
use super::{EditorMode, EditorView, time_selection::TimeSelection};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipId, Command, Created};
use hex_session::service::{Event, Request, Service};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

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
pub(super) fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
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
        assert!(
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn fixture(
    cx: &mut TestAppContext,
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
    ClipId,
    Arc<super::test_environment::Environment>,
) {
    fixture_with_bounce_media(cx, false)
}
pub(super) fn fixture_with_bounce_media(
    cx: &mut TestAppContext,
    bounce: bool,
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    ClipId,
    ClipId,
    Arc<super::test_environment::Environment>,
) {
    cx.update(super::bind_editor_keys);
    let files = super::test_environment::Environment::new().unwrap();
    let directory = files.root().to_owned();
    std::fs::create_dir_all(directory.join("samples")).unwrap();
    let wav = directory.join("samples/loop.wav");
    let count = 48_000_u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + count * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&96_000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(count * 2).to_le_bytes());
    for _ in 0..count {
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
    }
    std::fs::write(&wav, bytes).unwrap();
    let service = if bounce {
        Service::with_recovery(
            48_000,
            32,
            8_000_000,
            directory.join("recovery/untitled.hex-journal"),
        )
    } else {
        Service::new(48_000, 32, 8_000_000)
    }
    .unwrap();
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
                EditorView::new_for_test_with_environment(window, cx, service, Arc::clone(&files))
            });
            editor.update(cx, |view, cx| {
                assert!(Arc::ptr_eq(&files, &view.test_environment));
                for (path, name) in [
                    (view.browser.test_preferences_path(), "browser.json"),
                    (view.recent_projects.test_path(), "recent-projects.json"),
                    (view.theme_settings.test_path(), "theme.json"),
                    (view.project_directory.test_path(), "project-directory.json"),
                ] {
                    assert_eq!(path, directory.join("config").join(name));
                }
                assert_eq!(view.browser.directory, directory.join("samples"));
                assert_eq!(view.browser.places, vec![directory.join("samples")]);
                // These interaction fixtures declare their visible frame span.
                // Default/retained navigation bounds have their own regressions.
                view.viewport = Some(super::viewport::Viewport::fit(event.state.timeline_frames));
                view.state = Some(event.state);
                view.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, audio, midi, files)
}
pub(super) fn click(visual: &mut VisualTestContext, clip: ClipId, additive: bool) {
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
fn loop_keys_toggle_matching_or_absent_selection_and_compare_exact_frames(cx: &mut TestAppContext) {
    for key in ["l", "w"] {
        let cx = &mut cx.new_app();
        let (handle, editor, audio, _, files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let region = |visual: &VisualTestContext| {
            editor.read_with(visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .loop_region()
                    .map(|r| (r.start(), r.end(), r.enabled()))
            })
        };
        let initial = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        assert_eq!(region(&visual), None, "no range to invent: {key}");
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().revision, initial)
        });

        click(&mut visual, audio, false);
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        assert_eq!(region(&visual), Some((0, 48_000, true)), "{key}");
        let enabled =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        // The canonical ACK has not been installed: another key cannot queue
        // a duplicate action or resolve against the old enabled state.
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.worker_requests_pending, 1, "{key}");
            assert!(v.pending_requests.is_empty());
        });
        pump(&editor, &mut visual);
        assert_eq!(region(&visual), Some((0, 48_000, false)), "{key}");
        let disabled =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let mut expected = enabled.clone();
        expected.loop_region = disabled.loop_region;
        assert_eq!(disabled, expected, "toggle preserves music: {key}");
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), enabled)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), disabled)
        });
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        assert_eq!(region(&visual), Some((0, 48_000, true)), "{key}");

        // A different selection kind with equal canonical bounds still toggles.
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(TimeSelection {
                anchor_track: Some(track),
                end_track: Some(track),
                anchor_frame: 0,
                end_frame: 48_000,
            });
            v.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(track),
                frame: 0,
                generation: v.project_generation,
                pulse_color: 0,
                pulse_started: Instant::now(),
            });
        });
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        assert_eq!(region(&visual), Some((0, 48_000, false)), "{key}");
        // One frame of difference enables a new range, even with a remembered
        // disabled loop and visually indistinguishable rounded positions.
        editor.update(&mut visual, |v, _| {
            v.time_selection.as_mut().unwrap().anchor_frame = 1;
            v.edit_cursor.as_mut().unwrap().frame = 1;
            let range = v.normalized_time_selection().unwrap();
            assert_eq!((range.start(), range.end()), (1, 48_000));
        });
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        editor.read_with(&visual, |v, _| {
            assert!(
                matches!(
                    v.last_enqueued_edit,
                    Some(Command::SetLoopRegion {
                        start: 1,
                        end: 48_000,
                        enabled: true
                    })
                ),
                "exact selection command: {:?}, selection: {:?}, error: {:?}",
                v.last_enqueued_edit,
                v.time_selection,
                v.error
            );
        });
        pump(&editor, &mut visual);
        assert_eq!(region(&visual), Some((1, 48_000, true)), "{key}");
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| {
                let track = v.selected_track.unwrap();
                v.time_selection = None;
                v.edit_cursor = None;
                v.select_track(track, window);
                cx.notify();
            });
        });
        visual.run_until_parked();
        for enabled in [false, true] {
            crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
            pump(&editor, &mut visual);
            assert_eq!(region(&visual), Some((1, 48_000, enabled)), "{key}");
            editor.read_with(&visual, |v, _| {
                assert!(
                    v.selected_clip.is_none(),
                    "loop ACK preserves empty selection: {key}"
                )
            });
        }
        let path = files.root().join("remembered-loop.hex");
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::SaveNew(path.clone())))
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
        pump(&editor, &mut visual);
        visual.update(|window, cx| editor.read(cx).focus.focus(window));
        for enabled in [false, true] {
            editor.read_with(&visual, |v, _| {
                assert!(v.selected_track.is_none() && v.selected_clip.is_none());
            });
            crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
            pump(&editor, &mut visual);
            assert_eq!(
                region(&visual),
                Some((1, 48_000, enabled)),
                "saved range: {key}"
            );
            editor.read_with(&visual, |v, _| {
                assert!(
                    v.selected_track.is_none() && v.selected_clip.is_none(),
                    "saved unselected context survives loop ACK: {key}"
                );
            });
        }
    }
}

#[gpui::test]
fn loop_selection_keys_set_audio_midi_and_multi_clip_bounds_preserving_contents_selection_and_history(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    for (key, first, second, start, end) in [
        ("l", audio, None, 0, 48_000),
        ("w", midi, None, 72_000, 120_000),
        ("w", audio, Some(midi), 0, 120_000),
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
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let region = p.loop_region().expect("shortcut creates selected loop");
            assert_eq!(
                (region.start(), region.end(), region.enabled()),
                (start, end, true)
            );
            let mut expected = before.clone();
            expected.loop_region = p.loop_region();
            assert_eq!(p.snapshot(), expected, "contents and IDs unchanged");
            assert_eq!(v.selected_clip_ids(), selection);
            let snapshot = p.snapshot();
            assert_eq!(
                hex_project::Project::from_snapshot(snapshot.clone())
                    .unwrap()
                    .snapshot(),
                snapshot,
                "current-format round trip"
            );
            snapshot
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
}
#[gpui::test]
fn l_time_bounds_win_including_reverse_and_main_proxy_with_stale_refusal(cx: &mut TestAppContext) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    for project_scope in [false, true] {
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(TimeSelection {
                anchor_track: Some(track),
                end_track: Some(track),
                anchor_frame: 36_000,
                end_frame: 12_000,
            });
            v.time_selection_project_scope = project_scope;
            // The real body gesture owns a matching generation-scoped edit cursor.
            v.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(track),
                frame: 36_000,
                generation: v.project_generation,
                pulse_color: 0,
                pulse_started: Instant::now(),
            });
        });
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let r = v.state.as_ref().unwrap().project.loop_region().unwrap();
            assert_eq!((r.start(), r.end()), (12_000, 36_000));
            assert_eq!(v.playback_start_frame(), 12_000);
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
        v.time_selection.as_mut().unwrap().end_track = Some(stale);
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, before);
        assert!(v.error.is_some());
    });
}

#[gpui::test]
fn w_focus_empty_stale_pending_and_musical_keyboard_guards(cx: &mut TestAppContext) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    // No selection and no remembered range must not invent a loop.
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.state.as_ref().unwrap().project.loop_region().is_none())
    });
    click(&mut visual, audio, false);
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    // Browser search owns its input and enclosing browser key context.
    visual
        .update(|window, cx| editor.read_with(cx, |v, _| v.browser.search.read(cx).focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("cmd-shift-l");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.changelog.is_open()));
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
    visual.run_until_parked();
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    // Actual numeric widget owns typing; Escape cancels its draft.
    let value = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(value.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(value.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("w");
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
    visual.simulate_keystrokes("w");
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
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "W remains a note"
        )
    });
    editor.update(&mut visual, |v, _| {
        v.computer_keyboard.enabled = false;
        v.worker_requests_pending = 1;
    });
    visual.simulate_keystrokes("w");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    editor.update(&mut visual, |v, _| {
        v.worker_requests_pending = 0;
        v.selected_clip = None;
        v.time_selection = None;
    });
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision + 1);
        let region = v.state.as_ref().unwrap().project.loop_region().unwrap();
        assert_eq!(
            (region.start(), region.end(), region.enabled()),
            (0, 48_000, false)
        );
    });
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    editor.update(&mut visual, |v, _| {
        let mut history =
            hex_project::Editor::new(v.state.as_ref().unwrap().project.as_ref().clone(), 4)
                .unwrap();
        let Created::Clip(stale) = history.execute(Command::DuplicateClip(audio)).unwrap() else {
            panic!("clip")
        };
        v.selected_clip = Some(stale);
    });
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision);
        assert!(v.error.is_some());
    });
    // Session retains the existing global loop-toggle action.
    editor.update(&mut visual, |v, cx| {
        v.mode = EditorMode::Session;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("w");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        )
    });
}

#[gpui::test]
fn l_midi_end_uses_absolute_beat_endpoint_across_tempo_change(cx: &mut TestAppContext) {
    let (handle, editor, _, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetTempoPoint {
            beat: BeatTime::from_raw(4 << 32),
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })));
    });
    pump(&editor, &mut visual);
    click(&mut visual, midi, false);
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        let region = p.loop_region().unwrap();
        assert_eq!(
            (region.start(), region.end(), region.enabled()),
            (72_000, 144_000, true)
        );
        assert!(p.tracks().iter().any(|t| {
            t.midi_clips()
                .iter()
                .any(|c| c.id() == midi && c.length() == BeatTime::from_raw(2 << 32))
        }));
    });
}

#[gpui::test]
fn selection_loop_uses_all_three_clip_ids_and_nonprimary_outer_bounds_with_one_undo(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        assert!(v.enqueue(Request::Edit(Command::CopyClipTo {
            clip: audio,
            track,
            start: 144_000
        })));
    });
    pump(&editor, &mut visual);
    let extra = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0]
            .clips()
            .iter()
            .find(|c| c.start() == 144_000)
            .unwrap()
            .id()
    });
    for order in [
        [midi, extra, audio],
        [audio, midi, extra],
        [audio, extra, midi],
    ] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::ClearLoopRegion)));
        });
        pump(&editor, &mut visual);
        for (index, id) in order.into_iter().enumerate() {
            click(&mut visual, id, index != 0);
        }
        let (before, selection) = editor.read_with(&visual, |v, _| {
            let selection = v.selected_clip_ids();
            assert_eq!(selection.len(), 3);
            (v.state.as_ref().unwrap().project.snapshot(), selection)
        });
        // Plain selection-loop shortcut; Command-L has separate toggle coverage.
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let r = p.loop_region().expect("selection loop sets full selection");
            assert_eq!((r.start(), r.end(), r.enabled()), (0, 192_000, true));
            let mut expected = before.clone();
            expected.loop_region = p.loop_region();
            assert_eq!(p.snapshot(), expected);
            assert_eq!(v.selected_clip_ids(), selection);
            p.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "selection loop selection is one history edit"
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn selection_loop_time_selection_precedes_clip_and_protects_stale_and_owned_focus(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(&mut visual, audio, false);
    for project_scope in [false, true] {
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(TimeSelection {
                anchor_track: Some(track),
                end_track: Some(track),
                anchor_frame: 36_000,
                end_frame: 12_000,
            });
            v.time_selection_project_scope = project_scope;
            v.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(track),
                frame: 36_000,
                generation: v.project_generation,
                pulse_color: 0,
                pulse_started: Instant::now(),
            });
        });
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let r = v.state.as_ref().unwrap().project.loop_region().unwrap();
            assert_eq!(
                (r.start(), r.end(), r.enabled()),
                (12_000, 36_000, !project_scope)
            );
            assert_eq!(v.time_selection.unwrap().anchor_frame, 36_000);
            assert_eq!(v.time_selection_project_scope, project_scope);
        });
    }
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    // A stale non-empty time selection must not fall back to primary audio.
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
        v.time_selection.as_mut().unwrap().end_track = Some(stale);
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision);
        assert!(v.error.is_some());
    });
    editor.update(&mut visual, |v, _| {
        v.time_selection = None;
        let mut history =
            hex_project::Editor::new(v.state.as_ref().unwrap().project.as_ref().clone(), 4)
                .unwrap();
        let Created::Clip(stale) = history.execute(Command::DuplicateClip(audio)).unwrap() else {
            panic!("clip")
        };
        v.selected_clip = Some(stale);
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().revision, revision)
    });
    editor.update(&mut visual, |v, _| {
        v.time_selection = None;
        v.selected_clip = None;
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision + 1,
            "empty selection toggles the remembered loop"
        );
        let r = v.state.as_ref().unwrap().project.loop_region().unwrap();
        assert_eq!((r.start(), r.end(), r.enabled()), (12_000, 36_000, true));
    });
    let revision = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
    click(&mut visual, audio, false);
    editor.update(&mut visual, |v, _| {
        v.worker_requests_pending = 1;
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "pending edit refuses selection loop"
        );
        assert!(v.pending_requests.is_empty(), "no deferred selection write");
    });
    editor.update(&mut visual, |v, _| {
        v.worker_requests_pending = 0;
    });

    visual
        .update(|window, cx| editor.read_with(cx, |v, _| v.browser.search.read(cx).focus(window)));
    visual.run_until_parked();
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "browser input owns focus"
        )
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
    visual.run_until_parked();
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "MIDI child owns focus"
        )
    });
    let tempo = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_mouse_down(tempo.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(tempo.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "numeric input owns focus"
        )
    });
    visual.simulate_keystrokes("escape");
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let header = visual
        .debug_bounds(Box::leak(
            format!("arrangement-track-header-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let p = gpui::point(
        header.right() - gpui::px(12.0),
        header.top() + gpui::px(1.0),
    );
    visual.simulate_mouse_down(p, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(p, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("cmd-r");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.track_name.is_editing()));
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision,
            "rename input owns focus"
        )
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.mode = EditorMode::Session;
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state.as_ref().unwrap().revision,
            revision + 1,
            "plain l retains the Session loop toggle"
        )
    });
}

#[gpui::test]
fn loop_review_session_cell_selection_is_not_a_stale_arrangement_selection(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, midi, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "loop review".into()
        })));
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                source: midi,
                scene
            }))
        );
    });
    pump(&editor, &mut visual);
    let (session_id, track) = editor.read_with(&visual, |v, _| {
        let cell = &v.state.as_ref().unwrap().project.session_cells()[0];
        let hex_project::SessionCellContent::Midi(clip) = cell.content() else {
            panic!("session midi");
        };
        (clip.id(), cell.track())
    });
    // Use the same canonical selection entry point as a Session MIDI cell
    // callback, with its real worker-created ID, then the actual mode toolbar.
    editor.update(&mut visual, |v, cx| {
        v.select_session_midi(track, scene, session_id);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip, Some(session_id));
        assert_eq!(v.selected_cell, Some((track, scene)));
    });
    let position = visual
        .debug_bounds("workspace-view-toggle")
        .unwrap()
        .center();
    visual.simulate_click(position, Modifiers::default());
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for key in ["l", "w"] {
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(
                v.error.is_none(),
                "Session ownership is not a deleted Arrangement ID: {:?}",
                v.error
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert_eq!(v.selected_clip, Some(session_id));
        });
    }
}

#[gpui::test]
fn loop_review_retained_editor_focus_does_not_edit_during_browser_or_device_drag(
    cx: &mut TestAppContext,
) {
    use super::drag::{BrowserAudioDrag, NativeDeviceDrag, NativeDeviceDragSource};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    for native in [false, true] {
        let (handle, editor, audio, _, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        editor.update(&mut visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            if native {
                let entry = &hex_engine::native_device_catalog_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                )
                .unwrap()[0];
                let payload = NativeDeviceDrag {
                    project_generation: v.project_generation,
                    revision: state.revision,
                    source: NativeDeviceDragSource::Browser {
                        class: hex_engine::NativeDeviceClass::Effect,
                        type_id: entry.type_id,
                    },
                    destination: Rc::new(Cell::new(None)),
                    return_destination: Rc::new(Cell::new(None)),
                };
                v.begin_native_device_drag(&payload);
                assert!(v.native_device_drag_is_current(&payload));
            } else {
                let viewport = v.visible_viewport(state.timeline_frames);
                let payload = BrowserAudioDrag {
                    project_generation: v.project_generation,
                    revision: state.revision,
                    browser_generation: Some(v.browser.scan_generation()),
                    sample_rate: state.project.sample_rate(),
                    path: state
                        .project
                        .asset(state.project.clip(audio).unwrap().asset())
                        .unwrap()
                        .path()
                        .to_owned(),
                    label: "loop.wav".into(),
                    viewport_start: viewport.start,
                    viewport_length: viewport.length,
                    scroll: Default::default(),
                    snap_grid: v.snap_grid,
                    snap_modifier: Rc::new(Cell::new(false)),
                    copy_modifier: Rc::new(Cell::new(false)),
                    metadata: Rc::new(RefCell::new(None)),
                    metadata_error: Rc::new(RefCell::new(None)),
                    preview: Rc::new(Cell::new(None)),
                    destination: Rc::new(Cell::new(None)),
                    insertion_line_y: Rc::new(Cell::new(None)),
                    lane_hovered: Rc::new(Cell::new(false)),
                    last_pointer: Rc::new(Cell::new(None)),
                };
                v.begin_browser_audio_drag(&payload);
                assert!(v.browser_audio_drag_is_current(&payload));
            }
        });
        // Controlled retained-focus dispatch through actual platform keys. This is
        // not a claim that an ordinary browser row click retains editor focus.
        for key in ["l", "w"] {
            crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.worker_requests_pending, 0, "{native} / {key}");
                assert!(v.pending_requests.is_empty());
                assert_eq!(v.active_native_device_drag.is_some(), native);
                assert_eq!(v.active_browser_audio_drag.is_some(), !native);
                assert!(v.state.as_ref().unwrap().project.loop_region().is_none());
            });
        }
    }
}

#[gpui::test]
fn loop_review_held_middle_pan_refuses_selection_loop_until_release(cx: &mut TestAppContext) {
    for key in ["l", "w"] {
        let cx = &mut cx.new_app();
        let (handle, editor, audio, midi, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        click(&mut visual, midi, true);
        visual.update(|window, cx| {
            window.activate_window();
            editor.update(cx, |view, cx| {
                let horizon = view.state.as_ref().unwrap().timeline_frames;
                let fit = super::viewport::Viewport::fit(horizon);
                let zoom = fit.zoom(horizon, 0, true);
                assert!(zoom.length < fit.length);
                view.viewport = Some(zoom);
                view.focus.focus(window);
                cx.notify();
            });
        });
        visual.run_until_parked();
        let body = visual.debug_bounds("arrangement-content").unwrap();
        let start = body.origin + gpui::point(body.size.width * 0.65, gpui::px(30.0));
        let end = start - gpui::point(gpui::px(24.0), gpui::px(0.0));
        let initial = editor.read_with(&visual, |view, _| view.viewport);
        visual.simulate_mouse_down(start, MouseButton::Middle, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Middle, Modifiers::default());
        visual.run_until_parked();
        let (before, selection, viewport, owner) = editor.read_with(&visual, |view, _| {
            let pan = view.arrangement_pan.expect("real middle-button owner");
            assert_ne!(view.viewport, initial, "actual held pointer movement pans");
            assert!(!view.computer_keyboard.enabled);
            assert_eq!(view.selected_clip_ids().len(), 2);
            (
                view.state.as_ref().unwrap().clone(),
                view.selected_clip_ids(),
                view.viewport,
                (
                    pan.generation,
                    pan.revision,
                    pan.last_x,
                    pan.last_y,
                    pan.timeline_width,
                ),
            )
        });
        assert!(visual.update(|window, cx| editor.read(cx).focus.is_focused(window)));
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.worker_requests_pending, 0,
                "held {key} must not enqueue"
            );
            assert!(view.pending_requests.is_empty());
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let pan = view
                .arrangement_pan
                .expect("shortcut retains actual pan until release");
            assert_eq!(
                (
                    pan.generation,
                    pan.revision,
                    pan.last_x,
                    pan.last_y,
                    pan.timeline_width
                ),
                owner
            );
            assert_eq!(view.viewport, viewport);
            assert_eq!(view.selected_clip_ids(), selection);
            let state = view.state.as_ref().unwrap();
            assert_eq!(state.project.snapshot(), before.project.snapshot());
            assert_eq!(state.revision, before.revision);
            assert_eq!(
                (state.can_undo, state.can_redo),
                (before.can_undo, before.can_redo)
            );
        });
        visual.simulate_mouse_up(end, MouseButton::Middle, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| assert!(view.arrangement_pan.is_none()));
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let region = project
                .loop_region()
                .expect("same key works after pan release");
            assert_eq!(
                (region.start(), region.end(), region.enabled()),
                (0, 120_000, true)
            );
            let mut expected = before.project.snapshot();
            expected.loop_region = project.loop_region();
            assert_eq!(project.snapshot(), expected);
            assert_eq!(view.selected_clip_ids(), selection);
            project.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                before.project.snapshot()
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn loop_review_ruler_scrub_keeps_held_and_released_targets_until_refusal(cx: &mut TestAppContext) {
    for key in ["l", "w"] {
        let cx = &mut cx.new_app();
        let (handle, editor, _, midi, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, midi, false);
        // Existing offline scrub tests hold this real seek-ack gate. No audio
        // device or fake publication is constructed; actual ruler callbacks own
        // the scrub and coalesce the final target behind the gate.
        editor.update(&mut visual, |view, _| view.pending_seek_ticket = Some(1));
        let bounds = visual.debug_bounds("timeline-ruler").unwrap();
        let start = bounds.center();
        let end = start + gpui::point(gpui::px(10.0), gpui::px(0.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        editor.update(&mut visual, |view, _| {
            assert!(view.update_scrub(f32::from(end.x), false));
            assert!(view.scrub_active(), "actual held ruler owner");
            assert!(!view.tick_scrub(), "seek-ack gate holds the target");
        });
        assert!(visual.update(|window, cx| editor.read(cx).focus.is_focused(window)));
        let (before, selection, viewport) = editor.read_with(&visual, |view, _| {
            assert_eq!(view.selected_clip, Some(midi));
            (
                view.state.as_ref().unwrap().clone(),
                view.selected_clip_ids(),
                view.viewport,
            )
        });
        for released in [false, true] {
            if released {
                visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    assert!(
                        view.scrub_active(),
                        "released final target awaits admission"
                    );
                });
            }
            crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    view.worker_requests_pending, 0,
                    "{key} during scrub released={released} must not enqueue"
                );
                assert!(view.pending_requests.is_empty());
            });
            pump(&editor, &mut visual);
            editor.update(&mut visual, |view, _| {
                assert!(
                    view.scrub_active(),
                    "shortcut must retain actual ruler target"
                );
                assert!(
                    !view.tick_scrub(),
                    "same pending target remains behind seek ACK"
                );
                assert_eq!(view.pending_seek_ticket, Some(1));
                assert_eq!(view.viewport, viewport);
                assert_eq!(view.selected_clip_ids(), selection);
                let state = view.state.as_ref().unwrap();
                assert_eq!(state.project.snapshot(), before.project.snapshot());
                assert_eq!(state.revision, before.revision);
                assert_eq!(
                    (state.can_undo, state.can_redo),
                    (before.can_undo, before.can_redo)
                );
            });
        }
        editor.update(&mut visual, |view, _| {
            view.pending_seek_ticket = None;
            assert!(
                view.tick_scrub(),
                "retained released target reaches existing offline admission/refusal"
            );
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|e| e.contains("audio output"))
            );
            assert!(!view.scrub_active());
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                before.project.snapshot()
            );
        });
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, key);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            let region = project
                .loop_region()
                .expect("same shortcut works after scrub retirement");
            assert_eq!(
                (region.start(), region.end(), region.enabled()),
                (72_000, 120_000, true)
            );
            let mut expected = before.project.snapshot();
            expected.loop_region = project.loop_region();
            assert_eq!(project.snapshot(), expected);
            assert_eq!(view.selected_clip_ids(), selection);
            project.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                before.project.snapshot()
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

// Section focus follows the terminal of a real input-driven loop request.
fn section_loop_fixture(
    cx: &mut TestAppContext,
) -> (
    Entity<EditorView>,
    VisualTestContext,
    Vec<hex_project::SectionId>,
    Arc<super::test_environment::Environment>,
) {
    let (handle, editor, _, _, files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    for (start, end) in [(1, 2), (4, 5), (6, 7)] {
        editor.update(&mut visual, |v, _| {
            let revision = v.state.as_ref().unwrap().revision;
            assert!(v.enqueue(Request::EditArrangementSection {
                project_generation: v.project_generation,
                revision,
                command: Command::AddArrangementSection {
                    name: format!("section {start}"),
                    color: hex_project::ElementColor::Cyan,
                    start: BeatTime::from_quarters(start).unwrap(),
                    end: BeatTime::from_quarters(end).unwrap(),
                },
            }));
        });
        pump(&editor, &mut visual);
    }
    editor.update(&mut visual, |v, cx| {
        v.section_autoplay = false;
        v.computer_keyboard.enabled = false;
        v.viewport = Some(super::viewport::Viewport {
            start: 0,
            length: 192_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    let ids = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .arrangement_sections()
            .iter()
            .map(|s| s.id())
            .collect()
    });
    (editor, visual, ids, files)
}
fn choose_loop_section(visual: &mut VisualTestContext, index: usize, additive: bool) {
    let selector = Box::leak(format!("arrangement-section-header-{index}").into_boxed_str());
    let at = visual.debug_bounds(selector).unwrap().center();
    let modifiers = Modifiers {
        shift: additive,
        platform: additive,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
fn install_loop_terminal(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    event: Event,
) {
    editor.update(&mut *visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
}
#[gpui::test]
fn section_loop_focus_waits_for_own_ack_switches_sections_and_preserves_repeat_toggle(
    cx: &mut TestAppContext,
) {
    let (editor, mut visual, ids, _files) = section_loop_fixture(cx);
    let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    choose_loop_section(&mut visual, 1, false);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.sections.selected_ids(), &[ids[1]])
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    let own = editor.update(&mut visual, |v, _| next(&v.service));
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(
                !v.arrangement_focus.projection.enabled,
                "no optimistic focus before ACK"
            )
        });
    }
    install_loop_terminal(&editor, &mut visual, own);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert_eq!(&*v.arrangement_focus_ranges(), &[(96_000, 120_000)]);
        assert_eq!(
            v.playback_start_frame(),
            96_000,
            "section loop supplies the play start"
        );
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
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
        );
        assert!(v.arrangement_focus.projection.enabled);
    });
    choose_loop_section(&mut visual, 2, false);
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(&*v.arrangement_focus_ranges(), &[(144_000, 168_000)]);
        assert!(v.arrangement_focus.projection.enabled);
    });
    visual.simulate_keystrokes("q");
    editor.read_with(&visual, |v, _| {
        assert!(!v.arrangement_focus.projection.enabled)
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            !v.arrangement_focus.projection.enabled,
            "same-range toggle does not re-arm focus"
        )
    });
    choose_loop_section(&mut visual, 0, false);
    choose_loop_section(&mut visual, 2, true);
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.arrangement_focus.projection.enabled);
        assert_eq!(
            &*v.arrangement_focus_ranges(),
            &[(24_000, 48_000), (144_000, 168_000)]
        );
        let project = &v.state.as_ref().unwrap().project;
        let region = project.loop_region().unwrap();
        assert_eq!(
            (region.start(), region.end(), region.enabled()),
            (24_000, 168_000, true)
        );
        assert!(
            !v.arrangement_track_visible(project.tracks()[1].id()),
            "MIDI in the gap stays excluded"
        );
        assert_eq!(v.sections.selected_ids(), &[ids[0], ids[2]]);
        assert_eq!(project.tracks(), original.tracks());
        assert_eq!(
            project.arrangement_sections(),
            original.arrangement_sections()
        );
    });
}
#[gpui::test]
fn section_loop_focus_ignores_changed_selection_and_refused_or_replaced_terminals(
    cx: &mut TestAppContext,
) {
    for case in ["selection", "refusal", "replacement"] {
        let cx = &mut cx.new_app();
        let (editor, mut visual, _ids, _files) = section_loop_fixture(cx);
        choose_loop_section(&mut visual, 1, false);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
        let mut own = editor.update(&mut visual, |v, _| next(&v.service));
        match case {
            "selection" => {
                let track = editor.read_with(&visual, |v, _| {
                    v.state.as_ref().unwrap().project.tracks()[0].id()
                });
                // A later selection intent supersedes the admitted loop's view intent.
                visual.update(|window, cx| {
                    editor.update(cx, |v, cx| {
                        v.select_track(track, window);
                        cx.notify();
                    })
                });
                visual.run_until_parked();
                editor.read_with(&visual, |v, _| assert_eq!(v.selected_track, Some(track)));
            }
            // Protocol-negative injection at the receiving boundary; the positive
            // case above uses the genuine Service response unchanged.
            "refusal" => {
                own.error = Some("refused test loop".into());
                own.state = before;
                own.playback = None;
            }
            "replacement" => {
                own.project_replaced = true;
            }
            _ => unreachable!(),
        }
        install_loop_terminal(&editor, &mut visual, own);
        editor.read_with(&visual, |v, _| {
            assert!(!v.arrangement_focus.projection.enabled, "{case}")
        });
    }
}

#[gpui::test]
fn section_loop_focus_respects_held_foreground_automation_input(cx: &mut TestAppContext) {
    let (editor, mut visual, ids, _files) = section_loop_fixture(cx);
    choose_loop_section(&mut visual, 1, false);
    visual.simulate_keystrokes("a");
    visual.run_until_parked();
    let track = editor.read_with(&visual, |v, _| {
        assert!(v.automation_view.enabled);
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(&mut visual, |v, cx| {
        v.gain_lane.track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let selector =
        Box::leak(format!("arrangement-automation-plot-{}", track.value()).into_boxed_str());
    let plot = visual.debug_bounds(selector).unwrap();
    let down = gpui::point(
        plot.left() + plot.size.width * 0.25,
        plot.top() + plot.size.height * 0.4,
    );
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    editor.update(&mut visual, |v, _| v.last_enqueued_edit = None);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.gain_lane.pointer_is_active());
        assert_eq!(v.sections.selected_ids(), &[ids[1]]);
    });
    visual.simulate_keystrokes("l cmd-l");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.worker_requests_pending, 0,
            "held curve cannot submit a section loop/focus"
        );
        assert!(v.last_enqueued_edit.is_none());
        assert!(!v.arrangement_focus.projection.enabled);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
}

#[gpui::test]
fn command_l_toggles_active_loop_preserves_bounds_and_keeps_selection_play_origin(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 24_000,
            end: 48_000,
            enabled: false
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, cx| {
        let track = view.state.as_ref().unwrap().project.tracks()[0].id();
        view.time_selection = Some(TimeSelection {
            anchor_track: Some(track),
            end_track: Some(track),
            anchor_frame: 36_000,
            end_frame: 72_000,
        });
        view.edit_cursor = Some(super::edit_cursor::EditCursor {
            track: Some(track),
            frame: 96_000,
            generation: view.project_generation,
            pulse_color: 0,
            pulse_started: Instant::now(),
        });
        cx.notify();
    });
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let loop_region = view.state.as_ref().unwrap().project.loop_region().unwrap();
        assert_eq!(
            (
                loop_region.start(),
                loop_region.end(),
                loop_region.enabled()
            ),
            (24_000, 48_000, true)
        );
        assert_eq!(
            view.edit_cursor.unwrap().frame,
            96_000,
            "loop toggle does not rewrite the edit cursor"
        );
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    visual.simulate_keystrokes("space");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(36_000)),
            "an enabled loop does not override the time-selection start"
        )
    });
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let loop_region = view.state.as_ref().unwrap().project.loop_region().unwrap();
        assert_eq!(
            (
                loop_region.start(),
                loop_region.end(),
                loop_region.enabled()
            ),
            (24_000, 48_000, false)
        );
        assert_eq!(
            view.playback_start_frame(),
            36_000,
            "loop off keeps time-selection precedence"
        );
    });
    for enabled in [true, false] {
        let toggle = visual
            .debug_bounds("transport-loop-toggle")
            .unwrap()
            .center();
        visual.simulate_click(toggle, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .loop_region()
                    .unwrap()
                    .enabled(),
                enabled
            );
            assert_eq!(view.playback_start_frame(), 36_000);
        });
    }
    editor.update(&mut visual, |view, _| view.computer_keyboard.enabled = true);
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, revision)
    });
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        )
    });
    editor.update(&mut visual, |view, cx| {
        view.computer_keyboard.enabled = false;
        view.mode = EditorMode::Session;
        cx.notify();
    });
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        )
    });
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    let tempo = visual.debug_bounds("project-tempo-value").unwrap().center();
    visual.simulate_click(tempo, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("cmd-l");
    visual.update(|window, cx| window.dispatch_action(Box::new(super::ToggleLoop), cx));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.state.as_ref().unwrap().revision,
            revision,
            "numeric text owns the modified shortcut and direct action"
        )
    });
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    editor.update(&mut visual, |view, _| view.worker_requests_pending = 1);
    visual.simulate_keystrokes("cmd-l");
    editor.read_with(&visual, |view, _| assert!(view.pending_requests.is_empty()));
    editor.update(&mut visual, |view, _| {
        view.worker_requests_pending = 0;
        assert!(view.enqueue(Request::Edit(Command::ClearLoopRegion)));
    });
    pump(&editor, &mut visual);
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.state.as_ref().unwrap().project.loop_region().is_none());
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 72_000,
            end: 36_000,
            enabled: true
        })))
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_some());
        assert!(view.state.as_ref().unwrap().project.loop_region().is_none());
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
        assert_eq!(
            view.playback_start_frame(),
            36_000,
            "refused loop edits cannot publish a fake play origin"
        );
    });
}

#[gpui::test]
fn enabling_the_loop_retires_a_point_cursor_until_a_new_click(cx: &mut TestAppContext) {
    let (handle, editor, clip, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 24_000,
            end: 48_000,
            enabled: false
        })))
    });
    pump(&editor, &mut visual);
    let track = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let line = Box::leak(format!("edit-cursor-line-{}", track.value()).into_boxed_str());
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .expect("lane");
    let place = |visual: &mut VisualTestContext, fraction: f32| {
        let at = gpui::point(
            lane.origin.x + lane.size.width * fraction,
            lane.origin.y + gpui::px(22.0),
        );
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&*visual, |view, _| {
            let frame = view.edit_cursor.expect("click places cursor").frame;
            assert_eq!(
                view.playback_start_frame(),
                frame,
                "placed cursor owns play"
            );
            frame
        })
    };
    let first = place(&mut visual, 0.6);
    assert_ne!(first, 24_000);
    assert!(visual.debug_bounds(line).is_some());

    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        );
        assert!(view.edit_cursor.is_none(), "loop enable retires the cursor");
        assert_eq!(view.playback_start_frame(), 24_000);
    });
    assert!(
        visual.debug_bounds(line).is_none(),
        "retired cursor is hidden"
    );
    visual.simulate_keystrokes("space");
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.last_transport_command,
            Some(hex_engine::playback::PlaybackCommand::PlayFrom(24_000))
        )
    });
    visual.simulate_keystrokes("space");
    pump(&editor, &mut visual);

    let second = place(&mut visual, 0.8);
    assert!(visual.debug_bounds(line).is_some());
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(
            !view
                .state
                .as_ref()
                .unwrap()
                .project
                .loop_region()
                .unwrap()
                .enabled()
        );
        assert_eq!(
            view.playback_start_frame(),
            second,
            "disabling the loop keeps the new cursor"
        );
    });

    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::ClearLoopRegion)))
    });
    pump(&editor, &mut visual);
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.state.as_ref().unwrap().project.loop_region().is_none());
        assert_eq!(
            view.edit_cursor.map(|cursor| cursor.frame),
            Some(second),
            "a refused toggle keeps the cursor"
        );
    });

    // A cursor inside the selected clip outranks the clip start; once a loop
    // enable retires it, the loop start (not the clip start) owns Play.
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 24_000,
            end: 48_000,
            enabled: false
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        view.time_selection = None;
        view.selected_clip = Some(clip);
        view.edit_cursor_in_selected_clip = Some(clip);
        assert_eq!(view.playback_start_frame(), second);
    });
    visual.simulate_keystrokes("cmd-l");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.edit_cursor.is_none());
        assert_eq!(view.selected_clip, Some(clip), "clip selection is kept");
        assert_eq!(view.playback_start_frame(), 24_000);
    });
    let third = place(&mut visual, 0.7);
    assert_ne!(third, 24_000);
}

#[gpui::test]
fn new_selection_loop_takes_play_start_only_after_ack_and_later_cursor_wins(
    cx: &mut TestAppContext,
) {
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetLoopRegion {
            start: 24_000,
            end: 36_000,
            enabled: true
        })))
    });
    pump(&editor, &mut visual);
    click(&mut visual, audio, false);
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        v.edit_cursor = Some(super::edit_cursor::EditCursor {
            track: Some(track),
            frame: 18_000,
            generation: v.project_generation,
            pulse_color: 0,
            pulse_started: Instant::now(),
        });
        v.edit_cursor_in_selected_clip = Some(audio);
        v.time_selection = None;
        v.advance_selection_intent();
        assert_eq!(v.playback_start_frame(), 18_000);
    });
    crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
    let terminal = editor.update(&mut visual, |v, _| next(&v.service));
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.playback_start_frame(),
            18_000,
            "uninstalled edit cannot move play start"
        )
    });
    install_loop_terminal(&editor, &mut visual, terminal);
    editor.read_with(&visual, |v, _| assert_eq!(v.playback_start_frame(), 0));
    editor.update(&mut visual, |v, _| {
        v.edit_cursor.as_mut().unwrap().frame = 12_000;
        v.edit_cursor_in_selected_clip = Some(audio);
        v.advance_selection_intent();
        assert_eq!(v.playback_start_frame(), 12_000);
    });
}

#[gpui::test]
fn selection_loop_does_not_override_a_new_cursor_or_a_refused_terminal(cx: &mut TestAppContext) {
    for case in ["new cursor", "refusal", "replacement"] {
        let cx = &mut cx.new_app();
        let (handle, editor, audio, _, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().clone());
        crate::editor::section_launch::test_tap_loop_shortcut(&mut visual, "l");
        let mut event = editor.update(&mut visual, |v, _| next(&v.service));
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.edit_cursor = Some(super::edit_cursor::EditCursor {
                track: Some(track),
                frame: 18_000,
                generation: v.project_generation,
                pulse_color: 0,
                pulse_started: Instant::now(),
            });
            v.edit_cursor_in_selected_clip = Some(audio);
            v.time_selection = None;
            v.advance_selection_intent();
        });
        if case == "refusal" {
            event.error = Some("refused test loop".into());
            event.state = before;
            event.playback = None;
        } else if case == "replacement" {
            event.project_replaced = true;
        }
        install_loop_terminal(&editor, &mut visual, event);
        editor.read_with(&visual, |v, _| {
            assert!(v.selection_loop_start.is_none(), "{case}");
            if case != "replacement" {
                assert_eq!(v.playback_start_frame(), 18_000, "{case}");
            }
        });
    }
}
