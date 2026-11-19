//! Arrangement destination envelope over accepted prepared waveforms.
//! Frame playback attaches the Outer fade after source processing. Reuse that
//! same processed signal, including repeats and sparse spans, without preparing
//! or retaining another buffer. The musical bake exposes pre-fade peak bins
//! so the same projected clock drives held and accepted paint.
use super::waveform_source::aggregate_peaks;
use hex_project::{
    AudioClipFadeClock, AudioDomainClock, Clip, ClipFade, ClipGain, Project,
    ProjectedOuterAudioFade, TempoMap,
};
use hex_session::{CachedOverrange, CachedPeak, CachedWaveform, service::AudioClipWaveform};
use std::{ops::Range, sync::Arc};

const ZERO: [CachedPeak; 2] = [CachedPeak { min: 0.0, max: 0.0 }; 2];

/// A peak-only display law. A display column weights the cached extrema at
/// its clipped midpoint and carries no sample-level clipping evidence.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PreparedMusicalFade {
    clip: Clip,
    tempo: Arc<TempoMap>,
    held: Option<ClipFade>,
    gain: ClipGain,
}

impl PreparedMusicalFade {
    pub(super) fn for_clip(
        project: &Project,
        clip: &Clip,
        prepared: &AudioClipWaveform,
        held: Option<ClipFade>,
        gain: ClipGain,
    ) -> Option<(Arc<CachedWaveform>, Arc<Self>)> {
        let source = prepared.neutral_musical.as_ref()?;
        if project.clip(clip.id())? != clip
            || prepared.length != clip.duration()
            || (held.is_some() && !project.arrangement_fade_edit_uses_outer(clip))
        {
            return None;
        }
        let display = Self {
            clip: clip.clone(),
            tempo: Arc::clone(&source.tempo),
            held,
            gain,
        };
        display.projection()?;
        Some((Arc::clone(&source.waveform), Arc::new(display)))
    }

    pub(super) fn start(&self) -> u64 {
        self.clip.start()
    }

    pub(super) fn projection(&self) -> Option<ProjectedOuterAudioFade<'_>> {
        if let Some(fade) = self.held {
            return self
                .clip
                .projected_outer_fade_preview(&self.tempo, fade)
                .ok();
        }
        if let Some(fade) = self.clip.projected_outer_fade(&self.tempo).ok()? {
            return Some(fade);
        }
        let neutral = ClipFade::neutral(self.clip.duration()).ok()?;
        self.clip
            .projected_outer_fade_preview(&self.tempo, neutral)
            .ok()
    }

    pub(super) fn peaks(
        &self,
        projection: &ProjectedOuterAudioFade<'_>,
        waveform: &CachedWaveform,
        visible: Range<u64>,
    ) -> Option<[CachedPeak; 2]> {
        let origin = self.start();
        let start = visible.start.max(origin);
        let end = visible.end.min(origin.checked_add(self.clip.duration())?);
        if start >= end {
            return Some(ZERO);
        }
        let gain = projection.gain_at(start + (end - start - 1) / 2).ok()?;
        let peaks = aggregate_peaks(
            waveform,
            usize::try_from(start - origin).ok()?,
            usize::try_from(end - origin).ok()?,
        )?;
        let mut values = (start != visible.start || end != visible.end).then_some(ZERO);
        super::waveform_frame::merge(&mut values, scale(peaks, gain, self.gain.linear()));
        values
    }
}

/// Effective rendered shape shared by held, pending and canonical paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PreparedFade {
    pub(super) timeline_origin: u64,
    pub(super) root_offset: u64,
    end: u64,
    fade: ClipFade,
    gain: ClipGain,
}

