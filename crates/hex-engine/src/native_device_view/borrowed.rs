//! Finite built-in, current-format metadata queries on the actual borrowed state.
//! No registry, migration, default, owning view or descriptor Vec is created.
use super::*;
use hex_lattice_contract::{ParameterAccess, ParameterId};

/// A scoped control read, never a device-state owner.
/// ```compile_fail
/// fn escape(state: &hex_lattice_contract::NativeDeviceState) -> hex_engine::NativeDeviceRead<'static> {
///     hex_engine::NativeDeviceRead::on_control(hex_engine::NativeDeviceClass::Effect, state).unwrap()
/// }
/// ```
#[derive(Clone, Copy)]
pub struct NativeDeviceRead<'a> {
    state: &'a NativeDeviceState,
    metadata: &'static dyn NativeDeviceMetadata,
    eq: Option<hex_devices::eq::EqStateRead<'a>>,
}

impl<'a> NativeDeviceRead<'a> {
    pub fn on_control(
        class: NativeDeviceClass,
        state: &'a NativeDeviceState,
    ) -> Result<Self, Invalid> {
        // Adding a registry entry does not implicitly qualify its read methods.
        if !matches!(
            state.type_id(),
            hex_devices::utility::UTILITY_TYPE_ID
                | hex_devices::eq8::EQ8_TYPE_ID
                | hex_devices::eq::EQ_TYPE_ID
                | hex_devices::saturator::SATURATOR_TYPE_ID
                | hex_devices::compressor::COMPRESSOR_TYPE_ID
                | hex_devices::gate::GATE_TYPE_ID
                | hex_devices::auto_pan::AUTO_PAN_TYPE_ID
                | hex_devices::fm4::device::FM4_TYPE_ID
                | hex_devices::operator_state::OPERATOR_TYPE_ID
                | hex_devices::SINE_INSTRUMENT_TYPE_ID
                | hex_devices::sampler::state::TYPE_ID
        ) {
            return Err(Invalid("native borrowed codec is unavailable"));
        }
        let metadata = match class {
            NativeDeviceClass::Effect => hex_devices::native_effect_metadata(state.type_id()),
            NativeDeviceClass::Instrument
                if state.type_id() == hex_devices::sampler::state::TYPE_ID =>
            {
                Some(&hex_devices::sampler::metadata::METADATA as &dyn NativeDeviceMetadata)
            }
            NativeDeviceClass::Instrument => {
                hex_devices::native_instrument_metadata(state.type_id())
            }
        }
        .ok_or(Invalid("native device type is unavailable"))?;
        if state.state_version() != metadata.state_version() {
            return Err(Invalid("native device state version is unsupported"));
        }
        let eq = if state.type_id() == hex_devices::eq::EQ_TYPE_ID {
            Some(
                hex_devices::eq::EqStateRead::from_native(state)
                    .map_err(|_| Invalid("native eq state is invalid"))?,
            )
        } else {
            None
        };
        if state.type_id() == hex_devices::SINE_INSTRUMENT_TYPE_ID && !state.payload().is_empty() {
            return Err(Invalid("native sine state is invalid"));
        }
        let result = Self {
            state,
            metadata,
            eq,
        };
        // Validate the entire actual current payload, including compound codecs,
        // before exposing even one display fact. Empty Sampler is still decoded.
        if state.type_id() == hex_devices::sampler::state::TYPE_ID {
            hex_devices::sampler::state::DeviceStateRead::from_native(state)
                .map_err(|_| Invalid("native sampler state is invalid"))?;
        }
        DEVICE_INPUT_GAIN
            .validate(ParameterValue::Float(state.input_gain_db()))
            .map_err(|_| Invalid("native input gain is invalid"))?;
        DEVICE_OUTPUT_GAIN
            .validate(ParameterValue::Float(state.output_gain_db()))
            .map_err(|_| Invalid("native output gain is invalid"))?;
        for index in 0..result.parameter_count() {
            let _ = result.parameter_at(index)?;
        }
        Ok(result)
    }

