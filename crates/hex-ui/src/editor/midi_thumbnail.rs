//! Display-only geometry for control-prepared Arrangement note samples.
//! Source projection and tempo conversion never run here.
pub(super) const ROW_NOTE_SHAPE_LIMIT: usize = 128;

pub(super) fn note_shape(
    frame_start: u64,
    frame_gate_end: u64,
    pitch: u8,
    visible_start: u64,
    visible_end: u64,
) -> Option<(f32, f32, u8)> {
    if visible_end <= visible_start || frame_gate_end <= frame_start || pitch > 127 {
        return None;
    }
    let start = frame_start.max(visible_start);
    let end = frame_gate_end.min(visible_end);
    if end <= start {
        return None;
    }
    let span = (visible_end - visible_start) as f64;
    // Preserve a positive display sliver when a huge frame range rounds x to 1.
    let x =
        (((start - visible_start) as f64 / span) as f32).min(f32::from_bits(1.0_f32.to_bits() - 1));
    let width = (((end - start) as f64 / span) as f32)
        .max(0.005)
        .min(1.0 - x);
    Some((x, width, pitch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cropped_view_uses_visible_clip_coordinates_and_excludes_hidden_notes() {
        assert_eq!(note_shape(90, 120, 60, 100, 200), Some((0.0, 0.2, 60)));
        assert_eq!(note_shape(175, 225, 62, 100, 200), Some((0.75, 0.25, 62)));
        assert_eq!(note_shape(0, 100, 60, 100, 200), None);
        assert_eq!(note_shape(200, 220, 60, 100, 200), None);
    }

    #[test]
    fn tiny_samples_at_large_endpoints_stay_positive_and_inside_the_clip() {
        let (x, width, _) = note_shape(u64::MAX - 1, u64::MAX, 127, 0, u64::MAX).unwrap();
        assert!(x.is_finite() && width.is_finite());
        assert!((0.0..1.0).contains(&x) && width > 0.0 && x + width <= 1.0);
        assert_eq!(note_shape(10, 10, 60, 0, 20), None);
        assert_eq!(note_shape(0, 10, 128, 0, 20), None);
        assert_eq!(note_shape(0, 10, 60, 10, 10), None);
    }
}
