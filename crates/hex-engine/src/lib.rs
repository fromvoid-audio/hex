//! Deterministic, sample-rate-locked stereo arrangement rendering.
//! This is an original prototype, not a verified Live-compatible engine.
pub use hex_devices::SineTable;
pub use hex_devices::compressor::{
    CompressorTransferCurve, transfer_curve as native_compressor_transfer_curve,
};
pub use hex_devices::saturator::{
    SaturatorTransferCurve, transfer_curve as native_saturator_transfer_curve,
};
pub use hex_lattice::ProcessorAuditionSelection;
pub use hex_lattice::ProcessorChainOwner;
pub use hex_lattice_contract::DeviceTypeId;
pub mod compensation;

use std::{fmt, sync::Arc};

mod audio_candidate;
mod audio_storage;
pub use audio_candidate::{
    AudioCandidateAnchors, AudioCandidateConstructorClaims, AudioCandidateConstructorSummary,
    AudioCandidateDisposalPhase, AudioCandidateDisposed, AudioCandidateIdentity,
    AudioCandidateOwner, AudioCandidateRefusal, AudioCandidateRuntimeDisposed,
    AudioCandidateSplitRefusal, AudioCandidateTransfer, ClaimedAudioCandidateRuntime,
};
pub use audio_storage::{
    AudioRootStorageLedger, AudioRootStorageSummary, AudioStorageIncomplete,
    AudioStorageLedgerReservation, AudioStorageRole, StreamedSourceBindingObligation,
    audio_storage_constructor_layout_supported,
};
mod audio_input_storage;
mod audio_only_inputs;
mod audio_preparation;
mod session_audio_preparation;
mod session_shared_preparation;
mod session_structure_preparation;
pub use session_shared_preparation::SessionSharedCapacity;
mod session_tempo_preparation;
pub use audio_preparation::{
    ArrangementClipCapacity, ArrangementReturnCapacity, ArrangementReturnMetadata,
    ArrangementRootCapacity, ArrangementTrackCapacity, ArrangementTrackMetadata,
    AudioArrangementCapacity, AudioArrangementInput, AudioArrangementRefusal,
    AudioPreparationCustody, AudioPreparationPhase, AudioPreparationToken, ReturnFeedCapacity,
};
pub use session_audio_preparation::SessionSourceCapacity;
pub use session_inventory::audio_assembly::{SessionAssemblyCapacity, SessionInventoryMetadata};
pub use session_structure_preparation::SessionStructureCapacity;
pub use session_tempo_preparation::SessionTempoCapacity;
pub mod audio_transform;
mod native_device_view;
pub use audio_input_storage::{
    AudioInputStorageAndObligations, AudioInputStorageRefusal, AudioInputStorageSummary,
};
pub use audio_only_inputs::{
    AudioInputsRefusal, AudioPreparationCapacity, AudioSourceToken, AudioStorageError,
    AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource, OwnedAudioSourceView,
    ResidentSourceRefusal, StreamedSourceRefusal,
};
mod owned_shared;
mod pcm_arc;
pub use midi_preview::persistent::{AudioPreviewCapacity, AudioPreviewMetadata};
pub use native_device_view::{
    NativeDeviceCatalogEntry, NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView,
    native_device_catalog, native_device_catalog_for_class, native_device_descriptors_for_class,
    native_device_parameter_for_class, native_device_view, native_device_view_for_class,
    native_effect_frequency_response,
};

mod gain_envelope;
pub use gain_envelope::{
    GainAutomationCapacity, GainAutomationPreview, GainEnvelopeKnot, GainPreviewSegment,
    MAX_PREPARED_GAIN_BYTES, MAX_PREPARED_GAIN_KNOTS,
};
mod return_feed_plan;
mod signal_graph;
mod streamed;
pub use streamed::{
    BorrowedMediaPage, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
};

pub type Frame = [f32; 2];

/// Stable identity and post-node signal for an actually processed native
/// device. This is an observation-only engine boundary; it does not expose
/// mutable processor state or imply that retired tails are part of the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostNodeIdentity {
    pub owner: ProcessorChainOwner,
    pub device_id: u64,
    pub device_type: DeviceTypeId,
}

impl PostNodeIdentity {
    pub const fn new(
        owner: ProcessorChainOwner,
        device_id: u64,
        device_type: DeviceTypeId,
    ) -> Self {
        Self {
            owner,
            device_id,
            device_type,
        }
    }
}

pub use hex_lattice::DeviceMeter;
pub use native_device_view::{DEVICE_INPUT_GAIN, DEVICE_OUTPUT_GAIN};

/// Borrowed post-node samples delivered during one render call.
pub type PostNodeObserver<'observer> =
    dyn for<'frames> FnMut(PostNodeIdentity, usize, &'frames [Frame]) + 'observer;
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
    pub gain: f32,
    pub fade: Option<(&'a hex_project::ClipFade, u64)>,
}

pub(crate) fn render_streamed_page_span(
    mix: PreparedTrackMix,
    source: &StreamedSourceMetadata,
    span: StreamedPageSpan<'_>,
    output: &mut [Frame],
    pages: &mut (impl MediaPages + ?Sized),
) -> usize {
    render_streamed_page_span_captured(mix, source, span, output, pages, None)
}

