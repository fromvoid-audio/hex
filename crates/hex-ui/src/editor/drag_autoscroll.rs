//! Bounded edge scrolling for captured clip, import and trim gestures.
use super::{EditorMode, EditorView, drag::LaneBounds, viewport::Viewport};
use gpui::{Context, point, px};
use std::{cell::Cell, rc::Rc, time::Instant};

pub(super) type DragScroll = Rc<Cell<ScrollState>>;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ScrollState {
    pub(super) viewport: Option<Viewport>,
    last_tick: Option<Instant>,
    last_track_tick: Option<Instant>,
}

/// Only the in-lane 24px edge bands scroll. A delayed tick advances at most
/// 50ms, and scrolling never starts while the pointer is outside the lane.
fn next_viewport(
    state: &DragScroll,
    viewport: Viewport,
    horizon: u64,
    lane: LaneBounds,
    x: f32,
    now: Instant,
) -> Option<Viewport> {
    let elapsed = next_elapsed(state, now)?;
    if !x.is_finite()
        || !lane.left.is_finite()
        || !lane.width.is_finite()
        || lane.width <= 0.0
        || x < lane.left
        || x > lane.left + lane.width
        || viewport.length == 0
        || elapsed == 0.0
    {
        return None;
    }
    let edge = 24.0_f32.min(lane.width / 4.0);
    let direction = if x <= lane.left + edge {
        -1
    } else if x >= lane.left + lane.width - edge {
        1
    } else {
        0
    };
    if direction == 0 {
        return None;
    }
    let distance = (viewport.length as f64 * 1.5 * elapsed).round().max(1.0) as i128;
    let next = viewport.pan_by_frames(horizon, distance * direction);
    (next != viewport).then_some(next)
}

fn next_elapsed(state: &DragScroll, now: Instant) -> Option<f64> {
    let mut scroll = state.get();
    let previous = scroll.last_tick.replace(now);
    state.set(scroll);
    let elapsed = now
        .saturating_duration_since(previous?)
        .as_secs_f64()
        .min(0.05);
    (elapsed > 0.0).then_some(elapsed)
}

fn next_track_tick(state: &DragScroll, now: Instant) -> bool {
    let mut scroll = state.get();
    let Some(previous) = scroll.last_track_tick else {
        scroll.last_track_tick = Some(now);
        state.set(scroll);
        return false;
    };
    if now.saturating_duration_since(previous) < std::time::Duration::from_millis(50) {
        return false;
    }
    scroll.last_track_tick = Some(now);
    state.set(scroll);
    true
}

/// Return a one-row UniformList offset when the pointer is in its top/bottom
/// edge band. UniformList offsets are negative while scrolled down.
fn next_track_offset(
    state: &DragScroll,
    bounds: LaneBounds,
    pointer: (f32, f32),
    row_height: f32,
    offset_y: f32,
    max_scroll_distance_y: f32,
    now: Instant,
) -> Option<f32> {
    let (pointer_x, pointer_y) = pointer;
    let right = bounds.left + bounds.width;
    let bottom = bounds.top + bounds.height;
    if !pointer_x.is_finite()
        || !pointer_y.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || !bounds.left.is_finite()
        || !bounds.top.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || !row_height.is_finite()
        || !offset_y.is_finite()
        || !max_scroll_distance_y.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
        || row_height <= 0.0
        || max_scroll_distance_y < 0.0
        || pointer_x < bounds.left
        || pointer_x >= right
        || pointer_y < bounds.top
        || pointer_y > bottom
    {
        return None;
    }
    let edge = 24.0_f32.min(bounds.height / 4.0);
    let direction = if pointer_y <= bounds.top + edge {
        1.0
    } else if pointer_y >= bottom - edge {
        -1.0
    } else {
        return None;
    };
    if !next_track_tick(state, now) {
        return None;
    }
    let next = (offset_y + row_height * direction).clamp(-max_scroll_distance_y, 0.0);
    (next != offset_y).then_some(next)
}

