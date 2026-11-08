//! Bounded cut-slope wheel steps shared by the editor and catalog.

pub(super) fn advance(value: u32, delta: i32) -> u32 {
    (i64::from(value) + i64::from(delta)).clamp(0, 3) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slope_steps_saturate_without_overflow() {
        assert_eq!(advance(0, -1), 0);
        assert_eq!(advance(1, 1), 2);
        assert_eq!(advance(3, 1), 3);
        assert_eq!(advance(2, i32::MIN), 0);
        assert_eq!(advance(2, i32::MAX), 3);
    }
}
