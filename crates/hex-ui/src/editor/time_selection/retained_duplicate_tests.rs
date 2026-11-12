//! Current-source input/Service/late-frame diagnostics for pulse2880.
use super::*;
use crate::editor::test_environment::Environment;
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px, size,
};
use hex_project::{BeatTime, ClipId, MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
use hex_session::service::Service;
use std::{sync::Arc, time::Duration};

fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            let changed =
                view.tick(cx) | view.tick_edit_cursor(Instant::now() + Duration::from_millis(16));
            if changed {
                cx.notify();
            }
            !view.edit_pending()
        });
        visual.run_until_parked();
        if done {
            break;
        }
        assert!(Instant::now() < until, "actual Service never settled");
        std::thread::yield_now();
    }
    for _ in 0..8 {
        editor.update(&mut *visual, |view, cx| {
            let _ = view.tick(cx);
            let _ = view.tick_edit_cursor(Instant::now() + Duration::from_millis(16));
            cx.notify();
        });
        visual.run_until_parked();
    }
}

type RangeFixture = (
    VisualTestContext,
    Entity<EditorView>,
    Arc<Environment>,
    Vec<TrackId>,
    Vec<ClipId>,
);
fn fixture(cx: &mut TestAppContext) -> RangeFixture {
    fixture_with_audio(cx, false)
}
fn fixture_with_audio(cx: &mut TestAppContext, audio: bool) -> RangeFixture {
    fixture_with_recovery(cx, audio, false)
}
fn fixture_with_recovery(
    cx: &mut TestAppContext,
    audio: bool,
    recovery_failure: bool,
) -> RangeFixture {
    cx.update(crate::editor::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir_all(environment.root().join("samples")).unwrap();
    let service = if recovery_failure {
        // A deterministic filesystem refusal exercises the real worker's post-commit
        // checkpoint error without touching the user's recovery journal.
        let blocker = environment.root().join("recovery-blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        Service::with_recovery(48_000, 32, 8_000_000, blocker.join("untitled.hex-journal")).unwrap()
    } else {
        Service::new(48_000, 32, 8_000_000).unwrap()
    };
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_environment(
                    window,
                    cx,
                    service,
                    Arc::clone(&environment),
                )
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    visual.simulate_resize(size(px(1500.0), px(800.0)));
    let until = crate::test_wait::hang_deadline();
    loop {
        editor.update(&mut visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
        });
        visual.run_until_parked();
        if editor.read_with(&visual, |view, _| {
            view.state.is_some() && !view.audio_import_settings.is_loading()
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "fixture_with_recovery: hang guard expired"
        );
        std::thread::yield_now();
    }
    let mut tracks = Vec::new();
    let mut clips = Vec::new();
    for index in 0..2 {
        if audio && index == 0 {
            let path = environment.root().join("samples/range.wav");
            let frames = 96_000_u32;
            let rate = 48_000_u32;
            let n = frames * 2;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + n).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16_u32.to_le_bytes());
            bytes.extend_from_slice(&1_u16.to_le_bytes());
            bytes.extend_from_slice(&1_u16.to_le_bytes());
            bytes.extend_from_slice(&rate.to_le_bytes());
            bytes.extend_from_slice(&(rate * 2).to_le_bytes());
            bytes.extend_from_slice(&2_u16.to_le_bytes());
            bytes.extend_from_slice(&16_u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&n.to_le_bytes());
            for _ in 0..frames {
                bytes.extend_from_slice(&8192_i16.to_le_bytes());
            }
            std::fs::write(&path, bytes).unwrap();
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Import(path)))
            });
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert!(view.error.is_none(), "{:?}", view.error);
                let track = &view.state.as_ref().unwrap().project.tracks()[0];
                assert_eq!(track.clips()[0].duration(), u64::from(frames));
                tracks.push(track.id());
                clips.push(track.clips()[0].id());
            });
            continue;
        }
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiTrack {
                name: format!("range {index}")
            })))
        });
        pump(&editor, &mut visual);
        let track = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.tracks()[index].id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(4 << 32)
            })))
        });
        pump(&editor, &mut visual);
        let clip = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()[0]
                .id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(2 << 32),
                pitch: 60 + index as u8,
                channel: 0,
                velocity: 100
            })))
        });
        pump(&editor, &mut visual);
        tracks.push(track);
        clips.push(clip);
    }
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(Arc::ptr_eq(&environment, &view.test_environment));
            view.viewport = Some(super::super::viewport::Viewport {
                start: 0,
                length: 384_000,
            });
            view.snap_grid = super::super::musical_time::SnapGrid::Off;
            view.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    (visual, editor, environment, tracks, clips)
}
fn lane(visual: &mut VisualTestContext, track: TrackId) -> gpui::Bounds<gpui::Pixels> {
    visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .expect("actual rendered lane")
}
fn pointer_range(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    tracks: &[TrackId],
) -> hex_project::TimeRange {
    pointer_range_frames(visual, editor, tracks, 24_000, 72_000)
}
fn pointer_range_frames(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    tracks: &[TrackId],
    from: u64,
    to: u64,
) -> hex_project::TimeRange {
    let first = lane(visual, tracks[0]);
    let last = lane(visual, *tracks.last().unwrap());
    let a = point(
        first.origin.x + first.size.width * (from as f32 / 384_000.0),
        first.origin.y + first.size.height * 0.7,
    );
    let b = point(
        last.origin.x + last.size.width * (to as f32 / 384_000.0),
        last.origin.y + last.size.height * 0.7,
    );
    visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(b, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(b, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(visual, |view, _| {
        let range = view
            .normalized_time_selection()
            .expect("actual pointer made time range");
        assert_eq!(range.tracks(), tracks);
        assert!(range.duration() > 0);
        range
    })
}
#[gpui::test]
fn real_command_d_three_times_retains_destination_range_after_worker_and_late_ticks(
    cx: &mut TestAppContext,
) {
    keyboard_copies(cx, false);
}
#[gpui::test]
fn committed_keyboard_copies_retain_range_despite_recovery_warning(cx: &mut TestAppContext) {
    keyboard_copies(cx, true);
}

#[gpui::test]
fn committed_time_copy_paints_exact_rows_until_its_terminal(cx: &mut TestAppContext) {
    use hex_session::service::{ArrangementEditViewKind, ArrangementEditViewObservation};
    let (mut visual, editor, _environment, tracks, originals) = fixture_with_audio(cx, true);
    let source = pointer_range(&mut visual, &editor, &tracks);
    visual.simulate_keystrokes("d");
    let deadline = crate::test_wait::hang_deadline();
    let observation = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "committed copy view");
        std::thread::yield_now();
    };
    assert!(matches!(
        observation.kind,
        ArrangementEditViewKind::TimeRangeCopy
    ));
    let committed = observation.state.upgrade().unwrap();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "copy terminal");
        std::thread::yield_now();
    };
    assert!(terminal.error.is_none());
    let copied_audio = committed
        .project
        .track(tracks[0])
        .unwrap()
        .clips()
        .iter()
        .find(|clip| clip.id() != originals[0] && clip.start() == source.end())
        .expect("copied audio at destination")
        .id();
    let copied_midi = committed
        .project
        .track(tracks[1])
        .unwrap()
        .midi_clips()
        .iter()
        .find(|clip| {
            clip.id() != originals[1]
                && committed
                    .project
                    .tempo_map()
                    .beat_to_frame(clip.start())
                    .unwrap()
                    == source.end()
        })
        .expect("copied MIDI at destination")
        .id();
    editor.update(&mut visual, |view, cx| {
        let invalid = ArrangementEditViewObservation {
            project_generation: observation.project_generation + 1,
            first_previous_revision: observation.first_previous_revision,
            previous_revision: observation.previous_revision,
            kind: ArrangementEditViewKind::TimeRangeCopy,
            state: Arc::downgrade(&committed),
        };
        assert!(!view.observe_time_range_copy_view(invalid));
        let wrong_revision = ArrangementEditViewObservation {
            project_generation: observation.project_generation,
            first_previous_revision: observation.previous_revision + 1,
            previous_revision: observation.previous_revision + 1,
            kind: ArrangementEditViewKind::TimeRangeCopy,
            state: Arc::downgrade(&committed),
        };
        assert!(!view.observe_time_range_copy_view(wrong_revision));
        assert!(view.observe_time_range_copy_view(observation));
        assert!(Arc::ptr_eq(
            &view.arrangement_rows_state().unwrap(),
            &committed
        ));
        assert_eq!(view.normalized_time_selection().as_ref(), Some(&source));
        cx.notify();
    });
    visual.run_until_parked();
    let audio_selector = Box::leak(format!("clip-name-{}", copied_audio.value()).into_boxed_str());
    let midi_selector = Box::leak(format!("clip-name-{}", copied_midi.value()).into_boxed_str());
    let audio_before = visual
        .debug_bounds(audio_selector)
        .expect("committed audio paints before terminal");
    let midi_before = visual
        .debug_bounds(midi_selector)
        .expect("committed MIDI paints before terminal");
    let newer = pointer_range(&mut visual, &editor, &tracks[1..]);
    editor.update(&mut visual, |view, cx| {
        view.receive(terminal);
        assert!(view.committed_time_range_copy_view.is_none());
        assert!(view.pending_time_range_move.is_none());
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &committed));
        assert_eq!(view.normalized_time_selection().as_ref(), Some(&newer));
        cx.notify();
    });
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds(audio_selector), Some(audio_before));
    assert_eq!(visual.debug_bounds(midi_selector), Some(midi_before));
}

