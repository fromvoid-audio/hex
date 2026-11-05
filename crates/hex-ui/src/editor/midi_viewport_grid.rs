//! Finite signed source-grid geometry and a translation period for hidden origins.
//! A blank prefix must preserve fine snapping, four-quarter source-grid accents,
//! sampled piano lines and ruler ticks at the captured viewport. This source grid
//! is independent of the project's meter/transport clock; it authors no saved time.
use hex_project::BeatTime;

pub(super) const ACCENT_PERIOD: u64 = 4 * BeatTime::ONE.raw();

pub(super) struct Ticks {
    pub(super) first: i128,
    pub(super) last: i128,
    pub(super) stride: u64,
}
/// At most `limit` sampled source-grid lines, including signed blank space.
pub(super) fn ticks(start: i128, span: u64, step: u64, limit: u64) -> Option<Ticks> {
    if span == 0
        || step == 0
        || limit == 0
        || start < -i128::from(u64::MAX)
        || start > i128::from(u64::MAX - span)
    {
        return None;
    }
    let wide_step = i128::from(step);
    let first = -(-start).div_euclid(wide_step);
    let last = (start + i128::from(span)).div_euclid(wide_step);
    let count = (last - first + 1).max(0) as u128;
    let stride = u64::try_from(count.div_ceil(u128::from(limit)).max(1)).ok()?;
    let first = -(-first).div_euclid(i128::from(stride)) * i128::from(stride);
    Some(Ticks {
        first,
        last,
        stride,
    })
}
/// The source ruler uses at most eight intervals and nine endpoint ticks.
pub(super) fn ruler_step(span: u64, step: u64) -> u64 {
    let mut step = step.max(1);
    while span / step > 8 {
        step = step.saturating_mul(2);
    }
    step
}
fn lcm(a: u64, b: u64) -> Option<u64> {
    if a == 0 || b == 0 {
        return None;
    }
    let (mut x, mut y) = (a, b);
    while y != 0 {
        let remainder = x % y;
        x = y;
        y = remainder;
    }
    (a / x).checked_mul(b)
}
/// Resolve a negative source position against actual storage capacity. The
/// positive prefix is a common grid period, rather than a fractional origin
/// shift that changes snapping/paint phase. Exhausted storage clamps navigation
/// authoring at its finite boundary, as the positive authoring boundary does.
pub(super) fn source_prefix(
    signed_start: i128,
    capacity: u64,
    view_start: i128,
    span: u64,
    step: u64,
) -> Option<(i128, u64)> {
    let grid = ticks(view_start, span, step, 256)?;
    let quantum = lcm(
        lcm(ACCENT_PERIOD, step.checked_mul(grid.stride)?)?,
        ruler_step(span, step),
    )?;
    let maximum = capacity / quantum * quantum;
    let start = signed_start.max(-i128::from(maximum));
    if start >= 0 {
        return Some((start, 0));
    }
    let needed = u64::try_from(-start).ok()?;
    let prefix = needed.div_ceil(quantum).checked_mul(quantum)?;
    Some((start, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_translation_preserves_every_displayed_grid_period_and_subgrid_offset() {
        for start in [
            -i128::from(BeatTime::ONE.raw()) - 17,
            0,
            i128::from(BeatTime::ONE.raw()) + 31,
        ] {
            for span in [
                BeatTime::ONE.raw() / 8,
                4 * BeatTime::ONE.raw(),
                400 * BeatTime::ONE.raw(),
                1 << 50,
            ] {
                for step in [
                    BeatTime::ONE.raw() / 8,
                    BeatTime::ONE.raw() / 4,
                    BeatTime::ONE.raw(),
                ] {
                    let negative = -i128::from(BeatTime::ONE.raw() / 8) - 1;
                    let (position, prefix) =
                        source_prefix(negative, u64::MAX / 2, start, span, step).unwrap();
                    assert_eq!(position, negative);
                    assert!(i128::from(prefix) + negative > 0);
                    assert_eq!(prefix % ACCENT_PERIOD, 0);
                    assert_eq!(prefix % ruler_step(span, step), 0);
                    let old = ticks(start, span, step, 256).unwrap();
                    let next = ticks(start + i128::from(prefix), span, step, 256).unwrap();
                    assert_eq!(old.stride, next.stride);
                    assert_eq!(
                        old.first * i128::from(step) - start,
                        next.first * i128::from(step) - (start + i128::from(prefix))
                    );
                }
            }
        }
    }
    #[test]
    fn finite_storage_and_malformed_viewports_cannot_overflow_origin_resolution() {
        assert!(ticks(i128::MIN, 1, 1, 256).is_none());
        assert!(ticks(i128::MAX, 1, 1, 256).is_none());
        assert!(ticks(0, 0, 1, 256).is_none());
        assert!(ticks(0, 1, 0, 256).is_none());
        assert!(ticks(0, 1, 1, 0).is_none());
        assert_eq!(
            source_prefix(
                -1,
                ACCENT_PERIOD - 1,
                0,
                BeatTime::ONE.raw(),
                BeatTime::ONE.raw()
            ),
            Some((0, 0))
        );
        let (position, prefix) = source_prefix(
            i128::MIN,
            2 * ACCENT_PERIOD,
            0,
            BeatTime::ONE.raw(),
            BeatTime::ONE.raw(),
        )
        .unwrap();
        assert_eq!(
            (position, prefix),
            (-i128::from(2 * ACCENT_PERIOD), 2 * ACCENT_PERIOD)
        );
    }
}
