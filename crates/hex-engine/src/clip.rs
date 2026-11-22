//! Validated tempo, audio buffer, clip scheduling, and prepared clip view types.
use crate::{
    Frame, Invalid, PreparedTrackMix, SparseFrameMap, StreamedSourceMetadata, audio_transform,
    pcm_arc,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct Tempo(f64);
impl Tempo {
    pub fn new(bpm: f64) -> Result<Self, Invalid> {
        if !bpm.is_finite() || bpm <= 0.0 {
            return Err(Invalid("tempo must be finite and positive"));
        }
        Ok(Self(bpm))
    }
    /// Convert quarter-note beats to frames, rounded to the nearest frame.
    pub fn frames_at(self, beats: f64, sample_rate: u32) -> Result<u64, Invalid> {
        if sample_rate == 0 || !beats.is_finite() || beats < 0.0 {
            return Err(Invalid("invalid beat position or sample rate"));
        }
        let frames = (beats * 60.0 / self.0 * f64::from(sample_rate)).round();
        if !frames.is_finite() || frames >= u64::MAX as f64 {
            return Err(Invalid("frame position overflow"));
        }
        Ok(frames as u64)
    }
}

#[derive(Debug, Clone)]
pub struct Audio {
    pub(crate) sample_rate: u32,
    pub(crate) frames: Arc<AudioData>,
    pub(crate) pcm_arc_receipt: Option<pcm_arc::PcmArcReceipt>,
}
#[derive(Debug)]
pub(crate) struct AudioData {
    pub(crate) values: Vec<Frame>,
    pub(crate) _derived: Option<audio_transform::Reservation>,
}
impl std::ops::Deref for AudioData {
    type Target = [Frame];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
impl Audio {
    pub(crate) fn from_derived(
        sample_rate: u32,
        frames: Vec<Frame>,
        reservation: audio_transform::Reservation,
    ) -> Result<Self, Invalid> {
        if frames.is_empty() || frames.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(Invalid("prepared audio must contain finite samples"));
        }
        Ok(Self {
            sample_rate,
            pcm_arc_receipt: None,
            frames: Arc::new(AudioData {
                values: frames,
                _derived: Some(reservation),
            }),
        })
    }
    /// Source rate; compilation currently requires an exact project-rate match.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Shared immutable stereo PCM, available for import validation and analysis.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
    /// Control-side capacity of the retained PCM buffer, in stereo frames.
    /// This excludes owner/graph metadata and is not a resource-admission proof.
    pub fn pcm_capacity_frames(&self) -> usize {
        self.frames.values.capacity()
    }
    /// True when two audio handles retain the same immutable PCM allocation.
    /// Exact retained PCM identity for control-side cache/summary reuse.
    pub fn shares_pcm_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.frames, &other.frames)
    }
    pub fn new(sample_rate: u32, frames: Vec<Frame>) -> Result<Self, Invalid> {
        if sample_rate == 0 || frames.is_empty() {
            return Err(Invalid("audio needs a sample rate and frames"));
        }
        if frames.iter().flatten().any(|s| !s.is_finite()) {
            return Err(Invalid("audio must contain finite samples"));
        }
        Ok(Self {
            sample_rate,
            pcm_arc_receipt: None,
            frames: Arc::new(AudioData {
                values: frames,
                _derived: None,
            }),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub(crate) active: bool,
    pub(crate) audio: Option<Audio>,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) looping: bool,
    pub(crate) source_offset: usize,
    pub(crate) source_length: usize,
    pub(crate) metadata: Option<Box<ClipMetadata>>,
}

/// Control-owned uncommon playback state; neutral clips keep their compact stride.
#[derive(Debug, Clone)]
pub(crate) struct ClipMetadata {
    pub(crate) gain: f32,
    pub(crate) source_phase: u64,
    pub(crate) reversed: bool,
    pub(crate) fade: Option<hex_project::ClipFade>,
    pub(crate) streamed: Option<Arc<StreamedSourceMetadata>>,
    pub(crate) sparse: Option<Arc<SparseFrameMap>>,
}
impl Clip {
    /// The frame range outside which this clip never renders.
    pub(crate) fn index_bounds(&self) -> (u64, u64) {
        (self.start, self.end)
    }

    /// The timeline interval is [start, start + duration). Loops repeat the entire source.
    pub fn new(audio: Audio, start: u64, duration: u64, looping: bool) -> Result<Self, Invalid> {
        if duration == 0 {
            return Err(Invalid("clip duration must be positive"));
        }
        let source_length = audio.frames.len();
        Self::region_with_fade(
            audio,
            start,
            duration,
            0,
            source_length as u64,
            looping,
            hex_project::ClipFade::neutral(duration).map_err(|error| Invalid(error.0))?,
        )
    }
    /// Compile a validated source region without copying its PCM buffer.
    pub fn region(
        audio: Audio,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
    ) -> Result<Self, Invalid> {
        if duration == 0 || source_length == 0 {
            return Err(Invalid("clip and source lengths must be positive"));
        }
        let fade = hex_project::ClipFade::neutral(duration).map_err(|error| Invalid(error.0))?;
        Self::region_with_fade(
            audio,
            start,
            duration,
            source_offset,
            source_length,
            looping,
            fade,
        )
    }

    /// Compile a validated source region and canonical fixed-domain envelope.
    pub fn region_with_fade(
        audio: Audio,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
        fade: hex_project::ClipFade,
    ) -> Result<Self, Invalid> {
        if duration == 0 || source_length == 0 {
            return Err(Invalid("clip and source lengths must be positive"));
        }
        let end = start
            .checked_add(duration)
            .ok_or(Invalid("clip end overflow"))?;
        let source_end = source_offset
            .checked_add(source_length)
            .ok_or(Invalid("source end overflow"))?;
        if source_end > audio.frames.len() as u64 {
            return Err(Invalid("clip region exceeds audio"));
        }
        fade.validate_segment(duration)
            .map_err(|error| Invalid(error.0))?;
        let metadata = if fade.is_neutral() {
            None
        } else {
            Some(Box::new(ClipMetadata {
                gain: 1.0,
                source_phase: 0,
                reversed: false,
                fade: Some(fade),
                streamed: None,
                sparse: None,
            }))
        };
        Ok(Self {
            active: true,
            audio: Some(audio),
            start,
            end,
            looping,
            source_offset: source_offset as usize,
            source_length: source_length as usize,
            metadata,
        })
    }
    /// Set the source-relative frame played at the clip's timeline start.
    /// One-shot clips require phase zero; loop phases must be inside the source region.
    /// This control-side builder may allocate/reclaim metadata; never call it in render.
    pub fn with_source_phase(mut self, phase: u64) -> Result<Self, Invalid> {
        if (!self.looping && phase != 0) || (self.looping && phase >= self.source_length as u64) {
            return Err(Invalid("clip source phase is outside its source interval"));
        }
        if let Some(metadata) = self.metadata.as_mut() {
            metadata.source_phase = phase;
            if phase == 0
                && metadata.fade.is_none()
                && metadata.streamed.is_none()
                && metadata.sparse.is_none()
                && metadata.gain == 1.0
            {
                self.metadata = None;
            }
        } else if phase != 0 {
            self.metadata = Some(Box::new(ClipMetadata {
                gain: 1.0,
                source_phase: phase,
                reversed: false,
                fade: None,
                streamed: None,
                sparse: None,
            }));
        }
        Ok(self)
    }
    /// Set the finite destination-clock envelope on control, outside render.
    /// Preserves source, gain, streamed and sparse metadata. This builder may
    /// allocate or reclaim metadata; never call it in the audio callback.
    pub fn with_fade(mut self, fade: hex_project::ClipFade) -> Result<Self, Invalid> {
        fade.validate_segment(self.end - self.start)
            .map_err(|error| Invalid(error.0))?;
        let fade = (!fade.is_neutral()).then_some(fade);
        if let Some(metadata) = self.metadata.as_mut() {
            metadata.fade = fade;
            if fade.is_none()
                && metadata.source_phase == 0
                && !metadata.reversed
                && metadata.streamed.is_none()
                && metadata.sparse.is_none()
                && metadata.gain == 1.0
            {
                self.metadata = None;
            }
        } else if fade.is_some() {
            self.metadata = Some(Box::new(ClipMetadata {
                gain: 1.0,
                source_phase: 0,
                reversed: false,
                fade,
                streamed: None,
                sparse: None,
            }));
        }
        Ok(self)
    }

    /// Control-side clip gain preparation, preserving the compact unity path.
    pub fn with_gain(mut self, gain: hex_project::ClipGain) -> Self {
        let linear = gain.linear();
        if let Some(metadata) = self.metadata.as_mut() {
            metadata.gain = linear;
            if linear == 1.0
                && metadata.source_phase == 0
                && metadata.fade.is_none()
                && metadata.streamed.is_none()
                && metadata.sparse.is_none()
            {
                self.metadata = None;
            }
        } else if linear != 1.0 {
            self.metadata = Some(Box::new(ClipMetadata {
                gain: linear,
                source_phase: 0,
                reversed: false,
                fade: None,
                streamed: None,
                sparse: None,
            }));
        }
        self
    }

    pub(crate) fn sparse_region(
        audio: Audio,
        sparse: Arc<SparseFrameMap>,
        start: u64,
        duration: u64,
        source_offset: u64,
    ) -> Result<Self, Invalid> {
        let end = start
            .checked_add(duration)
            .ok_or(Invalid("clip end overflow"))?;
        let offset =
            usize::try_from(source_offset).map_err(|_| Invalid("sparse audio offset overflow"))?;
        let length =
            usize::try_from(duration).map_err(|_| Invalid("sparse audio duration overflow"))?;
        if length == 0
            || offset
                .checked_add(length)
                .is_none_or(|end| end > sparse.extent())
            || audio.frames().len() != sparse.packed_len()
        {
            return Err(Invalid("clip region exceeds sparse audio"));
        }
        Ok(Self {
            active: true,
            audio: Some(audio),
            start,
            end,
            looping: false,
            source_offset: offset,
            source_length: length,
            metadata: Some(Box::new(ClipMetadata {
                gain: 1.0,
                source_phase: 0,
                reversed: false,
                fade: None,
                streamed: None,
                sparse: Some(sparse),
            })),
        })
    }

    pub(crate) fn at(&self, position: u64, source_phase: u64) -> Frame {
        if !self.active || position < self.start || position >= self.end {
            return [0.0; 2];
        }
        let offset = position - self.start;
        let length = self.source_length as u64;
        if !self.looping && offset >= length {
            return [0.0; 2];
        }
        let source_offset = if !self.looping {
            offset
        } else {
            let relative = offset % length;
            if source_phase == 0 {
                relative
            } else {
                let until_wrap = length - source_phase;
                if relative >= until_wrap {
                    relative - until_wrap
                } else {
                    relative + source_phase
                }
            }
        };
        self.audio.as_ref().map_or([0.0; 2], |audio| {
            let index = self.source_offset + source_offset as usize;
            if let Some(map) = self.metadata.as_ref().and_then(|m| m.sparse.as_ref()) {
                return map
                    .packed_index(index)
                    .map_or([0.0; 2], |i| audio.frames[i]);
            }
            audio.frames[index]
        })
    }

    // Track supplies an intersected, neutral, uncaptured interval. Admit the
    // source view before writing, then visit each contiguous piece in order.
    // No PCM owner is cloned or reclaimed, and no position modulo is needed
    // inside the per-frame mix loop.
    pub(crate) fn render_neutral_additive(
        &self,
        position: u64,
        phase: u64,
        mut output: &mut [Frame],
        mix: PreparedTrackMix,
    ) -> bool {
        let Some(source) = self.audio.as_ref().and_then(|audio| {
            self.source_offset
                .checked_add(self.source_length)
                .and_then(|end| audio.frames().get(self.source_offset..end))
        }) else {
            return false;
        };
        let Ok(length) = u64::try_from(source.len()) else {
            return false;
        };
        if length == 0 || phase >= length {
            return false;
        }
        let Some(relative) = position.checked_sub(self.start) else {
            return false;
        };
        let offset = if self.looping {
            let relative = relative % length;
            let until_wrap = length - phase;
            if relative >= until_wrap {
                relative - until_wrap
            } else {
                relative + phase
            }
        } else {
            relative
        };
        let Ok(mut index) = usize::try_from(offset) else {
            return false;
        };
        if index >= source.len() || (!self.looping && output.len() > source.len() - index) {
            return false;
        }
        // All refusal paths precede mixing, so the caller can retain its
        // established scalar fallback without adding any frame twice.
        while !output.is_empty() {
            let available = &source[index..];
            let count = output.len().min(available.len());
            let (chunk, tail) = output.split_at_mut(count);
            for (destination, sample) in chunk.iter_mut().zip(available) {
                mix.add_frame(destination, *sample);
            }
            output = tail;
            index = 0;
        }
        true
    }

    /// Build a clip backed by a validated generation-scoped streamed source.
    pub fn streamed_region(
        source: Arc<StreamedSourceMetadata>,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
    ) -> Result<Self, Invalid> {
        let fade = hex_project::ClipFade::neutral(duration).map_err(|error| Invalid(error.0))?;
        Self::streamed_region_with_fade(
            source,
            start,
            duration,
            source_offset,
            source_length,
            looping,
            fade,
        )
    }

    /// Build a reverse-reading streamed region. This is a playback boundary;
    /// it does not admit or change any saved clip edit.
    pub fn streamed_region_reversed(
        source: Arc<StreamedSourceMetadata>,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
    ) -> Result<Self, Invalid> {
        let fade = hex_project::ClipFade::neutral(duration).map_err(|error| Invalid(error.0))?;
        Self::streamed_region_reversed_with_fade(
            source,
            start,
            duration,
            source_offset,
            source_length,
            looping,
            fade,
        )
    }

    /// Build a reverse-reading streamed region with a destination fade.
    pub fn streamed_region_reversed_with_fade(
        source: Arc<StreamedSourceMetadata>,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
        fade: hex_project::ClipFade,
    ) -> Result<Self, Invalid> {
        let mut clip = Self::streamed_region_with_fade(
            source,
            start,
            duration,
            source_offset,
            source_length,
            looping,
            fade,
        )?;
        if let Some(metadata) = clip.metadata.as_mut() {
            metadata.reversed = true;
        }
        Ok(clip)
    }

    /// Build a streamed clip with the canonical fixed-domain fade envelope.
    pub fn streamed_region_with_fade(
        source: Arc<StreamedSourceMetadata>,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
        fade: hex_project::ClipFade,
    ) -> Result<Self, Invalid> {
        if duration == 0 || source_length == 0 {
            return Err(Invalid("clip and source lengths must be positive"));
        }
        let end = start
            .checked_add(duration)
            .ok_or(Invalid("clip end overflow"))?;
        let source_end = source_offset
            .checked_add(source_length)
            .ok_or(Invalid("source end overflow"))?;
        if source_end > source.playback_frames() {
            return Err(Invalid("clip region exceeds streamed source"));
        }
        fade.validate_segment(duration)
            .map_err(|error| Invalid(error.0))?;
        Ok(Self {
            active: true,
            audio: None,
            start,
            end,
            looping,
            source_offset: usize::try_from(source_offset)
                .map_err(|_| Invalid("source offset exceeds addressable range"))?,
            source_length: usize::try_from(source_length)
                .map_err(|_| Invalid("source length exceeds addressable range"))?,
            metadata: Some(Box::new(ClipMetadata {
                gain: 1.0,
                source_phase: 0,
                reversed: false,
                fade: (!fade.is_neutral()).then_some(fade),
                streamed: Some(source),
                sparse: None,
            })),
        })
    }
}

impl Clip {
    pub(crate) fn is_streamed(&self) -> bool {
        self.metadata
            .as_deref()
            .is_some_and(|metadata| metadata.streamed.is_some())
    }
}

/// Immutable control-side root/crop view for worker waveform preparation.
/// Audio retains the same PCM reservation as playback; never build summaries in render.
/// Without `repeat`, the clip plays `audio[offset..offset + length)` once. With it,
/// the clip's `length` frames loop `audio[offset..offset + period)` from `phase`.
#[derive(Debug, Clone)]
pub struct PreparedAudioClipView {
    pub clip: hex_project::ClipId,
    pub audio: Audio,
    pub offset: u64,
    pub length: u64,
    pub repeat: Option<PreparedAudioRepeat>,
    /// Maps logical root frames to packed PCM for a non-repeating source.
    pub sparse: Option<Arc<SparseFrameMap>>,
    /// Unweighted destination bins for a musical outer fade, including before
    /// the first fade or after a reset. Empty means capture was unavailable.
    pub neutral_musical_peaks: Vec<PreparedAudioPeak>,
    /// Single-source transformed processing clock, before event and root fades.
    pub event_source_peaks: Option<Arc<audio_transform::PreparedEventSourcePeaks>>,
    /// Loop read of those event peaks when all musical occurrences share their
    /// processing root. This remains available before a baked instance fade.
    pub event_source_repeat: Option<PreparedAudioRepeat>,
    /// Per-occurrence source reads for a materialized forward musical view.
    pub event_source_occurrences: Option<Arc<audio_transform::PreparedEventOccurrences>>,
    /// Peak summaries for independent sources before their events.
    pub event_sources: Option<Arc<audio_transform::PreparedEventSources>>,
}

/// Exact extrema of one consecutive stereo destination range before outer
/// fade and clip gain. Bins partition the view's length as evenly as possible,
/// with one extra frame in each of the first `length % bins.len()` bins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedAudioPeak {
    pub min: Frame,
    pub max: Frame,
}

