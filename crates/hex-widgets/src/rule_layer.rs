//! Noninteractive native rules with one shared element lifecycle. Each rule
//! retains a lightweight Taffy leaf, preserving percentage resolution and
//! device-pixel rounding without a full Div's state/prepaint/paint machinery.
//! Callers own timing, colors and input behavior; this layer owns only layout
//! and drawing. It never clips beyond the caller's existing ancestor mask.
use gpui::{
    App, Background, Bounds, Div, Edges, Element, ElementId, GlobalElementId, IntoElement,
    LayoutId, Pixels, Position, Size, Style, Window, div, fill, prelude::*, px, relative, rgb,
};

/// Per-layer display work budget; this is not a project/content limit.
pub const MAX_RULE_LAYER_RULES: usize = 4_096;

/// One checked vertical rule. Position is a fraction of the containing width;
/// thickness is in logical pixels. RGB and opacity come from the caller.
#[derive(Clone, Copy, Debug)]
pub struct VerticalRule {
    position: f32,
    thickness: Pixels,
    color: u32,
    opacity: f32,
}
impl VerticalRule {
    pub fn new(position: f32, thickness: Pixels, color: u32, opacity: f32) -> Option<Self> {
        if !position.is_finite()
            || !(0.0..=1.0).contains(&position)
            || !f32::from(thickness).is_finite()
            || thickness <= px(0.0)
            || !opacity.is_finite()
            || !(0.0..=1.0).contains(&opacity)
        {
            return None;
        }
        Some(Self {
            position,
            thickness,
            color,
            opacity,
        })
    }
}

/// Absolute-fill passive rule layer. Empty or oversized iterators are refused;
/// iterator consumption/allocation stop at the work budget. Parent containers
/// retain clipping, opacity and pointer/focus ownership.
pub fn vertical_rule_layer(rules: impl IntoIterator<Item = VerticalRule>) -> Option<Div> {
    let mut owned = Vec::new();
    for rule in rules {
        if owned.len() == MAX_RULE_LAYER_RULES {
            return None;
        }
        owned.push(rule);
    }
    (!owned.is_empty()).then(|| div().absolute().inset_0().child(RuleLayer { rules: owned }))
}

struct RuleLayer {
    rules: Vec<VerticalRule>,
}
struct PreparedRule {
    bounds: Bounds<Pixels>,
    background: Background,
}

fn fill_style() -> Style {
    Style {
        position: Position::Absolute,
        inset: Edges::all(px(0.0).into()),
        ..Style::default()
    }
}

fn valid_bounds(bounds: Bounds<Pixels>, scale: f32) -> bool {
    [
        bounds.left(),
        bounds.top(),
        bounds.right(),
        bounds.bottom(),
        bounds.size.width,
        bounds.size.height,
    ]
    .into_iter()
    .all(|value| {
        let value = f32::from(value);
        value.is_finite() && (value * scale).is_finite()
    }) && bounds.size.width > px(0.0)
        && bounds.size.height > px(0.0)
}

impl IntoElement for RuleLayer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for RuleLayer {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = Vec<PreparedRule>;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let scale = window.scale_factor();
        let valid = scale.is_finite()
            && scale > 0.0
            && self
                .rules
                .iter()
                .all(|rule| (f32::from(rule.thickness) * scale).is_finite());
        let leaves = if valid {
            self.rules
                .iter()
                .map(|rule| {
                    let style = Style {
                        position: Position::Absolute,
                        inset: Edges {
                            top: px(0.0).into(),
                            bottom: px(0.0).into(),
                            left: relative(rule.position).into(),
                            ..Edges::auto()
                        },
                        size: Size {
                            width: rule.thickness.into(),
                            ..Size::auto()
                        },
                        ..Style::default()
                    };
                    window.request_layout(style, [], cx)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let layout = window.request_layout(fill_style(), leaves.iter().copied(), cx);
        (layout, leaves)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        leaves: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let scale = window.scale_factor();
        self.rules
            .iter()
            .zip(leaves.iter())
            .filter_map(|(rule, layout)| {
                let bounds = window.layout_bounds(*layout);
                if !valid_bounds(bounds, scale) {
                    return None;
                }
                let background: Background = rgb(rule.color).into();
                Some(PreparedRule {
                    bounds,
                    background: background.opacity(rule.opacity),
                })
            })
            .collect()
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepared: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        for rule in prepared {
            window.paint_quad(fill(rule.bounds, rule.background));
        }
    }
}

#[cfg(test)]
#[path = "rule_layer_tests.rs"]
mod tests;
