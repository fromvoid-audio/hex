//! Thin Arrangement bounce routing; rendering/adoption belong to the worker.
use super::{EditorMode, EditorView};
use gpui::Window;
use hex_session::service::{ExportControl, Request};
impl EditorView {
    pub(super) fn bounce_arrangement_selection(&mut self, in_place: bool, window: &Window) {
        if self.mode != EditorMode::Arrangement
            || !self.focus.is_focused(window)
            || self.computer_keyboard.enabled
            || self.sections.naming()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.context_menu.is_some()
            || self.changelog.is_open()
            || self.theme_settings.open
            || self.tuning.open
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return;
        }
        if self.edit_pending()
            || self.arrangement_pan.is_some()
            || self.time_range_move_gesture.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.track_resize_active()
            || self.loop_rail_gesture.is_some()
            || self.device_gesture.snapshot().is_some()
            || self.time_selection_gesture.is_some()
            || self.active_clip_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.sections.held()
            || self.section_install_pending()
            || self.scrub_active()
            || self.bounce_control.is_some()
        {
            self.error = Some("finish or cancel the current edit before bouncing".into());
            return;
        }
        let target = if self
            .time_selection
            .is_some_and(|s| s.anchor_frame != s.end_frame)
        {
            let Some(range) = self.normalized_time_selection() else {
                self.error =
                    Some("bounce time selection no longer belongs to current tracks".into());
                return;
            };
            hex_project::BounceTarget::Range(range)
        } else {
            hex_project::BounceTarget::Clips(self.selected_clip_ids())
        };
        let Some(state) = &self.state else {
            return;
        };
        if let Err(reason) = state.project.plan_arrangement_bounce(&target, in_place) {
            self.error = Some(reason.to_string().into());
            return;
        }
        let control = ExportControl::default();
        if self.enqueue(Request::BounceArrangement {
            project_generation: self.project_generation,
            revision: state.revision,
            target,
            in_place,
            control: control.clone(),
        }) {
            self.bounce_control = Some(control);
            self.status = "bouncing arrangement · escape cancels".into();
        }
    }
    pub(super) fn cancel_bounce(&mut self) -> bool {
        if let Some(control) = &self.bounce_control
            && control.cancel()
        {
            self.status = "cancelling bounce".into();
            return true;
        }
        false
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use crate::editor::loop_selection_ui_test::{click, fixture_with_bounce_media, pump};
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    #[gpui::test]
    fn rendered_b_and_platform_b_adopt_below_or_replace_with_one_undo(cx: &mut TestAppContext) {
        let (handle, editor, audio, _, files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        let original =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("cmd-b");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), original.tracks.len() + 1);
            assert!(p.clip(audio).is_some());
            let clip = &p.tracks()[1].clips()[0];
            assert_eq!((clip.start(), clip.duration()), (0, 48_000));
            assert!(
                p.asset(clip.asset())
                    .unwrap()
                    .path()
                    .starts_with(std::fs::canonicalize(files.root()).unwrap())
            );
            assert!(v.bounce_control.is_none());
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        click(&mut visual, audio, false);
        visual.simulate_keystrokes("b");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), original.tracks.len());
            assert!(p.clip(audio).is_none());
            assert_eq!(p.tracks()[0].clips()[0].duration(), 48_000);
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot().tracks,
                original.tracks
            )
        });
    }
    #[gpui::test]
    fn range_precedence_is_exact_and_shortcuts_preserve_input_contexts(cx: &mut TestAppContext) {
        let (handle, editor, audio, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().revision);
        visual.update(|window, cx| {
            editor.read_with(cx, |v, _| v.browser.search.read(cx).focus(window))
        });
        visual.run_until_parked();
        for key in ["b", "cmd-b"] {
            visual.simulate_keystrokes(key);
            pump(&editor, &mut visual);
        }
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.midi_focus.focus(window)));
        visual.run_until_parked();
        for key in ["b", "cmd-b"] {
            visual.simulate_keystrokes(key);
            pump(&editor, &mut visual);
        }
        let value = visual.debug_bounds("project-tempo-value").unwrap();
        visual.simulate_mouse_down(value.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(value.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        for key in ["b", "cmd-b"] {
            visual.simulate_keystrokes(key);
            pump(&editor, &mut visual);
        }
        visual.simulate_keystrokes("escape");
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
        visual.run_until_parked();
        editor.update(&mut visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().revision, before);
            assert!(v.bounce_control.is_none());
            v.computer_keyboard.enabled = true;
        });
        for key in ["b", "cmd-b"] {
            visual.simulate_keystrokes(key);
            pump(&editor, &mut visual);
        }
        editor.update(&mut visual, |v, _| {
            v.computer_keyboard.enabled = false;
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(crate::editor::time_selection::TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 36_007,
                end_frame: 12_013,
            });
        });
        visual.simulate_keystrokes("cmd-b");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let clip = &v.state.as_ref().unwrap().project.tracks()[1].clips()[0];
            assert_eq!((clip.start(), clip.duration()), (12_013, 23_994));
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            v.time_selection = Some(crate::editor::time_selection::TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 12_013,
                end_frame: 36_007,
            });
        });
        visual.simulate_keystrokes("b");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let clips = v.state.as_ref().unwrap().project.tracks()[0].clips();
            assert_eq!(clips.len(), 3);
            assert!(
                clips
                    .iter()
                    .any(|c| (c.start(), c.duration()) == (12_013, 23_994))
            );
            assert!(
                clips
                    .iter()
                    .any(|c| (c.start(), c.duration()) == (0, 12_013))
            );
            assert!(
                clips
                    .iter()
                    .any(|c| (c.start(), c.duration()) == (36_007, 11_993))
            );
        });
    }
}