impl EditorView {
    pub(super) fn tick_clip_gesture_autoscroll(
        &mut self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mode != EditorMode::Arrangement
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.edit_pending()
        {
            return false;
        }
        let Some(horizon) = self.state.as_ref().map(|state| state.timeline_frames) else {
            return false;
        };
        let horizon = self.viewport_horizon(horizon);
        let viewport = self.visible_viewport(horizon);
        if let Some(drag) = self.active_clip_drag.clone() {
            if !self.drag_start_is_current(&drag) {
                return false;
            }
            let Some((x, y)) = drag.last_pointer.get() else {
                return false;
            };
            let horizontal = self
                .hovered_drag_target(x, y)
                .and_then(|(_, lane)| next_viewport(&drag.scroll, viewport, horizon, lane, x, now));
            let vertical = self.scroll_tracks_for_drag(&drag.scroll, x, y, now);
            if let Some(next) = horizontal {
                self.apply_drag_scroll(&drag.scroll, next, horizon);
            }
            if vertical {
                drag.group.borrow_mut().invalidate();
                drag.preview.set(None);
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                drag.lane_hovered.set(false);
                return true;
            }
            if horizontal.is_some() {
                let (preview, hovered) = self.clip_drag_drop_position(&drag, x, y);
                drag.preview.set(preview);
                drag.lane_hovered.set(hovered);
                return true;
            }
            if self.arrangement_bounds_epoch == self.drag_layout_generation {
                let (preview, hovered) = self.clip_drag_drop_position(&drag, x, y);
                let preview_changed = drag.preview.replace(preview) != preview;
                let hovered_changed = drag.lane_hovered.replace(hovered) != hovered;
                let changed = preview_changed || hovered_changed;
                return changed;
            }
            return false;
        }
        if let Some(drag) = self.active_browser_audio_drag.clone() {
            if !self.browser_audio_drag_is_current(&drag) {
                return false;
            }
            let Some((x, y)) = drag.last_pointer.get() else {
                return false;
            };
            let horizontal = self
                .hovered_drag_target(x, y)
                .and_then(|(_, lane)| next_viewport(&drag.scroll, viewport, horizon, lane, x, now));
            let vertical = self.scroll_tracks_for_drag(&drag.scroll, x, y, now);
            if let Some(next) = horizontal {
                self.apply_drag_scroll(&drag.scroll, next, horizon);
            }
            if vertical {
                drag.preview.set(None);
                drag.destination.set(None);
                drag.insertion_line_y.set(None);
                drag.lane_hovered.set(false);
                return true;
            }
            if horizontal.is_some() {
                self.refresh_browser_audio_drag(&drag);
                return true;
            }
            if self.arrangement_bounds_epoch == self.drag_layout_generation {
                let before = drag.destination.get();
                self.refresh_browser_audio_drag(&drag);
                return before != drag.destination.get();
            }
            return false;
        }
        if let Some(drag) = self.active_trim_drag.clone() {
            if !self.trim_drag_current(&drag) {
                self.cancel_trim_gesture(cx);
                return true;
            }
            let Some(x) = drag.last_pointer_x.get() else {
                return false;
            };
            let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
                return false;
            };
            let Some(next) = next_viewport(&drag.scroll, viewport, u64::MAX, lane, x, now) else {
                return false;
            };
            // Source/minimum bounds, not grid quantization, may stop navigation.
            let Some(next) = self.trim_pan_viewport(&drag, x, next) else {
                return false;
            };
            self.apply_drag_scroll(&drag.scroll, next, horizon);
            self.update_trim_preview(&drag, x, cx);
            return true;
        }
        if let Some(drag) = self.active_midi_trim_drag.clone() {
            if !self.midi_trim_drag_current(&drag) {
                self.clear_midi_trim_drag(drag.project_generation, drag.clip.id(), cx);
                return true;
            }
            let Some(x) = drag.last_pointer_x.get() else {
                return false;
            };
            let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
                return false;
            };
            let Some(next) = next_viewport(&drag.scroll, viewport, u64::MAX, lane, x, now) else {
                return false;
            };
            let Some(next) = self.midi_trim_pan_viewport(&drag, x, next) else {
                return false;
            };
            self.apply_drag_scroll(&drag.scroll, next, horizon);
            self.update_midi_trim_preview(&drag, x, cx);
            return true;
        }
        false
    }

    fn apply_drag_scroll(&mut self, scroll: &DragScroll, next: Viewport, horizon: u64) {
        let mut state = scroll.get();
        state.viewport = Some(next);
        scroll.set(state);
        self.viewport = (next != Viewport::fit(horizon)).then_some(next);
    }

    fn scroll_tracks_for_drag(
        &mut self,
        scroll: &DragScroll,
        x: f32,
        y: f32,
        now: Instant,
    ) -> bool {
        let Some(bounds) = self
            .arrangement_bounds
            .filter(|_| self.arrangement_bounds_epoch == self.drag_layout_generation)
        else {
            return false;
        };
        let handle = self.track_scroll.clone();
        let offset = handle.offset();
        let max_offset = handle.max_offset();
        let Some(next_y) = next_track_offset(
            scroll,
            bounds,
            (x, y),
            super::tracks::TRACK_ROW_HEIGHT,
            f32::from(offset.y),
            f32::from(max_offset.height),
            now,
        ) else {
            return false;
        };
        handle.set_offset(point(offset.x, px(next_y)));
        // Invalidate cached lane and insertion geometry immediately. GPUI will
        // repaint the list; until then, all drop-target epoch checks refuse it.
        self.drag_layout_generation = self.drag_layout_generation.wrapping_add(1);
        true
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn held_edges_scroll_with_bounded_time_and_stop_at_horizon() {
        let state = DragScroll::default();
        let lane = LaneBounds {
            left: 100.0,
            width: 400.0,
            top: 0.0,
            height: 40.0,
        };
        let view = Viewport {
            start: 100,
            length: 1000,
        };
        let now = Instant::now();
        assert_eq!(next_viewport(&state, view, 10_000, lane, 490.0, now), None);
        let next = next_viewport(
            &state,
            view,
            10_000,
            lane,
            490.0,
            now + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            next,
            Viewport {
                start: 175,
                length: 1000
            }
        );
        assert_eq!(
            next_viewport(
                &state,
                next,
                10_000,
                lane,
                300.0,
                now + Duration::from_secs(6)
            ),
            None
        );
        assert_eq!(
            next_viewport(
                &state,
                next,
                10_000,
                lane,
                f32::NAN,
                now + Duration::from_secs(7)
            ),
            None
        );
        assert_eq!(
            next_viewport(
                &state,
                next,
                10_000,
                lane,
                99.0,
                now + Duration::from_secs(8)
            ),
            None
        );
        assert_eq!(
            next_viewport(
                &state,
                Viewport {
                    start: 0,
                    length: 1000
                },
                10_000,
                lane,
                101.0,
                now + Duration::from_secs(9)
            ),
            None
        );
        assert_eq!(
            next_viewport(
                &state,
                Viewport {
                    start: 9000,
                    length: 1000
                },
                10_000,
                lane,
                499.0,
                now + Duration::from_secs(10)
            ),
            None
        );
    }

    #[test]
    fn vertical_edges_scroll_one_row_and_clamp_to_uniform_list_bounds() {
        let state = DragScroll::default();
        let body = LaneBounds {
            left: 100.0,
            width: 500.0,
            top: 50.0,
            height: 300.0,
        };
        let now = Instant::now();
        let scroll = |state: &DragScroll, y, offset, max, time| {
            next_track_offset(state, body, (300.0, y), 40.0, offset, max, time)
        };
        assert_eq!(scroll(&state, 51.0, 0.0, 400.0, now), None);
        assert_eq!(
            scroll(
                &state,
                349.0,
                0.0,
                400.0,
                now + std::time::Duration::from_millis(50)
            ),
            Some(-40.0),
            "bottom movement advances one row"
        );
        assert_eq!(
            scroll(
                &state,
                349.0,
                -400.0,
                400.0,
                now + std::time::Duration::from_millis(100)
            ),
            None,
            "bottom boundary does not advance or notify"
        );
        assert_eq!(
            scroll(
                &state,
                349.0,
                -400.0,
                400.0,
                now + std::time::Duration::from_millis(150)
            ),
            None
        );
        assert_eq!(
            scroll(
                &state,
                51.0,
                -20.0,
                400.0,
                now + std::time::Duration::from_millis(200)
            ),
            Some(0.0),
            "top movement clamps to the first row"
        );
        assert_eq!(
            scroll(
                &state,
                200.0,
                -40.0,
                400.0,
                now + std::time::Duration::from_millis(250)
            ),
            None,
            "middle of the list does not scroll"
        );
        assert_eq!(
            scroll(
                &state,
                349.0,
                -40.0,
                400.0,
                now + std::time::Duration::from_millis(300)
            ),
            Some(-80.0),
            "one delayed tick scrolls no more than one row"
        );
        assert_eq!(
            scroll(
                &state,
                349.0,
                -40.0,
                400.0,
                now + std::time::Duration::from_secs(2)
            ),
            Some(-80.0),
            "a delayed tick is clamped to one row"
        );
    }

    #[test]
    fn vertical_scroll_accumulates_sub_fifty_millisecond_ticks() {
        let state = DragScroll::default();
        let body = LaneBounds {
            left: 100.0,
            width: 500.0,
            top: 50.0,
            height: 300.0,
        };
        let now = Instant::now();
        let tick = |time| next_track_offset(&state, body, (300.0, 349.0), 40.0, 0.0, 400.0, time);
        assert_eq!(tick(now), None);
        assert_eq!(tick(now + std::time::Duration::from_millis(16)), None);
        assert_eq!(tick(now + std::time::Duration::from_millis(32)), None);
        assert_eq!(tick(now + std::time::Duration::from_millis(48)), None);
        assert_eq!(
            tick(now + std::time::Duration::from_millis(64)),
            Some(-40.0),
            "frequent controller ticks accumulate until one row is eligible"
        );
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "resize_pan_tests.rs"]
mod resize_pan_tests;
