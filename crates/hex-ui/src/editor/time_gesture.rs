//! Pointer projection and edge scrolling for captured Arrangement time gestures.
use super::time_selection::{TimeRangeMoveGesture, TimeSelectionGesture};
use super::{EditorView, drag::LaneBounds, viewport::Viewport};
use hex_project::TrackId;
use std::time::Instant;

impl EditorView {
    pub(super) fn refresh_time_gesture_modifiers(&mut self, free: bool, copy: bool) -> bool {
        let mut changed = false;
        let selection_pointer = self.time_selection_gesture.as_mut().map(|gesture| {
            if gesture.free_modifier != free {
                gesture.free_modifier = free;
                changed = true;
            }
            (gesture.pointer_x, gesture.pointer_y, gesture.free_modifier)
        });
        let range_pointer = self.time_range_move_gesture.as_mut().map(|gesture| {
            if gesture.free_modifier != free || gesture.copy_modifier != copy {
                gesture.free_modifier = free;
                gesture.copy_modifier = copy;
                changed = true;
            }
            (gesture.pointer_x, gesture.pointer_y, gesture.free_modifier)
        });
        if let Some((x, y, free)) = selection_pointer {
            changed |= self.update_time_selection_gesture(x, y, free);
        }
        if let Some((x, y, free)) = range_pointer {
            changed |= self.update_time_range_move_gesture(x, y, free);
        }
        changed
    }

    pub(super) fn tick_time_gesture_autoscroll(&mut self, now: Instant) -> bool {
        // Idle maintenance must not cancel a completed, persistent time range.
        if self.time_selection_gesture.is_none() && self.time_range_move_gesture.is_none() {
            return false;
        }
        let pending_tail = self
            .time_selection_gesture
            .is_some_and(|gesture| gesture.tail_press.is_some() && !gesture.moved);
        let Some(horizon) = self.state.as_ref().map(|state| state.timeline_frames) else {
            return false;
        };
        if !self.time_gesture_is_current() {
            if pending_tail {
                self.time_selection_gesture = None;
            } else {
                self.cancel_time_selection_gesture();
            }
            return false;
        }
        if pending_tail {
            return false;
        }
        let Some((x, y, last_tick)) = self
            .time_range_move_gesture
            .as_ref()
            .map(|gesture| (gesture.pointer_x, gesture.pointer_y, gesture.last_tick))
            .or_else(|| {
                self.time_selection_gesture
                    .as_ref()
                    .map(|gesture| (gesture.pointer_x, gesture.pointer_y, gesture.last_tick))
            })
        else {
            return false;
        };
        let Some((_, lane)) = self.time_gesture_lane_near_y(y) else {
            update_last_tick(
                &mut self.time_selection_gesture,
                &mut self.time_range_move_gesture,
                now,
            );
            return false;
        };
        let horizon = self.viewport_horizon(horizon);
        let viewport = self.visible_viewport(horizon);
        let edge = edge_direction(x, lane);
        let elapsed = now
            .saturating_duration_since(last_tick)
            .as_secs_f64()
            .min(0.05);
        update_last_tick(
            &mut self.time_selection_gesture,
            &mut self.time_range_move_gesture,
            now,
        );
        if edge == 0 || elapsed == 0.0 || viewport.length == 0 {
            return false;
        }
        let frames = (viewport.length as f64 * 1.5 * elapsed).round().max(1.0);
        let signed = if edge < 0 { -frames } else { frames };
        let delta = signed.clamp(i128::MIN as f64, i128::MAX as f64) as i128;
        let next = viewport.pan_by_frames(horizon, delta);
        if next == viewport {
            return false;
        }
        self.viewport = (next != Viewport::fit(horizon)).then_some(next);
        if let Some((x, y, free)) = self
            .time_range_move_gesture
            .as_ref()
            .map(|gesture| (gesture.pointer_x, gesture.pointer_y, gesture.free_modifier))
        {
            self.update_time_range_move_gesture(x, y, free);
        } else if let Some((x, y, free)) = self
            .time_selection_gesture
            .as_ref()
            .map(|gesture| (gesture.pointer_x, gesture.pointer_y, gesture.free_modifier))
        {
            self.update_time_selection_gesture(x, y, free);
        }
        true
    }

