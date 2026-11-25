//! # Purpose
//! Compile canonical projects into immutable playback revisions and render deterministic stereo
//! audio with bounded runtime state.
//!
//! # Dependencies
//! Uses hex-project, hex-devices, hex-lattice/hex-lattice-contract, hex-warp and control-side sha2
//! resource verification; no UI or device-I/O dependency.
//!
//! # Does not own
//! Does not edit canonical projects, open media files/devices, own page-worker queues or infer
//! physical deadlines from headless checks. Callback paths borrow prepared data;
//! control owns preparation and final destruction.
mod native_effect_storage;
mod serial_effect_power;
mod source_effect_stage;
pub use hex_devices::SineTable;
pub use hex_devices::auto_pan::{
    AutoPanWaveformPreview, waveform_preview as native_auto_pan_waveform_preview,
};
pub use hex_devices::compressor::{
    CompressorTransferCurve, transfer_curve as native_compressor_transfer_curve,
};
/// Native delay metadata and control-side filter display queries.
pub use hex_devices::delay;
/// Control-side EQ node editing and its opaque versioned state contract.
pub use hex_devices::eq;
pub use hex_devices::filter::response as native_filter_response;
pub use hex_devices::operator_state::algorithms::CATALOG as native_operator_algorithms;
pub use hex_devices::operator_waveform::builtin_waveform_preview as native_operator_waveform_preview;
pub use hex_devices::overdrive::{
    OverdriveFilterResponse, filter_response as native_overdrive_filter_response,
};
pub use hex_devices::reverb::{ReverbPreview, preview as native_reverb_preview};
pub use hex_devices::saturator::{
    SaturatorTransferCurve, transfer_curve as native_saturator_transfer_curve,
};
pub use hex_lattice::ProcessorAuditionSelection;
pub use hex_lattice::ProcessorChainOwner;
pub use hex_lattice_contract::DeviceTypeId;

/// Whether a track's input monitoring mode passes live input given record arm
/// and active clip playback (Ableton Live 12 §17.1 semantics).
pub fn monitoring_passes_input(
    mode: hex_project::InputMonitoring,
    armed: bool,
    clip_playing: bool,
) -> bool {
    match mode {
        hex_project::InputMonitoring::In => true,
        hex_project::InputMonitoring::Auto => armed && !clip_playing,
        hex_project::InputMonitoring::Off => false,
    }
}

/// Whether a track's input monitoring mode plays the track's own authored clips.
/// `InputMonitoring::In` suppresses clip playback; `Auto` and `Off` play clips.
pub fn monitoring_plays_clips(mode: hex_project::InputMonitoring) -> bool {
    mode != hex_project::InputMonitoring::In
}
pub use hex_warp::{Error as TransientDetectionError, detect_transients};
pub mod compensation;
pub mod foreign_automation;

pub mod midi_capture;
mod source_routing;
pub use source_routing::PreparedSourceRouting;

// Historical analytic/import proofs opt into the shared pre-global kernel.
#[cfg(test)]
#[path = "../../hex-devices/tests/support/mod.rs"]
mod operator_legacy_test;

use std::fmt;

