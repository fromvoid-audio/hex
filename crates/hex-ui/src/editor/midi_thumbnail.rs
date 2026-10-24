//! Display-only geometry for control-prepared Arrangement note samples.
//! Source projection and tempo conversion never run here.
pub(super) const ROW_NOTE_SHAPE_LIMIT: usize = 128;

// Bounds pair checks stay bounded. This is a submission batch size, never a
// note budget: full or intersecting batches flush without discarding a note.
const PAINT_BATCH_SIZE: usize = 32;

fn paint_batches(
    rectangles: impl IntoIterator<Item = gpui::Bounds<gpui::Pixels>>,
    mut submit: impl FnMut(&[gpui::Bounds<gpui::Pixels>], gpui::Bounds<gpui::Pixels>),
) {
    let mut batch = [gpui::Bounds::default(); PAINT_BATCH_SIZE];
    let mut length = 0;
    let mut union = gpui::Bounds::default();
    for rectangle in rectangles {
        if length > 0
            && (length == PAINT_BATCH_SIZE
                || batch[..length]
                    .iter()
                    .any(|previous| previous.intersects(&rectangle)))
        {
            submit(&batch[..length], union);
            length = 0;
        }
        union = if length == 0 {
            rectangle
        } else {
            union.union(&rectangle)
        };
        batch[length] = rectangle;
        length += 1;
    }
    if length > 0 {
        submit(&batch[..length], union);
    }
}

/// Submit the same snapped quads in contiguous disjoint groups. A layer's union
/// contains every actual rectangle, including edge overhang before clipping.
/// Only these same-kind quads share an order; clip chrome, labels, fades and
/// overlays retain their separate ordering and interactive elements.
pub(super) fn paint_note_shapes(
    bounds: gpui::Bounds<gpui::Pixels>,
    shapes: &[(f32, f32, u8)],
    window: &mut gpui::Window,
) {
    let scale = window.scale_factor();
    paint_batches(
        shapes
            .iter()
            .filter_map(|&(x, width, pitch)| paint_bounds(bounds, x, width, pitch, scale)),
        |rectangles, union| {
            let paint = |window: &mut gpui::Window| {
                for rectangle in rectangles {
                    window.paint_quad(gpui::fill(*rectangle, gpui::rgb(0x000000)));
                }
            };
            let finite_layer = [
                union.left(),
                union.top(),
                union.right(),
                union.bottom(),
                union.size.width,
                union.size.height,
            ]
            .into_iter()
            .all(|value| (f32::from(value) * scale).is_finite());
            if rectangles.len() == 1 || !finite_layer {
                paint(window);
            } else {
                window.paint_layer(union, paint);
            }
        },
    );
}

/// Snap display samples to physical pixels just as the former layout leaves
/// did. The enclosing body still clips notes below the clip name and at edges.
pub(super) fn paint_bounds(
    bounds: gpui::Bounds<gpui::Pixels>,
    x: f32,
    width: f32,
    pitch: u8,
    scale: f32,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let left = f32::from(bounds.left());
    let top = f32::from(bounds.top());
    let body_width = f32::from(bounds.size.width);
    let body_height = f32::from(bounds.size.height);
    if ![left, top, body_width, body_height, x, width, scale]
        .iter()
        .all(|v| v.is_finite())
        || body_width <= 0.0
        || body_height <= 0.0
        || scale <= 0.0
        || !(0.0..1.0).contains(&x)
        || width <= 0.0
        || x + width > 1.0
        || pitch > 127
    {
        return None;
    }
    let snap = |value: f32| (value * scale).round() / scale;
    let start = snap(left + x * body_width);
    let end = snap(left + (x + width) * body_width);
    let y = snap(top + f32::from(127 - pitch) / 128.0 * body_height);
    if ![start, end, y].iter().all(|v| v.is_finite()) || end <= start {
        return None;
    }
    Some(gpui::Bounds::new(
        gpui::point(gpui::px(start), gpui::px(y)),
        gpui::size(gpui::px(end - start), gpui::px(2.0)),
    ))
}

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

