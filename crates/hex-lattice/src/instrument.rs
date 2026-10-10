//! Registered analytic instrument preparation and compact immutable note samplers.
use super::{
    MAX_NATIVE_DEVICE_TYPES, MAX_PARAMETERS_PER_DEVICE, PreparedMutableInstrumentRuntime,
    StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
};
use std::fmt;

/// Maximum storage carried by one immutable analytic note sampler. The sampler
/// plus its release count and function pointer occupies 344 bytes on 64-bit
/// targets. The fixed payload can hold the validated standalone FM4 kernel.
pub const ANALYTIC_NOTE_PAYLOAD_WORDS: usize = 41;

/// Type-erased immutable analytic note data. The safe function pointer is
/// prepared on the control side and receives authored age and gate duration;
/// `sample` borrows this value to avoid copying its bounded payload per frame;
/// it performs no allocation or state mutation.
#[derive(Clone, Copy)]
pub struct PreparedAnalyticNote {
    payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    release_frames: u64,
    sample_fn: fn(&[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], u64, u64, u64) -> StereoFrame,
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
        sample_fn: fn(&[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], u64, u64, u64) -> StereoFrame,
    ) -> Self {
        Self {
            payload,
            release_frames,
            sample_fn,
        }
    }

    pub const fn release_frames(&self) -> u64 {
        self.release_frames
    }

    /// Sample one stereo frame by authored age and gate duration, both in
    /// frames. The instrument definition owns the deterministic transfer law.
    pub fn sample(&self, age_frames: u64, gate_frames: u64) -> StereoFrame {
        (self.sample_fn)(&self.payload, age_frames, gate_frames, self.release_frames)
    }
}

/// Immutable sample-rate-specific instrument configuration prepared on the
/// control side. Note samplers are compact copyable values so release tails can
/// retain their exact source law without callback allocation or final drops.
pub trait PreparedNativeInstrument: Send + Sync {
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
}

/// Registered analytic instrument definition. Instruments use a separate
/// registry from effects because they prepare authored notes rather than a
/// stereo effect matrix.
pub trait NativeInstrumentDefinition: NativeDeviceMetadata {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError>;
}

/// Bounded immutable registry for native analytic instruments.
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
            if definition.descriptors().len() > MAX_PARAMETERS_PER_DEVICE {
                return Err(DeviceContractError::InvalidDescriptor);
            }
            for (parameter_index, descriptor) in definition.descriptors().iter().enumerate() {
                descriptor.validate_descriptor()?;
                if definition.descriptors()[..parameter_index]
                    .iter()
                    .any(|prior| prior.id == descriptor.id)
                {
                    return Err(DeviceContractError::InvalidDescriptor);
                }
            }
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
        self.validate_current(definition, definition.migrate_state(&state)?)
    }

    pub fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        self.validate_current(definition, definition.migrate_state(state)?)
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
        let current = self.validate_current(definition, definition.migrate_state(state)?)?;
        let descriptor = definition
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.id == parameter)
            .ok_or(DeviceContractError::UnknownParameter)?;
        descriptor.validate(value)?;
        let patched = definition.patch_parameter(&current, parameter, value)?;
        self.validate_current(definition, definition.migrate_state(&patched)?)
    }

    pub fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(definition, definition.migrate_state(state)?)?;
        definition.prepare_instrument(&current, sample_rate)
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
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterValue};

    struct TestToneDefinition {
        id: &'static str,
        sampler: fn(&[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], u64, u64, u64) -> StereoFrame,
        payload: [u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    }
    struct TestTonePrepared {
        sampler: fn(&[u64; ANALYTIC_NOTE_PAYLOAD_WORDS], u64, u64, u64) -> StereoFrame,
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
            sample_rate: u32,
        ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
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
            let instrument = registry.prepare_instrument(&state, 48_000).unwrap();
            let note = instrument.prepare_note(60, 100).unwrap();
            assert_eq!(note.sample(0, 32), expected);
            assert_eq!(note.sample(39, 32), expected);
            assert_eq!(note.sample(40, 32), [0.0, 0.0]);
        }
    }
}
