//! Actual pointer, completed canvas fragments, real worker and global history.
use super::super::{EditorView, test_environment::Environment};
use super::creation_tests::{fixture, pump};
use gpui::{
    Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px,
    size,
};
use hex_project::{BeatTime, ClipId, Command, MidiNoteInsert, NoteId};
use hex_session::service::Request;
use std::sync::Arc;

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}
fn half(value: u64) -> BeatTime {
    BeatTime::from_raw(BeatTime::ONE.raw() * value / 2)
}

struct Seed {
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    _environment: Arc<Environment>,
    clip: ClipId,
    old: NoteId,
    incoming: NoteId,
    pitch: u8,
}
fn seed(cx: &mut TestAppContext, vertical: bool) -> Seed {
    let (mut visual, editor, environment, clip) = fixture(cx);
    let pitch = editor.read_with(&visual, |v, _| {
        v.piano_pitch_low + (v.piano_pitch_rows - 1) / 2
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNotes {
            clip,
            notes: vec![
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::ZERO,
                    duration: beat(2),
                    pitch,
                    channel: 3,
                    velocity: 91
                },
                MidiNoteInsert {
                    active: true,
                    start: beat(3),
                    duration: half(1),
                    pitch: pitch + u8::from(vertical),
                    channel: 7,
                    velocity: 103
                },
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::ONE,
                    duration: beat(2),
                    pitch: pitch + 2,
                    channel: 1,
                    velocity: 77
                },
            ],
        })))
    });
    pump(&editor, &mut visual);
    let (old, incoming) = editor.update(&mut visual, |v, cx| {
        v.midi_selection.clear();
        cx.notify();
        let notes = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .notes();
        (notes[0].id(), notes[1].id())
    });
    visual.run_until_parked();
    Seed {
        visual,
        editor,
        _environment: environment,
        clip,
        old,
        incoming,
        pitch,
    }
}

fn bodies(visual: &mut VisualTestContext, y: Pixels) -> Vec<Bounds<Pixels>> {
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let primary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into();
    let secondary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into();
    let mut bodies: Vec<_> = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        .filter(|(_, color)| color == &primary || color == &secondary)
        .map(|(bounds, _)| {
            Bounds::new(
                point(px(bounds.origin.x / scale), px(bounds.origin.y / scale)),
                size(
                    px(bounds.size.width / scale),
                    px(bounds.size.height / scale),
                ),
            )
        })
        .filter(|bounds| {
            // These fixtures' note bodies exceed three pixels. Passive hover
            // paints a separate 1/3 px edge accent, not another note body.
            bounds.origin.y <= y
                && bounds.bottom() > y
                && bounds.size.width > px(3.0)
                && bounds.size.height > px(1.5)
                && bounds.size.height < roll.size.height / 4.0
        })
        .collect();
    bodies.sort_by(|a, b| f32::from(a.origin.x).total_cmp(&f32::from(b.origin.x)));
    bodies
}

fn pitch_y(visual: &mut VisualTestContext, editor: &Entity<EditorView>, pitch: u8) -> Pixels {
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let (low, rows) = editor.read_with(&*visual, |v, _| (v.piano_pitch_low, v.piano_pitch_rows));
    roll.origin.y
        + px(1.0)
        + (roll.size.height - px(2.0))
            * ((f32::from(rows - 1 - (pitch - low)) + 0.5) / f32::from(rows))
}

fn close(a: Pixels, b: Pixels) {
    assert!((f32::from(a - b)).abs() < 0.05, "{a:?} != {b:?}");
}

#[gpui::test]
fn held_move_and_leading_resize_paint_exact_prefix_then_real_ack_and_history(
    cx: &mut TestAppContext,
) {
    for (vertical, leading) in [(false, false), (true, false), (false, true)] {
        let Seed {
            mut visual,
            editor,
            _environment,
            clip,
            old,
            incoming,
            pitch,
        } = seed(cx, vertical);
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let old_y = pitch_y(&mut visual, &editor, pitch);
        let row =
            roll.size.height / f32::from(editor.read_with(&visual, |v, _| v.piano_pitch_rows));
        let baseline = bodies(&mut visual, old_y);
        assert_eq!(baseline.len(), if vertical { 1 } else { 2 });
        let old_rect = baseline[0];
        let incoming_rect = *bodies(&mut visual, old_y - if vertical { row } else { px(0.0) })
            .last()
            .unwrap();
        let unrelated = bodies(&mut visual, old_y - row * 2.0);
        assert_eq!(unrelated.len(), 1);
        let per_beat = old_rect.size.width / 2.0;
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let down = if leading {
            point(incoming_rect.origin.x + px(1.5), incoming_rect.center().y)
        } else {
            incoming_rect.center()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        let end = point(down.x - per_beat * 2.0, old_rect.center().y);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &before);
            assert!(!v.edit_pending());
            let gesture = v.midi_note_gesture.as_ref().expect("actual note gesture");
            assert_eq!(gesture.preview.start, BeatTime::ONE.raw());
            assert_eq!(gesture.preview.pitch, pitch);
        });
        let held = bodies(&mut visual, old_y);
        assert_eq!(held.len(), 2, "no old/source duplicate canvas bodies");
        close(held[0].origin.x, old_rect.origin.x);
        close(held[0].size.width, per_beat);
        close(held[1].origin.x, old_rect.origin.x + per_beat);
        close(
            held[1].size.width,
            per_beat * if leading { 2.5 } else { 0.5 },
        );
        assert_eq!(bodies(&mut visual, old_y - row * 2.0), unrelated);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let project = &v.state.as_ref().unwrap().project;
            assert_eq!(project.midi_note(old).unwrap().duration(), BeatTime::ONE);
            let moved = project.midi_note(incoming).unwrap();
            assert_eq!(moved.start(), BeatTime::ONE);
            assert_eq!(moved.duration(), if leading { half(5) } else { half(1) });
            assert_eq!(
                (moved.pitch(), moved.channel(), moved.velocity()),
                (pitch, 7, 103)
            );
            project.clone()
        });
        assert_eq!(bodies(&mut visual, old_y), held);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(bodies(&mut visual, old_y), held, "later completed repaint");
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip),
                before.midi_source(clip)
            )
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &after)
        });
        assert_eq!(bodies(&mut visual, old_y), held);
    }
}

