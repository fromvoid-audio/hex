//! Version 1 saves a one-byte operation tag and two signed little-endian f32
//! controls. Their stable parameter IDs do not depend on the selected operation.
pub use hex_lattice::ModulationMathOperation as Operation;
use hex_lattice::{ModulationSourceId, PreparedModulationSource};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, NativeDeviceState, ParameterId,
};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.modulator.math");
pub const INPUT_A: ParameterId = ParameterId::from_static(1);
pub const INPUT_B: ParameterId = ParameterId::from_static(2);
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub operation: Operation,
    pub inputs: [f32; 2],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            operation: Operation::Multiply,
            inputs: [0.0, 1.0],
        }
    }
}

impl Settings {
    fn validated(self) -> Result<Self, Error> {
        if self.inputs.iter().all(|value| (-1.0..=1.0).contains(value)) {
            Ok(self)
        } else {
            Err(Error::InvalidParameterValue)
        }
    }

    pub fn encode(self) -> Result<NativeDeviceState, Error> {
        self.validated()?;
        let tag = match self.operation {
            Operation::Multiply => 0,
            Operation::Add => 1,
            Operation::Subtract => 2,
            Operation::Minimum => 3,
            Operation::Maximum => 4,
        };
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(9)
            .map_err(|_| Error::RuntimeAllocation)?;
        payload.push(tag);
        for input in self.inputs {
            payload.extend_from_slice(&input.to_le_bytes());
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
        let [tag, a0, a1, a2, a3, b0, b1, b2, b3] = *state.payload() else {
            return Err(Error::MalformedState);
        };
        let operation = match tag {
            0 => Operation::Multiply,
            1 => Operation::Add,
            2 => Operation::Subtract,
            3 => Operation::Minimum,
            4 => Operation::Maximum,
            _ => return Err(Error::MalformedState),
        };
        Self {
            operation,
            inputs: [
                f32::from_le_bytes([a0, a1, a2, a3]),
                f32::from_le_bytes([b0, b1, b2, b3]),
            ],
        }
        .validated()
    }

    pub fn prepare(
        self,
        id: ModulationSourceId,
        sample_rate: u32,
    ) -> Result<PreparedModulationSource, Error> {
        PreparedModulationSource::math(id, sample_rate, self.inputs, self.operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_operations_and_signed_controls_round_trip_with_stable_tags() {
        for (tag, operation) in [
            Operation::Multiply,
            Operation::Add,
            Operation::Subtract,
            Operation::Minimum,
            Operation::Maximum,
        ]
        .into_iter()
        .enumerate()
        {
            let settings = Settings {
                operation,
                inputs: [-0.25, 0.625],
            };
            let state = settings.encode().unwrap();
            assert_eq!(state.payload()[0], tag as u8);
            assert_eq!(Settings::decode(&state), Ok(settings));
        }
        assert!(
            Settings {
                inputs: [0.0, f32::NAN],
                ..Settings::default()
            }
            .encode()
            .is_err()
        );
        for payload in [vec![0; 8], vec![5; 9]] {
            let invalid = NativeDeviceState::new(TYPE_ID, VERSION, payload).unwrap();
            assert_eq!(Settings::decode(&invalid), Err(Error::MalformedState));
        }
    }
}
