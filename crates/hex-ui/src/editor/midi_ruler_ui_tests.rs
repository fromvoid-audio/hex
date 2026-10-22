//! Rendered source-marker capture and canonical worker/history/current-format proof.
use super::*;
use crate::editor::{
    midi_canvas::creation_tests::{fixture, pump},
    test_environment::Environment,
};
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command, MidiNoteInsert};
use hex_session::service::{Event, Service};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn marker_at(visual: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let b = visual.debug_bounds("midi-source-ruler").unwrap();
    point(
        b.origin.x + px(1.0) + (b.size.width - px(2.0)) * fraction,
        b.origin.y + px(13.0),
    )
}
fn assert_marker(visual: &mut VisualTestContext, fraction: f32) {
    let bounds = visual.debug_bounds("midi-source-ruler").unwrap();
    let scale = gpui::ScaledPixels::from(visual.update(|window, _| window.scale_factor()));
    let primary: gpui::Background = rgb(theme::color(theme::PaletteRole::Primary)).into();
    let quads: Vec<_> = visual
        .rendered_filled_quads("midi-source-ruler")
        .into_iter()
        .filter(|(b, c)| {
            c == &primary
                && (b.size.width / scale - 1.0).abs() < 0.01
                && b.size.height / scale > 10.0
        })
        .collect();
    assert_eq!(
        quads.len(),
        1,
        "one persisted/preview playback-start witness"
    );
    let expected =
        f32::from(bounds.origin.x) + 1.0 + (f32::from(bounds.size.width) - 2.0) * fraction;
    assert!((quads[0].0.origin.x / scale - expected).abs() < 0.05);
}
fn drag(visual: &mut VisualTestContext, from: f32, to: f32) {
    let down = marker_at(visual, from);
    let up = marker_at(visual, to);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
}
fn next(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}
struct FileCustody {
    environment: Arc<Environment>,
    acknowledged: bool,
}
impl Drop for FileCustody {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.environment.retain_for_unacknowledged_setup_failure();
        }
    }
}
fn roundtrip(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    environment: &Arc<Environment>,
) -> Arc<hex_project::Project> {
    let path = environment.root().join("source-start.hex");
    let mut custody = FileCustody {
        environment: Arc::clone(environment),
        acknowledged: false,
    };
    editor.update(&mut *visual, |v, _| {
        assert!(v.enqueue(Request::SaveNew(path.clone())))
    });
    pump(editor, visual);
    custody.acknowledged = true;
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    custody.acknowledged = false;
    service.submit(Request::Open(path)).unwrap();
    let event = next(&service);
    custody.acknowledged = true;
    event.state.project.clone()
}
#[gpui::test]
fn marker_drag_at_nonzero_arrangement_placement_acks_once_and_preserves_other_state(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::MoveMidiClip {
            clip,
            track: v.selected_track.unwrap(),
            start: beat(8)
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiNotes {
            clip,
            notes: vec![
                MidiNoteInsert {
                    start: beat(0),
                    duration: beat(1),
                    pitch: 60,
                    channel: 2,
                    velocity: 37
                },
                MidiNoteInsert {
                    start: beat(2),
                    duration: beat(1),
                    pitch: 64,
                    channel: 2,
                    velocity: 93
                },
            ]
        })))
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(0),
            loop_start: beat(1),
            loop_end: beat(3),
            looping: true,
            length: beat(4)
        })))
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.state.as_ref().unwrap().revision,
            v.midi_marker,
            v.offline_cursor,
        )
    });
    let down = marker_at(&mut visual, 0.0);
    let up = marker_at(&mut visual, 0.5);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.0);
        assert_eq!(v.midi_ruler.gesture.unwrap().preview, beat(2).raw());
        assert!(!v.edit_pending());
    });
    assert_marker(&mut visual, 0.5);
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let source = state.project.midi_source(clip).unwrap();
        assert_eq!(state.revision, before.1 + 1);
        assert_eq!(source.source_start(), beat(2));
        assert_eq!(source.source_length(), beat(4));
        assert_eq!(
            (source.loop_start(), source.loop_end(), source.looping()),
            (beat(1), beat(3), true)
        );
        assert_eq!(state.project.midi_clip(clip).unwrap().start(), beat(8));
        assert_eq!(
            state.project.midi_clip(clip).unwrap().arrangement_length(),
            beat(4)
        );
        assert_eq!(
            source
                .notes()
                .iter()
                .map(|n| (n.pitch(), n.channel(), n.velocity()))
                .collect::<Vec<_>>(),
            vec![(60, 2, 37), (64, 2, 93)]
        );
        assert_eq!((v.midi_marker, v.offline_cursor), (before.2, before.3));
        assert!(v.last_transport_command.is_none());
        let events = hex_engine::midi_compile::midi_events(&state.project).unwrap();
        assert_eq!(events[0].pitch, 64);
        assert_eq!(
            events[0].frame,
            state.project.tempo_map().beat_to_frame(beat(8)).unwrap()
        );
        state.project.snapshot()
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    assert_marker(&mut visual, 0.5);
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.0)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
    assert_eq!(
        roundtrip(&editor, &mut visual, &environment).snapshot(),
        after
    );
}
#[gpui::test]
fn session_marker_uses_its_own_source_and_blank_click_and_unchanged_drag_are_noops(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, environment, arrangement) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "start scene".into()
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
    let clip = editor.update(&mut visual, |v, cx| {
        let track = v.selected_track.unwrap();
        let hex_project::SessionCellContent::Midi(cell) = v
            .state
            .as_ref()
            .unwrap()
            .project
            .session_cell(track, scene)
            .unwrap()
        else {
            panic!("midi")
        };
        let clip = cell.id();
        v.mode = crate::editor::EditorMode::Session;
        v.selected_scene = Some(scene);
        v.selected_cell = Some((track, scene));
        v.selected_clip = Some(clip);
        v.piano_view_clip = None;
        cx.notify();
        clip
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let blank = marker_at(&mut visual, 0.5);
    visual.simulate_click(blank, Modifiers::default());
    let same = marker_at(&mut visual, 0.0);
    visual.simulate_click(same, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    drag(&mut visual, 0.0, 0.25);
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.midi_source(clip).unwrap().source_start(), beat(1));
        assert_eq!(p.midi_source(arrangement).unwrap().source_start(), beat(0));
        assert!(v.last_transport_command.is_none());
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    assert_eq!(
        roundtrip(&editor, &mut visual, &environment).snapshot(),
        after
    );
}
#[gpui::test]
fn marker_cancels_on_escape_focus_viewport_competing_ack_and_clip_retirement(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for cancel in 0..5 {
        editor.update(&mut visual, |v, cx| {
            v.piano_beat_start = BeatTime::ZERO;
            v.piano_beat_span = beat(4);
            cx.notify();
        });
        visual.run_until_parked();
        let down = marker_at(&mut visual, 0.0);
        let up = marker_at(&mut visual, 0.5);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
        editor.read_with(&visual, |v, _| assert!(v.midi_ruler.gesture.is_some()));
        match cancel {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window))),
            2 => {
                editor.update(&mut visual, |v, cx| {
                    assert!(v.zoom_piano_wheel_at([0.0, 0.0, 400.0, 180.0], 200.0, -80.0));
                    cx.notify();
                });
            }
            3 => {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(Command::RenameTrack {
                        track: v.selected_track.unwrap(),
                        name: "new context".into()
                    })))
                });
                pump(&editor, &mut visual);
            }
            _ => {
                editor.update(&mut visual, |v, cx| {
                    v.selected_clip = None;
                    cx.notify();
                });
            }
        };
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert!(v.midi_ruler.gesture.is_none()));
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            if cancel < 3 {
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            }
            assert_eq!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .midi_source(clip)
                    .unwrap()
                    .source_start(),
                beat(0)
            )
        });
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .source_length(),
            beat(4)
        );
        assert_eq!(v.midi_marker, Some(0));
    });
}

#[gpui::test]
fn stale_rendered_generation_cannot_capture_a_start_marker(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let bounds = visual.debug_bounds("midi-source-ruler").unwrap();
    editor.update(&mut visual, |v, _| {
        let b = CanvasBounds {
            x: f32::from(bounds.origin.x) + 1.0,
            y: f32::from(bounds.origin.y) + 1.0,
            width: f32::from(bounds.size.width) - 2.0,
            height: f32::from(bounds.size.height) - 2.0,
        };
        let rendered = v.midi_ruler_context(b).unwrap();
        v.project_generation += 1;
        assert!(!v.begin_midi_start(rendered, b.x));
        assert!(v.midi_ruler.gesture.is_none());
        assert!(!v.edit_pending());
    });
}
