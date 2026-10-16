//! Bounded edge scrolling for captured clip, import and trim gestures.
use super::{EditorMode, EditorView, drag::LaneBounds, viewport::Viewport};
use gpui::Context;
use std::{cell::Cell, rc::Rc, time::Instant};

pub(super) type DragScroll = Rc<Cell<ScrollState>>;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ScrollState {
    pub(super) viewport: Option<Viewport>,
    last_tick: Option<Instant>,
}

/// Only the in-lane 24px edge bands scroll: entering the browser or toolbar
/// cannot accidentally move the timeline. A delayed tick advances at most 50ms.
fn next_viewport(
    state: &DragScroll,
    viewport: Viewport,
    horizon: u64,
    lane: LaneBounds,
    x: f32,
    now: Instant,
) -> Option<Viewport> {
    let mut scroll = state.get();
    let previous = scroll.last_tick.replace(now);
    state.set(scroll);
    let elapsed = now
        .saturating_duration_since(previous?)
        .as_secs_f64()
        .min(0.05);
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
        let viewport = self.visible_viewport(horizon);
        if let Some(drag) = self.active_clip_drag.clone() {
            if !self.drag_start_is_current(&drag) {
                return false;
            }
            let Some((x, y)) = drag.last_pointer.get() else {
                return false;
            };
            let Some((_, lane)) = self.hovered_drag_target(x, y) else {
                return false;
            };
            let Some(next) = next_viewport(&drag.scroll, viewport, horizon, lane, x, now) else {
                return false;
            };
            self.apply_drag_scroll(&drag.scroll, next, horizon);
            let (preview, hovered) = self.clip_drag_drop_position(&drag, x, y);
            drag.preview.set(preview);
            drag.lane_hovered.set(hovered);
            return true;
        }
        if let Some(drag) = self.active_browser_audio_drag.clone() {
            if !self.browser_audio_drag_is_current(&drag) {
                return false;
            }
            let Some((x, y)) = drag.last_pointer.get() else {
                return false;
            };
            let Some((_, lane)) = self.hovered_drag_target(x, y) else {
                return false;
            };
            let Some(next) = next_viewport(&drag.scroll, viewport, horizon, lane, x, now) else {
                return false;
            };
            self.apply_drag_scroll(&drag.scroll, next, horizon);
            self.refresh_browser_audio_drag(&drag);
            return true;
        }
        if let Some(drag) = self.active_trim_drag.clone() {
            if !self.trim_drag_current(&drag) {
                return false;
            }
            let Some(x) = drag.last_pointer_x.get() else {
                return false;
            };
            let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
                return false;
            };
            let Some(next) = next_viewport(&drag.scroll, viewport, horizon, lane, x, now) else {
                return false;
            };
            self.apply_drag_scroll(&drag.scroll, next, horizon);
            self.update_trim_preview(&drag, x, cx);
            return true;
        }
        if let Some(drag) = self.active_midi_trim_drag.clone() {
            if !self.midi_trim_drag_current(&drag) {
                return false;
            }
            let Some(x) = drag.last_pointer_x.get() else {
                return false;
            };
            let Some(lane) = self.drag_lanes.get(&drag.source_track).copied() else {
                return false;
            };
            let Some(next) = next_viewport(&drag.scroll, viewport, horizon, lane, x, now) else {
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
}

#[cfg(test)]
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
}
