//! Registered instrument preparation and compact immutable note samplers.
//!
//! Effects and instruments resolve through separate registries; a missing or malformed present
//! instrument must refuse rather than fall back to another type. Preparation validates authored
//! state and tuning on control. Analytic notes are copyable scalar samplers; shared contexts and
//! mutable factories declare their own bounded storage and processing capabilities. Mutable
//! committed/scratch histories participate in a whole-block transaction, with compatibility
//! checked before any history transfer. These trusted native interfaces provide no foreign-code
//! sandbox or host resource lease.
use super::{MAX_NATIVE_DEVICE_TYPES, PreparedMutableInstrumentRuntime, StereoFrame};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ModulationSourceId, NativeDeviceMetadata, NativeDeviceState,
};
use std::fmt;

/// Maximum storage carried by one immutable analytic note sampler. The sampler
/// plus its release count, sampler and continuation descriptor occupies 352 bytes on 64-bit
/// targets. The fixed payload can hold the validated standalone FM4 kernel.
pub const ANALYTIC_NOTE_PAYLOAD_WORDS: usize = 41;

/// Shared analytic instruments prepare a compact note payload against one
/// immutable configuration context. This payload is deliberately smaller than
/// `PreparedAnalyticNote`, whose existing fixed layout remains unchanged.
pub const SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS: usize = 8;

/// Maximum declared immutable context storage admitted by the engine for one
/// shared analytic configuration. The declaration includes all context-owned
/// allocations and is validated on the control side.
pub const MAX_SHARED_ANALYTIC_CONTEXT_BYTES: usize = 4_096;

/// Copyable per-note data prepared by a shared immutable context. Context
/// implementations must make this payload self-contained with respect to the
/// borrowed context and deterministic for a fixed context, pitch, and velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedSharedAnalyticNote {
    payload: [u64; SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
    release_frames: u64,
}

impl PreparedSharedAnalyticNote {
    pub const fn new(
        payload: [u64; SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
        release_frames: u64,
    ) -> Self {
        Self {
            payload,
            release_frames,
        }
    }

    pub const fn release_frames(&self) -> u64 {
        self.release_frames
    }

    pub const fn payload(&self) -> &[u64; SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS] {
        &self.payload
    }
}

/// Control-prepared immutable configuration shared by many note occurrences.
/// Implementations perform all validation and pitch/velocity preparation in
/// `prepare_note`; `sample` borrows compact note data and must be deterministic,
/// allocation-free, and free of interior mutation.
pub trait PreparedSharedAnalyticContext: Send + Sync {
    /// Optional provider identity for an explicitly authorized analytic-to-mutable import.
    fn import_identity(&self) -> Option<&dyn std::any::Any> {
        None
    }
    /// Optional provider-defined coefficient continuation. Default providers
    /// retain their immutable sampler law across publication. Opted-in methods
    /// must be bounded, read-only and allocation-free, including refusal paths.
    fn analytic_continuation(&self) -> Option<&dyn crate::PreparedAnalyticContinuation> {
        None
    }

    /// Total bytes owned by this context, including allocations reachable only
    /// through it. The host checks this against its finite per-context limit.
    fn declared_context_bytes(&self) -> usize;

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError>;

    fn sample(
        &self,
        note: &PreparedSharedAnalyticNote,
        age_frames: u64,
        gate_frames: u64,
        _resources: &dyn crate::DeviceResources,
    ) -> StereoFrame;
}

/// Provider callback for bounded, read-only continuation from actual source
/// coefficients to target coefficients at the supplied occurrence age.
pub type ContinueAnalyticPayload =
    fn(
        &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        u64,
    ) -> Result<[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], DeviceContractError>;

/// Static provider authority for per-occurrence pitch continuation. The provider
/// validates every nonpitch coefficient and preserves authored phase/envelope law.
/// Static descriptor identity, rather than function-address equality, binds formats.
/// The callback must be bounded, allocation-free and free of interior mutation.
pub struct AnalyticNoteContinuation {
    continue_payload: ContinueAnalyticPayload,
}
impl AnalyticNoteContinuation {
    pub const fn new(continue_payload: ContinueAnalyticPayload) -> Self {
        Self { continue_payload }
    }
}

/// Type-erased immutable note data, sampled by authored age and gate duration
/// without allocation or state mutation. The callback borrows the fixed payload.
#[derive(Clone, Copy)]
pub struct PreparedAnalyticNote {
    payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    release_frames: u64,
    continuation: Option<&'static AnalyticNoteContinuation>,
    sample_fn: fn(
        &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        u64,
        u64,
        u64,
        &dyn crate::DeviceResources,
    ) -> StereoFrame,
}

impl fmt::Debug for PreparedAnalyticNote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAnalyticNote")
            .field("release_frames", &self.release_frames)
            .finish_non_exhaustive()
    }
}

