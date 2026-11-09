//! View-only Arrangement insertion position and scoped canonical split dispatch.
//! Selection normalization stays with time_selection; project/session own cuts and history.
use super::{EditorMode, EditorView, time_selection::TimeSelection};
use crate::theme;
use gpui::{Context, Div, Window, div, prelude::*, px};
use hex_project::{ClipId, Command, TrackId};
use hex_session::service::Request;
use std::time::{Duration, Instant};

const PULSE_PERIOD: Duration = Duration::from_millis(1_200);

#[derive(Clone, Copy)]
pub(super) struct EditCursor {
    pub(super) track: TrackId,
    pub(super) frame: u64,
    pub(super) generation: u64,
    pub(super) pulse_color: u32,
    pub(super) pulse_started: Instant,
}

impl EditCursor {
    fn pulse(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.pulse_started);
        let phase = (elapsed.as_millis() % PULSE_PERIOD.as_millis()) as f64
            / PULSE_PERIOD.as_millis() as f64;
        let blend = (1.0 - (phase * std::f64::consts::TAU).cos()) * 0.5;
        let mut color = 0;
        for shift in [16, 8, 0] {
            let white = f64::from((theme::color(theme::PaletteRole::Text) >> shift) & 0xff);
            let cyan = f64::from((theme::color(theme::PaletteRole::Secondary) >> shift) & 0xff);
            let channel = (white + (cyan - white) * blend).round() as u32;
            color |= channel << shift;
        }
        if color == self.pulse_color {
            return false;
        }
        self.pulse_color = color;
        true
    }

    pub(super) fn color(&self) -> u32 {
        self.pulse_color
    }
}

