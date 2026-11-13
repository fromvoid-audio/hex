//! Four independent normalized values. Version 1 stores four little-endian
//! f32 values in control order. A control and its corresponding output share
//! the same stable numeric identity.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, ModulationOutputId, NativeDeviceState, ParameterId,
};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.macro-four");
pub const VALUE_PARAMETERS: [ParameterId; 4] = [
    ParameterId::from_static(1),
    ParameterId::from_static(2),
    ParameterId::from_static(3),
    ParameterId::from_static(4),
];
const VERSION: u32 = 1;
const PAYLOAD_BYTES: usize = 4 * VALUE_PARAMETERS.len();

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub values: [f32; VALUE_PARAMETERS.len()],
}

pub fn output_for_parameter(parameter: ParameterId) -> Option<ModulationOutputId> {
    VALUE_PARAMETERS
        .contains(&parameter)
        .then(|| ModulationOutputId::new(parameter.value()).ok())
        .flatten()
}

impl Settings {
    fn validate(self) -> Result<Self, Error> {
        if self.values.iter().all(|value| (0.0..=1.0).contains(value)) {
            Ok(self)
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(PAYLOAD_BYTES)
            .map_err(|_| Error::RuntimeAllocation)?;
        for value in self.values {
            payload.extend_from_slice(&value.to_le_bytes());
        }
        NativeDeviceState::new(TYPE_ID, VERSION, payload)
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, Error> {
        if state.type_id() != TYPE_ID {
            return Err(Error::WrongDeviceType);
        }
        state.require_asset_free_processing()?;
        if state.state_version() != VERSION
            || state.input_gain_millidb() != 0
            || state.output_gain_millidb() != 0
            || state.payload().len() != PAYLOAD_BYTES
        {
            return Err(Error::MalformedState);
        }
        let mut settings = Self::default();
        for (value, bytes) in settings
            .values
            .iter_mut()
            .zip(state.payload().chunks_exact(4))
        {
            *value = f32::from_le_bytes(bytes.try_into().map_err(|_| Error::MalformedState)?);
        }
        settings.validate()
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
        output: ModulationOutputId,
    ) -> Result<PreparedModulationSource, Error> {
        let index = (output.value() - 1) as usize;
        let value = *self.values.get(index).ok_or(Error::InvalidId)?;
        Ok(PreparedModulationSource::macro_value(id, sample_rate, value)?.with_output(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_in_output_order_and_invalid_states_refuse() {
        let settings = Settings {
            values: [0.0, 1.0, 0.25, 0.75],
        };
        assert_eq!(Settings::decode(&settings.encode().unwrap()), Ok(settings));
        assert_eq!(
            Settings::decode(&NativeDeviceState::new(TYPE_ID, VERSION, vec![0; 15]).unwrap()),
            Err(Error::MalformedState),
        );
        assert!(
            Settings {
                values: [0.0, f32::NAN, 0.0, 1.0]
            }
            .encode()
            .is_err()
        );
        assert_eq!(
            settings
                .prepare(
                    ModulationSourceId::new(1).unwrap(),
                    48_000,
                    ModulationOutputId::new(5).unwrap()
                )
                .unwrap_err(),
            Error::InvalidId,
        );
    }
}
