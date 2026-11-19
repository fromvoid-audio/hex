//! Exact original-source reads for unprocessed frame-clock fade visualization.
//! This control-only metadata query owns no PCM, cache, preparation or playback
//! authority. The caller binds the returned asset/frame to its accepted resident
//! waveform, then applies gains before aggregating exact visible extrema.
use crate::{Frame, Invalid};
use hex_project::{
    Asset, AssetId, AudioClipTiming, AudioDomainClock, AudioEvent, AudioEvents,
    AudioViewProjection, Clip, ClipFade, ClipGain, MAX_AUDIO_EVENTS, TempoMap,
};
use std::ops::Range;

/// One original converted-asset sample and its playback-ordered amplitude laws.
/// It contains no source owner. Clip activation remains a separate display style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameFadeSample {
    source_frame: u64,
    event: Option<u64>,
    gains: [f32; 4],
}
impl FrameFadeSample {
    pub fn source_frame(self) -> u64 {
        self.source_frame
    }
    pub fn event(self) -> Option<u64> {
        self.event
    }
    /// Event fade, event gain, container fade, then outer clip gain. Combining
    /// these factors first can change rounding at the full-scale boundary.
    pub fn gains(self) -> [f32; 4] {
        self.gains
    }
    pub fn apply_to(self, mut sample: Frame) -> Frame {
        for gain in self.gains {
            if gain != 1.0 {
                for channel in &mut sample {
                    *channel *= gain;
                }
            }
        }
        sample
    }
}

/// Borrowed display payloads from existing canonical, held or pending owners.
/// Absence uses the canonical law; no Project clone or command execution occurs.
#[derive(Clone, Copy, Default)]
pub struct FrameFadeOverrides<'a> {
    pub fade: Option<ClipFade>,
    /// The editor's existing preview, edited from `Clip::editable_audio_events()`
    /// when a crop creates virtual fragments. This preserves local event IDs.
    pub events: Option<&'a AudioEvents>,
    pub gain: Option<ClipGain>,
}

/// One active destination-root interval and its original-source interval union.
/// The source union has at most two pieces even across many source loop cycles.
/// `sample` holds gains sampled at the destination interval midpoint: applying
/// them to cached extrema is the established coarse display approximation,
/// not exact faded PCM extrema. Exact detail uses the per-sample query instead.
pub struct FrameFadeRange {
    pub root: Range<u64>,
    pub source: [Option<Range<u64>>; 2],
    pub sample: FrameFadeSample,
}

/// Completed range coverage. Silent gaps must contribute genuine zero to the
/// column extrema. Unavailable/budget-exhausted queries return `None` instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameFadeCoverage {
    pub has_silence: bool,
}

