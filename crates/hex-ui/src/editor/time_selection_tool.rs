//! Momentary input routing only. Holding unmodified 2 overrides new pointer
//! starts, not persisted draw/pan modes or a gesture already in custody. Each
//! canvas retains its existing range model, snap and capture/release contract.
use super::EditorView;
use gpui::{Context, CursorStyle, Div, KeyDownEvent, KeyUpEvent, Window, div, prelude::*};

impl EditorView {
    /// Paint last within a selectable surface so clip headers/handles cannot
    /// advertise a different idle tool. No hit handler or occlusion is added;
    /// existing pointer captures retain priority over this hover cursor.
    pub(super) fn time_selection_cursor_layer(&self) -> Option<Div> {
        self.time_selection_key_held
            .then(|| div().absolute().inset_0().cursor(CursorStyle::IBeam))
    }

    pub(super) fn time_selection_pointer_cursor(&self, ordinary: CursorStyle) -> CursorStyle {
        if self.time_selection_key_held {
            CursorStyle::IBeam
        } else {
            ordinary
        }
    }

    fn time_selection_tool_has_focus(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
            || self.midi_focus.is_focused(window)
            || self.audio_event_focus.is_focused(window)
            || self.focused_header_target(window).is_some()
    }

    pub(super) fn time_selection_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key != "2"
            || event.keystroke.modifiers.modified()
            || (event.is_held && !self.time_selection_key_held)
            || !self.time_selection_tool_has_focus(window)
            || self.clip_name_edit.is_some()
            || self.track_name.is_editing()
            || self.sections.naming()
            || self.context_menu.is_some()
            || self.theme_settings.open
            || self.tuning.open
            || self.gain_numeric_editing(cx)
            || self.clip_numeric_editing(cx)
            || self.project_tempo_editing(cx)
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return false;
        }
        if !self.time_selection_key_held {
            self.time_selection_key_held = true;
            cx.notify();
        }
        cx.stop_propagation();
        true
    }

    /// macOS reports shifted number-row symbols on release. Also retain the
    /// keyboard layout's public equivalent (for example AZERTY's unshifted key).
    pub(super) fn time_selection_key_up(
        &mut self,
        event: &KeyUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.time_selection_key_held {
            return false;
        }
        let key = event.keystroke.key.as_str();
        let equivalent = cx
            .keyboard_mapper()
            .get_key_equivalents()
            .and_then(|map| map.get(&'2'))
            .copied();
        if !matches!(key, "2" | "@" | "\"")
            && !equivalent.is_some_and(|alias| key.chars().eq(std::iter::once(alias)))
        {
            return false;
        }
        self.time_selection_key_held = false;
        cx.notify();
        true
    }

    pub(super) fn clear_time_selection_key_on_blur(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.time_selection_key_held && !self.time_selection_tool_has_focus(window) {
            self.time_selection_key_held = false;
            cx.notify();
            // Focus listeners run after frame swap. Retire the hover overlay
            // through the next effect cycle without requiring pointer movement.
            cx.defer_in(window, |_, _, cx| cx.notify());
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{
        KeyUpEvent, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
    };

    fn key(visual: &mut VisualTestContext, down: bool) {
        let keystroke = Keystroke::parse("2").unwrap();
        if down {
            visual.simulate_event(KeyDownEvent {
                keystroke,
                is_held: false,
            });
        } else {
            visual.simulate_event(KeyUpEvent { keystroke });
        }
        visual.run_until_parked();
    }

    #[gpui::test]
    fn held_two_paints_arrangement_header_and_survives_key_release_until_mouseup(
        cx: &mut TestAppContext,
    ) {
        use crate::editor::clip_drag_ui_test::{audio_project, editor_window};
        cx.update(crate::editor::bind_editor_keys);
        let (project, source, _, clip) = audio_project();
        let before = project.snapshot();
        let (window, editor) = editor_window(cx, project, Default::default());
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.update(|window, cx| {
            editor.update(cx, |v, cx| v.track_header_focus(source, cx).focus(window))
        });
        visual.run_until_parked();
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap();
        let down = header.center();
        let end = point(down.x + px(40.), down.y);
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        key(&mut visual, true);
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        let main = visual.debug_bounds("main-track-lane").unwrap();
        visual.simulate_mouse_move(main.center(), None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(v.time_selection_gesture.is_some());
            assert!(v.active_clip_drag.is_none());
        });
        key(&mut visual, false);
        assert_ne!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        editor.read_with(&visual, |v, _| {
            assert!(v.normalized_time_selection().is_some());
            assert!(v.time_selection_gesture.is_none());
            assert!(!v.time_selection_key_held);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(v.last_enqueued_edit.is_none());
        });
    }

    #[gpui::test]
    fn held_two_overrides_note_edit_and_draw_without_changing_draw_mode(cx: &mut TestAppContext) {
        use crate::editor::midi_canvas::creation_tests::{double_down, fixture, pump};
        for draw in [false, true] {
            let (mut visual, editor, _environment, clip) = fixture(cx);
            let roll = visual.debug_bounds("midi-note-roll").unwrap();
            let at = |x| point(roll.left() + roll.size.width * x, roll.center().y);
            double_down(&mut visual, at(0.25), Modifiers::default());
            visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
            visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
            let before = editor.update(&mut visual, |v, cx| {
                assert!(
                    !v.state
                        .as_ref()
                        .unwrap()
                        .project
                        .midi_source(clip)
                        .unwrap()
                        .notes()
                        .is_empty()
                );
                v.midi_draw_mode = draw;
                v.midi_pan.enabled = draw;
                cx.notify();
                v.state.as_ref().unwrap().project.snapshot()
            });
            visual.run_until_parked();
            visual.simulate_mouse_move(at(0.3), None, Modifiers::default());
            visual.run_until_parked();
            key(&mut visual, true);
            assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
            visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
            key(&mut visual, false);
            assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
            visual.simulate_mouse_move(at(0.7), Some(MouseButton::Left), Modifiers::default());
            visual.simulate_mouse_up(at(0.7), MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert!(v.midi_time_selection_bounds().is_some());
                assert_eq!(v.midi_draw_mode, draw);
                assert_eq!(v.midi_pan.enabled, draw);
                assert!(v.midi_note_gesture.is_none());
                assert!(v.midi_stroke.is_none());
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }

    #[gpui::test]
    fn held_two_selects_velocity_handles_without_editing_them(cx: &mut TestAppContext) {
        use crate::editor::midi_canvas::creation_tests::{fixture, pump};
        use hex_project::{BeatTime, Command, MidiNoteInsert};
        use hex_session::service::Request;
        let (mut visual, editor, _environment, clip) = fixture(cx);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNotes {
                clip,
                notes: vec![MidiNoteInsert {
                    active: true,
                    start: BeatTime::ONE,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 93
                }]
            })))
        });
        pump(&editor, &mut visual);
        let lane = visual.debug_bounds("midi-velocity-lane").unwrap();
        let down = point(
            lane.left() + lane.size.width * 0.25,
            lane.top()
                + px(
                    crate::editor::midi_velocity::handle_y(93, f32::from(lane.size.height))
                        .unwrap(),
                ),
        );
        let end = point(lane.left() + lane.size.width * 0.75, down.y);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        key(&mut visual, true);
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        key(&mut visual, false);
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ResizeUpDown)
        );
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.midi_time_selection_bounds().is_some());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }

    #[gpui::test]
    fn held_two_resets_on_window_deactivation_and_real_project_replacement(
        cx: &mut TestAppContext,
    ) {
        use crate::editor::midi_canvas::creation_tests::{fixture, pump};
        for replacement in [false, true] {
            let (mut visual, editor, _environment, _) = fixture(cx);
            key(&mut visual, true);
            editor.read_with(&visual, |v, _| assert!(v.time_selection_key_held));
            if replacement {
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(hex_session::service::Request::NewWithAudioMode {
                        mode: hex_session::DocumentAudioMode::Resident,
                        reservation: None
                    }))
                });
                pump(&editor, &mut visual);
                editor.read_with(&visual, |v, _| assert!(v.project_generation > 0));
            } else {
                visual.deactivate_window();
            }
            editor.read_with(&visual, |v, _| assert!(!v.time_selection_key_held));
        }
    }

    #[gpui::test]
    fn held_two_releases_shifted_number_symbols_without_losing_other_key_chords(
        cx: &mut TestAppContext,
    ) {
        for symbol in ["@", "\""] {
            let (mut visual, editor, _environment, _) =
                crate::editor::midi_canvas::creation_tests::fixture(cx);
            key(&mut visual, true);
            visual.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse("f").unwrap(),
            });
            editor.read_with(&visual, |v, _| assert!(v.time_selection_key_held));
            visual.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(symbol).unwrap(),
            });
            editor.read_with(&visual, |v, _| {
                assert!(
                    !v.time_selection_key_held,
                    "shifted release {symbol} must restore normal input"
                )
            });
        }
    }

    #[gpui::test]
    fn held_two_clears_on_text_focus_and_modified_two_does_not_arm(cx: &mut TestAppContext) {
        let (mut visual, editor, _environment, _) =
            crate::editor::midi_canvas::creation_tests::fixture(cx);
        visual.simulate_keystrokes("cmd-2");
        editor.read_with(&visual, |v, _| assert!(!v.time_selection_key_held));
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        visual.simulate_mouse_move(roll.center(), None, Modifiers::default());
        visual.run_until_parked();
        key(&mut visual, true);
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        editor.read_with(&visual, |v, _| assert!(v.time_selection_key_held));
        visual.update(|window, cx| {
            editor.read_with(cx, |v, _| {
                v.browser.search.read(cx).focus_handle().focus(window)
            })
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert!(!v.time_selection_key_held));
        assert_ne!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        visual.simulate_keystrokes("2");
        editor.read_with(&visual, |v, cx| {
            assert!(!v.time_selection_key_held);
            assert_eq!(v.browser.search.read(cx).text(), "2");
        });
    }
    #[gpui::test]
    fn held_two_keeps_an_existing_note_drag_cursor(cx: &mut TestAppContext) {
        use crate::editor::midi_canvas::creation_tests::{double_down, fixture, pump};
        let (mut visual, editor, _environment, _) = fixture(cx);
        let roll = visual.debug_bounds("midi-note-roll").unwrap();
        let at = |x| point(roll.left() + roll.size.width * x, roll.center().y);
        double_down(&mut visual, at(0.25), Modifiers::default());
        visual.simulate_mouse_move(at(0.5), Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(at(0.5), MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| assert!(v.midi_note_gesture.is_some()));
        let captured = visual.rendered_cursor_style();
        assert_eq!(captured, Some(CursorStyle::ClosedHand));
        key(&mut visual, true);
        assert_eq!(visual.rendered_cursor_style(), captured);
        visual.simulate_mouse_up(at(0.3), MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
        key(&mut visual, false);
        assert_ne!(visual.rendered_cursor_style(), Some(CursorStyle::IBeam));
    }
}
