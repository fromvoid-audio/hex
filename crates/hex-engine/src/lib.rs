//! Deterministic, sample-rate-locked stereo arrangement rendering.
//! This is an original prototype, not a verified Live-compatible engine.
pub use hex_devices::SineTable;
use std::{fmt, sync::Arc};

mod native_device_view;
pub use native_device_view::{
    NativeDeviceCatalogEntry, NativeDeviceParameterView, NativeDeviceView, native_device_catalog,
    native_device_view,
};

mod streamed;
pub use streamed::{
    BorrowedMediaPage, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
};

pub type Frame = [f32; 2];
/// Maximum stable track routes exposed to native callback telemetry.
pub const MAX_SESSION_ROUTE_TRACKS: usize = hex_project::MAX_TRACKS;

/// Add one contiguous streamed source span, split at its page boundary. Both
/// Arrangement clips and Session one-shots use this path so page validation,
/// short-page silence, and bounded miss requests have one implementation.
#[derive(Clone, Copy)]
pub(crate) struct StreamedPageSpan<'a> {
    pub source_frame: u64,
    pub timeline_frame: u64,
    pub output_offset: usize,
    pub count: usize,
    pub fade: Option<(&'a hex_project::ClipFade, u64)>,
}

pub(crate) fn render_streamed_page_span(
    mix: PreparedTrackMix,
    source: &StreamedSourceMetadata,
    span: StreamedPageSpan<'_>,
    output: &mut [Frame],
    pages: &mut (impl MediaPages + ?Sized),
) -> usize {
    let StreamedPageSpan {
        source_frame,
        timeline_frame,
        output_offset,
        count: requested_count,
        fade,
    } = span;
    if requested_count == 0 {
        return 0;
    }
    let page_index = source_frame / STREAM_PAGE_FRAMES as u64;
    let page_offset = (source_frame % STREAM_PAGE_FRAMES as u64) as usize;
    let count = requested_count.min(STREAM_PAGE_FRAMES - page_offset);
    let Some(output_end) = output_offset.checked_add(count) else {
        return 0;
    };
    let Some(destination) = output.get_mut(output_offset..output_end) else {
        return 0;
    };
    let mut incomplete = true;
    if let Some(page) = pages.page(source.key(), page_index) {
        let valid = page.page_index == page_index;
        let available = if valid {
            page.frames.len().saturating_sub(page_offset).min(count)
        } else {
            0
        };
        for (offset, destination) in destination.iter_mut().take(available).enumerate() {
            let mut sample = page.frames[page_offset + offset];
            if let Some((fade, fade_start)) = fade {
                if let Some(frame) = timeline_frame.checked_add(offset as u64) {
                    let gain = fade.gain_at(frame.saturating_sub(fade_start));
                    sample[0] *= gain;
                    sample[1] *= gain;
                }
            }
            mix.add_unmuted(destination, sample);
        }
        incomplete = !valid || available < count;
    }
    if incomplete {
        pages.request_page(source.key(), page_index);
    }
    count
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Invalid {}

pub fn default_native_device_state(
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    registry
        .default_state(type_id)
        .map_err(|_| Invalid("native device default state is invalid"))
}

pub fn patch_native_device_parameter(
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
    value: hex_lattice_contract::ParameterValue,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    registry
        .patch_parameter(state, parameter, value)
        .map_err(|error| {
            use hex_lattice_contract::DeviceContractError as Error;
            Invalid(match error {
                Error::InvalidId => "device parameter id is invalid",
                Error::InvalidStateVersion => "device state version is invalid",
                Error::StateTooLarge => "native device state exceeds its size limit",
                Error::WrongDeviceType => "device state has the wrong type",
                Error::UnsupportedStateVersion => "device state version is unsupported",
                Error::MalformedState => "native device state is malformed",
                Error::UnknownParameter => "device parameter is unknown",
                Error::InvalidParameterValue => "device parameter value is invalid",
                Error::InvalidDescriptor => "native device parameter descriptor is invalid",
                Error::DuplicateDeviceType => "native device registry contains a duplicate type",
            })
        })
}

pub fn native_device_parameter(
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
) -> Result<hex_lattice_contract::ParameterValue, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    let definition = registry
        .definition(state.type_id())
        .ok_or(Invalid("native device type is unavailable"))?;
    let migrated = registry
        .migrate_state(state)
        .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
    definition
        .read_parameter(&migrated, parameter)
        .map_err(|_| Invalid("native device parameter is unavailable"))
}

