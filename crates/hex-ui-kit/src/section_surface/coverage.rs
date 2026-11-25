//! Same-frame paint coverage from the section's solid rectangular fills.

use super::SectionFillRects;
use gpui::{Bounds, ScaledPixels, Window};
use std::{cell::Ref, cell::RefCell, rc::Rc};

#[derive(Clone, Default)]
pub struct SectionCoverage(Rc<RefCell<Vec<SectionInterior>>>);

pub struct SectionCoverageRead<'a>(Ref<'a, Vec<SectionInterior>>);

struct SectionInterior {
    upper: PhysicalRect,
    lower_left: Option<PhysicalRect>,
}

impl SectionCoverage {
    pub(super) fn record(&self, rects: SectionFillRects, window: &Window) {
        let scale = window.scale_factor();
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let mask = window.content_mask().bounds.scale(scale);
        let Some(upper) = PhysicalRect::clipped(rects.upper.scale(scale), mask) else {
            return;
        };
        let lower_left = rects
            .lower_left
            .and_then(|bounds| PhysicalRect::clipped(bounds.scale(scale), mask));
        self.0
            .borrow_mut()
            .push(SectionInterior { upper, lower_left });
    }

    // The Main canvas and section wrapper share their enclosing lane's
    // opacity. The wrapper and the fill canvas apply no opacity of their own.
    pub fn for_paint(&self, window: &Window) -> Option<SectionCoverageRead<'_>> {
        let interiors = self.0.borrow();
        if interiors.is_empty()
            || !window.is_paint_fully_opaque()
            || !interiors.is_sorted_by(|a, b| a.upper.left <= b.upper.left)
        {
            return None;
        }
        Some(SectionCoverageRead(interiors))
    }
}

impl SectionCoverageRead<'_> {
    pub fn covers(&self, mark: Bounds<ScaledPixels>, mask: Bounds<ScaledPixels>) -> bool {
        let Some(mark) = PhysicalRect::clipped(mark, mask) else {
            return false;
        };
        let next = self
            .0
            .partition_point(|section| section.upper.left <= mark.left);
        let Some(section) = next.checked_sub(1).and_then(|index| self.0.get(index)) else {
            return false;
        };
        section.upper.covers(mark) || section.lower_left.is_some_and(|rect| rect.covers(mark))
    }
}

#[derive(Clone, Copy)]
struct PhysicalRect {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
}

impl PhysicalRect {
    fn clipped(rect: Bounds<ScaledPixels>, mask: Bounds<ScaledPixels>) -> Option<Self> {
        // Match the independently scaled quad and mask as the shader receives
        // them. Reconstructing a size after clipping can move a rounded edge.
        let [left, right, top, bottom] =
            [rect.left(), rect.right(), rect.top(), rect.bottom()].map(f64::from);
        let [mask_left, mask_right, mask_top, mask_bottom] =
            [mask.left(), mask.right(), mask.top(), mask.bottom()].map(f64::from);
        if ![
            left,
            right,
            top,
            bottom,
            mask_left,
            mask_right,
            mask_top,
            mask_bottom,
        ]
        .into_iter()
        .all(f64::is_finite)
        {
            return None;
        }
        let clipped = Self {
            left: left.max(mask_left),
            right: right.min(mask_right),
            top: top.max(mask_top),
            bottom: bottom.min(mask_bottom),
        };
        (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
    }

    fn covers(self, mark: Self) -> bool {
        // Keep every mark at a join, seam, chamfer or mask edge. Use one
        // opaque rectangle, with a margin of one physical pixel on all sides.
        mark.left - self.left >= 1.0
            && self.right - mark.right >= 1.0
            && mark.top - self.top >= 1.0
            && self.bottom - mark.bottom >= 1.0
    }
}
