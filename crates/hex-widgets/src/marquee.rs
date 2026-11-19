//! Shared rubber-band selection law for point editors (response graphs and
//! automation lanes). Owners project their points to pixels and own selection
//! membership as view state; this module owns only the pixel rectangle, the
//! click/drag threshold, inclusive hit-testing, modifier policy and painting.
//!
//! The owner chooses which blank presses can start a marquee. It becomes one
//! after moving at least [`MARQUEE_DRAG_THRESHOLD`] pixels. Shift can reserve
//! marquee selection on surfaces that add points on a plain click. The EQ
//! reserves double-click for creation and uses plain drag for marquee. Owners
//! capture modifiers at press time and restore the selection on cancellation.
use gpui::{Bounds, Modifiers, PathBuilder, Pixels, Window, point, px, rgb, rgba};

/// Pixel distance a blank press must travel before it becomes a marquee.
pub const MARQUEE_DRAG_THRESHOLD: f32 = 3.0;

/// How marquee hits combine with the selection present at the press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarqueeMode {
    /// The hits become the selection.
    Replace,
    /// Union with the press-time selection.
    Add,
    /// Shift+Cmd: symmetric difference with the press-time selection.
    Toggle,
}

impl MarqueeMode {
    /// Whether a blank press starts a marquee on a Shift-only surface.
    pub fn starts(modifiers: Modifiers) -> bool {
        modifiers.shift
    }

    /// Mode for a marquee started by [`Self::starts`]; Cmd toggles.
    pub fn from_modifiers(modifiers: Modifiers) -> Self {
        if modifiers.platform {
            Self::Toggle
        } else {
            Self::Replace
        }
    }

    /// Combine press-time `base` membership with the current `hits`. The result
    /// is sorted and deduplicated so owners can compare successive sets.
    pub fn combine<T: Copy + Ord>(self, base: &[T], hits: &[T]) -> Vec<T> {
        let mut hits = hits.to_vec();
        hits.sort_unstable();
        hits.dedup();
        let mut base = base.to_vec();
        base.sort_unstable();
        base.dedup();
        match self {
            Self::Replace => hits,
            Self::Add => {
                base.extend(hits);
                base.sort_unstable();
                base.dedup();
                base
            }
            Self::Toggle => {
                let mut result: Vec<T> = base
                    .iter()
                    .copied()
                    .filter(|id| hits.binary_search(id).is_err())
                    .collect();
                result.extend(
                    hits.iter()
                        .copied()
                        .filter(|id| base.binary_search(id).is_err()),
                );
                result.sort_unstable();
                result
            }
        }
    }
}

/// One pixel-space rubber band from its press origin to the current pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marquee {
    origin: [f32; 2],
    current: [f32; 2],
}

impl Marquee {
    /// Refuses non-finite pointer coordinates.
    pub fn new(origin: [f32; 2]) -> Option<Self> {
        origin
            .iter()
            .all(|value| value.is_finite())
            .then_some(Self {
                origin,
                current: origin,
            })
    }

    /// Non-finite updates are ignored, preserving the last valid rectangle.
    pub fn update(&mut self, current: [f32; 2]) {
        if current.iter().all(|value| value.is_finite()) {
            self.current = current;
        }
    }

    pub fn origin(&self) -> [f32; 2] {
        self.origin
    }

    /// Whether the pointer has travelled far enough to be a drag, not a click.
    pub fn crossed_threshold(&self) -> bool {
        let dx = self.current[0] - self.origin[0];
        let dy = self.current[1] - self.origin[1];
        dx.mul_add(dx, dy * dy) >= MARQUEE_DRAG_THRESHOLD * MARQUEE_DRAG_THRESHOLD
    }

    /// Normalized `[left, top, right, bottom]`, independent of drag direction.
    pub fn rect(&self) -> [f32; 4] {
        [
            self.origin[0].min(self.current[0]),
            self.origin[1].min(self.current[1]),
            self.origin[0].max(self.current[0]),
            self.origin[1].max(self.current[1]),
        ]
    }

    /// Inclusive edges, so a point exactly on the band boundary is selected.
    pub fn contains(&self, position: [f32; 2]) -> bool {
        let [left, top, right, bottom] = self.rect();
        position[0].is_finite()
            && position[1].is_finite()
            && (left..=right).contains(&position[0])
            && (top..=bottom).contains(&position[1])
    }