pub fn native_device_descriptors(
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<&'static [hex_lattice_contract::ParameterDescriptor], Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    registry
        .definition(type_id)
        .map(|definition| definition.descriptors())
        .ok_or(Invalid("native device type is unavailable"))
}

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
    sample_rate: u32,
    frames: Arc<[Frame]>,
}
impl Audio {
    /// Source rate; compilation currently requires an exact project-rate match.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Shared immutable stereo PCM, available for import validation and analysis.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
    /// True when two audio handles retain the same immutable PCM allocation.
    pub(crate) fn shares_pcm_with(&self, other: &Self) -> bool {
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
            frames: frames.into(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Clip {
    audio: Option<Audio>,
    start: u64,
    end: u64,
    looping: bool,
    source_offset: usize,
    source_length: usize,
    metadata: Option<Box<ClipMetadata>>,
}

/// Control-owned uncommon playback state; neutral clips keep their compact stride.
#[derive(Debug, Clone)]
struct ClipMetadata {
    source_phase: u64,
    fade: Option<hex_project::ClipFade>,
    streamed: Option<Arc<StreamedSourceMetadata>>,
}
impl Clip {
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
        let metadata = if fade.fade_in() == 0 && fade.fade_out() == 0 {
            None
        } else {
            Some(Box::new(ClipMetadata {
                source_phase: 0,
                fade: Some(fade),
                streamed: None,
            }))
        };
        Ok(Self {
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
            if phase == 0 && metadata.fade.is_none() && metadata.streamed.is_none() {
                self.metadata = None;
            }
        } else if phase != 0 {
            self.metadata = Some(Box::new(ClipMetadata {
                source_phase: phase,
                fade: None,
                streamed: None,
            }));
        }
        Ok(self)
    }
    fn at(&self, position: u64, source_phase: u64) -> Frame {
        if position < self.start || position >= self.end {
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
            audio.frames[self.source_offset + source_offset as usize]
        })
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
            audio: None,
            start,
            end,
            looping,
            source_offset: usize::try_from(source_offset)
                .map_err(|_| Invalid("source offset exceeds addressable range"))?,
            source_length: usize::try_from(source_length)
                .map_err(|_| Invalid("source length exceeds addressable range"))?,
            metadata: Some(Box::new(ClipMetadata {
                source_phase: 0,
                fade: (fade.fade_in() != 0 || fade.fade_out() != 0).then_some(fade),
                streamed: Some(source),
            })),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Track {
    track_id: Option<hex_project::TrackId>,
    clips: Vec<Clip>,
    mix: PreparedTrackMix,
    instrument: Option<InstrumentTrack>,
}

/// Validated per-track stereo coefficients shared by independent source paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedTrackMix {
    channel_gain: Frame,
    effects: Option<hex_lattice::StereoMatrix>,
    muted: bool,
}
impl PreparedTrackMix {
    pub fn new(gain: f32, balance: f32, muted: bool) -> Result<Self, Invalid> {
        Self::with_effects(gain, balance, muted, None)
    }

    pub(crate) fn with_effects(
        gain: f32,
        balance: f32,
        muted: bool,
        effects: Option<hex_lattice::StereoMatrix>,
    ) -> Result<Self, Invalid> {
        if !gain.is_finite() || gain < 0.0 {
            return Err(Invalid("gain must be finite and nonnegative"));
        }
        if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
            return Err(Invalid("balance must be finite and within -1..1"));
        }
        Ok(Self {
            channel_gain: [
                gain * (1.0 - balance.max(0.0)),
                gain * (1.0 + balance.min(0.0)),
            ],
            effects: effects.filter(|matrix| matrix.coefficients() != [[1.0, 0.0], [0.0, 1.0]]),
            muted,
        })
    }

    pub fn channel_gain(self) -> Frame {
        self.channel_gain
    }

    pub fn muted(self) -> bool {
        self.muted
    }

    /// Add one source frame with the prepared per-channel coefficients.
    pub fn add_frame(self, destination: &mut Frame, source: Frame) {
        if !self.muted {
            self.add_unmuted(destination, source);
        }
    }

    fn add_unmuted(self, destination: &mut Frame, source: Frame) {
        let source = self.effects.map_or(source, |matrix| matrix.apply(source));
        destination[0] += source[0] * self.channel_gain[0];
        destination[1] += source[1] * self.channel_gain[1];
    }
}

impl Track {
    /// Overlapping clips are summed in this prototype; Live overlap semantics are unverified.
    pub fn new(clips: Vec<Clip>, gain: f32, muted: bool) -> Result<Self, Invalid> {
        Self::with_balance(clips, gain, muted, 0.0)
    }
    /// Prepare validated original Hex balance coefficients for rendering.
    pub fn with_balance(
        clips: Vec<Clip>,
        gain: f32,
        muted: bool,
        balance: f32,
    ) -> Result<Self, Invalid> {
        Ok(Self {
            track_id: None,
            clips,
            mix: PreparedTrackMix::new(gain, balance, muted)?,
            instrument: None,
        })
    }

    pub(crate) fn with_audio_instrument_and_effects(
        clips: Vec<Clip>,
        gain: f32,
        muted: bool,
        balance: f32,
        instrument: Option<InstrumentTrack>,
        effects: Option<hex_lattice::StereoMatrix>,
    ) -> Result<Self, Invalid> {
        Ok(Self {
            track_id: None,
            clips,
            mix: PreparedTrackMix::with_effects(gain, balance, muted, effects)?,
            instrument,
        })
    }

    pub fn mix(&self) -> PreparedTrackMix {
        self.mix
    }

    pub(crate) fn has_instrument_source(&self) -> bool {
        self.instrument.is_some()
    }

    pub(crate) fn with_track_id(mut self, track: hex_project::TrackId) -> Self {
        self.track_id = Some(track);
        self
    }

    pub(crate) fn canonical_track_id(&self) -> Option<hex_project::TrackId> {
        self.track_id
    }

    /// Add only this track's finite Arrangement source to an existing mix buffer.
    /// `start_frame` anchors output[0]; transport is not changed. The range end is
    /// checked before any samples are added, so overflow leaves output untouched.
    pub fn render_additive(&self, start_frame: u64, output: &mut [Frame]) -> Result<(), Invalid> {
        if self.clips.iter().any(Clip::is_streamed) {
            return Err(Invalid("streamed clips require a media page provider"));
        }
        self.render_additive_with_midi_floor(start_frame, output, None)
    }

    pub(crate) fn render_additive_with_midi_floor(
        &self,
        start_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        if !self.mix.muted {
            self.render_unmuted_range(start_frame, end_frame, output, midi_onset_floor_exclusive);
        }
        Ok(())
    }

    pub(crate) fn render_additive_with_pages(
        &self,
        start_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        if !self.mix.muted {
            self.render_unmuted_range_with_pages(start_frame, end_frame, output, None, pages);
        }
        Ok(())
    }

    pub(crate) fn for_each_active_midi_span(
        &self,
        frame: u64,
        mut visit: impl FnMut(&midi_compile::CompiledNoteSpan),
    ) -> Result<(), Invalid> {
        let end = frame
            .checked_add(1)
            .ok_or(Invalid("midi span frame overflow"))?;
        if let Some(instrument) = &self.instrument {
            instrument.for_each_intersecting(frame, end, None, |span| visit(span));
        }
        Ok(())
    }

    fn render_unmuted_range(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
    ) {
        for clip in &self.clips {
            if !clip.is_streamed() {
                self.render_resident_clip(clip, start_frame, end_frame, output);
            }
        }
        self.render_instrument_range(start_frame, end_frame, output, midi_onset_floor_exclusive);
    }

    fn render_unmuted_range_with_pages(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        pages: &mut (impl MediaPages + ?Sized),
    ) {
        for clip in &self.clips {
            if !clip.is_streamed() {
                self.render_resident_clip(clip, start_frame, end_frame, output);
                continue;
            }
            let Some(metadata) = clip.metadata.as_deref() else {
                continue;
            };
            let Some(source) = metadata.streamed.as_deref() else {
                continue;
            };
            let span_end = if clip.looping {
                end_frame.min(clip.end)
            } else {
                end_frame
                    .min(clip.end)
                    .min(clip.start.saturating_add(clip.source_length as u64))
            };
            let mut position = start_frame.max(clip.start);
            while position < span_end {
                let timeline_offset = position - clip.start;
                let relative = if clip.looping {
                    timeline_offset % clip.source_length as u64
                } else {
                    timeline_offset
                };
                let source_relative = if metadata.source_phase == 0 {
                    relative
                } else {
                    let until_wrap = clip.source_length as u64 - metadata.source_phase;
                    if relative >= until_wrap {
                        relative - until_wrap
                    } else {
                        relative + metadata.source_phase
                    }
                };
                let Some(source_frame) = (clip.source_offset as u64).checked_add(source_relative)
                else {
                    break;
                };
                let remaining = (span_end - position) as usize;
                let until_wrap = if clip.looping {
                    (clip.source_length as u64 - source_relative) as usize
                } else {
                    usize::MAX
                };
                let count = remaining.min(until_wrap);
                let output_offset = (position - start_frame) as usize;
                let rendered = render_streamed_page_span(
                    self.mix,
                    source,
                    StreamedPageSpan {
                        source_frame,
                        timeline_frame: position,
                        output_offset,
                        count,
                        fade: metadata.fade.as_ref().map(|fade| (fade, clip.start)),
                    },
                    output,
                    pages,
                );
                if rendered == 0 {
                    break;
                }
                position = position.saturating_add(rendered as u64);
            }
        }
        self.render_instrument_range(start_frame, end_frame, output, midi_onset_floor_exclusive);
    }

    fn render_resident_clip(
        &self,
        clip: &Clip,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
    ) {
        let mut span_end = end_frame.min(clip.end);
        if !clip.looping {
            span_end = span_end.min(clip.start.saturating_add(clip.source_length as u64));
        }
        let span_start = start_frame.max(clip.start);
        if span_start >= span_end {
            return;
        }
        let output_start = (span_start - start_frame) as usize;
        let output_end = (span_end - start_frame) as usize;
        let metadata = clip.metadata.as_deref();
        let source_phase = metadata.map_or(0, |metadata| metadata.source_phase);
        if let Some(fade) = metadata.and_then(|metadata| metadata.fade.as_ref()) {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                let position = span_start + offset as u64;
                let mut sample = clip.at(position, source_phase);
                let gain = fade.gain_at(position - clip.start);
                sample[0] *= gain;
                sample[1] *= gain;
                self.mix.add_unmuted(frame, sample);
            }
        } else if source_phase == 0 {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                self.mix
                    .add_unmuted(frame, clip.at(span_start + offset as u64, 0));
            }
        } else {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                self.mix
                    .add_unmuted(frame, clip.at(span_start + offset as u64, source_phase));
            }
        }
    }

    fn render_instrument_range(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
    ) {
        if let Some(instrument) = &self.instrument {
            for chunk_start in (start_frame..end_frame).step_by(256) {
                let chunk_end = chunk_start.saturating_add(256).min(end_frame);
                let output_start = (chunk_start - start_frame) as usize;
                let output_end = (chunk_end - start_frame) as usize;
                let target = &mut output[output_start..output_end];
                instrument.for_each_intersecting(
                    chunk_start,
                    chunk_end,
                    midi_onset_floor_exclusive,
                    |span| {
                        let start = span.start.max(chunk_start);
                        let end = span.end.min(chunk_end);
                        for frame_position in start..end {
                            let relative = frame_position - span.start;
                            let sample = instrument.table.sample(
                                span.voice,
                                relative,
                                span.gate_end - span.start,
                            );
                            self.mix.add_unmuted(
                                &mut target[(frame_position - chunk_start) as usize],
                                [sample; 2],
                            );
                        }
                    },
                );
            }
        }
    }
}

