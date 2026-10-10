//! Small validated primitives for memoryless stereo DSP.
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_TYPES, MAX_PARAMETERS_PER_DEVICE,
    NativeDeviceMetadata, NativeDeviceState,
};
use std::fmt;

/// A registered memoryless linear effect definition. Implementations own their
/// parameter codec in the contract trait and prepare a validated matrix here.
pub trait NativeDeviceDefinition: NativeDeviceMetadata {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError>;
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
            definition.type_id().validate()?;
            if definition.state_version() == 0 {
                return Err(DeviceContractError::InvalidStateVersion);
            }
            if definitions[..index]
                .iter()
                .any(|prior| prior.type_id() == definition.type_id())
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
    pub fn definition(&self, type_id: DeviceTypeId) -> Option<&'a dyn NativeDeviceDefinition> {
        self.definitions
            .iter()
            .copied()
            .find(|definition| definition.type_id() == type_id)
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
}