impl PreparedAnalyticNote {
    /// Prepare a bounded analytic note sampler. Zero release is valid for
    /// instruments with an instantaneous note-off.
    pub const fn new(
        payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        release_frames: u64,
        sample_fn: fn(
            &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
            u64,
            u64,
            u64,
            &dyn crate::DeviceResources,
        ) -> StereoFrame,
    ) -> Self {
        Self {
            payload,
            release_frames,
            continuation: None,
            sample_fn,
        }
    }

    pub const fn release_frames(&self) -> u64 {
        self.release_frames
    }

    pub const fn with_continuation(mut self, provider: &'static AnalyticNoteContinuation) -> Self {
        self.continuation = Some(provider);
        self
    }

    /// Prepare this exact target from the actually applied occurrence sampler.
    /// The returned copy retains target authority and unchanged release duration;
    /// hosts must preflight all carried notes before publishing any replacement.
    pub fn continue_from(
        &self,
        source: &Self,
        age_frames: u64,
    ) -> Result<Self, DeviceContractError> {
        let (Some(target), Some(previous)) = (self.continuation, source.continuation) else {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        };
        if !std::ptr::eq(target, previous) || self.release_frames != source.release_frames {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let payload = (target.continue_payload)(&source.payload, &self.payload, age_frames)?;
        Ok(Self { payload, ..*self })
    }

    /// Sample one stereo frame by authored age and gate duration, both in
    /// frames. The instrument definition owns the deterministic transfer law.
    pub fn sample(
        &self,
        age_frames: u64,
        gate_frames: u64,
        resources: &dyn crate::DeviceResources,
    ) -> StereoFrame {
        (self.sample_fn)(
            &self.payload,
            age_frames,
            gate_frames,
            self.release_frames,
            resources,
        )
    }
}

/// Control-side declaration of the entire exclusively owned provider pointee
/// and nested heap payload. This is not allocator/RSS measurement or a DSP
/// runtime declaration. Shared, Weak, or opaque children require Unknown.
/// Static program storage is outside graph ownership and must be documented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedStorageDeclaration {
    Unknown,
    ExclusiveOnly { bytes: usize },
}

/// Immutable sample-rate-specific instrument configuration prepared on the
/// control side. Note samplers are compact copyable values so release tails can
/// retain their exact source law without callback allocation or final drops.
pub trait PreparedNativeInstrument: Send + Sync {
    /// Fixed algorithmic buffering latency at this prepared sample rate.
    /// Unknown is explicit; envelope attack, filter phase and musical release
    /// are not buffering latency. A mutable factory must agree with its source.
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        None
    }

    /// Prepare the immutable sampler for one authored pitch/velocity pair.
    /// Implementations must be deterministic for a fixed prepared instrument
    /// configuration, sample rate, pitch and velocity. The engine may intern
    /// one returned sampler per pair instead of preparing one per note
    /// occurrence, so this method must not encode occurrence-specific state.
    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError>;

    /// Control-side validation before admitting authored mutable note sources.
    /// Devices with pitch-dependent domains must reject unsupported excursions
    /// here as well as in their callback event preflight.
    fn validate_event_note(&self, _pitch: u8, _velocity: u8) -> Result<(), DeviceContractError> {
        Ok(())
    }

    /// Optional shared immutable analytic context. For the same prepared
    /// instrument state and sample rate, context preparation must describe the
    /// same future PCM law; the host may reuse a prior context with exact state
    /// identity. `prepare_note` must be deterministic for fixed context,
    /// pitch and velocity. When present, the engine stores one bounded context
    /// owner per prepared instrument configuration and small copyable note
    /// payloads in source banks and releases. The context is created on the
    /// control side, never in the callback.
    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn PreparedSharedAnalyticContext>>, DeviceContractError> {
        Ok(None)
    }

    /// Optional fixed-storage mutable voice runtime. The default keeps
    /// existing instruments analytic-only. A mutable-only instrument may
    /// return an explicit unsupported-class error from `prepare_note`; callers
    /// must select this capability and must never substitute an analytic
    /// sampler when note preparation fails.
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(None)
    }
    /// Prepare the same mutable provider with one occurrence-pressure input.
    /// A slot can collect pressure before any route is assigned. Unsupported
    /// providers must refuse instead of silently falling back to stock playback.
    fn prepare_mutable_note_pressure(
        &self,
        _source: ModulationSourceId,
        _route: Option<crate::PreparedPressureRoute>,
    ) -> Result<Box<dyn PreparedMutableInstrumentRuntime>, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    /// Once opted in, the control compiler keeps this device's continuation
    /// runtime across subsequent revisions, including Undo to an analytic state.
    /// New opens and never-authored devices retain ordinary capability selection.
    fn prefers_mutable_continuation(&self) -> bool {
        false
    }
    fn prepare_mutable_continuation_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(None)
    }
    /// Alternative single-owner law plus both mutable histories. Default absent
    /// preserves all existing instrument factories. Host selection MUST reject
    /// simultaneous independent and owned-pair mutable capabilities; neither
    /// may silently substitute for a failed/unsupported capability.
    fn prepare_owned_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn crate::PreparedOwnedInstrumentRuntime>>, DeviceContractError> {
        Ok(None)
    }
}