#[gpui::test]
fn committed_time_copy_view_is_invalidated_by_project_replacement(cx: &mut TestAppContext) {
    use hex_session::service::{ArrangementEditViewKind, ArrangementEditViewObservation};
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    pointer_range(&mut visual, &editor, &tracks[..1]);
    visual.simulate_keystrokes("d");
    let deadline = crate::test_wait::hang_deadline();
    let observation = loop {
        if let Ok(view) =
            editor.read_with(&visual, |view, _| view.service.try_arrangement_edit_view())
        {
            break view;
        }
        assert!(Instant::now() < deadline, "committed copy view");
        std::thread::yield_now();
    };
    let old_generation = observation.project_generation;
    let previous_revision = observation.previous_revision;
    let committed = observation.state.upgrade().unwrap();
    let _held_terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "copy terminal");
        std::thread::yield_now();
    };
    editor.update(&mut visual, |view, cx| {
        assert!(view.observe_time_range_copy_view(observation));
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        view.service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::Resident,
                reservation: None,
            })
            .unwrap()
    });
    let replacement = loop {
        if let Ok(event) = editor.read_with(&visual, |view, _| view.service.try_event()) {
            break event;
        }
        assert!(Instant::now() < deadline, "replacement terminal");
        std::thread::yield_now();
    };
    assert!(replacement.project_replaced);
    editor.update(&mut visual, |view, cx| {
        view.receive(replacement);
        assert_ne!(view.project_generation, old_generation);
        assert!(view.pending_time_range_move.is_none());
        assert!(view.committed_time_range_copy_view.is_none());
        assert!(
            !view.observe_time_range_copy_view(ArrangementEditViewObservation {
                project_generation: old_generation,
                first_previous_revision: previous_revision,
                previous_revision,
                kind: ArrangementEditViewKind::TimeRangeCopy,
                state: Arc::downgrade(&committed),
            })
        );
        cx.notify();
    });
    visual.run_until_parked();
}

