//! Printable finite Arrangement reversal through the canonical edit/receipt path.
use super::{EditorMode, EditorView};
use gpui::{Context, KeyDownEvent, Modifiers, Window};
use hex_project::Command;
use hex_session::service::Request;
impl EditorView {
    pub(super) fn reverse_clip_key(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key != "r"
            || event.keystroke.modifiers != Modifiers::default()
            || event.is_held
            || self.mode != EditorMode::Arrangement
            || self.automation_view.enabled
            || !self.focus.is_focused(window)
            || window
                .context_stack()
                .iter()
                .any(|context| context.contains("hex_text_editing"))
            || self.sections.naming()
            || self.changelog.is_open()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.context_menu.is_some()
            || self.theme_settings.open
            || self.tuning.open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return false;
        }
        let clips = self.selected_clip_ids();
        if clips.is_empty() {
            return false;
        }
        if self.edit_pending()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.time_range_move_gesture.is_some()
            || self.time_selection_gesture.is_some()
            || self.sections.held()
            || self.section_install_pending()
        {
            self.error = Some("finish the current edit before reversing clips".into());
        } else if self.enqueue(Request::Edit(Command::ReverseClips(clips))) {
            self.duplicate_intent.cancel_chain();
        }
        cx.notify();
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::loop_selection_ui_test::{click, fixture, pump};
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
    #[gpui::test]
    fn printable_reverse_uses_mixed_selection_and_preserves_command_and_text_focus(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, audio, midi, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        click(&mut visual, midi, true);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("r");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert!(
                p.clip(audio)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .content_view
                    .is_some()
            );
            assert!(p.midi_clip(midi).unwrap().content_view().is_some());
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        assert_eq!(
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
            before
        );
        visual.simulate_keystrokes("cmd-r");
        visual.run_until_parked();
        assert_eq!(
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
            before
        );
        // A real native text editor owns printable keys/IME while renaming.

        visual.simulate_keystrokes("r");
        visual.run_until_parked();
        assert_eq!(
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot()),
            before
        );
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    #[gpui::test]
    fn accepted_reversed_finite_waveform_survives_trim_ack_and_second_rendered_trim(
        cx: &mut TestAppContext,
    ) {
        let (handle, editor, audio, _midi, _files) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        visual.simulate_keystrokes("r");
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, cx| {
            v.snap_grid = super::super::musical_time::SnapGrid::Off;
            cx.notify();
        });
        visual.run_until_parked();
        let selector: &'static str =
            Box::leak(format!("clip-trim-left-{}", audio.value()).into_boxed_str());
        let down = crate::editor::clip_drag_ui_test::trim_grab_point(
            visual.debug_bounds(selector).unwrap(),
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(4.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.simulate_mouse_up(
            point(down.x + px(4.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        pump(&editor, &mut visual);
        let accepted = editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let c = state.project.clip(audio).unwrap();
            assert!(c.start() > 0);
            let p = state.audio_clip_waveforms.get(&audio).unwrap();
            assert_eq!(p.offset, 0);
            assert_eq!(p.waveform.source_frames as u64, c.duration());
            std::sync::Arc::clone(&p.waveform)
        });
        let down = crate::editor::clip_drag_ui_test::trim_grab_point(
            visual.debug_bounds(selector).unwrap(),
        );
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(3.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.update(&mut visual, |v, cx| {
            v.waveform_geometry_cache.borrow_mut().clear();
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert!(v.active_trim_drag.is_some());
            let cache = v.waveform_geometry_cache.borrow();
            let rendered = cache.rendered_waveforms();
            assert!(
                rendered
                    .iter()
                    .any(|w| std::sync::Arc::ptr_eq(&w.source, &accepted)
                        && w.prepared.is_some_and(|(_, offset)| offset > 0)
                        && w.spans.iter().all(|s| *s > 1.0)),
                "accepted finite directed peaks must paint during a second trim"
            );
        });
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
    }
}