mod audio_candidate;
mod audio_storage;
pub use audio_candidate::{
    AudioCandidateAnchors, AudioCandidateConstructorClaims, AudioCandidateConstructorSummary,
    AudioCandidateDisposalPhase, AudioCandidateDisposed, AudioCandidateDraft,
    AudioCandidateEffects, AudioCandidateIdentity, AudioCandidateOwner, AudioCandidatePreparation,
    AudioCandidateRefusal, AudioCandidateReservationQuote, AudioCandidateRuntimeDisposed,
    AudioCandidateRuntimeStatus, AudioCandidateSplitRefusal, AudioCandidateTransfer,
    ClaimedAudioCandidateRuntime, StreamPlayback,
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
    AudioArrangementStartRefusal, AudioPreparationCustody, AudioPreparationPhase,
    AudioPreparationToken, AudioSourceContext, ReturnFeedCapacity,
};
pub use session_audio_preparation::SessionSourceCapacity;
pub use session_inventory::audio_assembly::{SessionAssemblyCapacity, SessionInventoryMetadata};
pub use session_structure_preparation::SessionStructureCapacity;
pub use session_tempo_preparation::SessionTempoCapacity;
pub mod audio_transform;
mod effect_clock;
mod main_chain;
mod mixer_envelope;
mod native_device_view;
pub mod wave_executor;
pub use audio_input_storage::{
    AudioInputStorageAndObligations, AudioInputStorageRefusal, AudioInputStorageSummary,
};
pub use audio_only_inputs::{
    AudioInputsRefusal, AudioPreparationCapacity, AudioSourceToken, AudioStorageError,
    AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource, OwnedAudioSourceView,
    ResidentSourceRefusal, StreamedSourceRefusal,
};
pub use signal_graph::OwnerWave;
pub use wave_executor::{SerialWaveExecutor, WaveExecutor};
pub mod native_device_resources;
/// The loaded Sampler's existing authored gain control.
pub use hex_devices::sampler::catalog::GAIN as SAMPLER_GAIN_PARAMETER;
#[cfg(feature = "instrument-test-support")]
pub mod compiled_fade_conformance;
mod owned_shared;
mod pcm_arc;
mod sparse_audio;
pub use midi_preview::persistent::{AudioPreviewCapacity, AudioPreviewMetadata};
pub use native_device_view::{
    NativeDeviceCatalogEntry, NativeDeviceClass, NativeDeviceParameterView, NativeDeviceView,
    SamplerParameter, is_empty_sampler_state_on_control,
    is_sampler_configuration_history_on_control, native_device_catalog,
    native_device_catalog_for_class, native_device_descriptors_for_class,
    native_device_parameter_for_class, native_device_view, native_device_view_for_class,
    native_effect_frequency_response, native_operator_filter_frequency_response,
    patch_sampler_parameter_on_control, sampler_parameter_active_on_control,
    sampler_parameter_value_on_control, sampler_replacement_template_on_control,
    validate_sampler_replacement_on_control,
};
pub use sparse_audio::SparseFrameMap;

mod device_automation;
mod device_modulation;
pub use device_modulation::foreign_modulation_target as project_vst3_modulation_target;
pub use device_modulation::foreign_modulation_targets as project_vst3_modulation_targets;
pub use device_modulation::{
    device_available_modulation_outputs, fm4_note_pressure_parameter_supported,
    project_device_can_add_note_pressure, project_device_has_modulation_note_events,
    project_device_host_gain_modulation_supported,
    project_device_instrument_output_modulation_supported,
    project_device_modulation_target_supported, project_source_utility_matrix_edit_supported,
    project_source_utility_mute_edit_supported,
};
/// The authored aggregate note-gate ADSR settings.
pub use hex_devices::modulation::adsr as adsr_modulator;
/// The authored note-release attack, hold and decay settings.
pub use hex_devices::modulation::ahd_release as ahd_release_modulator;
/// Versioned, control-side settings for the built-in envelope follower.
pub use hex_devices::modulation::audio_rate as audio_rate_modulator;
/// The authored on/off source codec.
pub use hex_devices::modulation::button as button_modulator;
/// The authored pair of latched switches and its output identities.
pub use hex_devices::modulation::buttons as buttons_modulator;
pub use hex_devices::modulation::envelope_follower as follower_modulator;
/// Versioned, control-side settings for the built-in LFO modulator.
pub use hex_devices::modulation::lfo as lfo_modulator;
/// Versioned settings and control identities for the four-output macro.
pub use hex_devices::modulation::macro_four as macro_four_modulator;
/// Versioned control-side settings for the built-in manual macro source.
pub use hex_devices::modulation::macro_value as macro_modulator;
/// Versioned Math settings and its two modulation destinations.
pub use hex_devices::modulation::math as math_modulator;
/// Versioned Mix settings and its three modulation destinations.
pub use hex_devices::modulation::mix as mix_modulator;
/// The authored note counter settings. The instrument supplies sample offsets.
pub use hex_devices::modulation::note_counter as note_counter_modulator;
/// The authored note-pressure source codec. Active readings belong to the voice.
pub use hex_devices::modulation::note_pressure as note_pressure_modulator;
/// The authored Ramp duration, direction, curve and loop settings.
pub use hex_devices::modulation::ramp as ramp_modulator;
/// The authored reusable random sample-and-hold source codec.
pub use hex_devices::modulation::random_hold as random_modulator;
/// Authored step pattern and free-running timing.
pub use hex_devices::modulation::steps as steps_modulator;

