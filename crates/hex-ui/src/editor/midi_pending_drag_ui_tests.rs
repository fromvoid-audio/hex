//! Rendered released geometry with the real worker terminal deliberately held.
use super::super::EditorView;
use super::creation_tests::{fixture, pump};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::{BeatTime, ClipId, Command, MidiNoteInsert};
use hex_session::service::{Event, Request};
use std::time::Instant;

fn beat(value: f64) -> BeatTime {
    BeatTime::from_raw((value * BeatTime::ONE.raw() as f64) as u64)
}
fn position(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    beat: f64,
    pitch: u8,
) -> gpui::Point<gpui::Pixels> {
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    editor.read_with(visual, |v, _| {
        point(
            roll.origin.x
                + px(1.0)
                + (roll.size.width - px(2.0))
                    * ((beat * BeatTime::ONE.raw() as f64 - v.piano_beat_start as f64)
                        / v.piano_beat_span.raw() as f64) as f32,
            roll.origin.y
                + px(1.0)
                + (roll.size.height - px(2.0))
                    * ((f32::from(v.piano_pitch_low + v.piano_pitch_rows - 1 - pitch) + 0.5)
                        / f32::from(v.piano_pitch_rows)),
        )
    })
}
fn bodies(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Vec<[i32; 4]> {
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    let rows = editor.read_with(&*visual, |v, _| v.piano_pitch_rows);
    let body_height = (f32::from(roll.size.height) - 2.0) / f32::from(rows) - 2.0;
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let primary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Primary)).into();
    let secondary: gpui::Background =
        gpui::rgb(crate::theme::color(crate::theme::PaletteRole::Secondary)).into();
    let mut result: Vec<_> = visual
        .rendered_filled_quads("midi-note-roll")
        .into_iter()
        .filter(|(b, c)| {
            (c == &primary || c == &secondary) && (b.size.height / scale - body_height).abs() < 0.01
        })
        .map(|(b, _)| {
            [
                b.origin.x / scale,
                b.origin.y / scale,
                b.size.width / scale,
                b.size.height / scale,
            ]
            .map(|x| (x * 100.0).round() as i32)
        })
        .collect();
    result.sort_unstable();
    result
}
fn terminal(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = editor.read_with(&*visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                return event;
            }
            editor.update(&mut *visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        }
        assert!(Instant::now() < deadline, "real worker terminal");
        std::thread::yield_now();
    }
}
fn repaint(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    editor.update(&mut *visual, |_, cx| cx.notify());
    visual.run_until_parked();
}
fn seed(editor: &Entity<EditorView>, visual: &mut VisualTestContext, clip: ClipId) {
    editor.update(&mut *visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: [(0.5, 0.5, 60), (1.5, 0.5, 64), (2.0, 1.5, 60)]
                    .into_iter()
                    .map(|(start, duration, pitch)| MidiNoteInsert {
                        active: true,
                        start: beat(start),
                        duration: beat(duration),
                        pitch,
                        channel: 2,
                        velocity: 93,
                    })
                    .collect(),
            }))
        )
    });
    pump(editor, visual);
}