impl PreparedFade {
    pub(super) fn for_clip(
        project: &Project,
        clip: &Clip,
        prepared: &AudioClipWaveform,
        held: Option<ClipFade>,
        gain: ClipGain,
    ) -> Option<Self> {
        let accepted = project.clip(clip.id())?;
        let root = &accepted.audio_edit()?.domain;
        if prepared.length != accepted.duration()
            || !matches!(root.clock, AudioDomainClock::Frames(_))
            || (root.fade_clock == AudioClipFadeClock::LegacyRoot && !root.fade.is_neutral())
        {
            return None;
        }
        let timeline_origin = clip.start();
        let end = timeline_origin.checked_add(clip.duration())?;
        // A visible edge trim can translate within the retained waveform.
        // The original summary's offset belongs to the accepted placement.
        let root_offset = u64::try_from(
            i128::from(prepared.offset) + i128::from(timeline_origin)
                - i128::from(accepted.start()),
        )
        .ok()?;
        if root_offset.checked_add(clip.duration())?
            > u64::try_from(prepared.waveform.source_frames()).ok()?
        {
            return None;
        }
        let at = usize::try_from(root_offset).ok()?;
        prepared
            .waveform
            .bounded_exact_peaks_for_frames(at, at.checked_add(1)?, 1)?;
        let tempo = project.tempo_map();
        let fade = match held {
            Some(fade) if project.arrangement_fade_edit_uses_outer(clip) => clip
                .projected_outer_fade_preview(tempo, fade)
                .ok()?
                .frame_playback_envelope()
                .ok()?,
            Some(_) => return None,
            None => match clip.projected_outer_fade(tempo).ok()? {
                Some(fade) => fade.frame_playback_envelope().ok()?,
                None => ClipFade::neutral(clip.duration()).ok()?,
            },
        };
        Some(Self {
            timeline_origin,
            root_offset,
            end,
            fade,
            gain,
        })
    }

    fn active(&self, visible: Range<u64>) -> Option<Range<u64>> {
        if visible.start >= visible.end {
            return None;
        }
        Some(
            visible.start.clamp(self.timeline_origin, self.end) - self.timeline_origin
                ..visible.end.clamp(self.timeline_origin, self.end) - self.timeline_origin,
        )
    }

    fn source(&self, active: &Range<u64>) -> Option<Range<usize>> {
        Some(
            usize::try_from(self.root_offset.checked_add(active.start)?).ok()?
                ..usize::try_from(self.root_offset.checked_add(active.end)?).ok()?,
        )
    }

    pub(super) fn peaks(
        &self,
        waveform: &CachedWaveform,
        visible: Range<u64>,
        detail_limit: usize,
    ) -> Option<[CachedPeak; 2]> {
        let active = self.active(visible.clone())?;
        if active.is_empty() {
            return Some(ZERO);
        }
        let source = self.source(&active)?;
        let mut values = (active.end - active.start < visible.end - visible.start).then_some(ZERO);
        let gain = self.gain.linear();
        if visible.end - visible.start <= detail_limit as u64 {
            for (frame, position) in source.zip(active) {
                let peak =
                    waveform.bounded_exact_peaks_for_frames(frame, frame.checked_add(1)?, 1)?;
                super::waveform_frame::merge(
                    &mut values,
                    scale(peak, self.fade.gain_at(position), gain),
                );
            }
        } else {
            let midpoint = active.start + (active.end - active.start - 1) / 2;
            let peak = aggregate_peaks(waveform, source.start, source.end)?;
            super::waveform_frame::merge(
                &mut values,
                scale(peak, self.fade.gain_at(midpoint), gain),
            );
        }
        values
    }

    pub(super) fn overrange(
        &self,
        waveform: &CachedWaveform,
        visible: Range<u64>,
        limit: usize,
    ) -> Option<CachedOverrange> {
        let active = self.active(visible)?;
        if active.is_empty() {
            return Some(CachedOverrange::default());
        }
        let source = self.source(&active)?;
        let first = source.start;
        let gain = self.gain.linear();
        waveform.bounded_overrange(source, limit, |frame| {
            let position = active
                .start
                .checked_add(u64::try_from(frame.checked_sub(first)?).ok()?)?;
            Some((self.fade.gain_at(position), gain))
        })
    }
}