mod native_parameter_input;
pub use device_automation::{
    DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED, NativeParameterInputTarget,
    native_device_automation_target_supported, native_parameter_automation_supported,
    native_parameter_input_target, project_device_automation_target_supported,
};
// Host control values cross the existing prepared native-input API.
pub use hex_lattice::{NativeTransientOperation, NativeTransientValue};
pub use native_parameter_input::{NativeParameterApplication, NativeParameterCapture};
mod gain_envelope;
pub use gain_envelope::{
    GainAutomationCapacity, GainAutomationPreview, GainEnvelopeKnot, GainPreviewSegment,
    MAX_PREPARED_GAIN_BYTES, MAX_PREPARED_GAIN_KNOTS,
};
mod clip_index;
mod loop_seam;
mod stream_miss_fade;
pub use stream_miss_fade::{STREAM_MISS_FADE_VOICES, StreamMissFades};
mod return_feed_plan;
mod signal_graph;
#[cfg(feature = "instrument-test-support")]
pub use signal_graph::PrintedBranchProbe;
pub use signal_graph::{OutputLatency, ProcessorGraphLatency};
mod stream_demand;
pub use stream_demand::{
    StreamChunkWindow, StreamPageExtent, StreamResourceExtent, visit_scalar_page_range,
};
mod streamed;
pub use streamed::{
    BorrowedMediaPage, GranularStreamIdentity, MediaPages, RepitchStreamIdentity,
    STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
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

pub use hex_lattice::{DeviceMeter, DevicePeaks, DynamicsMeter, RenderWorkSample};
pub use native_device_view::{DEVICE_INPUT_GAIN, DEVICE_OUTPUT_GAIN};

/// Borrowed stereo Main after master gain from a successful engine render.
/// This is candidate telemetry, not accepted hardware output. Callers must
/// validate their final output/window before publication. Do not allocate,
/// block, panic or retain the sample borrow in a real-time observer.
/// Paused/empty successful renders may supply silence/an empty slice.
pub type MainOutputObserver<'observer> = dyn for<'frames> FnMut(&'frames [Frame]) + 'observer;

/// Borrowed post-node samples delivered during one render call.
pub type PostNodeObserver<'observer> =
    dyn for<'frames> FnMut(PostNodeIdentity, usize, &'frames [Frame]) + 'observer;
pub use hex_lattice::NodeSignalPhase;

/// Synchronous input/output signals around one native entry, before host input
/// trim and after the entire effect/output trim. Both borrows are speculative
/// until the full render and caller's final output validation succeed.
/// Real-time observers must not allocate, block or retain either sample borrow.
pub trait NodeSignalSink {
    fn observe_signal(
        &mut self,
        identity: PostNodeIdentity,
        phase: NodeSignalPhase,
        offset: usize,
        frames: &[Frame],
    );

    /// Reports whether this observer needs synchronous samples from `owner`.
    /// Returning `false` allows `owner` to remain eligible for parallel wave
    /// dispatch even while another owner is observed.
    fn observes_owner(&self, _owner: ProcessorChainOwner) -> bool {
        true
    }
}

impl<F> NodeSignalSink for F
where
    F: for<'frames> FnMut(PostNodeIdentity, NodeSignalPhase, usize, &'frames [Frame]),
{
    fn observe_signal(
        &mut self,
        identity: PostNodeIdentity,
        phase: NodeSignalPhase,
        offset: usize,
        frames: &[Frame],
    ) {
        self(identity, phase, offset, frames);
    }
}