#[gpui::test]
fn released_note_move_copy_and_both_resize_edges_keep_geometry_through_real_terminal(
    cx: &mut TestAppContext,
) {
    // Single/group move, both edge owners and Option-copy all share the same
    // release-to-terminal custody. The long note also exercises overwrite victims.
    for (index, (group, copy, edge, delta)) in [
        (false, false, 0, 2.0),
        (false, false, -1, 0.25),
        (false, false, 1, 1.5),
        (true, false, 0, 1.5),
        (true, false, -1, 0.25),
        (true, false, 1, 0.5),
        (false, true, 0, 2.0),
        (true, true, 0, 1.5),
        (false, false, 0, 4.5),
        (true, false, 0, 4.5),
    ]
    .into_iter()
    .enumerate()
    {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        seed(&editor, &mut visual, clip);
        let beyond_source = delta > 4.0;
        let span = if beyond_source { 8.0 } else { 4.0 };
        let before = editor.update(&mut visual, |v, cx| {
            v.piano_beat_span = beat(span);
            let source = v
                .state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone();
            if group {
                v.midi_selection
                    .select_all(source.notes()[..2].iter().map(|n| n.id()))
                    .unwrap();
            } else {
                v.midi_selection.set_single(source.notes()[0].id());
            }
            cx.notify();
            source
        });
        visual.run_until_parked();
        let mut down = position(
            &editor,
            &mut visual,
            if edge < 0 {
                0.5
            } else if edge > 0 {
                1.0
            } else {
                0.75
            },
            60,
        );
        if edge < 0 {
            down.x += px(1.0);
        } else if edge > 0 {
            down.x -= px(1.0);
        }
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let end = down + point((roll.size.width - px(2.0)) * (delta / span) as f32, px(0.0));
        let modifiers = Modifiers {
            alt: copy,
            ..Default::default()
        };
        let baseline = bodies(&editor, &mut visual);
        visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
        visual.simulate_mouse_move(end, Some(MouseButton::Left), modifiers);
        visual.run_until_parked();
        let held = bodies(&editor, &mut visual);
        assert_ne!(
            held, baseline,
            "case {index} must actually move/resize/copy"
        );
        if index == 0 || beyond_source {
            editor.update(&mut visual, |v, _| {
                let tuning = v.state.as_ref().unwrap().project.tuning();
                v.tuning.client.begin(v.project_generation, tuning).unwrap();
                v.tuning.client.change(tuning.with_enabled(true)).unwrap();
                v.tuning.client.finish(tuning.with_enabled(true)).unwrap();
            });
        }
        visual.simulate_mouse_up(end, MouseButton::Left, modifiers);
        repaint(&editor, &mut visual);
        assert_eq!(
            bodies(&editor, &mut visual),
            held,
            "case {index}: release cannot paint original positions"
        );
        editor.read_with(&visual, |v, _| {
            assert!(
                v.midi_note_gesture.is_none() && v.midi_group_gesture.is_none(),
                "input capture has ended"
            );
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before,
                "UI has not received the canonical terminal"
            );
        });
        if index == 0 || beyond_source {
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.pending_requests.len(),
                    1,
                    "real tuning terminal precedes queued note edit"
                );
                let command = v.last_enqueued_edit.clone().unwrap();
                assert!(v.pending_midi_drag_matches(&Request::Edit(command.clone())));
                assert!(!v.pending_midi_drag_matches(&Request::Undo));
                if beyond_source {
                    let Command::UpdateMidiNotesWithSourceExtent { clip, mut notes } = command
                    else {
                        panic!("out-of-source move must grow canonical source extent");
                    };
                    assert!(notes.iter().all(|note| note.start >= beat(5.0)));
                    notes[0].velocity -= 1;
                    assert!(!v.pending_midi_drag_matches(&Request::Edit(
                        Command::UpdateMidiNotesWithSourceExtent { clip, notes }
                    )));
                    return;
                }
                let Command::UpdateMidiNote {
                    note,
                    start,
                    duration,
                    pitch,
                    channel,
                    velocity,
                } = command
                else {
                    panic!("single move");
                };
                assert!(
                    !v.pending_midi_drag_matches(&Request::Edit(Command::UpdateMidiNote {
                        note,
                        start,
                        duration,
                        pitch,
                        channel,
                        velocity: velocity - 1
                    }))
                );
            });
            let deadline = crate::test_wait::hang_deadline();
            while editor.read_with(&visual, |v, _| !v.pending_requests.is_empty()) {
                editor.update(&mut visual, |v, cx| {
                    v.tick(cx);
                    cx.notify();
                });
                visual.run_until_parked();
                assert_eq!(
                    bodies(&editor, &mut visual),
                    held,
                    "earlier tuning completion cannot retire note display"
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "released_note_move_copy_and_both_resize_edges_keep_geometry_through_real_terminal: hang guard expired"
                );
                std::thread::yield_now();
            }
        }
        let event = terminal(&editor, &mut visual);
        assert!(event.error.is_none(), "case {index}: {:?}", event.error);
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(bodies(&editor, &mut visual), held);
        }
        // Blur/Escape after submission cannot cancel the already admitted edit.
        visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert_eq!(
                bodies(&editor, &mut visual),
                held,
                "case {index}: accepted canonical geometry and later frames agree"
            );
        }
        let accepted = editor.read_with(&visual, |v, _| {
            assert!(v.pending_midi_drag.is_none());
            let project = &v.state.as_ref().unwrap().project;
            let source = project.midi_source(clip).unwrap();
            if beyond_source {
                let window = project.midi_clip(clip).unwrap();
                assert_eq!((window.start(), window.length()), (beat(0.0), beat(4.0)));
                assert_eq!(source.source_start(), before.source_start());
                assert_eq!(source.loop_start(), before.loop_start());
                assert_eq!(source.loop_end(), before.loop_end());
                assert_eq!(source.source_length(), beat(if group { 6.5 } else { 5.5 }));
                for original in before.notes().iter().take(if group { 2 } else { 1 }) {
                    let moved = source
                        .notes()
                        .iter()
                        .find(|n| n.id() == original.id())
                        .unwrap();
                    assert_eq!(
                        moved.start().raw(),
                        original.start().raw() + beat(delta).raw()
                    );
                    assert_eq!(
                        (moved.duration(), moved.velocity(), moved.channel()),
                        (original.duration(), original.velocity(), original.channel())
                    );
                }
            }
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone()
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before,
                "one Undo restores the complete source"
            )
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &accepted
            )
        });
    }
}

