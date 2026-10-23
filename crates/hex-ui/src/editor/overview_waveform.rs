//! Full-source, peak-only waveform display shared by sampler and browser.
//! The two-entry UI cache holds bounded device-pixel silhouettes, never PCM.
//! Sampler cache identity uses verified original content facts, not a bank or a
//! retained overview table; New/Open can therefore release its charged backing.
use super::{theme, waveform::waveform_column_count};
use gpui::{
    Bounds, ContentMask, Div, Path, PathBuilder, Pixels, canvas, div, point, prelude::*, px, rgb,
    size,
};
use hex_project::{AssetId, OriginalSourceFacts};
use hex_session::{CachedPeak, CachedWaveform, OriginalSourceOverviews};
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
                u64::try_from(waveform.source_frames).ok()?,
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
    paths: [Path<Pixels>; 2],
    #[cfg(test)]
    columns: usize,
    #[cfg(test)]
    outlines: [Vec<gpui::Point<Pixels>>; 2],
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
    let mut paths = Vec::with_capacity(2);
    #[cfg(test)]
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
        let mut builder = if area {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(px(1.0 / scale))
        };
        builder.move_to(*upper.first()?);
        for p in upper.iter().skip(1) {
            builder.line_to(*p);
        }
        if area {
            for p in lower.iter().rev() {
                builder.line_to(*p);
            }
            builder.close();
        }
        paths.push(builder.build().ok()?);
        #[cfg(test)]
        outlines.push(upper.into_iter().chain(lower).collect());
    }
    Some(Geometry {
        paths: paths.try_into().ok()?,
        #[cfg(test)]
        columns,
        #[cfg(test)]
        outlines: outlines.try_into().ok()?,
    })
}

pub(super) fn render(source: Source, cache: Rc<RefCell<Cache>>, active: Option<(f64, f64)>) -> Div {
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let Some(geometry) = cache
                    .try_borrow_mut()
                    .ok()
                    .and_then(|mut cache| cache.geometry(&source, bounds, window.scale_factor()))
                else {
                    return;
                };
                let width = f32::from(bounds.size.width);
                let (start, end) = active.unwrap_or((0.0, 0.0));
                for (left, right, role) in [
                    (0.0, start, theme::PaletteRole::Muted),
                    (start, end, theme::PaletteRole::Waveform),
                    (end, 1.0, theme::PaletteRole::Muted),
                ] {
                    if right <= left {
                        continue;
                    }
                    let mask = Bounds::new(
                        bounds.origin + point(px(left as f32 * width), px(0.0)),
                        size(px((right - left) as f32 * width), bounds.size.height),
                    );
                    window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                        for path in &geometry.paths {
                            window.paint_path(path.clone(), rgb(theme::color(role)));
                        }
                    });
                }
            },
        )
        .absolute()
        .inset_0(),
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