fn scale(mut peaks: [CachedPeak; 2], fade: f32, gain: f32) -> [CachedPeak; 2] {
    for peak in &mut peaks {
        peak.min = (peak.min * fade) * gain;
        peak.max = (peak.max * fade) * gain;
    }
    peaks
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::{
        Audio, PreparedRevision, SineTable, audio_transform::AudioTransformPreparation,
    };
    use hex_media::waveform::{self, Repeat};
    use hex_project::{AudioEventEdit, Command, Created, Editor};
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn repeated_source_applies_one_outer_fade_before_clip_gain() {
        let audio = Audio::new(
            48_000,
            vec![[9.0, -9.0], [2.0, -2.0], [3.0, -3.0], [4.0, -4.0]],
        )
        .unwrap();
        let repeated = waveform::summarize_repeated(
            &audio,
            Repeat {
                offset: 1,
                period: 3,
                phase: 2,
            },
            12,
            6,
        )
        .unwrap();
        let projection = PreparedFade {
            timeline_origin: 10,
            root_offset: 1,
            end: 14,
            fade: ClipFade::new(4, 0, 3, 0).unwrap(),
            gain: ClipGain::new(-6_000).unwrap(),
        };
        let gain = projection.gain.linear();
        assert_eq!(projection.peaks(&repeated, 9..11, 2), Some(ZERO));
        let expected = (3.0 * projection.fade.gain_at(1)) * gain;
        assert_eq!(
            projection.peaks(&repeated, 11..12, 1).unwrap()[0].max,
            expected
        );
        let wide = projection.peaks(&repeated, 11..14, 1).unwrap();
        assert_eq!(wide[0].max, 4.0 * gain);
        let exact = projection.overrange(&repeated, 10..14, 4).unwrap();
        assert_eq!(exact.positive, [true, false]);
        assert_eq!(exact.negative, [false, true]);
        assert!(projection.overrange(&repeated, 10..14, 3).is_none());
    }

    #[test]
    fn prepared_group_preserves_sparse_silence_and_refuses_its_baked_legacy_fade() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "sparse".into(),
                path: "sparse.wav".into(),
                sample_rate: 48_000,
                frames: 24,
            })
            .unwrap()
        else {
            panic!("import source");
        };
        for operation in [
            AudioEventEdit::Split { at: 8 },
            AudioEventEdit::Place {
                event: 1,
                start: 16_384,
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Right,
                frame: 16_392,
            })
            .unwrap();
        let audio = Audio::new(48_000, vec![[2.0, -2.0]; 24]).unwrap();
        let media = BTreeMap::from([(asset, audio)]);
        let prepare = |project: &Project| {
            let playback = PreparedRevision::prepare_with_audio_transforms(
                project,
                &media,
                Arc::new(SineTable::new()),
                None,
                &AudioTransformPreparation::default(),
            )
            .unwrap();
            let view = &playback.arrangement().audio_clip_views()[0];
            let sparse = Arc::clone(view.sparse.as_ref().expect("real packed group"));
            AudioClipWaveform {
                event_source: None,
                neutral_musical: None,
                waveform: Arc::new(waveform::summarize_sparse(&view.audio, sparse, 64).unwrap()),
                offset: view.offset,
                length: view.length,
                preview_root: None,
            }
        };
        let length = editor.project().clip(clip).unwrap().duration();
        let held = ClipFade::new(length, 0, 2, 0).unwrap();
        let project = editor.project();
        let neutral = prepare(project);
        let initial = PreparedFade::for_clip(
            project,
            project.clip(clip).unwrap(),
            &neutral,
            Some(held),
            ClipGain::UNITY,
        )
        .unwrap();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 3,
                fade_out: 0,
            })
            .unwrap();
        let project = editor.project();
        let baked = prepare(project);
        assert!(
            PreparedFade::for_clip(
                project,
                project.clip(clip).unwrap(),
                &baked,
                Some(held),
                ClipGain::UNITY
            )
            .is_none()
        );
        editor
            .execute(Command::SetClipFadeEnvelope { clip, fade: held })
            .unwrap();
        let project = editor.project();
        let prepared = prepare(project);
        let canonical = PreparedFade::for_clip(
            project,
            project.clip(clip).unwrap(),
            &prepared,
            None,
            ClipGain::UNITY,
        )
        .unwrap();
        assert_eq!(initial, canonical);
        assert_eq!(
            canonical.peaks(&prepared.waveform, 8192..8196, 4),
            Some(ZERO)
        );
        let around_event = canonical
            .peaks(&prepared.waveform, 16_383..16_385, 2)
            .unwrap();
        assert_eq!(around_event[0], CachedPeak { min: 0.0, max: 2.0 });
        assert_eq!(
            around_event[1],
            CachedPeak {
                min: -2.0,
                max: 0.0
            }
        );
        let clipped = canonical
            .overrange(&prepared.waveform, 16_383..16_385, 2)
            .unwrap();
        assert_eq!(clipped.positive, [true, false]);
        assert_eq!(clipped.negative, [false, true]);
    }
}
