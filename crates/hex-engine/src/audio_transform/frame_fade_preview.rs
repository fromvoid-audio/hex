//! Source reads for frame-clock fade visualization.
//! This control-only metadata query owns no PCM, cache, preparation or playback
//! authority. The caller binds frames to an accepted asset or pre-event
//! processing-root waveform, then applies the ordered gains.
use crate::{Frame, Invalid};
use hex_project::{
    Asset, AssetId, AudioClipTiming, AudioDomainClock, AudioEvent, AudioEvents,
    AudioViewProjection, Clip, ClipFade, ClipGain, MAX_AUDIO_EVENTS, ProjectedOuterAudioFade,
    TempoMap,
};
use std::ops::Range;

mod musical;
pub use musical::{DirectedMusicalDestinationFade, MusicalEventFadePreview};

/// One source frame and its playback-ordered amplitude laws.
/// It contains no source owner. Clip activation remains a separate display style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameFadeSample {
    source_frame: u64,
    source_id: u64,
    event: Option<u64>,
    gains: [f32; 4],
}
impl FrameFadeSample {
    pub fn source_frame(self) -> u64 {
        self.source_frame
    }
    /// Zero selects the direct source. A retained child uses its group-local ID.
    pub fn source_id(self) -> u64 {
        self.source_id
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
    /// Use the retained-root law when an existing overlap needs its Legacy
    /// ramp proof. Callers obtain the policy from the arrangement project.
    pub legacy_root_fade: bool,
    /// The editor's existing preview, edited from `Clip::editable_audio_events()`
    /// when a crop creates virtual fragments. This preserves local event IDs.
    pub events: Option<&'a AudioEvents>,
    pub gain: Option<ClipGain>,
}

/// One active destination-root interval and its source interval union.
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
/// Unprocessed reads use the converted asset. Transformed frame events use
/// the accepted, pre-event processing root after warp, pitch and source gain.
/// Directed frame reads reflect the event read and keep the clip envelope in
/// the forward destination clock. Grouped reads address each compiled child by
/// its retained ID. The caller binds the waveform to the
/// chosen source and its `source_frames()`. Held/pending edits use that same
/// basis without dividing baked peaks by an old fade.
/// Construction validates on control and may allocate validation metadata;
/// individual sample/range queries do not allocate or prepare audio, excluding
/// caller-supplied callbacks.
pub struct FrameFadePreview<'a> {
    clip: &'a Clip,
    tempo: &'a TempoMap,
    projection: Option<AudioViewProjection<'a>>,
    outer_fade: Option<ProjectedOuterAudioFade<'a>>,
    directed_legacy: Option<DirectedLegacyFade>,
    source_frames: u64,
    source: SourceRead,
    overrides: FrameFadeOverrides<'a>,
    events: Option<&'a AudioEvents>,
    // Prepare scalar coefficients once, not inside visible sample loops.
    event_gains: [f32; MAX_AUDIO_EVENTS],
    clip_gain: f32,
}

#[derive(Clone, Copy)]
struct SourceRead {
    offset: u64,
    length: u64,
    phase: u64,
    looping: bool,
}

#[derive(Clone, Copy)]
enum PreviewSource {
    Asset,
    Prepared(u64),
    Group,
}

/// Forward envelope coverage within a directed frame clip. The reflected
/// content read has independent bounds and can be silent within this range.
struct DirectedLegacyFade {
    destination: Range<u64>,
    root_start: u64,
    cycle: u64,
    phase_at_start: u64,
}

impl DirectedLegacyFade {
    fn new(projection: &AudioViewProjection<'_>) -> Result<Option<Self>, Invalid> {
        let Some(view) = projection
            .clip()
            .audio_edit()
            .and_then(|edit| edit.content_view)
        else {
            return Ok(None);
        };
        let (start, end) = projection.outer_frame_bounds();
        let base = projection.base_reference_start();
        let destination = if let Some(last) = projection.occurrence_count().checked_sub(1) {
            // Directed frame occurrences are contiguous and have no lead-in.
            // A Once read can end early, independently of the reflected read.
            let first = projection.occurrence(0).map_err(|error| Invalid(error.0))?;
            let last = projection
                .occurrence(last)
                .map_err(|error| Invalid(error.0))?;
            let frame = |elapsed| {
                (base + i128::from(elapsed)).clamp(i128::from(start), i128::from(end)) as u64
            };
            frame(first.elapsed_start)..frame(last.elapsed_end)
        } else {
            start..start
        };
        let cycle = view.base_view_end - view.base_view_start;
        let phase = view
            .base_placement
            .map_or(0, |placement| placement.cycle_phase);
        let phase_at_start = (i128::from(destination.start) - base + i128::from(phase))
            .rem_euclid(i128::from(cycle)) as u64;
        Ok(Some(Self {
            destination,
            root_start: view.base_view_start,
            cycle,
            phase_at_start,
        }))
    }

