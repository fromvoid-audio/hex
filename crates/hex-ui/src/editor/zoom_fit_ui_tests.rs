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
            anchor_track: Some(track),
            end_track: Some(track),
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
    visual.simulate_keystrokes("v");
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
    visual.simulate_keystrokes("v");
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
            (v.piano_beat_start, v.piano_beat_span.raw()),
            (i128::from(BeatTime::ONE.raw()), 2 * BeatTime::ONE.raw())
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
        // Cmd zooms time; a plain wheel scrolls pitch.
        modifiers: gpui::Modifiers::command(),
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
                v.piano_beat_start,
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
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (v.piano_beat_start, v.piano_beat_span.raw()),
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
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| assert!(v.zoom_fit.midi.is_none()));
}
#[gpui::test]
fn text_numeric_modal_and_musical_typing_keep_shortcuts_from_navigation(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    editor.update(&mut visual, |v, _| {
        v.viewport = Some(Viewport {
            start: 48_000,
            length: 96_000,
        });
    });
    let original = editor.read_with(&visual, |v, _| v.viewport);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
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
    visual.simulate_keystrokes("z x v");
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport, original));
    let tempo = visual.debug_bounds("project-tempo-value").unwrap();
    visual.simulate_click(tempo.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a z x v");
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
    let velocity = editor.read_with(&visual, |v, _| v.computer_keyboard.velocity);
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| {
        assert!(v.computer_keyboard.velocity > velocity);
        assert_eq!(v.viewport, original);
        assert!(v.zoom_fit.arrange.is_none());
    });
    editor.update(&mut visual, |v, _| {
        v.computer_keyboard.enabled = false;
        v.close_prompt_open = true;
    });
    visual.simulate_keystrokes("z x v");
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.viewport, original);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
    });
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
            Viewport::fit(v.normal_navigation_horizon(v.state.as_ref().unwrap().timeline_frames))
        )
    });
    visual.simulate_keystrokes("v");
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
        v.piano_beat_start = i128::from(prior.0.raw());
        v.piano_beat_span = prior.1;
        v.piano_pitch_low = prior.2;
        v.piano_pitch_rows = prior.3;
        cx.notify();
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.run_until_parked();
    click(&mut visual, "midi-zoom-fit");
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            (
                v.piano_beat_start,
                v.piano_beat_span,
                v.piano_pitch_low,
                v.piano_pitch_rows
            ),
            (i128::from(prior.0.raw()), prior.1, prior.2, prior.3)
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
    visual.simulate_keystrokes("v");
    editor.read_with(&visual, |v, _| {
        assert!(v.zoom_fit.midi.is_none());
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn shortening_edit_preserves_arranger_view_through_real_ack_and_repaints(cx: &mut TestAppContext) {
    use crate::editor::loop_selection_ui_test::{click, fixture, pump};
    let (handle, editor, audio, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let (track, asset) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        (p.tracks()[0].id(), p.clip(audio).unwrap().asset())
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceClip {
            track,
            asset,
            start: 9_000_000,
            duration: 48_000,
            source_offset: 0,
            source_length: 48_000,
            looping: false,
        })));
    });
    pump(&editor, &mut visual);
    let tail = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert_eq!(
            state.timeline_frames,
            9_048_000 + 2 * 48_000,
            "existing worker query remains unchanged"
        );
        assert_eq!(
            v.normal_navigation_horizon(state.timeline_frames),
            9_048_000 + 8 * 48_000,
            "normal arranger allowance is exactly eight seconds after far content"
        );
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .find(|c| c.start() == 9_000_000)
            .unwrap()
            .id()
    });
    let expected = Viewport {
        start: 8_900_000,
        length: 200_000,
    };
    editor.update(&mut visual, |v, cx| {
        v.viewport = Some(expected);
        cx.notify();
    });
    visual.run_until_parked();
    click(&mut visual, tail, false);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip, Some(tail));
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            expected
        );
    });
    visual.simulate_keystrokes("backspace");
    pump(&editor, &mut visual);
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            assert!(
                state.project.clip(tail).is_none(),
                "actual delete reached canonical state"
            );
            assert!(
                state.project.clip(audio).is_some(),
                "original audio retained"
            );
            assert_eq!(
                v.visible_viewport(state.timeline_frames),
                expected,
                "shortening must not clamp the visible anchor or zoom"
            );
        });
    }
    let after_delete =
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    // Fit may deliberately leave the old tail, while Back restores the exact
    // saved view. This explicit restoration is separate from ordinary scrolling.
    visual.simulate_keystrokes("z");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_ne!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            expected
        )
    });
    visual.simulate_keystrokes("v");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            expected
        )
    });
    // Explicit Fit must honor a still-visible time selection beyond the new
    // content bound, rather than clamping both ends to the normal buffer.
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let down = point(
        lane.left() + lane.size.width * 0.25,
        lane.top() + lane.size.height * 0.6,
    );
    let up = point(lane.left() + lane.size.width * 0.75, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let selected_view = editor.read_with(&visual, |v, _| {
        let range = v.normalized_time_selection().unwrap();
        assert!(range.start() >= expected.start && range.end() <= expected.end().unwrap());
        Viewport {
            start: range.start(),
            length: range.duration(),
        }
    });
    visual.simulate_keystrokes("z");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            selected_view,
            "explicit selection fit may restore a visible empty interval beyond content"
        )
    });
    visual.simulate_keystrokes("z"); // A no-op repeated fit must retain Back.
    visual.simulate_keystrokes("v");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            expected
        )
    });
    // Return to a point selection before testing ordinary scrolling.
    visual.simulate_click(lane.center(), Modifiers::default());
    visual.run_until_parked();
    visual.simulate_keystrokes("alt-left");
    visual.run_until_parked();
    let panned = Viewport {
        start: expected.start - expected.length / 2,
        ..expected
    };
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            panned,
            "retirement must not add a second navigation jump"
        );
        assert_eq!(
            v.viewport_horizon(state.timeline_frames),
            panned.end().unwrap()
        );
    });
    visual.simulate_keystrokes("alt-right");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            panned,
            "scrolling cannot revisit retired space"
        )
    });
    for _ in 0..100 {
        visual.simulate_keystrokes("alt-left");
    }
    visual.run_until_parked();
    let origin_view = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let normal = v.normal_navigation_horizon(state.timeline_frames);
        assert_eq!(v.viewport_horizon(state.timeline_frames), normal);
        assert_eq!(
            normal,
            state.timeline_frames + 6 * 48_000,
            "existing two-second buffer gains six seconds; empty minimum gains the same allowance"
        );
        assert!(
            normal >= 120_000 + 8 * 48_000,
            "eight seconds remain beyond all fixture content"
        );
        assert_eq!(
            state.project.snapshot(),
            after_delete,
            "navigation never edits project content"
        );
        let visible = v.visible_viewport(state.timeline_frames);
        assert_eq!(visible.start, 0);
        visible
    });
    visual.simulate_resize(gpui::size(px(1100.0), px(740.0)));
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            origin_view
        )
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert!(
            state.project.clip(tail).is_some(),
            "undo restores far content"
        );
        assert!(v.viewport_horizon(state.timeline_frames) > expected.end().unwrap());
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            origin_view,
            "growth remains reachable without refitting"
        );
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert!(state.project.clip(tail).is_none());
        assert_eq!(v.visible_viewport(state.timeline_frames), origin_view);
    });
    visual.simulate_keystrokes("cmd--");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            Viewport {
                start: 0,
                length: origin_view.length * 2
            }
        );
        assert_eq!(
            v.viewport_horizon(state.timeline_frames),
            v.normal_navigation_horizon(state.timeline_frames),
            "zoom-out retires an obsolete offscreen tail without extra movement"
        );
    });
    visual.simulate_keystrokes("cmd-0");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let normal = v.normal_navigation_horizon(state.timeline_frames);
        assert_eq!(
            v.viewport_horizon(state.timeline_frames),
            normal,
            "explicit fit retires the obsolete tail"
        );
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            Viewport::fit(normal)
        );
    });
}