/// Inspect completed canvas rectangles, including replay and ancestor clipping.
#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn rendered_notes(
    visual: &mut gpui::VisualTestContext,
    clip: hex_project::ClipId,
    dragging: bool,
) -> Vec<(gpui::Bounds<gpui::Pixels>, gpui::Background)> {
    let selector = Box::leak(
        format!(
            "clip-{}notes-{}",
            if dragging { "drag-" } else { "" },
            clip.value()
        )
        .into_boxed_str(),
    );
    let scale = visual.update(|window, _| window.scale_factor());
    visual
        .rendered_filled_quads(selector)
        .into_iter()
        .filter(|(_, color)| *color == gpui::Background::from(gpui::rgb(0x000000)))
        .map(|(bounds, color)| {
            (
                gpui::Bounds::new(
                    gpui::point(
                        gpui::px(f64::from(bounds.origin.x) as f32 / scale),
                        gpui::px(f64::from(bounds.origin.y) as f32 / scale),
                    ),
                    gpui::size(
                        gpui::px(f64::from(bounds.size.width) as f32 / scale),
                        gpui::px(f64::from(bounds.size.height) as f32 / scale),
                    ),
                ),
                color,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_preserve_all_rectangles_and_admit_only_bounded_disjoint_unions() {
        use gpui::{Bounds, point, px, size};
        // More than one batch, then exact overlaps and edge-touching rectangles.
        // Capacity limits comparisons, never the number submitted.
        let mut rectangles: Vec<_> = (0..97)
            .map(|index| {
                Bounds::new(
                    point(px(index as f32 * 3.0), px(0.0)),
                    size(px(2.0), px(2.0)),
                )
            })
            .collect();
        rectangles.extend([
            rectangles[96],
            rectangles[96],
            Bounds::new(point(px(290.0), px(0.0)), size(px(2.0), px(2.0))),
        ]);
        let mut submitted = Vec::new();
        let mut sizes = Vec::new();
        paint_batches(rectangles.iter().copied(), |batch, union| {
            assert!(!batch.is_empty() && batch.len() <= PAINT_BATCH_SIZE);
            for (index, rectangle) in batch.iter().enumerate() {
                assert_eq!(union.intersect(rectangle), *rectangle);
                assert!(
                    batch[..index]
                        .iter()
                        .all(|other| !other.intersects(rectangle))
                );
            }
            sizes.push(batch.len());
            submitted.extend_from_slice(batch);
        });
        assert_eq!(submitted, rectangles);
        assert_eq!(&sizes[..3], &[32, 32, 32]);
        assert!(
            sizes[3..].contains(&1),
            "overlap must flush before sharing a layer"
        );
        paint_batches([], |_, _| panic!("empty input must not create a layer"));
    }

    #[test]
    fn batches_preserve_snapping_overhang_and_rejected_geometry_at_both_scales() {
        for scale in [1.0, 2.0] {
            for width in [4.0, 127.5] {
                let bounds = gpui::Bounds::new(
                    gpui::point(gpui::px(-3.25), gpui::px(1.75)),
                    gpui::size(gpui::px(width), gpui::px(7.0)),
                );
                let shapes = [
                    (0.0, 0.2, 0),
                    (0.0, 0.2, 127),
                    (0.95, 0.05, 60),
                    (0.95, 0.05, 60),
                    (f32::NAN, 0.2, 60),
                ];
                let legacy: Vec<_> = shapes
                    .iter()
                    .filter_map(|&(x, width, pitch)| paint_bounds(bounds, x, width, pitch, scale))
                    .collect();
                let mut actual = Vec::new();
                paint_batches(legacy.iter().copied(), |batch, _| {
                    actual.extend_from_slice(batch)
                });
                assert_eq!(actual, legacy);
            }
        }
    }

    #[test]
    fn malformed_display_geometry_never_reaches_the_scene() {
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(1.0), gpui::px(2.0)),
            gpui::size(gpui::px(100.0), gpui::px(50.0)),
        );
        assert!(paint_bounds(bounds, 0.1, 0.2, 60, 2.0).is_some());
        for (x, width, pitch, scale) in [
            (f32::NAN, 0.2, 60, 2.0),
            (0.1, f32::INFINITY, 60, 2.0),
            (0.9, 0.2, 60, 2.0),
            (-0.1, 0.2, 60, 2.0),
            (0.1, 0.0, 60, 2.0),
            (0.1, 0.2, 128, 2.0),
            (0.1, 0.2, 60, 0.0),
            (0.1, 0.2, 60, f32::NAN),
        ] {
            assert!(paint_bounds(bounds, x, width, pitch, scale).is_none());
        }
    }

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

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_thumbnail_ui_tests.rs"]
mod ui_tests;
