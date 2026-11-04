//! Rendered held-note creation through the real Session worker and history.
use super::super::{EditorView, midi_drag::MidiGrid, test_environment::Environment};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, MouseDownEvent, TestAppContext, VisualTestContext,
    point,
};
use hex_project::{BeatTime, Command};
use hex_session::service::{Event, Request, Service};
use std::sync::Arc;

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
        std::thread::yield_now();
    }
}
pub(in crate::editor) fn fixture(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    Arc<Environment>,
    hex_project::ClipId,
) {
    cx.update(super::super::bind_editor_keys);
    let environment = Environment::new().unwrap();
    std::fs::create_dir(environment.root().join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service
        .submit(Request::Edit(Command::AddMidiTrack {
            name: "held creation".into(),
        }))
        .unwrap();
    let event = next(&service);
    let track = event.state.project.tracks()[0].id();
    service
        .submit(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        }))
        .unwrap();
    let event = next(&service);
    let clip = event.state.project.track(track).unwrap().midi_clips()[0].id();
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
                v.selected_track = Some(track);
                v.selected_clip = Some(clip);
                v.clip_view_open = true;
                v.piano_beat_span = BeatTime::from_quarters(4).unwrap();
                v.piano_beat_start = BeatTime::ZERO;
                v.midi_focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    (visual, editor, environment, clip)
}
pub(in crate::editor) fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            !v.edit_pending()
        });
        visual.run_until_parked();
        if done {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "pump: hang guard expired"
        );
        std::thread::yield_now();
    }
}
pub(in crate::editor) fn double_down(
    visual: &mut VisualTestContext,
    at: gpui::Point<gpui::Pixels>,
    modifiers: Modifiers,
) {
    visual.simulate_mouse_down(at, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(at, MouseButton::Left, modifiers);
    visual.simulate_event(MouseDownEvent {
        position: at,
        button: MouseButton::Left,
        click_count: 2,
        modifiers,
        first_mouse: false,
    });
}
#[gpui::test]
fn double_click_hold_resizes_one_snapped_note_and_one_history_step(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction| {
        point(
            roll.origin.x + gpui::px(1.0) + (roll.size.width - gpui::px(2.0)) * fraction,
            roll.center().y,
        )
    };
    let down = at(0.25);
    double_down(&mut visual, down, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(!v.edit_pending(), "held preview must not submit an edit")
    });
    let snap = Modifiers::command();
    visual.simulate_mouse_move(at(0.875), Some(MouseButton::Left), snap);
    visual.simulate_mouse_move(at(0.625), Some(MouseButton::Left), snap);
    visual.run_until_parked();
    visual.simulate_mouse_up(at(0.625), MouseButton::Left, snap);
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert_eq!(source.notes().len(), 1);
        let note = &source.notes()[0];
        assert_eq!(note.start(), BeatTime::ONE);
        assert_eq!(
            note.duration(),
            BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 2)
        );
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        )
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
}

#[gpui::test]
fn held_creation_end_is_free_by_default_and_snaps_while_cmd_is_held(cx: &mut TestAppContext) {
    for (grid, override_snap, motion, expected) in [
        (MidiGrid::Quarter, false, true, 1.56),
        (MidiGrid::Quarter, true, true, 2.0),
        (MidiGrid::Off, false, true, 1.56),
        (MidiGrid::Off, true, true, 1.5),
        (MidiGrid::Sixteenth, false, false, 0.25),
        (MidiGrid::Off, false, false, 0.0),
    ] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, cx| {
            v.midi_snap_grid = grid;
            cx.notify();
        });
        visual.run_until_parked();
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let at = |fraction| {
            point(
                roll.origin.x + gpui::px(1.0) + (roll.size.width - gpui::px(2.0)) * fraction,
                roll.origin.y + roll.size.height * 0.52,
            )
        };
        double_down(&mut visual, at(0.25), Modifiers::default());
        let start = editor.read_with(&visual, |v, _| {
            v.midi_creation.as_ref().unwrap().preview.start
        });
        let modifiers = Modifiers {
            platform: override_snap,
            ..Modifiers::default()
        };
        let end = at(if motion { 0.64 } else { 0.25 });
        if motion {
            visual.simulate_mouse_move(end, Some(MouseButton::Left), modifiers);
        }
        visual.simulate_mouse_up(end, MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let notes = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].start(), start, "drag must not move the anchor");
            let expected = if grid == MidiGrid::Off && !motion {
                4.0 / f64::from(f32::from(roll.size.width - gpui::px(2.0)))
            } else {
                expected
            };
            let gate = notes[0].duration().raw() as f64 / BeatTime::ONE.raw() as f64;
            assert!(
                (gate - expected).abs() < 0.01,
                "grid={grid:?} override={override_snap}: {gate} != {expected}"
            );
        });
    }
}