#[gpui::test]
fn escape_and_real_stale_revision_restore_unclipped_canonical_notes(cx: &mut TestAppContext) {
    for stale in [false, true] {
        let Seed {
            mut visual,
            editor,
            _environment,
            clip,
            pitch,
            ..
        } = seed(cx, false);
        let y = pitch_y(&mut visual, &editor, pitch);
        let before_bodies = bodies(&mut visual, y);
        assert_eq!(before_bodies.len(), 2);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let down = before_bodies[1].center();
        let end = point(down.x - before_bodies[0].size.width, down.y);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.run_until_parked();
        assert_ne!(bodies(&mut visual, y), before_bodies);
        if stale {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::MasterGain { gain: 0.7 })))
            });
            pump(&editor, &mut visual);
        } else {
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
        }
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        assert_eq!(bodies(&mut visual, y), before_bodies);
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_note_gesture.is_none());
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip),
                before.midi_source(clip)
            );
        });
    }
}

#[gpui::test]
fn trailing_resize_and_group_move_paint_retirement_before_atomic_worker_commit(
    cx: &mut TestAppContext,
) {
    for group in [false, true] {
        let Seed {
            mut visual,
            editor,
            _environment,
            clip,
            old,
            incoming,
            pitch,
        } = seed(cx, false);
        let companion = if group {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::AddMidiNotes {
                    clip,
                    notes: vec![MidiNoteInsert {
                        active: true,
                        start: half(5),
                        duration: half(1),
                        pitch: pitch + 1,
                        channel: 5,
                        velocity: 87,
                    }],
                })))
            });
            pump(&editor, &mut visual);
            Some(editor.read_with(&visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .notes()[3]
                    .id()
            }))
        } else {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::UpdateMidiNotes {
                    clip,
                    notes: vec![
                        hex_project::MidiNoteUpdate {
                            note: old,
                            start: beat(2),
                            duration: half(3),
                            pitch,
                            channel: 3,
                            velocity: 91,
                        },
                        hex_project::MidiNoteUpdate {
                            note: incoming,
                            start: BeatTime::ZERO,
                            duration: half(1),
                            pitch,
                            channel: 7,
                            velocity: 103,
                        },
                    ],
                })))
            });
            pump(&editor, &mut visual);
            None
        };
        editor.update(&mut visual, |v, cx| {
            v.midi_selection.clear();
            if let Some(companion) = companion {
                v.midi_selection.select_all([incoming, companion]).unwrap();
            }
            cx.notify();
        });
        visual.run_until_parked();
        let y = pitch_y(&mut visual, &editor, pitch);
        let other_y = pitch_y(&mut visual, &editor, pitch + 2);
        let companion_y = pitch_y(&mut visual, &editor, pitch + 1);
        let baseline = bodies(&mut visual, y);
        assert_eq!(baseline.len(), 2);
        let per_beat = if group {
            baseline[0].size.width / 2.0
        } else {
            baseline[0].size.width * 2.0
        };
        let origin = baseline[0].origin.x;
        let unrelated = bodies(&mut visual, other_y);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let down = if group {
            baseline[1].center()
        } else {
            point(baseline[0].right() - px(1.5), baseline[0].center().y)
        };
        let end = point(down.x + per_beat * if group { -2.0 } else { 2.5 }, down.y);
        // A trailing resize snaps only while cmd is held; the group case moves.
        let held_modifiers = if group {
            Modifiers::default()
        } else {
            Modifiers::command()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), held_modifiers);
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &before);
            assert!(!v.edit_pending());
            if group {
                assert!(v.midi_group_gesture.is_some());
            }
        });
        let held = bodies(&mut visual, y);
        assert_eq!(held.len(), if group { 2 } else { 1 });
        close(held[0].origin.x, origin);
        close(held[0].size.width, per_beat * if group { 1.0 } else { 3.0 });
        let held_companion = bodies(&mut visual, companion_y);
        if group {
            close(held[1].origin.x, origin + per_beat);
            close(held[1].size.width, per_beat / 2.0);
            assert_eq!(held_companion.len(), 1);
            close(held_companion[0].origin.x, origin + per_beat / 2.0);
            close(held_companion[0].size.width, per_beat / 2.0);
        }
        assert_eq!(bodies(&mut visual, other_y), unrelated);
        visual.simulate_mouse_up(end, MouseButton::Left, held_modifiers);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            if let Some(companion) = companion {
                assert_eq!(p.midi_note(old).unwrap().duration(), beat(1));
                let c = p.midi_note(companion).unwrap();
                assert_eq!(
                    (
                        c.start(),
                        c.duration(),
                        c.pitch(),
                        c.channel(),
                        c.velocity()
                    ),
                    (half(1), half(1), pitch + 1, 5, 87)
                );
            } else {
                assert!(p.midi_note(old).is_none());
            }
            let n = p.midi_note(incoming).unwrap();
            assert_eq!(
                (
                    n.start(),
                    n.duration(),
                    n.pitch(),
                    n.channel(),
                    n.velocity()
                ),
                (
                    if group { beat(1) } else { BeatTime::ZERO },
                    if group { half(1) } else { beat(3) },
                    pitch,
                    7,
                    103
                )
            );
            p.clone()
        });
        assert_eq!(bodies(&mut visual, y), held);
        assert_eq!(bodies(&mut visual, companion_y), held_companion);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(bodies(&mut visual, y), held);
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip),
                before.midi_source(clip)
            )
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &after)
        });
        assert_eq!(bodies(&mut visual, y), held);
    }
}