    /// Ids of the projected points inside the band, in input order.
    pub fn hits<T: Copy>(&self, points: impl IntoIterator<Item = (T, [f32; 2])>) -> Vec<T> {
        points
            .into_iter()
            .filter(|(_, position)| self.contains(*position))
            .map(|(id, _)| id)
            .collect()
    }

    /// Paint the band clipped to `clip`: a faint fill and a 1px outline in `color`.
    pub fn paint(&self, clip: Bounds<Pixels>, color: u32, window: &mut Window) {
        let [left, top, right, bottom] = self.rect();
        let left = left.max(f32::from(clip.left()));
        let top = top.max(f32::from(clip.top()));
        let right = right.min(f32::from(clip.right()));
        let bottom = bottom.min(f32::from(clip.bottom()));
        if right <= left || bottom <= top {
            return;
        }
        let corners = [
            point(px(left), px(top)),
            point(px(right), px(top)),
            point(px(right), px(bottom)),
            point(px(left), px(bottom)),
        ];
        let mut fill = PathBuilder::fill();
        fill.move_to(corners[0]);
        for corner in &corners[1..] {
            fill.line_to(*corner);
        }
        fill.close();
        if let Ok(fill) = fill.build() {
            window.paint_path(fill, rgba(((color & 0xff_ffff) << 8) | 0x24));
        }
        let mut outline = PathBuilder::stroke(px(1.0));
        outline.move_to(corners[0]);
        for corner in &corners[1..] {
            outline.line_to(*corner);
        }
        outline.line_to(corners[0]);
        if let Ok(outline) = outline.build() {
            window.paint_path(outline, rgb(color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_normalizes_any_drag_direction_and_hits_inclusive_edges() {
        let mut marquee = Marquee::new([50.0, 40.0]).unwrap();
        marquee.update([10.0, 80.0]);
        assert_eq!(marquee.rect(), [10.0, 40.0, 50.0, 80.0]);
        assert!(marquee.contains([10.0, 40.0]));
        assert!(marquee.contains([50.0, 80.0]));
        assert!(marquee.contains([30.0, 60.0]));
        assert!(!marquee.contains([9.9, 60.0]));
        assert!(!marquee.contains([30.0, 80.1]));
        assert!(!marquee.contains([f32::NAN, 60.0]));
        let points = [(1, [20.0, 50.0]), (2, [60.0, 50.0]), (3, [50.0, 40.0])];
        assert_eq!(marquee.hits(points), vec![1, 3]);
    }

    #[test]
    fn threshold_separates_clicks_from_drags_and_rejects_non_finite_input() {
        assert!(Marquee::new([f32::INFINITY, 0.0]).is_none());
        let mut marquee = Marquee::new([0.0, 0.0]).unwrap();
        marquee.update([2.0, 2.0]);
        assert!(!marquee.crossed_threshold());
        marquee.update([3.0, 0.0]);
        assert!(marquee.crossed_threshold());
        marquee.update([f32::NAN, 9.0]);
        assert_eq!(marquee.rect(), [0.0, 0.0, 3.0, 0.0]);
    }

    #[test]
    fn only_shift_starts_a_marquee_and_cmd_toggles_against_press_time_selection() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let cmd = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        assert!(!MarqueeMode::starts(Modifiers::default()));
        assert!(!MarqueeMode::starts(cmd));
        assert!(MarqueeMode::starts(shift));
        assert_eq!(MarqueeMode::from_modifiers(shift), MarqueeMode::Replace);
        assert_eq!(
            MarqueeMode::from_modifiers(Modifiers {
                shift: true,
                platform: true,
                ..Modifiers::default()
            }),
            MarqueeMode::Toggle
        );
        let base = [5, 1, 3];
        let hits = [3, 4, 4];
        assert_eq!(MarqueeMode::Replace.combine(&base, &hits), vec![3, 4]);
        assert_eq!(MarqueeMode::Add.combine(&base, &hits), vec![1, 3, 4, 5]);
        assert_eq!(MarqueeMode::Toggle.combine(&base, &hits), vec![1, 4, 5]);
        assert_eq!(MarqueeMode::Add.combine(&base, &[]), vec![1, 3, 5]);
        assert!(MarqueeMode::Replace.combine::<u32>(&base, &[]).is_empty());
    }
}