#[gpui::test]
fn repeated_time_duplicates_queue_before_first_receipt(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    let source = pointer_range(&mut visual, &editor, &tracks);
    let (revision, mut original) = editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        (state.revision, state.project.snapshot())
    });
    visual.simulate_keystrokes("d d d");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.worker_requests_pending, 1);
        assert_eq!(view.error, None);
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.revision, revision + 3);
        let range = view.normalized_time_selection().unwrap();
        assert_eq!(range.tracks(), source.tracks());
        assert_eq!(range.start(), source.start() + 3 * source.duration());
        assert_eq!(range.duration(), source.duration());
    });
    for _ in 0..3 {
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |view, _| {
        let restored = view.state.as_ref().unwrap().project.snapshot();
        original.next_id = restored.next_id;
        assert_eq!(restored, original);
    });
}

fn keyboard_copies(cx: &mut TestAppContext, recovery_failure: bool) {
    for rows in [1, 2] {
        let (mut visual, editor, _environment, tracks, originals) =
            fixture_with_recovery(cx, false, recovery_failure);
        let mut expected = pointer_range(&mut visual, &editor, &tracks[..rows]);
        let original_project = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for iteration in 0..3 {
            let start = expected.end();
            let end = start + expected.duration();
            let epoch = editor.read_with(&visual, |view, _| view.selection_intent_epoch);
            visual.simulate_keystrokes(if recovery_failure {
                "cmd-d cmd-d"
            } else {
                "cmd-d"
            });
            editor.read_with(&visual, |view, _| {
                assert!(view.pending_time_range_move.is_some())
            });
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.selection_intent_epoch, epoch);
                if recovery_failure {
                    assert!(
                        view.error
                            .as_deref()
                            .unwrap()
                            .contains("automatic recovery:")
                    );
                }
            });
            expected = editor.read_with(&visual, |view, _| {
                let actual = view
                    .normalized_time_selection()
                    .expect("duplicate retains range");
                assert_eq!(
                    (actual.start(), actual.end()),
                    (start, end),
                    "rows={rows} repetition={iteration}"
                );
                assert_eq!(actual.tracks(), &tracks[..rows]);
                assert!(
                    view.selected_clip.is_none(),
                    "range must not collapse into one clip"
                );
                for (&track, &source) in tracks[..rows].iter().zip(&originals) {
                    let data = view.state.as_ref().unwrap().project.track(track).unwrap();
                    assert!(data.midi_clips().iter().any(|clip| clip.id() == source));
                    assert!(data.midi_clips().iter().any(|clip| {
                        clip.id() != source
                            && view
                                .state
                                .as_ref()
                                .unwrap()
                                .project
                                .tempo_map()
                                .beat_to_frame(clip.start())
                                .unwrap()
                                == start
                    }));
                }
                actual
            });
        }
        let copied_project = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for _ in 0..3 {
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |view, _| {
            let mut original = original_project;
            original.next_id = view.state.as_ref().unwrap().project.snapshot().next_id;
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                original,
                "three duplicates use three undo steps"
            );
        });
        for _ in 0..3 {
            visual.simulate_keystrokes("cmd-shift-z");
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.state.as_ref().unwrap().project.snapshot(),
                copied_project
            );
        });
    }
}
#[gpui::test]
fn real_option_drag_then_command_d_retains_range_after_worker_and_late_ticks(
    cx: &mut TestAppContext,
) {
    drag_then_keyboard_copy(cx, false);
}
#[gpui::test]
fn committed_drag_copy_retains_range_despite_recovery_warning(cx: &mut TestAppContext) {
    drag_then_keyboard_copy(cx, true);
}
fn drag_then_keyboard_copy(cx: &mut TestAppContext, recovery_failure: bool) {
    let (mut visual, editor, _environment, tracks, _) =
        fixture_with_recovery(cx, false, recovery_failure);
    let original = pointer_range(&mut visual, &editor, &tracks[..1]);
    let row = lane(&mut visual, tracks[0]);
    let grab = original.start() + original.duration() / 2;
    let endframe = grab + original.duration() * 2;
    let a = point(
        row.origin.x + row.size.width * (grab as f32 / 384_000.0),
        row.origin.y + row.size.height * 0.15,
    );
    let b = point(
        row.origin.x + row.size.width * (endframe as f32 / 384_000.0),
        a.y,
    );
    let option = Modifiers {
        alt: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_down(a, MouseButton::Left, option);
    visual.simulate_mouse_move(b, Some(MouseButton::Left), option);
    let expected = editor.read_with(&visual, |view, _| {
        let preview = view
            .time_range_move_display()
            .expect("actual Option-drag range preview");
        assert!(preview.copy_modifier);
        (preview.destination_start, preview.destination_end)
    });
    let epoch = editor.read_with(&visual, |view, _| view.selection_intent_epoch);
    visual.simulate_mouse_up(b, MouseButton::Left, option);
    editor.read_with(&visual, |view, _| {
        assert!(view.pending_time_range_move.is_some())
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.selection_intent_epoch, epoch)
    });
    editor.read_with(&visual, |view, _| {
        let range = view
            .normalized_time_selection()
            .expect("range remains after copy");
        assert_eq!((range.start(), range.end()), expected);
        assert!(view.selected_clip.is_none());
        if recovery_failure {
            assert!(
                view.error
                    .as_deref()
                    .unwrap()
                    .contains("automatic recovery:")
            );
        }
    });
    visual.simulate_keystrokes("cmd-d");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let range = view.normalized_time_selection().unwrap();
        assert_eq!(
            (range.start(), range.end()),
            (expected.1, expected.1 + expected.1 - expected.0)
        );
    });
}

