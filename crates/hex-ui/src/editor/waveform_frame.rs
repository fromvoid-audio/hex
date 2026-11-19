//! Original resident-source amplitude for eligible clips and events.
//! Geometry and the overrange worker share the engine's source clock and
//! multiplication order. Wide columns use midpoint-weighted cached peaks.
use super::waveform_source::aggregate_peaks;
use hex_engine::{
    Invalid,
    audio_transform::{
        FrameFadeCoverage, FrameFadeEvent, FrameFadeOverrides, FrameFadePreview, FrameFadeRange,
        FrameFadeSample,
    },
};
use hex_project::{
    Asset, AudioClipEdit, AudioClipFadeClock, AudioEvent, AudioEvents, Clip, ClipFade, ClipGain,
    Project, Tempo, TempoMap,
};
use hex_session::{CachedOverrange, CachedPeak, CachedWaveform};
use std::{ops::Range, sync::Arc};

pub(super) const RANGE_BUDGET: usize = 16_384;
const ZERO: [CachedPeak; 2] = [CachedPeak { min: 0.0, max: 0.0 }; 2];

/// Effective waveform law, independent of the preview or receipt that owns it.
/// The geometry store separately retains the exact original waveform Arc.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) start: u64,
    duration: u64,
    root: AudioClipEdit,
    gain: ClipGain,
}

pub(super) struct Projection {
    clip: Clip,
    asset: Asset,
    tempo: TempoMap,
    fade: Option<ClipFade>,
    legacy_root_fade: bool,
    events: Option<Arc<AudioEvents>>,
    gain: Option<ClipGain>,
    pub(super) key: Arc<Key>,
}

impl Projection {
    pub(super) fn new(
        project: &Project,
        clip: &Clip,
        waveform: &CachedWaveform,
        fade: Option<ClipFade>,
        events: Option<AudioEvents>,
        gain: Option<ClipGain>,
    ) -> Option<Self> {
        let asset = project.asset(clip.asset())?;
        if u64::try_from(waveform.source_frames()).ok()? != asset.playback_frames()
            || waveform.bounded_samples(0..1, 1).is_none()
        {
            return None;
        }
        // Only frame edits reach this projection. The project rate matters
        // for validation, but tempo points do not participate in its source law.
        let tempo = TempoMap::constant(project.sample_rate(), Tempo::DEFAULT).ok()?;
        let legacy_root_fade = fade.is_some() && !project.arrangement_fade_edit_uses_outer(clip);
        let events = events.map(Arc::new);
        let mut root = clip.audio_event_root();
        if let Some(fade) = fade {
            let start = root.view_start;
            let domain = Arc::make_mut(&mut root.domain);
            domain.fade = fade;
            domain.fade_projection = None;
            if legacy_root_fade {
                domain.fade_start = start;
            } else {
                domain.fade_clock = AudioClipFadeClock::Outer;
                domain.fade_start = 0;
            }
        }
        if let Some(events) = &events {
            Arc::make_mut(&mut root.domain).events = Some(Arc::clone(events));
        }
        let projection = Self {
            clip: clip.clone(),
            asset: asset.clone(),
            tempo,
            fade,
            legacy_root_fade,
            events,
            gain,
            key: Arc::new(Key {
                start: clip.start(),
                duration: clip.duration(),
                root,
                gain: gain.unwrap_or_else(|| clip.gain()),
            }),
        };
        projection.query()?;
        Some(projection)
    }

    pub(super) fn query(&self) -> Option<FrameFadePreview<'_>> {
        FrameFadePreview::with_preview(
            &self.clip,
            &self.tempo,
            &self.asset,
            FrameFadeOverrides {
                fade: self.fade,
                legacy_root_fade: self.legacy_root_fade,
                events: self.events.as_deref(),
                gain: self.gain,
            },
        )
        .ok()?
    }
}

/// Coordinates supplied by the containing waveform. Event windows use the
/// retained source-root clock, including the event's source start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Clock {
    Arrangement,
    Event(u64),
}

impl Clock {
    pub(super) fn resolve<'q, 'source>(
        self,
        query: &'q FrameFadePreview<'source>,
    ) -> Option<Read<'q, 'source>> {
        match self {
            Self::Arrangement => Some(Read::Arrangement(query)),
            Self::Event(id) => Some(Read::Event(query.event(id)?)),
        }
    }
}

pub(super) enum Read<'q, 'source> {
    Arrangement(&'q FrameFadePreview<'source>),
    Event(FrameFadeEvent<'q, 'source>),
}

