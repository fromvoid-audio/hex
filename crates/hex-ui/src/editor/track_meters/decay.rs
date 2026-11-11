//! Shared track and Main meter decay.
use super::scale;
use std::time::Instant;

pub(super) fn decay_factor(updated: &mut Option<Instant>, now: Instant) -> f32 {
    let elapsed = updated.map_or(0.0, |previous| {
        now.saturating_duration_since(previous).as_secs_f32()
    });
    *updated = Some(now);
    10.0_f32.powf(-12.0 * elapsed / 20.0)
}

pub(super) fn decay_level(level: f32, decay: f32) -> f32 {
    let next = level * decay;
    if next < scale::FLOOR_AMPLITUDE {
        0.0
    } else {
        next
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;

    #[test]
    fn decay_keeps_the_new_finite_floor_without_changing_its_rate() {
        assert_eq!(decay_level(0.001, 0.5), 0.0005);
        assert_eq!(
            decay_level(scale::FLOOR_AMPLITUDE, 1.0),
            scale::FLOOR_AMPLITUDE
        );
        assert_eq!(decay_level(scale::FLOOR_AMPLITUDE, 0.5), 0.0);
    }
}