#[gpui::test]
fn arranger_bounds_include_unfocused_automation_and_empty_project_buffer(cx: &mut TestAppContext) {
    use crate::editor::loop_selection_ui_test::{fixture, pump};
    let (handle, editor, _, _, _files) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let track = editor.read_with(&visual, |v, _| {
        assert!(v.gain_lane.track.is_none());
        v.state.as_ref().unwrap().project.tracks()[1].id()
    });
    let beat = BeatTime::from_quarters(100).unwrap();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![hex_project::GainPoint {
                incoming_gain: None,
                beat,
                gain: 0.5,
                outgoing: hex_project::GainInterpolation::Linear
            }],
        })))
    });
    pump(&editor, &mut visual);
    visual.simulate_keystrokes("cmd-0");
    visual.run_until_parked();
    let before_empty = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert!(
            v.gain_lane.track.is_none(),
            "lane need not be focused to keep its authored extent reachable"
        );
        let expected = state.project.tempo_map().beat_to_frame(beat).unwrap() + 8 * 48_000;
        assert_eq!(v.viewport_horizon(state.timeline_frames), expected);
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            Viewport::fit(expected)
        );
        expected
    });
    editor.update(&mut visual, |v, _| {
        let tracks = v
            .state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .map(|t| t.id())
            .collect();
        assert!(v.enqueue(Request::Edit(Command::RemoveTracks(tracks))));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert!(state.project.tracks().is_empty());
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            Viewport::fit(before_empty),
            "even emptying the project retains the current view"
        );
    });
    visual.simulate_keystrokes("cmd-0");
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let normal = 14 * 48_000; // Existing eight-second minimum plus six seconds.
        assert_eq!(
            state.timeline_frames,
            8 * 48_000,
            "canonical query/export bounds are unchanged"
        );
        assert_eq!(v.viewport_horizon(state.timeline_frames), normal);
        assert_eq!(
            v.visible_viewport(state.timeline_frames),
            Viewport::fit(normal)
        );
    });
}

