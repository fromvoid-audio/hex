//! Actual key/button dispatch proves view-only fitting and owner-scoped Back.
use super::*;
use crate::editor::{
    midi_canvas::creation_tests::{fixture, pump},
    time_selection::TimeSelection,
};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use hex_project::Command;
use hex_session::service::Request;
fn click(v: &mut VisualTestContext, id: &'static str) {
    let b = v.debug_bounds(id).unwrap();
    v.simulate_click(b.center(), Modifiers::default());
    v.run_until_parked();
}
#[gpui::test]
fn arrange_keys_and_buttons_fit_reversed_range_clip_content_and_restore_exact_prior(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let prior = Some(Viewport {
        start: 96_000,
        length: 192_000,
    });
    let before = editor.update(&mut visual, |v, cx| {
        v.viewport = prior;
        let track = v.selected_track.unwrap();
        v.time_selection = Some(TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: 96_000,
            end_frame: 48_000,
        });
        cx.notify();
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.state.as_ref().unwrap().can_undo,
            v.state.as_ref().unwrap().can_redo,
            v.offline_cursor,
        )
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, _| {
            v.focus_track_header_after_navigation(v.selected_track.unwrap(), window);
        })
    });
    visual.simulate_keystrokes("z");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.viewport,
            Some(Viewport {
                start: 48_000,
                length: 48_000
            })
        )
    });
    visual.simulate_keystrokes("z");
    click(&mut visual, "arrange-zoom-back");
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport, prior));
    editor.update(&mut visual, |v, cx| {
        v.time_selection = None;
        cx.notify();
    });
    visual.run_until_parked();
    click(&mut visual, "arrange-zoom-fit");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.viewport,
            Some(Viewport {
                start: 0,
                length: 96_000
            })
        )
    });
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport, prior));
    editor.update(&mut visual, |v, cx| {
        v.selected_clip = None;
        cx.notify();
    });
    visual.run_until_parked();
    click(&mut visual, "arrange-zoom-fit");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.viewport,
            Some(Viewport {
                start: 0,
                length: 96_000
            })
        );
        let s = v.state.as_ref().unwrap();
        assert_eq!(
            (
                s.project.snapshot(),
                s.can_undo,
                s.can_redo,
                v.offline_cursor
            ),
            before
        );
        assert!(s.project.midi_source(clip).is_some());
    });
    editor.update(&mut visual, |v, cx| {
        v.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    let fitted = editor.read_with(&visual, |v, _| v.viewport);
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.viewport, fitted);
        assert!(v.zoom_fit.arrange.is_none());
    });
}
#[gpui::test]
fn midi_ruler_selection_fit_back_geometry_ack_and_document_reset_are_view_only(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    let b = visual.debug_bounds("midi-source-ruler").unwrap();
    let at = |f| {
        point(
            b.origin.x + px(1.0) + (b.size.width - px(2.0)) * f,
            b.center().y,
        )
    };
    visual.simulate_mouse_down(at(0.25), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(at(0.75), Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(at(0.75), MouseButton::Left, Modifiers::default());
    let before = editor.read_with(&visual, |v, _| {
        (
            v.state.as_ref().unwrap().project.snapshot(),
            v.midi_marker,
            v.piano_pitch_low,
            v.piano_pitch_rows,
        )
    });
    visual.simulate_keystrokes("z");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (v.piano_beat_start.raw(), v.piano_beat_span.raw()),
            (BeatTime::ONE.raw(), 2 * BeatTime::ONE.raw())
        );
        assert_eq!(
            v.midi_time_selection_bounds(),
            Some((BeatTime::ONE.raw(), 3 * BeatTime::ONE.raw()))
        );
    });
    visual.simulate_keystrokes("z");
    let roll = visual.debug_bounds("midi-note-roll").unwrap();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: roll.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(80.0))),
        ..Default::default()
    });
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        v.piano_pitch_low = 36;
        v.piano_pitch_rows = 12;
        cx.notify();
    });
    visual.run_until_parked();
    click(&mut visual, "midi-zoom-back");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (
                v.piano_beat_start.raw(),
                v.piano_beat_span.raw(),
                v.piano_pitch_low,
                v.piano_pitch_rows
            ),
            (0, 4 * BeatTime::ONE.raw(), before.2, before.3)
        )
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before.0)
    });
    // A canonical unrelated ACK retains both manual view and the pending Back owner.
    click(&mut visual, "midi-zoom-fit");
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RenameTrack {
            track: v.selected_track.unwrap(),
            name: "fit ack".into()
        })))
    });
    pump(&editor, &mut visual);
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (v.piano_beat_start.raw(), v.piano_beat_span.raw()),
            (0, 4 * BeatTime::ONE.raw())
        )
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.midi_marker, before.1);
        assert!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .midi_source(clip)
                .is_some()
        );
    });
    click(&mut visual, "midi-zoom-fit");
    editor.update(&mut visual, |v, cx| {
        v.project_generation += 1;
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| assert!(v.zoom_fit.midi.is_none()));
}
#[gpui::test]
fn text_numeric_modal_and_musical_typing_keep_z_x_from_navigation(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        v.viewport = Some(Viewport {
            start: 48_000,
            length: 96_000,
        });
    });
    let original = editor.read_with(&visual, |v, _| v.viewport);
    let browser = editor.read_with(&visual, |v, cx| {
        v.browser.search.read(cx).focus_handle().clone()
    });
    visual.update(|window, _| browser.focus(window));
    let search = editor.read_with(&visual, |v, _| v.browser.search.clone());
    visual.update(|window, cx| {
        search.update(cx, |input, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(
                input,
                None,
                "z",
                Some(0..1),
                window,
                cx,
            );
            assert!(gpui::EntityInputHandler::marked_text_range(input, window, cx).is_some());
        })
    });
    visual.simulate_keystrokes("z x");
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport, original));
    let tempo = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_click(tempo.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a z x");
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport, original));
    visual.simulate_keystrokes("escape");
    visual.update(|window, cx| {
        editor.update(cx, |v, _| {
            v.focus.focus(window);
            v.computer_keyboard.enabled = true;
        })
    });
    let pitch = editor.read_with(&visual, |v, _| v.computer_keyboard.base_pitch);
    visual.simulate_keystrokes("z");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.base_pitch, pitch.saturating_sub(12))
    });
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.computer_keyboard.base_pitch, pitch)
    });
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.viewport, original);
        assert!(v.zoom_fit.arrange.is_none());
    });
    editor.update(&mut visual, |v, _| {
        v.computer_keyboard.enabled = false;
        v.close_prompt_open = true;
    });
    visual.simulate_keystrokes("z x");
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport, original));
}
#[test]
fn fit_interval_normalizes_bounds_and_handles_empty_minimum_and_overflow_limits() {
    assert_eq!(
        interval(9, 3, 12, 1),
        Some(Viewport {
            start: 3,
            length: 6
        })
    );
    assert_eq!(
        interval(0, 1, 16, 4),
        Some(Viewport {
            start: 0,
            length: 4
        })
    );
    assert_eq!(interval(9, 9, 12, 1), None);
    assert_eq!(interval(0, 1, 0, 1), None);
    assert_eq!(
        interval(u64::MAX - 1, u64::MAX, u64::MAX, 8),
        Some(Viewport {
            start: u64::MAX - 8,
            length: 8
        })
    );
}

