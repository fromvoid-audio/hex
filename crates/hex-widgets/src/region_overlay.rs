//! Shared normalized region/range overlay geometry, edge hit-testing, bounded
//! drag draft state, and GPUI visual primitives for sample, zone, loop, and
//! analysis editors.
//!
//! Coordinates are normalized in `0.0..=1.0` so callers own canonical frame,
//! note, or parameter units while sharing deterministic hit-testing, deadzone,
//! clamping, cancellation, and marker/handle layout rules.
use gpui::{Div, div, prelude::*, px, relative, rgb};

/// Default half-width hit tolerance (in logical pixels) for dragging a region edge.
pub const REGION_EDGE_HIT_TOLERANCE_PX: f32 = 5.0;
/// Default drag deadzone (in logical pixels) before a pointer press becomes a move.
pub const REGION_DRAG_DEADZONE_PX: f32 = 2.0;

/// A validated normalized sub-interval `[start, end]` inside `0.0..=1.0` with `start < end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionSpan {
    start: f64,
    end: f64,
}

impl RegionSpan {
    /// Construct a validated span where `0.0 <= start < end <= 1.0` and both are finite.
    pub fn new(start: f64, end: f64) -> Option<Self> {
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end > 1.0 || start >= end {
            return None;
        }
        Some(Self { start, end })
    }

    /// Construct a validated normalized span from frame offsets inside `total_frames`.
    pub fn from_frames(start: f64, end: f64, total_frames: u64) -> Option<Self> {
        if total_frames == 0
            || !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || start >= end
            || end > total_frames as f64
        {
            return None;
        }
        Self::new(start / total_frames as f64, end / total_frames as f64)
    }

    pub const fn start(self) -> f64 {
        self.start
    }

    pub const fn end(self) -> f64 {
        self.end
    }

    pub fn width(self) -> f64 {
        self.end - self.start
    }
}

/// Which edge of a `RegionSpan` is targeted by a pointer gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionEdge {
    Start,
    End,
}

/// Convert a local horizontal pixel coordinate `x` inside `0.0..=width` into a
/// bounded normalized fraction `0.0..=1.0`. Rejects non-finite or out-of-bounds inputs.
pub fn normalized_region_x(x: f32, width: f32) -> Option<f64> {
    if !x.is_finite() || !width.is_finite() || width <= 0.0 || x < 0.0 || x > width {
        return None;
    }
    Some((f64::from(x) / f64::from(width)).clamp(0.0, 1.0))
}

/// Convert a local horizontal pixel coordinate `x` inside `0.0..=width` into the
/// nearest discrete frame boundary in `0..=total_frames`.
pub fn region_frame_at_x(x: f32, width: f32, total_frames: u64) -> Option<u64> {
    if total_frames == 0 {
        return None;
    }
    let fraction = normalized_region_x(x, width)?;
    Some(((fraction * total_frames as f64).round() as u64).min(total_frames))
}

/// Hit-test the start and end edges of `span` at local horizontal coordinate `x`
/// inside `width` pixels using `tolerance_px`. Ties resolve deterministically to
/// `RegionEdge::Start`.
pub fn hit_test_region_edge(
    span: RegionSpan,
    x: f32,
    width: f32,
    tolerance_px: f32,
) -> Option<RegionEdge> {
    if !x.is_finite()
        || !width.is_finite()
        || !tolerance_px.is_finite()
        || width <= 0.0
        || tolerance_px < 0.0
    {
        return None;
    }
    let left = (x - span.start() as f32 * width).abs();
    let right = (x - span.end() as f32 * width).abs();
    if left.min(right) > tolerance_px {
        return None;
    }
    if left <= right {
        Some(RegionEdge::Start)
    } else {
        Some(RegionEdge::End)
    }
}

/// Failure modes for a bounded normalized region edge drag update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionDragError {
    IdentityChanged,
    InvalidGeometry,
}

/// Outcome of one pointer update on a `RegionDragDraft`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegionDragUpdate {
    /// Pointer remained within the deadzone; original position is preserved.
    WithinDeadzone,
    /// Pointer crossed the deadzone and produced a clamped normalized position in `0.0..=1.0`.
    Moved(f64),
}

/// Identity-bound local drag state for one normalized region edge (`0.0..=1.0`).
///
/// The owner supplies an opaque identity `K` (e.g. device + parameter + bounds)
/// and validates every update against that identity before mutating canonical state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionDragDraft<K: Copy + PartialEq> {
    identity: K,
    edge: RegionEdge,
    anchor_x: f32,
    original_fraction: f64,
    current_fraction: f64,
    deadzone_px: f32,
    moved: bool,
}