/// Registered analytic instrument definition. Instruments use a separate
/// registry from effects because they prepare authored notes rather than a
/// stereo effect matrix.
pub trait NativeInstrumentDefinition: NativeDeviceMetadata {
    /// Pre-allocation bound for one inline-only resource factory/prepared module payload.
    /// Default unknown; resource-aware hosts cross-check the actual factory's declaration after
    /// compilation. This quote grants neither source custody nor heap/graph provenance.
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        None
    }
    /// Opt into immutable, host-retained resource inventories. The default
    /// refuses asset bindings before migration or provider execution.
    fn supports_asset_resources(&self) -> bool {
        false
    }
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError>;

    /// Prepare against an explicit host runtime ceiling. Existing providers
    /// retain their ordinary factory; runtime construction validates its limits.
    fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
        _limits: crate::NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_instrument(state, context)
    }
}

/// Bounded immutable registry for native instruments.
pub struct NativeInstrumentRegistry<'a> {
    definitions: &'a [&'a dyn NativeInstrumentDefinition],
}

impl<'a> NativeInstrumentRegistry<'a> {
    pub fn new(
        definitions: &'a [&'a dyn NativeInstrumentDefinition],
    ) -> Result<Self, DeviceContractError> {
        if definitions.len() > MAX_NATIVE_DEVICE_TYPES {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        for (index, definition) in definitions.iter().enumerate() {
            let type_id = NativeDeviceMetadata::type_id(*definition);
            type_id.validate()?;
            if definition.display_name().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 128
                    || name.trim() != name
                    || name
                        .chars()
                        .any(|character| character.is_control() || character.is_uppercase())
            }) {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            if definition.state_version() == 0 {
                return Err(DeviceContractError::InvalidStateVersion);
            }
            if definitions[..index]
                .iter()
                .any(|prior| NativeDeviceMetadata::type_id(*prior) == type_id)
            {
                return Err(DeviceContractError::DuplicateDeviceType);
            }
            hex_lattice_contract::validate_parameter_descriptors(definition.descriptors())?;
        }
        Ok(Self { definitions })
    }

    pub fn definition(&self, type_id: DeviceTypeId) -> Option<&'a dyn NativeInstrumentDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| NativeDeviceMetadata::type_id(*definition) == type_id)
    }

    pub fn definitions(&self) -> &'a [&'a dyn NativeInstrumentDefinition] {
        self.definitions
    }

    pub fn default_state(
        &self,
        type_id: DeviceTypeId,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(type_id)
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let state = definition.default_state()?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&state)?
                .with_host_metadata_from(&state)?,
        )
    }

    pub fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )
    }

    pub fn patch_parameters(
        &self,
        state: &NativeDeviceState,
        snapshot: &hex_lattice_contract::DeviceParameterSnapshot,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        hex_lattice_contract::validate_parameter_snapshot_for_write(
            definition, &current, snapshot,
        )?;
        let patched = definition.patch_parameters(&current, snapshot)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&patched)?
                .with_host_metadata_from(state)?,
        )
    }

    pub fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        hex_lattice_contract::validate_parameter_write(definition, &current, parameter, value)?;
        let patched = definition.patch_parameter(&current, parameter, value)?;
        self.validate_current(
            definition,
            definition
                .migrate_state(&patched)?
                .with_host_metadata_from(state)?,
        )
    }

    /// Prepare the exact effective configuration. Ordinary definitions refuse resource bindings
    /// before migration; opted-in definitions receive a validated inventory whose lifetime remains
    /// the host's responsibility.
    pub fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_instrument_checked(state, context, None)
    }

    /// The same state/capability/resource validation with explicit host limits.
    pub fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
        limits: crate::NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_instrument_checked(state, context, Some(limits))
    }

    fn prepare_instrument_checked(
        &self,
        state: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
        limits: Option<crate::NativeInstrumentRuntimeLimits>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        if !definition.supports_asset_resources() {
            state.require_asset_free_processing()?;
        }
        context.validate_for(state)?;
        let current = self.validate_current(
            definition,
            definition
                .migrate_state(state)?
                .with_host_metadata_from(state)?,
        )?;
        match limits {
            Some(limits) => {
                definition.prepare_instrument_with_runtime_limits(&current, context, limits)
            }
            None => definition.prepare_instrument(&current, context),
        }
    }

    fn validate_current(
        &self,
        definition: &'a dyn NativeInstrumentDefinition,
        state: NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        state.type_id().validate()?;
        if state.type_id() != definition.type_id() {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != definition.state_version() {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        // Validate descriptors once. Current values are checked on read,
        // patch, and presentation so large state codecs are not decoded once
        // per control at every registry boundary.
        hex_lattice_contract::validated_state_descriptors(definition, &state)?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{
        DeviceParameterSnapshot, ParameterDescriptor, ParameterId, ParameterKind, ParameterValue,
    };

    const PATCH_PARAMETER: ParameterId = ParameterId::from_static(1);
    const PATCH_DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
        id: PATCH_PARAMETER,
        label: "level",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.25),
        presentation: hex_lattice_contract::ParameterPresentation::Linear,
    }];
    const PATCH_INPUT_GAIN: i32 = -12_345;
    const PATCH_OUTPUT_GAIN: i32 = 6_789;

    struct SnapshotPatchDefinition;

    impl NativeDeviceMetadata for SnapshotPatchDefinition {
        fn type_id(&self) -> DeviceTypeId {
            DeviceTypeId::from_static("test.instrument-snapshot-patch")
        }

        fn state_version(&self) -> u32 {
            1
        }

        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            PATCH_DESCRIPTORS
        }

        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(self.type_id(), 1, vec![64])
        }

        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != self.type_id() || state.state_version() != 1 {
                return Err(DeviceContractError::MalformedState);
            }
            NativeDeviceState::new(self.type_id(), 1, state.payload().to_vec())
        }

        fn read_parameter(
            &self,
            state: &NativeDeviceState,
            parameter: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            if parameter != PATCH_PARAMETER {
                return Err(DeviceContractError::UnknownParameter);
            }
            let value = *state
                .payload()
                .first()
                .ok_or(DeviceContractError::MalformedState)?;
            Ok(ParameterValue::Float(f32::from(value) / 255.0))
        }

        fn patch_parameter(
            &self,
            _state: &NativeDeviceState,
            parameter: ParameterId,
            value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if parameter != PATCH_PARAMETER {
                return Err(DeviceContractError::UnknownParameter);
            }
            let ParameterValue::Float(value) = value else {
                return Err(DeviceContractError::InvalidParameterValue);
            };
            NativeDeviceState::new(self.type_id(), 1, vec![(value * 255.0).round() as u8])
        }

        fn patch_parameters(
            &self,
            state: &NativeDeviceState,
            snapshot: &DeviceParameterSnapshot,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.input_gain_millidb() != PATCH_INPUT_GAIN
                || state.output_gain_millidb() != PATCH_OUTPUT_GAIN
            {
                return Err(DeviceContractError::MalformedState);
            }
            let mut patched = self.migrate_state(state)?;
            for &(parameter, value) in snapshot.entries() {
                patched = self.patch_parameter(&patched, parameter, value)?;
            }
            Ok(patched)
        }
    }

    impl NativeInstrumentDefinition for SnapshotPatchDefinition {
        fn prepare_instrument(
            &self,
            _state: &NativeDeviceState,
            context: &crate::DevicePreparationContext<'_>,
        ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
            if !context.tuning.is_neutral() {
                return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
            }
            Err(DeviceContractError::MalformedState)
        }
    }

    #[test]
    fn snapshot_patch_preserves_host_gains_across_migrations() {
        let definition = SnapshotPatchDefinition;
        let definitions: [&dyn NativeInstrumentDefinition; 1] = [&definition];
        let registry = NativeInstrumentRegistry::new(&definitions).unwrap();
        let state = registry
            .default_state(definition.type_id())
            .unwrap()
            .with_input_gain_millidb(PATCH_INPUT_GAIN)
            .unwrap()
            .with_output_gain_millidb(PATCH_OUTPUT_GAIN)
            .unwrap()
            .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                asset: hex_lattice_contract::DeviceAssetKey::new(42).unwrap(),
            }])
            .unwrap();
        let snapshot =
            DeviceParameterSnapshot::new(&[(PATCH_PARAMETER, ParameterValue::Float(1.0))]).unwrap();

        let patched = registry.patch_parameters(&state, &snapshot).unwrap();

        assert_eq!(patched.input_gain_millidb(), PATCH_INPUT_GAIN);
        assert_eq!(patched.output_gain_millidb(), PATCH_OUTPUT_GAIN);
        assert_eq!(patched.asset_references(), state.asset_references());
        assert_eq!(
            definition
                .read_parameter(&patched, PATCH_PARAMETER)
                .unwrap(),
            ParameterValue::Float(1.0)
        );
    }

    struct TestToneDefinition {
        id: &'static str,
        sampler: fn(
            &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
            u64,
            u64,
            u64,
            &dyn crate::DeviceResources,
        ) -> StereoFrame,
        payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    }
    struct TestTonePrepared {
        sampler: fn(
            &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
            u64,
            u64,
            u64,
            &dyn crate::DeviceResources,
        ) -> StereoFrame,
        payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    }

    impl NativeDeviceMetadata for TestToneDefinition {
        fn type_id(&self) -> DeviceTypeId {
            DeviceTypeId::from_static(self.id)
        }
        fn state_version(&self) -> u32 {
            1
        }
        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            &[]
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(self.type_id(), 1, Vec::new())
        }
        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != self.type_id()
                || state.state_version() != 1
                || !state.payload().is_empty()
            {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(state.clone())
        }
        fn read_parameter(
            &self,
            _state: &NativeDeviceState,
            _parameter: ParameterId,
        ) -> Result<ParameterValue, DeviceContractError> {
            Err(DeviceContractError::UnknownParameter)
        }
        fn patch_parameter(
            &self,
            _state: &NativeDeviceState,
            _parameter: ParameterId,
            _value: ParameterValue,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            Err(DeviceContractError::UnknownParameter)
        }
    }
    impl NativeInstrumentDefinition for TestToneDefinition {
        fn prepare_instrument(
            &self,
            _state: &NativeDeviceState,
            context: &crate::DevicePreparationContext<'_>,
        ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
            let sample_rate = context.sample_rate;
            if !context.tuning.is_neutral() {
                return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
            }
            if !(8_000..=192_000).contains(&sample_rate) {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            Ok(Box::new(TestTonePrepared {
                sampler: self.sampler,
                payload: self.payload,
            }))
        }
    }
    impl PreparedNativeInstrument for TestTonePrepared {
        fn prepare_note(
            &self,
            _pitch: u8,
            _velocity: u8,
        ) -> Result<PreparedAnalyticNote, DeviceContractError> {
            let mut payload = [0; ANALYTIC_NOTE_PAYLOAD_WORDS];
            payload[..self.payload.len()].copy_from_slice(&self.payload);
            Ok(PreparedAnalyticNote::new(payload, 8, self.sampler))
        }
    }

    fn constant_left(
        payload: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        age: u64,
        gate: u64,
        release: u64,
        _resources: &dyn crate::DeviceResources,
    ) -> StereoFrame {
        if age < gate + release {
            [f32::from_bits(payload[0] as u32), 0.0]
        } else {
            [0.0, 0.0]
        }
    }
    fn constant_right(
        payload: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
        age: u64,
        gate: u64,
        release: u64,
        _resources: &dyn crate::DeviceResources,
    ) -> StereoFrame {
        if age < gate + release {
            [0.0, f32::from_bits(payload[0] as u32)]
        } else {
            [0.0, 0.0]
        }
    }

    #[test]
    fn two_independent_instrument_definitions_prepare_and_sample_without_type_switches() {
        let mut left_payload = [0; ANALYTIC_NOTE_PAYLOAD_WORDS];
        left_payload[0] = u64::from(0.25_f32.to_bits());
        let left = TestToneDefinition {
            id: "test.instrument.left",
            sampler: constant_left,
            payload: left_payload,
        };
        let mut right_payload = [0; ANALYTIC_NOTE_PAYLOAD_WORDS];
        right_payload[0] = u64::from(0.75_f32.to_bits());
        let right = TestToneDefinition {
            id: "test.instrument.right",
            sampler: constant_right,
            payload: right_payload,
        };
        let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
        let registry = NativeInstrumentRegistry::new(&definitions).unwrap();
        for (type_id, expected) in [
            (left.type_id(), [0.25, 0.0]),
            (right.type_id(), [0.0, 0.75]),
        ] {
            let state = registry.default_state(type_id).unwrap();
            let instrument = registry
                .prepare_instrument(
                    &state,
                    &crate::DevicePreparationContext {
                        sample_rate: 48_000,
                        tuning: &crate::TuningTable::default(),
                        resources: &crate::EMPTY_DEVICE_RESOURCES,
                    },
                )
                .unwrap();
            let note = instrument.prepare_note(60, 100).unwrap();
            assert_eq!(note.sample(0, 32, &crate::EMPTY_DEVICE_RESOURCES), expected);
            assert_eq!(
                note.sample(39, 32, &crate::EMPTY_DEVICE_RESOURCES),
                expected
            );
            assert_eq!(
                note.sample(40, 32, &crate::EMPTY_DEVICE_RESOURCES),
                [0.0, 0.0]
            );
        }
    }
    #[test]
    fn legacy_provider_accepts_exact_neutral_and_refuses_custom_table() {
        let provider = TestToneDefinition {
            id: "test.instrument.neutral",
            sampler: constant_left,
            payload: [0; ANALYTIC_NOTE_PAYLOAD_WORDS],
        };
        let definitions: [&dyn NativeInstrumentDefinition; 1] = [&provider];
        let registry = NativeInstrumentRegistry::new(&definitions).unwrap();
        let state = registry.default_state(provider.type_id()).unwrap();
        assert!(
            registry
                .prepare_instrument(
                    &state,
                    &crate::DevicePreparationContext {
                        sample_rate: 48000,
                        tuning: &crate::TuningTable::default(),
                        resources: &crate::EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_ok()
        );
        let custom = crate::TuningTable::new([333.0; 128]).unwrap();
        assert!(matches!(
            registry.prepare_instrument(
                &state,
                &crate::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &custom,
                    resources: &crate::EMPTY_DEVICE_RESOURCES
                }
            ),
            Err(DeviceContractError::UnsupportedProcessingClass)
        ));
    }
    #[test]
    fn inline_continuation_requires_same_static_authority_and_exact_release_law() {
        fn carry(
            source: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
            _: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
            _: u64,
        ) -> Result<[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], DeviceContractError> {
            Ok(*source)
        }
        static A: AnalyticNoteContinuation = AnalyticNoteContinuation::new(carry);
        static B: AnalyticNoteContinuation = AnalyticNoteContinuation::new(carry);
        let a = PreparedAnalyticNote::new([0; ANALYTIC_NOTE_PAYLOAD_WORDS], 8, constant_left)
            .with_continuation(&A);
        let b = PreparedAnalyticNote::new([0; ANALYTIC_NOTE_PAYLOAD_WORDS], 8, constant_left)
            .with_continuation(&B);
        assert!(a.continue_from(&a, 777).is_ok());
        assert!(
            a.continue_from(&b, 777).is_err(),
            "same callback does not grant foreign provider authority"
        );
        assert!(
            PreparedAnalyticNote::new([0; ANALYTIC_NOTE_PAYLOAD_WORDS], 9, constant_left)
                .with_continuation(&A)
                .continue_from(&a, 777)
                .is_err()
        );
        assert!(
            PreparedAnalyticNote::new([0; ANALYTIC_NOTE_PAYLOAD_WORDS], 8, constant_left)
                .continue_from(&a, 777)
                .is_err()
        );
        assert_eq!(std::mem::size_of::<PreparedAnalyticNote>(), 352);
    }
}