#[gpui::test]
fn empty_arrangement_fits_existing_horizon_and_restores_without_a_musical_edit(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RemoveMidiClip(clip))))
    });
    pump(&editor, &mut visual);
    let prior = Some(Viewport {
        start: 24_000,
        length: 48_000,
    });
    let before = editor.update(&mut visual, |v, cx| {
        v.viewport = prior;
        cx.notify();
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.update(|window, cx| editor.update(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("z");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            Viewport::fit(v.state.as_ref().unwrap().timeline_frames)
        )
    });
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.viewport, prior);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}
#[gpui::test]
fn console_midi_fit_back_is_local_and_explicit_clip_changes_retire_back(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, original) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddScene {
            name: "fit console".into()
        })))
    });
    pump(&editor, &mut visual);
    let scene = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.scenes()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(
            v.enqueue(Request::Edit(Command::CopyArrangementMidiToSession {
                source: original,
                scene
            }))
        )
    });
    pump(&editor, &mut visual);
    let (track, clip) = editor.read_with(&visual, |v, _| {
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
        (track, cell.id())
    });
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.select_session_midi(track, scene, clip);
            v.midi_focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    let prior = (
        BeatTime::from_quarters(2).unwrap(),
        BeatTime::from_quarters(8).unwrap(),
        36,
        20,
    );
    let before = editor.update(&mut visual, |v, cx| {
        v.piano_beat_start = prior.0;
        v.piano_beat_span = prior.1;
        v.piano_pitch_low = prior.2;
        v.piano_pitch_rows = prior.3;
        cx.notify();
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.run_until_parked();
    click(&mut visual, "midi-zoom-fit");
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (
                v.piano_beat_start,
                v.piano_beat_span,
                v.piano_pitch_low,
                v.piano_pitch_rows
            ),
            prior
        )
    });
    click(&mut visual, "midi-zoom-fit");
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.set_mode(EditorMode::Arrangement);
            assert!(v.set_selected_clip(track, original, window, cx));
            v.select_session_midi(track, scene, clip);
            v.midi_focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
    visual.simulate_keystrokes("x");
    editor.read_with(&visual, |v, _| {
        assert!(v.zoom_fit.midi.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}