#[gpui::test]
fn normal_and_draw_creation_paint_whole_old_tail_retirement_then_real_ack(cx: &mut TestAppContext) {
    for draw in [false, true] {
        let Seed {
            mut visual,
            editor,
            _environment,
            clip,
            old,
            incoming,
            pitch,
        } = seed(cx, false);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::RemoveMidiNote(incoming))))
        });
        pump(&editor, &mut visual);
        let old_start = if draw {
            BeatTime::from_raw(BeatTime::ONE.raw() * 5 / 8)
        } else {
            beat(1)
        };
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::UpdateMidiNote {
                note: old,
                start: old_start,
                duration: beat(2),
                pitch,
                channel: 3,
                velocity: 91,
            })))
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, cx| {
            v.midi_selection.clear();
            v.midi_draw_mode = draw;
            cx.notify();
        });
        visual.run_until_parked();
        let y = pitch_y(&mut visual, &editor, pitch);
        let unrelated_y = pitch_y(&mut visual, &editor, pitch + 2);
        let baseline = bodies(&mut visual, y);
        assert_eq!(baseline.len(), 1);
        let per_beat = baseline[0].size.width / 2.0;
        let origin = baseline[0].origin.x - per_beat * if draw { 0.625 } else { 1.0 };
        let unrelated = bodies(&mut visual, unrelated_y);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let down = point(origin + per_beat * if draw { 0.55 } else { 0.25 }, y);
        let end = if draw {
            down
        } else {
            point(origin + per_beat * 2.0, y)
        };
        if draw {
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        } else {
            super::creation_tests::double_down(&mut visual, down, Modifiers::default());
            visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        }
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &before);
            assert!(!v.edit_pending());
        });
        let held = bodies(&mut visual, y);
        assert_eq!(held.len(), 1, "no surviving old tail or duplicate body");
        close(
            held[0].origin.x,
            origin + per_beat * if draw { 0.5 } else { 0.25 },
        );
        close(
            held[0].size.width,
            per_beat * if draw { 0.25 } else { 1.75 },
        );
        assert_eq!(bodies(&mut visual, unrelated_y), unrelated);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert!(p.midi_note(old).is_none());
            let notes: Vec<_> = p
                .midi_source(clip)
                .unwrap()
                .notes()
                .iter()
                .filter(|n| n.pitch() == pitch)
                .collect();
            assert_eq!(notes.len(), 1);
            assert_ne!(notes[0].id(), old);
            assert_eq!(
                notes[0].start(),
                BeatTime::from_raw(BeatTime::ONE.raw() / if draw { 2 } else { 4 })
            );
            assert_eq!(
                notes[0].duration(),
                BeatTime::from_raw(BeatTime::ONE.raw() * if draw { 1 } else { 7 } / 4)
            );
            p.clone()
        });
        assert_eq!(bodies(&mut visual, y), held);
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(bodies(&mut visual, y), held);
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip),
                before.midi_source(clip)
            )
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(&v.state.as_ref().unwrap().project, &after)
        });
        assert_eq!(bodies(&mut visual, y), held);
    }
}
