//! Thin Arrangement bounce routing; rendering/adoption belong to the worker.
//! Plain `b` bounces the selection to new tracks; `cmd-b` also deactivates the
//! bounced sources in the same adoption edit, matching contextual `c`/`0`.
//! Shift selects one mixed bounce for a time range with either shortcut.
//! `b` held at the release of an Arrangement clip drag drops a bounce of the
//! dragged clips at the drop position instead of moving them.
use super::{EditorMode, EditorView, drag::ClipDrag, drag::DropPreview};
use gpui::{KeyUpEvent, Window};
use hex_project::{BounceAdoption, BounceDrop, BounceTarget, TrackId};
use hex_session::service::{Event, ExportControl, Request};

const DRAG_BOUNCE_HINT: &str = "b held · drop bounces the dragged clips · release b to move";

/// Released bounce drop: the frozen destination keeps painting, with sources
/// visible, until its own bounce job's terminal installs the adopted state.
pub(super) struct PendingBounceDrop {
    drag: ClipDrag,
    control: ExportControl,
    cursor_epoch: Option<u64>,
}

impl PendingBounceDrop {
    pub(super) fn complete(self, event: &Event) -> Option<super::edit_cursor::DropCursor> {
        let cursor =
            super::pending_clip_move::committed_drop_cursor(&self.drag, self.cursor_epoch, event);
        self.drag.retire();
        cursor
    }
}

impl EditorView {
    /// `b` during a held Arrangement clip drag arms a bounce drop instead of
    /// bouncing the selection. Returns false outside a drag.
    pub(super) fn arm_drag_bounce(&mut self) -> bool {
        if self.active_clip_drag.is_none() {
            return false;
        }
        if !self.drag_bounce_held {
            self.drag_bounce_held = true;
            self.status = DRAG_BOUNCE_HINT.into();
        }
        true
    }

    pub(super) fn drag_bounce_key_up(&mut self, event: &KeyUpEvent) -> bool {
        if event.keystroke.key != "b" || !self.drag_bounce_held {
            return false;
        }
        self.drag_bounce_held = false;
        if self.status == DRAG_BOUNCE_HINT {
            self.status.clear();
        }
        true
    }

    pub(super) fn pending_bounce_drag(&self) -> Option<&ClipDrag> {
        self.pending_bounce_drop
            .as_ref()
            .filter(|p| {
                p.drag.project_generation == self.project_generation
                    && self
                        .bounce_control
                        .as_ref()
                        .is_some_and(|active| active.same_job(&p.control))
            })
            .map(|p| &p.drag)
    }

    /// Validate and submit a held-`b` drop; the sources never move.
    pub(super) fn drop_clip_bounce(
        &mut self,
        drag: &ClipDrag,
        target_track: TrackId,
        preview: DropPreview,
    ) {
        if self.status == DRAG_BOUNCE_HINT {
            self.status.clear();
        }
        let Some(state) = &self.state else {
            self.retire_active_clip_drag();
            return;
        };
        let clips = {
            let group = drag.group.borrow();
            if group.ids.len() > 1 {
                group.ids.clone()
            } else {
                vec![drag.clip]
            }
        };
        let target = BounceTarget::Clips(clips);
        let drop = BounceDrop {
            anchor: drag.clip,
            track: target_track,
            position: super::drag::group_position(preview.position),
        };
        if let Err(reason) = state.project.validate_bounce_drop(&target, drop) {
            self.error = Some(reason.to_string().into());
            self.retire_active_clip_drag();
            return;
        }
        // Input capture ends here; the bounce gate refuses while a drag is held.
        let Some(active) = self.active_clip_drag.take() else {
            return;
        };
        active.copy_modifier.set(true);
        active.preview.set(Some(preview));
        self.start_arrangement_bounce(target, false, BounceAdoption::Drop(drop));
        match self.bounce_control.clone() {
            Some(control) => {
                self.pending_bounce_drop = Some(PendingBounceDrop {
                    drag: active,
                    control,
                    cursor_epoch: self.selection_intent_epoch,
                });
            }
            None => active.retire(),
        }
    }

    pub(super) fn bounce_arrangement_selection(
        &mut self,
        window: &Window,
        mute_sources: bool,
        merged: bool,
    ) {
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
        let target = if self
            .time_selection
            .is_some_and(|s| s.anchor_frame != s.end_frame)
        {
            let Some(range) = self.normalized_time_selection() else {
                self.error =
                    Some("bounce time selection no longer belongs to current tracks".into());
                return;
            };
            if merged {
                BounceTarget::MergedRange(range)
            } else {
                BounceTarget::Range(range)
            }
        } else if merged {
            self.error = Some("select an arrangement time range to bounce to one track".into());
            return;
        } else {
            BounceTarget::Clips(self.selected_clip_ids())
        };
        self.start_arrangement_bounce(
            target,
            false,
            if mute_sources {
                BounceAdoption::NewTracksMutingSources
            } else {
                BounceAdoption::NewTracks
            },
        );
    }

