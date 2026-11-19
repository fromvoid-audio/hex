//! Destination-only fade finalization after the source program's last read.
//! Musical fades use bounded PCM on control. The callback sees an ordinary
//! resident clip and never evaluates a project tempo map.
use super::{Audio, AudioClipEdit, AudioTransformPreparation, Clip, Invalid};
use crate::{Frame, PreparedAudioPeak};
use hex_project::{
    AudioClipFadeClock, AudioDomainClock, ClipFade, MAX_AUDIO_DOMAIN_FRAMES,
    ProjectedOuterAudioFade,
};

#[derive(Clone, Copy)]
pub(super) enum OuterPlaybackFade<'a> {
    Frame(ClipFade),
    Musical(&'a ProjectedOuterAudioFade<'a>),
}

impl<'a> OuterPlaybackFade<'a> {
    pub(super) fn projected(
        outer: Option<&'a ProjectedOuterAudioFade<'a>>,
        clock: AudioDomainClock,
    ) -> Result<Option<Self>, Invalid> {
        outer
            .map(|fade| match clock {
                AudioDomainClock::Frames(_) => Ok(Self::Frame(fade.frame_playback_envelope()?)),
                AudioDomainClock::Beats(_) => Ok(Self::Musical(fade)),
            })
            .transpose()
    }

    pub(super) fn admit(self, duration: u64, resident: bool) -> Result<(), Invalid> {
        if let Self::Musical(fade) = self
            && !fade.is_neutral()
        {
            if !resident {
                return Err(Invalid("musical outer fade requires resident audio"));
            }
            if duration == 0 || duration > MAX_AUDIO_DOMAIN_FRAMES {
                return Err(Invalid(
                    "musical outer fade exceeds bounded preparation frames",
                ));
            }
        }
        Ok(())
    }
}

/// A materialized output consumes one additional finite frame pass and buffer.
pub(crate) fn materializes_musical_outer_fade(edit: &AudioClipEdit) -> bool {
    edit.domain.fade_clock == AudioClipFadeClock::Outer
        && matches!(edit.domain.clock, AudioDomainClock::Beats(_))
        && !edit.domain.fade.is_neutral()
}

/// The collecting Arrangement compile owns this slot for one destination clip.
pub(super) struct MusicalPeakCapture {
    limit: usize,
    peaks: Vec<PreparedAudioPeak>,
}

struct PeakBins {
    peaks: Vec<PreparedAudioPeak>,
    end: usize,
    width: usize,
    extra: usize,
}

impl PeakBins {
    fn new(frames: usize, limit: usize) -> Option<Self> {
        let count = limit.min(frames);
        if count == 0 {
            return None;
        }
        let mut peaks = Vec::new();
        peaks.try_reserve_exact(count).ok()?;
        Some(Self {
            peaks,
            end: 0,
            width: frames / count,
            extra: frames % count,
        })
    }

    fn observe(&mut self, index: usize, sample: Frame) {
        if index == self.end {
            self.peaks.push(PreparedAudioPeak {
                min: sample,
                max: sample,
            });
            self.end += self.width + usize::from(self.peaks.len() <= self.extra);
        } else if let Some(peak) = self.peaks.last_mut() {
            for (channel, value) in sample.into_iter().enumerate() {
                peak.min[channel] = peak.min[channel].min(value);
                peak.max[channel] = peak.max[channel].max(value);
            }
        }
    }
}

impl AudioTransformPreparation {
    /// Optional control-side read for the first musical fade or a neutral
    /// reset. Uses the same bins and destination samples as the bake.
    pub(crate) fn capture_unbaked_musical_peaks(
        &self,
        clip: &Clip,
        limit: usize,
        remaining_frames: &mut u64,
    ) -> Vec<PreparedAudioPeak> {
        let duration = clip.end - clip.start;
        if duration > *remaining_frames || clip.audio.is_none() {
            return Vec::new();
        }
        let phase = match neutral_phase(clip) {
            Some(phase) => phase,
            None => return Vec::new(),
        };
        let Ok(length) = usize::try_from(duration) else {
            return Vec::new();
        };
        let Some(mut peaks) = PeakBins::new(length, limit) else {
            return Vec::new();
        };
        *remaining_frames -= duration;
        for (index, frame) in (clip.start..clip.end).enumerate() {
            if index % 1024 == 0 && self.cancelled() {
                return Vec::new();
            }
            peaks.observe(index, clip.at(frame, phase));
        }
        peaks.peaks
    }