/// Predicate deciding which processor chain owners a [`FilteredNodeSignalObserver`]
/// observes synchronously.
pub trait NodeSignalOwnerFilter {
    fn observes_owner(&self, owner: ProcessorChainOwner) -> bool;
}

impl<P> NodeSignalOwnerFilter for P
where
    P: Fn(ProcessorChainOwner) -> bool,
{
    fn observes_owner(&self, owner: ProcessorChainOwner) -> bool {
        self(owner)
    }
}

impl NodeSignalOwnerFilter for ProcessorChainOwner {
    fn observes_owner(&self, owner: ProcessorChainOwner) -> bool {
        *self == owner
    }
}

impl NodeSignalOwnerFilter for Option<ProcessorChainOwner> {
    fn observes_owner(&self, owner: ProcessorChainOwner) -> bool {
        *self == Some(owner)
    }
}

/// Zero-allocation [`NodeSignalSink`] wrapper that marks only matching owners
/// ineligible for parallel wave dispatch.
pub struct FilteredNodeSignalObserver<F, P> {
    filter: P,
    observe: F,
}

impl<F, P> FilteredNodeSignalObserver<F, P>
where
    F: for<'frames> FnMut(PostNodeIdentity, NodeSignalPhase, usize, &'frames [Frame]),
    P: NodeSignalOwnerFilter,
{
    pub const fn new(filter: P, observe: F) -> Self {
        Self { filter, observe }
    }
}

impl<F> FilteredNodeSignalObserver<F, ProcessorChainOwner>
where
    F: for<'frames> FnMut(PostNodeIdentity, NodeSignalPhase, usize, &'frames [Frame]),
{
    pub const fn for_owner(owner: ProcessorChainOwner, observe: F) -> Self {
        Self::new(owner, observe)
    }
}

impl<F, P> NodeSignalSink for FilteredNodeSignalObserver<F, P>
where
    F: for<'frames> FnMut(PostNodeIdentity, NodeSignalPhase, usize, &'frames [Frame]),
    P: NodeSignalOwnerFilter,
{
    fn observe_signal(
        &mut self,
        identity: PostNodeIdentity,
        phase: NodeSignalPhase,
        offset: usize,
        frames: &[Frame],
    ) {
        (self.observe)(identity, phase, offset, frames);
    }

    fn observes_owner(&self, owner: ProcessorChainOwner) -> bool {
        self.filter.observes_owner(owner)
    }
}

/// Shifts observed frame offsets by `offset` while forwarding `observes_owner`
/// from the wrapped [`NodeSignalObserver`].
pub struct ShiftedNodeSignalObserver<'a, 'observer> {
    inner: &'a mut NodeSignalObserver<'observer>,
    offset: usize,
}

impl<'a, 'observer> ShiftedNodeSignalObserver<'a, 'observer> {
    pub fn new(inner: &'a mut NodeSignalObserver<'observer>, offset: usize) -> Self {
        Self { inner, offset }
    }
}

impl NodeSignalSink for ShiftedNodeSignalObserver<'_, '_> {
    fn observe_signal(
        &mut self,
        identity: PostNodeIdentity,
        phase: NodeSignalPhase,
        at: usize,
        frames: &[Frame],
    ) {
        self.inner
            .observe_signal(identity, phase, self.offset.saturating_add(at), frames);
    }

    fn observes_owner(&self, owner: ProcessorChainOwner) -> bool {
        self.inner.observes_owner(owner)
    }
}

pub(crate) struct PostNodeSignalAdapter<'a, 'observer>(
    pub(crate) &'a mut PostNodeObserver<'observer>,
);