/// Borrowed exact frame-source mapping for ordinary clips and frame placements.
/// Unsupported source processing returns `None` at construction, independently
/// from a supported query returning `None` for a hole or exhausted source.
///
/// No warp, pitch, source envelope, musical/frozen clock, retained child or
/// directed content read is represented. Those retain their prepared waveform
/// path. Source conversion has already happened in the caller's resident asset;
/// its immutable waveform must belong to `asset()` and cover `source_frames()`.
/// A held/pending clip may replace the canonical clip without replacing that
/// original source. This query never divides baked peaks by an old fade.
/// Construction validates on control and may allocate validation metadata;
/// individual sample/range queries do not allocate or prepare audio, excluding
/// caller-supplied callbacks.
pub struct FrameFadePreview<'a> {
    clip: &'a Clip,
    tempo: &'a TempoMap,
    projection: Option<AudioViewProjection<'a>>,
    source_frames: u64,
    overrides: FrameFadeOverrides<'a>,
    events: Option<&'a AudioEvents>,
    // Prepare scalar coefficients once, not inside visible sample loops.
    event_gains: [f32; MAX_AUDIO_EVENTS],
    clip_gain: f32,
}
impl<'a> FrameFadePreview<'a> {
    pub fn new(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        asset: &Asset,
    ) -> Result<Option<Self>, Invalid> {
        Self::with_preview(clip, tempo, asset, FrameFadeOverrides::default())
    }
    pub fn with_preview(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        asset: &Asset,
        overrides: FrameFadeOverrides<'a>,
    ) -> Result<Option<Self>, Invalid> {
        if clip.asset() != asset.id() {
            return Err(Invalid("fade preview asset differs from clip"));
        }
        let source_frames = asset.playback_frames();
        let projection = if let Some(edit) = clip.audio_edit() {
            if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
                || !matches!(edit.timing, AudioClipTiming::Frames)
                || edit.domain.warp_enabled
                || edit.domain.pitch_cents != 0
                || !edit.domain.source_gain.is_empty()
                || edit.domain.musical_event_source.is_some()
                || edit.processing_projection.is_some()
                || edit.content_view.is_some()
                || edit.domain.events.as_ref().is_some_and(|events| {
                    !events.sources.is_empty()
                        || events.events.iter().any(|event| event.source != 0)
                })
            {
                return Ok(None);
            }
            edit.validate(clip, tempo, source_frames)
                .map_err(|error| Invalid(error.0))?;
            Some(AudioViewProjection::new(clip, tempo).map_err(|error| Invalid(error.0))?)
        } else {
            if clip.duration() == 0
                || clip.source_length() == 0
                || clip
                    .source_offset()
                    .checked_add(clip.source_length())
                    .is_none_or(|end| end > source_frames)
                || (clip.looping() && clip.source_phase() >= clip.source_length())
                || (!clip.looping() && clip.source_phase() != 0)
                || clip.start().checked_add(clip.duration()).is_none()
            {
                return Err(Invalid("fade preview source interval is invalid"));
            }
            clip.fade()
                .validate_segment(clip.duration())
                .map_err(|error| Invalid(error.0))?;
            None
        };
        if overrides
            .fade
            .is_some_and(|fade| fade.span() != clip.duration() || fade.offset() != 0)
        {
            return Err(Invalid("edited fade must match the current clip"));
        }
        let events = overrides.events.or_else(|| {
            clip.audio_edit()
                .and_then(|edit| edit.domain.events.as_deref())
        });
        if let Some(events) = events {
            if !events.sources.is_empty() || events.events.iter().any(|event| event.source != 0) {
                return Ok(None);
            }
            events
                .validate(
                    clip.audio_edit()
                        .map_or(clip.duration(), |edit| edit.domain.clock.length()),
                )
                .map_err(|error| Invalid(error.0))?;
        }
        let mut event_gains = [1.0; MAX_AUDIO_EVENTS];
        if let Some(events) = events {
            for (gain, event) in event_gains.iter_mut().zip(&events.events) {
                *gain = event.gain.linear();
            }
        }
        let clip_gain = overrides.gain.unwrap_or_else(|| clip.gain()).linear();
        Ok(Some(Self {
            clip,
            tempo,
            projection,
            source_frames,
            overrides,
            events,
            event_gains,
            clip_gain,
        }))
    }
    pub fn asset(&self) -> AssetId {
        self.clip.asset()
    }
    pub fn source_frames(&self) -> u64 {
        self.source_frames
    }
    /// Sample in the Arrangement project-frame clock. Finite placement and
    /// source loops retain their distinct phases; Once holes remain silence.
    pub fn timeline_sample(&self, frame: u64) -> Result<Option<FrameFadeSample>, Invalid> {
        if let Some(projection) = &self.projection {
            let Some(root) = projection
                .root_position_for_frame(frame)
                .map_err(|error| Invalid(error.0))?
            else {
                return Ok(None);
            };
            self.root_sample(root)
        } else {
            let Some(elapsed) = frame
                .checked_sub(self.clip.start())
                .filter(|elapsed| *elapsed < self.clip.duration())
            else {
                return Ok(None);
            };
            self.root_sample(elapsed)
        }
    }
    /// Sample in the retained processing root's destination-frame clock, before
    /// outer placement. This is the clip editor's event timeline coordinate.
    pub fn root_sample(&self, frame: u64) -> Result<Option<FrameFadeSample>, Invalid> {
        if frame
            >= self
                .clip
                .audio_edit()
                .map_or(self.clip.duration(), |edit| edit.domain.clock.length())
        {
            return Ok(None);
        }
        let event = if let Some(events) = self.events {
            let index = events.events.partition_point(|event| event.end <= frame);
            let Some(event) = events
                .events
                .get(index)
                .filter(|event| event.active && event.start <= frame)
            else {
                return Ok(None);
            };
            Some(event)
        } else {
            None
        };
        self.event_root_sample(frame, event)
    }
    /// Resolve one local event identity once before visiting its visible samples.
    /// The implicit initial event has identity 1, like `Clip::audio_events()`.
    /// Lookup examines at most 1024 events; the returned sample query uses a
    /// bounded binary coefficient lookup and retains no PCM. Missing identities
    /// are not a supported sample.
    pub fn event(&self, id: u64) -> Option<FrameFadeEvent<'_, 'a>> {
        let event = match self.events {
            Some(events) => Some(events.events.iter().find(|event| event.id == id)?),
            None if id == 1 => None,
            None => return None,
        };
        Some(FrameFadeEvent {
            preview: self,
            event,
        })
    }
    fn event_root_sample(
        &self,
        frame: u64,
        event: Option<&AudioEvent>,
    ) -> Result<Option<FrameFadeSample>, Invalid> {
        let read = match event {
            Some(event) => event
                .source_start
                .checked_add(frame - event.start)
                .ok_or(Invalid("fade preview source frame overflow"))?,
            None => frame,
        };
        let event_fade = if let Some(fade) = event.and_then(|event| event.fade) {
            let age = read
                .checked_sub(fade.source_start())
                .filter(|age| *age < fade.shape().span())
                .ok_or(Invalid("fade preview event exceeds fade domain"))?;
            fade.shape().gain_at(age)
        } else {
            1.0
        };
        let clip_fade = if let Some(fade) = self.overrides.fade {
            let start = self.clip.audio_edit().map_or(0, |edit| edit.view_start);
            if self.clip.audio_edit().is_some() && fade.is_neutral() {
                1.0
            } else {
                frame
                    .checked_sub(start)
                    .map_or(0.0, |age| fade.gain_at(age))
            }
        } else if let Some(edit) = self.clip.audio_edit() {
            edit.projected_fade(self.tempo)
                .and_then(|fade| fade.gain_at(frame))
                .map_err(|error| Invalid(error.0))?
        } else {
            self.clip.fade().gain_at(frame)
        };
        let event_gain = if event.is_some() {
            let events = self
                .events
                .ok_or(Invalid("fade preview event owner is missing"))?;
            let index = events.events.partition_point(|event| event.end <= frame);
            *self
                .event_gains
                .get(index)
                .ok_or(Invalid("fade preview event coefficient is missing"))?
        } else {
            1.0
        };
        self.original_sample(
            read,
            event.map(|event| event.id),
            [event_fade, event_gain, clip_fade, self.clip_gain],
        )
    }
    fn original_sample(
        &self,
        read: u64,
        event: Option<u64>,
        gains: [f32; 4],
    ) -> Result<Option<FrameFadeSample>, Invalid> {
        let local = if self.clip.looping() {
            ((u128::from(self.clip.source_phase()) + u128::from(read))
                % u128::from(self.clip.source_length())) as u64
        } else if read < self.clip.source_length() {
            read
        } else {
            return Ok(None);
        };
        let source_frame = self
            .clip
            .source_offset()
            .checked_add(local)
            .filter(|frame| *frame < self.source_frames)
            .ok_or(Invalid("fade preview sample exceeds source"))?;
        Ok(Some(FrameFadeSample {
            source_frame,
            event,
            gains,
        }))
    }
}

/// A resolved event within a borrowed frame-source query. Resolve once per
/// visible event rather than scanning event identities for each sample.
pub struct FrameFadeEvent<'p, 'a> {
    preview: &'p FrameFadePreview<'a>,
    event: Option<&'a AudioEvent>,
}
impl FrameFadeEvent<'_, '_> {
    /// `source_position` is the event's retained source-root frame
    /// (`event.source_start + age`), NOT an original asset frame. Returned
    /// `source_frame()` includes the domain's source offset, loop and phase.
    pub fn sample(&self, source_position: u64) -> Result<Option<FrameFadeSample>, Invalid> {
        let Some(event) = self.event else {
            return self.preview.root_sample(source_position);
        };
        let Some(age) = source_position
            .checked_sub(event.source_start)
            .filter(|age| *age < event.length() && event.active)
        else {
            return Ok(None);
        };
        let frame = event
            .start
            .checked_add(age)
            .ok_or(Invalid("fade preview event frame overflow"))?;
        self.preview.event_root_sample(frame, Some(event))
    }
}

mod ranges;

#[cfg(test)]
mod tests;