    pub fn type_id(self) -> DeviceTypeId {
        self.state.type_id()
    }
    pub fn saturator_transfer_curve(
        self,
        overrides: &[(ParameterId, ParameterValue)],
    ) -> Result<hex_devices::saturator::SaturatorTransferCurve, Invalid> {
        hex_devices::saturator::transfer_curve_with_overrides(self.state, overrides)
            .map_err(|_| Invalid("native saturator curve is invalid"))
    }
    pub fn compressor_transfer_curve(
        self,
        overrides: &[(ParameterId, ParameterValue)],
    ) -> Result<hex_devices::compressor::CompressorTransferCurve, Invalid> {
        hex_devices::compressor::transfer_curve_with_overrides(self.state, overrides)
            .map_err(|_| Invalid("native compressor curve is invalid"))
    }
    pub fn name(self) -> &'static str {
        self.metadata.display_name().unwrap_or("native device")
    }
    pub fn parameter_count(self) -> usize {
        self.eq.map_or_else(
            || self.metadata.descriptors().len(),
            |eq| 2 + eq.node_count() * 7,
        )
    }
    fn descriptor_at(self, index: usize) -> Result<ParameterDescriptor, Invalid> {
        if let Some(eq) = self.eq {
            if index >= 2 {
                let node = eq
                    .node_at((index - 2) / 7)
                    .map_err(|_| Invalid("native parameter is unavailable"))?;
                let field = hex_devices::eq::NodeParameter::ALL[(index - 2) % 7];
                return hex_devices::eq::descriptor(node.id, field)
                    .map_err(|_| Invalid("native parameter descriptor is invalid"));
            }
        }
        self.metadata
            .descriptors()
            .get(index)
            .copied()
            .ok_or(Invalid("native parameter is unavailable"))
    }
    pub fn parameter_at(self, index: usize) -> Result<NativeDeviceParameterView, Invalid> {
        self.read_descriptor(self.descriptor_at(index)?)
    }
    pub fn parameter(self, id: ParameterId) -> Result<NativeDeviceParameterView, Invalid> {
        if id == DEVICE_INPUT_GAIN.id || id == DEVICE_OUTPUT_GAIN.id {
            let (descriptor, value) = if id == DEVICE_INPUT_GAIN.id {
                (DEVICE_INPUT_GAIN, self.state.input_gain_db())
            } else {
                (DEVICE_OUTPUT_GAIN, self.state.output_gain_db())
            };
            return Ok(NativeDeviceParameterView {
                descriptor,
                value: ParameterValue::Float(value),
                semantics: None,
                enabled: true,
            });
        }
        for index in 0..self.parameter_count() {
            let descriptor = self.descriptor_at(index)?;
            if descriptor.id == id {
                return self.read_descriptor(descriptor);
            }
        }
        Err(Invalid("native parameter is unavailable"))
    }
    fn read_descriptor(
        self,
        descriptor: ParameterDescriptor,
    ) -> Result<NativeDeviceParameterView, Invalid> {
        let value = self
            .metadata
            .read_parameter(self.state, descriptor.id)
            .map_err(|_| Invalid("native device parameter is invalid"))?;
        descriptor
            .validate(value)
            .map_err(|_| Invalid("native device parameter is invalid"))?;
        let semantics = self
            .metadata
            .parameter_semantics_for_state(self.state, descriptor.id)
            .map_err(|_| Invalid("native parameter semantics are invalid"))?;
        if let Some(semantics) = semantics {
            semantics
                .validate(descriptor)
                .map_err(|_| Invalid("native parameter semantics are invalid"))?;
        }
        // These exact built-ins override the allocating default availability
        // implementation. All other finite built-ins have unconditional controls;
        // their entire current payload was validated above by their borrowed codec.
        let conditional = matches!(
            self.state.type_id(),
            hex_devices::utility::UTILITY_TYPE_ID
                | hex_devices::eq8::EQ8_TYPE_ID
                | hex_devices::eq::EQ_TYPE_ID
                | hex_devices::operator_state::OPERATOR_TYPE_ID
                | hex_devices::sampler::state::TYPE_ID
        );
        let enabled = (!conditional
            || self
                .metadata
                .parameter_enabled(self.state, descriptor.id)
                .map_err(|_| Invalid("native parameter availability is invalid"))?)
            && !semantics.is_some_and(|s| s.access == ParameterAccess::ReadOnly);
        Ok(NativeDeviceParameterView {
            descriptor,
            semantics,
            value,
            enabled,
        })
    }
}

#[cfg(test)]
#[path = "borrowed_tests.rs"]
mod tests;