#[gpui::test]
fn real_audio_midi_partial_gap_copies_preserve_scope_media_and_one_undo(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, tracks, clips) = fixture_with_audio(cx, true);
    let initial = pointer_range_frames(&mut visual, &editor, &tracks, 24_000, 120_000);
    assert!(initial.start() > 0 && initial.start() < 48_000);
    assert!(
        initial.end() > 96_000,
        "actual pointer captures a partial clip and trailing gap"
    );
    let offset = initial.start();
    let gap = initial.end() - 96_000;
    let (before, original) = editor.read_with(&visual, |view, _| {
        let p = &view.state.as_ref().unwrap().project;
        (p.snapshot(), p.track(tracks[0]).unwrap().clips()[0].clone())
    });
    let mut source = initial;
    for _ in 0..2 {
        let start = source.end();
        let end = start + source.duration();
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        source = editor.read_with(&visual, |view, _| {
            let p = &view.state.as_ref().unwrap().project;
            let range = view.normalized_time_selection().unwrap();
            assert_eq!((range.start(), range.end()), (start, end));
            assert_eq!(range.tracks(), tracks);
            assert!(view.selected_clip.is_none());
            assert_eq!(p.clip(clips[0]).unwrap(), &original);
            let copied = p
                .track(tracks[0])
                .unwrap()
                .clips()
                .iter()
                .find(|clip| clip.start() == start)
                .unwrap();
            assert_ne!(copied.id(), original.id());
            assert_eq!(copied.duration(), 96_000 - offset);
            assert_eq!(copied.source_offset(), offset);
            assert_eq!(copied.asset(), original.asset());
            assert_eq!(
                copied.start() + copied.duration(),
                end - gap,
                "trailing gap preserved"
            );
            let midi = p
                .track(tracks[1])
                .unwrap()
                .midi_clips()
                .iter()
                .find(|clip| p.tempo_map().beat_to_frame(clip.start()).unwrap() == start)
                .unwrap();
            assert_ne!(midi.id(), clips[1]);
            assert_eq!(midi.notes().len(), 1);
            assert_eq!(midi.notes()[0].start(), BeatTime::ZERO);
            assert_eq!(
                p.tempo_map()
                    .beat_to_frame(midi.notes()[0].duration())
                    .unwrap(),
                p.tempo_map()
                    .beat_to_frame(BeatTime::from_raw(
                        (2 << 32) - p.tempo_map().frame_to_beat(offset).unwrap().raw()
                    ))
                    .unwrap()
            );
            range
        });
        let bounds = visual
            .debug_bounds(Box::leak(
                format!("edit-time-selection-{}", tracks[0].value()).into_boxed_str(),
            ))
            .expect("completed destination range is rendered");
        let row = lane(&mut visual, tracks[0]);
        assert!(
            (f32::from(bounds.origin.x - row.origin.x) / f32::from(row.size.width)
                - start as f32 / 384_000.0)
                .abs()
                < 0.002
        );
    }
    let after = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    for _ in 0..2 {
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |view, _| {
        let mut expected = before;
        expected.next_id = after.next_id;
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), expected);
    });
    for _ in 0..2 {
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
    }
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });
}

