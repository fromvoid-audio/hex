//! Shared display-only block caret for native text and numeric editing.
//! The focused painter owns animation frames; selection, IME and source text stay
//! with their input owners. Keyed state expires when that caret stops painting.
use gpui::{App, EntityId, PaintQuad, Window};
use std::time::{Duration, Instant};

fn opacity(elapsed: Duration) -> f32 {
    let phase = (elapsed.as_secs_f64() % 1.2) / 1.2;
    (0.30 + 0.35 * (0.5 + 0.5 * (phase * std::f64::consts::TAU).cos())) as f32
}

pub(crate) fn paint(mut block: PaintQuad, owner: EntityId, window: &mut Window, cx: &mut App) {
    let epoch = window.use_keyed_state(("text-block-caret", owner.as_u64()), cx, |_, _| {
        Instant::now()
    });
    block.background = block.background.opacity(opacity(epoch.read(cx).elapsed()));
    window.paint_quad(block);
    window.request_animation_frame();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pulse_is_smooth_bounded_and_periodic() {
        assert!((opacity(Duration::ZERO) - 0.65).abs() < 0.0001);
        assert!((opacity(Duration::from_millis(600)) - 0.30).abs() < 0.0001);
        assert!((opacity(Duration::from_millis(1200)) - opacity(Duration::ZERO)).abs() < 0.0001);
        assert!(opacity(Duration::from_millis(150)) > opacity(Duration::from_millis(300)));
    }
}
