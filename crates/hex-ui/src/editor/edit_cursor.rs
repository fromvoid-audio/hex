//! View-only Arrangement insertion position, distinct from the audio playhead.
use super::{EditorMode, EditorView, time_selection::TimeSelection};
use crate::theme;
use gpui::{Context, Div, PathBuilder, Window, canvas, div, point, prelude::*, px, rgb};
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
        self.time_selection = Some(TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: frame,
            end_frame: frame,
        });
        self.time_selection_project_scope = false;
        self.selected_track = Some(track);
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn tick_edit_cursor(&mut self, now: Instant) -> bool {
        if self.context_menu.is_some()
            || self.close_prompt_open
            || self.mode != EditorMode::Arrangement
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
        cursor.pulse(now)
    }

    pub(super) fn edit_cursor_layer(&self, viewport: super::viewport::Viewport) -> Option<Div> {
        let cursor = self.edit_cursor?;
        if cursor.generation != self.project_generation {
            return None;
        }
        let fraction = viewport.fraction(cursor.frame)?;
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(px(super::tracks::TRACK_HEADER_WIDTH))
                .debug_selector(|| "edit-cursor-layer".into())
                .children([canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let x = bounds.origin.x + bounds.size.width * fraction;
                        let top = bounds.origin.y;
                        let bottom = bounds.origin.y + bounds.size.height;
                        for points in [
                            [
                                point(x - px(4.0), top),
                                point(x + px(4.0), top),
                                point(x, top + px(5.0)),
                            ],
                            [
                                point(x - px(4.0), bottom),
                                point(x + px(4.0), bottom),
                                point(x, bottom - px(5.0)),
                            ],
                        ] {
                            let mut path = PathBuilder::fill();
                            path.move_to(points[0]);
                            path.line_to(points[1]);
                            path.line_to(points[2]);
                            path.close();
                            if let Ok(path) = path.build() {
                                window
                                    .paint_path(path, rgb(theme::color(theme::PaletteRole::Text)));
                            }
                        }
                    },
                )
                .absolute()
                .inset_0()]),
        )
    }

    pub(super) fn split_at_edit_cursor(&mut self) {
        if self.mode != EditorMode::Arrangement {
            self.error = Some("split is available in arrangement".into());
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