fn submit_range_copy(
    visual: &mut VisualTestContext,
    editor: &Entity<EditorView>,
    source: &hex_project::TimeRange,
    option_drag: bool,
) -> u64 {
    let destination = if option_drag {
        let row = lane(visual, source.tracks()[0]);
        let grab = source.start() + source.duration() / 2;
        let a = point(
            row.origin.x + row.size.width * (grab as f32 / 384_000.0),
            row.origin.y + row.size.height * 0.15,
        );
        let b = point(
            a.x + row.size.width * (source.duration() as f32 / 384_000.0),
            a.y,
        );
        let option = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(a, MouseButton::Left, option);
        visual.simulate_mouse_move(b, Some(MouseButton::Left), option);
        let destination = editor.read_with(&*visual, |view, _| {
            view.time_range_move_display()
                .expect("actual copy preview")
                .destination_start
        });
        visual.simulate_mouse_up(b, MouseButton::Left, option);
        destination
    } else {
        visual.simulate_keystrokes("cmd-d");
        source.end()
    };
    editor.read_with(&*visual, |view, _| {
        assert!(view.pending_time_range_move.is_some())
    });
    destination
}

#[gpui::test]
fn delayed_keyboard_copy_ack_respects_pointer_selection_aba(cx: &mut TestAppContext) {
    delayed_copy_selection_aba(cx, false, false);
}

#[gpui::test]
fn delayed_option_copy_ack_respects_pointer_selection_aba(cx: &mut TestAppContext) {
    delayed_copy_selection_aba(cx, true, false);
}

#[gpui::test]
fn exhausted_selection_epoch_invalidates_old_ack_and_refuses_new_copy(cx: &mut TestAppContext) {
    delayed_copy_selection_aba(cx, false, true);
}

fn delayed_copy_selection_aba(cx: &mut TestAppContext, option_drag: bool, exhaust: bool) {
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    let source = pointer_range(&mut visual, &editor, &tracks[..1]);
    if exhaust {
        editor.update(&mut visual, |view, _| {
            view.selection_intent_epoch = Some(u64::MAX - 1)
        });
    }
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    let destination = submit_range_copy(&mut visual, &editor, &source, option_drag);
    let other = pointer_range(&mut visual, &editor, &tracks[1..]);
    assert_ne!(source, other);
    let returned = pointer_range(&mut visual, &editor, &tracks[..1]);
    assert_eq!(
        returned, source,
        "actual pointer returns to identical range A"
    );
    editor.read_with(&visual, |view, _| {
        assert!(
            view.pending_time_range_move.is_some(),
            "ACK remains unconsumed across A→B→A"
        );
        assert_eq!(view.state.as_ref().unwrap().revision, revision);
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        assert_eq!(
            state.revision,
            revision + 1,
            "accepted copy still commits exactly once"
        );
        assert!(
            state
                .project
                .track(tracks[0])
                .unwrap()
                .midi_clips()
                .iter()
                .any(|clip| {
                    state
                        .project
                        .tempo_map()
                        .beat_to_frame(clip.start())
                        .unwrap()
                        == destination
                }),
            "canonical destination copy survives selection cancellation"
        );
        assert_eq!(
            view.normalized_time_selection().as_ref(),
            Some(&source),
            "later actual A selection owns intent, option_drag={option_drag}"
        );
        assert_eq!(view.selected_track, Some(tracks[0]));
        assert!(view.selected_clip.is_none());
    });
    if exhaust {
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selection_intent_epoch, None)
        });
        visual.simulate_keystrokes("cmd-d");
        editor.read_with(&visual, |view, _| {
            assert!(
                !view.edit_pending(),
                "exhausted epoch refuses before command admission"
            );
            assert!(view.pending_time_range_move.is_none());
            assert!(
                view.error
                    .as_deref()
                    .unwrap()
                    .contains("selection intent exhausted")
            );
        });
        // Further actual selection cannot restore a spent epoch or admit a copy.
        pointer_range(&mut visual, &editor, &tracks[1..]);
        visual.simulate_keystrokes("cmd-d");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.selection_intent_epoch, None);
            assert_eq!(view.state.as_ref().unwrap().revision, revision + 1);
        });
    }
}

#[gpui::test]
fn delayed_keyboard_copy_ack_respects_console_navigation(cx: &mut TestAppContext) {
    delayed_copy_console_selection(cx, false);
}