impl NodeSignalSink for PostNodeSignalAdapter<'_, '_> {
    fn observe_signal(
        &mut self,
        identity: PostNodeIdentity,
        phase: NodeSignalPhase,
        offset: usize,
        frames: &[Frame],
    ) {
        if phase == NodeSignalPhase::Output {
            (self.0)(identity, offset, frames);
        }
    }
}

/// Synchronous input/output signals around one native entry, before host input
/// trim and after the entire effect/output trim. Both borrows are speculative
/// until the full render and caller's final output validation succeed.
/// Real-time observers must not allocate, block or retain either sample borrow.
pub type NodeSignalObserver<'observer> = dyn NodeSignalSink + 'observer;

/// Maximum stable track routes exposed to native callback telemetry.
pub const MAX_SESSION_ROUTE_TRACKS: usize = hex_project::MAX_TRACKS;

mod clip;
mod project;
mod renderer;
mod streamed_span;
mod track;

pub use clip::{Audio, Clip, PreparedAudioClipView, PreparedAudioPeak, PreparedAudioRepeat, Tempo};
pub(crate) use clip::{AudioData, ClipMetadata};
pub use project::{PreparedMasterGain, Project};
pub use renderer::Renderer;
pub(crate) use renderer::{RendererRuntime, render_source_generators};
pub(crate) use streamed_span::{
    StreamedPageSpan, render_streamed_page_span, render_streamed_page_span_captured,
};
pub(crate) use track::{CompiledSend, InstrumentTrack, ReturnBus, TrackSoloSelection};
pub use track::{PreparedTrackMix, Track};

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
        NativeDeviceClass::Instrument => {
            let state = hex_devices::native_instrument_registry()
                .map_err(|_| Invalid("invalid built-in native instrument registry"))?
                .default_state(type_id)
                .map_err(|_| Invalid("native instrument default state is invalid"))?;
            // A new project sampler takes its older-reader saved form (v2), not
            // the registry's in-memory current version (Pulse #4088).
            if type_id == hex_devices::sampler::state::TYPE_ID {
                return hex_devices::sampler::state::saved_form(&state)
                    .map_err(|_| Invalid("native instrument default state is invalid"));
            }
            Ok(state)
        }
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