#[gpui::test]
fn releasing_cmd_mid_creation_returns_end_to_free_motion(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction| {
        point(
            roll.origin.x + gpui::px(1.0) + (roll.size.width - gpui::px(2.0)) * fraction,
            roll.center().y,
        )
    };
    double_down(&mut visual, at(0.25), Modifiers::default());
    let gate = |visual: &mut VisualTestContext| {
        editor.read_with(visual, |v, _| {
            v.midi_creation.as_ref().unwrap().preview.duration.raw() as f64
                / BeatTime::ONE.raw() as f64
        })
    };
    visual.simulate_mouse_move(at(0.64), Some(MouseButton::Left), Modifiers::command());
    assert_eq!(gate(&mut visual), 1.5, "cmd snaps the held end");
    visual.simulate_mouse_move(at(0.64), Some(MouseButton::Left), Modifiers::default());
    let free = gate(&mut visual);
    assert!((free - 1.56).abs() < 0.01, "released cmd is free: {free}");
    let preview = editor.read_with(&visual, |v, _| v.midi_creation.as_ref().unwrap().preview);
    visual.simulate_mouse_up(at(0.64), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let notes = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].start(), preview.start);
        assert_eq!(
            notes[0].duration(),
            preview.duration,
            "commit matches preview"
        );
    });
}

#[gpui::test]
fn held_creation_cancels_without_partial_edit_on_escape_focus_outside_and_stale_view(
    cx: &mut TestAppContext,
) {
    for reason in [
        "escape", "focus", "outside", "clip", "revision", "zoom", "grid", "resize",
    ] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let down = point(roll.origin.x + roll.size.width * 0.25, roll.center().y);
        double_down(&mut visual, down, Modifiers::default());
        let end = point(roll.origin.x + roll.size.width * 0.75, roll.center().y);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        match reason {
            "escape" => visual.simulate_keystrokes("escape"),
            "focus" => {
                visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)))
            }
            "clip" => editor.update(&mut visual, |v, cx| {
                v.selected_clip = None;
                cx.notify();
            }),
            "revision" => editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::RenameTrack {
                    track: v.selected_track.unwrap(),
                    name: "changed".into()
                })));
            }),
            "zoom" => editor.update(&mut visual, |v, cx| {
                v.piano_beat_span = BeatTime::from_quarters(2).unwrap();
                cx.notify();
            }),
            "grid" => editor.update(&mut visual, |v, cx| {
                v.midi_snap_grid = MidiGrid::Quarter;
                cx.notify();
            }),
            "resize" => visual.simulate_resize(gpui::size(gpui::px(1400.0), gpui::px(900.0))),
            _ => {}
        }
        visual.run_until_parked();
        let up = if reason == "outside" {
            point(roll.origin.x - gpui::px(5.0), roll.center().y)
        } else {
            end
        };
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_creation.is_none(), "{reason}");
            assert!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .notes()
                    .is_empty(),
                "{reason}"
            );
        });
    }
}

#[gpui::test]
fn x_deletes_selected_midi_note_and_cmd_z_restores_it(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = point(
        roll.origin.x + roll.size.width * 0.25,
        roll.origin.y + roll.size.height * 0.52,
    );
    double_down(&mut visual, at, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(project.midi_source(clip).unwrap().notes().len(), 1);
        project.snapshot()
    });
    visual.simulate_keystrokes("cmd-a x");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        )
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn normal_delete_and_draw_mode_still_use_their_own_gestures(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let at = |fraction| {
        point(
            roll.origin.x + gpui::px(1.0) + (roll.size.width - gpui::px(2.0)) * fraction,
            roll.origin.y + roll.size.height * 0.52,
        )
    };
    double_down(&mut visual, at(0.25), Modifiers::default());
    visual.simulate_mouse_up(at(0.25), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    double_down(&mut visual, at(0.28), Modifiers::default());
    visual.simulate_mouse_up(at(0.28), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        )
    });
    visual.simulate_keystrokes("b");
    visual.simulate_mouse_down(at(0.25), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert!(v.midi_creation.is_none());
        assert!(v.midi_stroke.is_some());
    });
    visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .len()
                > 1
        )
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()
                .is_empty()
        )
    });
}