impl Read<'_, '_> {
    fn sample(&self, frame: u64) -> Result<Option<FrameFadeSample>, Invalid> {
        match self {
            Self::Arrangement(query) => query.timeline_sample(frame),
            Self::Event(event) => event.sample(frame),
        }
    }

    fn visit_ranges(
        &self,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        match self {
            Self::Arrangement(query) => query.visit_timeline_ranges(range, remaining, visit),
            Self::Event(event) => event.visit_ranges(range, remaining, visit),
        }
    }
}

/// Translate a clipped event's destination window into its retained read clock.
pub(super) fn event_source_window(event: AudioEvent, visible: Range<u64>) -> Option<Range<u64>> {
    if visible.start >= visible.end || visible.end > event.end {
        return None;
    }
    let first = event
        .source_start
        .checked_add(visible.start.checked_sub(event.start)?)?;
    let last = first.checked_add(visible.end - visible.start)?;
    Some(first..last)
}

pub(super) fn detail(
    query: &Read<'_, '_>,
    waveform: &CachedWaveform,
    range: Range<u64>,
    limit: usize,
) -> Option<[CachedPeak; 2]> {
    let frames = usize::try_from(range.end.checked_sub(range.start)?).ok()?;
    if frames == 0 || frames > limit {
        return None;
    }
    let mut peaks = None;
    for frame in range {
        let sample = match query.sample(frame).ok()? {
            Some(sample) => {
                let source = usize::try_from(sample.source_frame()).ok()?;
                let pcm = waveform.bounded_samples(source..source.checked_add(1)?, 1)?;
                sample.apply_to(*pcm.first()?)
            }
            None => [0.0; 2],
        };
        merge(
            &mut peaks,
            sample.map(|value| CachedPeak {
                min: value,
                max: value,
            }),
        );
    }
    peaks
}

/// A capped approximation for wide columns. Silence contributes real zero;
/// unavailable data or budget exhaustion defers to the caller's fallback.
pub(super) fn coarse(
    query: &Read<'_, '_>,
    waveform: &CachedWaveform,
    range: Range<u64>,
    remaining: &mut usize,
) -> Option<[CachedPeak; 2]> {
    let mut peaks = None;
    let mut available = true;
    let coverage = query
        .visit_ranges(range, remaining, |part| {
            for range in part.source.into_iter().flatten() {
                let Some(mut pair) = usize::try_from(range.start).ok().and_then(|start| {
                    aggregate_peaks(waveform, start, usize::try_from(range.end).ok()?)
                }) else {
                    available = false;
                    continue;
                };
                for gain in part.sample.gains() {
                    if gain != 1.0 {
                        for peak in &mut pair {
                            peak.min *= gain;
                            peak.max *= gain;
                        }
                    }
                }
                merge(&mut peaks, pair);
            }
        })
        .ok()??;
    if !available {
        return None;
    }
    if coverage.has_silence {
        merge(&mut peaks, ZERO);
    }
    peaks
}

pub(super) fn overrange(peaks: [CachedPeak; 2]) -> CachedOverrange {
    CachedOverrange {
        positive: peaks.map(|peak| peak.max > 1.0),
        negative: peaks.map(|peak| peak.min < -1.0),
    }
}

