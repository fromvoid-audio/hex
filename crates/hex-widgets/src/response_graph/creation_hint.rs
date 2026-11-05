//! Bounded owner-resolved blank-plot guidance, sharing press hit arbitration.
use super::*;

/// Small owner-authored curve icon in graph coordinates: x increases right and
/// y increases up. Paths have 2..=16 finite normalized points. IDs are nonempty
/// ASCII letters/digits/hyphens/underscores, at most 48 bytes, for debug selectors.
/// Invalid hints are omitted; they never alter the underlying creation protocol.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResponseGraphCreationHint {
    pub id: &'static str,
    pub points: &'static [[f32; 2]],
}

impl ResponseGraphCreationHint {
    fn valid(self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= 48
            && self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            && (2..=16).contains(&self.points.len())
            && self
                .points
                .iter()
                .flatten()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Hover {
    pointer: Point<Pixels>,
    hint: ResponseGraphCreationHint,
}

#[derive(Clone, Copy)]
pub(super) enum Hit {
    Node(u32),
    Blank { inside_plot: bool },
}

impl ResponseGraph {
    /// Opt into owner-authored blank creation guidance. This does not enable
    /// creation or change click/drag events. Identical setters preserve stable
    /// hover through owner redraws, including live analyzer model updates.
    pub fn set_creation_hint_resolver(
        &mut self,
        resolver: Option<fn([f32; 2]) -> ResponseGraphCreationHint>,
        cx: &mut Context<Self>,
    ) {
        let same = match (self.creation_hint_resolver, resolver) {
            (None, None) => true,
            (Some(left), Some(right)) => std::ptr::fn_addr_eq(left, right),
            _ => false,
        };
        if !same {
            self.creation_hint_resolver = resolver;
            self.refresh_creation_hint(cx);
        }
    }

    /// Node radius wins even in axis margins; blank creation retains its legacy
    /// clamping behavior, while hover guidance is restricted to the actual plot.
    pub(super) fn pointer_hit(&self, pointer: Point<Pixels>) -> Option<([f32; 2], Hit)> {
        let raw = self.unclamped_position(pointer)?;
        if !raw.iter().all(|value| value.is_finite()) {
            return None;
        }
        let plot = self.plot_bounds()?;
        let width = f32::from(plot.size.width);
        let height = f32::from(plot.size.height);
        let hit = self
            .model
            .nodes
            .iter()
            .filter_map(|node| {
                let dx = (node.position[0] - raw[0]) * width;
                let dy = (node.position[1] - raw[1]) * height;
                let distance = dx.mul_add(dx, dy * dy);
                (distance <= NODE_RADIUS * NODE_RADIUS).then_some((distance, node.id))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0));
        Some((
            raw.map(|value| value.clamp(0.0, 1.0)),
            hit.map_or(
                Hit::Blank {
                    inside_plot: plot.contains(&pointer),
                },
                |(_, id)| Hit::Node(id),
            ),
        ))
    }

    fn resolve_creation_hint(&self, pointer: Point<Pixels>) -> Option<Hover> {
        if !self.editable
            || !self.creation_enabled
            || self.drag.is_some()
            || self.press.is_some()
            || self.scroll.active()
        {
            return None;
        }
        let resolver = self.creation_hint_resolver?;
        let (position, Hit::Blank { inside_plot: true }) = self.pointer_hit(pointer)? else {
            return None;
        };
        let hint = resolver(position);
        hint.valid().then_some(Hover { pointer, hint })
    }

    pub(super) fn clear_creation_hint(&mut self, cx: &mut Context<Self>) {
        if self.creation_hover.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn refresh_creation_hint(&mut self, cx: &mut Context<Self>) {
        let next = self
            .creation_hover
            .and_then(|hover| self.resolve_creation_hint(hover.pointer));
        if next != self.creation_hover {
            self.creation_hover = next;
            cx.notify();
        }
    }

    pub(super) fn hover_creation_hint(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let next = event
            .pressed_button
            .is_none()
            .then(|| self.resolve_creation_hint(event.position))
            .flatten();
        if next != self.creation_hover {
            self.creation_hover = next;
            cx.notify();
        }
    }

    pub(super) fn creation_hint_element(&self) -> Option<gpui::Div> {
        let hover = self.creation_hover?;
        let bounds = self.bounds?;
        let width = 18.0;
        let height = 14.0;
        if bounds.size.width < px(width) || bounds.size.height < px(height) {
            return None;
        }
        let left = (hover.pointer.x - bounds.left() + px(10.0))
            .clamp(px(0.0), bounds.size.width - px(width));
        let top = (hover.pointer.y - bounds.top() + px(10.0))
            .clamp(px(0.0), bounds.size.height - px(height));
        let selector = format!("{}-creation-hint-{}", self.id, hover.hint.id);
        let style = self.style;
        Some(
            div()
                .debug_selector(move || selector.clone())
                .absolute()
                .left(left)
                .top(top)
                .w(px(width))
                .h(px(height))
                .bg(rgb(style.raised))
                .border_1()
                .border_color(rgb(style.border))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let points = hover.hint.points;
                            // A contrasting ink under-stroke keeps the small accent curve
                            // legible without product-specific color or DSP assumptions.
                            for (stroke, color) in [(2.5, style.text), (1.25, style.accent)] {
                                let mut path = PathBuilder::stroke(px(stroke));
                                for (index, [x, y]) in points.iter().copied().enumerate() {
                                    let point = point(
                                        bounds.left() + px(2.0) + (bounds.size.width - px(4.0)) * x,
                                        bounds.top()
                                            + px(2.0)
                                            + (bounds.size.height - px(4.0)) * (1.0 - y),
                                    );
                                    if index == 0 {
                                        path.move_to(point);
                                    } else {
                                        path.line_to(point);
                                    }
                                }
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, rgb(color));
                                }
                            }
                        },
                    )
                    .size_full(),
                ),
        )
    }
}

#[cfg(test)]
mod tests;
