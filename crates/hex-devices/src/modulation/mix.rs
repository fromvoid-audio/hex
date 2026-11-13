//! Version 1 stores three little-endian f32 controls in A, B, Blend order.
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, NativeDeviceState, ParameterId,
};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.mix");
pub const INPUT_A: ParameterId = ParameterId::from_static(1);
pub const INPUT_B: ParameterId = ParameterId::from_static(2);
pub const BLEND: ParameterId = ParameterId::from_static(3);
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub inputs: [f32; 2],
    pub blend: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            inputs: [0.0, 1.0],
            blend: 0.5,
        }
    }
}

impl Settings {
    fn validated(self) -> Result<Self, Error> {
        if self.inputs.iter().all(|value| (-1.0..=1.0).contains(value))
            && (0.0..=1.0).contains(&self.blend)
        {
            Ok(self)
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        let Self {
            inputs: [a, b],
            blend,
        } = self.validated()?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(12)
            .map_err(|_| Error::RuntimeAllocation)?;
        for control in [a, b, blend] {
            payload.extend_from_slice(&control.to_le_bytes());
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
        {
            return Err(Error::MalformedState);
        }
        let [a0, a1, a2, a3, b0, b1, b2, b3, t0, t1, t2, t3] = *state.payload() else {
            return Err(Error::MalformedState);
        };
        Self {
            inputs: [
                f32::from_le_bytes([a0, a1, a2, a3]),
                f32::from_le_bytes([b0, b1, b2, b3]),
            ],
            blend: f32::from_le_bytes([t0, t1, t2, t3]),
        }
        .validated()
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        PreparedModulationSource::mix(id, sample_rate, self.inputs, self.blend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_endpoints_and_normalized_blend_round_trip() {
        let settings = Settings {
            inputs: [-0.25, 0.625],
            blend: 0.25,
        };
        let state = settings.encode().unwrap();
        assert_eq!(Settings::decode(&state), Ok(settings));
        assert_eq!(&state.payload()[8..], &0.25_f32.to_le_bytes());
        for bad in [f32::NAN, -0.01, 1.01] {
            assert_eq!(
                Settings {
                    blend: bad,
                    ..settings
                }
                .encode(),
                Err(Error::InvalidParameterValue)
            );
        }
        let invalid = NativeDeviceState::new(TYPE_ID, VERSION, vec![0; 11]).unwrap();
        assert_eq!(Settings::decode(&invalid), Err(Error::MalformedState));
    }
}
