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
    phased: bool,
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
        self.scroll.phased = false;
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

    /// Capture can see Ended outside the graph. Defer ownership retirement so
    /// ordinary inside bubble handling keeps its existing consumption priority.
    pub(super) fn observe_scroll_end(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(token) = self.scroll.gesture.map(|gesture| gesture.token) else {
            // A zero-delta Started may own phase metadata without any edit.
            self.scroll.phased = false;
            return;
        };
        cx.defer_in(window, move |graph, window, cx| {
            if graph.scroll.gesture.is_some_and(|gesture| {
                gesture.token == token
                        // Fractional intent has not taken focus or begun an
                        // edit; its phase metadata can retire without focus.
                        && (!gesture.begun || graph.focus.is_focused(window))
            }) {
                graph.finish_scroll(false, cx);
            }
        });
    }

    pub(super) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.is_momentum && matches!(event.touch_phase, TouchPhase::Ended) {
            self.scroll.phased = false;
            if self.scroll.active() {
                self.finish_scroll(false, cx);
                cx.stop_propagation();
            }
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
        let Some(plot) = self.plot_bounds() else {
            return;
        };
        let Some(position) = self.unclamped_position(event.position) else {
            return;
        };
        let width = f32::from(plot.size.width);
        let height = f32::from(plot.size.height);
        let node = self
            .model
            .nodes
            .iter()
            .filter_map(|node| {
                let dx = (node.position[0] - position[0]) * width;
                let dy = (node.position[1] - position[1]) * height;
                let distance = dx.mul_add(dx, dy * dy);
                (distance <= NODE_RADIUS * NODE_RADIUS).then_some((distance, node.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        let Some(node_id) = node else { return };
        if !self.scroll.eligible.contains(&node_id) {
            // Match pointer hit ownership before opting into parameter input.
            return;
        }
        // This is a parameter wheel, not content scrolling. Undo the platform's
        // preference inversion exactly once so physical up means increasing.
        let units = f32::from(event.physical_delta().pixel_delta(px(24.0)).y) / 24.0;
        if !units.is_finite() {
            return;
        }
        if !event.is_momentum && matches!(event.touch_phase, TouchPhase::Started) {
            // A zero-delta physical start still identifies later Moved input.
            // Retain only metadata here, without taking horizontal scrolling.
            self.scroll.phased = true;
            self.scroll.quiet = None;
        }
        if units == 0.0 {
            // A horizontal-only wheel belongs to the outer content scroller.
            // Keep nonzero fractional vertical intent for deliberate coalescing.
            return;
        }
        if event.is_momentum {
            // Only physical input owns parameter steps/history. Consume vertical
            // inertia over an opted-in node without allocating or ending a
            // gesture; horizontal-only input has already bubbled above.
            cx.stop_propagation();
            return;
        }
        let phased = self.scroll.phased;
        if self
            .scroll
            .gesture
            .is_some_and(|gesture| gesture.node_id != node_id)
        {
            self.finish_scroll(false, cx);
        }
        self.scroll.phased = phased;
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
        // Physical streams end explicitly, even after a held-finger pause.
        // Only streams without Started need the mouse-wheel quiet fallback.
        if self.scroll.phased {
            return;
        }
        // One retained UI timer; replacing it cancels the prior quiet deadline.
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
    fn momentum_metadata_is_neutral_by_default_and_does_not_change_scroll_direction() {
        let ordinary = ScrollWheelEvent {
            delta: gpui::ScrollDelta::Pixels(point(px(7.0), px(-12.0))),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        };
        assert!(!ordinary.is_momentum);
        let inertial = ScrollWheelEvent {
            is_momentum: true,
            ..ordinary.clone()
        };
        assert_eq!(
            inertial.delta.pixel_delta(px(24.0)),
            ordinary.delta.pixel_delta(px(24.0))
        );
        assert_eq!(
            inertial.physical_delta().pixel_delta(px(24.0)),
            ordinary.physical_delta().pixel_delta(px(24.0))
        );
        assert!(matches!(inertial.touch_phase, TouchPhase::Moved));
        let inverted = ScrollWheelEvent {
            direction_inverted_from_device: true,
            ..inertial
        };
        assert_eq!(
            inverted.delta.pixel_delta(px(24.0)),
            ordinary.delta.pixel_delta(px(24.0)),
            "ordinary content delta stays raw"
        );
        assert_eq!(
            inverted.physical_delta().pixel_delta(px(24.0)),
            point(px(-7.0), px(12.0))
        );
    }

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

#[cfg(test)]
#[path = "scroll_terminal_tests.rs"]
mod terminal_tests;