#[gpui::test]
fn held_creation_clamps_to_clip_end_and_positive_minimum(cx: &mut TestAppContext) {
    for (grid, snap, from, to, expected) in [
        (
            MidiGrid::Sixteenth,
            true,
            0.25,
            0.01,
            BeatTime::ONE.raw() / 4,
        ),
        (MidiGrid::Sixteenth, false, 0.25, 0.01, 0),
        (MidiGrid::Off, false, 0.25, 0.01, 0),
        (
            MidiGrid::Sixteenth,
            true,
            0.875,
            0.98,
            BeatTime::ONE.raw() / 2,
        ),
    ] {
        let modifiers = Modifiers {
            platform: snap,
            ..Modifiers::default()
        };
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, cx| {
            v.midi_snap_grid = grid;
            cx.notify();
        });
        visual.run_until_parked();
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let at = |fraction| {
            point(
                roll.origin.x + gpui::px(1.0) + (roll.size.width - gpui::px(2.0)) * fraction,
                roll.center().y,
            )
        };
        double_down(&mut visual, at(from), Modifiers::default());
        visual.simulate_mouse_move(at(to), Some(MouseButton::Left), modifiers);
        visual.simulate_mouse_up(at(to), MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
            assert_eq!(
                source.notes().len(),
                1,
                "grid={grid:?} snap={snap} from={from} to={to}"
            );
            let note = &source.notes()[0];
            if expected > 0 {
                assert_eq!(note.duration().raw(), expected);
            } else {
                let map = v.state.as_ref().unwrap().project.tempo_map();
                let onset = map.beat_to_frame(note.start()).unwrap();
                let off = map
                    .beat_to_frame(BeatTime::from_raw(
                        note.start().raw() + note.duration().raw(),
                    ))
                    .unwrap();
                assert!(
                    off > onset,
                    "free minimum must survive canonical frame quantization"
                );
            }
            assert!(note.start().raw() + note.duration().raw() <= source.source_length().raw());
        });
    }
}

#[gpui::test]
fn zoomed_out_creation_continues_past_first_two_time_cells(cx: &mut TestAppContext) {
    for draw in [false, true] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        let half = BeatTime::from_raw(BeatTime::ONE.raw() / 2);
        let arrangement = BeatTime::from_raw(BeatTime::ONE.raw() * 11 / 2);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: half,
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: half,
                looping: false,
                length: arrangement,
            })))
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, cx| {
            v.midi_draw_mode = draw;
            cx.notify();
        });
        visual.run_until_parked();
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let before = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone()
        });
        let outside_source = point(roll.origin.x + roll.size.width * 0.75, roll.center().y);
        if draw {
            visual.simulate_mouse_down(outside_source, MouseButton::Left, Modifiers::default());
        } else {
            double_down(&mut visual, outside_source, Modifiers::default());
        }
        visual.simulate_mouse_up(
            point(roll.origin.x - gpui::px(10.0), roll.center().y),
            MouseButton::Left,
            Modifiers::default(),
        );
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before,
                "cancel cannot grow source"
            )
        });
        for (index, fraction) in [0.01, 0.0625, 0.125, 0.25, 0.5, 0.875]
            .into_iter()
            .enumerate()
        {
            let at = point(
                roll.origin.x + gpui::px(1.0) + (roll.size.width - gpui::px(2.0)) * fraction,
                roll.origin.y + roll.size.height * (index as f32 + 3.5) / 24.0,
            );
            let before = editor.read_with(&visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .clone()
            });
            if draw {
                visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
            } else {
                double_down(&mut visual, at, Modifiers::default());
            }
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                    &before,
                    "preview cannot grow source"
                )
            });
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
            let authored = editor.read_with(&visual, |v, _| {
                let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
                assert_eq!(
                    source.notes().len(),
                    index + 1,
                    "visible time cell {index}, draw={draw}"
                );
                let expected = [0, 1, 2, 4, 8, 14][index] * (BeatTime::ONE.raw() / 4);
                assert_eq!(
                    source.notes()[index].start().raw(),
                    expected,
                    "must not alias final source cell"
                );
                assert_eq!(source.loop_end(), half);
                assert_eq!(
                    v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .midi_clip(clip)
                        .unwrap()
                        .arrangement_length(),
                    arrangement
                );
                source.clone()
            });
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                    &before
                )
            });
            visual.simulate_keystrokes("cmd-shift-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                    &authored
                )
            });
        }
    }
}
