//! Full-source, peak-only waveform display shared by sampler and browser.
//! The two-entry UI cache holds bounded device-pixel silhouettes, never PCM.
//! Sampler cache identity uses verified original content facts, not a bank or a
//! retained overview table; New/Open can therefore release its charged backing.
use super::{theme, waveform::waveform_column_count};
#[cfg(test)]
use gpui::size;
use gpui::{Bounds, Div, Pixels, div, point, prelude::*, px};
use hex_project::{AssetId, OriginalSourceFacts};
use hex_session::{CachedPeak, CachedWaveform, OriginalSourceOverviews};
use hex_widgets::{
    WaveformChannel, WaveformColors, WaveformEmphasis, WaveformEnvelope, WaveformEnvelopeMode,
    WaveformRange, waveform_plot,
};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};

#[derive(Clone)]
pub(super) enum Source {
    Sampler {
        table: OriginalSourceOverviews,
        asset: AssetId,
    },
    Browser(Arc<CachedWaveform>),
}
#[derive(Clone)]
enum Identity {
    Sampler(OriginalSourceFacts, u64, u32),
    Browser(Arc<CachedWaveform>),
}
impl Identity {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Sampler(a, af, ar), Self::Sampler(b, bf, br)) => a == b && af == bf && ar == br,
            (Self::Browser(a), Self::Browser(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}
impl Source {
    fn identity(&self) -> Option<(Identity, u64)> {
        match self {
            Self::Sampler { table, asset } => {
                let overview = table.get(*asset)?;
                Some((
                    Identity::Sampler(
                        overview.content(),
                        overview.source_frames(),
                        overview.sample_rate(),
                    ),
                    overview.source_frames(),
                ))
            }
            Self::Browser(waveform) => Some((
                Identity::Browser(Arc::clone(waveform)),
                u64::try_from(waveform.source_frames()).ok()?,
            )),
        }
    }
    fn peaks(&self, start: u64, end: u64) -> Option<[CachedPeak; 2]> {
        match self {
            Self::Sampler { table, asset } => table.get(*asset)?.peaks_for_frames(start, end),
            Self::Browser(waveform) => {
                waveform.peaks_for_frames(usize::try_from(start).ok()?, usize::try_from(end).ok()?)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Key([u32; 5]);
impl Key {
    fn new(bounds: Bounds<Pixels>, scale: f32) -> Self {
        Self([
            f32::from(bounds.origin.x).to_bits(),
            f32::from(bounds.origin.y).to_bits(),
            f32::from(bounds.size.width).to_bits(),
            f32::from(bounds.size.height).to_bits(),
            scale.to_bits(),
        ])
    }
}
struct Geometry {
    display: hex_widgets::WaveformGeometry,
    #[cfg(all(test, feature = "ui-unit-tier"))]
    zero_range_spans: [Vec<(gpui::Point<Pixels>, gpui::Point<Pixels>)>; 2],
    #[cfg(test)]
    columns: usize,
    #[cfg(all(test, feature = "ui-unit-tier"))]
    outlines: [Vec<gpui::Point<Pixels>>; 2],
}
impl AsRef<hex_widgets::WaveformGeometry> for Geometry {
    fn as_ref(&self) -> &hex_widgets::WaveformGeometry {
        &self.display
    }
}
struct Entry {
    identity: Identity,
    key: Key,
    geometry: Rc<Geometry>,
}
#[derive(Default)]
pub(super) struct Cache {
    entries: VecDeque<Entry>,
}
impl Cache {
    /// Read-only wiring witness: entries are populated by the canvas paint
    /// closure. Geometry unit tests separately verify their actual outline.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn prepared_columns(
        &self,
        source: &Source,
        bounds: Bounds<Pixels>,
        scale: f32,
    ) -> Option<usize> {
        let (identity, _) = source.identity()?;
        let key = Key::new(bounds, scale);
        self.entries
            .iter()
            .find(|entry| entry.key == key && entry.identity.same(&identity))
            .map(|entry| entry.geometry.columns)
    }

    fn geometry(
        &mut self,
        source: &Source,
        bounds: Bounds<Pixels>,
        scale: f32,
    ) -> Option<Rc<Geometry>> {
        let (identity, frames) = source.identity()?;
        let key = Key::new(bounds, scale);
        if let Some(index) = self
            .entries
            .iter()
            .position(|e| e.key == key && e.identity.same(&identity))
        {
            let entry = self.entries.remove(index)?;
            let result = Rc::clone(&entry.geometry);
            self.entries.push_back(entry);
            return Some(result);
        }
        let geometry = Rc::new(prepare(frames, bounds, scale, |start, end| {
            source.peaks(start, end)
        })?);
        while self.entries.len() >= 2 {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            identity,
            key,
            geometry: Rc::clone(&geometry),
        });
        Some(geometry)
    }
}

/// Source-relative pixel columns include every original frame, with conservative
/// cached extrema at leaf boundaries. Stereo remains signed and independently
/// scaled; connecting the envelopes avoids the old widely spaced block bars.
fn prepare(
    frames: u64,
    bounds: Bounds<Pixels>,
    scale: f32,
    mut peaks: impl FnMut(u64, u64) -> Option<[CachedPeak; 2]>,
) -> Option<Geometry> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if frames == 0
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !scale.is_finite()
        || scale <= 0.0
    {
        return None;
    }
    let columns = waveform_column_count(width, scale);
    let mut sampled = Vec::with_capacity(columns);
    for column in 0..columns {
        let start = (u128::from(frames) * column as u128 / columns as u128) as u64;
        let end = ((u128::from(frames) * (column + 1) as u128).div_ceil(columns as u128) as u64)
            .max(start + 1)
            .min(frames);
        sampled.push(peaks(start, end)?);
    }
    let mut channels = Vec::with_capacity(2);
    #[cfg(all(test, feature = "ui-unit-tier"))]
    let mut zero_range_spans = Vec::with_capacity(2);
    #[cfg(all(test, feature = "ui-unit-tier"))]
    let mut outlines = Vec::with_capacity(2);
    for channel in 0..2 {
        let half = height * 0.25;
        let center = half + channel as f32 * height * 0.5;
        let amplitude = (half - 1.0 / scale).max(0.0);
        let mut upper = Vec::with_capacity(columns + 2);
        let mut lower = Vec::with_capacity(columns + 2);
        for (index, pair) in sampled.iter().enumerate() {
            let x = (index as f32 + 0.5) * width / columns as f32;
            let peak = pair[channel];
            if !peak.min.is_finite() || !peak.max.is_finite() || peak.min > peak.max {
                return None;
            }
            upper.push(
                bounds.origin + point(px(x), px(center - peak.max.clamp(-1.0, 1.0) * amplitude)),
            );
            lower.push(
                bounds.origin + point(px(x), px(center - peak.min.clamp(-1.0, 1.0) * amplitude)),
            );
        }
        // Cover both exact source boundaries, including a one-frame source.
        let mut left = *upper.first()?;
        left.x = bounds.left();
        upper.insert(0, left);
        let mut right = *upper.last()?;
        right.x = bounds.right();
        upper.push(right);
        let mut left = *lower.first()?;
        left.x = bounds.left();
        lower.insert(0, left);
        let mut right = *lower.last()?;
        right.x = bounds.right();
        lower.push(right);
        let area = sampled
            .iter()
            .any(|pair| pair[channel].max != pair[channel].min);
        // Filled envelopes have no area at DC/silence. Keep the exact original
        // per-bin strokes without widening extrema or changing source mapping.
        let spans = (0..columns).filter_map(|column| {
            let top = upper.get(column + 1)?;
            let bottom = lower.get(column + 1)?;
            if !area || top.y != bottom.y {
                return None;
            }
            let left = point(
                bounds.left() + px(column as f32 * width / columns as f32),
                top.y,
            );
            let right = point(
                bounds.left() + px((column + 1) as f32 * width / columns as f32),
                top.y,
            );
            Some([left, right])
        });
        #[cfg(all(test, feature = "ui-unit-tier"))]
        zero_range_spans.push(spans.clone().map(|[left, right]| (left, right)).collect());
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
                centerline: None,
            },
            spans,
        )?);
        #[cfg(all(test, feature = "ui-unit-tier"))]
        outlines.push(upper.into_iter().chain(lower).collect());
    }
    Some(Geometry {
        display: hex_widgets::WaveformGeometry::stereo(channels.try_into().ok()?),
        #[cfg(all(test, feature = "ui-unit-tier"))]
        zero_range_spans: zero_range_spans.try_into().ok()?,
        #[cfg(test)]
        columns,
        #[cfg(all(test, feature = "ui-unit-tier"))]
        outlines: outlines.try_into().ok()?,
    })
}