/// Loop geometry of a prepared clip that plays a shared root instead of outer PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedAudioRepeat {
    pub offset: u64,
    pub period: u64,
    pub phase: u64,
}

impl PreparedAudioRepeat {
    pub(crate) fn from_clip(clip: &Clip) -> Option<Self> {
        clip.looping.then(|| Self {
            offset: clip.source_offset as u64,
            period: clip.source_length as u64,
            phase: clip.metadata.as_deref().map_or(0, |m| m.source_phase),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tempo_and_invalid_positions() {
        let tempo = Tempo::new(120.0).unwrap();
        assert_eq!(tempo.frames_at(4.0, 48_000).unwrap(), 96_000);
        assert!(Tempo::new(f64::NAN).is_err());
        assert!(Tempo::new(0.0).is_err());
        assert!(tempo.frames_at(-1.0, 48_000).is_err());
        assert!(tempo.frames_at(f64::MAX, 48_000).is_err());
    }

    #[test]
    fn audio_and_clip_reject_bad_sources_and_overflow() {
        assert!(Audio::new(0, vec![[0.0; 2]]).is_err());
        assert!(Audio::new(48_000, vec![[f32::INFINITY; 2]]).is_err());
        let a = Audio::new(44_100, vec![[0.0; 2]]).unwrap();
        assert!(Clip::new(a, u64::MAX, 1, false).is_err());
    }
}
