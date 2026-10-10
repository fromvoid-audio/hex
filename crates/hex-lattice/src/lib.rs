//! Validated stereo DSP primitives and the registered native processor seam.
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_TYPES, MAX_PARAMETERS_PER_DEVICE,
    NativeDeviceMetadata, NativeDeviceState,
};
use std::any::Any as StdAny;
use std::fmt;

/// A registered memoryless linear effect definition. Implementations own their
/// parameter codec in the contract trait and prepare a validated matrix here.
pub trait NativeDeviceDefinition: NativeDeviceMetadata {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError>;

    /// Linear contribution for an admitted full device effect. Definitions
    /// with a stateful companion can override this while `prepare_matrix`
    /// continues to refuse matrix-only compilation of that state.
    fn prepare_effect_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.prepare_matrix(state)
    }

    /// Prepare an optional stateful processor using the active sample rate.
    /// Memoryless definitions keep the default `None` implementation.
    fn prepare_processor(
        &self,
        _state: &NativeDeviceState,
        _sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(None)
    }
}

mod instrument;
mod instrument_runtime;
pub use instrument::{
    ANALYTIC_NOTE_PAYLOAD_WORDS, NativeInstrumentDefinition, NativeInstrumentRegistry,
    PreparedAnalyticNote, PreparedNativeInstrument,
};
pub use instrument_runtime::{
    MutableInstrumentRuntime, NativeInstrumentEvent, NativeInstrumentRuntimeError,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, PreparedMutableInstrumentRuntime,
};

/// Immutable, control-prepared configuration for one stateful native effect.
/// Runtime construction happens before publication and is covered by the
/// definition's checked per-instance byte declaration.
pub trait PreparedNativeProcessor: Send + Sync {
    fn runtime_version(&self) -> u32;
    fn declared_runtime_bytes(&self) -> usize;
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime>;
}

/// Audio-owned state with allocation-free, infallible block processing and
/// history operations once the prepared configurations have been admitted.
pub trait NativeProcessorRuntime: StdAny + Send {
    fn process_block(&mut self, frames: &mut [StereoFrame]);
    /// Advance private state while preserving a bypassed signal.
    fn process_bypassed_block(&mut self, frames: &mut [StereoFrame]) {
        for frame in frames {
            let input = *frame;
            self.process_block(std::slice::from_mut(frame));
            *frame = input;
        }
    }
    fn reset(&mut self);
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool;
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime);
    fn as_any(&self) -> &dyn StdAny;
}

/// Maximum admitted stateful instances in one runtime bank.
pub const MAX_STATEFUL_PROCESSORS: usize = 256;
/// Maximum control-declared storage for one runtime twin.
pub const MAX_PROCESSOR_RUNTIME_BYTES: usize = 4_096;

/// One processor prepared for a stable track/device identity.
#[derive(Clone)]
pub struct PreparedProcessorInstance {
    pub track_id: u64,
    pub device_id: u64,
    pub device_type: DeviceTypeId,
    pub matrix: StereoMatrix,
    pub bypassed: bool,
    pub sample_rate: u32,
    pub definition: Option<std::sync::Arc<dyn PreparedNativeProcessor>>,
}

pub struct PreparedNativeDevice {
    pub matrix: StereoMatrix,
    pub processor: Option<Box<dyn PreparedNativeProcessor>>,
}
impl fmt::Debug for PreparedProcessorInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedProcessorInstance")
            .field("track_id", &self.track_id)
            .field("device_id", &self.device_id)
            .field("device_type", &self.device_type)
            .field("matrix", &self.matrix)
            .field("bypassed", &self.bypassed)
            .field("sample_rate", &self.sample_rate)
            .field("stateful", &self.definition.is_some())
            .finish()
    }
}

struct RuntimeEntry {
    track_id: u64,
    device_id: u64,
    device_type: DeviceTypeId,
    matrix: StereoMatrix,
    bypassed: bool,
    runtime_version: u32,
    sample_rate: u32,
    primary: Option<Box<dyn NativeProcessorRuntime>>,
    scratch: Option<Box<dyn NativeProcessorRuntime>>,
}

/// Paired audio-owned runtime state. `primary` is committed state; processing
/// operates on `scratch` and `commit_block` is called only after the entire
/// containing block succeeds. Construction and final destruction are control
/// side responsibilities.
pub struct NativeRuntimeChain {
    entries: Vec<RuntimeEntry>,
    stateful_count: usize,
    declared_runtime_bytes: usize,
}

