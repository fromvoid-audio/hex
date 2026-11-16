//! Real loop-brace input, canonical ACK/history and saved prepared-repeat proof.
use super::tests::{FileCustody, content_bounds, next, roundtrip};
use super::*;
use crate::editor::midi_canvas::creation_tests::{fixture, pump};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command};
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn at(v: &mut VisualTestContext, fraction: f32) -> gpui::Point<gpui::Pixels> {
    let b = content_bounds(v, "midi-loop-ruler");
    point(
        (b.origin.x + b.size.width * fraction).min(b.bottom_right().x - px(1.0)),
        b.origin.y + px(11.0),
    )
}
fn assert_brace(v: &mut VisualTestContext, first: f32, last: f32, on: bool) {
    let b = content_bounds(v, "midi-loop-ruler");
    let scale_factor = v.update(|window, _| window.scale_factor());
    let scale = gpui::ScaledPixels::from(scale_factor);
    let color: gpui::Background = gpui::Hsla::from(rgb(theme::color(if on {
        theme::PaletteRole::Text
    } else {
        theme::PaletteRole::Muted
    })))
    .opacity(if on { 0.94 } else { 0.62 })
    .into();
    let bars: Vec<_> = v
        .rendered_filled_quads("midi-loop-ruler")
        .into_iter()
        .filter(|(b, c)| {
            c == &color && (b.size.height / scale - 3.0).abs() < 0.01 && b.size.width / scale > 6.0
        })
        .collect();
    assert_eq!(bars.len(), 1);
    let width = f32::from(b.size.width);
    // Both edges are rounded in physical pixels by layout and painting.
    let pixel = 1.0 / scale_factor;
    assert!(
        (bars[0].0.origin.x / scale - (f32::from(b.origin.x) + width * first)).abs() <= pixel,
        "content={b:?} quad={:?} scale={scale:?} first={first}",
        bars[0].0
    );
    assert!((bars[0].0.size.width / scale - width * (last - first)).abs() <= pixel);
}
#[gpui::test]
fn loop_endpoints_and_body_preview_then_ack_once_with_undo_redo_and_saved_repeats(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    for command in [
        Command::InsertInstrument {
            track: editor.read_with(&visual, |v, _| v.selected_track.unwrap()),
            state: hex_project::default_instrument_state().unwrap(),
        },
        Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end: beat(4),
            looping: false,
            length: beat(12),
        },
        Command::AddMidiNote {
            clip,
            start: BeatTime::from_ratio(5, 2).unwrap(),
            duration: BeatTime::from_ratio(1, 4).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 93,
        },
    ] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(command)))
        });
        pump(&editor, &mut visual);
    }
    for (from, to, first, last) in [(0.0, 0.25, 1, 4), (1.0, 0.75, 1, 3), (0.5, 0.75, 2, 4)] {
        let before = editor.read_with(&visual, |v, _| {
            (
                v.state.as_ref().unwrap().project.snapshot(),
                v.state.as_ref().unwrap().revision,
                v.midi_marker,
                v.offline_cursor,
            )
        });
        let down = at(&mut visual, from);
        let up = at(&mut visual, to);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.0);
            let p = v.midi_ruler.gesture.unwrap().preview;
            assert_eq!(
                (p.loop_start, p.loop_end),
                (beat(first).raw(), beat(last).raw())
            );
            assert!(!p.looping);
        });
        assert_brace(&mut visual, first as f32 / 4.0, last as f32 / 4.0, false);
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        let terminal = crate::editor::clip_editor_loop_ui_tests::hold(&editor, &mut visual);
        assert!(
            terminal.request_complete && terminal.error.is_none(),
            "{:?}",
            terminal.error
        );
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
            assert_brace(&mut visual, first as f32 / 4.0, last as f32 / 4.0, false);
        }
        editor.update(&mut visual, |v, cx| {
            v.receive(terminal);
            cx.notify();
        });
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let s = v.state.as_ref().unwrap();
            let c = s.project.midi_clip(clip).unwrap();
            assert_eq!(s.revision, before.1 + 1);
            assert_eq!((c.loop_start(), c.loop_end()), (beat(first), beat(last)));
            assert_eq!(
                (c.source_start(), c.source_length(), c.arrangement_length()),
                (beat(0), beat(4), beat(12))
            );
            assert_eq!(c.notes().len(), 1);
            assert_eq!(c.notes()[0].velocity(), 93);
            assert_eq!((v.midi_marker, v.offline_cursor), (before.2, before.3));
            s.project.snapshot()
        });
        for _ in 0..3 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        assert_brace(&mut visual, first as f32 / 4.0, last as f32 / 4.0, false);
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
    }
    let toggle = visual.debug_bounds("midi-loop-ruler-toggle").unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_brace(&mut visual, 0.5, 1.0, true);
    let after = editor.read_with(&visual, |v, _| {
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .looping()
        );
        v.state.as_ref().unwrap().project.snapshot()
    });
    assert_eq!(
        roundtrip(&editor, &mut visual, &environment).snapshot(),
        after
    );
    // Read the real saved source through the existing worker; its accepted
    // Playback is rendered headlessly, never through a native output device.
    let service = hex_session::service::Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    let mut custody = FileCustody {
        environment: std::sync::Arc::clone(&environment),
        acknowledged: false,
    };
    service
        .submit(Request::Open(environment.root().join("source-start.hex")))
        .unwrap();
    let mut opened = next(&service);
    custody.acknowledged = true;
    let project = &opened.state.project;
    let mut playback = opened
        .playback
        .take()
        .expect("accepted prepared source playback");
    for (position, audible) in [
        (BeatTime::from_ratio(5, 2).unwrap(), true),
        (BeatTime::from_ratio(7, 2).unwrap(), false),
        (BeatTime::from_ratio(9, 2).unwrap(), true),
    ] {
        let frame = project.tempo_map().beat_to_frame(position).unwrap();
        playback.apply(hex_engine::PlaybackCommand::PlayFrom(frame));
        let mut output = [[0.0; 2]; 128];
        playback.render(&mut output).unwrap();
        let energy: f64 = output.iter().flatten().map(|x| f64::from(*x).powi(2)).sum();
        assert_eq!(
            energy > 1e-8,
            audible,
            "loop repeat at {}: {energy}",
            position.raw()
        );
    }
}
#[gpui::test]
fn disabled_loop_keeps_markers_and_refuses_invalid_enable_without_history(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(3),
            loop_start: beat(1),
            loop_end: beat(2),
            looping: false,
            length: beat(4)
        })))
    });
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.state.as_ref().unwrap().revision,
        )
    });
    assert_brace(&mut visual, 0.25, 0.5, false);
    let toggle = visual.debug_bounds("midi-loop-ruler-toggle").unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (
                v.state.as_ref().unwrap().project.snapshot(),
                v.state.as_ref().unwrap().revision
            ),
            before
        );
        assert!(
            v.error
                .as_ref()
                .is_some_and(|error| error.contains("invalid"))
        );
    });
    let down = at(&mut visual, 0.5);
    let up = at(&mut visual, 1.0);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let s = v.state.as_ref().unwrap().project.midi_source(clip).unwrap();
        assert!(!s.looping());
        assert_eq!(
            (s.loop_start(), s.loop_end(), s.source_start()),
            (beat(1), beat(4), beat(3))
        );
    });
    visual.simulate_click(toggle.center(), Modifiers::default());
    pump(&editor, &mut visual);
    assert_brace(&mut visual, 0.25, 1.0, true);
}
#[gpui::test]
fn loop_capture_cancel_and_stale_frame_preserve_source_and_transport(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    for cancel in 0..3 {
        let down = at(&mut visual, 1.0);
        let up = at(&mut visual, 0.5);
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(up, Some(MouseButton::Left), Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.midi_ruler.gesture.unwrap().preview.loop_end,
                beat(2).raw()
            )
        });
        match cancel {
            0 => visual.simulate_keystrokes("escape"),
            1 => visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window))),
            _ => {
                editor.update(&mut visual, |v, cx| {
                    v.midi_snap_grid = v.midi_snap_grid.next();
                    cx.notify();
                });
            }
        };
        visual.run_until_parked();
        assert_brace(&mut visual, 0.0, 1.0, false);
        visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
    let down = at(&mut visual, 1.0);
    visual.update(|window, cx| {
        editor.update(cx, |v, _| v.project_generation += 1);
        window.dispatch_input(
            gpui::PlatformInput::MouseDown(MouseDownEvent {
                position: down,
                button: MouseButton::Left,
                click_count: 1,
                modifiers: Modifiers::default(),
                first_mouse: false,
            }),
            cx,
        );
        assert!(editor.read(cx).midi_ruler.gesture.is_none());
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .unwrap()
                .loop_end(),
            beat(4)
        );
        assert!(v.last_transport_command.is_none());
    });
}

#[gpui::test]
fn console_loop_resize_enable_and_roundtrip_leave_arrangement_source_unchanged(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, environment, arrangement) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "loop scene".into()
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
    let down = at(&mut visual, 1.0);
    let up = at(&mut visual, 0.5);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.midi_source(clip).unwrap().loop_end(), beat(2));
        assert_eq!(p.midi_source(arrangement).unwrap().loop_end(), beat(4));
        p.snapshot()
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
    let toggle = visual.debug_bounds("midi-loop-ruler-toggle").unwrap();
    visual.simulate_click(toggle.center(), Modifiers::default());
    pump(&editor, &mut visual);
    let saved = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert!(p.midi_source(clip).unwrap().looping());
        assert!(!p.midi_source(arrangement).unwrap().looping());
        p.snapshot()
    });
    assert_eq!(
        roundtrip(&editor, &mut visual, &environment).snapshot(),
        saved
    );
}
