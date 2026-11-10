//! Horizontal rectangle interaction shared by native timeline-like surfaces.
//!
//! The owner projects pixels and stable keys. This module owns local hit policy,
//! original-press capture and typed pixel intents, never musical coordinates,
//! selection membership, commands or admission. Native drag and raw window input
//! are transports into the same capture. Release consumes input; it does not
//! retire an owner's immutable pending display. Cancel affects held input only.
use gpui::{CursorStyle, Div, Modifiers, Styled, div, prelude::*, px, relative};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectanglePart {
    Body,
    Leading,
    Trailing,
    Seam,
}
impl RectanglePart {
    pub fn cursor(self) -> CursorStyle {
        match self {
            Self::Body => CursorStyle::OpenHand,
            Self::Leading => CursorStyle::ResizeLeftBracket,
            Self::Trailing => CursorStyle::ResizeRightBracket,
            Self::Seam => CursorStyle::ResizeLeftRight,
        }
    }
}

/// Inner edges can reserve a minimum body width (clips), or meet at the
/// midpoint (sections). The leading edge wins an exact midpoint tie.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectangleEdges {
    pub width: f32,
    pub minimum_body: f32,
}
impl RectangleEdges {
    pub fn handle_width(self, width: f32) -> f32 {
        if !width.is_finite()
            || width <= 0.0
            || !self.width.is_finite()
            || self.width < 0.0
            || !self.minimum_body.is_finite()
            || self.minimum_body < 0.0
        {
            return 0.0;
        }
        self.width.min((width - self.minimum_body).max(0.0) / 2.0)
    }
    pub fn hit(self, x: f32, start: f32, end: f32) -> Option<RectanglePart> {
        if !x.is_finite()
            || !start.is_finite()
            || !end.is_finite()
            || end <= start
            || !(end - start).is_finite()
            || x < start
            || x > end
        {
            return None;
        }
        let width = self.handle_width(end - start);
        if width > 0.0 && (x - start).min(end - x) <= width {
            Some(if x - start <= end - x {
                RectanglePart::Leading
            } else {
                RectanglePart::Trailing
            })
        } else {
            Some(RectanglePart::Body)
        }
    }
    /// A touching pair's seam takes precedence over either individual edge.
    /// Adjacency/visibility is supplied by the owner, never inferred from IDs.
    pub fn seam_hit(x: f32, edge: f32, radius: f32) -> bool {
        x.is_finite()
            && edge.is_finite()
            && radius.is_finite()
            && radius > 0.0
            && (x - edge).abs() <= radius
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectangleSelection {
    Replace,
    Toggle,
    Range { additive: bool },
}
#[derive(Clone, Copy, Debug)]
pub enum RectangleSelectionPolicy {
    /// Clip headers: Shift range, Command toggle, Command+Shift additive range.
    Range,
    /// Section markers: Shift range, Command+Shift toggle. Command alone
    /// belongs to the lane's marker-only edit latch, not selection.
    Section,
}
impl RectangleSelectionPolicy {
    pub fn intent(self, modifiers: Modifiers) -> RectangleSelection {
        match self {
            Self::Range if modifiers.shift => RectangleSelection::Range {
                additive: modifiers.platform,
            },
            Self::Range if modifiers.platform => RectangleSelection::Toggle,
            Self::Section if modifiers.shift && modifiers.platform => RectangleSelection::Toggle,
            Self::Section if modifiers.shift => RectangleSelection::Range { additive: false },
            _ => RectangleSelection::Replace,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectanglePhase {
    Begin,
    Change,
    Commit,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectangleIntent<K> {
    pub key: K,
    pub part: RectanglePart,
    pub phase: RectanglePhase,
    pub origin_x: f32,
    pub pointer_x: f32,
    pub delta_x: f32,
}

/// Finite, keyed press retained across render replacement. Horizontal drag
/// thresholds use current displacement, deliberately not an ever-moved latch:
/// moving out and back retains the existing stationary section-click contract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectangleCapture<K> {
    pub key: K,
    pub part: RectanglePart,
    pub origin_x: f32,
    pointer_x: f32,
}
impl<K: Copy + PartialEq> RectangleCapture<K> {
    pub fn begin(key: K, part: RectanglePart, x: f32, enabled: bool) -> Option<Self> {
        (enabled && x.is_finite()).then_some(Self {
            key,
            part,
            origin_x: x,
            pointer_x: x,
        })
    }
    pub fn take_matching(held: &mut Option<Self>, key: K, part: RectanglePart) -> Option<Self> {
        if held.is_some_and(|held| held.key == key && held.part == part) {
            held.take()
        } else {
            None
        }
    }
    pub fn update(&mut self, x: f32) -> Option<RectangleIntent<K>> {
        if !x.is_finite() || !(x - self.origin_x).is_finite() {
            return None;
        }
        self.pointer_x = x;
        Some(self.intent(RectanglePhase::Change))
    }
    pub fn displaced(self, threshold: f32) -> bool {
        threshold.is_finite()
            && threshold >= 0.0
            && (self.pointer_x - self.origin_x).abs() >= threshold
    }
    pub fn intent(self, phase: RectanglePhase) -> RectangleIntent<K> {
        RectangleIntent {
            key: self.key,
            part: self.part,
            phase,
            origin_x: self.origin_x,
            pointer_x: self.pointer_x,
            delta_x: self.pointer_x - self.origin_x,
        }
    }
    pub fn finish(mut self, x: f32) -> Option<RectangleIntent<K>> {
        self.update(x)?;
        Some(self.intent(RectanglePhase::Commit))
    }
    pub fn cancel(self) -> RectangleIntent<K> {
        self.intent(RectanglePhase::Cancel)
    }
}

/// Standard rectangle border inset in logical pixels. Narrow intervals keep
/// their authored width; native child hit regions use this same inset rather
/// than assuming that every surface has a one-point border.
pub fn rectangle_border_width(pixel_width: f32) -> f32 {
    if pixel_width.is_finite() && pixel_width >= 2.0 {
        1.0
    } else {
        0.0
    }
}

/// Display-only composition. Fractions are projected by the owner; invalid
/// geometry is hidden instead of emitting nonfinite layout values. Children,
/// styling, selection paint and held/released preview content remain composable.
/// `pixel_width` is the owner's projection of the interval onto the full lane.
/// It controls the standard one-point border, never a minimum layout width:
/// omit that border below two points so it cannot widen the interval itself.
/// No pointer cancellation can clear a separately supplied pending display.
pub fn rectangle_surface(left: f32, width: f32, pixel_width: f32) -> Div {
    let valid = left.is_finite()
        && width.is_finite()
        && width > 0.0
        && pixel_width.is_finite()
        && pixel_width >= 0.0;
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .overflow_hidden()
        .left(relative(if valid { left } else { 0.0 }))
        .w(relative(if valid { width } else { 0.0 }))
        .when(
            valid && rectangle_border_width(pixel_width) > 0.0,
            |surface| surface.border_1(),
        )
        .when(!valid, |this| this.invisible())
}

/// Same edge geometry/cursor for native clip handles and raw-pointer sections.
/// Inset compensates the owner's border; it does not change logical hit widths.
pub fn rectangle_edge(part: RectanglePart, width: f32, inset: f32) -> Div {
    let valid = width.is_finite() && width > 0.0 && inset.is_finite();
    let width = if valid { width } else { 0.0 };
    let inset = if valid { inset } else { 0.0 };
    div()
        .absolute()
        .top(px(inset))
        .bottom(px(inset))
        .w(px(width))
        .cursor(part.cursor())
        .when(part == RectanglePart::Leading, |this| this.left(px(inset)))
        .when(part == RectanglePart::Trailing, |this| {
            this.right(px(inset))
        })
        .when(!valid, |this| this.invisible())
}

/// A header-only body receiver can be composed over other content. Hosts bind
/// rectangle drag to this receiver while the remaining body keeps its own input.
pub fn rectangle_body(top: f32, height: f32) -> Div {
    let valid = top.is_finite() && height.is_finite() && height > 0.0;
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(if valid { top } else { 0.0 }))
        .h(px(if valid { height } else { 0.0 }))
        .cursor(RectanglePart::Body.cursor())
        .when(!valid, |this| this.invisible())
}

/// Adapt either pointer transport to a shared pixel intent. An absent original
/// press preserves the host's pre-existing native-drag fallback; it never
/// fabricates a known anchor. Captured presses always validate displacement.
pub fn rectangle_pointer<K: Copy + PartialEq>(
    held: &std::cell::Cell<Option<RectangleCapture<K>>>,
    x: f32,
) -> Option<f32> {
    if let Some(mut capture) = held.get() {
        let intent = capture.update(x)?;
        held.set(Some(capture));
        Some(intent.pointer_x)
    } else {
        x.is_finite().then_some(x)
    }
}

/// Absolute pixel counterpart for a floating or cross-lane preview. It uses
/// the same clipped rectangle composition as in-lane normalized displays.
/// Preserve horizontal endpoints even below handle size, with the same
/// room-dependent standard border as the normalized surface.
pub fn rectangle_surface_pixels(left: f32, top: f32, width: f32, height: f32) -> Div {
    let valid = left.is_finite()
        && top.is_finite()
        && width.is_finite()
        && width > 0.0
        && height.is_finite()
        && height > 0.0;
    div()
        .absolute()
        .overflow_hidden()
        .left(px(if valid { left } else { 0.0 }))
        .top(px(if valid { top } else { 0.0 }))
        .w(px(if valid { width } else { 0.0 }))
        .h(px(if valid { height.max(3.0) } else { 0.0 }))
        .when(valid && rectangle_border_width(width) > 0.0, |surface| {
            surface.border_1()
        })
        .when(!valid, |this| this.invisible())
}