#[gpui::test]
fn delayed_option_copy_ack_respects_console_navigation(cx: &mut TestAppContext) {
    delayed_copy_console_selection(cx, true);
}

fn delayed_copy_console_selection(cx: &mut TestAppContext, option_drag: bool) {
    for navigate in [false, true] {
        let (mut visual, editor, _environment, tracks, _) = fixture(cx);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddScene {
                name: "console".into()
            })));
        });
        pump(&editor, &mut visual);
        let scene = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.scenes()[0].id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::CreateSessionMidiClip {
                track: tracks[1],
                scene,
                name: "console midi".into(),
                source_length: BeatTime::from_raw(4 << 32),
            })));
        });
        pump(&editor, &mut visual);
        let session_clip = editor.read_with(&visual, |view, _| {
            let hex_project::SessionCellContent::Midi(clip) = view
                .state
                .as_ref()
                .unwrap()
                .project
                .session_cell(tracks[1], scene)
                .unwrap()
            else {
                panic!("session midi")
            };
            clip.id()
        });
        visual.update(|window, cx| {
            editor.update(cx, |view, cx| {
                view.set_mode(EditorMode::Arrangement);
                view.selected_cell = None;
                view.selected_scene = None;
                view.focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        let source = pointer_range(&mut visual, &editor, &tracks[..1]);
        let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
        let destination = submit_range_copy(&mut visual, &editor, &source, option_drag);
        let epoch = editor.read_with(&visual, |view, _| view.selection_intent_epoch);
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.mode, EditorMode::Session);
            assert_eq!(view.selection_intent_epoch, epoch, "Tab alone keeps intent");
            assert!(view.pending_time_range_move.is_some());
        });
        // Clicks are deliberately refused by the existing pending-edit guard.
        let bounds = visual
            .debug_bounds(Box::leak(
                format!("console-cell-{}-{}", tracks[1].value(), scene.value()).into_boxed_str(),
            ))
            .expect("rendered console midi cell");
        let click = point(bounds.origin.x + bounds.size.width * 0.4, bounds.center().y);
        visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.selection_intent_epoch, epoch,
                "refused click keeps intent"
            );
            assert!(view.selected_cell.is_none());
        });
        if navigate {
            visual.simulate_keystrokes("right right");
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.selected_cell, Some((tracks[1], scene)));
                assert_eq!(view.selected_clip, Some(session_clip));
            });
        }
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert_eq!(
                state.revision,
                revision + 1,
                "accepted Arrange copy commits"
            );
            assert!(
                state
                    .project
                    .track(tracks[0])
                    .unwrap()
                    .midi_clips()
                    .iter()
                    .any(|clip| state
                        .project
                        .tempo_map()
                        .beat_to_frame(clip.start())
                        .unwrap()
                        == destination)
            );
            let range = view.normalized_time_selection().unwrap();
            if navigate {
                assert_eq!(view.selected_cell, Some((tracks[1], scene)));
                assert_eq!(view.selected_track, Some(tracks[1]));
                assert_eq!(
                    view.selected_clip,
                    Some(session_clip),
                    "actual Console choice owns selection after Arrange ACK"
                );
                assert_eq!(range, source);
            } else {
                assert_eq!(
                    (range.start(), range.end()),
                    (destination, destination + source.duration())
                );
                assert_eq!(view.selection_intent_epoch, epoch);
            }
        });
        if !navigate {
            visual.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(click, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert_eq!(view.selected_clip, Some(session_clip));
                assert_ne!(
                    view.selection_intent_epoch, epoch,
                    "accepted actual Console click advances intent"
                );
            });
        }
    }
}

#[gpui::test]
fn console_navigation_without_target_keeps_pending_copy_intent(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    let source = pointer_range(&mut visual, &editor, &tracks[..1]);
    let destination = submit_range_copy(&mut visual, &editor, &source, false);
    let epoch = editor.read_with(&visual, |view, _| view.selection_intent_epoch);
    visual.simulate_keystrokes("tab right");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.mode, EditorMode::Session);
        assert!(view.selected_cell.is_none() && view.selected_scene.is_none());
        assert_eq!(
            view.selection_intent_epoch, epoch,
            "no target cannot consume selection intent"
        );
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let range = view.normalized_time_selection().unwrap();
        assert_eq!(
            (range.start(), range.end()),
            (destination, destination + source.duration())
        );
        assert_eq!(view.selection_intent_epoch, epoch);
    });
}