    /// A project-scope gesture keeps the projection of the surface it began in,
    /// even when the pointer leaves it or regular lanes have different widths.
    pub(super) fn time_gesture_lane_near_y(&self, y: f32) -> Option<(Option<TrackId>, LaneBounds)> {
        if let Some(gesture) = self
            .time_selection_gesture
            .filter(|g| g.tail_press.is_some())
        {
            return Some((
                gesture.selection.end_track,
                self.painted_arrangement_timeline()?,
            ));
        }
        if !self.time_selection_project_scope {
            return self
                .visible_lane_near_y(y)
                .map(|(track, lane)| (Some(track), lane));
        }
        if !y.is_finite() || self.main_track_lane_epoch != self.drag_layout_generation {
            return None;
        }
        let track = self
            .state
            .as_ref()?
            .project
            .tracks()
            .last()
            .map(|track| track.id());
        let lane = self.main_track_lane_bounds.filter(|lane| {
            lane.left.is_finite()
                && lane.width.is_finite()
                && lane.width > 0.0
                && lane.top.is_finite()
                && lane.height.is_finite()
                && lane.height > 0.0
        })?;
        Some((track, lane))
    }

    pub(super) fn visible_lane_near_y(&self, y: f32) -> Option<(TrackId, LaneBounds)> {
        if !y.is_finite() {
            return None;
        }
        let state = self.state.as_ref()?;
        self.drag_lanes
            .iter()
            .filter(|(track, lane)| {
                self.drag_lane_epochs.get(track) == Some(&self.drag_layout_generation)
                    && state.project.track(**track).is_some()
                    && lane.top.is_finite()
                    && lane.height.is_finite()
                    && lane.height > 0.0
                    && lane.left.is_finite()
                    && lane.width.is_finite()
                    && lane.width > 0.0
            })
            .min_by(|(track_a, a), (track_b, b)| {
                vertical_distance(y, **a)
                    .total_cmp(&vertical_distance(y, **b))
                    .then_with(|| lane_contains_y(y, **b).cmp(&lane_contains_y(y, **a)))
                    .then_with(|| a.top.total_cmp(&b.top))
                    .then_with(|| track_a.value().cmp(&track_b.value()))
            })
            .map(|(track, lane)| (*track, *lane))
    }

    fn time_gesture_is_current(&self) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let has_gesture =
            self.time_selection_gesture.is_some() || self.time_range_move_gesture.is_some();
        let selection_current = self.time_selection_gesture.as_ref().is_none_or(|gesture| {
            gesture.generation == self.project_generation && gesture.revision == state.revision
        });
        let range_current = self.time_range_move_gesture.as_ref().is_none_or(|gesture| {
            gesture.generation == self.project_generation && gesture.revision == state.revision
        });
        let current = has_gesture && selection_current && range_current;
        current
            && self.mode == super::EditorMode::Arrangement
            && self.context_menu.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && !self.save_dialog_open
            && !self.export_dialog_open
            && self.active_clip_drag.is_none()
            && self.active_browser_audio_drag.is_none()
            && self.active_native_device_drag.is_none()
            && self.active_trim_drag.is_none()
            && self.active_fade_drag.is_none()
            && self.active_midi_trim_drag.is_none()
    }
}

fn update_last_tick(
    selection: &mut Option<TimeSelectionGesture>,
    range_move: &mut Option<TimeRangeMoveGesture>,
    now: Instant,
) {
    if let Some(gesture) = selection.as_mut() {
        gesture.last_tick = now;
    }
    if let Some(gesture) = range_move.as_mut() {
        gesture.last_tick = now;
    }
}

fn edge_direction(x: f32, lane: LaneBounds) -> i8 {
    if x < lane.left {
        -1
    } else if x >= lane.left + lane.width {
        1
    } else {
        0
    }
}

fn vertical_distance(y: f32, lane: LaneBounds) -> f32 {
    if y < lane.top {
        lane.top - y
    } else if y >= lane.top + lane.height {
        y - (lane.top + lane.height)
    } else {
        0.0
    }
}

fn lane_contains_y(y: f32, lane: LaneBounds) -> bool {
    y >= lane.top && y < lane.top + lane.height
}