/// A bounded control-side transaction; publication happens only after preparation.
pub fn patch_native_device_snapshot_for_class(
    class: NativeDeviceClass,
    state: &hex_lattice_contract::NativeDeviceState,
    snapshot: &hex_lattice_contract::DeviceParameterSnapshot,
) -> Result<hex_lattice_contract::NativeDeviceState, Invalid> {
    let result = match class {
        NativeDeviceClass::Effect => hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?
            .patch_parameters(state, snapshot),
        NativeDeviceClass::Instrument => hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?
            .patch_parameters(state, snapshot),
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
        Error::AssetResourcesUnavailable => "native device asset resources are unavailable",
        Error::ForeignProcessingUnavailable => {
            "foreign processing resources are unavailable for this graph"
        }
        Error::ForeignProcessingFailed => "foreign processor exchange failed",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// New project samplers take the older-reader saved form (Pulse #4088).
    #[test]
    fn default_sampler_instrument_is_the_saved_v2_empty_marker() {
        use hex_devices::sampler::state;
        let sampler =
            default_native_device_state_for_class(NativeDeviceClass::Instrument, state::TYPE_ID)
                .unwrap();
        assert_eq!(sampler, state::empty_state().unwrap());
        assert_eq!(sampler.state_version(), state::COMPATIBLE_STATE_VERSION);
        assert!(is_empty_sampler_state_on_control(&sampler));
    }
}

pub mod computer_keyboard;
pub mod live;
pub mod live_expression;
mod midi_activity;
mod playback_observation;
pub use playback_observation::{
    PreparedDeviceProcessing, PreparedMidiActivity, PreparedPlaybackObservation,
};
mod launch_quantization;
mod midi_clip_volume;
pub mod midi_compile;
mod midi_note_bank;
mod midi_preview;
pub mod midi_template;
mod mutable_instrument;
mod prepared_revision;
mod prepared_tempo;
pub use prepared_tempo::CaptureTempo;
mod session_inventory;
pub mod session_playback;
mod shared_analytic;
pub mod tuning_audition;

pub mod compile;

pub mod playback;
mod scalar_automation;
pub use scalar_automation::ScalarAutomationBinding;
mod scalar_streamed;
pub use scalar_streamed::{
    BorrowedScalarPage, ScalarFileIdentity, ScalarProjectionIdentity, ScalarSourceKey,
    ScalarSourceMetadata, ScalarSourceTarget, StreamResourceKey, StreamResourceMetadata,
    read_scalar_values,
};
mod project_output;
pub use hex_project::{MAX_RETURN_TRACKS, ReturnId};
pub use midi_preview::{
    MAX_MIDI_PREVIEW_SAMPLES_PER_CLIP, MidiArrangementPreview, MidiClipPreview, MidiPreviewSample,
    midi_clip_preview,
};
pub use midi_template::{
    MIDI_SPAN_BUFFER_CAPACITY, MIDI_TEMPLATE_MAX_EDGES, MIDI_TEMPLATE_MAX_ONSETS,
    MIDI_TEMPLATE_QUERY_FRAMES, MidiEdgeBuffer, MidiInstrumentRenderContext, MidiRenderSpan,
    MidiSpanBuffer, MidiTemplateError, PreparedMidiInstrumentTemplate, PreparedMidiSourceTemplate,
    SessionMidiOrigin,
};
pub use playback::{
    MAX_TIMED_TRANSPORT_COMMANDS, NoteChase, PlaybackAction, PlaybackActionError,
    PlaybackActionRejection, PlaybackCommand, PlaybackHandoffError, PlaybackHandoffRejection,
    SessionCapability, SessionCapabilityStatus, TimedTransportCommand, TimedTransportError,
    TransportFadeCompletion,
};
pub use prepared_revision::PreparedRevision;
pub use prepared_revision::assembly::{AudioPreparedRevisionMetadata, AudioRevisionRootCapacity};
pub use prepared_revision::foreign_programs::ForeignProgramFrontier;
pub use prepared_tempo::{
    MAX_PREPARED_TEMPO_POINTS, MidiTiming, PreparedTempoError, PreparedTempoMap, SessionClockAnchor,
};
pub use session_inventory::{
    PreparedSessionAudioClip, PreparedSessionCell, PreparedSessionCellContent,
    PreparedSessionInventory, PreparedSessionMidiClip, PreparedSessionScene, PreparedSessionTrack,
    SessionWorkEnvelope,
};
pub use session_playback::route_transitions::{
    AppliedSessionRouteTarget, AppliedSessionRouteTransition, MAX_APPLIED_SESSION_ROUTE_TRANSITIONS,
};
pub use session_playback::{
    ArrangementPrerollError, LaunchQuantization, MAX_ARRANGEMENT_PREROLL_FRAMES,
    MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES, MAX_OUTPUT_CAPTURE_FRAMES, MAX_OUTPUT_CAPTURES,
    OutputCapture, OutputCaptureError, OutputCaptureId, OutputCapturePlan, PausedKeyboardMonitor,
    PausedMonitorAdmissionError, PausedMonitorForeignBinding, PausedMonitorStorageHold,
    PausedSourceMonitor, PausedTrackMonitor, PreparedPrerollPages, PreparedStreamAction,
    ProjectOutputTap, ProjectSignal, ProjectSignalObserver, SessionHandoffReport,
    SessionPendingRouteStatus, SessionPlayback, SessionPlaybackCommand, SessionPlaybackError,
    SessionPlaybackOwner, SessionRouteStatus, SessionRouteTarget,
};

#[cfg(test)]
mod main_chain_review_tests;

#[cfg(test)]
#[path = "../tests/support/routed_midi_fixture.rs"]
mod routed_midi_fixture;