#[gpui::test]
fn actual_later_range_selection_is_not_stolen_by_queued_range_copy_ack(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    let _ = pointer_range(&mut visual, &editor, &tracks[..1]);
    let revision = editor.read_with(&visual, |view, _| view.state.as_ref().unwrap().revision);
    visual.simulate_keystrokes("cmd-d cmd-d");
    editor.read_with(&visual, |view, _| {
        assert!(
            view.pending_time_range_move.is_some(),
            "own ACK intentionally not consumed yet"
        )
    });
    let newer = pointer_range(&mut visual, &editor, &tracks[1..]);
    editor.read_with(&visual, |view, _| {
        assert!(
            view.pending_time_range_move.is_some(),
            "actual newer body range created before consuming copy ACK"
        )
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.normalized_time_selection().as_ref(),
            Some(&newer),
            "later actual pointer range owns selection"
        );
        assert_eq!(view.selected_track, Some(tracks[1]));
        assert!(view.selected_clip.is_none());
        assert_eq!(view.state.as_ref().unwrap().revision, revision + 1);
    });
}

#[gpui::test]
fn escape_before_submission_cancels_copy_but_after_submission_only_cancels_selection_intent(
    cx: &mut TestAppContext,
) {
    for submitted in [false, true] {
        let (mut visual, editor, _environment, tracks, _) = fixture(cx);
        let source = pointer_range(&mut visual, &editor, &tracks[..1]);
        let before = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        if submitted {
            let presses = vec!["d"; usize::from(MAX_QUEUED_REPEATS) + 2].join(" ");
            visual.simulate_keystrokes(&presses);
            editor.read_with(&visual, |view, _| {
                let pending = view.pending_time_range_move.as_ref().unwrap();
                assert_eq!(
                    pending.keyboard.as_ref().unwrap().repeats,
                    Some(MAX_QUEUED_REPEATS)
                );
                assert_eq!(
                    view.error.as_deref(),
                    Some("duplicate queue full · maximum 32 queued repeats")
                );
            });
        } else {
            let row = lane(&mut visual, tracks[0]);
            let grab = source.start() + source.duration() / 2;
            let a = point(
                row.origin.x + row.size.width * (grab as f32 / 384_000.0),
                row.origin.y + row.size.height * 0.15,
            );
            let b = point(
                a.x + row.size.width * (source.duration() as f32 / 384_000.0),
                a.y,
            );
            let option = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(a, MouseButton::Left, option);
            visual.simulate_mouse_move(b, Some(MouseButton::Left), option);
            editor.read_with(&visual, |view, _| {
                assert!(view.time_range_move_gesture.is_some())
            });
        }
        visual.simulate_keystrokes("escape");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(view.time_range_move_gesture.is_none());
            assert!(view.pending_time_range_move.is_none());
            let p = &view.state.as_ref().unwrap().project;
            if submitted {
                assert!(
                    view.normalized_time_selection().is_none(),
                    "accepted copy cannot restore escaped range intent"
                );
                assert_ne!(
                    p.snapshot(),
                    before,
                    "already accepted command still commits"
                );
            } else {
                assert_eq!(p.snapshot(), before, "cancelled preview queued no edit");
                assert_eq!(view.normalized_time_selection().as_ref(), Some(&source));
            }
        });
        if submitted {
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                let actual = view.state.as_ref().unwrap().project.snapshot();
                let mut expected = before;
                expected.next_id = actual.next_id;
                assert_eq!(actual, expected);
            });
        }
    }
}

#[gpui::test]
fn refused_worker_copy_preserves_source_selection(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, tracks, _) = fixture(cx);
    let source = pointer_range(&mut visual, &editor, &tracks[..1]);
    let before = editor.read_with(&visual, |view, _| Arc::clone(view.state.as_ref().unwrap()));
    editor.update(&mut visual, |view, _| {
        // Exercise a real transaction refusal at the completion boundary, rather
        // than the UI's earlier overflow guard. No canonical revision commits.
        assert!(view.enqueue(Request::Edit(Command::CopyTimeRangeTo {
            range: source.clone(),
            track: tracks[0],
            frame: u64::MAX,
        })));
        view.pending_time_range_move = Some(PendingTimeRangeMove {
            source: source.clone(),
            range: hex_project::TimeRange::new(
                tracks[..1].to_vec(),
                source.end(),
                source.end() + source.duration(),
            )
            .unwrap(),
            project_scope: false,
            destination_first_index: 0,
            destination_last_index: 0,
            source_revision: before.revision,
            selection_intent_epoch: view.selection_intent_epoch.unwrap(),
            copy_modifier: true,
            keyboard: None,
        });
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_some());
        assert!(view.pending_time_range_move.is_none());
        assert_eq!(view.state.as_ref().unwrap().revision, before.revision);
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            before.project.snapshot()
        );
        assert_eq!(view.normalized_time_selection().as_ref(), Some(&source));
    });
}

