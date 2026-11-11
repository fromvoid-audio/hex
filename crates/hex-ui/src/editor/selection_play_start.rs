//! Shared play-start query for whole Arrangement rectangle selections.
//! Membership remains with clip_selection and arrangement_sections; transport
//! precedence and display consumers use this query without seeking playback.
use super::EditorView;
use hex_project::ClipId;

#[derive(Clone, Copy)]
pub(super) struct SelectedClipPlayStart {
    scope: (u64, u64, u64, ClipId),
    frame: Option<u64>,
}

impl EditorView {
    /// Whole Arrangement selection supplies Play's start, as in Live: the earliest
    /// selected clip or Main section start in frames. Audio starts are
    /// frame-anchored; MIDI and section beats convert through the tempo map, and
    /// one that maps beyond the frame range is skipped. An offscreen start still
    /// applies and the viewport does not move. Stale and Session IDs are ignored.
    /// A non-empty time selection outranks this in `playback_start_frame`; a
    /// point cursor placed inside the selected clip afterwards keeps its frame,
    /// so split-at-cursor stays visible. The selected clip's marker replaces
    /// the retained point cursor until the whole-clip selection clears.
    pub(super) fn selected_arrangement_play_start(&self) -> Option<u64> {
        if self.project_point_cursor().is_some()
            || self.clip_drop_cursor_epoch.is_some_and(|epoch| {
                self.selection_intent_epoch == Some(epoch)
                    && self
                        .edit_cursor
                        .is_some_and(|cursor| cursor.generation == self.project_generation)
            })
        {
            return None;
        }
        let state = self.state.as_ref()?;
        let project = &state.project;
        let Some(primary) = self.selected_clip else {
            // Q can retain section IDs while ordinary lanes own the selection.
            // Only current Main ownership makes those IDs a play-start source.
            if self.mode != super::EditorMode::Arrangement
                || self.selected_track.is_some()
                || self.focused_return.is_some()
                || self.device_panel.main_selected != Some(self.project_generation)
            {
                return None;
            }
            let selected = self.sections.selected_ids();
            return project
                .arrangement_sections()
                .iter()
                .filter(|section| selected.contains(&section.id()))
                .filter_map(|section| project.tempo_map().beat_to_frame(section.start()).ok())
                .min();
        };
        // Any newly placed cursor restores the ordinary precedence below.
        if self.loop_start_over_clip == Some(primary) && self.edit_cursor.is_none() {
            return None;
        }
        if self.edit_cursor_in_selected_clip == Some(primary)
            && self
                .edit_cursor
                .is_some_and(|cursor| cursor.generation == self.project_generation)
        {
            return None;
        }
        // Rulers and the editor tick read the same projection. Scan the clip
        // collection only when the canonical project or selection changes.
        let scope = (
            self.project_generation,
            state.revision,
            self.clip_selection.epoch,
            primary,
        );
        if let Some(cached) = self.selected_clip_play_start.get()
            && cached.scope == scope
        {
            return cached.frame;
        }
        let tempo = project.tempo_map();
        let frame = project
            .tracks()
            .iter()
            .flat_map(|track| {
                let audio = track
                    .clips()
                    .iter()
                    .filter(|clip| self.clip_is_selected(clip.id()))
                    .map(|clip| Some(clip.start()));
                let midi = track
                    .midi_clips()
                    .iter()
                    .filter(|clip| self.clip_is_selected(clip.id()))
                    .map(|clip| tempo.beat_to_frame(clip.start()).ok());
                audio.chain(midi)
            })
            .flatten()
            .min();
        self.selected_clip_play_start
            .set(Some(SelectedClipPlayStart { scope, frame }));
        frame
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::{
        arrangement_sections::tests::{fixture, position},
        edit_cursor::EditCursor,
        time_selection::TimeSelection,
    };
    use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
    use hex_engine::playback::PlaybackCommand::PlayFrom;
    use std::time::Instant;

    #[gpui::test]
    fn section_selection_uses_the_clip_play_start_precedence_and_display(cx: &mut TestAppContext) {
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, cx| {
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            view.selected_clip = None;
            view.reconcile_clip_selection();
            view.section_autoplay = false;
            view.edit_cursor = Some(EditCursor {
                track: Some(track),
                frame: 73_000,
                generation: view.project_generation,
                pulse_color: 0xffffff,
                pulse_started: Instant::now(),
            });
            view.last_transport_command = None;
            cx.notify();
        });
        visual.run_until_parked();
        let down = editor.read_with(&visual, |view, _| position(view, 10));
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.playback_start_frame(),
                73_000,
                "hover does not select or seek"
            )
        });
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(down, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let expected = editor.read_with(&visual, |view, _| {
            assert_eq!(view.sections.selected_ids(), &[ids[1]]);
            assert_eq!(
                view.last_transport_command, None,
                "selection never dispatches a transport jump"
            );
            let expected = view
                .state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .beat_to_frame(hex_project::BeatTime::from_quarters(8).unwrap())
                .unwrap();
            assert_eq!(view.playback_start_frame(), expected);
            expected
        });
        assert!(
            visual.debug_bounds("edit-cursor-layer").is_none(),
            "selected sections hide the retained point cursor like clips"
        );
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.last_transport_command, Some(PlayFrom(expected)))
        });
        let first = editor.read_with(&visual, |view, _| position(view, 2));
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(first, MouseButton::Left, shift);
        visual.simulate_mouse_up(first, MouseButton::Left, shift);
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.sections.selected_ids().len(), 2);
            assert_eq!(
                view.playback_start_frame(),
                0,
                "earliest selected section wins"
            );
        });
        editor.update(&mut visual, |view, _| {
            let track = view.edit_cursor.unwrap().track.unwrap();
            view.time_selection = Some(TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 10_000,
                end_frame: 5_000,
            });
            assert_eq!(
                view.playback_start_frame(),
                5_000,
                "time ranges keep clip precedence"
            );
            view.locked_play_start = view.edit_cursor;
            assert_eq!(
                view.playback_start_frame(),
                73_000,
                "explicit lock keeps clip precedence"
            );
            view.locked_play_start = None;
            view.time_selection = None;
            view.selected_track = Some(track);
            assert_eq!(
                view.selected_arrangement_play_start(),
                None,
                "Q-retained section IDs are not a new Main selection"
            );
            view.selected_track = None;
            view.device_panel.main_selected = Some(view.project_generation.wrapping_add(1));
            assert_eq!(
                view.selected_arrangement_play_start(),
                None,
                "old Main ownership cannot select reused section IDs"
            );
            view.device_panel.main_selected = Some(view.project_generation);
            view.install_drop_cursor(super::super::edit_cursor::DropCursor {
                track,
                frame: 48_000,
                generation: view.project_generation,
                epoch: view.selection_intent_epoch.unwrap(),
            });
            assert_eq!(view.playback_start_frame(), 48_000);
            assert_eq!(view.sections.selected_ids().len(), 2);
            view.advance_selection_intent();
            assert_eq!(view.playback_start_frame(), 0);
        });
    }
    #[gpui::test]
    fn section_edit_receipts_follow_canonical_selection_without_seeking(cx: &mut TestAppContext) {
        use super::super::arrangement_sections::tests::next;
        let (handle, editor, ids) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, cx| {
            view.section_autoplay = false;
            view.last_transport_command = None;
            cx.notify();
        });
        visual.run_until_parked();
        let down = editor.read_with(&visual, |view, _| position(view, 2));
        let target = editor.read_with(&visual, |view, _| position(view, 13));
        let option = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, option);
        visual.simulate_mouse_move(target, MouseButton::Left, option);
        visual.simulate_mouse_up(target, MouseButton::Left, option);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.playback_start_frame(),
                0,
                "held/released copy preview does not replace canonical play start"
            )
        });
        let event = editor.update(&mut visual, |view, _| next(&view.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(&mut visual, |view, cx| {
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        let copy = editor.read_with(&visual, |view, _| {
            let copy = view.sections.selected.unwrap();
            assert_ne!(copy, ids[0]);
            assert_eq!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(copy)
                    .unwrap()
                    .start(),
                hex_project::BeatTime::from_quarters(13).unwrap()
            );
            assert_eq!(
                view.playback_start_frame(),
                13 * 24_000,
                "committed copy selection supplies its actual start"
            );
            copy
        });
        let down = editor.read_with(&visual, |view, _| position(view, 14));
        let target = editor.read_with(&visual, |view, _| position(view, 16));
        let marker_modifiers = Modifiers {
            alt: true,
            ..Modifiers::command()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, marker_modifiers);
        visual.simulate_mouse_move(target, MouseButton::Left, marker_modifiers);
        visual.simulate_mouse_up(target, MouseButton::Left, marker_modifiers);
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.playback_start_frame(),
                13 * 24_000,
                "move receipt retains canonical start"
            )
        });
        let event = editor.update(&mut visual, |view, _| next(&view.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(&mut visual, |view, cx| {
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.sections.selected, Some(copy));
            assert_eq!(view.playback_start_frame(), 15 * 24_000);
            assert_eq!(
                view.last_transport_command, None,
                "neither edit seeks playback"
            );
        });
        visual.simulate_keystrokes("cmd-d");
        let event = editor.update(&mut visual, |view, _| next(&view.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(&mut visual, |view, cx| {
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.sections.selected,
                Some(copy),
                "ordinary duplicate preserves its existing selection contract"
            );
            assert_eq!(view.playback_start_frame(), 15 * 24_000);
        });
        visual.simulate_keystrokes("backspace");
        let event = editor.update(&mut visual, |view, _| next(&view.service));
        assert!(event.error.is_none(), "{:?}", event.error);
        editor.update(&mut visual, |view, cx| {
            view.receive(event);
            cx.notify();
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .arrangement_section(copy)
                    .is_none()
            );
            assert_eq!(
                view.selected_arrangement_play_start(),
                None,
                "deleted selection cannot keep supplying a start"
            );
            assert_eq!(view.last_transport_command, None);
        });
    }
}
