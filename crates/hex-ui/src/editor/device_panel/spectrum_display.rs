//! Shared analyzer display scale; response-curve gain axes remain independent.
pub(super) const MIN_DBFS: f32 = -96.0;

pub(super) fn level(dbfs: f32) -> f32 {
    ((dbfs - MIN_DBFS) / -MIN_DBFS).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_signals_remain_visible_on_a_fixed_calibrated_range() {
        assert_eq!(level(-120.0), 0.0);
        assert_eq!(level(-96.0), 0.0);
        assert_eq!(level(-90.0), 0.0625);
        assert_eq!(level(-48.0), 0.5);
        assert_eq!(level(0.0), 1.0);
        assert_eq!(level(6.0), 1.0);
    }
}