pub(crate) fn render_streamed_page_span_captured(
    mix: PreparedTrackMix,
    source: &StreamedSourceMetadata,
    span: StreamedPageSpan<'_>,
    output: &mut [Frame],
    pages: &mut (impl MediaPages + ?Sized),
    mut capture: Option<&mut [Frame]>,
) -> usize {
    let StreamedPageSpan {
        source_frame,
        timeline_frame,
        output_offset,
        count: requested_count,
        fade,
        gain: clip_gain,
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
            if clip_gain != 1.0 {
                sample[0] *= clip_gain;
                sample[1] *= clip_gain;
            }
            mix.add_captured(
                destination,
                sample,
                capture
                    .as_deref_mut()
                    .and_then(|frames| frames.get_mut(output_offset + offset)),
            );
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
impl From<hex_project::Invalid> for Invalid {
    fn from(error: hex_project::Invalid) -> Self {
        Self(error.0)
    }
}

pub fn default_native_device_state(
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    default_native_device_state_for_class(NativeDeviceClass::Effect, type_id)
}

/// Construct a metadata-only default from exactly one native class registry.
pub fn default_native_device_state_for_class(
    class: NativeDeviceClass,
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    match class {
        NativeDeviceClass::Effect => hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?
            .default_state(type_id)
            .map_err(|_| Invalid("native device default state is invalid")),
        NativeDeviceClass::Instrument => hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?
            .default_state(type_id)
            .map_err(|_| Invalid("native instrument default state is invalid")),
    }
}

pub fn patch_native_device_parameter(
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
    value: hex_lattice_contract::ParameterValue,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    patch_native_device_parameter_for_class(NativeDeviceClass::Effect, state, parameter, value)
}

/// Patch a validated state through exactly one native class registry.
pub fn patch_native_device_parameter_for_class(
    class: NativeDeviceClass,
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
    value: hex_lattice_contract::ParameterValue,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    if parameter == native_device_view::DEVICE_INPUT_GAIN.id {
        native_device_view_for_class(class, state)?;
        let hex_lattice_contract::ParameterValue::Float(gain) = value else {
            return Err(Invalid("input gain must be a finite db value"));
        };
        return state
            .clone()
            .with_input_gain_db(gain)
            .map_err(map_device_contract_error);
    }
    if parameter == native_device_view::DEVICE_OUTPUT_GAIN.id {
        native_device_view_for_class(class, state)?;
        let hex_lattice_contract::ParameterValue::Float(gain) = value else {
            return Err(Invalid("output gain must be a finite db value"));
        };
        return state
            .clone()
            .with_output_gain_db(gain)
            .map_err(map_device_contract_error);
    }
    let result = match class {
        NativeDeviceClass::Effect => hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?
            .patch_parameter(state, parameter, value),
        NativeDeviceClass::Instrument => hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?
            .patch_parameter(state, parameter, value),
    };
    result.map_err(map_device_contract_error)
}

pub fn native_device_parameter(
    state: &hex_lattice_contract::NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
) -> Result<hex_lattice_contract::ParameterValue, Invalid> {
    native_device_parameter_for_class(NativeDeviceClass::Effect, state, parameter)
}

pub fn native_device_descriptors(
    type_id: hex_lattice_contract::DeviceTypeId,
) -> Result<&'static [hex_lattice_contract::ParameterDescriptor], Invalid> {
    native_device_descriptors_for_class(NativeDeviceClass::Effect, type_id)
}

fn map_device_contract_error(error: hex_lattice_contract::DeviceContractError) -> Invalid {
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
        Error::RuntimeBudgetExceeded => "native device runtime exceeds its bounded budget",
        Error::RuntimeAllocation => "native device runtime allocation failed",
        Error::UnsupportedProcessingClass => "native device processing class is not admitted",
    })
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
    frames: Arc<AudioData>,
    pcm_arc_receipt: Option<pcm_arc::PcmArcReceipt>,
}
#[derive(Debug)]
struct AudioData {
    values: Vec<Frame>,
    _derived: Option<audio_transform::Reservation>,
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
    gain: f32,
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
                gain: 1.0,
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
            if phase == 0
                && metadata.fade.is_none()
                && metadata.streamed.is_none()
                && metadata.gain == 1.0
            {
                self.metadata = None;
            }
        } else if phase != 0 {
            self.metadata = Some(Box::new(ClipMetadata {
                gain: 1.0,
                source_phase: phase,
                fade: None,
                streamed: None,
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
            {
                self.metadata = None;
            }
        } else if linear != 1.0 {
            self.metadata = Some(Box::new(ClipMetadata {
                gain: linear,
                source_phase: 0,
                fade: None,
                streamed: None,
            }));
        }
        self
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
                gain: 1.0,
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
    source_mix: PreparedTrackMix,
    gain_envelope: Option<gain_envelope::PreparedGainEnvelope>,
    instrument: Option<InstrumentTrack>,
    instrument_position: usize,
    ordered_prefix: Option<hex_lattice::StereoMatrix>,
    ordered_suffix: Option<hex_lattice::StereoMatrix>,
    device_chain: std::sync::Arc<[hex_lattice::PreparedProcessorInstance]>,
    sends: Vec<CompiledSend>,
    instrument_id: Option<hex_project::DeviceId>,
    instrument_state: Option<hex_lattice_contract::NativeDeviceState>,
    instrument_bypassed: bool,
    instrument_input_gain: f32,
    instrument_output_gain: f32,
    instrument_latency: Option<u32>,
    instrument_runtime_latency: Option<u32>,
    instrument_meter: std::sync::Arc<hex_lattice::DeviceMeter>,
    mutable_instrument: bool,
    mutable_looping_source: bool,
    midi_source_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CompiledSend {
    pub return_index: usize,
    pub gain: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct ReturnBus {
    pub id: hex_project::ReturnId,
    pub gain: f32,
    pub muted: bool,
    pub matrix: Option<hex_lattice::StereoMatrix>,
    pub device_chain: std::sync::Arc<[hex_lattice::PreparedProcessorInstance]>,
}

/// Validated per-track stereo coefficients shared by independent source paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedTrackMix {
    channel_gain: Frame,
    effects: Option<hex_lattice::StereoMatrix>,
    muted: bool,
}
impl PreparedTrackMix {
    pub(crate) const fn identity() -> Self {
        Self {
            channel_gain: [1.0, 1.0],
            effects: None,
            muted: false,
        }
    }
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
        let processed = self.apply_frame(source);
        destination[0] += processed[0];
        destination[1] += processed[1];
    }

    /// Return this track's post-fader contribution for routing to sends.
    pub(crate) fn apply_frame(self, source: Frame) -> Frame {
        if self.muted {
            return [0.0; 2];
        }
        let source = self.effects.map_or(source, |matrix| matrix.apply(source));
        [
            source[0] * self.channel_gain[0],
            source[1] * self.channel_gain[1],
        ]
    }

    pub(crate) fn with_absolute_gain(mut self, gain: f32) -> Self {
        self.channel_gain[0] *= gain;
        self.channel_gain[1] *= gain;
        self
    }

    fn add_captured(self, destination: &mut Frame, source: Frame, capture: Option<&mut Frame>) {
        let source = self.apply_frame(source);
        destination[0] += source[0];
        destination[1] += source[1];
        if let Some(capture) = capture {
            capture[0] += source[0];
            capture[1] += source[1];
        }
    }

    fn without_effects(self) -> Self {
        Self {
            effects: None,
            ..self
        }
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
            source_mix: PreparedTrackMix::new(gain, balance, muted)?,
            gain_envelope: None,
            instrument: None,
            instrument_position: 0,
            ordered_prefix: None,
            ordered_suffix: None,
            device_chain: std::sync::Arc::from([]),
            sends: Vec::new(),
            instrument_id: None,
            instrument_state: None,
            instrument_bypassed: false,
            instrument_input_gain: 1.0,
            instrument_output_gain: 1.0,
            instrument_latency: Some(0),
            instrument_runtime_latency: Some(0),
            instrument_meter: Default::default(),
            mutable_instrument: false,
            mutable_looping_source: false,
            midi_source_index: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_audio_instrument_and_effects(
        clips: Vec<Clip>,
        gain: f32,
        muted: bool,
        balance: f32,
        instrument: Option<InstrumentTrack>,
        instrument_position: usize,
        effects: Option<hex_lattice::StereoMatrix>,
        ordered_prefix: Option<hex_lattice::StereoMatrix>,
        ordered_suffix: Option<hex_lattice::StereoMatrix>,
        device_chain: Vec<hex_lattice::PreparedProcessorInstance>,
    ) -> Result<Self, Invalid> {
        let stateful = !device_chain.is_empty();
        let ordered = instrument_position != 0;
        let segmented =
            stateful || (ordered && (ordered_prefix.is_some() || ordered_suffix.is_some()));
        let mix_effects = if segmented { None } else { effects };
        let mix = PreparedTrackMix::with_effects(gain, balance, muted, mix_effects)?;
        let source_mix = if segmented {
            PreparedTrackMix::new(1.0, 0.0, false)?
        } else {
            mix
        };
        Ok(Self {
            track_id: None,
            clips,
            mix,
            source_mix,
            gain_envelope: None,
            instrument,
            instrument_position,
            ordered_prefix,
            ordered_suffix,
            device_chain: std::sync::Arc::from(device_chain),
            sends: Vec::new(),
            instrument_id: None,
            instrument_state: None,
            instrument_bypassed: false,
            instrument_input_gain: 1.0,
            instrument_output_gain: 1.0,
            instrument_latency: Some(0),
            instrument_runtime_latency: Some(0),
            instrument_meter: Default::default(),
            mutable_instrument: false,
            mutable_looping_source: false,
            midi_source_index: None,
        })
    }

    pub(crate) fn with_instrument_config(
        mut self,
        instrument: Option<&hex_project::Device>,
    ) -> Self {
        self.instrument_id = instrument.map(hex_project::Device::id);
        self.instrument_state =
            instrument.map(|device| device.state().clone().without_host_gains());
        self.instrument_bypassed = instrument.is_some_and(hex_project::Device::bypassed);
        self.instrument_input_gain =
            instrument.map_or(1.0, |device| device.state().input_gain_linear());
        self.instrument_output_gain =
            instrument.map_or(1.0, |device| device.state().output_gain_linear());
        if self.instrument_output_gain != 1.0 || self.instrument_input_gain != 1.0 {
            self.source_mix = PreparedTrackMix {
                channel_gain: [1.0; 2],
                effects: None,
                muted: false,
            };
        }
        self
    }

    pub(crate) fn with_instrument_latency(
        mut self,
        source: Option<u32>,
        runtime: Option<u32>,
    ) -> Self {
        self.instrument_latency = source;
        self.instrument_runtime_latency = runtime;
        self
    }
    pub(crate) fn instrument_config_matches(&self, other: &Self) -> bool {
        self.instrument_id == other.instrument_id
            && self.instrument_state == other.instrument_state
            && self.instrument_bypassed == other.instrument_bypassed
    }

    pub(crate) fn with_mutable_instrument(mut self, mutable: bool, looping_source: bool) -> Self {
        if mutable {
            // Device effects were prepared before the instrument capability
            // was attached. A gap-zero stateless chain may therefore still be
            // fused into the source/mix path; ordered rendering owns every
            // effect application once the mutable instrument is admitted.
            self.mix = self.mix.without_effects();
            self.source_mix = PreparedTrackMix::identity();
        }
        self.mutable_instrument = mutable;
        self.mutable_looping_source = mutable && looping_source;
        self
    }

    pub(crate) fn has_mutable_looping_source(&self) -> bool {
        self.mutable_looping_source
    }

    pub(crate) fn has_mutable_instrument(&self) -> bool {
        self.mutable_instrument
    }

    pub(crate) fn instrument_enabled(&self) -> bool {
        self.instrument_id.is_some() && !self.instrument_bypassed && self.instrument.is_some()
    }

    pub(crate) fn instrument_slot_enabled(&self) -> bool {
        self.instrument_id.is_some() && !self.instrument_bypassed
    }

    pub(crate) fn new_device_runtime(
        &self,
    ) -> Result<hex_lattice::NativeRuntimeChain, hex_lattice_contract::DeviceContractError> {
        let mut instances = Vec::new();
        instances
            .try_reserve_exact(self.device_chain.len())
            .map_err(|_| hex_lattice_contract::DeviceContractError::RuntimeAllocation)?;
        instances.extend(self.device_chain.iter().cloned());
        hex_lattice::NativeRuntimeChain::new(instances)
    }

    pub(crate) fn device_runtime_storage_preflight_bytes(&self) -> Option<usize> {
        hex_lattice::NativeRuntimeChain::declared_storage_preflight_bytes_for_instances(
            &self.device_chain,
        )
    }

    pub(crate) fn with_sends(mut self, sends: Vec<CompiledSend>) -> Self {
        self.sends = sends;
        if !self.sends.is_empty() {
            self.source_mix = PreparedTrackMix::identity();
        }
        self
    }

    pub(crate) fn sends(&self) -> &[CompiledSend] {
        &self.sends
    }

    pub(crate) fn has_device_chain(&self) -> bool {
        !self.device_chain.is_empty()
    }

    pub(crate) fn stateful_device_count(&self) -> usize {
        self.device_chain
            .iter()
            .filter(|device| device.definition.is_some())
            .count()
    }

    pub(crate) fn stateful_device_declared_bytes(&self) -> Option<usize> {
        self.device_chain
            .iter()
            .filter_map(|device| device.definition.as_ref())
            .try_fold(0_usize, |total, definition| {
                total.checked_add(definition.declared_runtime_bytes().checked_mul(2)?)
            })
    }

    pub(crate) fn instrument_position(&self) -> usize {
        self.instrument_position
    }

    pub(crate) fn has_ordered_instrument(&self) -> bool {
        self.instrument_position != 0
    }

    pub(crate) fn requires_ordered_processing(&self) -> bool {
        (self.instrument_id.is_some()
            && (self.instrument_output_gain != 1.0 || self.instrument_input_gain != 1.0))
            || self.mutable_instrument
            || self.midi_source_index.is_some()
            || self.has_device_chain()
            || (self.has_ordered_instrument()
                && (self.ordered_prefix.is_some() || self.ordered_suffix.is_some()))
    }

    pub(crate) fn apply_instrument_input_gain(&self, frames: &mut [Frame]) {
        if !self.instrument_bypassed && self.instrument_input_gain != 1.0 {
            for frame in frames {
                frame[0] *= self.instrument_input_gain;
                frame[1] *= self.instrument_input_gain;
            }
        }
    }
    pub(crate) fn apply_instrument_output_gain(&self, frames: &mut [Frame]) {
        if !self.instrument_bypassed {
            let gain = self.instrument_output_gain;
            if gain != 1.0 {
                for frame in frames {
                    frame[0] *= gain;
                    frame[1] *= gain;
                }
            }
        }
    }

    pub(crate) fn apply_ordered_prefix(&self, frames: &mut [Frame]) {
        if let Some(matrix) = self.ordered_prefix {
            matrix.apply_block(frames);
        }
    }

    pub(crate) fn apply_ordered_suffix(&self, frames: &mut [Frame]) {
        if let Some(matrix) = self.ordered_suffix {
            matrix.apply_block(frames);
        }
    }

    pub(crate) fn instrument_suffix_matrix(&self) -> Option<hex_lattice::StereoMatrix> {
        self.ordered_suffix
    }

    pub(crate) fn instrument_suffix_needs_tail(&self) -> bool {
        if self.has_ordered_instrument() {
            self.ordered_suffix.is_some()
                || self
                    .device_chain
                    .get(self.instrument_position..)
                    .is_some_and(|effects| {
                        effects.iter().any(|effect| {
                            effect.definition.is_some()
                                || effect.matrix.coefficients() != [[1.0, 0.0], [0.0, 1.0]]
                        })
                    })
        } else {
            self.has_device_chain()
        }
    }

    pub(crate) fn with_gain_envelope(
        mut self,
        lane: &hex_project::GainLane,
        tempo: &hex_project::TempoMap,
        balance: f32,
        fallback: f32,
    ) -> Result<Self, Invalid> {
        let neutral =
            PreparedTrackMix::with_effects(1.0, balance, self.mix.muted, self.mix.effects)?;
        self.gain_envelope =
            gain_envelope::PreparedGainEnvelope::prepare(lane, tempo, neutral, fallback)?;
        if self.gain_envelope.is_some() {
            self.source_mix = PreparedTrackMix::identity();
        }
        Ok(self)
    }
    /// Read-only prepared curve metadata. Empty means static fallback; this is not an applied-value acknowledgment.
    pub fn gain_envelope_knots(&self) -> &[GainEnvelopeKnot] {
        self.gain_envelope
            .as_ref()
            .map_or(&[], |envelope| envelope.knots())
    }
    /// Logical prepared gain at an Arrangement frame, independent of the runtime source owner.
    pub fn prepared_gain_at(&self, frame: u64) -> Option<f32> {
        self.gain_envelope
            .as_ref()
            .map(|envelope| envelope.value_at(frame))
    }
    pub(crate) fn arm_gain_recording(
        &mut self,
        fallback: f32,
        balance: f32,
    ) -> Result<(), Invalid> {
        if self.gain_envelope.is_none() {
            let neutral =
                PreparedTrackMix::with_effects(1.0, balance, self.mix.muted, self.mix.effects)?;
            self.gain_envelope = Some(gain_envelope::PreparedGainEnvelope::constant(
                neutral, fallback,
            ));
        }
        self.source_mix = PreparedTrackMix::identity();
        Ok(())
    }
    pub(crate) fn recording_mix(&self, gain: f32) -> PreparedTrackMix {
        self.gain_envelope
            .as_ref()
            .map_or(self.mix, |envelope| envelope.mix_with_gain(gain))
    }
    pub(crate) fn has_gain_automation(&self) -> bool {
        self.gain_envelope.is_some()
    }
    pub(crate) fn mix_at(&self, frame: u64) -> PreparedTrackMix {
        self.gain_envelope
            .as_ref()
            .map_or(self.mix, |envelope| envelope.mix_at(frame))
    }
    pub fn mix(&self) -> PreparedTrackMix {
        self.mix
    }

    pub(crate) fn source_mix(&self) -> PreparedTrackMix {
        self.source_mix
    }

    pub(crate) fn has_instrument_source(&self) -> bool {
        self.instrument.is_some()
    }

    pub(crate) fn with_midi_source_index(mut self, source: Option<usize>) -> Self {
        self.midi_source_index = source;
        if source.is_some() {
            // Audio keeps its local owner; the block renderer adds the selected
            // MIDI bus at the destination instrument and applies its fader once.
            self.source_mix = PreparedTrackMix::identity();
        }
        self
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
        if self.has_gain_automation() {
            return Err(Invalid("automated track requires project rendering"));
        }
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
        if !self.source_mix.muted {
            self.render_unmuted_range(start_frame, end_frame, output, midi_onset_floor_exclusive);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_additive_with_suppressions_and_pages(
        &self,
        start_frame: u64,
        performance_frame: u64,
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        pages: Option<&mut (impl MediaPages + ?Sized)>,
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        if !self.source_mix.muted {
            if let Some(page_provider) = pages {
                self.render_audio_range_with_pages(start_frame, end_frame, output, page_provider);
            } else {
                self.render_audio_range(start_frame, end_frame, output);
            }
            self.render_instrument_range_with_suppressions(
                (start_frame, end_frame),
                output,
                None,
                performance_frame,
                suppressions,
                None,
                continuations,
            )?;
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
        if !self.source_mix.muted {
            self.render_unmuted_range_with_pages(start_frame, end_frame, output, None, pages);
        }
        Ok(())
    }

    pub(crate) fn render_audio_additive(
        &self,
        start_frame: u64,
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        self.render_audio_range(start_frame, end_frame, output);
        Ok(())
    }

    // The direct Main fast path taps each existing source addition once. It
    // does not regroup sources into a track buffer (which changes rounding).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_additive_captured<P: MediaPages + ?Sized>(
        &self,
        start_frame: u64,
        performance_frame: u64,
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        pages: Option<&mut P>,
        capture: &mut [Frame],
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Result<(), Invalid> {
        let end_frame = start_frame
            .checked_add(
                u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?,
            )
            .ok_or(Invalid("render range overflow"))?;
        if capture.len() != output.len() {
            return Err(Invalid("capture length mismatch"));
        }
        if !self.source_mix.muted {
            self.render_audio_range_captured(start_frame, end_frame, output, pages, Some(capture));
            self.render_instrument_range_with_suppressions(
                (start_frame, end_frame),
                output,
                None,
                performance_frame,
                suppressions,
                Some(capture),
                continuations,
            )?;
        }
        Ok(())
    }

    pub(crate) fn render_audio_additive_with_pages(
        &self,
        start_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        self.render_audio_range_with_pages(start_frame, end_frame, output, pages);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_instrument_additive_with_suppressions(
        &self,
        start_frame: u64,
        performance_frame: u64,
        output: &mut [Frame],
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Result<(), Invalid> {
        let length = u64::try_from(output.len()).map_err(|_| Invalid("render length overflow"))?;
        let end_frame = start_frame
            .checked_add(length)
            .ok_or(Invalid("render range overflow"))?;
        self.render_instrument_range_with_suppressions(
            (start_frame, end_frame),
            output,
            None,
            performance_frame,
            suppressions,
            None,
            continuations,
        )
    }

    pub(crate) fn for_each_active_midi_span(
        &self,
        frame: u64,
        mut visit: impl FnMut(&midi_compile::CompiledNoteSpan, midi_note_bank::PreparedMidiNote),
    ) -> Result<(), Invalid> {
        let end = frame
            .checked_add(1)
            .ok_or(Invalid("midi span frame overflow"))?;
        if let Some(instrument) = &self.instrument {
            instrument.for_each_intersecting(frame, end, None, |span| {
                if let Some(sampler) = instrument.bank.sampler(span.sampler_index) {
                    visit(span, sampler);
                }
            });
        }
        Ok(())
    }

    pub(crate) fn for_each_mutable_midi_span(
        &self,
        start: u64,
        end: u64,
        mut visit: impl FnMut(&midi_compile::CompiledNoteSpan),
    ) -> Result<(), Invalid> {
        if start > end {
            return Err(Invalid("midi span frame range is invalid"));
        }
        if let Some(instrument) = self
            .instrument
            .as_ref()
            .filter(|instrument| instrument.mutable)
        {
            instrument.for_each_intersecting(start, end, None, &mut visit);
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
        self.render_audio_range(start_frame, end_frame, output);
        self.render_instrument_range(start_frame, end_frame, output, midi_onset_floor_exclusive);
    }

    fn render_audio_range(&self, start_frame: u64, end_frame: u64, output: &mut [Frame]) {
        self.render_audio_range_captured::<dyn MediaPages>(
            start_frame,
            end_frame,
            output,
            None,
            None,
        );
    }

    fn render_audio_range_captured<P: MediaPages + ?Sized>(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: Option<&mut P>,
        mut capture: Option<&mut [Frame]>,
    ) {
        if let Some(pages) = pages {
            self.render_audio_range_with_pages_captured(
                start_frame,
                end_frame,
                output,
                pages,
                capture,
            );
            return;
        }
        for clip in &self.clips {
            if !clip.is_streamed() {
                self.render_resident_clip(
                    clip,
                    start_frame,
                    end_frame,
                    output,
                    capture.as_deref_mut(),
                );
            }
        }
    }

    fn render_unmuted_range_with_pages(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        pages: &mut (impl MediaPages + ?Sized),
    ) {
        self.render_audio_range_with_pages(start_frame, end_frame, output, pages);
        self.render_instrument_range(start_frame, end_frame, output, midi_onset_floor_exclusive);
    }

    fn render_audio_range_with_pages(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
    ) {
        self.render_audio_range_with_pages_captured(start_frame, end_frame, output, pages, None);
    }

    fn render_audio_range_with_pages_captured(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        pages: &mut (impl MediaPages + ?Sized),
        mut capture: Option<&mut [Frame]>,
    ) {
        for clip in &self.clips {
            if !clip.is_streamed() {
                self.render_resident_clip(
                    clip,
                    start_frame,
                    end_frame,
                    output,
                    capture.as_deref_mut(),
                );
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
                let rendered = render_streamed_page_span_captured(
                    self.source_mix,
                    source,
                    StreamedPageSpan {
                        source_frame,
                        timeline_frame: position,
                        output_offset,
                        count,
                        gain: metadata.gain,
                        fade: metadata.fade.as_ref().map(|fade| (fade, clip.start)),
                    },
                    output,
                    pages,
                    capture.as_deref_mut(),
                );
                if rendered == 0 {
                    break;
                }
                position = position.saturating_add(rendered as u64);
            }
        }
    }

    fn render_resident_clip(
        &self,
        clip: &Clip,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        mut capture: Option<&mut [Frame]>,
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
        let clip_gain = metadata.map_or(1.0, |metadata| metadata.gain);
        let fade = metadata.and_then(|metadata| metadata.fade.as_ref());
        if fade.is_some() || clip_gain != 1.0 {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                let position = span_start + offset as u64;
                let mut sample = clip.at(position, source_phase);
                if let Some(fade) = fade {
                    let gain = fade.gain_at(position - clip.start);
                    sample[0] *= gain;
                    sample[1] *= gain;
                }
                if clip_gain != 1.0 {
                    sample[0] *= clip_gain;
                    sample[1] *= clip_gain;
                }
                self.source_mix.add_captured(
                    frame,
                    sample,
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_start + offset)),
                );
            }
        } else if source_phase == 0 {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                self.source_mix.add_captured(
                    frame,
                    clip.at(span_start + offset as u64, 0),
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_start + offset)),
                );
            }
        } else {
            for (offset, frame) in output[output_start..output_end].iter_mut().enumerate() {
                self.source_mix.add_captured(
                    frame,
                    clip.at(span_start + offset as u64, source_phase),
                    capture
                        .as_deref_mut()
                        .and_then(|frames| frames.get_mut(output_start + offset)),
                );
            }
        }
    }

    pub(crate) fn render_instrument_range(
        &self,
        start_frame: u64,
        end_frame: u64,
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
    ) {
        let _ = self.render_instrument_range_with_suppressions(
            (start_frame, end_frame),
            output,
            midi_onset_floor_exclusive,
            0,
            &[],
            None,
            &[],
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_instrument_range_with_suppressions(
        &self,
        (start_frame, end_frame): (u64, u64),
        output: &mut [Frame],
        midi_onset_floor_exclusive: Option<u64>,
        performance_frame: u64,
        suppressions: &[midi_template::MidiOccurrenceSuppression],
        mut capture: Option<&mut [Frame]>,
        continuations: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Result<(), Invalid> {
        if let Some(instrument) = &self.instrument {
            if instrument.mutable {
                return Ok(());
            }
            for chunk_start in (start_frame..end_frame).step_by(256) {
                let chunk_end = chunk_start.saturating_add(256).min(end_frame);
                let output_start = (chunk_start - start_frame) as usize;
                let output_end = (chunk_end - start_frame) as usize;
                let target = &mut output[output_start..output_end];
                let chunk_performance_frame = performance_frame
                    .checked_add(chunk_start - start_frame)
                    .ok_or(Invalid("midi performance frame overflow"))?;
                let mut failure = None;
                instrument.for_each_intersecting(
                    chunk_start,
                    chunk_end,
                    midi_onset_floor_exclusive,
                    |span| {
                        let carried =
                            self.track_id.is_some_and(|track| {
                                suppressions
                                    .binary_search_by(|entry| {
                                        (entry.track, entry.clip, entry.note, entry.iteration)
                                            .cmp(&(track, span.clip, span.note, span.iteration))
                                    })
                                    .ok()
                                    .and_then(|index| suppressions.get(index))
                                    .is_some_and(|entry| {
                                        entry.until_frame > chunk_performance_frame
                                    })
                            });
                        if carried {
                            return;
                        }
                        let start = span.start.max(chunk_start);
                        let end = span.end.min(chunk_end);
                        let Some(voice) = instrument.bank.sampler(span.sampler_index) else {
                            return;
                        };
                        for frame_position in start..end {
                            let relative = frame_position - span.start;
                            let Some(absolute) =
                                chunk_performance_frame.checked_add(frame_position - chunk_start)
                            else {
                                failure = Some(Invalid("midi performance frame overflow"));
                                return;
                            };
                            match voice.sample_at(
                                &instrument.shared_contexts,
                                &[],
                                continuations,
                                absolute,
                                relative,
                                span.gate_end - span.start,
                            ) {
                                Ok(sample) => {
                                    self.source_mix.add_captured(
                                        &mut target[(frame_position - chunk_start) as usize],
                                        sample,
                                        capture.as_deref_mut().and_then(|frames| {
                                            frames.get_mut((frame_position - start_frame) as usize)
                                        }),
                                    );
                                }
                                Err(error) => {
                                    failure = Some(error);
                                    return;
                                }
                            }
                        }
                    },
                );
                if let Some(error) = failure {
                    return Err(error);
                }
            }
        }
        Ok(())
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
    pub(crate) timeline_end: Option<u64>,
    pub(crate) bank: Arc<midi_note_bank::PreparedMidiNoteBank>,
    pub(crate) shared_contexts: Arc<shared_analytic::SharedAnalyticContextTable>,
    pub(crate) mutable: bool,
}

/// Immutable control-side root/crop view for worker waveform preparation.
/// Audio retains the same PCM reservation as playback; never build summaries in render.
#[derive(Debug, Clone)]
pub struct PreparedAudioClipView {
    pub clip: hex_project::ClipId,
    pub audio: Audio,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone)]
pub struct Project {
    audio_clip_views: Vec<PreparedAudioClipView>,
    sample_rate: u32,
    tracks: Vec<Track>,
    return_buses: Vec<ReturnBus>,
    return_feed_plan: return_feed_plan::ReturnFeedPlan,
    signal_graph: Option<Arc<signal_graph::SignalGraph>>,
    tempo_map: Option<hex_project::TempoMap>,
    midi_routing: Vec<(hex_project::TrackId, hex_project::TrackId)>,
    loop_region: Option<hex_project::LoopRegion>,
    master_gain: PreparedMasterGain,
    audible_end_frame: Option<u64>,
    has_streamed_sources: bool,
    shared_contexts: owned_shared::OwnedShared<shared_analytic::SharedAnalyticContextTable>,
}
impl Project {
    /// Worker-only observation of accepted transformed PCM, independent of cache residency.
    pub fn audio_clip_views(&self) -> &[PreparedAudioClipView] {
        &self.audio_clip_views
    }

    /// Accounted Arrangement note-bank bytes, including each bank's entry
    /// allocation and fixed bank/Arc owner overhead.
    pub fn midi_note_bank_storage_bytes(&self) -> Result<usize, Invalid> {
        self.tracks
            .iter()
            .filter_map(|track| track.instrument.as_ref())
            .try_fold(0_usize, |total, instrument| {
                total
                    .checked_add(instrument.bank.storage_bytes()?)
                    .ok_or(Invalid("midi note bank byte count overflow"))
            })
    }

    /// Number of compact sampler indices retained by Arrangement note spans.
    pub fn midi_note_sampler_reference_count(&self) -> Result<usize, Invalid> {
        self.tracks
            .iter()
            .filter_map(|track| track.instrument.as_ref())
            .try_fold(0_usize, |total, instrument| {
                total
                    .checked_add(if instrument.mutable {
                        0
                    } else {
                        instrument.spans.len()
                    })
                    .ok_or(Invalid("midi note bank reference count overflow"))
            })
    }

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
            audio_clip_views: Vec::new(),
            audible_end_frame: tracks
                .iter()
                .flat_map(|track| track.clips.iter().map(|clip| clip.end))
                .max(),
            sample_rate,
            tracks,
            return_buses: Vec::new(),
            return_feed_plan: return_feed_plan::ReturnFeedPlan::default(),
            signal_graph: None,
            tempo_map: None,
            midi_routing: Vec::new(),
            loop_region,
            master_gain,
            has_streamed_sources,
            shared_contexts: owned_shared::OwnedShared::new_owned(
                shared_analytic::SharedAnalyticContextTable::default(),
            ),
        })
    }

    pub(crate) fn with_compiled_tracks(
        sample_rate: u32,
        tracks: Vec<Track>,
        return_buses: Vec<ReturnBus>,
        loop_region: Option<hex_project::LoopRegion>,
        master_gain: f32,
        shared_contexts: Arc<shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<Self, Invalid> {
        let mut project = Self::with_master_gain(sample_rate, tracks, loop_region, master_gain)?;
        project.return_buses = return_buses;
        project.shared_contexts = owned_shared::OwnedShared::untracked(shared_contexts);
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

    pub(crate) fn shared_contexts(&self) -> &Arc<shared_analytic::SharedAnalyticContextTable> {
        &self.shared_contexts
    }
    pub fn gain_automation_capacity(&self) -> GainAutomationCapacity {
        let mut capacity = GainAutomationCapacity::default();
        for track in &self.tracks {
            if let Some(envelope) = &track.gain_envelope {
                let actual = envelope.capacity();
                capacity.authored_points += actual.authored_points;
                capacity.prepared_knots += actual.prepared_knots;
                capacity.allocated_knot_slots += actual.allocated_knot_slots;
                capacity.allocated_knot_bytes += actual.allocated_knot_bytes;
                capacity.authored_units += actual.authored_units;
                capacity.prepared_touch_spans += actual.prepared_touch_spans;
                capacity.prepared_touch_steps += actual.prepared_touch_steps;
                capacity.allocated_touch_bytes += actual.allocated_touch_bytes;
            }
        }
        capacity
    }
    /// Known causal startup delay; compensation does not remove physical latency.
    pub fn main_latency_frames(&self) -> u32 {
        self.signal_graph
            .as_ref()
            .map_or(0, |graph| graph.main_latency())
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

    pub fn device_meter(
        &self,
        device_id: u64,
    ) -> Option<&std::sync::Arc<hex_lattice::DeviceMeter>> {
        for track in &self.tracks {
            if track
                .instrument_id
                .is_some_and(|id| id.value() == device_id)
            {
                return Some(&track.instrument_meter);
            }
            if let Some(device) = track
                .device_chain
                .iter()
                .find(|device| device.device_id == device_id)
            {
                return Some(&device.meter);
            }
        }
        self.return_buses
            .iter()
            .flat_map(|bus| bus.device_chain.iter())
            .find(|device| device.device_id == device_id)
            .map(|device| &device.meter)
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub(crate) fn return_buses(&self) -> &[ReturnBus] {
        &self.return_buses
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

pub(crate) struct RendererRuntime {
    device_chains: Vec<hex_lattice::NativeRuntimeChain>,
    return_chains: Vec<hex_lattice::NativeRuntimeChain>,
    return_scratch: Vec<[Frame; 256]>,
    runtime_error: Option<&'static str>,
    compensation: Option<signal_graph::SignalRuntime>,
}
impl RendererRuntime {
    pub(crate) fn new(project: &Project) -> Self {
        let mut device_chains = Vec::new();
        let mut return_chains = Vec::new();
        let mut return_scratch = Vec::new();
        let mut runtime_error = None;
        let compensation = match project
            .signal_graph
            .as_ref()
            .map(|graph| signal_graph::SignalRuntime::new(Arc::clone(graph)))
            .transpose()
        {
            Ok(compensation) => compensation,
            Err(_) => {
                runtime_error = Some("compensation runtime storage is unavailable");
                None
            }
        };
        if device_chains
            .try_reserve_exact(project.tracks.len())
            .is_err()
        {
            runtime_error = Some("native device runtime allocation failed");
        } else {
            for track in &project.tracks {
                match track.new_device_runtime() {
                    Ok(chain) => device_chains.push(chain),
                    Err(_) => {
                        runtime_error = Some("native device runtime preparation failed");
                        device_chains.clear();
                        break;
                    }
                }
            }
        }
        if runtime_error.is_none() {
            if return_chains
                .try_reserve_exact(project.return_buses.len())
                .is_err()
                || return_scratch
                    .try_reserve_exact(project.return_buses.len())
                    .is_err()
            {
                runtime_error = Some("return runtime allocation failed");
            } else {
                for bus in &project.return_buses {
                    let mut instances = Vec::new();
                    if instances.try_reserve_exact(bus.device_chain.len()).is_err() {
                        runtime_error = Some("return device allocation failed");
                        return Self {
                            device_chains,
                            return_chains,
                            return_scratch,
                            runtime_error,
                            compensation,
                        };
                    }
                    instances.extend(bus.device_chain.iter().cloned());
                    match hex_lattice::NativeRuntimeChain::new(instances) {
                        Ok(chain) => {
                            return_chains.push(chain);
                            return_scratch.push([[0.0; 2]; 256]);
                        }
                        Err(_) => {
                            runtime_error = Some("return runtime preparation failed");
                            return_chains.clear();
                            return_scratch.clear();
                            break;
                        }
                    }
                }
            }
        }
        Self {
            device_chains,
            return_chains,
            return_scratch,
            runtime_error,
            compensation,
        }
    }

    pub(crate) fn reset(&mut self) {
        if let Some(compensation) = self.compensation.as_mut() {
            compensation.delays.reset_epoch();
        }
        for chain in &mut self.device_chains {
            chain.reset();
        }
        for chain in &mut self.return_chains {
            chain.reset();
        }
    }

    pub(crate) fn select_processor_audition(
        &mut self,
        project: &Project,
        identity: crate::PostNodeIdentity,
        selection: Option<hex_lattice::ProcessorAuditionSelection>,
    ) -> bool {
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.select_processor_audition(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.select_processor_audition(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
        }
    }

    pub(crate) fn processor_audition_is_active(
        &mut self,
        project: &Project,
        identity: crate::PostNodeIdentity,
        selection: hex_lattice::ProcessorAuditionSelection,
    ) -> bool {
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.processor_audition_is_active(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.processor_audition_is_active(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                        selection,
                    )
                }),
        }
    }

    pub(crate) fn cancel_processor_audition(
        &mut self,
        project: &Project,
        identity: crate::PostNodeIdentity,
    ) -> bool {
        match identity.owner {
            hex_lattice::ProcessorChainOwner::Track(owner) => project
                .tracks()
                .iter()
                .position(|track| track.canonical_track_id().map(|id| id.value()) == Some(owner))
                .and_then(|index| self.device_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.cancel_processor_audition_for_identity(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                    )
                }),
            hex_lattice::ProcessorChainOwner::Return(owner) => project
                .return_buses()
                .iter()
                .position(|bus| bus.id.value() == owner)
                .and_then(|index| self.return_chains.get_mut(index))
                .is_some_and(|chain| {
                    chain.cancel_processor_audition_for_identity(
                        identity.owner,
                        identity.device_id,
                        identity.device_type,
                    )
                }),
        }
    }

    pub(crate) fn cancel_all_processor_auditions(&mut self) {
        for chain in &mut self.device_chains {
            chain.cancel_all_processor_auditions();
        }
        for chain in &mut self.return_chains {
            chain.cancel_all_processor_auditions();
        }
    }

    pub(crate) fn has_stateful_processors(&self) -> bool {
        self.device_chains
            .iter()
            .any(hex_lattice::NativeRuntimeChain::has_stateful_processors)
            || self
                .return_chains
                .iter()
                .any(hex_lattice::NativeRuntimeChain::has_stateful_processors)
    }

    pub(crate) fn can_inherit_from(
        &self,
        project: &Project,
        previous: &Self,
        previous_project: &Project,
    ) -> bool {
        let tracks_match = project.tracks().iter().enumerate().all(|(index, track)| {
            let Some(track_id) = track.canonical_track_id() else {
                return true;
            };
            let Some(previous_index) = previous_project
                .tracks()
                .iter()
                .position(|old_track| old_track.canonical_track_id() == Some(track_id))
            else {
                return true;
            };
            match (
                self.device_chains.get(index),
                previous.device_chains.get(previous_index),
            ) {
                (Some(candidate), Some(old)) => candidate.can_inherit_from(old),
                _ => false,
            }
        });
        tracks_match
            && project
                .return_buses()
                .iter()
                .enumerate()
                .all(|(index, bus)| {
                    let Some(previous_index) = previous_project
                        .return_buses()
                        .iter()
                        .position(|old_bus| old_bus.id == bus.id)
                    else {
                        return true;
                    };
                    match (
                        self.return_chains.get(index),
                        previous.return_chains.get(previous_index),
                    ) {
                        (Some(candidate), Some(old)) => candidate.can_inherit_from(old),
                        _ => false,
                    }
                })
    }

    pub(crate) fn inherit_from(
        &mut self,
        project: &Project,
        previous: &Self,
        previous_project: &Project,
    ) {
        for (index, track) in project.tracks().iter().enumerate() {
            let Some(track_id) = track.canonical_track_id() else {
                continue;
            };
            let Some(previous_index) = previous_project
                .tracks()
                .iter()
                .position(|old_track| old_track.canonical_track_id() == Some(track_id))
            else {
                continue;
            };
            if let (Some(candidate), Some(old)) = (
                self.device_chains.get_mut(index),
                previous.device_chains.get(previous_index),
            ) {
                candidate.inherit_from(old);
            }
        }
        for (index, bus) in project.return_buses().iter().enumerate() {
            let Some(previous_index) = previous_project
                .return_buses()
                .iter()
                .position(|old_bus| old_bus.id == bus.id)
            else {
                continue;
            };
            if let (Some(candidate), Some(old)) = (
                self.return_chains.get_mut(index),
                previous.return_chains.get(previous_index),
            ) {
                candidate.inherit_from(old);
            }
        }
    }

    pub(crate) fn render_without_pages(
        &mut self,
        project: &Project,
        position: &mut u64,
        playing: bool,
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        self.render_with_pages::<dyn MediaPages>(project, position, playing, output, None)
    }

    pub(crate) fn render_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        project: &Project,
        position: &mut u64,
        playing: bool,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        if let Some(message) = self.runtime_error {
            return Err(Invalid(message));
        }
        if project.tracks.iter().any(|track| track.mutable_instrument) {
            return Err(Invalid(
                "mutable instrument requires shared Session playback",
            ));
        }
        let next = if playing {
            position
                .checked_add(output.len() as u64)
                .ok_or(Invalid("transport overflow"))?
        } else {
            *position
        };
        if playing && !output.is_empty() && pages.is_none() && project.contains_streamed_sources() {
            return Err(Invalid("streamed project requires a media page provider"));
        }
        output.fill([0.0; 2]);
        if !playing {
            return Ok(());
        }
        if output.is_empty() {
            return Ok(());
        }
        let block_start = *position;
        if self.compensation.is_some() {
            self.render_compensated(project, block_start, output, pages.as_deref_mut())?;
            *position = next;
            return Ok(());
        }
        for chain in &mut self.device_chains {
            chain.begin_block();
        }
        let has_return_routes = !project.return_buses.is_empty()
            || project.tracks.iter().any(|track| !track.sends.is_empty());
        if has_return_routes {
            for chain in &mut self.return_chains {
                chain.begin_block();
            }
            if let Err(error) =
                self.render_return_routes(project, block_start, output, pages.as_deref_mut())
            {
                output.fill([0.0; 2]);
                return Err(error);
            }
            for chain in &mut self.device_chains {
                chain.commit_block();
            }
            for chain in &mut self.return_chains {
                chain.commit_block();
            }
            project.master_gain.apply(output);
            *position = next;
            return Ok(());
        }
        for (index, track) in project.tracks.iter().enumerate() {
            let stateful = track.has_device_chain();
            if stateful || track.requires_ordered_processing() || track.has_gain_automation() {
                let mut chain = if stateful {
                    Some(
                        self.device_chains
                            .get_mut(index)
                            .ok_or(Invalid("native device runtime is unavailable"))?,
                    )
                } else {
                    None
                };
                let mut scratch = [[0.0; 2]; 256];
                let mut offset = 0_usize;
                while offset < output.len() {
                    let count = (output.len() - offset).min(scratch.len());
                    let start = block_start
                        .checked_add(offset as u64)
                        .ok_or(Invalid("render range overflow"))?;
                    let end = start
                        .checked_add(count as u64)
                        .ok_or(Invalid("render range overflow"))?;
                    let target = &mut scratch[..count];
                    target.fill([0.0; 2]);
                    if let Some(page_provider) = pages.as_deref_mut() {
                        if track.requires_ordered_processing() {
                            track.render_audio_range_with_pages(start, end, target, page_provider);
                        } else {
                            track.render_unmuted_range_with_pages(
                                start,
                                end,
                                target,
                                None,
                                page_provider,
                            );
                        }
                    } else {
                        if track.requires_ordered_processing() {
                            track.render_audio_range(start, end, target);
                        } else {
                            track.render_unmuted_range(start, end, target, None);
                        }
                    }
                    if track.requires_ordered_processing() {
                        if let Some(chain) = chain.as_deref_mut() {
                            chain
                                .process_range(0..track.instrument_position(), target)
                                .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                        } else {
                            track.apply_ordered_prefix(target);
                        }
                        track.apply_instrument_input_gain(target);
                        track.render_instrument_range(start, end, target, None);
                        track.apply_instrument_output_gain(target);
                        if let Some(chain) = chain.as_deref_mut() {
                            let effect_count = chain.len();
                            chain
                                .process_range(track.instrument_position()..effect_count, target)
                                .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                        } else {
                            track.apply_ordered_suffix(target);
                        }
                    } else if let Some(chain) = chain.as_deref_mut() {
                        chain.process_block(target);
                    }
                    if track.has_gain_automation() {
                        for (frame_index, (destination, source)) in output[offset..offset + count]
                            .iter_mut()
                            .zip(target.iter().copied())
                            .enumerate()
                        {
                            track
                                .mix_at(start + frame_index as u64)
                                .add_frame(destination, source);
                        }
                    } else {
                        for (destination, source) in output[offset..offset + count]
                            .iter_mut()
                            .zip(target.iter().copied())
                        {
                            track.mix.add_frame(destination, source);
                        }
                    }
                    offset += count;
                }
            } else if !track.mix.muted {
                if let Some(page_provider) = pages.as_deref_mut() {
                    track.render_unmuted_range_with_pages(
                        block_start,
                        next,
                        output,
                        None,
                        page_provider,
                    );
                } else {
                    track.render_unmuted_range(block_start, next, output, None);
                }
            }
        }
        for chain in &mut self.device_chains {
            chain.commit_block();
        }
        // Neutral projects retain their original hot path. Non-neutral master
        // gain is deliberately post-sum rather than folded into track gains.
        project.master_gain.apply(output);
        *position = next;
        Ok(())
    }

    fn render_return_routes<P: MediaPages + ?Sized>(
        &mut self,
        project: &Project,
        block_start: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        for offset in (0..output.len()).step_by(256) {
            let count = (output.len() - offset).min(256);
            let start = block_start
                .checked_add(offset as u64)
                .ok_or(Invalid("render range overflow"))?;
            let end = start
                .checked_add(count as u64)
                .ok_or(Invalid("render range overflow"))?;
            let output_chunk = &mut output[offset..offset + count];
            for scratch in &mut self.return_scratch {
                scratch[..count].fill([0.0; 2]);
            }
            for (track_index, track) in project.tracks.iter().enumerate() {
                let stateful = track.has_device_chain();
                let ordered = track.requires_ordered_processing();
                if !stateful && !ordered && !track.has_gain_automation() && track.sends.is_empty() {
                    if let Some(provider) = pages.as_deref_mut() {
                        track.render_unmuted_range_with_pages(
                            start,
                            end,
                            output_chunk,
                            None,
                            provider,
                        );
                    } else {
                        track.render_unmuted_range(start, end, output_chunk, None);
                    }
                    continue;
                }

                let mut scratch = [[0.0; 2]; 256];
                let rendered = &mut scratch[..count];
                if let Some(provider) = pages.as_deref_mut() {
                    if ordered {
                        track.render_audio_range_with_pages(start, end, rendered, provider);
                    } else {
                        track.render_unmuted_range_with_pages(start, end, rendered, None, provider);
                    }
                } else if ordered {
                    track.render_audio_range(start, end, rendered);
                } else {
                    track.render_unmuted_range(start, end, rendered, None);
                }
                if ordered {
                    if stateful {
                        self.device_chains
                            .get_mut(track_index)
                            .ok_or(Invalid("native device runtime is unavailable"))?
                            .process_range(0..track.instrument_position, rendered)
                            .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                    } else {
                        track.apply_ordered_prefix(rendered);
                    }
                    track.apply_instrument_input_gain(rendered);
                    track.render_instrument_range(start, end, rendered, None);
                    track.apply_instrument_output_gain(rendered);
                    if stateful {
                        let chain = self
                            .device_chains
                            .get_mut(track_index)
                            .ok_or(Invalid("native device runtime is unavailable"))?;
                        let effect_count = chain.len();
                        chain
                            .process_range(track.instrument_position..effect_count, rendered)
                            .map_err(|_| Invalid("prepared effect segment is invalid"))?;
                    } else {
                        track.apply_ordered_suffix(rendered);
                    }
                } else if stateful {
                    self.device_chains
                        .get_mut(track_index)
                        .ok_or(Invalid("native device runtime is unavailable"))?
                        .process_block(rendered);
                }
                for (frame_index, source) in rendered.iter().copied().enumerate() {
                    let mixed = if track.has_gain_automation() {
                        track.mix_at(start + frame_index as u64).apply_frame(source)
                    } else {
                        track.mix.apply_frame(source)
                    };
                    output_chunk[frame_index][0] += mixed[0];
                    output_chunk[frame_index][1] += mixed[1];
                    for send in &track.sends {
                        let target = self
                            .return_scratch
                            .get_mut(send.return_index)
                            .ok_or(Invalid("compiled return send target is missing"))?;
                        target[frame_index][0] += mixed[0] * send.gain;
                        target[frame_index][1] += mixed[1] * send.gain;
                    }
                }
            }
            if !project.return_feed_plan.is_empty() {
                for &index in project.return_feed_plan.order() {
                    let bus = &project.return_buses[index];
                    let frames = &mut self.return_scratch[index][..count];
                    self.return_chains
                        .get_mut(index)
                        .ok_or(Invalid("return runtime is unavailable"))?
                        .process_block(frames);
                    if let Some(matrix) = bus.matrix {
                        matrix.apply_block(frames);
                    }
                    if bus.muted {
                        frames.fill([0.0; 2]);
                    } else {
                        for frame in frames {
                            frame[0] *= bus.gain;
                            frame[1] *= bus.gain;
                        }
                    }
                    project
                        .return_feed_plan
                        .propagate(index, &mut self.return_scratch, count);
                }
                // Main always sums in pinned bus order, not dependency order.
                for frames in &self.return_scratch {
                    for (destination, source) in output_chunk.iter_mut().zip(&frames[..count]) {
                        destination[0] += source[0];
                        destination[1] += source[1];
                    }
                }
                continue;
            }
            for (index, bus) in project.return_buses.iter().enumerate() {
                let bus_frames = self
                    .return_scratch
                    .get_mut(index)
                    .ok_or(Invalid("return scratch is unavailable"))?;
                let frames = &mut bus_frames[..count];
                if let Some(chain) = self.return_chains.get_mut(index) {
                    chain.process_block(frames);
                } else {
                    return Err(Invalid("return runtime is unavailable"));
                }
                if let Some(matrix) = bus.matrix {
                    matrix.apply_block(frames);
                }
                if !bus.muted {
                    for (destination, source) in output_chunk.iter_mut().zip(frames.iter()) {
                        destination[0] += source[0] * bus.gain;
                        destination[1] += source[1] * bus.gain;
                    }
                }
            }
        }
        Ok(())
    }
}

pub struct Renderer<'a> {
    project: &'a Project,
    position: u64,
    playing: bool,
    runtime: RendererRuntime,
}
impl<'a> Renderer<'a> {
    pub fn new(project: &'a Project) -> Self {
        Self {
            project,
            position: 0,
            playing: false,
            runtime: RendererRuntime::new(project),
        }
    }

    /// Enable effect telemetry explicitly for headless diagnostics. Offline
    /// exports leave this disabled; native instrument telemetry uses Playback.
    pub fn set_effect_metering(&mut self, enabled: bool) {
        for chain in self
            .runtime
            .device_chains
            .iter_mut()
            .chain(self.runtime.return_chains.iter_mut())
        {
            chain.set_metering(enabled);
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn seek(&mut self, position: u64) {
        if self.position != position {
            self.runtime.reset();
        }
        self.position = position;
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    /// Overwrites the output. Pausing emits silence without advancing.
    /// Runtime storage is prepared by `new`; rendering does not allocate.
    /// On position overflow returns an error without changing output or transport.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        self.runtime
            .render_without_pages(self.project, &mut self.position, self.playing, output)
    }

    /// Render with a borrowed page view. Missing streamed pages contribute
    /// silence while transport and other source contributions continue.
    pub fn render_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        self.runtime.render_with_pages(
            self.project,
            &mut self.position,
            self.playing,
            output,
            Some(pages),
        )
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
mod midi_note_bank;
mod midi_preview;
pub mod midi_template;
mod mutable_instrument;
mod prepared_revision;
mod prepared_tempo;
mod session_inventory;
pub mod session_playback;
mod shared_analytic;

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
pub use prepared_revision::assembly::{AudioPreparedRevisionMetadata, AudioRevisionRootCapacity};
pub use prepared_tempo::{
    MAX_PREPARED_TEMPO_POINTS, MidiTiming, PreparedTempoError, PreparedTempoMap,
};
pub use session_inventory::{
    PreparedSessionAudioClip, PreparedSessionCell, PreparedSessionCellContent,
    PreparedSessionInventory, PreparedSessionMidiClip, PreparedSessionScene, PreparedSessionTrack,
    SessionWorkEnvelope,
};
pub use session_playback::{
    ArrangementPrerollError, LaunchQuantization, MAX_ARRANGEMENT_PREROLL_FRAMES,
    MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES, MAX_OUTPUT_CAPTURE_FRAMES, MAX_OUTPUT_CAPTURES,
    OutputCapture, OutputCaptureError, OutputCaptureId, OutputCapturePlan, SessionHandoffReport,
    SessionPendingRouteStatus, SessionPlayback, SessionPlaybackCommand, SessionPlaybackError,
    SessionPlaybackOwner, SessionRouteStatus, SessionRouteTarget,
};