impl Clip {
    fn is_streamed(&self) -> bool {
        self.metadata
            .as_deref()
            .is_some_and(|metadata| metadata.streamed.is_some())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InstrumentTrack {
    pub(crate) spans: Vec<midi_compile::CompiledNoteSpan>,
    pub(crate) nodes: Vec<midi_compile::SpanNode>,
    pub(crate) root: Option<usize>,
    pub(crate) table: Arc<SineTable>,
    pub(crate) timeline_end: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Project {
    sample_rate: u32,
    tracks: Vec<Track>,
    loop_region: Option<hex_project::LoopRegion>,
    master_gain: PreparedMasterGain,
    audible_end_frame: Option<u64>,
    has_streamed_sources: bool,
}
impl Project {
    pub fn new(sample_rate: u32, tracks: Vec<Track>) -> Result<Self, Invalid> {
        Self::with_loop_region(sample_rate, tracks, None)
    }
    /// Compile an arrangement and its optional canonical playback loop region.
    pub fn with_loop_region(
        sample_rate: u32,
        tracks: Vec<Track>,
        loop_region: Option<hex_project::LoopRegion>,
    ) -> Result<Self, Invalid> {
        Self::with_master_gain(sample_rate, tracks, loop_region, 1.0)
    }
    /// Compile an arrangement with the validated linear master output gain.
    pub fn with_master_gain(
        sample_rate: u32,
        tracks: Vec<Track>,
        loop_region: Option<hex_project::LoopRegion>,
        master_gain: f32,
    ) -> Result<Self, Invalid> {
        if sample_rate == 0 {
            return Err(Invalid("sample rate must be positive"));
        }
        let master_gain = PreparedMasterGain::new(master_gain)?;
        let has_streamed_sources = tracks.iter().any(|track| {
            track.clips.iter().any(|clip| {
                clip.metadata
                    .as_deref()
                    .is_some_and(|metadata| metadata.streamed.is_some())
            })
        });
        if tracks.iter().flat_map(|t| &t.clips).any(|clip| {
            clip.audio
                .as_ref()
                .is_some_and(|audio| audio.sample_rate != sample_rate)
                || clip
                    .metadata
                    .as_deref()
                    .and_then(|metadata| metadata.streamed.as_deref())
                    .is_some_and(|source| source.project_rate() != sample_rate)
        }) {
            return Err(Invalid(
                "prepared audio sources must match the project rate",
            ));
        }
        Ok(Self {
            audible_end_frame: tracks
                .iter()
                .flat_map(|track| track.clips.iter().map(|clip| clip.end))
                .max(),
            sample_rate,
            tracks,
            loop_region,
            master_gain,
            has_streamed_sources,
        })
    }

    pub(crate) fn with_compiled_tracks(
        sample_rate: u32,
        tracks: Vec<Track>,
        loop_region: Option<hex_project::LoopRegion>,
        master_gain: f32,
    ) -> Result<Self, Invalid> {
        let mut project = Self::with_master_gain(sample_rate, tracks, loop_region, master_gain)?;
        project.audible_end_frame = project
            .tracks
            .iter()
            .flat_map(|track| {
                track
                    .clips
                    .iter()
                    .map(|clip| clip.end)
                    .chain(track.instrument.iter().flat_map(|instrument| {
                        instrument
                            .timeline_end
                            .into_iter()
                            .chain(instrument.spans.iter().map(|span| span.end))
                    }))
            })
            .max();
        Ok(project)
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn loop_region(&self) -> Option<hex_project::LoopRegion> {
        self.loop_region
    }
    /// Exclusive end of all compiled audio and instrument output, if any.
    pub fn audible_end_frame(&self) -> Option<u64> {
        self.audible_end_frame
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Whether any clip needs streamed pages to produce audio.
    pub fn contains_streamed_sources(&self) -> bool {
        self.has_streamed_sources
    }

    pub fn master_mix(&self) -> PreparedMasterGain {
        self.master_gain
    }
}

/// Validated post-sum master stage, reusable by other track-source mixes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedMasterGain(f32);
impl PreparedMasterGain {
    pub fn new(gain: f32) -> Result<Self, Invalid> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(Invalid("master gain must be finite and within 0..2"));
        }
        Ok(Self(gain))
    }

    pub fn value(self) -> f32 {
        self.0
    }

    /// Apply after all track contributions have been accumulated.
    pub fn apply(self, output: &mut [Frame]) {
        if self.0 != 1.0 {
            for frame in output {
                frame[0] *= self.0;
                frame[1] *= self.0;
            }
        }
    }
}

pub struct Renderer<'a> {
    project: &'a Project,
    position: u64,
    playing: bool,
}
impl<'a> Renderer<'a> {
    pub fn new(project: &'a Project) -> Self {
        Self {
            project,
            position: 0,
            playing: false,
        }
    }
    pub fn play(&mut self) {
        self.playing = true;
    }
    pub fn pause(&mut self) {
        self.playing = false;
    }
    pub fn seek(&mut self, position: u64) {
        self.position = position;
    }
    pub fn position(&self) -> u64 {
        self.position
    }
    /// Overwrites the output. Pausing emits silence without advancing.
    /// No allocation occurs here; real-time deadline safety is not yet established.
    /// On position overflow returns an error without changing output or transport.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        let next = if self.playing {
            self.position
                .checked_add(output.len() as u64)
                .ok_or(Invalid("transport overflow"))?
        } else {
            self.position
        };
        if self.playing && !output.is_empty() && self.project.contains_streamed_sources() {
            return Err(Invalid("streamed project requires a media page provider"));
        }
        output.fill([0.0; 2]);
        if !self.playing {
            return Ok(());
        }
        if output.is_empty() {
            return Ok(());
        }
        let block_start = self.position;
        for track in &self.project.tracks {
            if !track.mix.muted {
                track.render_unmuted_range(block_start, next, output, None);
            }
        }
        // Neutral projects retain their original hot path. Non-neutral master
        // gain is deliberately post-sum rather than folded into track gains.
        self.project.master_gain.apply(output);
        self.position = next;
        Ok(())
    }

