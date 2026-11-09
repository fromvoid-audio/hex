//! Version 1 stores one normalized f32 macro value in little-endian bytes.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{DeviceContractError as Error, DeviceTypeId, NativeDeviceState};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.macro");
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub value: f32,
}

impl Settings {
    fn validate(self) -> Result<(), Error> {
        if (0.0..=1.0).contains(&self.value) {
            Ok(())
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validate()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(4)
            .map_err(|_| Error::RuntimeAllocation)?;
        payload.extend_from_slice(&self.value.to_le_bytes());
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
        {
            return Err(Error::MalformedState);
        }
        let bytes = state
            .payload()
            .try_into()
            .map_err(|_| Error::MalformedState)?;
        let settings = Self {
            value: f32::from_le_bytes(bytes),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        PreparedModulationSource::macro_value(id, sample_rate, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_codec_preserves_value_and_refuses_invalid_payloads() {
        let settings = Settings { value: 0.625 };
        assert_eq!(
            Settings::decode(&settings.encode().unwrap()).unwrap(),
            settings
        );
        assert!(Settings { value: f32::NAN }.encode().is_err());
        assert!(Settings { value: 1.25 }.encode().is_err());
        assert!(
            Settings::decode(&NativeDeviceState::new(TYPE_ID, VERSION, vec![0; 3]).unwrap())
                .is_err()
        );
    }
}