impl<K: Copy + PartialEq> RegionDragDraft<K> {
    /// Begin a bounded drag draft. Returns `None` if `anchor_x`, `original_fraction`,
    /// or `deadzone_px` are non-finite or out of range.
    pub fn begin(
        identity: K,
        edge: RegionEdge,
        anchor_x: f32,
        original_fraction: f64,
        deadzone_px: f32,
    ) -> Option<Self> {
        if !anchor_x.is_finite()
            || !original_fraction.is_finite()
            || !(0.0..=1.0).contains(&original_fraction)
            || !deadzone_px.is_finite()
            || deadzone_px < 0.0
        {
            return None;
        }
        Some(Self {
            identity,
            edge,
            anchor_x,
            original_fraction,
            current_fraction: original_fraction,
            deadzone_px,
            moved: false,
        })
    }

    pub const fn identity(&self) -> K {
        self.identity
    }

    pub const fn edge(&self) -> RegionEdge {
        self.edge
    }

    pub const fn original_fraction(&self) -> f64 {
        self.original_fraction
    }

    pub const fn current_fraction(&self) -> f64 {
        self.current_fraction
    }

    pub const fn has_moved(&self) -> bool {
        self.moved
    }

    pub fn matches_identity(&self, identity: K) -> bool {
        self.identity == identity
    }

    /// Advance the draft from the latest pointer x-coordinate and track width.
    pub fn update(
        &mut self,
        identity: K,
        pointer_x: f32,
        width: f32,
    ) -> Result<RegionDragUpdate, RegionDragError> {
        if self.identity != identity {
            return Err(RegionDragError::IdentityChanged);
        }
        let delta = pointer_x - self.anchor_x;
        if !pointer_x.is_finite() || !delta.is_finite() || !width.is_finite() || width <= 0.0 {
            return Err(RegionDragError::InvalidGeometry);
        }
        if !self.moved && delta.abs() < self.deadzone_px {
            return Ok(RegionDragUpdate::WithinDeadzone);
        }
        self.moved = true;
        let x = (self.original_fraction as f32 * width + delta).clamp(0.0, width);
        let fraction = normalized_region_x(x, width).ok_or(RegionDragError::InvalidGeometry)?;
        self.current_fraction = fraction;
        Ok(RegionDragUpdate::Moved(fraction))
    }
}

/// Render an active sub-region highlight band above a bottom ruler inset.
pub fn region_active_band(
    selector: &'static str,
    span: RegionSpan,
    bottom_inset_px: f32,
    fill_rgb: u32,
) -> Div {
    let inset = if bottom_inset_px.is_finite() {
        bottom_inset_px.max(0.0)
    } else {
        0.0
    };
    div()
        .absolute()
        .left(relative(span.start() as f32))
        .w(relative(span.width() as f32))
        .top_0()
        .bottom(px(inset))
        .bg(rgb(fill_rgb))
        .debug_selector(move || selector.into())
}

/// Render a 1 px clamped vertical marker line inside a normalized region surface.
pub fn region_marker(
    selector: &'static str,
    fraction: f64,
    bottom_inset_px: f32,
    color_rgb: u32,
) -> Div {
    let clamped = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let inset = if bottom_inset_px.is_finite() {
        bottom_inset_px.max(0.0)
    } else {
        0.0
    };
    div()
        .absolute()
        .left(relative(clamped))
        .ml(px(-clamped))
        .top_0()
        .w(px(1.0))
        .bottom(px(inset))
        .bg(rgb(color_rgb))
        .debug_selector(move || selector.into())
}

