//! Original resident-source amplitude for eligible Arrangement frame clips.
//! Geometry and the overrange worker share the engine's source clock and
//! multiplication order. Wide columns use midpoint-weighted cached peaks.
use super::waveform_source::aggregate_peaks;
use hex_engine::audio_transform::{FrameFadeOverrides, FrameFadePreview};
use hex_project::{
    Asset, AudioClipEdit, AudioClipFadeClock, AudioEvents, Clip, ClipFade, ClipGain, Project,
    Tempo, TempoMap,
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

pub(super) fn detail(
    query: &FrameFadePreview<'_>,
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
        let sample = match query.timeline_sample(frame).ok()? {
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
    query: &FrameFadePreview<'_>,
    waveform: &CachedWaveform,
    range: Range<u64>,
    remaining: &mut usize,
) -> Option<[CachedPeak; 2]> {
    let mut peaks = None;
    let mut available = true;
    let coverage = query
        .visit_timeline_ranges(range, remaining, |part| {
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
            detail(&projection.query().unwrap(), &waveform, range, 8).unwrap()
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
                &installed.query().unwrap(),
                &waveform,
                12..24,
                &mut remaining
            )
            .unwrap()[0]
                .max,
            2.0
        );
        assert!(coarse(&installed.query().unwrap(), &waveform, 0..24, &mut 0).is_none());
    }
}
