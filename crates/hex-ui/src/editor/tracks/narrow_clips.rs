//! Paint-only Arrangement detail for clips a few device pixels wide. The
//! ordinary clip elements retain the exact interval and own pointer input.
use gpui::{Bounds, Div, Window, canvas, div, fill, point, prelude::*, px, rgb, size};
use std::{collections::BTreeMap, ops::Range};

const MAX_BAR_WIDTH_DEVICE_PIXELS: f32 = 4.0;

pub(super) fn is_narrow(width: f32, scale: f32) -> bool {
    width > 0.0 && width * scale <= MAX_BAR_WIDTH_DEVICE_PIXELS
}

#[derive(Clone, Copy)]
struct Ink {
    color: u32,
    selected: bool,
}

#[derive(Default)]
pub(super) struct Bars(Vec<(Range<f64>, Ink)>);

impl Bars {
    pub(super) fn push(&mut self, span: Range<f64>, color: u32, selected: bool) {
        self.0.push((span, Ink { color, selected }));
    }

    pub(super) fn view(self, track: hex_project::TrackId) -> Option<Div> {
        (!self.0.is_empty()).then(|| {
            div()
                .debug_selector(move || format!("narrow-clip-bars-{}", track.value()))
                .absolute()
                .inset_0()
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| self.paint(bounds, window),
                    )
                    .absolute()
                    .inset_0(),
                )
        })
    }

    fn paint(&self, bounds: Bounds<gpui::Pixels>, window: &mut Window) {
        let visible = bounds.intersect(&window.content_mask().bounds);
        if visible.is_empty() {
            return;
        }
        let scale = f64::from(window.scale_factor());
        let left = f64::from(bounds.left()) * scale;
        let width = f64::from(bounds.size.width) * scale;
        let first = (f64::from(visible.left()) * scale).floor() as i32;
        let last = (f64::from(visible.right()) * scale).ceil() as i32;
        let mut columns = BTreeMap::<i32, Ink>::new();
        for (span, ink) in &self.0 {
            // One touched device pixel is enough to make a subpixel clip visible.
            let start = (left + span.start * width).floor() as i32;
            let end = (left + span.end * width).ceil() as i32;
            for column in start.max(first)..end.min(last) {
                let existing = columns.entry(column).or_insert(*ink);
                if ink.selected || !existing.selected {
                    *existing = *ink;
                }
            }
        }
        if columns.is_empty() {
            return;
        }
        let paint = |window: &mut Window| {
            let mut columns = columns.into_iter().peekable();
            while let Some((start, ink)) = columns.next() {
                let mut end = start + 1;
                while columns
                    .next_if(|(column, next)| *column == end && next.color == ink.color)
                    .is_some()
                {
                    end += 1;
                }
                let bar = Bounds::new(
                    point(px((f64::from(start) / scale) as f32), bounds.top()),
                    size(
                        px((f64::from(end - start) / scale) as f32),
                        bounds.size.height,
                    ),
                )
                .intersect(&visible);
                window.paint_quad(fill(bar, rgb(ink.color)));
            }
        };
        // At these backing scales the aligned integer endpoints stay exact
        // after projection and clipping. The color runs have no common area.
        let visible_left = f64::from(visible.left()) * scale;
        let visible_right = f64::from(visible.right()) * scale;
        if matches!(scale, 1.0 | 2.0)
            && first >= 0
            && last <= (1 << 23)
            && visible_left.fract() == 0.0
            && visible_right.fract() == 0.0
        {
            window.paint_layer(visible, paint);
        } else {
            paint(window);
        }
    }
}