/// Render a centered draggable edge hit-target strip for a normalized region boundary.
pub fn region_edge_handle(
    selector: &'static str,
    fraction: f64,
    bottom_inset_px: f32,
    hit_width_px: f32,
    enabled: bool,
) -> Div {
    let clamped = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let inset = if bottom_inset_px.is_finite() {
        bottom_inset_px.max(0.0)
    } else {
        0.0
    };
    let width = if hit_width_px.is_finite() {
        hit_width_px.max(2.0)
    } else {
        10.0
    };
    div()
        .absolute()
        .left(relative(clamped))
        .ml(px(-width * 0.5))
        .w(px(width))
        .top_0()
        .bottom(px(inset))
        .when(enabled, |handle| handle.cursor_col_resize())
        .debug_selector(move || selector.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_span_validates_normalized_and_frame_bounds() {
        let span = RegionSpan::from_frames(250.0, 750.0, 1000).expect("valid frame span");
        assert_eq!(span.start(), 0.25);
        assert_eq!(span.end(), 0.75);
        assert_eq!(span.width(), 0.5);

        assert!(RegionSpan::new(0.5, 0.5).is_none());
        assert!(RegionSpan::new(0.7, 0.3).is_none());
        assert!(RegionSpan::new(-0.1, 0.5).is_none());
        assert!(RegionSpan::new(0.0, 1.1).is_none());
        assert!(RegionSpan::new(f64::NAN, 1.0).is_none());
        assert!(RegionSpan::from_frames(0.0, 10.0, 0).is_none());
        assert!(RegionSpan::from_frames(0.0, 1001.0, 1000).is_none());
    }

    #[test]
    fn frame_conversion_rounds_and_rejects_invalid_geometry() {
        assert_eq!(region_frame_at_x(0.0, 200.0, 1000), Some(0));
        assert_eq!(region_frame_at_x(100.0, 200.0, 1000), Some(500));
        assert_eq!(region_frame_at_x(200.0, 200.0, 1000), Some(1000));
        assert_eq!(region_frame_at_x(-1.0, 200.0, 1000), None);
        assert_eq!(region_frame_at_x(201.0, 200.0, 1000), None);
        assert_eq!(region_frame_at_x(50.0, 0.0, 1000), None);
        assert_eq!(region_frame_at_x(f32::NAN, 200.0, 1000), None);
        assert_eq!(region_frame_at_x(50.0, 200.0, 0), None);
    }

    #[test]
    fn edge_hit_testing_enforces_tolerance_and_deterministic_tie_break() {
        let span = RegionSpan::new(0.25, 0.75).unwrap();
        assert_eq!(
            hit_test_region_edge(span, 50.0, 200.0, REGION_EDGE_HIT_TOLERANCE_PX),
            Some(RegionEdge::Start)
        );
        assert_eq!(
            hit_test_region_edge(span, 154.0, 200.0, REGION_EDGE_HIT_TOLERANCE_PX),
            Some(RegionEdge::End)
        );
        assert_eq!(
            hit_test_region_edge(span, 100.0, 200.0, REGION_EDGE_HIT_TOLERANCE_PX),
            None
        );

        // Equal distance to both edges deterministically selects Start.
        let narrow = RegionSpan::new(0.49, 0.51).unwrap();
        assert_eq!(
            hit_test_region_edge(narrow, 100.0, 200.0, REGION_EDGE_HIT_TOLERANCE_PX),
            Some(RegionEdge::Start)
        );
        assert_eq!(hit_test_region_edge(span, 50.0, 0.0, 5.0), None);
        assert_eq!(hit_test_region_edge(span, 50.0, 200.0, -1.0), None);
    }

    #[test]
    fn drag_draft_enforces_deadzone_clamping_and_identity_cancellation() {
        assert!(RegionDragDraft::begin(1u64, RegionEdge::Start, f32::NAN, 0.5, 2.0).is_none());
        assert!(RegionDragDraft::begin(1u64, RegionEdge::Start, 10.0, 1.5, 2.0).is_none());
        assert!(RegionDragDraft::begin(1u64, RegionEdge::Start, 10.0, 0.5, -1.0).is_none());

        let mut draft = RegionDragDraft::begin(
            42u64,
            RegionEdge::Start,
            50.0,
            0.25,
            REGION_DRAG_DEADZONE_PX,
        )
        .expect("valid draft");
        assert!(draft.matches_identity(42));
        assert!(!draft.has_moved());

        // Inside deadzone preserves original fraction.
        assert_eq!(
            draft.update(42, 51.5, 200.0),
            Ok(RegionDragUpdate::WithinDeadzone)
        );
        assert!(!draft.has_moved());
        assert_eq!(draft.current_fraction(), 0.25);

        // Crossing deadzone updates normalized position and clamps extremes to 0.0..=1.0.
        assert_eq!(
            draft.update(42, 100.0, 200.0),
            Ok(RegionDragUpdate::Moved(0.5))
        );
        assert!(draft.has_moved());
        assert_eq!(
            draft.update(42, 500.0, 200.0),
            Ok(RegionDragUpdate::Moved(1.0))
        );
        assert_eq!(
            draft.update(42, -500.0, 200.0),
            Ok(RegionDragUpdate::Moved(0.0))
        );

        // Changed identity or invalid geometry fails explicitly without mutating position.
        assert_eq!(
            draft.update(99, 60.0, 200.0),
            Err(RegionDragError::IdentityChanged)
        );
        assert_eq!(
            draft.update(42, f32::NAN, 200.0),
            Err(RegionDragError::InvalidGeometry)
        );
        assert_eq!(
            draft.update(42, 60.0, 0.0),
            Err(RegionDragError::InvalidGeometry)
        );
        assert_eq!(draft.current_fraction(), 0.0);
    }
}