    /// Render with a borrowed page view. Missing streamed pages contribute
    /// silence while transport and other source contributions continue.
    pub fn render_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        let next = if self.playing {
            self.position
                .checked_add(output.len() as u64)
                .ok_or(Invalid("transport overflow"))?
        } else {
            self.position
        };
        output.fill([0.0; 2]);
        if !self.playing || output.is_empty() {
            return Ok(());
        }
        let block_start = self.position;
        for track in &self.project.tracks {
            if !track.mix.muted {
                track.render_unmuted_range_with_pages(block_start, next, output, None, pages);
            }
        }
        self.project.master_gain.apply(output);
        self.position = next;
        Ok(())
    }
}

impl InstrumentTrack {
    fn for_each_intersecting(
        &self,
        start: u64,
        end: u64,
        minimum_start_exclusive: Option<u64>,
        mut visit: impl FnMut(&midi_compile::CompiledNoteSpan),
    ) {
        let Some(root) = self.root else { return };
        let mut stack = [(0_usize, false); 64];
        let mut size = 1;
        stack[0] = (root, false);
        while size > 0 {
            size -= 1;
            let (index, emit) = stack[size];
            let node = &self.nodes[index];
            if emit {
                let span = &self.spans[node.span_index];
                if span.start < end
                    && span.end > start
                    && minimum_start_exclusive.is_none_or(|floor| span.start > floor)
                {
                    visit(span);
                }
                continue;
            }
            // A median onset beyond the query end only rules out this node and
            // its right subtree. Earlier left-subtree spans may still be active.
            if node.max_end > start {
                if let Some(right) = node.right {
                    if node.span_start < end && self.nodes[right].max_end > start {
                        stack[size] = (right, false);
                        size += 1;
                    }
                }
                if node.span_start < end && self.spans[node.span_index].end > start {
                    stack[size] = (index, true);
                    size += 1;
                }
                if let Some(left) = node.left {
                    if self.nodes[left].max_end > start {
                        stack[size] = (left, false);
                        size += 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project(looping: bool, duration: u64) -> Project {
        let audio = Audio::new(48_000, vec![[1.0, -1.0], [0.5, -0.5]]).unwrap();
        Project::new(
            48_000,
            vec![
                Track::new(
                    vec![Clip::new(audio, 2, duration, looping).unwrap()],
                    0.5,
                    false,
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }
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
    fn scheduled_loop_has_exact_boundaries() {
        let p = project(true, 5);
        let mut r = Renderer::new(&p);
        r.play();
        let mut out = [[9.0; 2]; 8];
        r.render(&mut out).unwrap();
        assert_eq!(
            out,
            [
                [0.0; 2],
                [0.0; 2],
                [0.5, -0.5],
                [0.25, -0.25],
                [0.5, -0.5],
                [0.25, -0.25],
                [0.5, -0.5],
                [0.0; 2]
            ]
        );
    }
    #[test]
    fn one_shot_exhausts_source() {
        let p = project(false, 5);
        let mut r = Renderer::new(&p);
        r.play();
        let mut out = [[0.0; 2]; 8];
        r.render(&mut out).unwrap();
        assert_eq!(out[3], [0.25, -0.25]);
        assert_eq!(out[4..], [[0.0; 2]; 4]);
    }
    #[test]
    fn block_partition_is_invariant() {
        let p = project(true, 20);
        let mut a = Renderer::new(&p);
        let mut b = Renderer::new(&p);
        a.play();
        b.play();
        let mut whole = [[0.0; 2]; 23];
        let mut chunks = whole;
        a.render(&mut whole).unwrap();
        for chunk in chunks.chunks_mut(3) {
            b.render(chunk).unwrap();
        }
        assert_eq!(whole, chunks);
        assert_eq!(a.position(), b.position());
    }
    #[test]
    fn pause_and_seek() {
        let p = project(true, 5);
        let mut r = Renderer::new(&p);
        r.seek(3);
        let mut out = [[9.0; 2]; 1];
        r.render(&mut out).unwrap();
        assert_eq!(out, [[0.0; 2]]);
        assert_eq!(r.position(), 3);
        r.play();
        r.render(&mut out).unwrap();
        assert_eq!(out, [[0.25, -0.25]]);
        r.pause();
        r.render(&mut out).unwrap();
        assert_eq!(r.position(), 4);
    }
    #[test]
    fn mixing_and_mute() {
        let a = Audio::new(48_000, vec![[0.5, -0.5]]).unwrap();
        let c = Clip::new(a, 0, 1, false).unwrap();
        let tracks = vec![
            Track::new(vec![c.clone()], 1.0, false).unwrap(),
            Track::new(vec![c.clone()], 0.5, false).unwrap(),
            Track::new(vec![c], 9.0, true).unwrap(),
        ];
        let p = Project::new(48_000, tracks).unwrap();
        let mut r = Renderer::new(&p);
        r.play();
        let mut out = [[0.0; 2]; 1];
        r.render(&mut out).unwrap();
        assert_eq!(out, [[0.75, -0.75]]);
    }
    #[test]
    fn rejects_bad_sources_and_overflow() {
        assert!(Audio::new(0, vec![[0.0; 2]]).is_err());
        assert!(Audio::new(48_000, vec![[f32::INFINITY; 2]]).is_err());
        let a = Audio::new(44_100, vec![[0.0; 2]]).unwrap();
        assert!(Clip::new(a.clone(), u64::MAX, 1, false).is_err());
        let t = Track::new(vec![Clip::new(a, 0, 1, false).unwrap()], 1.0, false).unwrap();
        assert!(Project::new(48_000, vec![t]).is_err());
        let p = project(true, 5);
        let mut r = Renderer::new(&p);
        r.seek(u64::MAX);
        r.play();
        let mut out = [[9.0; 2]; 1];
        assert!(r.render(&mut out).is_err());
        assert_eq!(r.position(), u64::MAX);
        assert_eq!(out, [[9.0; 2]]);
    }
}

pub mod live;
pub mod midi_compile;
mod midi_preview;
pub mod midi_template;
mod prepared_revision;
mod prepared_tempo;
mod session_inventory;
pub mod session_playback;

pub mod compile;

pub mod playback;
pub use midi_preview::{
    MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP, MidiArrangementPreview, MidiClipPreview, MidiPreviewSample,
};
pub use midi_template::{
    MIDI_SPAN_BUFFER_CAPACITY, MIDI_TEMPLATE_MAX_EDGES, MIDI_TEMPLATE_MAX_ONSETS,
    MIDI_TEMPLATE_QUERY_FRAMES, MidiEdgeBuffer, MidiInstrumentRenderContext, MidiRenderSpan,
    MidiSpanBuffer, MidiTemplateError, PreparedMidiInstrumentTemplate, PreparedMidiSourceTemplate,
};
pub use playback::{
    PlaybackAction, PlaybackActionError, PlaybackActionRejection, PlaybackCommand,
    PlaybackHandoffError, PlaybackHandoffRejection, SessionCapability, SessionCapabilityStatus,
};
pub use prepared_revision::PreparedRevision;
pub use prepared_tempo::{
    MAX_PREPARED_TEMPO_POINTS, MidiTiming, PreparedTempoError, PreparedTempoMap,
};
pub use session_inventory::{
    PreparedSessionAudioClip, PreparedSessionCell, PreparedSessionCellContent,
    PreparedSessionInventory, PreparedSessionMidiClip, PreparedSessionScene, PreparedSessionTrack,
    SessionWorkEnvelope,
};
pub use session_playback::{
    LaunchQuantization, SessionHandoffReport, SessionPendingRouteStatus, SessionPlayback,
    SessionPlaybackCommand, SessionPlaybackError, SessionPlaybackOwner, SessionRouteStatus,
    SessionRouteTarget,
};