    pub(super) fn bounce_track_in_place(
        &mut self,
        track: hex_project::TrackId,
        generation: u64,
        revision: u64,
    ) {
        if generation != self.project_generation
            || self.state.as_ref().is_none_or(|s| s.revision != revision)
        {
            return;
        }
        self.start_arrangement_bounce(BounceTarget::Track(track), true, BounceAdoption::NewTracks);
    }

    fn start_arrangement_bounce(
        &mut self,
        target: BounceTarget,
        in_place: bool,
        adoption: BounceAdoption,
    ) {
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
            || self.midi_note_gesture.is_some()
            || self.midi_group_gesture.is_some()
            || self.midi_stroke.is_some()
            || self.midi_ruler.held()
            || self.midi_time_selection.gesture.is_some()
            || self.audio_event_editor.gesture.is_some()
            || self.clip_loop_held()
        {
            self.error = Some("finish or cancel the current edit before bouncing".into());
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        if let Err(reason) = state
            .project
            .validate_bounce_adoption(&target, in_place, adoption)
            .and_then(|()| state.project.plan_arrangement_bounce(&target, in_place))
        {
            self.error = Some(reason.to_string().into());
            return;
        }
        let control = ExportControl::default();
        if self.enqueue(Request::BounceArrangement {
            project_generation: self.project_generation,
            revision: state.revision,
            target,
            in_place,
            adoption,
            control: control.clone(),
        }) {
            self.bounce_control = Some(control);
            self.status = "bouncing audio · escape cancels".into();
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
    fn rendered_shift_b_bounces_a_multitrack_range_to_one_track(cx: &mut TestAppContext) {
        let (handle, editor, _, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let (tracks, before) = editor.update(&mut visual, |view, cx| {
            view.viewport = Some(crate::editor::viewport::Viewport {
                start: 0,
                length: 96_000,
            });
            view.snap_grid = crate::editor::musical_time::SnapGrid::Off;
            cx.notify();
            let project = &view.state.as_ref().unwrap().project;
            let tracks = project
                .tracks()
                .iter()
                .map(hex_project::Track::id)
                .collect::<Vec<_>>();
            (tracks, project.snapshot().tracks)
        });
        visual.run_until_parked();
        let lane_id = |index: usize| -> &'static str {
            Box::leak(format!("arrangement-lane-{}", tracks[index].value()).into_boxed_str())
        };
        let audio_id = lane_id(0);
        let midi_id = lane_id(1);
        for mute in [false, true] {
            let midi = visual.debug_bounds(midi_id).unwrap();
            let audio = visual.debug_bounds(audio_id).unwrap();
            let from = gpui::point(midi.origin.x + midi.size.width * 0.3, midi.center().y);
            let to = gpui::point(audio.origin.x + audio.size.width * 0.85, audio.center().y);
            visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let range =
                editor.read_with(&visual, |view, _| view.normalized_time_selection().unwrap());
            assert_eq!(range.tracks(), tracks);
            visual.simulate_keystrokes(if mute { "cmd-shift-b" } else { "shift-b" });
            pump(&editor, &mut visual);
            let after = editor.read_with(&visual, |view, _| {
                assert!(view.error.is_none(), "{:?}", view.error);
                let project = &view.state.as_ref().unwrap().project;
                assert_eq!(project.tracks().len(), 3);
                let [clip] = project.tracks()[2].clips() else {
                    panic!("one bounced clip")
                };
                assert_eq!(
                    (clip.start(), clip.duration()),
                    (range.start(), range.duration())
                );
                assert_eq!(
                    project.time_range_has_active_clips(&range).unwrap(),
                    Some(!mute)
                );
                let snapshot = project.snapshot().tracks;
                if !mute {
                    assert_eq!(snapshot[..2], before);
                }
                snapshot
            });
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |view, _| {
                assert_eq!(
                    view.state.as_ref().unwrap().project.snapshot().tracks,
                    before
                );
            });
            if mute {
                visual.simulate_keystrokes("cmd-shift-z");
                pump(&editor, &mut visual);
                editor.read_with(&visual, |view, _| {
                    assert_eq!(
                        view.state.as_ref().unwrap().project.snapshot().tracks,
                        after
                    );
                });
            }
        }
    }

    #[gpui::test]
    fn rendered_b_bounces_a_group_range_without_touching_the_source_rows(cx: &mut TestAppContext) {
        check_rendered_group_range(cx, false);
    }

    #[gpui::test]
    fn rendered_cmd_b_commits_the_group_gate_and_bounce_in_one_undo(cx: &mut TestAppContext) {
        check_rendered_group_range(cx, true);
    }

    fn check_rendered_group_range(cx: &mut TestAppContext, mute: bool) {
        use hex_project::{Command, TrackSelectionIds};
        use hex_session::service::Request;

        let (handle, editor, _, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        editor.update(&mut visual, |v, _| {
            let tracks: Vec<_> = v
                .state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .map(hex_project::Track::id)
                .collect();
            assert!(v.enqueue(Request::Edit(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&tracks).unwrap(),
                name: "sources".into(),
            })));
        });
        pump(&editor, &mut visual);
        let (group, before) = editor.update(&mut visual, |v, cx| {
            let project = &v.state.as_ref().unwrap().project;
            let group = &project.tracks()[0];
            assert!(group.is_group());
            v.viewport = Some(crate::editor::viewport::Viewport {
                start: 0,
                length: 96_000,
            });
            v.snap_grid = crate::editor::musical_time::SnapGrid::Off;
            cx.notify();
            (group.id(), project.snapshot())
        });
        visual.run_until_parked();
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", group.value()).into_boxed_str(),
            ))
            .unwrap();
        let from = gpui::point(lane.origin.x + lane.size.width * 0.4, lane.center().y);
        let to = gpui::point(lane.origin.x + lane.size.width * 0.6, lane.center().y);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let range = editor.read_with(&visual, |v, _| v.normalized_time_selection().unwrap());
        assert_eq!(range.tracks(), &[group]);
        assert!(range.start() < 48_000 && range.end() > 48_000);
        visual.simulate_keystrokes(if mute { "cmd-b" } else { "b" });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            let mut tracks = p.snapshot().tracks;
            let bounce = p.tracks().last().unwrap();
            assert_eq!(bounce.parent_group(), None);
            let clip = &bounce.clips()[0];
            assert_eq!(
                (clip.start(), clip.duration()),
                (range.start(), range.duration())
            );
            tracks.pop();
            if mute {
                assert_eq!(
                    tracks[0].group_output_mutes,
                    vec![
                        hex_project::GroupOutputMuteRange::new(range.start(), range.end()).unwrap()
                    ]
                );
                tracks[0].group_output_mutes.clear();
            }
            assert_eq!(tracks, before.tracks);
        });
        if mute {
            visual.simulate_keystrokes("cmd-z");
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.state.as_ref().unwrap().project.snapshot().tracks,
                    before.tracks
                );
            });
        }
    }
    #[gpui::test]
    fn rendered_b_bounces_and_cmd_b_also_mutes_sources_with_one_undo(cx: &mut TestAppContext) {
        let (handle, editor, audio, _, files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        let original =
            editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("b");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), original.tracks.len() + 1);
            assert!(p.clip(audio).unwrap().active());
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
        visual.simulate_keystrokes("cmd-b");
        pump(&editor, &mut visual);
        let muted = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), original.tracks.len() + 1);
            assert!(!p.clip(audio).unwrap().active());
            assert!(p.tracks()[1].clips()[0].active());
            assert!(v.bounce_control.is_none());
            p.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = v.state.as_ref().unwrap().project.snapshot();
            assert_eq!(p.tracks, original.tracks);
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot().tracks,
                muted.tracks
            );
        });
    }
    /// Start a rendered drag of `clip` and hover the middle of `track`'s lane.
    fn hover_drag(
        visual: &mut VisualTestContext,
        clip: hex_project::ClipId,
        track: hex_project::TrackId,
    ) -> gpui::Point<gpui::Pixels> {
        let surface = visual
            .debug_bounds(Box::leak(
                format!("clip-drag-surface-{}", clip.value()).into_boxed_str(),
            ))
            .expect("audio drag surface");
        let lane = visual
            .debug_bounds(Box::leak(
                format!("arrangement-lane-{}", track.value()).into_boxed_str(),
            ))
            .expect("destination lane");
        let down = surface.center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            gpui::point(down.x + gpui::px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let target = gpui::point(down.x + gpui::px(40.0), lane.center().y);
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        target
    }
    fn b_up(visual: &mut VisualTestContext) {
        visual.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("b").unwrap(),
        });
        visual.run_until_parked();
    }
    #[gpui::test]
    fn held_b_drop_bounces_at_the_drop_and_leaves_the_source(cx: &mut TestAppContext) {
        let (handle, editor, audio, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (original, source_track, target_track) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            (p.snapshot(), p.tracks()[0].id(), p.tracks()[1].id())
        });
        let release = hover_drag(&mut visual, audio, target_track);
        visual.simulate_keystrokes("b");
        visual.run_until_parked();
        let start = editor.read_with(&visual, |v, _| {
            assert!(v.drag_bounce_held);
            assert_eq!(v.status, super::DRAG_BOUNCE_HINT);
            assert!(
                v.bounce_control.is_none(),
                "b never bounces the selection mid-drag"
            );
            let preview = v.active_clip_drag.as_ref().unwrap().preview.get();
            preview.expect("lane preview").start_frame
        });
        let x = visual.debug_bounds("clip-drop-preview").unwrap().left();
        crate::editor::clip_drag_ui_test::assert_clip_move_guides(&mut visual, x);
        visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        // Before the worker's terminal is received the frozen ghost still paints
        // at the destination and the source remains where it was.
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_bounce_drag().is_some());
            assert!(v.active_clip_drag.is_none() && !v.drag_bounce_held);
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.snapshot().tracks, original.tracks);
        });
        assert!(visual.debug_bounds("clip-drop-preview").is_some());
        crate::editor::clip_drag_ui_test::assert_clip_move_guides(&mut visual, x);
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("clip-name-{}", audio.value()).into_boxed_str()
                ))
                .is_some(),
            "the source clip keeps painting at its original place"
        );
        pump(&editor, &mut visual);
        let dropped = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(v.pending_bounce_drop.is_none() && v.bounce_control.is_none());
            let cursor = v.edit_cursor.expect("cursor at the adopted bounce");
            assert_eq!((cursor.track, cursor.frame), (Some(target_track), start));
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), original.tracks.len());
            let source = p.clip(audio).unwrap();
            assert_eq!(
                (source.start(), source.duration(), source.active()),
                (0, 48_000, true)
            );
            assert_eq!(p.track(source_track).unwrap().clips().len(), 1);
            let bounced = &p.track(target_track).unwrap().clips()[0];
            assert_eq!((bounced.start(), bounced.duration()), (start, 48_000));
            p.snapshot()
        });
        assert!(visual.debug_bounds("clip-drop-preview").is_none());
        assert!(visual.debug_bounds("clip-move-guide-line").is_none());
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = v.state.as_ref().unwrap().project.snapshot();
            assert_eq!(p.tracks, original.tracks);
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let p = v.state.as_ref().unwrap().project.snapshot();
            assert_eq!(p.tracks, dropped.tracks);
        });
    }
    #[gpui::test]
    fn b_released_before_the_drop_moves_normally(cx: &mut TestAppContext) {
        let (handle, editor, audio, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let (tracks, target_track) = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            (p.tracks().len(), p.tracks()[1].id())
        });
        let release = hover_drag(&mut visual, audio, target_track);
        visual.simulate_keystrokes("b");
        b_up(&mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(!v.drag_bounce_held);
            assert!(v.status.is_empty());
        });
        visual.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(v.pending_bounce_drop.is_none() && v.bounce_control.is_none());
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), tracks);
            assert!(p.tracks()[0].clips().is_empty(), "the source moved");
            assert!(
                p.track(target_track)
                    .unwrap()
                    .clips()
                    .iter()
                    .any(|c| c.id() == audio)
            );
        });
    }
    #[gpui::test]
    fn range_precedence_is_exact_and_shortcuts_preserve_input_contexts(cx: &mut TestAppContext) {
        let (handle, editor, audio, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        click(&mut visual, audio, false);
        let (before, tracks) = editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            (state.revision, state.project.tracks().len())
        });
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
            let p = &v.state.as_ref().unwrap().project;
            let clip = &p.tracks()[1].clips()[0];
            assert_eq!((clip.start(), clip.duration()), (12_013, 23_994));
            // Same fragments as contextual `c`/`0` on that time selection.
            let sources = p.tracks()[0]
                .clips()
                .iter()
                .map(|c| (c.start(), c.duration(), c.active()))
                .collect::<Vec<_>>();
            assert_eq!(
                sources,
                [
                    (0, 12_013, true),
                    (12_013, 23_994, false),
                    (36_007, 11_993, true)
                ]
            );
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            assert_eq!(p.tracks().len(), tracks);
            assert_eq!(p.tracks()[0].clips().len(), 1);
            assert!(p.tracks()[0].clips()[0].active());
            let track = p.tracks()[0].id();
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
            let p = &v.state.as_ref().unwrap().project;
            let clip = &p.tracks()[1].clips()[0];
            assert_eq!((clip.start(), clip.duration()), (12_013, 23_994));
            assert_eq!(p.tracks()[0].clips().len(), 1);
            let source = &p.tracks()[0].clips()[0];
            assert_eq!((source.start(), source.duration()), (0, 48_000));
            assert!(source.active());
            assert!(v.bounce_control.is_none());
        });
    }
    #[gpui::test]
    fn track_header_bounce_in_place_ignores_smaller_clip_and_time_selections(
        cx: &mut TestAppContext,
    ) {
        for console in [false, true] {
            check_track_header_console_bounce(cx, console);
        }
    }
    fn check_track_header_console_bounce(cx: &mut TestAppContext, console: bool) {
        use hex_project::{Command, TimeRange};
        use hex_session::service::Request;
        let (handle, editor, audio, _, _files) = fixture_with_bounce_media(cx, true);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(900.0)));
        visual.run_until_parked();
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::DuplicateClip(audio))));
        });
        pump(&editor, &mut visual);
        click(&mut visual, audio, false);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddScene {
                name: "console".into()
            })));
        });
        pump(&editor, &mut visual);
        let scene = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.scenes()[0].id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(
                v.enqueue(Request::Edit(Command::CopyArrangementAudioClipToSession {
                    source: audio,
                    scene
                }))
            );
        });
        pump(&editor, &mut visual);
        let cell = editor.read_with(&visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let hex_project::SessionCellContent::Audio(c) =
                p.session_cell(p.tracks()[0].id(), scene).unwrap()
            else {
                panic!("cell")
            };
            c.id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetSessionAudioClipLoop {
                clip: cell,
                loop_range: Some(hex_project::SessionAudioLoop::new(0, 24000).unwrap())
            })));
        });
        pump(&editor, &mut visual);

        let (track, original) = editor.update(&mut visual, |v, _| {
            let p = &v.state.as_ref().unwrap().project;
            let track = p.tracks()[0].id();
            assert_eq!(p.track(track).unwrap().clips().len(), 2);
            let original = p.snapshot();
            v.time_selection = Some(crate::editor::time_selection::TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 12000,
                end_frame: 24000,
            });
            (track, original)
        });
        editor.update(&mut visual, |v, cx| {
            v.mode = if console {
                crate::editor::EditorMode::Session
            } else {
                crate::editor::EditorMode::Arrangement
            };
            cx.notify();
        });
        visual.run_until_parked();
        let header = visual
            .debug_bounds(Box::leak(
                format!(
                    "{}-{}",
                    if console {
                        "session-track"
                    } else {
                        "track-name"
                    },
                    track.value()
                )
                .into_boxed_str(),
            ))
            .unwrap();
        visual.simulate_mouse_down(header.center(), MouseButton::Right, Modifiers::default());
        visual.simulate_mouse_up(header.center(), MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        let index = editor.read_with(&visual, |v, _| {
            v.context_menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .position(|item| item.label == "bounce in place")
                .expect("actual header menu offers whole-track bounce")
        });
        editor.update(&mut visual, |v, _| {
            // Header selection may clear a range. Keep a smaller range active
            // at dispatch to verify that the typed track intent never reads it.
            v.time_selection = Some(crate::editor::time_selection::TimeSelection {
                anchor_track: track,
                end_track: track,
                anchor_frame: 12000,
                end_frame: 24000,
            });
            assert_eq!(
                v.normalized_time_selection(),
                Some(TimeRange::new(vec![track], 12000, 24000).unwrap())
            );
        });
        for _ in 0..index {
            visual.simulate_keystrokes("down");
        }
        visual.simulate_keystrokes("enter");
        pump(&editor, &mut visual);
        let bounced = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            let t = p.track(track).unwrap();
            assert_eq!(t.clips().len(), 1);
            assert_eq!((t.clips()[0].start(), t.clips()[0].duration()), (0, 96000));
            assert!(p.clip(audio).is_none());
            assert!(t.devices().is_empty() && t.instrument().is_none());
            let c = p.session_audio_clip(cell).unwrap();
            assert_eq!(
                c.loop_range(),
                Some(hex_project::SessionAudioLoop::new(24000, 48000).unwrap())
            );
            assert_eq!(c.id(), cell);

            p.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot().tracks,
                original.tracks
            );
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot().session,
                original.session
            )
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot().tracks,
                bounced.tracks
            );
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot().session,
                bounced.session
            )
        });
    }
}
