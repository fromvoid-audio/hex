//! Owner-opted-in, bounded node wheel intent. No device/filter knowledge.
use super::*;
use gpui::{ScrollWheelEvent, Task, TouchPhase};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseGraphScrollPhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseGraphScrollEvent {
    pub phase: ResponseGraphScrollPhase,
    pub node_id: u32,
    /// Monotonic stream identity within this graph entity.
    pub gesture_id: u64,
    /// Accumulated whole steps since Begin, not an event-local delta.
    pub steps: i32,
}

#[derive(Default)]
pub(super) struct State {
    eligible: Vec<u32>,
    next_token: u64,
    gesture: Option<Gesture>,
    quiet: Option<Task<()>>,
}

#[derive(Clone, Copy)]
struct Gesture {
    token: u64,
    node_id: u32,
    remainder: f32,
    steps: i32,
    begun: bool,
}

impl State {
    pub(super) fn active(&self) -> bool {
        self.gesture.is_some()
    }
}

impl ResponseGraph {
    /// Defaults empty. Only these existing model nodes consume bare wheel input.
    pub fn set_scroll_nodes(
        &mut self,
        mut nodes: Vec<u32>,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if nodes.len() > MAX_NODES || nodes.contains(&0) {
            return Err("invalid scroll nodes");
        }
        nodes.sort_unstable();
        nodes.dedup();
        if self
            .scroll
            .gesture
            .is_some_and(|gesture| !nodes.contains(&gesture.node_id))
        {
            self.finish_scroll(true, cx);
        }
        self.scroll.eligible = nodes;
        Ok(())
    }

    pub(super) fn retire_missing_scroll_node(&mut self, cx: &mut Context<Self>) {
        if self.scroll.gesture.is_some_and(|gesture| {
            !self
                .model
                .nodes
                .iter()
                .any(|node| node.id == gesture.node_id)
        }) {
            self.finish_scroll(true, cx);
        }
    }

    pub(super) fn finish_scroll(&mut self, cancel: bool, cx: &mut Context<Self>) {
        self.scroll.quiet = None;
        if let Some(gesture) = self.scroll.gesture.take()
            && gesture.begun
        {
            cx.emit(ResponseGraphScrollEvent {
                phase: if cancel {
                    ResponseGraphScrollPhase::Cancel
                } else {
                    ResponseGraphScrollPhase::Commit
                },
                node_id: gesture.node_id,
                gesture_id: gesture.token,
                steps: gesture.steps,
            });
            cx.notify();
        }
    }

    pub(super) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event.touch_phase, TouchPhase::Ended) && self.scroll.active() {
            self.finish_scroll(false, cx);
            cx.stop_propagation();
            return;
        }
        if !self.editable
            || self.drag.is_some()
            || self.press.is_some()
            || event.modifiers.alt
            || event.modifiers.platform
            || event.modifiers.control
            || event.modifiers.shift
        {
            return;
        }
        let Some(plot) = self
            .plot_bounds()
            .filter(|bounds| bounds.contains(&event.position))
        else {
            return;
        };
        let Some(position) = self.normalized_position(event.position) else {
            return;
        };
        let width = f32::from(plot.size.width);
        let height = f32::from(plot.size.height);
        let node = self
            .model
            .nodes
            .iter()
            .filter(|node| self.scroll.eligible.contains(&node.id))
            .filter_map(|node| {
                let dx = (node.position[0] - position[0]) * width;
                let dy = (node.position[1] - position[1]) * height;
                let distance = dx.mul_add(dx, dy * dy);
                (distance <= NODE_RADIUS * NODE_RADIUS).then_some((distance, node.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        let Some(node_id) = node else { return };
        // This is a parameter wheel, not content scrolling. Undo the platform's
        // preference inversion exactly once so physical up means increasing.
        let units = f32::from(event.physical_delta().pixel_delta(px(24.0)).y) / 24.0;
        if !units.is_finite() {
            return;
        }
        if self
            .scroll
            .gesture
            .is_some_and(|gesture| gesture.node_id != node_id)
        {
            self.finish_scroll(false, cx);
        }
        let mut gesture = if let Some(gesture) = self.scroll.gesture {
            gesture
        } else {
            let Some(token) = self.scroll.next_token.checked_add(1) else {
                return;
            };
            self.scroll.next_token = token;
            Gesture {
                token,
                node_id,
                remainder: 0.0,
                steps: 0,
                begun: false,
            }
        };
        let (remainder, steps) = accumulate(gesture.remainder, units);
        gesture.remainder = remainder;
        if steps != 0 {
            if !gesture.begun {
                gesture.begun = true;
                self.focus.focus(window);
                cx.emit(ResponseGraphScrollEvent {
                    phase: ResponseGraphScrollPhase::Begin,
                    node_id,
                    gesture_id: gesture.token,
                    steps: 0,
                });
            }
            gesture.steps = gesture
                .steps
                .saturating_add(steps)
                .clamp(-1_000_000, 1_000_000);
            cx.emit(ResponseGraphScrollEvent {
                phase: ResponseGraphScrollPhase::Change,
                node_id,
                gesture_id: gesture.token,
                steps: gesture.steps,
            });
            cx.notify();
        }
        let token = gesture.token;
        self.scroll.gesture = Some(gesture);
        cx.stop_propagation();
        // One retained UI timer; replacing it cancels the prior quiet deadline.
        // Ended gives trackpads an immediate terminal. Mouse wheels use quiet.
        self.scroll.quiet = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |graph, cx| {
                if graph
                    .scroll
                    .gesture
                    .is_some_and(|gesture| gesture.token == token)
                {
                    graph.finish_scroll(false, cx);
                }
            });
        }));
    }
}

fn accumulate(remainder: f32, units: f32) -> (f32, i32) {
    let total = remainder + units.clamp(-16.0, 16.0);
    let steps = total.trunc() as i32;
    (total - steps as f32, steps)
}

impl EventEmitter<ResponseGraphScrollEvent> for ResponseGraph {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_scroll_accumulates_without_event_local_undo_steps() {
        let mut remainder = 0.0;
        let mut total = 0;
        for _ in 0..6 {
            let (r, step) = accumulate(remainder, 1.0 / 6.0);
            remainder = r;
            total += step;
        }
        assert_eq!(total, 1);
        let (r, step) = accumulate(0.25, -0.25);
        assert_eq!((r, step), (0.0, 0));
        assert_eq!(accumulate(0.0, 10000.0), (0.0, 16));
        assert_eq!(accumulate(0.0, -10000.0), (0.0, -16));
    }
}