    /// Collect an optional display sidecar from the existing bake pass. A failed
    /// or non-materializing compile produces no peaks and keeps no scratch.
    pub(crate) fn capture_musical_peaks<T>(
        &self,
        limit: usize,
        prepare: impl FnOnce() -> Result<T, Invalid>,
    ) -> Result<(T, Vec<PreparedAudioPeak>), Invalid> {
        let capture = (limit > 0).then(|| MusicalPeakCapture {
            limit,
            peaks: Vec::new(),
        });
        let previous = self.musical_peak_capture.replace(capture);
        let prepared = prepare();
        let capture = self.musical_peak_capture.replace(previous);
        prepared.map(|prepared| {
            (
                prepared,
                capture.map_or_else(Vec::new, |capture| capture.peaks),
            )
        })
    }

    pub(super) fn finish_outer_fade(
        &self,
        clip: Clip,
        fade: Option<OuterPlaybackFade<'_>>,
    ) -> Result<Clip, Invalid> {
        match fade {
            Some(OuterPlaybackFade::Frame(fade)) => clip.with_fade(fade),
            Some(OuterPlaybackFade::Musical(fade)) if !fade.is_neutral() => {
                self.materialize_musical_outer_fade(clip, fade)
            }
            _ => Ok(clip),
        }
    }

    fn materialize_musical_outer_fade(
        &self,
        mut clip: Clip,
        fade: &ProjectedOuterAudioFade<'_>,
    ) -> Result<Clip, Invalid> {
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        let source = clip
            .audio
            .as_ref()
            .ok_or(Invalid("musical outer fade requires resident audio"))?;
        let duration = clip
            .end
            .checked_sub(clip.start)
            .ok_or(Invalid("musical outer fade placement is reversed"))?;
        OuterPlaybackFade::Musical(fade).admit(duration, true)?;
        let phase = neutral_phase(&clip).ok_or(Invalid(
            "musical outer fade requires unfaded resident placement",
        ))?;
        let length = usize::try_from(duration)
            .map_err(|_| Invalid("musical outer fade span exceeds addressable frames"))?;
        let bytes = length
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .ok_or(Invalid("musical outer fade byte count overflow"))?;
        let reservation = self.reserve_with_eviction(bytes)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(length)
            .map_err(|_| Invalid("musical outer fade allocation failed"))?;
        let mut peaks = self
            .musical_peak_capture
            .borrow()
            .as_ref()
            .and_then(|capture| PeakBins::new(length, capture.limit));
        for (index, frame) in (clip.start..clip.end).enumerate() {
            if index % 1024 == 0 && self.cancelled() {
                return Err(Invalid("audio preparation cancelled"));
            }
            let gain = fade.gain_at(frame)?;
            let sample = clip.at(frame, phase);
            if let Some(peaks) = &mut peaks {
                peaks.observe(index, sample);
            }
            frames.push(sample.map(|sample| sample * gain));
        }
        if self.cancelled() {
            return Err(Invalid("audio preparation cancelled"));
        }
        clip.audio = Some(Audio::from_derived(
            source.sample_rate(),
            frames,
            reservation,
        )?);
        clip.source_offset = 0;
        clip.source_length = length;
        clip.looping = false;
        if let Some(metadata) = clip.metadata.as_mut() {
            metadata.source_phase = 0;
        }
        if let Some(peaks) = peaks
            && let Some(capture) = self.musical_peak_capture.borrow_mut().as_mut()
        {
            capture.peaks = peaks.peaks;
        }
        Ok(clip)
    }
}

fn neutral_phase(clip: &Clip) -> Option<u64> {
    let Some(metadata) = &clip.metadata else {
        return Some(0);
    };
    (metadata.fade.is_none()
        && metadata.sparse.is_none()
        && metadata.streamed.is_none()
        && !metadata.reversed)
        .then_some(metadata.source_phase)
}