fn merge(current: &mut Option<[CachedPeak; 2]>, next: [CachedPeak; 2]) {
    let Some(peaks) = current.as_mut() else {
        *current = Some(next);
        return;
    };
    for (peak, next) in peaks.iter_mut().zip(next) {
        peak.min = peak.min.min(next.min);
        peak.max = peak.max.max(next.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{AudioEventEdit, AudioEventFade, Command, Created, Editor};

    #[test]
    fn panned_event_uses_its_slipped_source_and_ordered_fade_gain() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Imported { clip, .. } = model
            .execute(Command::ImportAudioTrack {
                name: "slipped".into(),
                path: "slipped.wav".into(),
                sample_rate: 48_000,
                frames: 24,
            })
            .unwrap()
        else {
            panic!("import a frame clip");
        };
        let audio = hex_engine::Audio::new(
            48_000,
            (0..24)
                .map(|frame| [frame as f32, -(frame as f32)])
                .collect(),
        )
        .unwrap();
        let waveform = hex_media::waveform::summarize(&audio, 24).unwrap();
        let project = model.project();
        let clip = project.clip(clip).unwrap();
        let mut events = clip.editable_audio_events().unwrap();
        let fade = AudioEventFade::new(4, ClipFade::new(8, 0, 4, 0).unwrap()).unwrap();
        let gain = ClipGain::new(-6_000).unwrap();
        let event = AudioEvent {
            start: 9,
            end: 17,
            source_start: 4,
            fade: Some(fade),
            gain,
            ..events.events[0]
        };
        assert!(event_source_window(event, 8..12).is_none());
        assert!(event_source_window(event, 11..18).is_none());
        let visible = event_source_window(event, 11..15).unwrap();
        assert_eq!(visible, 6..10);
        events.events[0] = event;
        let projection =
            Projection::new(project, clip, &waveform, None, Some(events), None).unwrap();
        let query = projection.query().unwrap();
        let event_read = Clock::Event(event.id).resolve(&query).unwrap();
        let arrangement = Clock::Arrangement.resolve(&query).unwrap();
        let exact = detail(&event_read, &waveform, 6..7, 1).unwrap();
        assert_eq!(exact, detail(&arrangement, &waveform, 11..12, 1).unwrap());
        assert_eq!(
            exact[0].max,
            (6.0 * fade.shape().gain_at(2)) * gain.linear()
        );
        assert_eq!(exact[1].min, -exact[0].max);
        let mut remaining = RANGE_BUDGET;
        let coarse = coarse(&event_read, &waveform, visible, &mut remaining).unwrap();
        assert_eq!(coarse[0].max, 9.0 * gain.linear());
        assert!(remaining < RANGE_BUDGET);
        assert!(Clock::Event(event.id + 1).resolve(&query).is_none());
    }

    #[test]
    fn frame_fades_reveal_original_samples_and_keep_effective_identity_at_commit() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Imported { clip, .. } = model
            .execute(Command::ImportAudioTrack {
                name: "original".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 24,
            })
            .unwrap()
        else {
            panic!("import a frame clip");
        };
        let audio = hex_engine::Audio::new(48_000, vec![[2.0, -2.0]; 24]).unwrap();
        let waveform = hex_media::waveform::summarize(&audio, 1).unwrap();
        let projection = |model: &Editor, fade, events| {
            let project = model.project();
            Projection::new(
                project,
                project.clip(clip).unwrap(),
                &waveform,
                fade,
                events,
                None,
            )
            .unwrap()
        };
        let peaks = |projection: &Projection, range| {
            detail(
                &Read::Arrangement(&projection.query().unwrap()),
                &waveform,
                range,
                8,
            )
            .unwrap()
        };
        let fade = ClipFade::new(24, 0, 16, 0).unwrap();
        let plain = projection(&model, None, None);
        let held = projection(&model, Some(fade), None);
        assert_eq!(peaks(&held, 0..1), ZERO);
        assert_eq!(peaks(&plain, 0..1)[0].max, 2.0);
        model
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        assert_eq!(projection(&model, None, None).key, held.key);

        let current = model.project().clip(clip).unwrap();
        let op = AudioEventEdit::SetFade {
            event: 1,
            fade: Some(AudioEventFade::new(0, ClipFade::new(24, 0, 8, 0).unwrap()).unwrap()),
        };
        let events = current
            .editable_audio_events()
            .unwrap()
            .edited(op.clone(), 24)
            .unwrap();
        let held_event = projection(&model, None, Some(events));
        assert!(peaks(&held_event, 2..3)[0].max < peaks(&held, 2..3)[0].max);
        model
            .execute(Command::EditAudioEvents {
                clip,
                operation: op,
            })
            .unwrap();
        assert_eq!(held_event.key, projection(&model, None, None).key);

        let shortened = ClipFade::new(24, 0, 2, 0).unwrap();
        let held_outer = projection(&model, Some(shortened), None);
        let revealed = peaks(&held_outer, 8..9);
        assert_eq!(overrange(revealed).positive, [true, false]);
        assert_eq!(overrange(revealed).negative, [false, true]);
        assert!(peaks(&held_event, 8..9)[0].max < revealed[0].max);
        model
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: shortened,
            })
            .unwrap();
        let installed = projection(&model, None, None);
        assert_eq!(installed.key, held_outer.key);
        assert_eq!(peaks(&installed, 8..9), revealed);
        let mut remaining = RANGE_BUDGET;
        assert_eq!(
            coarse(
                &Read::Arrangement(&installed.query().unwrap()),
                &waveform,
                12..24,
                &mut remaining
            )
            .unwrap()[0]
                .max,
            2.0
        );
        assert!(
            coarse(
                &Read::Arrangement(&installed.query().unwrap()),
                &waveform,
                0..24,
                &mut 0
            )
            .is_none()
        );
    }
}