#[gpui::test]
fn arrange_vertical_back_prunes_removed_rows_and_clamps_scroll_after_resize(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, _) = fixture(cx);
    for index in 0..4 {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddTrack {
                name: format!("fit {index}")
            })))
        });
        pump(&editor, &mut visual);
    }
    let tracks = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect::<Vec<_>>()
    });
    let resize = visual
        .debug_bounds(Box::leak(
            format!("track-height-resize-{}", tracks[0].value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    let end = point(resize.x, resize.y + px(40.0));
    visual.simulate_mouse_down(resize, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let prior = Some(Viewport {
        start: 12_000,
        length: 96_000,
    });
    editor.update(&mut visual, |view, cx| {
        view.selected_clip = None;
        view.selected_track = Some(tracks[2]);
        view.track_selection.replace(&tracks[..3]);
        view.time_selection = Some(TimeSelection {
            anchor_track: Some(tracks[0]),
            end_track: Some(tracks[2]),
            anchor_frame: 0,
            end_frame: 48_000,
        });
        view.viewport = prior;
        cx.notify();
    });
    visual.update(|window, cx| editor.update(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("z");
    visual.run_until_parked();
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::RemoveTrack(tracks[0]))))
    });
    pump(&editor, &mut visual);
    visual.simulate_resize(gpui::size(px(1200.0), px(680.0)));
    visual.run_until_parked();
    let project = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    click(&mut visual, "arrange-zoom-back");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.viewport, prior);
        assert_eq!(
            view.track_body_height(tracks[0]),
            super::super::tracks::TRACK_ROW_HEIGHT,
            "removed row has no retained custom geometry"
        );
        let state = view.state.as_ref().unwrap();
        assert_eq!(state.project.snapshot(), project);
        assert!(view.track_scroll.list.logical_scroll_top().item_ix < state.project.tracks().len());
        let maximum = view
            .arrangement_bounds
            .unwrap()
            .height
            .max(super::super::tracks::TRACK_ROW_HEIGHT);
        for track in state.project.tracks() {
            let height = view.track_body_height(track.id());
            assert!(
                height.is_finite()
                    && height >= super::super::track_layout::HEADER_ONLY_HEIGHT
                    && height <= maximum
            );
        }
    });
}
