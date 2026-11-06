//! Generated peak geometry for the actual shared waveform display. The one-entry
//! fixture cache owns only UI paths; there is no PCM, media or audio execution.
use gpui::{Bounds, Div, Pixels, point, prelude::*, px};
use hex_widgets::{
    ControlStyle, WaveformChannel, WaveformColors, WaveformEmphasis, WaveformEnvelope,
    WaveformEnvelopeMode, WaveformGeometry, WaveformRange, WaveformStroke, waveform_plot,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[derive(Default)]
pub(super) struct Cache {
    entry: Option<Entry>,
}
struct Entry {
    bounds: Bounds<Pixels>,
    scale: f32,
    stereo: bool,
    amount: f32,
    geometry: Arc<WaveformGeometry>,
}
impl Cache {
    // Only layout_review's sample tests read this; hex-sandbox includes this
    // module without them, so its test build would otherwise flag it unused.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn retained(&self) -> Option<Arc<WaveformGeometry>> {
        self.entry.as_ref().map(|entry| Arc::clone(&entry.geometry))
    }
    fn geometry(
        &mut self,
        bounds: Bounds<Pixels>,
        scale: f32,
        stereo: bool,
        amount: f32,
    ) -> Option<Arc<WaveformGeometry>> {
        if let Some(entry) = &self.entry
            && entry.bounds == bounds
            && entry.scale == scale
            && entry.stereo == stereo
            && entry.amount == amount
        {
            return Some(Arc::clone(&entry.geometry));
        }
        let geometry = Arc::new(prepare(bounds, scale, stereo, amount)?);
        self.entry = Some(Entry {
            bounds,
            scale,
            stereo,
            amount,
            geometry: Arc::clone(&geometry),
        });
        Some(geometry)
    }
}
fn prepare(
    bounds: Bounds<Pixels>,
    scale: f32,
    stereo: bool,
    amount: f32,
) -> Option<WaveformGeometry> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if !width.is_finite()
        || width <= 0.0
        || !height.is_finite()
        || height <= 0.0
        || !scale.is_finite()
        || scale <= 0.0
        || !amount.is_finite()
        || !(0.0..=1.0).contains(&amount)
    {
        return None;
    }
    let count = if stereo { 2 } else { 1 };
    let mut channels = Vec::with_capacity(count);
    const COLUMNS: usize = 64;
    for channel in 0..count {
        let band = height / count as f32;
        let center = bounds.origin.y + px((channel as f32 + 0.5) * band);
        let amplitude = band * 0.45 * amount;
        let mut upper = Vec::with_capacity(COLUMNS + 2);
        let mut lower = Vec::with_capacity(COLUMNS + 2);
        for column in 0..COLUMNS {
            let (minimum, maximum) = match column {
                0..=7 => (0.0, 0.0),
                8..=15 => {
                    let dc = if channel == 0 { 0.25 } else { -0.25 };
                    (dc, dc)
                }
                _ => {
                    let level = ((column * 7 + channel * 11) % 31) as f32 / 31.0;
                    (-0.15 - level * 0.8, 0.1 + (1.0 - level) * 0.7)
                }
            };
            let x = bounds.origin.x + px((column as f32 + 0.5) * width / COLUMNS as f32);
            upper.push(point(x, center - px(maximum * amplitude)));
            lower.push(point(x, center - px(minimum * amplitude)));
        }
        for points in [&mut upper, &mut lower] {
            let mut first = *points.first()?;
            first.x = bounds.left();
            points.insert(0, first);
            let mut last = *points.last()?;
            last.x = bounds.right();
            points.push(last);
        }
        let area = upper
            .iter()
            .zip(&lower)
            .any(|(top, bottom)| top.y != bottom.y);
        let flat = (0..COLUMNS).filter_map(|column| {
            let top = upper.get(column + 1)?;
            let bottom = lower.get(column + 1)?;
            (area && top.y == bottom.y).then(|| {
                [
                    point(
                        bounds.left() + px(column as f32 * width / COLUMNS as f32),
                        top.y,
                    ),
                    point(
                        bounds.left() + px((column + 1) as f32 * width / COLUMNS as f32),
                        top.y,
                    ),
                ]
            })
        });
        channels.push(WaveformChannel::new(
            WaveformEnvelope {
                upper: &upper,
                lower: &lower,
                mode: if area {
                    WaveformEnvelopeMode::Filled
                } else {
                    WaveformEnvelopeMode::Trace
                },
                trace_width: px(1.0 / scale),
                centerline: Some(WaveformStroke {
                    points: [point(bounds.left(), center), point(bounds.right(), center)],
                    width: px(0.5),
                }),
            },
            flat,
        )?);
    }
    Some(if stereo {
        WaveformGeometry::stereo(channels.try_into().ok()?)
    } else {
        WaveformGeometry::mono(channels.into_iter().next()?)
    })
}
pub(super) fn view(
    id: String,
    stereo: bool,
    values: [f64; 3],
    cache: Rc<RefCell<Cache>>,
    style: ControlStyle,
) -> Div {
    let range = WaveformRange::new(
        values[0].min(values[2]) / 100.0,
        values[0].max(values[2]) / 100.0,
    );
    let amount = (values[1] / 100.0) as f32;
    waveform_plot(
        move |paint| {
            cache
                .try_borrow_mut()
                .ok()?
                .geometry(paint.bounds, paint.scale, stereo, amount)
        },
        WaveformColors {
            waveform: style.text,
            inactive: style.muted,
            centerline: style.border,
        },
        WaveformEmphasis::Range(range),
    )
    .debug_selector(move || id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;
    #[test]
    fn fixture_cache_retains_same_paths_and_replaces_only_geometry_inputs() {
        let bounds = Bounds::new(point(px(4.0), px(8.0)), size(px(240.0), px(120.0)));
        let mut cache = Cache::default();
        let first = cache.geometry(bounds, 2.0, false, 0.5).unwrap();
        assert_eq!(first.channels().len(), 1);
        assert!(first.channels()[0].has_zero_range_trace());
        assert!(Arc::ptr_eq(
            &first,
            &cache.geometry(bounds, 2.0, false, 0.5).unwrap()
        ));
        let changed = cache.geometry(bounds, 2.0, true, 0.5).unwrap();
        assert_eq!(changed.channels().len(), 2);
        assert!(!Arc::ptr_eq(&first, &changed));
        let silent = cache.geometry(bounds, 2.0, true, 0.0).unwrap();
        assert!(
            silent
                .channels()
                .iter()
                .all(|channel| !channel.has_zero_range_trace())
        );
        assert!(cache.geometry(bounds, 0.0, true, 0.5).is_none());
        assert!(cache.geometry(bounds, 2.0, true, f32::NAN).is_none());
    }
}