impl NativeRuntimeChain {
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            stateful_count: 0,
            declared_runtime_bytes: 0,
        }
    }

    pub fn new(instances: Vec<PreparedProcessorInstance>) -> Result<Self, DeviceContractError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(instances.len())
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        let mut declared_bytes = 0_usize;
        let mut stateful_count = 0_usize;
        for instance in instances {
            if instance.track_id == 0 || instance.device_id == 0 || instance.sample_rate == 0 {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
            instance.device_type.validate()?;
            let (runtime_version, primary, scratch) = if let Some(definition) = &instance.definition
            {
                let bytes = definition.declared_runtime_bytes();
                if definition.runtime_version() == 0 || bytes > MAX_PROCESSOR_RUNTIME_BYTES {
                    return Err(DeviceContractError::RuntimeBudgetExceeded);
                }
                stateful_count = stateful_count
                    .checked_add(1)
                    .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                declared_bytes = declared_bytes
                    .checked_add(bytes)
                    .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
                if stateful_count > MAX_STATEFUL_PROCESSORS
                    || declared_bytes > MAX_STATEFUL_PROCESSORS * MAX_PROCESSOR_RUNTIME_BYTES
                {
                    return Err(DeviceContractError::RuntimeBudgetExceeded);
                }
                (
                    definition.runtime_version(),
                    Some(definition.create_runtime()),
                    Some(definition.create_runtime()),
                )
            } else {
                (0, None, None)
            };
            entries.push(RuntimeEntry {
                track_id: instance.track_id,
                device_id: instance.device_id,
                device_type: instance.device_type,
                matrix: instance.matrix,
                bypassed: instance.bypassed,
                runtime_version,
                sample_rate: instance.sample_rate,
                primary,
                scratch,
            });
        }
        Ok(Self {
            entries,
            stateful_count,
            declared_runtime_bytes: declared_bytes,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn has_stateful_processors(&self) -> bool {
        self.entries.iter().any(|entry| entry.primary.is_some())
    }

    pub const fn stateful_count(&self) -> usize {
        self.stateful_count
    }

    pub const fn declared_runtime_bytes(&self) -> usize {
        self.declared_runtime_bytes
    }

    /// Preflight every twin before copying any state. Matching identities whose
    /// runtime compatibility changed remain freshly reset on the candidate.
    pub fn can_inherit_from(&self, previous: &Self) -> bool {
        self.entries.iter().all(|entry| {
            let Some(new_runtime) = entry.primary.as_ref() else {
                return true;
            };
            let Some(old) = previous.entries.iter().find(|old| {
                old.track_id == entry.track_id
                    && old.device_id == entry.device_id
                    && old.device_type == entry.device_type
                    && old.runtime_version == entry.runtime_version
                    && old.sample_rate == entry.sample_rate
                    && old.primary.is_some()
            }) else {
                return true;
            };
            let Some(old_runtime) = old.primary.as_deref() else {
                return true;
            };
            new_runtime.can_copy_history_from(old_runtime)
                && entry
                    .scratch
                    .as_ref()
                    .is_some_and(|scratch| scratch.can_copy_history_from(old_runtime))
        })
    }

    /// Copy compatible histories to both twins after a successful preflight.
    /// Identity also includes the stable device type, runtime version and rate;
    /// persisted state versions may change while physical history is retained.
    pub fn inherit_from(&mut self, previous: &Self) {
        for entry in &mut self.entries {
            if let Some(old) = previous.entries.iter().find(|old| {
                old.track_id == entry.track_id
                    && old.device_id == entry.device_id
                    && old.device_type == entry.device_type
                    && old.runtime_version == entry.runtime_version
                    && old.sample_rate == entry.sample_rate
                    && old.primary.is_some()
            }) && let (Some(primary), Some(scratch), Some(old_primary)) = (
                entry.primary.as_mut(),
                entry.scratch.as_mut(),
                old.primary.as_deref(),
            ) && primary.can_copy_history_from(old_primary)
            {
                primary.copy_history_from(old_primary);
                scratch.copy_history_from(old_primary);
            }
        }
    }

    /// Start a transactional block by copying committed histories into twins.
    pub fn begin_block(&mut self) {
        for entry in &mut self.entries {
            if let (Some(primary), Some(scratch)) =
                (entry.primary.as_deref(), entry.scratch.as_mut())
            {
                if scratch.can_copy_history_from(primary) {
                    scratch.copy_history_from(primary);
                } else {
                    scratch.reset();
                }
            }
        }
    }

    /// Process the summed track signal through all registered stateful entries.
    pub fn process_block(&mut self, frames: &mut [StereoFrame]) {
        Self::process_entries(&mut self.entries, frames);
    }

    /// Process a control-prevalidated effect segment without ending the
    /// surrounding transactional block. The range indexes the prepared effect
    /// order; callers must begin once and commit only after the full render block.
    pub fn process_range(
        &mut self,
        range: std::ops::Range<usize>,
        frames: &mut [StereoFrame],
    ) -> Result<(), DeviceContractError> {
        let entries = self
            .entries
            .get_mut(range)
            .ok_or(DeviceContractError::InvalidDescriptor)?;
        Self::process_entries(entries, frames);
        Ok(())
    }

    fn process_entries(entries: &mut [RuntimeEntry], frames: &mut [StereoFrame]) {
        for entry in entries {
            if entry.bypassed {
                if let Some(runtime) = entry.scratch.as_mut() {
                    if entry.matrix.is_identity() {
                        runtime.process_bypassed_block(frames);
                    } else {
                        for frame in frames.iter_mut() {
                            let audible = *frame;
                            let mut internal = entry.matrix.apply(audible);
                            runtime.process_bypassed_block(std::slice::from_mut(&mut internal));
                            *frame = audible;
                        }
                    }
                }
            } else {
                entry.matrix.apply_block(frames);
                if let Some(runtime) = entry.scratch.as_mut() {
                    runtime.process_block(frames);
                }
            }
        }
    }

    /// Commit histories after all render chunks and source reads succeeded.
    pub fn commit_block(&mut self) {
        for entry in &mut self.entries {
            std::mem::swap(&mut entry.primary, &mut entry.scratch);
        }
    }

    /// Reset both committed and speculative state at a hard discontinuity.
    pub fn reset(&mut self) {
        for entry in &mut self.entries {
            if let Some(primary) = entry.primary.as_mut() {
                primary.reset();
            }
            if let Some(scratch) = entry.scratch.as_mut() {
                scratch.reset();
            }
        }
    }
}

/// Borrowed immutable device registry. Duplicate type IDs are rejected at
/// construction so resolution remains a bounded linear scan over built-ins.
pub struct NativeDeviceRegistry<'a> {
    definitions: &'a [&'a dyn NativeDeviceDefinition],
}
impl<'a> NativeDeviceRegistry<'a> {
    pub fn new(
        definitions: &'a [&'a dyn NativeDeviceDefinition],
    ) -> Result<Self, DeviceContractError> {
        if definitions.len() > MAX_NATIVE_DEVICE_TYPES {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        for (index, definition) in definitions.iter().enumerate() {
            NativeDeviceMetadata::type_id(*definition).validate()?;
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
            if definitions[..index].iter().any(|prior| {
                NativeDeviceMetadata::type_id(*prior) == NativeDeviceMetadata::type_id(*definition)
            }) {
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
    pub fn definition(&self, type_id: DeviceTypeId) -> Option<&'a dyn NativeDeviceDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| NativeDeviceMetadata::type_id(*definition) == type_id)
    }
    pub fn definitions(&self) -> &'a [&'a dyn NativeDeviceDefinition] {
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
    pub fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(definition, definition.migrate_state(state)?)?;
        definition.prepare_matrix(&current)
    }
    pub fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(definition, definition.migrate_state(state)?)?;
        definition.prepare_processor(&current, sample_rate)
    }
    pub fn prepare_device(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<PreparedNativeDevice, DeviceContractError> {
        let definition = self
            .definition(state.type_id())
            .ok_or(DeviceContractError::WrongDeviceType)?;
        let current = self.validate_current(definition, definition.migrate_state(state)?)?;
        Ok(PreparedNativeDevice {
            matrix: definition.prepare_effect_matrix(&current)?,
            processor: definition.prepare_processor(&current, sample_rate)?,
        })
    }
    fn validate_current(
        &self,
        definition: &'a dyn NativeDeviceDefinition,
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

/// One interleaved stereo frame represented as left and right samples.
pub type StereoFrame = [f32; 2];

/// A coefficient or composition failed finite single-precision validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMatrix(pub &'static str);

impl fmt::Display for InvalidMatrix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for InvalidMatrix {}

/// An immutable 2×2 transform whose coefficients are finite `f32` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoMatrix {
    coefficients: [[f32; 2]; 2],
}

impl StereoMatrix {
    /// Identity preserves each input sample bit-for-bit in [`Self::apply`].
    pub const fn identity() -> Self {
        Self {
            coefficients: [[1.0, 0.0], [0.0, 1.0]],
        }
    }

    /// Construct from output-row/input-column coefficients after finite checks.
    pub fn new(coefficients: [[f32; 2]; 2]) -> Result<Self, InvalidMatrix> {
        if coefficients
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(InvalidMatrix("stereo matrix coefficients must be finite"));
        }
        Ok(Self { coefficients })
    }

    /// Return the immutable output-row/input-column coefficient array.
    pub const fn coefficients(self) -> [[f32; 2]; 2] {
        self.coefficients
    }

    /// Report the fixed delay of this memoryless matrix transform.
    pub const fn latency_frames(self) -> u32 {
        0
    }

    /// Apply this matrix to one stereo frame.
    pub fn apply(self, input: StereoFrame) -> StereoFrame {
        if self.is_identity() {
            return input;
        }
        let matrix = self.coefficients;
        [
            matrix[0][0] * input[0] + matrix[0][1] * input[1],
            matrix[1][0] * input[0] + matrix[1][1] * input[1],
        ]
    }

    /// Apply this matrix in place to a caller-owned block, without allocation.
    pub fn apply_block(self, frames: &mut [StereoFrame]) {
        if self.is_identity() {
            return;
        }
        for frame in frames {
            *frame = self.apply(*frame);
        }
    }

    /// Compose in signal order: `first.then(next)` applies `first`, then `next`.
    /// The result is refused if any composed `f32` coefficient is non-finite.
    pub fn then(self, next: Self) -> Result<Self, InvalidMatrix> {
        if self.is_identity() {
            return Ok(next);
        }
        if next.is_identity() {
            return Ok(self);
        }
        let a = self.coefficients;
        let b = next.coefficients;
        let mut result = [[0.0; 2]; 2];
        for (row, output) in result.iter_mut().enumerate() {
            for (column, coefficient) in output.iter_mut().enumerate() {
                let value = f64::from(b[row][0]) * f64::from(a[0][column])
                    + f64::from(b[row][1]) * f64::from(a[1][column]);
                if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                    return Err(InvalidMatrix("stereo matrix composition overflowed"));
                }
                *coefficient = value as f32;
                if !coefficient.is_finite() {
                    return Err(InvalidMatrix("stereo matrix composition overflowed"));
                }
            }
        }
        Self::new(result)
    }

    fn is_identity(self) -> bool {
        self.coefficients == [[1.0, 0.0], [0.0, 1.0]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterValue};

    #[test]
    fn identity_is_a_zero_latency_bit_preserving_fast_path() {
        let identity = StereoMatrix::identity();
        assert_eq!(identity.latency_frames(), 0);
        let input = [f32::from_bits(0x8000_0000), f32::from_bits(0x7fc0_0123)];
        let output = identity.apply(input);
        assert_eq!(output[0].to_bits(), input[0].to_bits());
        assert_eq!(output[1].to_bits(), input[1].to_bits());
        let mut block = [input, [1.0, -1.0]];
        identity.apply_block(&mut block);
        assert_eq!(block[0][1].to_bits(), input[1].to_bits());
    }

    #[test]
    fn coefficients_are_finite_and_composition_overflow_is_refused() {
        assert!(StereoMatrix::new([[f32::NAN, 0.0], [0.0, 1.0]]).is_err());
        assert!(StereoMatrix::new([[f32::INFINITY, 0.0], [0.0, 1.0]]).is_err());
        let large = StereoMatrix::new([[f32::MAX, 0.0], [0.0, f32::MAX]]).unwrap();
        let scale = StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap();
        assert!(large.then(scale).is_err());
    }

    #[test]
    fn composition_preserves_serial_noncommutative_order() {
        let swap = StereoMatrix::new([[0.0, 1.0], [1.0, 0.0]]).unwrap();
        let left_gain = StereoMatrix::new([[2.0, 0.0], [0.0, 1.0]]).unwrap();
        let swapped_then_gain = swap.then(left_gain).unwrap();
        let gain_then_swap = left_gain.then(swap).unwrap();
        assert_eq!(swapped_then_gain.apply([1.0, 3.0]), [6.0, 1.0]);
        assert_eq!(gain_then_swap.apply([1.0, 3.0]), [3.0, 2.0]);
    }

    struct IndependentDefinition;
    struct IndependentPrepared(u32);
    #[derive(Default)]
    struct IndependentRuntime {
        history: f32,
    }

    impl NativeDeviceMetadata for IndependentDefinition {
        fn type_id(&self) -> DeviceTypeId {
            DeviceTypeId::from_static("test.independent")
        }
        fn state_version(&self) -> u32 {
            1
        }
        fn descriptors(&self) -> &'static [ParameterDescriptor] {
            &[]
        }
        fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
            NativeDeviceState::new(NativeDeviceMetadata::type_id(self), 1, vec![1])
        }
        fn migrate_state(
            &self,
            state: &NativeDeviceState,
        ) -> Result<NativeDeviceState, DeviceContractError> {
            if state.type_id() != NativeDeviceMetadata::type_id(self)
                || state.state_version() != 1
                || state.payload().len() != 1
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
    impl NativeDeviceDefinition for IndependentDefinition {
        fn prepare_matrix(
            &self,
            _state: &NativeDeviceState,
        ) -> Result<StereoMatrix, DeviceContractError> {
            Ok(StereoMatrix::identity())
        }
        fn prepare_processor(
            &self,
            state: &NativeDeviceState,
            _sample_rate: u32,
        ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
            if state.payload() != [1] {
                return Err(DeviceContractError::MalformedState);
            }
            Ok(Some(Box::new(IndependentPrepared(1))))
        }
    }
    impl PreparedNativeProcessor for IndependentPrepared {
        fn runtime_version(&self) -> u32 {
            self.0
        }
        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<IndependentRuntime>()
        }
        fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
            Box::new(IndependentRuntime::default())
        }
    }
    impl NativeProcessorRuntime for IndependentRuntime {
        fn process_block(&mut self, frames: &mut [StereoFrame]) {
            for frame in frames {
                frame[0] += self.history;
                frame[1] += self.history;
                self.history = frame[0];
            }
        }
        fn reset(&mut self) {
            self.history = 0.0;
        }
        fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }
        fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
                self.history = previous.history;
            }
        }
        fn as_any(&self) -> &dyn StdAny {
            self
        }
    }

    #[test]
    fn independently_registered_stateful_definition_uses_paired_transactional_runtime() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                48_000,
            )
            .unwrap()
            .unwrap();
        let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            track_id: 3,
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            definition: Some(std::sync::Arc::from(prepared)),
        }])
        .unwrap();
        let mut committed = [[1.0, 1.0]];
        chain.begin_block();
        chain.process_block(&mut committed);
        chain.commit_block();

        chain.begin_block();
        let mut failed_block = [[2.0, 2.0]];
        chain.process_block(&mut failed_block);
        // No commit models a source/render failure after this processor ran.
        chain.begin_block();
        let mut retry = [[2.0, 2.0]];
        chain.process_block(&mut retry);
        chain.commit_block();
        assert_eq!(retry, [[3.0, 3.0]]);

        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                48_000,
            )
            .unwrap()
            .unwrap();
        let mut successor = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            track_id: 3,
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            definition: Some(std::sync::Arc::from(prepared)),
        }])
        .unwrap();
        assert!(successor.can_inherit_from(&chain));
        successor.inherit_from(&chain);
        successor.begin_block();
        let mut inherited = [[1.0, 1.0]];
        successor.process_block(&mut inherited);
        assert_eq!(inherited, [[4.0, 4.0]]);

        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                44_100,
            )
            .unwrap()
            .unwrap();
        let mut incompatible_rate = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            track_id: 3,
            device_id: 9,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 44_100,
            definition: Some(std::sync::Arc::from(prepared)),
        }])
        .unwrap();
        assert!(incompatible_rate.can_inherit_from(&chain));
        incompatible_rate.inherit_from(&chain);
        incompatible_rate.begin_block();
        let mut reset_history = [[1.0, 1.0]];
        incompatible_rate.process_block(&mut reset_history);
        assert_eq!(reset_history, [[1.0, 1.0]]);
    }

    #[test]
    fn segmented_processing_shares_one_transaction_and_rolls_back_all_ranges() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                48_000,
            )
            .unwrap()
            .unwrap();
        let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
        let make_instance = |device_id| PreparedProcessorInstance {
            track_id: 3,
            device_id,
            device_type: DeviceTypeId::from_static("test.independent"),
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48_000,
            definition: Some(std::sync::Arc::clone(&prepared)),
        };
        let mut chain = NativeRuntimeChain::new(vec![make_instance(9), make_instance(10)]).unwrap();

        chain.begin_block();
        let mut first = [[1.0, 1.0]];
        chain.process_range(0..1, &mut first).unwrap();
        chain.process_range(1..2, &mut first).unwrap();
        chain.commit_block();

        chain.begin_block();
        let mut speculative = [[2.0, 2.0]];
        chain.process_range(0..1, &mut speculative).unwrap();
        chain.process_range(1..2, &mut speculative).unwrap();
        // A later render-stage failure leaves both segment histories uncommitted.
        chain.begin_block();
        let mut retry = [[2.0, 2.0]];
        chain.process_range(0..1, &mut retry).unwrap();
        chain.process_range(1..2, &mut retry).unwrap();
        chain.commit_block();
        assert_eq!(retry, [[4.0, 4.0]]);

        chain.begin_block();
        let mut next = [[3.0, 3.0]];
        chain.process_range(0..1, &mut next).unwrap();
        chain.process_range(1..2, &mut next).unwrap();
        chain.commit_block();
        assert_eq!(next, [[10.0, 10.0]]);

        chain.begin_block();
        let mut untouched = [[7.0, 7.0]];
        assert_eq!(
            chain.process_range(0..3, &mut untouched),
            Err(DeviceContractError::InvalidDescriptor)
        );
        assert_eq!(untouched, [[7.0, 7.0]]);
    }

    #[test]
    fn bypass_advances_history_after_matrix_without_changing_dry_pcm() {
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&IndependentDefinition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = registry
            .prepare_processor(
                &registry
                    .default_state(DeviceTypeId::from_static("test.independent"))
                    .unwrap(),
                48_000,
            )
            .unwrap()
            .unwrap();
        let prepared: std::sync::Arc<dyn PreparedNativeProcessor> = std::sync::Arc::from(prepared);
        let matrix = StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap();
        let make_chain = |bypassed| {
            NativeRuntimeChain::new(vec![PreparedProcessorInstance {
                track_id: 3,
                device_id: 9,
                device_type: DeviceTypeId::from_static("test.independent"),
                matrix,
                bypassed,
                sample_rate: 48_000,
                definition: Some(std::sync::Arc::clone(&prepared)),
            }])
            .unwrap()
        };
        let mut bypassed = make_chain(true);
        let mut active_reference = make_chain(false);
        let warmup = [[0.25, -0.125], [0.5, 0.75], [-0.25, 0.375]];
        let mut dry = warmup;

        bypassed.begin_block();
        // Different chunking must not change the state advanced while bypassed.
        bypassed.process_range(0..1, &mut dry[..1]).unwrap();
        bypassed.process_block(&mut dry[1..]);
        bypassed.commit_block();

        active_reference.begin_block();
        let mut audible_reference = warmup;
        active_reference.process_block(&mut audible_reference);
        active_reference.commit_block();

        for (actual, original) in dry.iter().zip(warmup) {
            assert_eq!(
                [actual[0].to_bits(), actual[1].to_bits()],
                [original[0].to_bits(), original[1].to_bits()],
                "bypass must preserve the original samples exactly"
            );
        }

        // Re-enable both processors and compare their output. Their hidden
        // histories must match the active reference despite bypass chunking.
        bypassed.entries[0].bypassed = false;
        let resumed = [[0.125, -0.125], [0.0, 0.25]];
        let mut resumed_bypassed = resumed;
        let mut resumed_reference = resumed;
        bypassed.begin_block();
        bypassed
            .process_range(0..1, &mut resumed_bypassed[..1])
            .unwrap();
        bypassed.process_block(&mut resumed_bypassed[1..]);
        bypassed.commit_block();
        active_reference.begin_block();
        active_reference.process_block(&mut resumed_reference);
        active_reference.commit_block();

        assert_eq!(resumed_bypassed, resumed_reference);
    }
}
