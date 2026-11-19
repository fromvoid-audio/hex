//! Eligible original-resident-source waveform projection. The engine owns every
//! source/fade query; this module owns bounded visible extrema and comparison
//! identity only. It neither edits projects nor decodes/prepares PCM.
use hex_engine::audio_transform::{
    FrameFadeEvent, FrameFadeOverrides, FrameFadePreview, FrameFadeRange, FrameFadeSample,
};
use hex_project::{Asset, AudioClipEdit, AudioEvents, Clip, ClipFade, ClipGain, TempoMap};
use hex_session::{CachedOverrange, CachedPeak, CachedWaveform};
use std::{ops::Range, sync::Arc};

pub(super) const MAX_DETAIL_FRAMES: usize = 4_194_304;
pub(super) const MAX_RANGE_WORK: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Coordinates {
    Timeline,
    /// Retained event source-root frames, before asset offset/phase/looping.
    Event(u64),
}

/// Complete effective geometry identity. Normalize the metadata-only frame
/// setter law so old canonical + held override equals new canonical + no override.
/// This value is comparison-only: playback/query math stays with the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) clip_id: Option<u64>,
    pub(super) start: u64,
    duration: u64,
    asset: hex_project::AssetId,
    source_frames: u64,
    conversion: u32,
    source_rate: u32,
    root: AudioClipEdit,
    tempo: Arc<TempoMap>,
    gain: ClipGain,
    coordinates: Coordinates,
}
impl Key {
    pub(super) fn timeline_origin(&self) -> u64 {
        match self.coordinates {
            Coordinates::Timeline => self.start,
            Coordinates::Event(_) => 0,
        }
    }
}

#[derive(Clone)]
pub(super) struct Projection {
    clip: Clip,
    asset: Asset,
    tempo: Arc<TempoMap>,
    fade: Option<ClipFade>,
    events: Arc<AudioEvents>,
    gain: ClipGain,
    coordinates: Coordinates,
    key: Key,
}
impl Projection {
    #[allow(clippy::too_many_arguments)] // Existing clip/source plus three typed display overrides and clock.
    pub(super) fn new(
        clip: &Clip,
        asset: &Asset,
        tempo: &TempoMap,
        waveform: &CachedWaveform,
        fade: Option<ClipFade>,
        events: Option<&AudioEvents>,
        gain: Option<ClipGain>,
        coordinates: Coordinates,
    ) -> Option<Self> {
        if waveform.source_frames() as u64 != asset.playback_frames()
            || waveform.bounded_samples(0..1, 1).is_none()
        {
            return None;
        }
        let events = Arc::new(match events {
            Some(events) => events.clone(),
            None => clip.editable_audio_events().ok()?,
        });
        let gain = gain.unwrap_or_else(|| clip.gain());
        let query = FrameFadePreview::with_preview(
            clip,
            tempo,
            asset,
            FrameFadeOverrides {
                fade,
                events: Some(&events),
                gain: Some(gain),
            },
        )
        .ok()??;
        if let Coordinates::Event(id) = coordinates {
            query.event(id)?;
        }
        let mut root = clip.audio_event_root();
        let domain = Arc::make_mut(&mut root.domain);
        domain.events = Some(Arc::clone(&events));
        if let Some(fade) = fade {
            domain.fade = fade;
            domain.fade_start = root.view_start;
            domain.fade_projection = None;
        }
        let tempo = Arc::new(tempo.clone());
        let key = Key {
            clip_id: Some(clip.id().value()),
            start: clip.start(),
            duration: clip.duration(),
            asset: asset.id(),
            source_frames: asset.playback_frames(),
            conversion: asset.conversion_version(),
            source_rate: asset.source_sample_rate(),
            root,
            tempo: Arc::clone(&tempo),
            gain,
            coordinates,
        };
        Some(Self {
            clip: clip.clone(),
            asset: asset.clone(),
            tempo,
            fade,
            events,
            gain,
            coordinates,
            key,
        })
    }
    pub(super) fn key(&self) -> Key {
        self.key.clone()
    }
    pub(super) fn query(&self) -> Option<FrameFadePreview<'_>> {
        FrameFadePreview::with_preview(
            &self.clip,
            &self.tempo,
            &self.asset,
            FrameFadeOverrides {
                fade: self.fade,
                events: Some(&self.events),
                gain: Some(self.gain),
            },
        )
        .ok()?
    }
    pub(super) fn sampler<'p, 'q>(
        &self,
        query: &'q FrameFadePreview<'p>,
        waveform: &'p CachedWaveform,
    ) -> Option<Sampler<'p, 'q>> {
        let clock = match self.coordinates {
            Coordinates::Timeline => Clock::Timeline(query),
            Coordinates::Event(id) => Clock::Event(query.event(id)?),
        };
        Some(Sampler {
            clock,
            waveform,
            detail_remaining: MAX_DETAIL_FRAMES,
            range_remaining: MAX_RANGE_WORK,
        })
    }
}