#[gpui::test]
fn refused_note_resize_retires_pending_geometry_only_on_real_failure_terminal(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    seed(&editor, &mut visual, clip);
    let before = editor.update(&mut visual, |v, cx| {
        v.midi_snap_grid = super::midi_drag::MidiGrid::Off;
        let source = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(clip)
            .unwrap()
            .clone();
        v.midi_selection.set_single(source.notes()[0].id());
        cx.notify();
        source
    });
    visual.run_until_parked();
    let down = position(&editor, &mut visual, 1.0, 60) - point(px(1.0), px(0.0));
    let end = position(&editor, &mut visual, 0.25, 60);
    let baseline = bodies(&editor, &mut visual);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let held = bodies(&editor, &mut visual);
    assert_ne!(held, baseline);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let event = terminal(&editor, &mut visual);
    assert!(
        event.error.is_some(),
        "sub-frame free resize must fail strict playback preparation"
    );
    for _ in 0..3 {
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    repaint(&editor, &mut visual);
    assert_eq!(bodies(&editor, &mut visual), baseline);
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_midi_drag.is_none());
        assert_eq!(
            v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
            &before
        );
    });
}

#[gpui::test]
fn held_note_cancel_creates_no_released_custody_or_history(cx: &mut TestAppContext) {
    for blur in [false, true] {
        let (mut visual, editor, _environment, clip) = fixture(cx);
        seed(&editor, &mut visual, clip);
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_span = beat(8.0);
            cx.notify();
        });
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .clone()
        });
        let down = position(&editor, &mut visual, 0.75, 60);
        let end = position(&editor, &mut visual, 5.25, 60);
        let baseline = bodies(&editor, &mut visual);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        if blur {
            visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
        } else {
            visual.simulate_keystrokes("escape");
        }
        visual.run_until_parked();
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), baseline);
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_midi_drag.is_none());
            assert!(!v.edit_pending());
            assert_eq!(
                v.state.as_ref().unwrap().project.midi_source(clip).unwrap(),
                &before
            );
        });
    }
}

#[gpui::test]
fn console_note_release_uses_its_own_source_identity_through_delayed_terminal(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, arrangement) = fixture(cx);
    seed(&editor, &mut visual, arrangement);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "pending console".into()
        })))
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                source: arrangement,
                scene
            }))
        )
    });
    pump(&editor, &mut visual);
    let (clip, before) = editor.update(&mut visual, |v, cx| {
        let track = v.selected_track.unwrap();
        let hex_project::SessionCellContent::Midi(clip) = v
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi cell");
        };
        let clip = clip.id();
        let before = v
            .state
            .as_ref()
            .unwrap()
            .project
            .midi_source(arrangement)
            .unwrap()
            .clone();
        v.mode = super::super::EditorMode::Session;
        v.selected_scene = Some(scene);
        v.selected_cell = Some((track, scene));
        v.selected_clip = Some(clip);
        v.piano_view_clip = None;
        cx.notify();
        (clip, before)
    });
    visual.run_until_parked();
    let down = position(&editor, &mut visual, 0.75, 60);
    let end = position(&editor, &mut visual, 1.25, 60);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.run_until_parked();
    let held = bodies(&editor, &mut visual);
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let event = terminal(&editor, &mut visual);
    assert!(event.error.is_none(), "{:?}", event.error);
    for _ in 0..3 {
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    repaint(&editor, &mut visual);
    assert_eq!(bodies(&editor, &mut visual), held);
    editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        assert_eq!(project.midi_source(arrangement).unwrap(), &before);
        assert_ne!(project.midi_source(clip).unwrap(), &before);
        assert!(v.pending_midi_drag.is_none());
    });
    editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .notes()[0]
                .start(),
            beat(0.5)
        )
    });
}