#[gpui::test]
fn leading_silence_copy_with_a_note_on_the_destination_edge_survives_real_worker(
    cx: &mut TestAppContext,
) {
    for (drag, bpm, leading_silence, reversed) in [
        (false, 120_000_000, true, false),
        (true, 120_000_000, true, false),
        (false, 143_000_000, false, false),
        (false, 143_000_000, true, true),
    ] {
        let (mut visual, editor, environment, tracks, clips) = fixture(cx);
        let clip = clips[0];
        let note = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .midi_clip(clip)
                .unwrap()
                .notes()[0]
                .id()
        });
        for command in [
            Command::UpdateMidiNotes {
                clip,
                notes: vec![hex_project::MidiNoteUpdate {
                    note,
                    start: BeatTime::ONE,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            },
            Command::MoveMidiClip {
                clip,
                track: tracks[0],
                start: BeatTime::ONE,
            },
            Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: hex_project::Tempo::new(bpm).unwrap(),
            },
        ] {
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Edit(command)))
            });
            pump(&editor, &mut visual);
        }
        if reversed {
            let source_length = editor.read_with(&visual, |view, _| {
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_clip(clip)
                    .unwrap()
                    .source_length()
            });
            let volume = MidiClipVolume::new(
                vec![
                    MidiClipVolumePoint::linear(BeatTime::ZERO, MidiClipGain::new(0.25).unwrap()),
                    MidiClipVolumePoint::linear(source_length, MidiClipGain::new(0.75).unwrap()),
                ],
                source_length,
            )
            .unwrap();
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Edit(Command::ReplaceMidiClipVolume {
                    clip,
                    volume
                })));
            });
            pump(&editor, &mut visual);
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Edit(Command::ReverseClips(vec![clip]))));
            });
            pump(&editor, &mut visual);
        }
        let before = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            view.state.as_ref().unwrap().project.snapshot()
        });
        editor.update(&mut visual, |view, cx| {
            view.snap_grid = super::super::musical_time::SnapGrid::Eighth;
            view.viewport = Some(super::super::viewport::Viewport {
                start: 0,
                length: 384_000,
            });
            cx.notify();
        });
        visual.run_until_parked();
        let (start, end) = editor.read_with(&visual, |view, _| {
            let tempo = view.state.as_ref().unwrap().project.tempo_map();
            (
                if leading_silence {
                    0
                } else {
                    // Begin inside the clip body, beyond its resize target.
                    tempo
                        .beat_to_frame(BeatTime::from_ratio(3, 2).unwrap())
                        .unwrap()
                },
                tempo
                    .beat_to_frame(BeatTime::from_quarters(2).unwrap())
                    .unwrap(),
            )
        });
        let range = pointer_range_frames(&mut visual, &editor, &tracks[..1], start, end);
        assert_eq!((range.start(), range.end()), (start, end));
        let duration = range.duration();
        let first = (end, end + duration);
        let second = (end + duration, end + duration * 2);
        if drag {
            let row = lane(&mut visual, tracks[0]);
            let a = point(
                row.origin.x + row.size.width * ((start + duration / 3) as f32 / 384_000.0),
                row.origin.y + row.size.height * 0.15,
            );
            let b = point(a.x + row.size.width * (duration as f32 / 384_000.0), a.y);
            let option = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(a, MouseButton::Left, option);
            visual.simulate_mouse_move(b, Some(MouseButton::Left), option);
            let destination = editor.read_with(&visual, |view, _| {
                let preview = view.time_range_move_display().expect("held range copy");
                assert!(preview.copy_modifier);
                (preview.destination_start, preview.destination_end)
            });
            assert_eq!(destination, first);
            visual.simulate_mouse_up(b, MouseButton::Left, option);
        } else {
            visual.simulate_keystrokes("cmd-d");
        }
        for (iteration, expected) in [first, second].into_iter().enumerate() {
            if iteration == 1 {
                visual.simulate_keystrokes("cmd-d");
            }
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert!(view.error.is_none(), "drag={drag}: {:?}", view.error);
                let range = view
                    .normalized_time_selection()
                    .expect("accepted copied range");
                assert_eq!((range.start(), range.end()), expected);
                assert_eq!(range.tracks(), &tracks[..1]);
                assert!(view.pending_time_range_move.is_none());
                assert!(view.selected_clip.is_none());
                assert_eq!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .midi_clip(clip)
                        .unwrap()
                        .notes()[0]
                        .id(),
                    note
                );
            });
        }
        let after = editor.read_with(&visual, |view, _| {
            view.state.as_ref().unwrap().project.snapshot()
        });
        for _ in 0..2 {
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |view, _| {
            let mut expected = before;
            expected.next_id = after.next_id;
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), expected);
        });
        for _ in 0..2 {
            visual.simulate_keystrokes("cmd-shift-z");
            pump(&editor, &mut visual);
        }
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
        });
        let saved = environment.root().join("frame-range-copy.hex");
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::SaveNew(saved.clone())));
        });
        pump(&editor, &mut visual);
        assert!(saved.is_file());
        let generation = editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert!(!view.state.as_ref().unwrap().unsaved);
            view.project_generation
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::RenameTrack {
                track: tracks[0],
                name: "unsaved sentinel".into(),
            })));
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_ne!(view.state.as_ref().unwrap().project.snapshot(), after);
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Open(saved)));
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_eq!(view.project_generation, generation + 1);
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after);
        });
    }
}