    fn root_position(&self, frame: u64) -> Option<u64> {
        self.destination.contains(&frame).then(|| {
            let elapsed = frame - self.destination.start;
            let offset =
                (u128::from(self.phase_at_start) + u128::from(elapsed)) % u128::from(self.cycle);
            self.root_start + offset as u64
        })
    }
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
        Self::mapped(clip, tempo, asset, overrides, PreviewSource::Asset)
    }

    /// Resolve coordinates against the accepted pre-event summary of this
    /// single-source frame clip. The caller supplies its root extent, with the
    /// source offset, phase and source processing already realized.
    pub fn with_prepared_source(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        asset: &Asset,
        overrides: FrameFadeOverrides<'a>,
        source_frames: u64,
    ) -> Result<Option<Self>, Invalid> {
        Self::mapped(
            clip,
            tempo,
            asset,
            overrides,
            PreviewSource::Prepared(source_frames),
        )
    }

    /// Query a grouped frame clip. Bind the returned child IDs to the accepted
    /// group table. Preview overrides can change event amplitude within that table.
    pub fn with_grouped_sources(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        asset: &Asset,
        overrides: FrameFadeOverrides<'a>,
    ) -> Result<Option<Self>, Invalid> {
        Self::mapped(clip, tempo, asset, overrides, PreviewSource::Group)
    }

    fn mapped(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        asset: &Asset,
        overrides: FrameFadeOverrides<'a>,
        source: PreviewSource,
    ) -> Result<Option<Self>, Invalid> {
        if clip.asset() != asset.id() {
            return Err(Invalid("fade preview asset differs from clip"));
        }
        let asset_frames = asset.playback_frames();
        let grouped = matches!(source, PreviewSource::Group);
        if grouped != clip.has_retained_audio_roots() {
            return Ok(None);
        }
        let prepared_frames = match source {
            PreviewSource::Prepared(frames) => Some(frames),
            _ => None,
        };
        let projection = if let Some(edit) = clip.audio_edit() {
            let supported_source = if let Some(frames) = prepared_frames {
                frames == edit.domain.clock.length()
                    && super::previews_transformed_frame_events(edit)
            } else {
                !edit.domain.warp_enabled
                    && edit.domain.pitch_cents == 0
                    && edit.domain.source_gain.is_empty()
                    && edit.processing_projection.is_none()
            };
            if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
                || !matches!(edit.timing, AudioClipTiming::Frames)
                || !supported_source
                || edit.domain.musical_event_source.is_some()
            {
                return Ok(None);
            }
            edit.validate(clip, tempo, asset_frames)
                .map_err(|error| Invalid(error.0))?;
            Some(AudioViewProjection::new(clip, tempo).map_err(|error| Invalid(error.0))?)
        } else {
            if prepared_frames.is_some() {
                return Ok(None);
            }
            if clip.duration() == 0
                || clip.source_length() == 0
                || clip
                    .source_offset()
                    .checked_add(clip.source_length())
                    .is_none_or(|end| end > asset_frames)
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
        let outer_fade = if projection.is_none() {
            None
        } else if let Some(fade) = overrides.fade.filter(|_| !overrides.legacy_root_fade) {
            Some(
                clip.projected_outer_fade_preview(tempo, fade)
                    .map_err(|error| Invalid(error.0))?,
            )
        } else {
            clip.projected_outer_fade(tempo)
                .map_err(|error| Invalid(error.0))?
        };
        let directed_legacy = if let Some(projection) = &projection
            && outer_fade.is_none()
            && clip
                .audio_edit()
                .is_some_and(|edit| !overrides.fade.unwrap_or(edit.domain.fade).is_neutral())
        {
            DirectedLegacyFade::new(projection)?
        } else {
            None
        };
        let events = overrides.events.or_else(|| {
            clip.audio_edit()
                .and_then(|edit| edit.domain.events.as_deref())
        });
        if let Some(events) = events {
            let canonical = clip
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_deref());
            if grouped == events.sources.is_empty()
                || (grouped && canonical.is_none_or(|old| old.sources != events.sources))
            {
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
        let source = if let Some(length) = prepared_frames {
            SourceRead {
                offset: 0,
                length,
                phase: 0,
                looping: false,
            }
        } else {
            SourceRead {
                offset: clip.source_offset(),
                length: clip.source_length(),
                phase: clip.source_phase(),
                looping: clip.looping(),
            }
        };
        Ok(Some(Self {
            clip,
            tempo,
            projection,
            outer_fade,
            directed_legacy,
            source_frames: prepared_frames.unwrap_or(asset_frames),
            source,
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
        if self
            .directed_legacy
            .as_ref()
            .is_some_and(|fade| !fade.destination.contains(&frame))
        {
            return Ok(None);
        }
        if let Some(projection) = &self.projection {
            let Some(root) = projection
                .root_position_for_frame(frame)
                .map_err(|error| Invalid(error.0))?
            else {
                return Ok(None);
            };
            let Some(mut sample) = self.root_sample(root)? else {
                return Ok(None);
            };
            if let Some(gain) = self.timeline_gain(frame)? {
                sample.gains[2] = gain;
            }
            Ok(Some(sample))
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

    fn timeline_gain(&self, frame: u64) -> Result<Option<f32>, Invalid> {
        if let Some(fade) = &self.directed_legacy {
            let root = fade.root_position(frame).ok_or(Invalid(
                "fade preview frame is outside its directed envelope",
            ))?;
            return self.root_fade_gain(root).map(Some);
        }
        self.outer_fade
            .as_ref()
            .map(|fade| fade.gain_at(frame))
            .transpose()
            .map_err(|error| Invalid(error.0))
    }

    fn root_fade_gain(&self, frame: u64) -> Result<f32, Invalid> {
        if self.outer_fade.is_some() {
            Ok(1.0)
        } else if let Some(fade) = self.overrides.fade {
            let start = self.clip.audio_edit().map_or(0, |edit| edit.view_start);
            Ok(frame
                .checked_sub(start)
                .map_or(0.0, |age| fade.gain_at(age)))
        } else if let Some(edit) = self.clip.audio_edit() {
            edit.projected_fade(self.tempo)
                .and_then(|fade| fade.gain_at(frame))
                .map_err(|error| Invalid(error.0))
        } else {
            Ok(self.clip.fade().gain_at(frame))
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
        let clip_fade = self.root_fade_gain(frame)?;
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
        let gains = [event_fade, event_gain, clip_fade, self.clip_gain];
        if let Some(event) = event.filter(|event| event.source != 0) {
            // The validated event interval is already in the compiled child's
            // output clock, including its own crop, phase and direction.
            return Ok(Some(FrameFadeSample {
                source_frame: read,
                source_id: event.source,
                event: Some(event.id),
                gains,
            }));
        }
        self.source_sample(read, event.map(|event| event.id), gains)
    }
    fn source_sample(
        &self,
        read: u64,
        event: Option<u64>,
        gains: [f32; 4],
    ) -> Result<Option<FrameFadeSample>, Invalid> {
        let local = if self.source.looping {
            ((u128::from(self.source.phase) + u128::from(read)) % u128::from(self.source.length))
                as u64
        } else if read < self.source.length {
            read
        } else {
            return Ok(None);
        };
        let source_frame = self
            .source
            .offset
            .checked_add(local)
            .filter(|frame| *frame < self.source_frames)
            .ok_or(Invalid("fade preview sample exceeds source"))?;
        Ok(Some(FrameFadeSample {
            source_frame,
            source_id: 0,
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
    /// (`event.source_start + age`). Returned `source_frame()` addresses the
    /// bound asset or the already transformed processing root.
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
