//! Bounded read-only overlaid curves; product owns identities, units and colors.
use crate::ControlStyle;
use gpui::{Div, PathBuilder, canvas, div, point, prelude::*, px, rgb};
use std::{ops::Deref, sync::Arc};

pub const MAX_SPECTRUM_PLOT_LAYERS: usize = 257;
const MAX_POINTS: usize = 256;
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumPlotLayer {
    pub points: Vec<[f32; 2]>,
    pub color: u32,
}
/// Validated immutable curves. Cloning shares all point storage, so an
/// unchanged render does not allocate or revalidate every member's points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpectrumPlotLayers(Arc<[SpectrumPlotLayer]>);
impl SpectrumPlotLayers {
    pub fn new(layers: Vec<SpectrumPlotLayer>) -> Option<Self> {
        if layers.len() > MAX_SPECTRUM_PLOT_LAYERS
            || layers.iter().any(|layer| {
                !(2..=MAX_POINTS).contains(&layer.points.len())
                    || layer.points.iter().any(|[x, y]| {
                        !x.is_finite()
                            || !y.is_finite()
                            || !(0.0..=1.0).contains(x)
                            || !(0.0..=1.0).contains(y)
                    })
                    || !layer.points.windows(2).all(|p| p[0][0] <= p[1][0])
            })
        {
            return None;
        }
        Some(Self(layers.into()))
    }
}
impl Deref for SpectrumPlotLayers {
    type Target = [SpectrumPlotLayer];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
/// Paint every supplied independent layer in order, with the final layer above
/// the others. No summation, domain conversion, signal processing or selection.
pub fn layered_spectrum_plot(layers: SpectrumPlotLayers, style: ControlStyle) -> Div {
    div()
        .relative()
        .size_full()
        .bg(rgb(style.background))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let width = bounds.size.width - px(8.0);
                    let height = bounds.size.height - px(8.0);
                    if width <= px(0.0) || height <= px(0.0) {
                        return;
                    }
                    let origin = bounds.origin + point(px(4.0), px(4.0));
                    let mut grid = PathBuilder::stroke(px(0.5));
                    for division in 1..4 {
                        let y = origin.y + height * division as f32 / 4.0;
                        grid.move_to(point(origin.x, y));
                        grid.line_to(point(origin.x + width, y));
                    }
                    for division in 1..8 {
                        let x = origin.x + width * division as f32 / 8.0;
                        grid.move_to(point(x, origin.y));
                        grid.line_to(point(x, origin.y + height));
                    }
                    if let Ok(path) = grid.build() {
                        window.paint_path(path, rgb(style.rail));
                    }
                    for layer in layers.iter() {
                        let mut curve = PathBuilder::stroke(px(1.0));
                        for (i, [x, y]) in layer.points.iter().enumerate() {
                            let p = point(origin.x + width * *x, origin.y + height * (1.0 - *y));
                            if i == 0 {
                                curve.move_to(p);
                            } else {
                                curve.line_to(p);
                            }
                        }
                        if let Ok(path) = curve.build() {
                            window.paint_path(path, rgb(layer.color));
                        }
                    }
                },
            )
            .size_full(),
        )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_or_excess_layers_are_refused_without_dropping_members() {
        let layer = SpectrumPlotLayer {
            points: vec![[0.0, 0.0], [1.0, 1.0]],
            color: 0x123456,
        };
        assert!(SpectrumPlotLayers::new(vec![layer.clone(); MAX_SPECTRUM_PLOT_LAYERS]).is_some());
        assert!(
            SpectrumPlotLayers::new(vec![layer.clone(); MAX_SPECTRUM_PLOT_LAYERS + 1]).is_none()
        );
        let invalid = SpectrumPlotLayer {
            points: vec![[1.0, 0.0], [0.0, 1.0]],
            ..layer
        };
        assert!(SpectrumPlotLayers::new(vec![invalid]).is_none());
    }
}