enum Clock<'p, 'q> {
    Timeline(&'q FrameFadePreview<'p>),
    Event(FrameFadeEvent<'q, 'p>),
}
impl Clock<'_, '_> {
    fn sample(&self, frame: u64) -> Option<Option<FrameFadeSample>> {
        match self {
            Self::Timeline(query) => query.timeline_sample(frame),
            Self::Event(query) => query.sample(frame),
        }
        .ok()
    }
    fn visit(
        &self,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Option<hex_engine::audio_transform::FrameFadeCoverage> {
        match self {
            Self::Timeline(query) => query.visit_timeline_ranges(range, remaining, visit),
            Self::Event(query) => query.visit_ranges(range, remaining, visit),
        }
        .ok()?
    }
}

pub(super) struct Sampler<'p, 'q> {
    clock: Clock<'p, 'q>,
    waveform: &'p CachedWaveform,
    detail_remaining: usize,
    range_remaining: usize,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Column {
    pub(super) peaks: [CachedPeak; 2],
    pub(super) overrange: CachedOverrange,
}
impl Sampler<'_, '_> {
    /// Exact detail applies every ordered gain before extrema and overrange.
    /// A logical frame, including source/placement silence, consumes one unit.
    pub(super) fn exact(&mut self, range: Range<u64>, limit: usize) -> Option<Column> {
        let length = usize::try_from(range.end.checked_sub(range.start)?).ok()?;
        if length == 0 || length > limit || length > self.detail_remaining {
            return None;
        }
        self.detail_remaining -= length;
        let mut peaks: Option<[CachedPeak; 2]> = None;
        let mut overrange = CachedOverrange::default();
        for frame in range {
            let value = if let Some(sample) = self.clock.sample(frame)? {
                let source = usize::try_from(sample.source_frame()).ok()?;
                sample.apply_to(
                    *self
                        .waveform
                        .bounded_samples(source..source.checked_add(1)?, 1)?
                        .first()?,
                )
            } else {
                [0.0; 2]
            };
            if value.iter().any(|value| !value.is_finite()) {
                return None;
            }
            let next = value.map(|value| CachedPeak {
                min: value,
                max: value,
            });
            merge(&mut peaks, next);
            for channel in 0..2 {
                overrange.positive[channel] |= value[channel] > 1.0;
                overrange.negative[channel] |= value[channel] < -1.0;
            }
        }
        Some(Column {
            peaks: peaks?,
            overrange,
        })
    }
    /// Wide bins keep the documented cached-extrema/midpoint-gain approximation.
    /// A shared visitor budget refuses the whole geometry, never partial events.
    pub(super) fn peaks(
        &mut self,
        range: Range<u64>,
        detail_limit: usize,
    ) -> Option<[CachedPeak; 2]> {
        let length = usize::try_from(range.end.checked_sub(range.start)?).ok()?;
        if length <= detail_limit && length <= self.detail_remaining {
            return self.exact(range, detail_limit).map(|column| column.peaks);
        }
        let mut peaks = None;
        let mut valid = true;
        let waveform = self.waveform;
        let coverage = self.clock.visit(range, &mut self.range_remaining, |part| {
            for source in part.source.into_iter().flatten() {
                let next = usize::try_from(source.start)
                    .ok()
                    .zip(usize::try_from(source.end).ok())
                    .and_then(|(start, end)| waveform.peaks_for_frames(start, end));
                let Some(next) = next else {
                    valid = false;
                    continue;
                };
                let minima = part.sample.apply_to([next[0].min, next[1].min]);
                let maxima = part.sample.apply_to([next[0].max, next[1].max]);
                if minima.iter().chain(&maxima).any(|value| !value.is_finite()) {
                    valid = false;
                    continue;
                }
                merge(
                    &mut peaks,
                    std::array::from_fn(|channel| CachedPeak {
                        min: minima[channel],
                        max: maxima[channel],
                    }),
                );
            }
        })?;
        if !valid {
            return None;
        }
        if coverage.has_silence {
            merge(&mut peaks, [CachedPeak { min: 0.0, max: 0.0 }; 2]);
        }
        peaks
    }
}
fn merge(peaks: &mut Option<[CachedPeak; 2]>, next: [CachedPeak; 2]) {
    if let Some(peaks) = peaks {
        for channel in 0..2 {
            peaks[channel].min = peaks[channel].min.min(next[channel].min);
            peaks[channel].max = peaks[channel].max.max(next[channel].max);
        }
    } else {
        *peaks = Some(next);
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests;