impl EditorView {
    pub(super) fn valid_locked_play_start(&self) -> Option<EditCursor> {
        self.locked_play_start.filter(|cursor| {
            cursor.generation == self.project_generation
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.project.track(cursor.track).is_some())
        })
    }

    pub(super) fn update_locked_play_start_from_point(&mut self) {
        if self.locked_play_start.is_some() {
            self.locked_play_start = self.edit_cursor;
        }
    }

    fn toggle_play_start_lock(&mut self, cx: &mut Context<Self>) {
        if self.mode != EditorMode::Arrangement
            || self.context_menu.is_some()
            || self.track_name.is_editing()
            || self.clip_name_edit.is_some()
            || self.sections.naming()
            || self.sections.held()
            || self.changelog.is_open()
            || self.project_tempo_editing(cx)
            || self.gain_numeric_editing(cx)
            || self.clip_numeric_editing(cx)
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.preset_dialog_open
            || self.theme_settings.open
            || self.tuning.open
            || self.controller_input_is_open()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.loop_rail_gesture.is_some()
            || self.scrub_active()
            || self.track_resize_active()
            || self.track_reorder_active()
            || cx.has_active_drag()
        {
            return;
        }
        if self.valid_locked_play_start().is_some() {
            self.locked_play_start = None;
        } else if self.normalized_time_selection().is_none() {
            self.locked_play_start = self.edit_cursor.filter(|cursor| {
                cursor.generation == self.project_generation
                    && self
                        .state
                        .as_ref()
                        .is_some_and(|state| state.project.track(cursor.track).is_some())
            });
        }
        cx.notify();
    }

    pub(super) fn place_edit_cursor(
        &mut self,
        track: TrackId,
        pointer_x: f32,
        free: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.suppress_cursor_click {
            self.suppress_cursor_click = false;
            return;
        }
        if self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.mode != EditorMode::Arrangement
        {
            return;
        }
        let Some(state) = &self.state else {
            return;
        };
        if state.project.track(track).is_none() {
            return;
        }
        let Some(lane) = self.drag_lanes.get(&track) else {
            return;
        };
        let viewport = self.visible_viewport(state.timeline_frames);
        let Some(frame) = super::seek::pointer_to_frame(pointer_x, lane.left, lane.width, viewport)
        else {
            return;
        };
        let Some(frame) = self.snap_grid.with_platform_modifier(free).snap_frame(
            &state.project,
            frame,
            viewport,
            lane.width,
        ) else {
            return;
        };
        self.edit_cursor = Some(EditCursor {
            track,
            frame,
            generation: self.project_generation,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: Instant::now(),
        });
        self.update_locked_play_start_from_point();
        self.time_selection = Some(TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: frame,
            end_frame: frame,
        });
        self.time_selection_project_scope = false;
        let under_pointer = self.arrangement_clip_at(track, frame, self.selected_clip);
        let selected_clip_under_pointer =
            under_pointer.is_some() && under_pointer == self.selected_clip;
        self.select_lane_track(track);
        self.edit_cursor_in_selected_clip =
            self.selected_clip.filter(|_| selected_clip_under_pointer);
        self.loop_start_over_clip = None;
        self.advance_selection_intent();
        if under_pointer.is_none() {
            self.focus_track_details(window);
        } else if !selected_clip_under_pointer {
            // A cursor inside a clip keeps it in the clip editor (see detail_context).
            self.set_detail_context(under_pointer);
            self.focus_detail_context(window, cx);
        }
        self.focus.focus(window);
        cx.notify();
    }

    /// An acknowledged loop enable hands the next Play start to the loop start
    /// by retiring a placed point cursor; a new click overrides it again. Ranges
    /// keep precedence, and disabling the loop never restores the cursor.
    pub(super) fn retire_point_cursor_on_loop_enable(&mut self, loop_was_enabled: bool) {
        let loop_enabled = self
            .state
            .as_ref()
            .and_then(|state| state.project.loop_region())
            .is_some_and(|region| region.enabled());
        if loop_was_enabled
            || !loop_enabled
            || self.pending_selection_loop_has_newer_intent()
            || self.edit_cursor.is_none()
            || self.normalized_time_selection().is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
        {
            return;
        }
        self.loop_start_over_clip = self.edit_cursor_in_selected_clip.take();
        self.edit_cursor = None;
        self.time_selection = None;
        self.time_selection_project_scope = false;
        self.suppress_cursor_click = false;
    }

    pub(super) fn tick_edit_cursor(&mut self, now: Instant) -> bool {
        if self.locked_play_start.is_some() && self.valid_locked_play_start().is_none() {
            self.locked_play_start = None;
        }
        if self.context_menu.is_some()
            || self.close_prompt_open
            || (self.mode != EditorMode::Arrangement
                && (self.time_selection_gesture.is_some()
                    || self.time_range_move_gesture.is_some()))
            || self.save_dialog_open
            || self.export_dialog_open
        {
            return self.cancel_time_selection_gesture();
        }
        if self.time_selection_project_scope {
            let first_last = self.state.as_ref().and_then(|state| {
                Some((
                    state.project.tracks().first()?.id(),
                    state.project.tracks().last()?.id(),
                ))
            });
            if let Some((first, last)) = first_last {
                if let Some(cursor) = self.edit_cursor.as_mut() {
                    cursor.track = first;
                }
                if let Some(selection) = self.time_selection.as_mut() {
                    selection.anchor_track = first;
                    selection.end_track = last;
                }
            } else {
                self.edit_cursor = None;
                self.time_selection = None;
                self.time_selection_project_scope = false;
                self.time_selection_gesture = None;
                self.suppress_cursor_click = false;
                return true;
            }
        }
        let Some(cursor) = &mut self.edit_cursor else {
            return self.cancel_time_selection_gesture();
        };
        if cursor.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.project.track(cursor.track).is_none())
        {
            self.edit_cursor = None;
            self.time_selection = None;
            self.time_selection_project_scope = false;
            self.time_selection_gesture = None;
            self.suppress_cursor_click = false;
            return true;
        }
        if self.time_selection.is_some_and(|selection| {
            self.state.as_ref().is_none_or(|state| {
                state.project.track(selection.anchor_track).is_none()
                    || state.project.track(selection.end_track).is_none()
            })
        }) {
            self.time_selection = None;
            self.time_selection_project_scope = false;
            self.time_selection_gesture = None;
            self.suppress_cursor_click = false;
            return true;
        }
        if self
            .time_selection
            .is_some_and(|selection| selection.anchor_frame != selection.end_frame)
        {
            return false;
        }
        // A completed range belongs to Arrangement even while Console is shown.
        // Its stale-source checks above still run; hidden point cursors need no pulse.
        // A point cursor hidden behind a rectangle play start needs no pulse either.
        self.mode == EditorMode::Arrangement
            && self.selected_arrangement_play_start().is_none()
            && self
                .edit_cursor
                .as_mut()
                .is_some_and(|cursor| cursor.pulse(now))
    }

    /// Paint the same live range bounds above the loop rail and at the bottom
    /// of Arrangement. No canonical state or pointer-capture ownership changes.
    pub(super) fn edit_cursor_markers(
        &self,
        viewport: super::viewport::Viewport,
        edge: hex_widgets::TimeMarkerEdge,
        guide: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        use hex_widgets::{TimeMarkerEndpoint as Endpoint, time_position_marker};
        let locked = self.valid_locked_play_start();
        let cursor = self
            .edit_cursor
            .filter(|cursor| cursor.generation == self.project_generation);
        let cursor = cursor.or(locked);
        let cursor_frame = cursor.map(|cursor| cursor.frame).or(guide)?;
        let (start, end) = self
            .time_selection
            .map_or((cursor_frame, cursor_frame), |selection| {
                (
                    selection.anchor_frame.min(selection.end_frame),
                    selection.anchor_frame.max(selection.end_frame),
                )
            });
        let edge_name = match edge {
            hex_widgets::TimeMarkerEdge::Top => "top",
            hex_widgets::TimeMarkerEdge::Bottom => "bottom",
        };
        if guide.is_none()
            && locked.is_none()
            && start == end
            && self.selected_arrangement_play_start().is_some()
        {
            return None;
        }
        let mut markers = Vec::new();
        if guide.is_none() && start != end {
            for (frame, endpoint, name) in [
                (start, Endpoint::Start, "start"),
                (end, Endpoint::End, "end"),
            ] {
                if let Some(marker) = viewport.fraction(frame).and_then(|position| {
                    time_position_marker(
                        format!("time-marker-{edge_name}-{name}").into(),
                        position,
                        endpoint,
                        edge,
                        theme::time_position_marker_color(),
                    )
                }) {
                    markers.push(marker.into_any_element());
                }
            }
        }
        if let Some(frame) = guide
            && let Some(marker) = viewport.fraction(frame).and_then(|position| {
                time_position_marker(
                    format!("clip-move-guide-{edge_name}").into(),
                    position,
                    Endpoint::Point,
                    edge,
                    theme::time_position_marker_color(),
                )
            })
        {
            markers.push(marker.into_any_element());
        }
        let point = locked.or_else(|| {
            (guide.is_none() && start == end && self.selected_arrangement_play_start().is_none())
                .then_some(cursor)
                .flatten()
        });
        if let Some(point) = point
            && let Some(marker) = viewport.fraction(point.frame).and_then(|position| {
                time_position_marker(
                    if locked.is_some() {
                        format!("locked-play-start-{edge_name}").into()
                    } else {
                        format!("time-marker-{edge_name}-start").into()
                    },
                    position,
                    Endpoint::Point,
                    edge,
                    if locked.is_some() {
                        theme::locked_time_position_marker_color()
                    } else {
                        theme::time_position_marker_color()
                    },
                )
            })
        {
            markers.push(
                marker
                    .id(if edge == hex_widgets::TimeMarkerEdge::Top {
                        "play-start-lock-top"
                    } else {
                        "play-start-lock-bottom"
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                            if event.modifiers == gpui::Modifiers::default() {
                                view.toggle_play_start_lock(cx);
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .into_any_element(),
            );
        }
        Some(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(hex_widgets::TIME_POSITION_MARKER_HEIGHT))
                .children(markers),
        )
    }

    pub(super) fn edit_cursor_layer(
        &self,
        viewport: super::viewport::Viewport,
        guide: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let markers =
            self.edit_cursor_markers(viewport, hex_widgets::TimeMarkerEdge::Bottom, guide, cx)?;
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(px(self.track_header_width()))
                .debug_selector(|| "edit-cursor-layer".into())
                .children([markers]),
        )
    }

    pub(super) fn split_arrangement_clips(&mut self, window: &Window, cx: &Context<Self>) {
        if self.mode != EditorMode::Arrangement {
            self.error = Some("split is available in arrangement".into());
            return;
        }
        // Ordinary track headers belong to the same arranger editing scope.
        let arranger_focused = self.focus.is_focused(window) || self.track_header_focused(window);
        if (!arranger_focused && !self.midi_focus.is_focused(window))
            || !self.arrangement_view_change_available(cx)
            || self.device_panel.has_pending_draft(cx)
            || (arranger_focused && self.automation_view.enabled)
        {
            return;
        }
        if arranger_focused
            && self
                .time_selection
                .is_some_and(|s| s.anchor_frame != s.end_frame)
        {
            let Some(range) = self.normalized_time_selection() else {
                self.error = Some("time selection is no longer available".into());
                return;
            };
            self.cancel_duplicate_chain();
            self.enqueue(Request::Edit(Command::SplitTimeRange { range }));
            return;
        }
        let Some(cursor) = self
            .edit_cursor
            .filter(|cursor| cursor.generation == self.project_generation)
        else {
            self.error = Some("click a track to place the edit cursor".into());
            return;
        };
        let Some(state) = &self.state else {
            return;
        };
        if self.time_selection_project_scope {
            let tracks: Vec<_> = state
                .project
                .tracks()
                .iter()
                .map(|track| track.id())
                .collect();
            let beat = state.project.tempo_map().frame_to_beat(cursor.frame).ok();
            let has_crossing_clip = state.project.tracks().iter().any(|track| {
                track.clips().iter().any(|clip| {
                    cursor.frame > clip.start()
                        && clip
                            .start()
                            .checked_add(clip.duration())
                            .is_some_and(|end| cursor.frame < end)
                }) || beat.is_some_and(|beat| {
                    track.midi_clips().iter().any(|clip| {
                        beat > clip.start()
                            && clip
                                .start()
                                .raw()
                                .checked_add(clip.length().raw())
                                .is_some_and(|end| beat.raw() < end)
                    })
                })
            });
            if !has_crossing_clip {
                self.error = Some("edit cursor must be inside a clip to split".into());
                return;
            }
            self.cancel_duplicate_chain();
            if self.enqueue(Request::Edit(Command::SplitTracksAtFrame {
                tracks,
                frame: cursor.frame,
            })) {
                self.selected_clip = None;
            }
            return;
        }
        let Some(track) = state.project.track(cursor.track) else {
            self.error = Some("edit cursor track no longer exists".into());
            return;
        };
        let audio = track.clips().iter().filter(|clip| {
            cursor.frame > clip.start()
                && clip
                    .start()
                    .checked_add(clip.duration())
                    .is_some_and(|end| cursor.frame < end)
        });
        let beat = state.project.tempo_map().frame_to_beat(cursor.frame).ok();
        let midi = track.midi_clips().iter().filter(|clip| {
            beat.is_some_and(|beat| {
                beat > clip.start()
                    && clip
                        .start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .is_some_and(|end| beat.raw() < end)
            })
        });
        let mut commands: Vec<(ClipId, Command)> = audio
            .map(|clip| {
                (
                    clip.id(),
                    Command::SplitClip {
                        clip: clip.id(),
                        frame: cursor.frame,
                    },
                )
            })
            .chain(midi.filter_map(|clip| {
                beat.map(|beat| {
                    (
                        clip.id(),
                        Command::SplitMidiClip {
                            clip: clip.id(),
                            beat,
                        },
                    )
                })
            }))
            .collect();
        let selected = self
            .selected_clip
            .and_then(|id| commands.iter().position(|(clip, _)| *clip == id))
            .unwrap_or(0);
        if commands.is_empty() {
            self.error = Some("edit cursor must be inside a clip to split".into());
            return;
        }
        let (clip, command) = commands.swap_remove(selected);
        self.cancel_duplicate_chain();
        if self.enqueue(Request::Edit(command)) {
            self.selected_clip = Some(clip);
            self.selected_track = Some(cursor.track);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_pulse_interpolates_smoothly_and_uses_elapsed_time_without_drift() {
        let now = Instant::now();
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).expect("project"), 4)
                .expect("editor");
        let hex_project::Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .expect("track")
        else {
            panic!("track");
        };
        let mut cursor = EditCursor {
            track,
            frame: 0,
            generation: 0,
            pulse_color: theme::color(theme::PaletteRole::Text),
            pulse_started: now,
        };
        assert!(!cursor.pulse(now));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Text));
        assert!(cursor.pulse(now + Duration::from_millis(300)));
        let midpoint = cursor.color();
        assert_ne!(midpoint, theme::color(theme::PaletteRole::Text));
        assert_ne!(midpoint, theme::color(theme::PaletteRole::Secondary));
        assert!(cursor.pulse(now + Duration::from_millis(600)));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Secondary));
        assert!(cursor.pulse(now + Duration::from_millis(900)));
        assert_eq!(cursor.color(), midpoint);
        assert!(cursor.pulse(now + Duration::from_millis(1_200)));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Text));
        // A skipped timer tick resumes the same phase, rather than toggling once.
        assert!(cursor.pulse(now + Duration::from_millis(3_000)));
        assert_eq!(cursor.color(), theme::color(theme::PaletteRole::Secondary));
        assert!(!cursor.pulse(now + Duration::from_millis(3_000)));
    }
}
