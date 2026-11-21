//! Opt-in native frame-pacing and CPU draw telemetry (`HEX_FRAME_TIMING=1`).
//!
//! When disabled (the default), [`enabled`] reduces to a single `OnceLock`
//! check with zero allocations, timers, or formatting overhead. When enabled,
//! wraps the root `EditorView` element to measure CPU render+layout+prepaint+paint
//! duration and wall-clock frame-to-frame present intervals on the live app,
//! logging a compact 1-second rolling summary to stderr.
use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Window,
};
use std::{
    cell::RefCell,
    sync::OnceLock,
    time::{Duration, Instant},
};

const MAX_SAMPLES: usize = 256;
const REPORT_INTERVAL: Duration = Duration::from_secs(1);

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("HEX_FRAME_TIMING")
            .ok()
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
    })
}

struct WindowStats {
    window_start: Instant,
    last_paint_end: Option<Instant>,
    cpu_ms: [f32; MAX_SAMPLES],
    interval_ms: [f32; MAX_SAMPLES],
    cpu_len: usize,
    interval_len: usize,
    frames: u64,
}

impl WindowStats {
    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            last_paint_end: None,
            cpu_ms: [0.0; MAX_SAMPLES],
            interval_ms: [0.0; MAX_SAMPLES],
            cpu_len: 0,
            interval_len: 0,
            frames: 0,
        }
    }

    fn record(&mut self, render_start: Instant, paint_end: Instant) {
        let cpu = paint_end
            .saturating_duration_since(render_start)
            .as_secs_f64()
            * 1000.0;
        if self.cpu_len < MAX_SAMPLES {
            self.cpu_ms[self.cpu_len] = cpu as f32;
            self.cpu_len += 1;
        }
        if let Some(prev) = self.last_paint_end {
            let interval = paint_end.saturating_duration_since(prev).as_secs_f64() * 1000.0;
            // Ignore long idle gaps (> 500 ms) so rolling FPS reflects active motion.
            if interval <= 500.0 && self.interval_len < MAX_SAMPLES {
                self.interval_ms[self.interval_len] = interval as f32;
                self.interval_len += 1;
            }
        }
        self.last_paint_end = Some(paint_end);
        self.frames = self.frames.saturating_add(1);

        let elapsed = paint_end.saturating_duration_since(self.window_start);
        if elapsed >= REPORT_INTERVAL && self.frames > 0 {
            let fps = if self.interval_len > 0 {
                let sum: f64 = self.interval_ms[..self.interval_len]
                    .iter()
                    .map(|&v| f64::from(v))
                    .sum();
                let avg = sum / self.interval_len as f64;
                if avg > 0.0 { 1000.0 / avg } else { 0.0 }
            } else {
                self.frames as f64 / elapsed.as_secs_f64().max(1e-6)
            };
            let (cpu_p50, cpu_p95) = percentiles(&mut self.cpu_ms[..self.cpu_len]);
            let (int_p50, int_p95) = percentiles(&mut self.interval_ms[..self.interval_len]);
            eprintln!(
                "hex frame-timing: fps={fps:.1} interval_p50={int_p50:.2}ms interval_p95={int_p95:.2}ms cpu_p50={cpu_p50:.2}ms cpu_p95={cpu_p95:.2}ms frames={}",
                self.frames
            );
            self.window_start = paint_end;
            self.cpu_len = 0;
            self.interval_len = 0;
            self.frames = 0;
        }
    }
}

fn percentiles(samples: &mut [f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let p50 = samples[(samples.len() - 1) / 2];
    let p95 = samples[((samples.len() - 1) * 95) / 100];
    (p50, p95)
}

thread_local! {
    static STATS: RefCell<Option<WindowStats>> = const { RefCell::new(None) };
}

pub(super) struct FrameTimingWrap<E> {
    child: E,
    render_start: Option<Instant>,
}

pub(super) fn wrap<E: Element>(child: E, render_start: Option<Instant>) -> FrameTimingWrap<E> {
    FrameTimingWrap {
        child,
        render_start,
    }
}

impl<E: Element> IntoElement for FrameTimingWrap<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: Element> Element for FrameTimingWrap<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.child.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.child.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.child.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        if let Some(start) = self.render_start {
            let now = Instant::now();
            STATS.with(|cell| {
                if let Ok(mut slot) = cell.try_borrow_mut() {
                    slot.get_or_insert_with(|| WindowStats::new(now))
                        .record(start, now);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::percentiles;

    #[test]
    fn percentiles_handle_empty_and_ordered_samples() {
        assert_eq!(percentiles(&mut []), (0.0, 0.0));
        let mut samples = [10.0, 2.0, 8.0, 4.0, 6.0];
        let (p50, p95) = percentiles(&mut samples);
        assert_eq!(p50, 6.0);
        assert_eq!(p95, 8.0);
    }
}