pub(super) fn render(source: Source, cache: Rc<RefCell<Cache>>, active: Option<(f64, f64)>) -> Div {
    let emphasis = match active {
        Some((start, end)) => {
            let Some(range) = WaveformRange::new(start, end) else {
                return div().absolute().inset_0();
            };
            WaveformEmphasis::Range(Some(range))
        }
        None => WaveformEmphasis::Range(None),
    };
    waveform_plot(
        move |paint| {
            cache
                .try_borrow_mut()
                .ok()
                .and_then(|mut cache| cache.geometry(&source, paint.bounds, paint.scale))
        },
        WaveformColors {
            waveform: theme::color(theme::PaletteRole::Waveform),
            inactive: theme::color(theme::PaletteRole::Muted),
            centerline: theme::color(theme::PaletteRole::Border),
        },
        emphasis,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bounds(width: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(17.25), px(23.5)), size(px(width), px(100.0)))
    }
    fn source() -> Source {
        let bins = (0..4096)
            .map(|index| {
                let peak = if index == 1237 {
                    CachedPeak {
                        min: -1.0,
                        max: 0.9,
                    }
                } else {
                    CachedPeak {
                        min: 0.125,
                        max: 0.25,
                    }
                };
                [
                    peak,
                    CachedPeak {
                        min: -0.5,
                        max: -0.25,
                    },
                ]
            })
            .collect();
        Source::Browser(Arc::new(CachedWaveform::from_peaks(4096, bins).unwrap()))
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn device_pixel_outline_is_bounded_connected_and_preserves_stereo_extrema_on_resize() {
        let source = source();
        for (width, scale, expected) in
            [(783.25, 1.0, 784), (783.25, 2.0, 1567), (5000.0, 2.0, 4096)]
        {
            let extent = bounds(width);
            let geometry =
                prepare(4096, extent, scale, |start, end| source.peaks(start, end)).unwrap();
            assert_eq!(geometry.columns, expected);
            for outline in &geometry.outlines {
                assert_eq!(outline.len(), (expected + 2) * 2);
                assert_eq!(outline.first().unwrap().x, extent.left());
                assert_eq!(outline[expected + 1].x, extent.right());
                assert!(outline.iter().all(|p| p.x >= extent.left()
                    && p.x <= extent.right()
                    && p.y >= extent.top()
                    && p.y <= extent.bottom()));
                for pair in outline[..expected + 2].windows(2) {
                    assert!(pair[1].x >= pair[0].x);
                    assert!(f32::from(pair[1].x - pair[0].x) <= width / expected as f32 + 0.001);
                }
            }
            let top = geometry.outlines[0]
                .iter()
                .map(|p| f32::from(p.y))
                .fold(f32::INFINITY, f32::min);
            let bottom = geometry.outlines[0]
                .iter()
                .map(|p| f32::from(p.y))
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((top - (23.5 + 25.0 - 0.9 * (25.0 - 1.0 / scale))).abs() < 0.001);
            assert!((bottom - (23.5 + 25.0 + (25.0 - 1.0 / scale))).abs() < 0.001);
            assert!(
                geometry.outlines[1]
                    .iter()
                    .all(|p| f32::from(p.y) > 23.5 + 75.0),
                "negative right channel stays below its own centerline"
            );
        }
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn silence_dc_short_sources_and_invalid_bounds_have_defined_geometry() {
        for sample in [0.0, 0.7, -0.7] {
            let geometry = prepare(1, bounds(200.0), 2.0, |start, end| {
                assert_eq!((start, end), (0, 1));
                Some(
                    [CachedPeak {
                        min: sample,
                        max: sample,
                    }; 2],
                )
            })
            .unwrap();
            assert!(
                geometry
                    .display
                    .channels()
                    .iter()
                    .all(|channel| !channel.has_zero_range_trace())
            );
            for (channel, outline) in geometry.outlines.iter().enumerate() {
                let expected = 23.5 + 25.0 + channel as f32 * 50.0 - sample * 24.5;
                assert!(
                    outline
                        .iter()
                        .all(|p| (f32::from(p.y) - expected).abs() < 0.001)
                );
            }
        }
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(prepare(1, bounds(width), 2.0, |_, _| None).is_none());
        }
        assert!(prepare(0, bounds(20.0), 2.0, |_, _| None).is_none());
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn mixed_transients_dc_and_silence_keep_true_extrema_and_zero_range_hairlines() {
        for scale in [1.0, 2.0] {
            let width = 120.0;
            let columns = waveform_column_count(width, scale);
            let query = |start: u64, _: u64| {
                let section = start / 100;
                Some(match section {
                    0 => [
                        CachedPeak {
                            min: -0.9,
                            max: 0.8,
                        },
                        CachedPeak {
                            min: -0.8,
                            max: 0.9,
                        },
                    ],
                    1 => [
                        CachedPeak { min: 0.5, max: 0.5 },
                        CachedPeak {
                            min: -0.5,
                            max: -0.5,
                        },
                    ],
                    _ => [CachedPeak { min: 0.0, max: 0.0 }; 2],
                })
            };
            let geometry = prepare(300, bounds(width), scale, query).unwrap();
            for channel in 0..2 {
                assert!(geometry.display.channels()[channel].has_zero_range_trace());
                assert_eq!(geometry.zero_range_spans[channel].len(), columns * 2 / 3);
                let center = 23.5 + 25.0 + channel as f32 * 50.0;
                let amplitude = 25.0 - 1.0 / scale;
                let dc = if channel == 0 { 0.5 } else { -0.5 };
                for (index, (left, right)) in geometry.zero_range_spans[channel].iter().enumerate()
                {
                    let expected_x = 17.25 + width / 3.0 + index as f32 * width / columns as f32;
                    let expected_y = if index < columns / 3 {
                        center - dc * amplitude
                    } else {
                        center
                    };
                    assert!((f32::from(left.x) - expected_x).abs() < 0.001);
                    assert!((f32::from(right.x - left.x) - 1.0 / scale).abs() < 0.001);
                    assert!((f32::from(left.y) - expected_y).abs() < 0.001);
                    assert_eq!(left.y, right.y);
                }
                let transient = &geometry.outlines[channel][1];
                let expected_peak = if channel == 0 { 0.8 } else { 0.9 };
                assert!(
                    (f32::from(transient.y) - (center - expected_peak * amplitude)).abs() < 0.001,
                    "hairlines must not change true transient peak positions"
                );
            }
        }
    }

    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn post_clamp_collapsed_overrange_peaks_remain_visible_inside_their_channel() {
        for scale in [1.0, 2.0] {
            let geometry = prepare(300, bounds(120.0), scale, |_, _| {
                Some([
                    CachedPeak { min: 2.0, max: 3.0 },
                    CachedPeak {
                        min: -3.0,
                        max: -2.0,
                    },
                ])
            })
            .unwrap();
            for channel in 0..2 {
                assert!(geometry.display.channels()[channel].has_zero_range_trace());
                assert_eq!(geometry.zero_range_spans[channel].len(), geometry.columns);
                let band_top = 23.5 + channel as f32 * 50.0;
                let expected_y = if channel == 0 {
                    band_top + 1.0 / scale
                } else {
                    band_top + 50.0 - 1.0 / scale
                };
                for (left, right) in &geometry.zero_range_spans[channel] {
                    assert!((f32::from(left.y) - expected_y).abs() < 0.001);
                    assert_eq!(left.y, right.y);
                    assert!(f32::from(left.y) - 0.5 / scale >= band_top);
                    assert!(f32::from(left.y) + 0.5 / scale <= band_top + 50.0);
                }
            }
        }
    }

    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn cached_paths_reuse_only_exact_source_geometry_and_device_scale() {
        let mut cache = Cache::default();
        let source = source();
        let first = cache.geometry(&source, bounds(700.0), 1.0).unwrap();
        assert!(Rc::ptr_eq(
            &first,
            &cache.geometry(&source, bounds(700.0), 1.0).unwrap()
        ));
        let retina = cache.geometry(&source, bounds(700.0), 2.0).unwrap();
        assert!(!Rc::ptr_eq(&first, &retina));
        assert!(!Rc::ptr_eq(
            &retina,
            &cache.geometry(&source, bounds(701.0), 2.0).unwrap()
        ));
        assert_eq!(cache.entries.len(), 2);
        let replacement = self::source();
        assert!(!Rc::ptr_eq(
            &retina,
            &cache.geometry(&replacement, bounds(700.0), 2.0).unwrap()
        ));
    }

    #[cfg(feature = "ui-test-support")]
    #[test]
    fn sampler_geometry_does_not_retain_overview_custody_and_reuses_verified_content_only() {
        use hex_project::{Command, Created, Editor, Project};
        use hex_session::source_overview_test_support::Fixture;
        let mut editor = Editor::new(Project::new(8000).unwrap(), 1).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "original.wav".into(),
                sample_rate: 8000,
                frames: 3,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        let facts = OriginalSourceFacts::new(1, [7; 32]).unwrap();
        let context = hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 2,
        };
        let mut fixture = Fixture::new(asset, &[[0.1, -0.2]; 3], 8000, facts, None).unwrap();
        let source = Source::Sampler {
            table: fixture.view(context),
            asset,
        };
        let mut cache = Cache::default();
        let first = cache.geometry(&source, bounds(700.0), 2.0).unwrap();
        drop(source);
        fixture.drop_table();
        fixture.drop_overview();
        assert_eq!(
            fixture.usage().bin_capacity_bytes,
            0,
            "cached paths must not keep Session's peaks alive after New/Open"
        );
        let reopened = Fixture::new(asset, &[[0.1, -0.2]; 3], 8000, facts, None).unwrap();
        let source = Source::Sampler {
            table: reopened.view(hex_engine::AudioSourceContext {
                project_generation: 3,
                editor_revision: 0,
            }),
            asset,
        };
        assert!(
            Rc::ptr_eq(
                &first,
                &cache.geometry(&source, bounds(700.0), 2.0).unwrap()
            ),
            "same verified content is independent of document generation"
        );
        let replaced = Fixture::new(
            asset,
            &[[0.2, -0.1]; 3],
            8000,
            OriginalSourceFacts::new(1, [8; 32]).unwrap(),
            None,
        )
        .unwrap();
        let source = Source::Sampler {
            table: replaced.view(context),
            asset,
        };
        assert!(!Rc::ptr_eq(
            &first,
            &cache.geometry(&source, bounds(700.0), 2.0).unwrap()
        ));
        let rate = Fixture::new(asset, &[[0.1, -0.2]; 3], 16000, facts, None).unwrap();
        let source = Source::Sampler {
            table: rate.view(context),
            asset,
        };
        assert!(!Rc::ptr_eq(
            &first,
            &cache.geometry(&source, bounds(700.0), 2.0).unwrap()
        ));
    }

    #[test]
    #[ignore = "manual release measurement of cold geometry and warm cache hits"]
    fn overview_geometry_preparation_profile() {
        use std::{hint::black_box, time::Instant};
        let sources = vec![("browser", source())];
        #[cfg(feature = "ui-test-support")]
        let sources = {
            use hex_project::{Command, Created, Editor, Project};
            use hex_session::source_overview_test_support::Fixture;
            let mut sources = sources;
            let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
            let Created::Asset(asset) = editor
                .execute(Command::AddAsset {
                    path: "original.wav".into(),
                    sample_rate: 48000,
                    frames: 8195,
                })
                .unwrap()
            else {
                panic!("asset");
            };
            let frames = (0..8195)
                .map(|index| {
                    let amplitude = (index % 37) as f32 / 37.0;
                    [amplitude, -amplitude]
                })
                .collect::<Vec<_>>();
            let fixture = Fixture::new(
                asset,
                &frames,
                48000,
                OriginalSourceFacts::new(1, [7; 32]).unwrap(),
                None,
            )
            .unwrap();
            sources.push((
                "sampler",
                Source::Sampler {
                    table: fixture.view(hex_engine::AudioSourceContext {
                        project_generation: 1,
                        editor_revision: 1,
                    }),
                    asset,
                },
            ));
            sources
        };
        let extent = bounds(1200.0);
        let rounds = 100;
        for (name, source) in sources {
            let (_, frames) = source.identity().unwrap();
            let mut cache = Cache::default();
            let geometry = cache.geometry(&source, extent, 2.0).unwrap();
            let started = Instant::now();
            for _ in 0..rounds {
                black_box(
                    prepare(frames, extent, 2.0, |start, end| source.peaks(start, end)).unwrap(),
                );
            }
            let cold = started.elapsed();
            let started = Instant::now();
            for _ in 0..rounds {
                let hit = cache.geometry(&source, extent, 2.0).unwrap();
                assert!(Rc::ptr_eq(&hit, &geometry));
                black_box(hit);
            }
            eprintln!(
                "peak-only {name}: {frames} frames/4096 stereo leaves, 1200 logical px, scale2, {} columns, {rounds} rounds, cold={cold:?}, warm={:?}; UI geometry only, no native deadline claim",
                geometry.columns,
                started.elapsed()
            );
        }
    }
}