#[gpui::test]
fn new_note_creation_keeps_released_geometry_until_its_real_terminal(cx: &mut TestAppContext) {
    for grows in [false, true] {
        let cx = &mut cx.new_app();
        let (mut visual, editor, _environment, clip) = fixture(cx);
        if grows {
            editor.update(&mut visual, |v, cx| {
                v.piano_beat_start = i128::from(beat(3.0).raw());
                cx.notify();
            });
            repaint(&editor, &mut visual);
        }
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let start = if grows { 4.0 } else { 1.0 };
        let down = position(&editor, &mut visual, start, 60);
        let end = position(&editor, &mut visual, start + 1.0, 60);
        super::creation_tests::double_down(&mut visual, down, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        repaint(&editor, &mut visual);
        let held = bodies(&editor, &mut visual);
        assert_eq!(held.len(), 1);
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        let event = terminal(&editor, &mut visual);
        assert!(event.error.is_none());
        for _ in 0..3 {
            repaint(&editor, &mut visual);
            assert!(
                !visual.update(|w, _| w.is_pointer_dragging()),
                "pending geometry does not retain input"
            );
            assert_eq!(
                bodies(&editor, &mut visual),
                held,
                "grows={grows}: released creation remains painted before ACK"
            );
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert!(v.midi_creation.is_none(), "input ends at release");
            });
        }
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        repaint(&editor, &mut visual);
        assert_eq!(bodies(&editor, &mut visual), held);
        editor.read_with(&visual, |v, _| {
            assert!(!v.edit_pending());
            assert!(
                v.pending_midi_drag.is_none(),
                "exact terminal retires display custody"
            );
            let source = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
            assert_eq!(source.notes().len(), 1);
            assert_eq!(source.notes()[0].start(), beat(start));
            assert_eq!(source.notes()[0].duration(), BeatTime::ONE);
            assert_eq!(source.source_length(), beat(if grows { 5.0 } else { 4.0 }));
        });
    }
}

#[gpui::test]
#[ignore = "opt-in held/released creation repaint CPU probe"]
fn new_note_creation_repaint_latency(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _) = fixture(cx);
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    visual.run_until_parked();
    let down = position(&editor, &mut visual, 1., 60);
    let end = position(&editor, &mut visual, 2., 60);
    super::creation_tests::double_down(&mut visual, down, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    repaint(&editor, &mut visual);
    assert_eq!(bodies(&editor, &mut visual).len(), 1);
    let mut reply = None;
    for phase in ["held", "released"] {
        if phase == "released" {
            visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
            let event = terminal(&editor, &mut visual);
            assert!(event.error.is_none());
            reply = Some(event);
        }
        let mut samples = Vec::new();
        for n in 0..144 {
            let start = Instant::now();
            repaint(&editor, &mut visual);
            if n >= 16 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        let count = bodies(&editor, &mut visual).len();
        println!(
            "creation_repaint_us phase={phase} p50={} p95={} max={} bodies={count} window=1280x800 tracks=1 clips=1 canonical_notes=0 warmups=16 samples=128",
            samples[64], samples[121], samples[127]
        );
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(reply.unwrap());
        cx.notify();
    });
}

#[path = "midi_creation_capture_tests.rs"]
mod creation_capture_tests;

#[path = "midi_note_capture_tests.rs"]
mod note_capture_tests;

#[path = "midi_draw_preview_ui_tests.rs"]
mod stroke_preview_tests;

#[path = "midi_source_prefix_ui_tests.rs"]
mod source_prefix_tests;
